//! Legacy (xterm-compatible) key encoding, used when no kitty keyboard flags
//! that change the encoding are active.

use super::{Keystroke, Modifiers, SequenceModifiers, TerminalKeyboardMode};

/// Named keys that xterm encodes as CSI/SS3 sequences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpecialKey {
    /// Cursor keys, Home and End: `CSI X`, `SS3 X` under DECCKM, `CSI 1;m X`.
    Cursor(u8),
    /// F1-F4: `SS3 X` unmodified, `CSI 1;m X` with modifiers.
    Ss3Function(u8),
    /// Editing keys and F5-F12: `CSI n ~`, `CSI n;m ~`.
    Tilde(u8),
}

/// `native` is false for [`super::portable_keystroke_to_input`], which
/// encodes as Linux does on every platform.
pub(super) fn basic_keystroke_to_input(
    keystroke: &Keystroke,
    keyboard_mode: TerminalKeyboardMode,
    native: bool,
) -> Option<Vec<u8>> {
    let modifiers = keystroke.modifiers;

    if let Some(special_key) = special_key(keystroke.key.as_str()) {
        return Some(special_key_input(special_key, modifiers, keyboard_mode));
    }

    if let Some((numeric_key, application_final)) = keypad_key(keystroke.key.as_str()) {
        if keyboard_mode.application_keypad() && !has_xterm_modifiers(modifiers) {
            return Some(vec![0x1b, b'O', application_final]);
        }

        // DECKPNM (or a modified keypad key): send what the main key sends.
        let numeric = Keystroke {
            modifiers,
            key: numeric_key.to_string(),
            key_char: keystroke.key_char.clone(),
        };
        return text_keystroke_input(&numeric, native);
    }

    text_keystroke_input(keystroke, native)
}

fn text_keystroke_input(keystroke: &Keystroke, native: bool) -> Option<Vec<u8>> {
    #[cfg(target_os = "windows")]
    if native && let Some(text) = super::windows_altgr_text(keystroke) {
        return Some(text.as_bytes().to_vec());
    }

    let input = unprefixed_text_input(keystroke)?;
    Some(with_alt_escape_prefix(input, keystroke.modifiers, native))
}

