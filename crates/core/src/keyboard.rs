#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TermyModifiers {
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    pub platform: bool,
    pub function: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TermyKeystroke {
    pub modifiers: TermyModifiers,
    pub key: String,
    pub key_char: Option<String>,
}

pub type Modifiers = TermyModifiers;
pub type Keystroke = TermyKeystroke;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TerminalKeyboardMode {
    application_cursor_keys: bool,
    disambiguate_escape_codes: bool,
    report_event_types: bool,
    report_alternate_keys: bool,
    report_all_keys_as_esc: bool,
    report_associated_text: bool,
}

impl TerminalKeyboardMode {
    pub const fn from_flags(
        application_cursor_keys: bool,
        disambiguate_escape_codes: bool,
        report_event_types: bool,
        report_alternate_keys: bool,
        report_all_keys_as_esc: bool,
        report_associated_text: bool,
    ) -> Self {
        Self {
            application_cursor_keys,
            disambiguate_escape_codes,
            report_event_types,
            report_alternate_keys,
            report_all_keys_as_esc,
            report_associated_text,
        }
    }

    pub fn disambiguate_escape_codes(self) -> bool {
        self.disambiguate_escape_codes
    }

    pub fn report_event_types(self) -> bool {
        self.report_event_types
    }

    pub fn report_all_keys_as_esc(self) -> bool {
        self.report_all_keys_as_esc
    }

    pub fn report_associated_text(self) -> bool {
        self.report_associated_text
    }

    pub fn report_alternate_keys(self) -> bool {
        self.report_alternate_keys
    }

