//! Keyboard, paste, and mouse protocol encoding.

use bitflags::bitflags;
use serde::{Deserialize, Serialize};

bitflags! {
    #[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
    pub struct Modifiers: u8 {
        const SHIFT = 1 << 0;
        const ALT = 1 << 1;
        const CONTROL = 1 << 2;
        const SUPER = 1 << 3;
        const HYPER = 1 << 4;
        const META = 1 << 5;
        const CAPS_LOCK = 1 << 6;
        const NUM_LOCK = 1 << 7;
    }
}

bitflags! {
    #[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
    pub struct KittyKeyboardFlags: u8 {
        const DISAMBIGUATE = 1 << 0;
        const REPORT_EVENT_TYPES = 1 << 1;
        const REPORT_ALTERNATE_KEYS = 1 << 2;
        const REPORT_ALL_KEYS = 1 << 3;
        const REPORT_ASSOCIATED_TEXT = 1 << 4;
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum KeyEventKind {
    Press,
    Repeat,
    Release,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Key {
    Character(char),
    Escape,
    Enter,
    Tab,
    Backtab,
    Backspace,
    Insert,
    Delete,
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    Function(u8),
    Keypad(KeypadKey),
    CapsLock,
    ScrollLock,
    NumLock,
    PrintScreen,
    Pause,
    Menu,
    Media(MediaKey),
    Modifier(ModifierKey),
}

/// A modifier key whose physical identity is reported by the Kitty keyboard protocol.
///
/// Modifier-only events are intentionally silent unless `REPORT_ALL_KEYS` is active.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ModifierKey {
    LeftShift,
    LeftControl,
    LeftAlt,
    LeftSuper,
    LeftHyper,
    LeftMeta,
    RightShift,
    RightControl,
    RightAlt,
    RightSuper,
    RightHyper,
    RightMeta,
    IsoLevel3Shift,
    IsoLevel5Shift,
}

/// A media key represented by the Kitty keyboard protocol's functional-key codepoints.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum MediaKey {
    Play,
    Pause,
    PlayPause,
    Reverse,
    Stop,
    FastForward,
    Rewind,
    TrackNext,
    TrackPrevious,
    Record,
    LowerVolume,
    RaiseVolume,
    Mute,
}

/// A key on the numeric keypad.
///
/// Keeping keypad identity separate lets application-keypad mode and the Kitty keyboard
/// protocol distinguish these keys from the equivalent keys on the main keyboard.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum KeypadKey {
    Digit(u8),
    Decimal,
    Divide,
    Multiply,
    Subtract,
    Add,
    Enter,
    Equal,
    Separator,
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    Insert,
    Delete,
    Begin,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct KeyEvent {
    pub key: Key,
    pub text: Option<String>,
    /// The shifted Unicode identity for this key in the active keyboard layout.
    ///
    /// This is stored separately from `text` because key-release events do not produce text, but
    /// Kitty alternate-key reporting still needs the same shifted identity on press and release.
    pub shifted_key: Option<u32>,
    /// The Unicode codepoint at this physical position in the PC-101 base layout.
    ///
    /// Kitty uses this optional value as the third alternate-key field so applications can
    /// identify a shortcut independently of the active keyboard layout.
    pub base_layout: Option<u32>,
    pub modifiers: Modifiers,
    pub kind: KeyEventKind,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ModifyOtherKeysState {
    pub(crate) level: Option<u8>,
    pub(crate) format: u8,
    pub(crate) excluded_modifiers: Modifiers,
}

impl Default for ModifyOtherKeysState {
    fn default() -> Self {
        Self {
            level: Some(0),
            format: 0,
            excluded_modifiers: Modifiers::empty(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum MouseTrackingMode {
    #[default]
    Disabled,
    Press,
    ButtonMotion,
    AnyMotion,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum MouseButton {
    None,
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum MouseEventKind {
    Press,
    Release,
    Motion,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MouseEvent {
    pub button: MouseButton,
    pub kind: MouseEventKind,
    pub column: usize,
    pub row: usize,
    pub pixel_x: usize,
    pub pixel_y: usize,
    pub modifiers: Modifiers,
}

#[must_use]
pub fn encode_key(
    event: &KeyEvent,
    kitty_flags: KittyKeyboardFlags,
    application_cursor: bool,
    application_keypad: bool,
    modify_other_keys: ModifyOtherKeysState,
) -> Vec<u8> {
    if event.kind == KeyEventKind::Release
        && !kitty_flags.contains(KittyKeyboardFlags::REPORT_EVENT_TYPES)
    {
        return Vec::new();
    }

    match &event.key {
        Key::Character(character) => {
            encode_character(*character, event, kitty_flags, modify_other_keys)
        }
        Key::Keypad(keypad) => encode_keypad(
            *keypad,
            event,
            kitty_flags,
            application_cursor,
            application_keypad,
        ),
        key => {
            if special_uses_kitty(key, event, kitty_flags) {
                encode_kitty_special(key, event, kitty_flags)
            } else if let Some(encoded) = encode_modify_other_key(key, event, modify_other_keys) {
                encoded
            } else {
                encode_legacy_special(key, event, application_cursor)
            }
        }
    }
}

#[must_use]
pub fn encode_text(text: &str, kitty_flags: KittyKeyboardFlags) -> Vec<u8> {
    if !kitty_flags
        .contains(KittyKeyboardFlags::REPORT_ALL_KEYS | KittyKeyboardFlags::REPORT_ASSOCIATED_TEXT)
    {
        return text.as_bytes().to_vec();
    }
    let codepoints = associated_codepoints(text);
    if codepoints.is_empty() {
        Vec::new()
    } else {
        format!("\x1b[0;;{}u", codepoints.join(":")).into_bytes()
    }
}

fn encode_character(
    character: char,
    event: &KeyEvent,
    kitty_flags: KittyKeyboardFlags,
    modify_other_keys: ModifyOtherKeysState,
) -> Vec<u8> {
    let uses_kitty = kitty_flags.contains(KittyKeyboardFlags::REPORT_ALL_KEYS)
        || (kitty_flags.contains(KittyKeyboardFlags::DISAMBIGUATE)
            && event.modifiers.intersects(
                Modifiers::ALT
                    | Modifiers::CONTROL
                    | Modifiers::SUPER
                    | Modifiers::HYPER
                    | Modifiers::META,
            ))
        || has_unsupported_legacy_modifiers(event.modifiers);
    if uses_kitty {
        encode_kitty_character(character, event, kitty_flags)
    } else if let Some(encoded) = encode_modify_other_character(character, event, modify_other_keys)
    {
        encoded
    } else {
        encode_legacy_character(character, event)
    }
}

fn encode_kitty_character(character: char, event: &KeyEvent, flags: KittyKeyboardFlags) -> Vec<u8> {
    let key_code = character.to_lowercase().next().unwrap_or(character) as u32;
    let produces_text = event
        .text
        .as_deref()
        .is_some_and(|text| text.chars().any(|character| !character.is_control()))
        && !event.modifiers.intersects(
            Modifiers::ALT
                | Modifiers::CONTROL
                | Modifiers::SUPER
                | Modifiers::HYPER
                | Modifiers::META,
        );
    encode_csi_u(key_code, event, flags, event.modifiers, produces_text, true)
}

fn special_uses_kitty(key: &Key, event: &KeyEvent, flags: KittyKeyboardFlags) -> bool {
    if matches!(key, Key::Modifier(_)) {
        return flags.contains(KittyKeyboardFlags::REPORT_ALL_KEYS);
    }
    if kitty_only_functional_code(key).is_some()
        || has_unsupported_legacy_modifiers(event.modifiers)
    {
        return true;
    }
    if flags.contains(KittyKeyboardFlags::REPORT_ALL_KEYS) {
        return true;
    }
    if !flags.intersects(KittyKeyboardFlags::DISAMBIGUATE | KittyKeyboardFlags::REPORT_EVENT_TYPES)
    {
        return false;
    }
    !matches!(key, Key::Enter | Key::Tab | Key::Backtab | Key::Backspace)
}

fn encode_kitty_special(key: &Key, event: &KeyEvent, flags: KittyKeyboardFlags) -> Vec<u8> {
    let modifiers = event_modifiers(key, event.modifiers);
    if let Some(code) = kitty_only_functional_code(key) {
        return encode_csi_u(code, event, flags, modifiers, false, false);
    }
    match key {
        Key::Escape => encode_csi_u(27, event, flags, modifiers, false, false),
        Key::Enter => encode_csi_u(13, event, flags, modifiers, false, false),
        Key::Tab | Key::Backtab => encode_csi_u(9, event, flags, modifiers, false, false),
        Key::Backspace => encode_csi_u(127, event, flags, modifiers, false, false),
        Key::Up => encode_kitty_legacy_shape('A', 1, false, event, flags, modifiers),
        Key::Down => encode_kitty_legacy_shape('B', 1, false, event, flags, modifiers),
        Key::Right => encode_kitty_legacy_shape('C', 1, false, event, flags, modifiers),
        Key::Left => encode_kitty_legacy_shape('D', 1, false, event, flags, modifiers),
        Key::Home => encode_kitty_legacy_shape('H', 1, false, event, flags, modifiers),
        Key::End => encode_kitty_legacy_shape('F', 1, false, event, flags, modifiers),
        Key::Insert => encode_kitty_legacy_shape('~', 2, true, event, flags, modifiers),
        Key::Delete => encode_kitty_legacy_shape('~', 3, true, event, flags, modifiers),
        Key::PageUp => encode_kitty_legacy_shape('~', 5, true, event, flags, modifiers),
        Key::PageDown => encode_kitty_legacy_shape('~', 6, true, event, flags, modifiers),
        Key::Function(1) => encode_kitty_legacy_shape('P', 1, false, event, flags, modifiers),
        Key::Function(2) => encode_kitty_legacy_shape('Q', 1, false, event, flags, modifiers),
        Key::Function(3) => encode_kitty_legacy_shape('~', 13, true, event, flags, modifiers),
        Key::Function(4) => encode_kitty_legacy_shape('S', 1, false, event, flags, modifiers),
        Key::Function(number @ 5..=12) => {
            let parameter = [15, 17, 18, 19, 20, 21, 23, 24][usize::from(*number - 5)];
            encode_kitty_legacy_shape('~', parameter, true, event, flags, modifiers)
        }
        Key::Function(number @ 13..=35) => encode_csi_u(
            57_363 + u32::from(*number),
            event,
            flags,
            modifiers,
            false,
            false,
        ),
        Key::Function(_)
        | Key::CapsLock
        | Key::ScrollLock
        | Key::NumLock
        | Key::PrintScreen
        | Key::Pause
        | Key::Menu
        | Key::Media(_)
        | Key::Modifier(_) => Vec::new(),
        Key::Keypad(keypad) => encode_kitty_keypad(*keypad, event, flags, modifiers, false),
        Key::Character(character) => encode_kitty_character(*character, event, flags),
    }
}

const fn kitty_only_functional_code(key: &Key) -> Option<u32> {
    match key {
        Key::CapsLock => Some(57_358),
        Key::ScrollLock => Some(57_359),
        Key::NumLock => Some(57_360),
        Key::PrintScreen => Some(57_361),
        Key::Pause => Some(57_362),
        Key::Menu => Some(57_363),
        Key::Media(media) => Some(kitty_media_code(*media)),
        Key::Modifier(modifier) => Some(kitty_modifier_key_code(*modifier)),
        _ => None,
    }
}

const fn kitty_media_code(key: MediaKey) -> u32 {
    match key {
        MediaKey::Play => 57_428,
        MediaKey::Pause => 57_429,
        MediaKey::PlayPause => 57_430,
        MediaKey::Reverse => 57_431,
        MediaKey::Stop => 57_432,
        MediaKey::FastForward => 57_433,
        MediaKey::Rewind => 57_434,
        MediaKey::TrackNext => 57_435,
        MediaKey::TrackPrevious => 57_436,
        MediaKey::Record => 57_437,
        MediaKey::LowerVolume => 57_438,
        MediaKey::RaiseVolume => 57_439,
        MediaKey::Mute => 57_440,
    }
}

const fn kitty_modifier_key_code(key: ModifierKey) -> u32 {
    match key {
        ModifierKey::LeftShift => 57_441,
        ModifierKey::LeftControl => 57_442,
        ModifierKey::LeftAlt => 57_443,
        ModifierKey::LeftSuper => 57_444,
        ModifierKey::LeftHyper => 57_445,
        ModifierKey::LeftMeta => 57_446,
        ModifierKey::RightShift => 57_447,
        ModifierKey::RightControl => 57_448,
        ModifierKey::RightAlt => 57_449,
        ModifierKey::RightSuper => 57_450,
        ModifierKey::RightHyper => 57_451,
        ModifierKey::RightMeta => 57_452,
        ModifierKey::IsoLevel3Shift => 57_453,
        ModifierKey::IsoLevel5Shift => 57_454,
    }
}

fn encode_csi_u(
    code: u32,
    event: &KeyEvent,
    flags: KittyKeyboardFlags,
    modifiers: Modifiers,
    text_key: bool,
    include_associated_text: bool,
) -> Vec<u8> {
    let mut sequence = format!("\x1b[{code}");
    let shifted_code = (flags.contains(KittyKeyboardFlags::REPORT_ALTERNATE_KEYS)
        && modifiers.contains(Modifiers::SHIFT))
    .then_some(event.shifted_key)
    .flatten()
    .filter(|shifted| *shifted != code);
    let base_layout = flags
        .contains(KittyKeyboardFlags::REPORT_ALTERNATE_KEYS)
        .then_some(event.base_layout)
        .flatten()
        .filter(|base| *base != code);
    if shifted_code.is_some() || base_layout.is_some() {
        sequence.push(':');
        if let Some(shifted) = shifted_code {
            sequence.push_str(&shifted.to_string());
        }
        if let Some(base) = base_layout {
            sequence.push(':');
            sequence.push_str(&base.to_string());
        }
    }

    let modifier_value = kitty_modifier_value(modifiers, text_key, flags);
    let reports_event = flags.contains(KittyKeyboardFlags::REPORT_EVENT_TYPES);
    let associated = include_associated_text
        && event.kind != KeyEventKind::Release
        && flags.contains(KittyKeyboardFlags::REPORT_ASSOCIATED_TEXT)
        && flags.contains(KittyKeyboardFlags::REPORT_ALL_KEYS)
        && event
            .text
            .as_deref()
            .is_some_and(|text| !associated_codepoints(text).is_empty());

    if modifier_value != 1 || reports_event {
        sequence.push(';');
        sequence.push_str(&modifier_value.to_string());
        if reports_event {
            sequence.push(':');
            sequence.push(event_type(event.kind));
        }
    } else if associated {
        sequence.push(';');
    }
    if associated && let Some(text) = &event.text {
        let codepoints = associated_codepoints(text);
        if !codepoints.is_empty() {
            sequence.push(';');
            sequence.push_str(&codepoints.join(":"));
        }
    }
    sequence.push('u');
    sequence.into_bytes()
}

fn encode_kitty_legacy_shape(
    suffix: char,
    parameter: u32,
    tilde: bool,
    event: &KeyEvent,
    flags: KittyKeyboardFlags,
    modifiers: Modifiers,
) -> Vec<u8> {
    let mut sequence = format!("\x1b[{parameter}");
    let modifier_value = kitty_modifier_value(modifiers, false, flags);
    if modifier_value != 1 || flags.contains(KittyKeyboardFlags::REPORT_EVENT_TYPES) {
        sequence.push(';');
        sequence.push_str(&modifier_value.to_string());
        if flags.contains(KittyKeyboardFlags::REPORT_EVENT_TYPES) {
            sequence.push(':');
            sequence.push(event_type(event.kind));
        }
    } else if !tilde && parameter == 1 {
        sequence.truncate("\x1b[".len());
    }
    sequence.push(if tilde { '~' } else { suffix });
    sequence.into_bytes()
}

fn encode_keypad(
    keypad: KeypadKey,
    event: &KeyEvent,
    kitty_flags: KittyKeyboardFlags,
    application_cursor: bool,
    application_keypad: bool,
) -> Vec<u8> {
    let num_lock_overrides_application = event.modifiers.contains(Modifiers::NUM_LOCK)
        && matches!(
            keypad,
            KeypadKey::Digit(0..=9)
                | KeypadKey::Decimal
                | KeypadKey::Divide
                | KeypadKey::Multiply
                | KeypadKey::Subtract
                | KeypadKey::Add
                | KeypadKey::Enter
                | KeypadKey::Equal
                | KeypadKey::Separator
        );
    let produces_text = keypad_produces_text(keypad);
    let modified = event.modifiers.intersects(
        Modifiers::ALT | Modifiers::CONTROL | Modifiers::SUPER | Modifiers::HYPER | Modifiers::META,
    );
    let application_keypad = application_keypad && !num_lock_overrides_application;
    let uses_kitty = kitty_flags.contains(KittyKeyboardFlags::REPORT_ALL_KEYS)
        || (kitty_flags
            .intersects(KittyKeyboardFlags::DISAMBIGUATE | KittyKeyboardFlags::REPORT_EVENT_TYPES)
            && (application_keypad || !produces_text || modified))
        || has_unsupported_legacy_modifiers(event.modifiers);
    if uses_kitty {
        encode_kitty_keypad(
            keypad,
            event,
            kitty_flags,
            event.modifiers,
            produces_text && !application_keypad && !modified,
        )
    } else {
        encode_legacy_keypad(keypad, event, application_cursor, application_keypad)
    }
}

const fn keypad_produces_text(keypad: KeypadKey) -> bool {
    matches!(
        keypad,
        KeypadKey::Digit(0..=9)
            | KeypadKey::Decimal
            | KeypadKey::Divide
            | KeypadKey::Multiply
            | KeypadKey::Subtract
            | KeypadKey::Add
            | KeypadKey::Equal
            | KeypadKey::Separator
    )
}

fn encode_kitty_keypad(
    keypad: KeypadKey,
    event: &KeyEvent,
    flags: KittyKeyboardFlags,
    modifiers: Modifiers,
    text_key: bool,
) -> Vec<u8> {
    let Some(code) = kitty_keypad_code(keypad) else {
        return Vec::new();
    };
    encode_csi_u(code, event, flags, modifiers, text_key, true)
}

const fn kitty_keypad_code(keypad: KeypadKey) -> Option<u32> {
    match keypad {
        KeypadKey::Digit(digit @ 0..=9) => Some(57_399 + digit as u32),
        KeypadKey::Digit(_) => None,
        KeypadKey::Decimal => Some(57_409),
        KeypadKey::Divide => Some(57_410),
        KeypadKey::Multiply => Some(57_411),
        KeypadKey::Subtract => Some(57_412),
        KeypadKey::Add => Some(57_413),
        KeypadKey::Enter => Some(57_414),
        KeypadKey::Equal => Some(57_415),
        KeypadKey::Separator => Some(57_416),
        KeypadKey::Left => Some(57_417),
        KeypadKey::Right => Some(57_418),
        KeypadKey::Up => Some(57_419),
        KeypadKey::Down => Some(57_420),
        KeypadKey::PageUp => Some(57_421),
        KeypadKey::PageDown => Some(57_422),
        KeypadKey::Home => Some(57_423),
        KeypadKey::End => Some(57_424),
        KeypadKey::Insert => Some(57_425),
        KeypadKey::Delete => Some(57_426),
        KeypadKey::Begin => Some(57_427),
    }
}

fn associated_codepoints(text: &str) -> Vec<String> {
    text.chars()
        .filter(|character| !character.is_control())
        .map(|character| (character as u32).to_string())
        .collect()
}

const fn event_type(kind: KeyEventKind) -> char {
    match kind {
        KeyEventKind::Press => '1',
        KeyEventKind::Repeat => '2',
        KeyEventKind::Release => '3',
    }
}

fn kitty_modifier_value(
    mut modifiers: Modifiers,
    text_key: bool,
    flags: KittyKeyboardFlags,
) -> u16 {
    if flags.is_empty() || (text_key && !flags.contains(KittyKeyboardFlags::REPORT_ALL_KEYS)) {
        modifiers.remove(Modifiers::CAPS_LOCK | Modifiers::NUM_LOCK);
    }
    u16::from(modifiers.bits()) + 1
}

fn has_unsupported_legacy_modifiers(modifiers: Modifiers) -> bool {
    modifiers.intersects(Modifiers::SUPER | Modifiers::HYPER)
}

fn event_modifiers(key: &Key, mut modifiers: Modifiers) -> Modifiers {
    if matches!(key, Key::Backtab) {
        modifiers.insert(Modifiers::SHIFT);
    }
    modifiers
}

fn encode_modify_other_character(
    character: char,
    event: &KeyEvent,
    state: ModifyOtherKeysState,
) -> Option<Vec<u8>> {
    encode_modify_other(character_code(character, event), &event.key, event, state)
}

fn encode_modify_other_key(
    key: &Key,
    event: &KeyEvent,
    state: ModifyOtherKeysState,
) -> Option<Vec<u8>> {
    let code = match key {
        Key::Escape => 27,
        Key::Enter => 13,
        Key::Tab | Key::Backtab => 9,
        Key::Backspace => 8,
        _ => return None,
    };
    encode_modify_other(code, key, event, state)
}

fn encode_modify_other(
    code: u32,
    key: &Key,
    event: &KeyEvent,
    state: ModifyOtherKeysState,
) -> Option<Vec<u8>> {
    if event.kind == KeyEventKind::Release {
        return None;
    }
    let level = state.level?;
    if level == 0 {
        return None;
    }
    let mut modifiers = event_modifiers(key, event.modifiers);
    modifiers.remove(Modifiers::CAPS_LOCK | Modifiers::NUM_LOCK);
    if modifiers.intersects(state.excluded_modifiers)
        && modifiers.difference(state.excluded_modifiers).is_empty()
    {
        return None;
    }
    let has_modifiers = !modifiers.is_empty();
    let alt_like = modifiers.intersects(Modifiers::ALT | Modifiers::META);
    let applies = match level {
        1 => match key {
            Key::Character(_) => {
                alt_like
                    || (modifiers.contains(Modifiers::CONTROL)
                        && (modifiers.contains(Modifiers::SHIFT)
                            || control_character(char::from_u32(code).unwrap_or('\0')).is_none()))
            }
            Key::Tab | Key::Backtab => !modifiers.difference(Modifiers::SHIFT).is_empty(),
            Key::Enter => has_modifiers,
            Key::Escape => alt_like,
            _ => false,
        },
        2 => has_modifiers,
        3.. => true,
        _ => false,
    };
    if !applies {
        return None;
    }
    let modifier_parameter = legacy_modifier(modifiers);
    Some(if state.format == 1 {
        format!("\x1b[{code};{modifier_parameter}u").into_bytes()
    } else {
        format!("\x1b[27;{modifier_parameter};{code}~").into_bytes()
    })
}

fn encode_legacy_character(character: char, event: &KeyEvent) -> Vec<u8> {
    if event.kind == KeyEventKind::Release {
        return Vec::new();
    }
    let mut encoded = Vec::new();
    if event.modifiers.intersects(Modifiers::ALT | Modifiers::META) {
        encoded.push(0x1b);
    }
    if event.modifiers.contains(Modifiers::CONTROL)
        && let Some(control) = control_character(character_code_character(character, event))
    {
        encoded.push(control);
        return encoded;
    }
    if let Some(text) = &event.text {
        encoded.extend_from_slice(text.as_bytes());
    } else {
        let mut buffer = [0; 4];
        encoded.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
    }
    encoded
}

fn character_code(character: char, event: &KeyEvent) -> u32 {
    character_code_character(character, event) as u32
}

fn character_code_character(character: char, event: &KeyEvent) -> char {
    event
        .text
        .as_deref()
        .and_then(|text| text.chars().find(|candidate| !candidate.is_control()))
        .unwrap_or_else(|| {
            if event.modifiers.contains(Modifiers::SHIFT) {
                character.to_uppercase().next().unwrap_or(character)
            } else {
                character
            }
        })
}

fn control_character(character: char) -> Option<u8> {
    let ascii = character.to_ascii_lowercase();
    match ascii {
        '@' | ' ' | '2' => Some(0),
        'a'..='z' => Some(ascii as u8 - b'a' + 1),
        '[' | '3' => Some(27),
        '\\' | '4' => Some(28),
        ']' | '5' => Some(29),
        '^' | '~' | '6' => Some(30),
        '_' | '/' | '7' => Some(31),
        '?' | '8' => Some(127),
        _ => None,
    }
}

fn encode_legacy_special(key: &Key, event: &KeyEvent, application_cursor: bool) -> Vec<u8> {
    if event.kind == KeyEventKind::Release {
        return Vec::new();
    }
    let event_modifiers = event_modifiers(key, event.modifiers);
    let modifiers = legacy_modifier(event_modifiers);
    let plain = modifiers == 1;
    match key {
        Key::Escape => prefix_meta(vec![0x1b], event_modifiers),
        Key::Enter => prefix_meta(vec![b'\r'], event_modifiers),
        Key::Tab | Key::Backtab => encode_legacy_tab(event_modifiers),
        Key::Backspace => prefix_meta(
            vec![if event_modifiers.contains(Modifiers::CONTROL) {
                0x08
            } else {
                0x7f
            }],
            event_modifiers,
        ),
        Key::Up => cursor_key('A', modifiers, plain, application_cursor),
        Key::Down => cursor_key('B', modifiers, plain, application_cursor),
        Key::Right => cursor_key('C', modifiers, plain, application_cursor),
        Key::Left => cursor_key('D', modifiers, plain, application_cursor),
        Key::Home => cursor_key('H', modifiers, plain, application_cursor),
        Key::End => cursor_key('F', modifiers, plain, application_cursor),
        Key::Insert => tilde_key(2, modifiers, plain),
        Key::Delete => tilde_key(3, modifiers, plain),
        Key::PageUp => tilde_key(5, modifiers, plain),
        Key::PageDown => tilde_key(6, modifiers, plain),
        Key::Function(number) => encode_legacy_function(*number, event_modifiers),
        Key::Keypad(_)
        | Key::CapsLock
        | Key::ScrollLock
        | Key::NumLock
        | Key::PrintScreen
        | Key::Pause
        | Key::Menu
        | Key::Media(_)
        | Key::Modifier(_) => Vec::new(),
        Key::Character(character) => encode_legacy_character(*character, event),
    }
}

fn legacy_modifier(modifiers: Modifiers) -> u16 {
    let mut value = 1;
    if modifiers.contains(Modifiers::SHIFT) {
        value += 1;
    }
    if modifiers.contains(Modifiers::ALT) {
        value += 2;
    }
    if modifiers.contains(Modifiers::CONTROL) {
        value += 4;
    }
    if modifiers.intersects(Modifiers::SUPER | Modifiers::META) {
        value += 8;
    }
    value
}

fn encode_legacy_tab(modifiers: Modifiers) -> Vec<u8> {
    if !modifiers.contains(Modifiers::SHIFT) {
        return prefix_meta(vec![b'\t'], modifiers);
    }
    let non_shift =
        modifiers.difference(Modifiers::SHIFT | Modifiers::CAPS_LOCK | Modifiers::NUM_LOCK);
    if non_shift.is_empty() {
        return b"\x1b[Z".to_vec();
    }
    if non_shift.intersects(Modifiers::ALT | Modifiers::META)
        && non_shift
            .difference(Modifiers::ALT | Modifiers::META)
            .is_empty()
    {
        let mut encoded = vec![0x1b];
        encoded.extend_from_slice(b"\x1b[Z");
        return encoded;
    }
    format!("\x1b[9;{}u", legacy_modifier(modifiers)).into_bytes()
}

fn prefix_meta(mut encoded: Vec<u8>, modifiers: Modifiers) -> Vec<u8> {
    if modifiers.intersects(Modifiers::ALT | Modifiers::META) {
        encoded.insert(0, 0x1b);
    }
    encoded
}

fn encode_legacy_function(number: u8, modifiers: Modifiers) -> Vec<u8> {
    let (base, synthetic) = match number {
        1..=12 => (number, Modifiers::empty()),
        13..=24 => (number - 12, Modifiers::SHIFT),
        25..=35 => (number - 24, Modifiers::CONTROL),
        _ => return Vec::new(),
    };
    let modifiers = modifiers | synthetic;
    let modifier = legacy_modifier(modifiers);
    let plain = modifier == 1;
    match base {
        1..=4 if plain => vec![0x1b, b'O', [b'P', b'Q', b'R', b'S'][usize::from(base - 1)]],
        1..=4 => format!(
            "\x1b[1;{modifier}{}",
            ['P', 'Q', 'R', 'S'][usize::from(base - 1)]
        )
        .into_bytes(),
        5..=12 => {
            let parameter = [15, 17, 18, 19, 20, 21, 23, 24][usize::from(base - 5)];
            tilde_key(parameter, modifier, plain)
        }
        _ => Vec::new(),
    }
}

fn encode_legacy_keypad(
    keypad: KeypadKey,
    event: &KeyEvent,
    application_cursor: bool,
    application_keypad: bool,
) -> Vec<u8> {
    if event.kind == KeyEventKind::Release {
        return Vec::new();
    }
    if event.modifiers.contains(Modifiers::NUM_LOCK) {
        match keypad {
            KeypadKey::Digit(digit @ 0..=9) => {
                return encode_legacy_character(char::from(b'0' + digit), event);
            }
            KeypadKey::Decimal => return encode_legacy_character('.', event),
            _ => {}
        }
    }
    if application_keypad && let Some(suffix) = keypad_application_suffix(keypad) {
        let modifier = legacy_modifier(event.modifiers);
        return if modifier == 1 {
            vec![0x1b, b'O', suffix]
        } else {
            format!("\x1b[1;{modifier}{}", suffix as char).into_bytes()
        };
    }

    match keypad {
        KeypadKey::Digit(digit @ 0..=9) => {
            let character = char::from(b'0' + digit);
            encode_legacy_character(character, event)
        }
        KeypadKey::Digit(_) => Vec::new(),
        KeypadKey::Decimal => encode_legacy_character('.', event),
        KeypadKey::Divide => encode_legacy_character('/', event),
        KeypadKey::Multiply => encode_legacy_character('*', event),
        KeypadKey::Subtract => encode_legacy_character('-', event),
        KeypadKey::Add => encode_legacy_character('+', event),
        KeypadKey::Equal => encode_legacy_character('=', event),
        KeypadKey::Separator => encode_legacy_character(',', event),
        KeypadKey::Enter => encode_legacy_special(&Key::Enter, event, application_cursor),
        KeypadKey::Left => encode_legacy_special(&Key::Left, event, application_cursor),
        KeypadKey::Right => encode_legacy_special(&Key::Right, event, application_cursor),
        KeypadKey::Up => encode_legacy_special(&Key::Up, event, application_cursor),
        KeypadKey::Down => encode_legacy_special(&Key::Down, event, application_cursor),
        KeypadKey::PageUp => encode_legacy_special(&Key::PageUp, event, application_cursor),
        KeypadKey::PageDown => encode_legacy_special(&Key::PageDown, event, application_cursor),
        KeypadKey::Home => encode_legacy_special(&Key::Home, event, application_cursor),
        KeypadKey::End => encode_legacy_special(&Key::End, event, application_cursor),
        KeypadKey::Insert => encode_legacy_special(&Key::Insert, event, application_cursor),
        KeypadKey::Delete => encode_legacy_special(&Key::Delete, event, application_cursor),
        KeypadKey::Begin => {
            let modifier = legacy_modifier(event.modifiers);
            cursor_key('E', modifier, modifier == 1, application_cursor)
        }
    }
}

const fn keypad_application_suffix(keypad: KeypadKey) -> Option<u8> {
    match keypad {
        KeypadKey::Digit(digit @ 0..=9) => Some(b'p' + digit),
        KeypadKey::Digit(_)
        | KeypadKey::Left
        | KeypadKey::Right
        | KeypadKey::Up
        | KeypadKey::Down
        | KeypadKey::PageUp
        | KeypadKey::PageDown
        | KeypadKey::Home
        | KeypadKey::End
        | KeypadKey::Insert
        | KeypadKey::Delete
        | KeypadKey::Begin => None,
        KeypadKey::Decimal => Some(b'n'),
        KeypadKey::Divide => Some(b'o'),
        KeypadKey::Multiply => Some(b'j'),
        KeypadKey::Subtract => Some(b'm'),
        KeypadKey::Add => Some(b'k'),
        KeypadKey::Enter => Some(b'M'),
        KeypadKey::Equal => Some(b'X'),
        KeypadKey::Separator => Some(b'l'),
    }
}

fn cursor_key(suffix: char, modifiers: u16, plain: bool, application_cursor: bool) -> Vec<u8> {
    if plain {
        let prefix = if application_cursor { b'O' } else { b'[' };
        vec![0x1b, prefix, suffix as u8]
    } else {
        format!("\x1b[1;{modifiers}{suffix}").into_bytes()
    }
}

fn tilde_key(parameter: u16, modifiers: u16, plain: bool) -> Vec<u8> {
    if plain {
        format!("\x1b[{parameter}~").into_bytes()
    } else {
        format!("\x1b[{parameter};{modifiers}~").into_bytes()
    }
}

#[must_use]
pub fn encode_mouse(event: MouseEvent, sgr: bool, pixels: bool) -> Vec<u8> {
    let mut code = match event.button {
        MouseButton::None => 3,
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        MouseButton::WheelUp => 64,
        MouseButton::WheelDown => 65,
    };
    if event.kind == MouseEventKind::Release && !sgr {
        code = 3;
    } else if event.kind == MouseEventKind::Motion {
        code += 32;
    }
    if event.modifiers.contains(Modifiers::SHIFT) {
        code += 4;
    }
    if event.modifiers.contains(Modifiers::ALT) {
        code += 8;
    }
    if event.modifiers.contains(Modifiers::CONTROL) {
        code += 16;
    }
    let (column, row) = if pixels {
        (
            event.pixel_x.saturating_add(1),
            event.pixel_y.saturating_add(1),
        )
    } else {
        (event.column.saturating_add(1), event.row.saturating_add(1))
    };
    if sgr || pixels {
        let suffix = if event.kind == MouseEventKind::Release {
            'm'
        } else {
            'M'
        };
        format!("\x1b[<{code};{column};{row}{suffix}").into_bytes()
    } else {
        vec![
            0x1b,
            b'[',
            b'M',
            u8::try_from(code + 32).unwrap_or(u8::MAX),
            u8::try_from(column.min(223) + 32).unwrap_or(u8::MAX),
            u8::try_from(row.min(223) + 32).unwrap_or(u8::MAX),
        ]
    }
}

#[must_use]
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    let sanitized = text.replace("\x1b[201~", "").replace("\x1b[200~", "");
    if bracketed {
        let mut encoded = Vec::with_capacity(sanitized.len() + 12);
        encoded.extend_from_slice(b"\x1b[200~");
        encoded.extend_from_slice(sanitized.as_bytes());
        encoded.extend_from_slice(b"\x1b[201~");
        encoded
    } else {
        sanitized.into_bytes()
    }
}
