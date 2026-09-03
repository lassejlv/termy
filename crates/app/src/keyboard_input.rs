use engine::{
    Key as TerminalKey, KeyEvent as TerminalKeyEvent, KeyEventKind, KeypadKey, MediaKey,
    ModifierKey, Modifiers,
};
use winit::{
    event::{ElementState, KeyEvent, Modifiers as WinitModifiers},
    keyboard::{
        Key, KeyCode, KeyLocation, ModifiersKeyState, ModifiersState, NamedKey, PhysicalKey,
    },
};

#[cfg(target_os = "macos")]
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct KeyboardState {
    held_modifiers: u16,
    // Winit does not expose lock bits. These toggles are intentionally best-effort: they become
    // accurate after Tmon observes a local lock-key press, but cannot discover an initial or
    // externally changed lock state on platforms that do not deliver that key.
    caps_lock: bool,
    num_lock: bool,
}

impl KeyboardState {
    pub(crate) fn modifiers_changed(&mut self, modifiers: WinitModifiers) {
        self.sync_modifier_pair(
            ModifierKey::LeftShift,
            ModifierKey::RightShift,
            modifiers.state().shift_key(),
            modifiers.lshift_state(),
            modifiers.rshift_state(),
        );
        self.sync_modifier_pair(
            ModifierKey::LeftControl,
            ModifierKey::RightControl,
            modifiers.state().control_key(),
            modifiers.lcontrol_state(),
            modifiers.rcontrol_state(),
        );
        self.sync_modifier_pair(
            ModifierKey::LeftAlt,
            ModifierKey::RightAlt,
            modifiers.state().alt_key(),
            modifiers.lalt_state(),
            modifiers.ralt_state(),
        );
        self.sync_modifier_pair(
            ModifierKey::LeftSuper,
            ModifierKey::RightSuper,
            modifiers.state().super_key(),
            modifiers.lsuper_state(),
            modifiers.rsuper_state(),
        );
    }

    pub(crate) fn clear_held_modifiers(&mut self) {
        self.held_modifiers = 0;
    }

    fn sync_modifier_pair(
        &mut self,
        left: ModifierKey,
        right: ModifierKey,
        active: bool,
        left_state: ModifiersKeyState,
        right_state: ModifiersKeyState,
    ) {
        if !active {
            self.set_held(left, false);
            self.set_held(right, false);
            return;
        }
        if left_state == ModifiersKeyState::Pressed || right_state == ModifiersKeyState::Pressed {
            self.set_held(left, left_state == ModifiersKeyState::Pressed);
            self.set_held(right, right_state == ModifiersKeyState::Pressed);
        }
    }

    fn set_held(&mut self, key: ModifierKey, held: bool) {
        let bit = modifier_key_bit(key);
        if held {
            self.held_modifiers |= bit;
        } else {
            self.held_modifiers &= !bit;
        }
    }

    fn is_held(self, key: ModifierKey) -> bool {
        self.held_modifiers & modifier_key_bit(key) != 0
    }

    fn update_for_event(&mut self, key: &TerminalKey, state: ElementState, repeat: bool) {
        if let TerminalKey::Modifier(modifier) = key {
            self.set_held(*modifier, state.is_pressed());
        }
        if state.is_pressed() && !repeat {
            match key {
                TerminalKey::CapsLock => self.caps_lock = !self.caps_lock,
                TerminalKey::NumLock => self.num_lock = !self.num_lock,
                _ => {}
            }
        }
    }

    fn apply_to_modifiers(self, key: &TerminalKey, modifiers: &mut Modifiers) {
        modifiers.set(Modifiers::HYPER, self.any_held(HYPER_KEYS));
        modifiers.set(Modifiers::META, self.any_held(META_KEYS));
        if let TerminalKey::Modifier(modifier) = key
            && let Some((terminal_modifier, keys)) = modifier_group(*modifier)
        {
            modifiers.set(terminal_modifier, self.any_held(keys));
        }
        modifiers.set(Modifiers::CAPS_LOCK, self.caps_lock);
        modifiers.set(Modifiers::NUM_LOCK, self.num_lock);
    }

    fn any_held(self, keys: &[ModifierKey]) -> bool {
        keys.iter().any(|key| self.is_held(*key))
    }
}

pub(crate) fn map_key(
    event: &KeyEvent,
    modifiers: WinitModifiers,
    keyboard_state: &mut KeyboardState,
) -> Option<TerminalKeyEvent> {
    let key_without_modifiers = key_without_modifiers(event);
    let logical_key = event.logical_key.as_ref();
    let text = produced_text(event, modifiers.state());
    let alt_is_terminal =
        terminal_alt_for_key(modifiers, &logical_key, &key_without_modifiers.as_ref());
    map_key_parts(
        event.physical_key,
        event.location,
        &logical_key,
        &key_without_modifiers.as_ref(),
        text,
        modifiers.state(),
        alt_is_terminal,
        event.state,
        event.repeat,
        keyboard_state,
    )
}

fn key_without_modifiers(event: &KeyEvent) -> Key {
    #[cfg(target_os = "macos")]
    {
        event.key_without_modifiers()
    }
    #[cfg(not(target_os = "macos"))]
    {
        event.logical_key.clone()
    }
}

fn produced_text(event: &KeyEvent, modifiers: ModifiersState) -> Option<&str> {
    #[cfg(target_os = "macos")]
    if event.state.is_pressed()
        && modifiers.alt_key()
        && !modifiers.control_key()
        && !modifiers.super_key()
        && is_text_key(&event.logical_key.as_ref())
    {
        return event
            .text_with_all_modifiers()
            .or(event.text.as_deref())
            .filter(|text| !text.is_empty());
    }

    event.text.as_deref().filter(|text| !text.is_empty())
}