    pub fn enhanced_reporting_active(self) -> bool {
        self.disambiguate_escape_codes || self.report_event_types || self.report_all_keys_as_esc
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalKeyEventKind {
    Press,
    Repeat,
    Release,
}

pub fn keystroke_to_input(
    keystroke: &Keystroke,
    event_kind: TerminalKeyEventKind,
    keyboard_mode: TerminalKeyboardMode,
    prompt_shortcuts_enabled: bool,
) -> Option<Vec<u8>> {
    keystroke_to_input_with_options(
        keystroke,
        event_kind,
        keyboard_mode,
        prompt_shortcuts_enabled,
        false,
    )
}

pub fn keystroke_to_input_with_options(
    keystroke: &Keystroke,
    event_kind: TerminalKeyEventKind,
    keyboard_mode: TerminalKeyboardMode,
    prompt_shortcuts_enabled: bool,
    macos_option_as_alt: bool,
) -> Option<Vec<u8>> {
    // Option-as-Alt only overrides legacy composed-character input. When the
    // kitty keyboard protocol is active the enhanced encoder already reports
    // the base key with the Alt modifier (and paired press/release events), so
    // short-circuiting here would send legacy ESC-prefixed bytes that break
    // the disambiguation TUIs negotiated.
    if macos_option_as_alt
        && !keyboard_mode.enhanced_reporting_active()
        && let Some(input) = macos_option_as_alt_input(keystroke, event_kind)
    {
        return Some(input);
    }

    // "Natural text editing" shortcuts (cmd/alt+arrows, cmd+backspace, etc.) send
    // legacy bytes (Ctrl-U, ESC b, ...) even when the kitty keyboard protocol is
    // active, so they keep working in TUIs that opted into enhanced reporting.
    if matches!(
        event_kind,
        TerminalKeyEventKind::Press | TerminalKeyEventKind::Repeat
    ) && let Some(input) = modified_special_keystroke_input(keystroke, prompt_shortcuts_enabled)
    {
        return Some(input.to_vec());
    }

    if !keyboard_mode.enhanced_reporting_active() {
        return match event_kind {
            TerminalKeyEventKind::Press | TerminalKeyEventKind::Repeat => {
                basic_keystroke_to_input(keystroke, keyboard_mode)
            }
            TerminalKeyEventKind::Release => None,
        };
    }

    enhanced_keystroke_to_input(keystroke, event_kind, keyboard_mode).or_else(|| match event_kind {
        TerminalKeyEventKind::Press | TerminalKeyEventKind::Repeat => {
            basic_keystroke_to_input(keystroke, keyboard_mode)
        }
        TerminalKeyEventKind::Release => None,
    })
}

fn macos_option_as_alt_input(
    keystroke: &Keystroke,
    event_kind: TerminalKeyEventKind,
) -> Option<Vec<u8>> {
    if !cfg!(target_os = "macos")
        || !matches!(
            event_kind,
            TerminalKeyEventKind::Press | TerminalKeyEventKind::Repeat
        )
        || !keystroke.modifiers.alt
        || keystroke.modifiers.control
        || keystroke.modifiers.platform
        || keystroke.modifiers.function
    {
        return None;
    }

    let text = if keystroke.key == "space" {
        " ".to_string()
    } else if keystroke.key.chars().count() == 1 {
        if keystroke.modifiers.shift {
            keystroke.key.to_uppercase()
        } else {
            keystroke.key.clone()
        }
    } else {
        return None;
    };

    let mut input = Vec::with_capacity(1 + text.len());
    input.push(0x1b);
    input.extend_from_slice(text.as_bytes());
    Some(input)
}

fn enhanced_keystroke_to_input(
    keystroke: &Keystroke,
    event_kind: TerminalKeyEventKind,
    keyboard_mode: TerminalKeyboardMode,
) -> Option<Vec<u8>> {
    SequenceBuilder::new(keystroke, event_kind, keyboard_mode).build()
}

fn should_disambiguate_escape_code(key: &str, modifiers: Modifiers) -> bool {
    if key == "escape" {
        return true;
    }

    let only_shift = modifiers.shift
        && !modifiers.control
        && !modifiers.alt
        && !modifiers.platform
        && !modifiers.function;
    !modifiers_are_empty(modifiers) && (!only_shift || matches!(key, "tab" | "enter" | "backspace"))
}

fn modifiers_are_empty(modifiers: Modifiers) -> bool {
    !modifiers.control
        && !modifiers.alt
        && !modifiers.shift
        && !modifiers.platform
        && !modifiers.function
}

fn basic_keystroke_to_input(
    keystroke: &Keystroke,
    keyboard_mode: TerminalKeyboardMode,
) -> Option<Vec<u8>> {
    let key = keystroke.key.as_str();
    let modifiers = keystroke.modifiers;

    let input = match key {
        "enter" => {
            if modifiers.shift {
                Some(vec![b'\n'])
            } else {
                Some(vec![b'\r'])
            }
        }
        "tab" => {
            if modifiers.shift
                && !modifiers.control
                && !modifiers.alt
                && !modifiers.platform
                && !modifiers.function
            {
                Some(b"\x1b[Z".to_vec())
            } else {
                Some(vec![b'\t'])
            }
        }
        "escape" => Some(vec![0x1b]),
        "backspace" => Some(vec![0x7f]),
        "delete" => Some(b"\x1b[3~".to_vec()),
        "up" => Some(legacy_cursor_key_input('A', keyboard_mode, modifiers)),
        "down" => Some(legacy_cursor_key_input('B', keyboard_mode, modifiers)),
        "right" => Some(legacy_cursor_key_input('C', keyboard_mode, modifiers)),
        "left" => Some(legacy_cursor_key_input('D', keyboard_mode, modifiers)),
        "home" => Some(legacy_cursor_key_input('H', keyboard_mode, modifiers)),
        "end" => Some(legacy_cursor_key_input('F', keyboard_mode, modifiers)),
        "pageup" => Some(b"\x1b[5~".to_vec()),
        "pagedown" => Some(b"\x1b[6~".to_vec()),
        "space" => Some(vec![b' ']),
        _ => None,
    };

    if let Some(input) = input {
        return Some(input);
    }

    if let Some(input) = legacy_function_key_input(key, modifiers) {
        return Some(input.to_vec());
    }

    #[cfg(target_os = "windows")]
    if let Some(text) = windows_altgr_text(keystroke) {
        return Some(text.as_bytes().to_vec());
    }

    if modifiers.control && !modifiers.platform && !modifiers.function && key.len() == 1 {
        let c = key.chars().next().unwrap();
        if c.is_ascii_alphabetic() {
            let ctrl_char = (c.to_ascii_lowercase() as u8) - b'a' + 1;
            return Some(vec![ctrl_char]);
        }
        // Handle CTRL with @, [, \, ], ^, _ (standard ASCII control range)
        if matches!(c, '@'..='_') {
            let ctrl_char = (c as u8) & 0x1F;
            return Some(vec![ctrl_char]);
        }
    }

    if !modifiers.control
        && !modifiers.platform
        && !modifiers.function
        && let Some(key_char) = keystroke.key_char.as_deref()
        && !key_char.is_empty()
    {
        return Some(key_char.as_bytes().to_vec());
    }

    if !modifiers.control && !modifiers.platform && !modifiers.function && key.len() == 1 {
        let c = key.chars().next().unwrap();
        if c.is_ascii() {
            return Some(vec![c as u8]);
        }

        let mut buf = [0u8; 4];
        let s = c.encode_utf8(&mut buf);
        return Some(s.as_bytes().to_vec());
    }

    None
}

fn legacy_cursor_key_input(
    suffix: char,
    keyboard_mode: TerminalKeyboardMode,
    modifiers: Modifiers,
) -> Vec<u8> {
    // Full-screen TUIs toggle DECCKM and expect SS3 cursor/home/end sequences
    // only for unmodified keys; modified keys keep the existing legacy fallback.
    let prefix = if modifiers_are_empty(modifiers) && keyboard_mode.application_cursor_keys {
        "\x1bO"
    } else {
        "\x1b["
    };
    format!("{prefix}{suffix}").into_bytes()
}

fn legacy_function_key_input(key: &str, modifiers: Modifiers) -> Option<&'static [u8]> {
    if modifiers.control || modifiers.alt || modifiers.shift || modifiers.platform {
        return None;
    }

    Some(match key {
        "f1" => b"\x1bOP".as_slice(),
        "f2" => b"\x1bOQ".as_slice(),
        "f3" => b"\x1bOR".as_slice(),
        "f4" => b"\x1bOS".as_slice(),
        "f5" => b"\x1b[15~".as_slice(),
        "f6" => b"\x1b[17~".as_slice(),
        "f7" => b"\x1b[18~".as_slice(),
        "f8" => b"\x1b[19~".as_slice(),
        "f9" => b"\x1b[20~".as_slice(),
        "f10" => b"\x1b[21~".as_slice(),
        "f11" => b"\x1b[23~".as_slice(),
        "f12" => b"\x1b[24~".as_slice(),
        _ => return None,
    })
}

fn modifier_or_control_sequence_base(
    keystroke: &Keystroke,
    keyboard_mode: TerminalKeyboardMode,
    modifiers: &mut SequenceModifiers,
) -> Option<SequenceBase> {
    let payload = match keystroke.key.as_str() {
        "tab" => "9",
        "enter" => "13",
        "escape" => "27",
        "space" => "32",
        "backspace" => "127",
        "shift" => {
            if !keyboard_mode.report_all_keys_as_esc() {
                return None;
            }
            modifiers.set(SequenceModifiers::SHIFT, keystroke.modifiers.shift);
            "57447"
        }
        "control" => {
            if !keyboard_mode.report_all_keys_as_esc() {
                return None;
            }
            modifiers.set(SequenceModifiers::CONTROL, keystroke.modifiers.control);
            "57448"
        }
        "alt" => {
            if !keyboard_mode.report_all_keys_as_esc() {
                return None;
            }
            modifiers.set(SequenceModifiers::ALT, keystroke.modifiers.alt);
            "57449"
        }
        "super" | "cmd" => {
            if !keyboard_mode.report_all_keys_as_esc() {
                return None;
            }
            modifiers.set(SequenceModifiers::SUPER, keystroke.modifiers.platform);
            "57450"
        }
        "capslock" => {
            if !keyboard_mode.report_all_keys_as_esc() {
                return None;
            }
            "57358"
        }
        "numlock" => {
            if !keyboard_mode.report_all_keys_as_esc() {
                return None;
            }
            "57360"
        }
        _ => return None,
    };

    Some(SequenceBase::new(payload.to_string(), 'u'))
}

fn textual_sequence_base(
    keystroke: &Keystroke,
    keyboard_mode: TerminalKeyboardMode,
    has_associated_text: bool,
) -> Option<SequenceBase> {
    if keystroke.key.chars().count() == 1 {
        let ch = keystroke.key.chars().next().unwrap();
        let unshifted = unshifted_text_character(keystroke, ch);

        let unicode_key_code = u32::from(unshifted);
        let alternate_key_code = u32::from(ch);
        let payload =
            if keyboard_mode.report_alternate_keys() && alternate_key_code != unicode_key_code {
                format!("{unicode_key_code}:{alternate_key_code}")
            } else {
                unicode_key_code.to_string()
            };

        return Some(SequenceBase::new(payload, 'u'));
    }

    if keyboard_mode.report_all_keys_as_esc() && has_associated_text {
        return Some(SequenceBase::new("0".to_string(), 'u'));
    }

    None
}

fn pure_text_event_text(keystroke: &Keystroke) -> Option<&str> {
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = keystroke;
        None
    }