fn unprefixed_text_input(keystroke: &Keystroke) -> Option<Vec<u8>> {
    let key = keystroke.key.as_str();
    let modifiers = keystroke.modifiers;

    match key {
        "enter" => return Some(vec![if modifiers.shift { b'\n' } else { b'\r' }]),
        "tab" => {
            let only_shift = modifiers.shift
                && !modifiers.control
                && !modifiers.alt
                && !modifiers.platform
                && !modifiers.function;
            return Some(if only_shift {
                b"\x1b[Z".to_vec()
            } else {
                vec![b'\t']
            });
        }
        "escape" => return Some(vec![0x1b]),
        "backspace" => return Some(vec![0x7f]),
        // Ctrl+Space sends NUL like xterm (emacs `C-SPC`, tmux `C-Space`).
        "space" if is_control_chord(modifiers) => return Some(vec![0x00]),
        // Text the layout composed on Space wins, as for any other key: on
        // macOS, Option+Space types U+00A0 unless Option is Alt.
        "space" => {
            let composed = keystroke.key_char.as_deref().filter(|text| {
                !text.is_empty() && !modifiers.control && !modifiers.platform && !modifiers.function
            });
            return Some(composed.map_or_else(|| vec![b' '], |text| text.as_bytes().to_vec()));
        }
        _ => {}
    }

    if is_control_chord(modifiers) && key.len() == 1 {
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
        // xterm's legacy mappings for the digit row and a few symbols, e.g.
        // readline undo on Ctrl+/.
        let ctrl_char = match c {
            '2' => 0x00,
            '3' => 0x1b,
            '4' => 0x1c,
            '5' => 0x1d,
            '6' => 0x1e,
            '7' | '/' | '-' => 0x1f,
            '8' | '?' => 0x7f,
            _ => return None,
        };
        return Some(vec![ctrl_char]);
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

fn is_control_chord(modifiers: Modifiers) -> bool {
    modifiers.control && !modifiers.platform && !modifiers.function
}

/// Alt sends ESC before the key's bytes (xterm `metaSendsEscape`). macOS
/// leaves Option to compose text unless Option-as-Alt is enabled, which is
/// handled before this encoder runs. Browser builds can't tell whether the
/// host is a Mac, so they keep Alt text unprefixed too. Portable encoding
/// (`native` false) always prefixes.
fn with_alt_escape_prefix(input: Vec<u8>, modifiers: Modifiers, native: bool) -> Vec<u8> {
    if (native && cfg!(any(target_os = "macos", target_family = "wasm")))
        || !modifiers.alt
        || modifiers.platform
    {
        return input;
    }

    let mut prefixed = Vec::with_capacity(input.len() + 1);
    prefixed.push(0x1b);
    prefixed.extend_from_slice(&input);
    prefixed
}

fn special_key(key: &str) -> Option<SpecialKey> {
    Some(match key {
        "up" => SpecialKey::Cursor(b'A'),
        "down" => SpecialKey::Cursor(b'B'),
        "right" => SpecialKey::Cursor(b'C'),
        "left" => SpecialKey::Cursor(b'D'),
        "home" => SpecialKey::Cursor(b'H'),
        "end" => SpecialKey::Cursor(b'F'),
        "insert" => SpecialKey::Tilde(2),
        "delete" => SpecialKey::Tilde(3),
        "pageup" => SpecialKey::Tilde(5),
        "pagedown" => SpecialKey::Tilde(6),
        "f1" => SpecialKey::Ss3Function(b'P'),
        "f2" => SpecialKey::Ss3Function(b'Q'),
        "f3" => SpecialKey::Ss3Function(b'R'),
        "f4" => SpecialKey::Ss3Function(b'S'),
        "f5" => SpecialKey::Tilde(15),
        "f6" => SpecialKey::Tilde(17),
        "f7" => SpecialKey::Tilde(18),
        "f8" => SpecialKey::Tilde(19),
        "f9" => SpecialKey::Tilde(20),
        "f10" => SpecialKey::Tilde(21),
        "f11" => SpecialKey::Tilde(23),
        "f12" => SpecialKey::Tilde(24),
        _ => return None,
    })
}

fn special_key_input(
    special_key: SpecialKey,
    modifiers: Modifiers,
    keyboard_mode: TerminalKeyboardMode,
) -> Vec<u8> {
    // xterm's modifier parameter is 1 + shift(1) + alt(2) + ctrl(4) + meta(8).
    // Fn is a hardware modifier and is not reported.
    let sequence_modifiers = SequenceModifiers::from_modifiers(modifiers);
    let parameter =
        (!sequence_modifiers.is_empty()).then(|| sequence_modifiers.encode_esc_sequence());

    match (special_key, parameter) {
        (SpecialKey::Cursor(suffix), None) if keyboard_mode.application_cursor_keys => {
            vec![0x1b, b'O', suffix]
        }
        (SpecialKey::Cursor(suffix), None) => vec![0x1b, b'[', suffix],
        (SpecialKey::Ss3Function(suffix), None) => vec![0x1b, b'O', suffix],
        (SpecialKey::Cursor(suffix) | SpecialKey::Ss3Function(suffix), Some(parameter)) => {
            format!("\x1b[1;{parameter}{}", suffix as char).into_bytes()
        }
        (SpecialKey::Tilde(number), None) => format!("\x1b[{number}~").into_bytes(),
        (SpecialKey::Tilde(number), Some(parameter)) => {
            format!("\x1b[{number};{parameter}~").into_bytes()
        }
    }
}

fn has_xterm_modifiers(modifiers: Modifiers) -> bool {
    !SequenceModifiers::from_modifiers(modifiers).is_empty()
}

/// Keypad keys, for hosts that can tell them apart from the main keys. Names
/// follow xkb's `KP_*` keysyms. Returns the main-key equivalent sent under
/// DECKPNM and the SS3 final byte sent under DECKPAM.
fn keypad_key(key: &str) -> Option<(&'static str, u8)> {
    Some(match key {
        "kp_0" => ("0", b'p'),
        "kp_1" => ("1", b'q'),
        "kp_2" => ("2", b'r'),
        "kp_3" => ("3", b's'),
        "kp_4" => ("4", b't'),
        "kp_5" => ("5", b'u'),
        "kp_6" => ("6", b'v'),
        "kp_7" => ("7", b'w'),
        "kp_8" => ("8", b'x'),
        "kp_9" => ("9", b'y'),
        "kp_multiply" => ("*", b'j'),
        "kp_add" => ("+", b'k'),
        "kp_separator" => (",", b'l'),
        "kp_subtract" => ("-", b'm'),
        "kp_decimal" => (".", b'n'),
        "kp_divide" => ("/", b'o'),
        "kp_equal" => ("=", b'X'),
        "kp_enter" => ("enter", b'M'),
        _ => return None,
    })
}

#[cfg(test)]
mod tests;