#[allow(clippy::too_many_arguments)]
fn map_key_parts(
    physical_key: PhysicalKey,
    location: KeyLocation,
    logical_key: &Key<&str>,
    key_without_modifiers: &Key<&str>,
    text: Option<&str>,
    modifier_state: ModifiersState,
    alt_is_terminal: bool,
    state: ElementState,
    repeat: bool,
    keyboard_state: &mut KeyboardState,
) -> Option<TerminalKeyEvent> {
    // A dead key is only an in-progress composition. Winit delivers the resulting text through
    // `Ime::Commit`, so emitting the accent here would duplicate or corrupt the composition.
    if matches!(logical_key, Key::Dead(_)) {
        return None;
    }

    let mut modifiers = map_modifier_state(modifier_state, alt_is_terminal);
    let key = if let Some(standalone) = standalone_key(physical_key, location, logical_key) {
        standalone
    } else if let Some(keypad) = keypad_key(physical_key, location, logical_key) {
        TerminalKey::Keypad(keypad)
    } else {
        match logical_key {
            Key::Character(logical_text) => {
                let identity = match key_without_modifiers {
                    Key::Character(unmodified_text) => unmodified_text,
                    _ => logical_text,
                };
                TerminalKey::Character(identity.chars().next()?)
            }
            Key::Named(NamedKey::Space) => TerminalKey::Character(' '),
            Key::Named(NamedKey::Escape) => TerminalKey::Escape,
            Key::Named(NamedKey::Enter) => TerminalKey::Enter,
            Key::Named(NamedKey::Tab) if modifiers.contains(Modifiers::SHIFT) => {
                TerminalKey::Backtab
            }
            Key::Named(NamedKey::Tab) => TerminalKey::Tab,
            Key::Named(NamedKey::Backspace) => TerminalKey::Backspace,
            Key::Named(NamedKey::Insert) => TerminalKey::Insert,
            Key::Named(NamedKey::Delete) => TerminalKey::Delete,
            Key::Named(NamedKey::ArrowUp) => TerminalKey::Up,
            Key::Named(NamedKey::ArrowDown) => TerminalKey::Down,
            Key::Named(NamedKey::ArrowLeft) => TerminalKey::Left,
            Key::Named(NamedKey::ArrowRight) => TerminalKey::Right,
            Key::Named(NamedKey::PageUp) => TerminalKey::PageUp,
            Key::Named(NamedKey::PageDown) => TerminalKey::PageDown,
            Key::Named(NamedKey::Home) => TerminalKey::Home,
            Key::Named(NamedKey::End) => TerminalKey::End,
            Key::Named(named) => TerminalKey::Function(function_number(*named)?),
            Key::Dead(_) | Key::Unidentified(_) => return None,
        }
    };
    keyboard_state.update_for_event(&key, state, repeat);
    keyboard_state.apply_to_modifiers(&key, &mut modifiers);
    let shifted_key = shifted_key_code(logical_key, &key, modifiers);

    Some(TerminalKeyEvent {
        key,
        text: text.map(str::to_owned),
        shifted_key,
        base_layout: pc101_base_layout(physical_key),
        modifiers,
        kind: match (state, repeat) {
            (ElementState::Released, _) => KeyEventKind::Release,
            (ElementState::Pressed, true) => KeyEventKind::Repeat,
            (ElementState::Pressed, false) => KeyEventKind::Press,
        },
    })
}

fn shifted_key_code(
    logical_key: &Key<&str>,
    terminal_key: &TerminalKey,
    modifiers: Modifiers,
) -> Option<u32> {
    if !modifiers.contains(Modifiers::SHIFT) {
        return None;
    }
    let (Key::Character(shifted), TerminalKey::Character(unshifted)) = (logical_key, terminal_key)
    else {
        return None;
    };
    shifted
        .chars()
        .find(|character| !character.is_control())
        .filter(|shifted| shifted != unshifted)
        .map(u32::from)
}

const SHIFT_KEYS: &[ModifierKey] = &[ModifierKey::LeftShift, ModifierKey::RightShift];
const CONTROL_KEYS: &[ModifierKey] = &[ModifierKey::LeftControl, ModifierKey::RightControl];
const ALT_KEYS: &[ModifierKey] = &[ModifierKey::LeftAlt, ModifierKey::RightAlt];
const SUPER_KEYS: &[ModifierKey] = &[ModifierKey::LeftSuper, ModifierKey::RightSuper];
const HYPER_KEYS: &[ModifierKey] = &[ModifierKey::LeftHyper, ModifierKey::RightHyper];
const META_KEYS: &[ModifierKey] = &[ModifierKey::LeftMeta, ModifierKey::RightMeta];

const fn modifier_key_bit(key: ModifierKey) -> u16 {
    1 << match key {
        ModifierKey::LeftShift => 0,
        ModifierKey::LeftControl => 1,
        ModifierKey::LeftAlt => 2,
        ModifierKey::LeftSuper => 3,
        ModifierKey::LeftHyper => 4,
        ModifierKey::LeftMeta => 5,
        ModifierKey::RightShift => 6,
        ModifierKey::RightControl => 7,
        ModifierKey::RightAlt => 8,
        ModifierKey::RightSuper => 9,
        ModifierKey::RightHyper => 10,
        ModifierKey::RightMeta => 11,
        ModifierKey::IsoLevel3Shift => 12,
        ModifierKey::IsoLevel5Shift => 13,
    }
}