    #[cfg(target_os = "macos")]
    {
        let modifiers = keystroke.modifiers;
        if !modifiers.alt || modifiers.control || modifiers.platform || modifiers.function {
            return None;
        }

        let text = keystroke.key_char.as_deref()?;
        if text.is_empty() || is_control_character(text) || !text.is_ascii() {
            return None;
        }

        // macOS Option-based layouts surface ASCII code symbols like `@`
        // through the key/text fields without exposing the original base key.
        // Treat these as pure text events so kitty/disambiguate mode doesn't
        // downgrade them into dead Alt+number shortcuts.
        let ch = text.chars().next().unwrap();
        if text != keystroke.key
            || ascii_shifted_symbol_base(ch).is_some()
            || matches!(ch, '[' | ']' | '\\')
        {
            return Some(text);
        }

        None
    }

    #[cfg(target_os = "windows")]
    {
        windows_altgr_text(keystroke)
    }
}

// Windows reports AltGr as a synthetic Left-Ctrl + Right-Alt chord. The OS's
// own key translation (surfaced to us as `key_char`) already resolves that
// combination to the correct composed character (e.g. AltGr+2 -> "@"); a
// genuine Ctrl+Alt shortcut never produces such a `key_char` because Ctrl
// suppresses text composition. Treat a populated, printable `key_char` here
// as authoritative over the control+alt modifier flags instead of
// misreporting it as a dead Ctrl+Alt shortcut.
#[cfg(target_os = "windows")]
fn windows_altgr_text(keystroke: &Keystroke) -> Option<&str> {
    let modifiers = keystroke.modifiers;
    if !modifiers.control || !modifiers.alt || modifiers.platform || modifiers.function {
        return None;
    }

    let text = keystroke.key_char.as_deref()?;
    if text.is_empty() || is_control_character(text) {
        return None;
    }

    Some(text)
}

fn unshifted_text_character(keystroke: &Keystroke, ch: char) -> char {
    if keystroke.modifiers.shift {
        if let Some(unshifted) = ascii_shifted_symbol_base(ch) {
            return unshifted;
        }

        return ch.to_lowercase().next().unwrap_or(ch);
    }

    if let Some(unshifted) = ascii_shifted_symbol_base(ch) {
        return unshifted;
    }

    if ch.is_ascii_uppercase() {
        return ch.to_ascii_lowercase();
    }

    ch
}

fn ascii_shifted_symbol_base(ch: char) -> Option<char> {
    // GPUI does not expose the pre-shift key for symbol keys, so recover the
    // common ASCII base here to keep kitty report-all sequences unambiguous.
    Some(match ch {
        '!' => '1',
        '@' => '2',
        '#' => '3',
        '$' => '4',
        '%' => '5',
        '^' => '6',
        '&' => '7',
        '*' => '8',
        '(' => '9',
        ')' => '0',
        '_' => '-',
        '+' => '=',
        '{' => '[',
        '}' => ']',
        '|' => '\\',
        ':' => ';',
        '"' => '\'',
        '<' => ',',
        '>' => '.',
        '?' => '/',
        '~' => '`',
        _ => return None,
    })
}

fn is_control_character(text: &str) -> bool {
    let codepoint = text.bytes().next().unwrap();
    text.len() == 1 && (codepoint < 0x20 || (0x7f..=0x9f).contains(&codepoint))
}

fn is_basic_named_control_key(key: &str) -> bool {
    matches!(key, "tab" | "enter" | "escape" | "backspace" | "space")
}

fn is_modifier_key(key: &str) -> bool {
    matches!(key, "shift" | "control" | "alt" | "super" | "cmd")
}

fn modified_special_keystroke_input(
    keystroke: &Keystroke,
    prompt_shortcuts_enabled: bool,
) -> Option<&'static [u8]> {
    let key = keystroke.key.as_str();
    let modifiers = keystroke.modifiers;
    #[cfg(target_os = "macos")]
    let _ = prompt_shortcuts_enabled;

    #[cfg(target_os = "macos")]
    {
        if is_plain_alt(modifiers) {
            return match key {
                "left" => Some(b"\x1bb"),
                "right" => Some(b"\x1bf"),
                "backspace" => Some(b"\x1b\x7f"),
                "delete" => Some(b"\x1bd"),
                _ => None,
            };
        }

        if is_plain_platform(modifiers) {
            return match key {
                "left" | "home" => Some(b"\x01"),
                "right" | "end" => Some(b"\x05"),
                "backspace" => Some(b"\x15"),
                "delete" => Some(b"\x0b"),
                _ => None,
            };
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        if prompt_shortcuts_enabled && is_plain_control(modifiers) {
            return match key {
                "left" => Some(b"\x1bb"),
                "right" => Some(b"\x1bf"),
                "backspace" => Some(b"\x17"),
                "delete" => Some(b"\x1bd"),
                _ => None,
            };
        }
    }

    None
}