const fn modifier_group(key: ModifierKey) -> Option<(Modifiers, &'static [ModifierKey])> {
    match key {
        ModifierKey::LeftShift | ModifierKey::RightShift => Some((Modifiers::SHIFT, SHIFT_KEYS)),
        ModifierKey::LeftControl | ModifierKey::RightControl => {
            Some((Modifiers::CONTROL, CONTROL_KEYS))
        }
        ModifierKey::LeftAlt | ModifierKey::RightAlt => Some((Modifiers::ALT, ALT_KEYS)),
        ModifierKey::LeftSuper | ModifierKey::RightSuper => Some((Modifiers::SUPER, SUPER_KEYS)),
        ModifierKey::LeftHyper | ModifierKey::RightHyper => Some((Modifiers::HYPER, HYPER_KEYS)),
        ModifierKey::LeftMeta | ModifierKey::RightMeta => Some((Modifiers::META, META_KEYS)),
        ModifierKey::IsoLevel3Shift | ModifierKey::IsoLevel5Shift => None,
    }
}

fn standalone_key(
    physical_key: PhysicalKey,
    location: KeyLocation,
    logical_key: &Key<&str>,
) -> Option<TerminalKey> {
    if let Some(modifier) = modifier_key(physical_key, location, logical_key) {
        return Some(TerminalKey::Modifier(modifier));
    }

    let named = match logical_key {
        Key::Named(named) => *named,
        Key::Character(_) | Key::Unidentified(_) | Key::Dead(_) => {
            return physical_functional_key(physical_key);
        }
    };
    Some(match named {
        NamedKey::CapsLock => TerminalKey::CapsLock,
        NamedKey::ScrollLock => TerminalKey::ScrollLock,
        NamedKey::NumLock => TerminalKey::NumLock,
        NamedKey::PrintScreen => TerminalKey::PrintScreen,
        NamedKey::Pause => TerminalKey::Pause,
        NamedKey::ContextMenu => TerminalKey::Menu,
        NamedKey::MediaPlay => TerminalKey::Media(MediaKey::Play),
        NamedKey::MediaPause => TerminalKey::Media(MediaKey::Pause),
        NamedKey::MediaPlayPause => TerminalKey::Media(MediaKey::PlayPause),
        NamedKey::MediaStop => TerminalKey::Media(MediaKey::Stop),
        NamedKey::MediaFastForward => TerminalKey::Media(MediaKey::FastForward),
        NamedKey::MediaRewind => TerminalKey::Media(MediaKey::Rewind),
        NamedKey::MediaTrackNext => TerminalKey::Media(MediaKey::TrackNext),
        NamedKey::MediaTrackPrevious => TerminalKey::Media(MediaKey::TrackPrevious),
        NamedKey::MediaRecord => TerminalKey::Media(MediaKey::Record),
        NamedKey::AudioVolumeDown => TerminalKey::Media(MediaKey::LowerVolume),
        NamedKey::AudioVolumeUp => TerminalKey::Media(MediaKey::RaiseVolume),
        NamedKey::AudioVolumeMute => TerminalKey::Media(MediaKey::Mute),
        _ => return physical_functional_key(physical_key),
    })
}

fn modifier_key(
    physical_key: PhysicalKey,
    location: KeyLocation,
    logical_key: &Key<&str>,
) -> Option<ModifierKey> {
    if matches!(logical_key, Key::Named(NamedKey::AltGraph)) {
        return Some(ModifierKey::IsoLevel3Shift);
    }
    match physical_key {
        PhysicalKey::Code(KeyCode::ShiftLeft) => Some(ModifierKey::LeftShift),
        PhysicalKey::Code(KeyCode::ShiftRight) => Some(ModifierKey::RightShift),
        PhysicalKey::Code(KeyCode::ControlLeft) => Some(ModifierKey::LeftControl),
        PhysicalKey::Code(KeyCode::ControlRight) => Some(ModifierKey::RightControl),
        PhysicalKey::Code(KeyCode::AltLeft) => Some(ModifierKey::LeftAlt),
        PhysicalKey::Code(KeyCode::AltRight) => Some(ModifierKey::RightAlt),
        PhysicalKey::Code(KeyCode::SuperLeft) => Some(ModifierKey::LeftSuper),
        PhysicalKey::Code(KeyCode::SuperRight) => Some(ModifierKey::RightSuper),
        PhysicalKey::Code(KeyCode::Hyper) => {
            sided_modifier(location, ModifierKey::LeftHyper, ModifierKey::RightHyper)
        }
        PhysicalKey::Code(KeyCode::Meta) => {
            sided_modifier(location, ModifierKey::LeftMeta, ModifierKey::RightMeta)
        }
        PhysicalKey::Code(_) | PhysicalKey::Unidentified(_) => match logical_key {
            Key::Named(NamedKey::Shift) => {
                sided_modifier(location, ModifierKey::LeftShift, ModifierKey::RightShift)
            }
            Key::Named(NamedKey::Control) => sided_modifier(
                location,
                ModifierKey::LeftControl,
                ModifierKey::RightControl,
            ),
            Key::Named(NamedKey::Alt) => {
                sided_modifier(location, ModifierKey::LeftAlt, ModifierKey::RightAlt)
            }
            Key::Named(NamedKey::Super) => {
                sided_modifier(location, ModifierKey::LeftSuper, ModifierKey::RightSuper)
            }
            Key::Named(NamedKey::Hyper) => {
                sided_modifier(location, ModifierKey::LeftHyper, ModifierKey::RightHyper)
            }
            Key::Named(NamedKey::Meta) => {
                sided_modifier(location, ModifierKey::LeftMeta, ModifierKey::RightMeta)
            }
            _ => None,
        },
    }
}

const fn sided_modifier(
    location: KeyLocation,
    left: ModifierKey,
    right: ModifierKey,
) -> Option<ModifierKey> {
    match location {
        KeyLocation::Right => Some(right),
        // Winit exposes generic Hyper/Meta physical codes without sided variants. When a backend
        // also omits location, retain the identifiable key with a deterministic left identity.
        KeyLocation::Left | KeyLocation::Standard => Some(left),
        KeyLocation::Numpad => None,
    }
}

fn physical_functional_key(physical_key: PhysicalKey) -> Option<TerminalKey> {
    let PhysicalKey::Code(code) = physical_key else {
        return None;
    };
    Some(match code {
        KeyCode::CapsLock => TerminalKey::CapsLock,
        KeyCode::ScrollLock => TerminalKey::ScrollLock,
        KeyCode::NumLock => TerminalKey::NumLock,
        KeyCode::PrintScreen => TerminalKey::PrintScreen,
        KeyCode::Pause => TerminalKey::Pause,
        KeyCode::ContextMenu => TerminalKey::Menu,
        KeyCode::MediaPlayPause => TerminalKey::Media(MediaKey::PlayPause),
        KeyCode::MediaStop => TerminalKey::Media(MediaKey::Stop),
        KeyCode::MediaTrackNext => TerminalKey::Media(MediaKey::TrackNext),
        KeyCode::MediaTrackPrevious => TerminalKey::Media(MediaKey::TrackPrevious),
        KeyCode::AudioVolumeDown => TerminalKey::Media(MediaKey::LowerVolume),
        KeyCode::AudioVolumeUp => TerminalKey::Media(MediaKey::RaiseVolume),
        KeyCode::AudioVolumeMute => TerminalKey::Media(MediaKey::Mute),
        _ => return None,
    })
}

const fn pc101_base_layout(physical_key: PhysicalKey) -> Option<u32> {
    let PhysicalKey::Code(code) = physical_key else {
        return None;
    };
    Some(match code {
        KeyCode::KeyA => b'a' as u32,
        KeyCode::KeyB => b'b' as u32,
        KeyCode::KeyC => b'c' as u32,
        KeyCode::KeyD => b'd' as u32,
        KeyCode::KeyE => b'e' as u32,
        KeyCode::KeyF => b'f' as u32,
        KeyCode::KeyG => b'g' as u32,
        KeyCode::KeyH => b'h' as u32,
        KeyCode::KeyI => b'i' as u32,
        KeyCode::KeyJ => b'j' as u32,
        KeyCode::KeyK => b'k' as u32,
        KeyCode::KeyL => b'l' as u32,
        KeyCode::KeyM => b'm' as u32,
        KeyCode::KeyN => b'n' as u32,
        KeyCode::KeyO => b'o' as u32,
        KeyCode::KeyP => b'p' as u32,
        KeyCode::KeyQ => b'q' as u32,
        KeyCode::KeyR => b'r' as u32,
        KeyCode::KeyS => b's' as u32,
        KeyCode::KeyT => b't' as u32,
        KeyCode::KeyU => b'u' as u32,
        KeyCode::KeyV => b'v' as u32,
        KeyCode::KeyW => b'w' as u32,
        KeyCode::KeyX => b'x' as u32,
        KeyCode::KeyY => b'y' as u32,
        KeyCode::KeyZ => b'z' as u32,
        KeyCode::Digit0 => b'0' as u32,
        KeyCode::Digit1 => b'1' as u32,
        KeyCode::Digit2 => b'2' as u32,
        KeyCode::Digit3 => b'3' as u32,
        KeyCode::Digit4 => b'4' as u32,
        KeyCode::Digit5 => b'5' as u32,
        KeyCode::Digit6 => b'6' as u32,
        KeyCode::Digit7 => b'7' as u32,
        KeyCode::Digit8 => b'8' as u32,
        KeyCode::Digit9 => b'9' as u32,
        KeyCode::Backquote => b'`' as u32,
        KeyCode::Minus => b'-' as u32,
        KeyCode::Equal => b'=' as u32,
        KeyCode::BracketLeft => b'[' as u32,
        KeyCode::BracketRight => b']' as u32,
        KeyCode::Backslash => b'\\' as u32,
        KeyCode::Semicolon => b';' as u32,
        KeyCode::Quote => b'\'' as u32,
        KeyCode::Comma => b',' as u32,
        KeyCode::Period => b'.' as u32,
        KeyCode::Slash => b'/' as u32,
        KeyCode::Space => b' ' as u32,
        _ => return None,
    })
}