#[cfg(target_os = "macos")]
#[inline]
fn is_plain_alt(modifiers: Modifiers) -> bool {
    modifiers.alt
        && !modifiers.control
        && !modifiers.platform
        && !modifiers.shift
        && !modifiers.function
}

#[cfg(target_os = "macos")]
#[inline]
fn is_plain_platform(modifiers: Modifiers) -> bool {
    modifiers.platform
        && !modifiers.control
        && !modifiers.alt
        && !modifiers.shift
        && !modifiers.function
}

#[cfg(not(target_os = "macos"))]
#[inline]
fn is_plain_control(modifiers: Modifiers) -> bool {
    modifiers.control
        && !modifiers.platform
        && !modifiers.alt
        && !modifiers.shift
        && !modifiers.function
}

#[derive(Debug, Clone)]
struct SequenceBase {
    payload: String,
    terminator: char,
}

impl SequenceBase {
    fn new(payload: String, terminator: char) -> Self {
        Self {
            payload,
            terminator,
        }
    }
}

struct SequenceBuilder<'a> {
    keystroke: &'a Keystroke,
    event_kind: TerminalKeyEventKind,
    keyboard_mode: TerminalKeyboardMode,
    modifiers: SequenceModifiers,
    pure_text_event: bool,
    include_event_type: bool,
    associated_text: Option<&'a str>,
}

impl<'a> SequenceBuilder<'a> {
    fn new(
        keystroke: &'a Keystroke,
        event_kind: TerminalKeyEventKind,
        keyboard_mode: TerminalKeyboardMode,
    ) -> Self {
        let pure_text_event = pure_text_event_text(keystroke).is_some();
        let include_event_type = keyboard_mode.report_event_types()
            && matches!(
                event_kind,
                TerminalKeyEventKind::Repeat | TerminalKeyEventKind::Release
            );
        let modifiers = if pure_text_event {
            SequenceModifiers::default()
        } else {
            SequenceModifiers::from_modifiers(keystroke.modifiers)
        };

        Self {
            keystroke,
            event_kind,
            keyboard_mode,
            modifiers,
            pure_text_event,
            include_event_type,
            associated_text: associated_text(keystroke, event_kind, keyboard_mode),
        }
    }

    fn build(mut self) -> Option<Vec<u8>> {
        if matches!(self.event_kind, TerminalKeyEventKind::Release)
            && !self.keyboard_mode.report_event_types()
        {
            return None;
        }

        if self.include_event_type
            && !self.keyboard_mode.report_all_keys_as_esc()
            && matches!(self.keystroke.key.as_str(), "enter" | "tab" | "backspace")
        {
            return None;
        }

        if !self.should_build() {
            return None;
        }

        let SequenceBase {
            payload,
            terminator,
        } = self
            .try_build_named()
            .or_else(|| self.try_build_control_char_or_mod())
            .or_else(|| self.try_build_textual())?;

        let mut sequence = format!("\x1b[{payload}");
        if self.include_event_type || !self.modifiers.is_empty() || self.associated_text.is_some() {
            sequence.push_str(&format!(";{}", self.modifiers.encode_esc_sequence()));
        }

        if self.include_event_type {
            sequence.push(':');
            let event_code = match self.event_kind {
                TerminalKeyEventKind::Press => '1',
                TerminalKeyEventKind::Repeat => '2',
                TerminalKeyEventKind::Release => '3',
            };
            sequence.push(event_code);
        }

        if let Some(text) = self.associated_text {
            let mut codepoints = text.chars().map(u32::from);
            if let Some(first) = codepoints.next() {
                sequence.push_str(&format!(";{first}"));
            }
            for codepoint in codepoints {
                sequence.push_str(&format!(":{codepoint}"));
            }
        }

        sequence.push(terminator);
        Some(sequence.into_bytes())
    }

    fn should_build(&self) -> bool {
        if self.pure_text_event {
            return self.keyboard_mode.report_all_keys_as_esc();
        }

        if self.keyboard_mode.report_all_keys_as_esc() {
            return true;
        }

        if matches!(self.event_kind, TerminalKeyEventKind::Release) {
            return self.keyboard_mode.report_event_types();
        }

        if named_sequence_key(self.keystroke.key.as_str()).is_some() {
            return true;
        }

        if is_modifier_key(self.keystroke.key.as_str()) {
            return false;
        }

        if self.keyboard_mode.disambiguate_escape_codes()
            && should_disambiguate_escape_code(
                self.keystroke.key.as_str(),
                self.keystroke.modifiers,
            )
        {
            return true;
        }

        if is_basic_named_control_key(self.keystroke.key.as_str()) {
            return false;
        }

        let has_plain_text = self
            .keystroke
            .key_char
            .as_deref()
            .is_some_and(|text| !text.is_empty())
            || (self.keystroke.key.chars().count() == 1
                && !self.keystroke.modifiers.control
                && !self.keystroke.modifiers.alt
                && !self.keystroke.modifiers.platform
                && !self.keystroke.modifiers.function);
        !has_plain_text
    }

    fn try_build_named(&self) -> Option<SequenceBase> {
        named_sequence_key(self.keystroke.key.as_str()).map(|named_sequence| {
            named_sequence.sequence_base(
                self.modifiers,
                self.associated_text.is_some(),
                self.include_event_type,
            )
        })
    }

    fn try_build_control_char_or_mod(&mut self) -> Option<SequenceBase> {
        modifier_or_control_sequence_base(self.keystroke, self.keyboard_mode, &mut self.modifiers)
    }

    fn try_build_textual(&self) -> Option<SequenceBase> {
        if self.pure_text_event {
            return Some(SequenceBase::new("0".to_string(), 'u'));
        }

        textual_sequence_base(
            self.keystroke,
            self.keyboard_mode,
            self.associated_text.is_some(),
        )
    }
}