fn keypad_key(
    physical_key: PhysicalKey,
    location: KeyLocation,
    logical_key: &Key<&str>,
) -> Option<KeypadKey> {
    let physical_code = match physical_key {
        PhysicalKey::Code(code) => Some(code),
        PhysicalKey::Unidentified(_) => None,
    };
    let is_keypad = location == KeyLocation::Numpad
        || physical_code.is_some_and(|code| {
            matches!(
                code,
                KeyCode::Numpad0
                    | KeyCode::Numpad1
                    | KeyCode::Numpad2
                    | KeyCode::Numpad3
                    | KeyCode::Numpad4
                    | KeyCode::Numpad5
                    | KeyCode::Numpad6
                    | KeyCode::Numpad7
                    | KeyCode::Numpad8
                    | KeyCode::Numpad9
                    | KeyCode::NumpadAdd
                    | KeyCode::NumpadClear
                    | KeyCode::NumpadComma
                    | KeyCode::NumpadDecimal
                    | KeyCode::NumpadDivide
                    | KeyCode::NumpadEnter
                    | KeyCode::NumpadEqual
                    | KeyCode::NumpadMultiply
                    | KeyCode::NumpadStar
                    | KeyCode::NumpadSubtract
            )
        });
    if !is_keypad {
        return None;
    }

    match logical_key {
        Key::Named(NamedKey::ArrowLeft) => return Some(KeypadKey::Left),
        Key::Named(NamedKey::ArrowRight) => return Some(KeypadKey::Right),
        Key::Named(NamedKey::ArrowUp) => return Some(KeypadKey::Up),
        Key::Named(NamedKey::ArrowDown) => return Some(KeypadKey::Down),
        Key::Named(NamedKey::PageUp) => return Some(KeypadKey::PageUp),
        Key::Named(NamedKey::PageDown) => return Some(KeypadKey::PageDown),
        Key::Named(NamedKey::Home) => return Some(KeypadKey::Home),
        Key::Named(NamedKey::End) => return Some(KeypadKey::End),
        Key::Named(NamedKey::Insert) => return Some(KeypadKey::Insert),
        Key::Named(NamedKey::Delete) => return Some(KeypadKey::Delete),
        Key::Named(NamedKey::Clear) => return Some(KeypadKey::Begin),
        Key::Named(NamedKey::Enter) => return Some(KeypadKey::Enter),
        _ => {}
    }

    Some(match physical_code? {
        KeyCode::Numpad0 => KeypadKey::Digit(0),
        KeyCode::Numpad1 => KeypadKey::Digit(1),
        KeyCode::Numpad2 => KeypadKey::Digit(2),
        KeyCode::Numpad3 => KeypadKey::Digit(3),
        KeyCode::Numpad4 => KeypadKey::Digit(4),
        KeyCode::Numpad5 => KeypadKey::Digit(5),
        KeyCode::Numpad6 => KeypadKey::Digit(6),
        KeyCode::Numpad7 => KeypadKey::Digit(7),
        KeyCode::Numpad8 => KeypadKey::Digit(8),
        KeyCode::Numpad9 => KeypadKey::Digit(9),
        KeyCode::NumpadDecimal => KeypadKey::Decimal,
        KeyCode::NumpadDivide => KeypadKey::Divide,
        KeyCode::NumpadMultiply | KeyCode::NumpadStar => KeypadKey::Multiply,
        KeyCode::NumpadSubtract => KeypadKey::Subtract,
        KeyCode::NumpadAdd => KeypadKey::Add,
        KeyCode::NumpadEnter => KeypadKey::Enter,
        KeyCode::NumpadEqual => KeypadKey::Equal,
        KeyCode::NumpadComma => KeypadKey::Separator,
        KeyCode::NumpadClear => KeypadKey::Begin,
        _ => return None,
    })
}

fn is_text_key(key: &Key<&str>) -> bool {
    matches!(key, Key::Character(_) | Key::Named(NamedKey::Space))
}

fn function_number(key: NamedKey) -> Option<u8> {
    Some(match key {
        NamedKey::F1 => 1,
        NamedKey::F2 => 2,
        NamedKey::F3 => 3,
        NamedKey::F4 => 4,
        NamedKey::F5 => 5,
        NamedKey::F6 => 6,
        NamedKey::F7 => 7,
        NamedKey::F8 => 8,
        NamedKey::F9 => 9,
        NamedKey::F10 => 10,
        NamedKey::F11 => 11,
        NamedKey::F12 => 12,
        NamedKey::F13 => 13,
        NamedKey::F14 => 14,
        NamedKey::F15 => 15,
        NamedKey::F16 => 16,
        NamedKey::F17 => 17,
        NamedKey::F18 => 18,
        NamedKey::F19 => 19,
        NamedKey::F20 => 20,
        NamedKey::F21 => 21,
        NamedKey::F22 => 22,
        NamedKey::F23 => 23,
        NamedKey::F24 => 24,
        NamedKey::F25 => 25,
        NamedKey::F26 => 26,
        NamedKey::F27 => 27,
        NamedKey::F28 => 28,
        NamedKey::F29 => 29,
        NamedKey::F30 => 30,
        NamedKey::F31 => 31,
        NamedKey::F32 => 32,
        NamedKey::F33 => 33,
        NamedKey::F34 => 34,
        NamedKey::F35 => 35,
        _ => return None,
    })
}

pub(crate) fn map_modifiers(modifiers: WinitModifiers) -> Modifiers {
    map_modifier_state(modifiers.state(), true)
}

fn terminal_alt_for_key(
    modifiers: WinitModifiers,
    logical_key: &Key<&str>,
    key_without_modifiers: &Key<&str>,
) -> bool {
    terminal_alt_for_key_parts(
        modifiers.state(),
        is_text_key(logical_key),
        text_key_was_transformed(logical_key, key_without_modifiers),
        modifiers.lalt_state(),
        modifiers.ralt_state(),
    )
}

fn text_key_was_transformed(logical_key: &Key<&str>, key_without_modifiers: &Key<&str>) -> bool {
    match (logical_key, key_without_modifiers) {
        (Key::Character(logical), Key::Character(unmodified)) => logical != unmodified,
        _ => false,
    }
}

fn terminal_alt_for_key_parts(
    state: ModifiersState,
    is_text_key: bool,
    text_was_transformed: bool,
    left_alt: ModifiersKeyState,
    right_alt: ModifiersKeyState,
) -> bool {
    if !state.alt_key() || !is_text_key {
        return state.alt_key();
    }

    #[cfg(target_os = "macos")]
    {
        // Both Option keys remain available to the active macOS layout. Layout-transformed text
        // is literal input (`@`, `|`, braces, and so on); an unchanged chord remains usable as a
        // terminal Alt/Meta shortcut. Side metadata is deliberately irrelevant to this decision.
        let _ = (left_alt, right_alt);
        !text_was_transformed
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (text_was_transformed, left_alt, right_alt);
        true
    }
}

fn map_modifier_state(state: ModifiersState, include_alt: bool) -> Modifiers {
    let mut mapped = Modifiers::empty();
    mapped.set(Modifiers::SHIFT, state.shift_key());
    mapped.set(Modifiers::ALT, include_alt && state.alt_key());
    mapped.set(Modifiers::CONTROL, state.control_key());
    mapped.set(Modifiers::SUPER, state.super_key());
    mapped
}

#[cfg(test)]
mod tests {
    use engine::{
        Key as TerminalKey, KeyEventKind, KeypadKey, MediaKey, ModifierKey, Modifiers, Terminal,
        TerminalConfig,
    };
    use winit::{
        event::ElementState,
        keyboard::{
            Key, KeyCode, KeyLocation, ModifiersKeyState, ModifiersState, NamedKey, PhysicalKey,
        },
    };

    use super::{
        KeyboardState, map_key_parts, terminal_alt_for_key_parts, text_key_was_transformed,
    };

    #[allow(clippy::needless_pass_by_value, clippy::too_many_arguments)]
    fn mapped(
        physical_key: PhysicalKey,
        logical_key: Key<&str>,
        key_without_modifiers: Key<&str>,
        text: Option<&str>,
        modifiers: ModifiersState,
        alt_is_terminal: bool,
        state: ElementState,
        repeat: bool,
    ) -> engine::KeyEvent {
        let mut keyboard_state = KeyboardState::default();
        map_key_parts(
            physical_key,
            KeyLocation::Standard,
            &logical_key,
            &key_without_modifiers,
            text,
            modifiers,
            alt_is_terminal,
            state,
            repeat,
            &mut keyboard_state,
        )
        .expect("key should map")
    }