fn associated_text(
    keystroke: &Keystroke,
    event_kind: TerminalKeyEventKind,
    keyboard_mode: TerminalKeyboardMode,
) -> Option<&str> {
    // `associated_text` is only part of kitty's report-all protocol, so
    // `Keystroke.key_char` must stay out of legacy/non-report-all sequences.
    if !keyboard_mode.report_all_keys_as_esc()
        || !keyboard_mode.report_associated_text()
        || matches!(event_kind, TerminalKeyEventKind::Release)
    {
        return None;
    }

    let text = keystroke.key_char.as_deref()?;
    if text.is_empty() || is_control_character(text) {
        return None;
    }
    Some(text)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NamedSequenceKey {
    OneBased(char),
    Tilde(&'static str),
    Kitty(&'static str, char),
}

impl NamedSequenceKey {
    fn sequence_base(
        self,
        modifiers: SequenceModifiers,
        has_associated_text: bool,
        include_event_type: bool,
    ) -> SequenceBase {
        match self {
            Self::OneBased(terminator) => {
                let payload = if modifiers.is_empty() && !has_associated_text && !include_event_type
                {
                    String::new()
                } else {
                    "1".to_string()
                };
                SequenceBase::new(payload, terminator)
            }
            Self::Tilde(payload) => SequenceBase::new(payload.to_string(), '~'),
            Self::Kitty(payload, terminator) => SequenceBase::new(payload.to_string(), terminator),
        }
    }
}

fn named_sequence_key(key: &str) -> Option<NamedSequenceKey> {
    Some(match key {
        "pageup" => NamedSequenceKey::Tilde("5"),
        "pagedown" => NamedSequenceKey::Tilde("6"),
        "delete" => NamedSequenceKey::Tilde("3"),
        "insert" => NamedSequenceKey::Tilde("2"),
        "home" => NamedSequenceKey::OneBased('H'),
        "end" => NamedSequenceKey::OneBased('F'),
        "left" => NamedSequenceKey::OneBased('D'),
        "right" => NamedSequenceKey::OneBased('C'),
        "up" => NamedSequenceKey::OneBased('A'),
        "down" => NamedSequenceKey::OneBased('B'),
        "f1" => NamedSequenceKey::OneBased('P'),
        "f2" => NamedSequenceKey::OneBased('Q'),
        // F3 diverges from the legacy xterm table in kitty mode.
        "f3" => NamedSequenceKey::Kitty("13", '~'),
        "f4" => NamedSequenceKey::OneBased('S'),
        "f5" => NamedSequenceKey::Tilde("15"),
        "f6" => NamedSequenceKey::Tilde("17"),
        "f7" => NamedSequenceKey::Tilde("18"),
        "f8" => NamedSequenceKey::Tilde("19"),
        "f9" => NamedSequenceKey::Tilde("20"),
        "f10" => NamedSequenceKey::Tilde("21"),
        "f11" => NamedSequenceKey::Tilde("23"),
        "f12" => NamedSequenceKey::Tilde("24"),
        "f13" => NamedSequenceKey::Kitty("57376", 'u'),
        "f14" => NamedSequenceKey::Kitty("57377", 'u'),
        "f15" => NamedSequenceKey::Kitty("57378", 'u'),
        "f16" => NamedSequenceKey::Kitty("57379", 'u'),
        "f17" => NamedSequenceKey::Kitty("57380", 'u'),
        "f18" => NamedSequenceKey::Kitty("57381", 'u'),
        "f19" => NamedSequenceKey::Kitty("57382", 'u'),
        "f20" => NamedSequenceKey::Kitty("57383", 'u'),
        "f21" => NamedSequenceKey::Kitty("57384", 'u'),
        "f22" => NamedSequenceKey::Kitty("57385", 'u'),
        "f23" => NamedSequenceKey::Kitty("57386", 'u'),
        "f24" => NamedSequenceKey::Kitty("57387", 'u'),
        "f25" => NamedSequenceKey::Kitty("57388", 'u'),
        "f26" => NamedSequenceKey::Kitty("57389", 'u'),
        "f27" => NamedSequenceKey::Kitty("57390", 'u'),
        "f28" => NamedSequenceKey::Kitty("57391", 'u'),
        "f29" => NamedSequenceKey::Kitty("57392", 'u'),
        "f30" => NamedSequenceKey::Kitty("57393", 'u'),
        "f31" => NamedSequenceKey::Kitty("57394", 'u'),
        "f32" => NamedSequenceKey::Kitty("57395", 'u'),
        "f33" => NamedSequenceKey::Kitty("57396", 'u'),
        "f34" => NamedSequenceKey::Kitty("57397", 'u'),
        "f35" => NamedSequenceKey::Kitty("57398", 'u'),
        "scrolllock" => NamedSequenceKey::Kitty("57359", 'u'),
        "printscreen" => NamedSequenceKey::Kitty("57361", 'u'),
        "pause" => NamedSequenceKey::Kitty("57362", 'u'),
        "menu" => NamedSequenceKey::Kitty("57363", 'u'),
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct SequenceModifiers(u8);

impl SequenceModifiers {
    const SHIFT: u8 = 1 << 0;
    const ALT: u8 = 1 << 1;
    const CONTROL: u8 = 1 << 2;
    const SUPER: u8 = 1 << 3;

    fn from_modifiers(modifiers: Modifiers) -> Self {
        let mut encoded = 0;
        if modifiers.shift {
            encoded |= Self::SHIFT;
        }
        if modifiers.alt {
            encoded |= Self::ALT;
        }
        if modifiers.control {
            encoded |= Self::CONTROL;
        }
        if modifiers.platform {
            encoded |= Self::SUPER;
        }
        Self(encoded)
    }

    fn encode_esc_sequence(self) -> u8 {
        self.0 + 1
    }

    fn is_empty(self) -> bool {
        self.0 == 0
    }

    fn set(&mut self, flag: u8, enabled: bool) {
        if enabled {
            self.0 |= flag;
        } else {
            self.0 &= !flag;
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    use super::pure_text_event_text;
    use super::{
        Keystroke, Modifiers, TerminalKeyEventKind, TerminalKeyboardMode, associated_text,
        keystroke_to_input, keystroke_to_input_with_options,
    };

    fn keystroke(key: &str, key_char: Option<&str>, modifiers: Modifiers) -> Keystroke {
        Keystroke {
            modifiers,
            key: key.to_string(),
            key_char: key_char.map(str::to_string),
        }
    }

    fn report_all_mode() -> TerminalKeyboardMode {
        TerminalKeyboardMode {
            report_all_keys_as_esc: true,
            ..TerminalKeyboardMode::default()
        }
    }

    fn report_all_with_event_types_mode() -> TerminalKeyboardMode {
        TerminalKeyboardMode {
            report_all_keys_as_esc: true,
            report_event_types: true,
            ..TerminalKeyboardMode::default()
        }
    }

    fn disambiguate_mode() -> TerminalKeyboardMode {
        TerminalKeyboardMode {
            disambiguate_escape_codes: true,
            ..TerminalKeyboardMode::default()
        }
    }

    fn application_cursor_mode() -> TerminalKeyboardMode {
        TerminalKeyboardMode {
            application_cursor_keys: true,
            ..TerminalKeyboardMode::default()
        }
    }

    #[test]
    fn enhanced_mode_reports_modifier_only_press() {
        let modifiers = Modifiers {
            platform: true,
            ..Modifiers::default()
        };

        assert_eq!(
            keystroke_to_input(
                &keystroke("super", None, modifiers),
                TerminalKeyEventKind::Press,
                report_all_mode(),
                true,
            ),
            Some(b"\x1b[57450;9u".to_vec())
        );
    }

    #[test]
    fn enhanced_mode_reports_modifier_only_release_with_event_types() {
        let modifiers = Modifiers::default();

        assert_eq!(
            keystroke_to_input(
                &keystroke("super", None, modifiers),
                TerminalKeyEventKind::Release,
                report_all_with_event_types_mode(),
                true,
            ),
            Some(b"\x1b[57450;1:3u".to_vec())
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn enhanced_mode_preserves_natural_editing_shortcuts() {
        let modifiers = Modifiers {
            platform: true,
            ..Modifiers::default()
        };

        assert_eq!(
            keystroke_to_input(
                &keystroke("left", None, modifiers),
                TerminalKeyEventKind::Press,
                disambiguate_mode(),
                true,
            ),
            Some(b"\x01".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("backspace", None, modifiers),
                TerminalKeyEventKind::Press,
                disambiguate_mode(),
                true,
            ),
            Some(b"\x15".to_vec())
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn enhanced_mode_does_not_remap_platform_modifier() {
        let modifiers = Modifiers {
            platform: true,
            ..Modifiers::default()
        };

        assert_eq!(
            keystroke_to_input(
                &keystroke("left", None, modifiers),
                TerminalKeyEventKind::Press,
                disambiguate_mode(),
                true,
            ),
            Some(b"\x1b[1;9D".to_vec())
        );
    }

    #[test]
    fn enhanced_mode_reports_text_key_releases() {
        let modifiers = Modifiers {
            platform: true,
            shift: true,
            ..Modifiers::default()
        };

        assert_eq!(
            keystroke_to_input(
                &keystroke("t", None, modifiers),
                TerminalKeyEventKind::Release,
                TerminalKeyboardMode {
                    disambiguate_escape_codes: true,
                    report_event_types: true,
                    ..TerminalKeyboardMode::default()
                },
                true,
            ),
            Some(b"\x1b[116;10:3u".to_vec())
        );
    }

    #[test]
    fn enhanced_mode_reports_arrow_releases_with_one_based_payload() {
        assert_eq!(
            keystroke_to_input(
                &keystroke("left", None, Modifiers::default()),
                TerminalKeyEventKind::Release,
                TerminalKeyboardMode {
                    report_event_types: true,
                    ..TerminalKeyboardMode::default()
                },
                true,
            ),
            Some(b"\x1b[1;1:3D".to_vec())
        );
    }

    #[test]
    fn enhanced_mode_skips_enter_release_without_report_all_keys() {
        assert_eq!(
            keystroke_to_input(
                &keystroke("enter", None, Modifiers::default()),
                TerminalKeyEventKind::Release,
                TerminalKeyboardMode {
                    report_event_types: true,
                    ..TerminalKeyboardMode::default()
                },
                true,
            ),
            None
        );
    }

    #[test]
    fn legacy_mode_keeps_plain_control_bytes() {
        let modifiers = Modifiers {
            control: true,
            ..Modifiers::default()
        };

        assert_eq!(
            keystroke_to_input(
                &keystroke("t", None, modifiers),
                TerminalKeyEventKind::Press,
                TerminalKeyboardMode::default(),
                true,
            ),
            Some(vec![0x14])
        );
    }

    #[test]
    fn legacy_mode_ctrl_right_bracket() {
        let modifiers = Modifiers {
            control: true,
            ..Modifiers::default()
        };

        assert_eq!(
            keystroke_to_input(
                &keystroke("]", None, modifiers),
                TerminalKeyEventKind::Press,
                TerminalKeyboardMode::default(),
                true,
            ),
            Some(vec![0x1D])
        );
    }

    #[test]
    fn legacy_mode_reports_plain_shift_tab_as_backtab() {
        let modifiers = Modifiers {
            shift: true,
            ..Modifiers::default()
        };

        assert_eq!(
            keystroke_to_input(
                &keystroke("tab", None, modifiers),
                TerminalKeyEventKind::Press,
                TerminalKeyboardMode::default(),
                true,
            ),
            Some(b"\x1b[Z".to_vec())
        );
    }

    #[test]
    fn legacy_mode_reports_plain_left_with_normal_cursor_keys() {
        assert_eq!(
            keystroke_to_input(
                &keystroke("left", None, Modifiers::default()),
                TerminalKeyEventKind::Press,
                TerminalKeyboardMode::default(),
                true,
            ),
            Some(b"\x1b[D".to_vec())
        );
    }

    #[test]
    fn legacy_mode_reports_plain_left_with_application_cursor_keys() {
        assert_eq!(
            keystroke_to_input(
                &keystroke("left", None, Modifiers::default()),
                TerminalKeyEventKind::Press,
                application_cursor_mode(),
                true,
            ),
            Some(b"\x1bOD".to_vec())
        );
    }

    #[test]
    fn legacy_mode_reports_plain_up_with_application_cursor_keys() {
        assert_eq!(
            keystroke_to_input(
                &keystroke("up", None, Modifiers::default()),
                TerminalKeyEventKind::Press,
                application_cursor_mode(),
                true,
            ),
            Some(b"\x1bOA".to_vec())
        );
    }

    #[test]
    fn legacy_mode_reports_plain_home_with_normal_cursor_keys() {
        assert_eq!(
            keystroke_to_input(
                &keystroke("home", None, Modifiers::default()),
                TerminalKeyEventKind::Press,
                TerminalKeyboardMode::default(),
                true,
            ),
            Some(b"\x1b[H".to_vec())
        );
    }

    #[test]
    fn legacy_mode_reports_plain_home_with_application_cursor_keys() {
        assert_eq!(
            keystroke_to_input(
                &keystroke("home", None, Modifiers::default()),
                TerminalKeyEventKind::Press,
                application_cursor_mode(),
                true,
            ),
            Some(b"\x1bOH".to_vec())
        );
    }

    #[test]
    fn legacy_mode_reports_plain_end_with_application_cursor_keys() {
        assert_eq!(
            keystroke_to_input(
                &keystroke("end", None, Modifiers::default()),
                TerminalKeyEventKind::Press,
                application_cursor_mode(),
                true,
            ),
            Some(b"\x1bOF".to_vec())
        );
    }

    #[test]
    fn legacy_mode_reports_plain_function_keys() {
        let cases = [
            ("f1", b"\x1bOP".as_slice()),
            ("f2", b"\x1bOQ".as_slice()),
            ("f3", b"\x1bOR".as_slice()),
            ("f4", b"\x1bOS".as_slice()),
            ("f5", b"\x1b[15~".as_slice()),
            ("f6", b"\x1b[17~".as_slice()),
            ("f7", b"\x1b[18~".as_slice()),
            ("f8", b"\x1b[19~".as_slice()),
            ("f9", b"\x1b[20~".as_slice()),
            ("f10", b"\x1b[21~".as_slice()),
            ("f11", b"\x1b[23~".as_slice()),
            ("f12", b"\x1b[24~".as_slice()),
        ];

        for (key, expected) in cases {
            assert_eq!(
                keystroke_to_input(
                    &keystroke(key, None, Modifiers::default()),
                    TerminalKeyEventKind::Press,
                    TerminalKeyboardMode::default(),
                    true,
                ),
                Some(expected.to_vec()),
                "{key} should use the xterm-256color function-key sequence"
            );
        }
    }

    #[test]
    fn legacy_mode_treats_fn_as_hardware_modifier_for_function_keys() {
        let modifiers = Modifiers {
            function: true,
            ..Modifiers::default()
        };

        assert_eq!(
            keystroke_to_input(
                &keystroke("f1", None, modifiers),
                TerminalKeyEventKind::Press,
                TerminalKeyboardMode::default(),
                true,
            ),
            Some(b"\x1bOP".to_vec())
        );
    }

    #[test]
    fn legacy_mode_does_not_emit_modified_function_keys() {
        let modifiers = Modifiers {
            shift: true,
            ..Modifiers::default()
        };

        assert_eq!(
            keystroke_to_input(
                &keystroke("f1", None, modifiers),
                TerminalKeyEventKind::Press,
                TerminalKeyboardMode::default(),
                true,
            ),
            None
        );
    }

    #[test]
    fn kitty_mode_reports_f3_with_protocol_sequence() {
        assert_eq!(
            keystroke_to_input(
                &keystroke("f3", None, Modifiers::default()),
                TerminalKeyEventKind::Press,
                disambiguate_mode(),
                true,
            ),
            Some(b"\x1b[13~".to_vec())
        );
    }

    #[test]
    fn kitty_mode_reports_high_function_keys_with_private_use_codes() {
        assert_eq!(
            keystroke_to_input(
                &keystroke("f13", None, Modifiers::default()),
                TerminalKeyEventKind::Press,
                disambiguate_mode(),
                true,
            ),
            Some(b"\x1b[57376u".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("f24", None, Modifiers::default()),
                TerminalKeyEventKind::Press,
                disambiguate_mode(),
                true,
            ),
            Some(b"\x1b[57387u".to_vec())
        );
    }

    #[test]
    fn report_all_mode_uses_unshifted_ascii_base_for_shifted_symbols() {
        assert_eq!(
            keystroke_to_input(
                &keystroke("!", Some("!"), Modifiers::default()),
                TerminalKeyEventKind::Press,
                report_all_mode(),
                true,
            ),
            Some(b"\x1b[49u".to_vec())
        );
    }

    #[test]
    fn report_alternate_keys_includes_shifted_symbol_alternate_code() {
        assert_eq!(
            keystroke_to_input(
                &keystroke("!", Some("!"), Modifiers::default()),
                TerminalKeyEventKind::Press,
                TerminalKeyboardMode {
                    report_all_keys_as_esc: true,
                    report_alternate_keys: true,
                    ..TerminalKeyboardMode::default()
                },
                true,
            ),
            Some(b"\x1b[49:33u".to_vec())
        );
    }

    #[test]
    fn augment_only_flags_do_not_force_kitty_sequences() {
        assert_eq!(
            keystroke_to_input(
                &keystroke("left", None, Modifiers::default()),
                TerminalKeyEventKind::Press,
                TerminalKeyboardMode {
                    report_alternate_keys: true,
                    ..TerminalKeyboardMode::default()
                },
                true,
            ),
            Some(b"\x1b[D".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("a", Some("a"), Modifiers::default()),
                TerminalKeyEventKind::Press,
                TerminalKeyboardMode {
                    report_associated_text: true,
                    ..TerminalKeyboardMode::default()
                },
                true,
            ),
            Some(b"a".to_vec())
        );
    }

    #[test]
    fn associated_text_requires_report_all_mode() {
        assert_eq!(
            associated_text(
                &keystroke("a", Some("a"), Modifiers::default()),
                TerminalKeyEventKind::Press,
                TerminalKeyboardMode {
                    report_associated_text: true,
                    ..TerminalKeyboardMode::default()
                },
            ),
            None
        );
    }

    #[test]
    fn associated_text_uses_key_char_in_report_all_mode() {
        assert_eq!(
            associated_text(
                &keystroke("a", Some("a"), Modifiers::default()),
                TerminalKeyEventKind::Press,
                TerminalKeyboardMode {
                    report_all_keys_as_esc: true,
                    report_associated_text: true,
                    ..TerminalKeyboardMode::default()
                },
            ),
            Some("a")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_option_layout_ascii_symbol_uses_pure_text_path() {
        let modifiers = Modifiers {
            alt: true,
            ..Modifiers::default()
        };

        assert_eq!(
            pure_text_event_text(&keystroke("@", Some("@"), modifiers)),
            Some("@")
        );
        assert_eq!(
            pure_text_event_text(&keystroke("2", Some("@"), modifiers)),
            Some("@")
        );
        assert_eq!(
            pure_text_event_text(&keystroke("[", Some("["), modifiers)),
            Some("[")
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_altgr_chord_uses_pure_text_path() {
        let modifiers = Modifiers {
            control: true,
            alt: true,
            ..Modifiers::default()
        };

        // AltGr+2 on a layout where that combo produces "@" (e.g. Danish).
        assert_eq!(
            pure_text_event_text(&keystroke("2", Some("@"), modifiers)),
            Some("@")
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_real_ctrl_alt_chord_is_not_treated_as_text() {
        let modifiers = Modifiers {
            control: true,
            alt: true,
            ..Modifiers::default()
        };

        // A genuine Ctrl+Alt shortcut never has a composed key_char.
        assert_eq!(pure_text_event_text(&keystroke("2", None, modifiers)), None);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_altgr_chord_falls_back_to_literal_text_without_report_all_keys() {
        let modifiers = Modifiers {
            control: true,
            alt: true,
            ..Modifiers::default()
        };

        assert_eq!(
            keystroke_to_input(
                &keystroke("2", Some("@"), modifiers),
                TerminalKeyEventKind::Press,
                TerminalKeyboardMode {
                    disambiguate_escape_codes: true,
                    ..TerminalKeyboardMode::default()
                },
                false,
            ),
            Some(b"@".to_vec())
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_option_layout_ascii_symbol_falls_back_to_utf8_in_disambiguate_mode() {
        let modifiers = Modifiers {
            alt: true,
            ..Modifiers::default()
        };

        assert_eq!(
            keystroke_to_input(
                &keystroke("@", Some("@"), modifiers),
                TerminalKeyEventKind::Press,
                disambiguate_mode(),
                true,
            ),
            Some(b"@".to_vec())
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_option_layout_ascii_symbol_uses_key_zero_in_report_all_mode() {
        let modifiers = Modifiers {
            alt: true,
            ..Modifiers::default()
        };

        assert_eq!(
            keystroke_to_input(
                &keystroke("@", Some("@"), modifiers),
                TerminalKeyEventKind::Press,
                TerminalKeyboardMode {
                    report_all_keys_as_esc: true,
                    report_associated_text: true,
                    ..TerminalKeyboardMode::default()
                },
                true,
            ),
            Some(b"\x1b[0;1;64u".to_vec())
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_option_as_alt_emits_escape_prefixed_printable_key() {
        let modifiers = Modifiers {
            alt: true,
            ..Modifiers::default()
        };

        assert_eq!(
            keystroke_to_input_with_options(
                &keystroke("b", Some("∫"), modifiers),
                TerminalKeyEventKind::Press,
                TerminalKeyboardMode::default(),
                true,
                true,
            ),
            Some(b"\x1bb".to_vec())
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_option_as_alt_emits_escape_prefixed_space() {
        let modifiers = Modifiers {
            alt: true,
            ..Modifiers::default()
        };

        assert_eq!(
            keystroke_to_input_with_options(
                &keystroke("space", Some(" "), modifiers),
                TerminalKeyEventKind::Press,
                TerminalKeyboardMode::default(),
                true,
                true,
            ),
            Some(b"\x1b ".to_vec())
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_option_as_alt_defers_to_enhanced_reporting() {
        let modifiers = Modifiers {
            alt: true,
            ..Modifiers::default()
        };
        let keyboard_mode = TerminalKeyboardMode {
            disambiguate_escape_codes: true,
            ..TerminalKeyboardMode::default()
        };

        assert_eq!(
            keystroke_to_input_with_options(
                &keystroke("b", Some("∫"), modifiers),
                TerminalKeyEventKind::Press,
                keyboard_mode,
                true,
                true,
            ),
            Some(b"\x1b[98;3u".to_vec())
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_option_as_alt_keeps_press_release_paired_in_event_type_mode() {
        let modifiers = Modifiers {
            alt: true,
            ..Modifiers::default()
        };
        let keyboard_mode = TerminalKeyboardMode {
            disambiguate_escape_codes: true,
            report_event_types: true,
            ..TerminalKeyboardMode::default()
        };

        assert_eq!(
            keystroke_to_input_with_options(
                &keystroke("b", Some("∫"), modifiers),
                TerminalKeyEventKind::Press,
                keyboard_mode,
                true,
                true,
            ),
            Some(b"\x1b[98;3u".to_vec())
        );
        assert_eq!(
            keystroke_to_input_with_options(
                &keystroke("b", None, modifiers),
                TerminalKeyEventKind::Release,
                keyboard_mode,
                true,
                true,
            ),
            Some(b"\x1b[98;3:3u".to_vec())
        );
    }
}