    #[test]
    fn localized_option_character_keeps_layout_text_without_terminal_alt() {
        let event = mapped(
            PhysicalKey::Code(KeyCode::Digit2),
            Key::Character("@"),
            Key::Character("2"),
            Some("@"),
            ModifiersState::ALT,
            false,
            ElementState::Pressed,
            false,
        );

        assert_eq!(event.key, TerminalKey::Character('2'));
        assert_eq!(event.text.as_deref(), Some("@"));
        assert_eq!(event.modifiers, Modifiers::empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn right_option_danish_at_encodes_one_literal_pty_byte() {
        let alt_is_terminal = terminal_alt_for_key_parts(
            ModifiersState::ALT,
            true,
            true,
            ModifiersKeyState::Unknown,
            ModifiersKeyState::Pressed,
        );
        let event = mapped(
            PhysicalKey::Code(KeyCode::Backslash),
            Key::Character("@"),
            Key::Character("'"),
            Some("@"),
            ModifiersState::ALT,
            alt_is_terminal,
            ElementState::Pressed,
            false,
        );
        let terminal = Terminal::new(TerminalConfig::default());

        assert_eq!(terminal.encode_key(&event), b"@");
    }

    #[test]
    fn terminal_option_side_retains_alt_for_meta_shortcuts() {
        let event = mapped(
            PhysicalKey::Code(KeyCode::KeyB),
            Key::Character("b"),
            Key::Character("b"),
            Some("b"),
            ModifiersState::ALT,
            true,
            ElementState::Pressed,
            false,
        );

        assert_eq!(event.key, TerminalKey::Character('b'));
        assert_eq!(event.text.as_deref(), Some("b"));
        assert_eq!(event.modifiers, Modifiers::ALT);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn option_sides_select_layout_text_or_terminal_alt() {
        let layout_text = text_key_was_transformed(&Key::Character("@"), &Key::Character("2"));
        let unchanged_text = text_key_was_transformed(&Key::Character("b"), &Key::Character("b"));
        assert!(!terminal_alt_for_key_parts(
            ModifiersState::ALT,
            true,
            true,
            ModifiersKeyState::Pressed,
            ModifiersKeyState::Unknown,
        ));
        assert!(terminal_alt_for_key_parts(
            ModifiersState::ALT,
            true,
            false,
            ModifiersKeyState::Unknown,
            ModifiersKeyState::Pressed,
        ));
        assert!(!terminal_alt_for_key_parts(
            ModifiersState::ALT,
            true,
            true,
            ModifiersKeyState::Unknown,
            ModifiersKeyState::Pressed,
        ));
        assert!(terminal_alt_for_key_parts(
            ModifiersState::ALT,
            false,
            false,
            ModifiersKeyState::Pressed,
            ModifiersKeyState::Unknown,
        ));
        assert!(!terminal_alt_for_key_parts(
            ModifiersState::ALT,
            true,
            layout_text,
            ModifiersKeyState::Unknown,
            ModifiersKeyState::Unknown,
        ));
        assert!(terminal_alt_for_key_parts(
            ModifiersState::ALT,
            true,
            unchanged_text,
            ModifiersKeyState::Unknown,
            ModifiersKeyState::Unknown,
        ));
    }

    #[test]
    fn shifted_tab_retains_shift_for_backtab_encoding() {
        let event = mapped(
            PhysicalKey::Code(KeyCode::Tab),
            Key::Named(NamedKey::Tab),
            Key::Named(NamedKey::Tab),
            Some("\t"),
            ModifiersState::SHIFT,
            true,
            ElementState::Pressed,
            false,
        );

        assert_eq!(event.key, TerminalKey::Backtab);
        assert_eq!(event.modifiers, Modifiers::SHIFT);
    }

    #[test]
    fn press_repeat_and_release_are_distinct_and_release_has_no_text() {
        let press = mapped(
            PhysicalKey::Code(KeyCode::KeyA),
            Key::Character("a"),
            Key::Character("a"),
            Some("a"),
            ModifiersState::empty(),
            true,
            ElementState::Pressed,
            false,
        );
        let repeat = mapped(
            PhysicalKey::Code(KeyCode::KeyA),
            Key::Character("a"),
            Key::Character("a"),
            Some("a"),
            ModifiersState::empty(),
            true,
            ElementState::Pressed,
            true,
        );
        let release = mapped(
            PhysicalKey::Code(KeyCode::KeyA),
            Key::Character("a"),
            Key::Character("a"),
            None,
            ModifiersState::empty(),
            true,
            ElementState::Released,
            false,
        );
        let shifted_release = mapped(
            PhysicalKey::Code(KeyCode::Equal),
            Key::Character("+"),
            Key::Character("="),
            None,
            ModifiersState::SHIFT,
            true,
            ElementState::Released,
            false,
        );

        assert_eq!(press.kind, KeyEventKind::Press);
        assert_eq!(repeat.kind, KeyEventKind::Repeat);
        assert_eq!(release.kind, KeyEventKind::Release);
        assert_eq!(release.text, None);
        assert_eq!(shifted_release.text, None);
        assert_eq!(shifted_release.shifted_key, Some(u32::from('+')));
    }

    #[test]
    fn dead_key_waits_for_ime_commit() {
        let mut keyboard_state = KeyboardState::default();
        assert!(
            map_key_parts(
                PhysicalKey::Code(KeyCode::Quote),
                KeyLocation::Standard,
                &Key::Dead(Some('\u{00b4}')),
                &Key::Character("'"),
                None,
                ModifiersState::empty(),
                true,
                ElementState::Pressed,
                false,
                &mut keyboard_state,
            )
            .is_none()
        );
    }

    #[test]
    fn navigation_function_and_combined_modifiers_are_preserved() {
        let navigation = mapped(
            PhysicalKey::Code(KeyCode::ArrowLeft),
            Key::Named(NamedKey::ArrowLeft),
            Key::Named(NamedKey::ArrowLeft),
            None,
            ModifiersState::SHIFT | ModifiersState::ALT | ModifiersState::CONTROL,
            true,
            ElementState::Pressed,
            false,
        );
        let function = mapped(
            PhysicalKey::Code(KeyCode::F35),
            Key::Named(NamedKey::F35),
            Key::Named(NamedKey::F35),
            None,
            ModifiersState::SUPER,
            true,
            ElementState::Pressed,
            false,
        );

        assert_eq!(navigation.key, TerminalKey::Left);
        assert_eq!(
            navigation.modifiers,
            Modifiers::SHIFT | Modifiers::ALT | Modifiers::CONTROL
        );
        assert_eq!(function.key, TerminalKey::Function(35));
        assert_eq!(function.modifiers, Modifiers::SUPER);
    }

    #[test]
    fn keypad_uses_physical_identity_and_logical_navigation_mode() {
        let mut keyboard_state = KeyboardState::default();
        let digit = map_key_parts(
            PhysicalKey::Code(KeyCode::Numpad1),
            KeyLocation::Numpad,
            &Key::Character("1"),
            &Key::Character("1"),
            Some("1"),
            ModifiersState::empty(),
            true,
            ElementState::Pressed,
            false,
            &mut keyboard_state,
        )
        .expect("keypad digit should map");
        let navigation = map_key_parts(
            PhysicalKey::Code(KeyCode::Numpad1),
            KeyLocation::Numpad,
            &Key::Named(NamedKey::End),
            &Key::Named(NamedKey::End),
            None,
            ModifiersState::empty(),
            true,
            ElementState::Pressed,
            false,
            &mut keyboard_state,
        )
        .expect("keypad navigation should map");
        let enter = map_key_parts(
            PhysicalKey::Code(KeyCode::NumpadEnter),
            KeyLocation::Numpad,
            &Key::Named(NamedKey::Enter),
            &Key::Named(NamedKey::Enter),
            Some("\r"),
            ModifiersState::empty(),
            true,
            ElementState::Pressed,
            false,
            &mut keyboard_state,
        )
        .expect("keypad enter should map");

        assert_eq!(digit.key, TerminalKey::Keypad(KeypadKey::Digit(1)));
        assert_eq!(digit.text.as_deref(), Some("1"));
        assert_eq!(navigation.key, TerminalKey::Keypad(KeypadKey::End));
        assert_eq!(enter.key, TerminalKey::Keypad(KeypadKey::Enter));
    }

    #[test]
    fn modifier_events_correct_stale_cached_state_for_press_and_release() {
        let mut keyboard_state = KeyboardState::default();
        let press = map_key_parts(
            PhysicalKey::Code(KeyCode::ShiftLeft),
            KeyLocation::Left,
            &Key::Named(NamedKey::Shift),
            &Key::Named(NamedKey::Shift),
            None,
            ModifiersState::empty(),
            true,
            ElementState::Pressed,
            false,
            &mut keyboard_state,
        )
        .expect("left shift press should map");
        let release = map_key_parts(
            PhysicalKey::Code(KeyCode::ShiftLeft),
            KeyLocation::Left,
            &Key::Named(NamedKey::Shift),
            &Key::Named(NamedKey::Shift),
            None,
            ModifiersState::SHIFT,
            true,
            ElementState::Released,
            false,
            &mut keyboard_state,
        )
        .expect("left shift release should map");

        assert_eq!(press.key, TerminalKey::Modifier(ModifierKey::LeftShift));
        assert_eq!(press.modifiers, Modifiers::SHIFT);
        assert_eq!(release.modifiers, Modifiers::empty());
    }

    #[test]
    fn lock_state_toggles_before_the_current_event_and_ignores_repeats() {
        let mut keyboard_state = KeyboardState::default();
        let mut map_caps = |state, repeat| {
            map_key_parts(
                PhysicalKey::Code(KeyCode::CapsLock),
                KeyLocation::Standard,
                &Key::Named(NamedKey::CapsLock),
                &Key::Named(NamedKey::CapsLock),
                None,
                ModifiersState::empty(),
                true,
                state,
                repeat,
                &mut keyboard_state,
            )
            .expect("caps lock should map")
        };

        let press = map_caps(ElementState::Pressed, false);
        let repeat = map_caps(ElementState::Pressed, true);
        let release = map_caps(ElementState::Released, false);
        let next_press = map_caps(ElementState::Pressed, false);

        assert_eq!(press.key, TerminalKey::CapsLock);
        assert!(press.modifiers.contains(Modifiers::CAPS_LOCK));
        assert!(repeat.modifiers.contains(Modifiers::CAPS_LOCK));
        assert!(release.modifiers.contains(Modifiers::CAPS_LOCK));
        assert!(!next_press.modifiers.contains(Modifiers::CAPS_LOCK));

        let num_lock = map_key_parts(
            PhysicalKey::Code(KeyCode::NumLock),
            KeyLocation::Numpad,
            &Key::Named(NamedKey::Clear),
            &Key::Named(NamedKey::Clear),
            None,
            ModifiersState::empty(),
            true,
            ElementState::Pressed,
            false,
            &mut keyboard_state,
        )
        .expect("keypad clear should retain num-lock identity");
        assert_eq!(num_lock.key, TerminalKey::NumLock);
        assert!(num_lock.modifiers.contains(Modifiers::NUM_LOCK));
    }

    #[test]
    fn alt_graph_precedes_right_alt_and_generic_hyper_has_a_stable_side() {
        let alt_graph = mapped(
            PhysicalKey::Code(KeyCode::AltRight),
            Key::Named(NamedKey::AltGraph),
            Key::Named(NamedKey::AltGraph),
            None,
            ModifiersState::empty(),
            true,
            ElementState::Pressed,
            false,
        );
        let hyper = mapped(
            PhysicalKey::Code(KeyCode::Hyper),
            Key::Named(NamedKey::Hyper),
            Key::Named(NamedKey::Hyper),
            None,
            ModifiersState::empty(),
            true,
            ElementState::Pressed,
            false,
        );

        assert_eq!(
            alt_graph.key,
            TerminalKey::Modifier(ModifierKey::IsoLevel3Shift)
        );
        assert_eq!(hyper.key, TerminalKey::Modifier(ModifierKey::LeftHyper));
        assert_eq!(hyper.modifiers, Modifiers::HYPER);
    }

    #[test]
    fn kitty_functional_and_media_keys_map_from_winit() {
        let print_screen = mapped(
            PhysicalKey::Code(KeyCode::PrintScreen),
            Key::Named(NamedKey::PrintScreen),
            Key::Named(NamedKey::PrintScreen),
            None,
            ModifiersState::empty(),
            true,
            ElementState::Pressed,
            false,
        );
        let menu = mapped(
            PhysicalKey::Code(KeyCode::ContextMenu),
            Key::Named(NamedKey::ContextMenu),
            Key::Named(NamedKey::ContextMenu),
            None,
            ModifiersState::empty(),
            true,
            ElementState::Pressed,
            false,
        );
        let volume = mapped(
            PhysicalKey::Code(KeyCode::AudioVolumeDown),
            Key::Named(NamedKey::AudioVolumeDown),
            Key::Named(NamedKey::AudioVolumeDown),
            None,
            ModifiersState::empty(),
            true,
            ElementState::Pressed,
            false,
        );

        assert_eq!(print_screen.key, TerminalKey::PrintScreen);
        assert_eq!(menu.key, TerminalKey::Menu);
        assert_eq!(volume.key, TerminalKey::Media(MediaKey::LowerVolume));
    }

    #[test]
    fn pc101_base_layout_tracks_physical_letters_under_localized_layouts() {
        let event = mapped(
            PhysicalKey::Code(KeyCode::KeyQ),
            Key::Character("\u{0439}"),
            Key::Character("\u{0439}"),
            Some("\u{0439}"),
            ModifiersState::empty(),
            true,
            ElementState::Pressed,
            false,
        );

        assert_eq!(event.key, TerminalKey::Character('\u{0439}'));
        assert_eq!(event.base_layout, Some(u32::from('q')));
    }
}
