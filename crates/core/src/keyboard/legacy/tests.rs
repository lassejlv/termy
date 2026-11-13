use super::super::{
    Keystroke, Modifiers, TerminalKeyEventKind, TerminalKeyboardMode, keystroke_to_input,
};

fn keystroke(key: &str, key_char: Option<&str>, modifiers: Modifiers) -> Keystroke {
    Keystroke {
        modifiers,
        key: key.to_string(),
        key_char: key_char.map(str::to_string),
    }
}

// Prompt shortcuts off so the natural-editing remaps don't preempt the
// xterm encoding under test.
fn encode(keystroke: &Keystroke, mode: TerminalKeyboardMode) -> Option<Vec<u8>> {
    keystroke_to_input(keystroke, TerminalKeyEventKind::Press, mode, false)
}

fn legacy_mode() -> TerminalKeyboardMode {
    TerminalKeyboardMode::default()
}

fn application_cursor_mode() -> TerminalKeyboardMode {
    TerminalKeyboardMode::from_flags(true, false, false, false, false, false)
}

fn application_keypad_mode() -> TerminalKeyboardMode {
    TerminalKeyboardMode::default().with_application_keypad(true)
}

fn shift() -> Modifiers {
    Modifiers {
        shift: true,
        ..Modifiers::default()
    }
}

fn control() -> Modifiers {
    Modifiers {
        control: true,
        ..Modifiers::default()
    }
}

fn control_shift() -> Modifiers {
    Modifiers {
        control: true,
        shift: true,
        ..Modifiers::default()
    }
}

#[test]
fn modified_cursor_keys_use_xterm_modifier_parameter() {
    let cases = [
        ("up", control(), b"\x1b[1;5A".as_slice()),
        ("down", shift(), b"\x1b[1;2B".as_slice()),
        ("right", shift(), b"\x1b[1;2C".as_slice()),
        ("left", control_shift(), b"\x1b[1;6D".as_slice()),
        ("home", shift(), b"\x1b[1;2H".as_slice()),
        ("end", control(), b"\x1b[1;5F".as_slice()),
    ];

    for (key, modifiers, expected) in cases {
        assert_eq!(
            encode(&keystroke(key, None, modifiers), legacy_mode()),
            Some(expected.to_vec()),
            "{key} with {modifiers:?}"
        );
    }
}

#[test]
fn modified_cursor_keys_ignore_application_cursor_mode() {
    assert_eq!(
        encode(&keystroke("up", None, control()), application_cursor_mode()),
        Some(b"\x1b[1;5A".to_vec())
    );
    assert_eq!(
        encode(
            &keystroke("up", None, Modifiers::default()),
            application_cursor_mode()
        ),
        Some(b"\x1bOA".to_vec())
    );
}

#[test]
fn modifier_parameter_counts_alt_and_super_but_not_fn() {
    let all = Modifiers {
        shift: true,
        alt: true,
        control: true,
        platform: true,
        function: true,
    };
    assert_eq!(
        encode(&keystroke("up", None, all), legacy_mode()),
        Some(b"\x1b[1;16A".to_vec())
    );

    let function = Modifiers {
        function: true,
        ..Modifiers::default()
    };
    assert_eq!(
        encode(&keystroke("up", None, function), application_cursor_mode()),
        Some(b"\x1bOA".to_vec())
    );
}

#[cfg(not(target_os = "macos"))]
#[test]
fn alt_cursor_keys_use_modifier_parameter() {
    let alt = Modifiers {
        alt: true,
        ..Modifiers::default()
    };
    assert_eq!(
        encode(&keystroke("up", None, alt), legacy_mode()),
        Some(b"\x1b[1;3A".to_vec())
    );
    assert_eq!(
        encode(&keystroke("left", None, alt), legacy_mode()),
        Some(b"\x1b[1;3D".to_vec())
    );
}

#[test]
fn modified_f1_through_f4_use_csi_with_modifier_parameter() {
    let cases = [
        ("f1", shift(), b"\x1b[1;2P".as_slice()),
        ("f2", control(), b"\x1b[1;5Q".as_slice()),
        ("f3", control_shift(), b"\x1b[1;6R".as_slice()),
        ("f4", shift(), b"\x1b[1;2S".as_slice()),
    ];

    for (key, modifiers, expected) in cases {
        assert_eq!(
            encode(&keystroke(key, None, modifiers), legacy_mode()),
            Some(expected.to_vec()),
            "{key} with {modifiers:?}"
        );
    }
}

#[test]
fn modified_tilde_keys_append_modifier_parameter() {
    let cases = [
        ("f5", shift(), b"\x1b[15;2~".as_slice()),
        ("f12", control(), b"\x1b[24;5~".as_slice()),
        ("insert", shift(), b"\x1b[2;2~".as_slice()),
        ("delete", control(), b"\x1b[3;5~".as_slice()),
        ("pageup", control(), b"\x1b[5;5~".as_slice()),
        ("pagedown", control_shift(), b"\x1b[6;6~".as_slice()),
    ];

    for (key, modifiers, expected) in cases {
        assert_eq!(
            encode(&keystroke(key, None, modifiers), legacy_mode()),
            Some(expected.to_vec()),
            "{key} with {modifiers:?}"
        );
    }
}

#[test]
fn plain_editing_keys_use_tilde_sequences() {
    let cases = [
        ("insert", b"\x1b[2~".as_slice()),
        ("delete", b"\x1b[3~".as_slice()),
        ("pageup", b"\x1b[5~".as_slice()),
        ("pagedown", b"\x1b[6~".as_slice()),
    ];

    for (key, expected) in cases {
        assert_eq!(
            encode(&keystroke(key, None, Modifiers::default()), legacy_mode()),
            Some(expected.to_vec()),
            "{key}"
        );
    }
}

#[test]
fn keypad_keys_send_ss3_sequences_in_application_keypad_mode() {
    let cases = [
        ("kp_0", Some("0"), b"\x1bOp".as_slice()),
        ("kp_9", Some("9"), b"\x1bOy".as_slice()),
        ("kp_multiply", Some("*"), b"\x1bOj".as_slice()),
        ("kp_add", Some("+"), b"\x1bOk".as_slice()),
        ("kp_subtract", Some("-"), b"\x1bOm".as_slice()),
        ("kp_decimal", Some("."), b"\x1bOn".as_slice()),
        ("kp_divide", Some("/"), b"\x1bOo".as_slice()),
        ("kp_enter", None, b"\x1bOM".as_slice()),
    ];

    for (key, key_char, expected) in cases {
        assert_eq!(
            encode(
                &keystroke(key, key_char, Modifiers::default()),
                application_keypad_mode()
            ),
            Some(expected.to_vec()),
            "{key}"
        );
    }
}

#[test]
fn keypad_keys_send_main_key_bytes_in_numeric_keypad_mode() {
    assert_eq!(
        encode(
            &keystroke("kp_5", Some("5"), Modifiers::default()),
            legacy_mode()
        ),
        Some(b"5".to_vec())
    );
    assert_eq!(
        encode(
            &keystroke("kp_5", None, Modifiers::default()),
            legacy_mode()
        ),
        Some(b"5".to_vec())
    );
    assert_eq!(
        encode(
            &keystroke("kp_enter", None, Modifiers::default()),
            legacy_mode()
        ),
        Some(b"\r".to_vec())
    );
    // Modified keypad keys fall back to the numeric bytes even under DECKPAM.
    assert_eq!(
        encode(
            &keystroke("kp_5", Some("5"), shift()),
            application_keypad_mode()
        ),
        Some(b"5".to_vec())
    );
}

#[test]
fn application_keypad_mode_leaves_main_keys_unchanged() {
    assert_eq!(
        encode(
            &keystroke("5", Some("5"), Modifiers::default()),
            application_keypad_mode()
        ),
        Some(b"5".to_vec())
    );
    assert_eq!(
        encode(
            &keystroke("enter", None, Modifiers::default()),
            application_keypad_mode()
        ),
        Some(b"\r".to_vec())
    );
}

#[test]
fn application_keypad_is_not_part_of_the_wire_format() {
    let mode = application_keypad_mode();
    let json = serde_json::to_string(&mode).unwrap();
    assert!(!json.contains("application_keypad"), "{json}");
    let decoded: TerminalKeyboardMode = serde_json::from_str(&json).unwrap();
    assert!(!decoded.application_keypad());
}

#[test]
fn kitty_mode_keeps_its_own_modified_cursor_encoding() {
    let disambiguate = TerminalKeyboardMode::from_flags(false, true, false, false, false, false);
    assert_eq!(
        encode(
            &keystroke("insert", None, Modifiers::default()),
            disambiguate
        ),
        Some(b"\x1b[2~".to_vec())
    );
    assert_eq!(
        encode(&keystroke("f3", None, shift()), disambiguate),
        Some(b"\x1b[13;2~".to_vec())
    );
}

#[cfg(not(target_os = "macos"))]
#[test]
fn alt_prefixes_text_keys_with_escape() {
    let alt = Modifiers {
        alt: true,
        ..Modifiers::default()
    };
    let alt_shift = Modifiers {
        alt: true,
        shift: true,
        ..Modifiers::default()
    };
    let control_alt = Modifiers {
        alt: true,
        control: true,
        ..Modifiers::default()
    };

    assert_eq!(
        encode(&keystroke("b", Some("b"), alt), legacy_mode()),
        Some(b"\x1bb".to_vec())
    );
    assert_eq!(
        encode(&keystroke("b", None, alt), legacy_mode()),
        Some(b"\x1bb".to_vec())
    );
    assert_eq!(
        encode(&keystroke("b", Some("B"), alt_shift), legacy_mode()),
        Some(b"\x1bB".to_vec())
    );
    assert_eq!(
        encode(&keystroke("backspace", None, alt), legacy_mode()),
        Some(b"\x1b\x7f".to_vec())
    );
    assert_eq!(
        encode(&keystroke("a", None, control_alt), legacy_mode()),
        Some(b"\x1b\x01".to_vec())
    );
}

#[cfg(target_os = "macos")]
#[test]
fn macos_option_composed_text_is_not_escape_prefixed() {
    let alt = Modifiers {
        alt: true,
        ..Modifiers::default()
    };

    assert_eq!(
        encode(&keystroke("b", Some("∫"), alt), legacy_mode()),
        Some("∫".as_bytes().to_vec())
    );
}

#[test]
fn space_sends_the_text_the_layout_composed() {
    let alt = Modifiers {
        alt: true,
        ..Modifiers::default()
    };
    assert_eq!(
        encode(
            &keystroke("space", Some("\u{a0}"), Modifiers::default()),
            legacy_mode()
        ),
        Some("\u{a0}".as_bytes().to_vec())
    );
    // Alt composes the same text; only an escape prefix may differ by platform.
    let composed = encode(&keystroke("space", Some("\u{a0}"), alt), legacy_mode()).unwrap();
    assert!(composed.ends_with("\u{a0}".as_bytes()), "{composed:?}");
    // Plain Space keeps sending a space.
    for key_char in [None, Some(""), Some(" ")] {
        assert_eq!(
            encode(
                &keystroke("space", key_char, Modifiers::default()),
                legacy_mode()
            ),
            Some(b" ".to_vec()),
            "{key_char:?}"
        );
    }
}

#[test]
fn ctrl_space_sends_nul() {
    for (key_char, modifiers) in [
        (None, control()),
        (Some(" "), control()),
        (Some("\u{a0}"), control()),
        (None, control_shift()),
    ] {
        assert_eq!(
            encode(&keystroke("space", key_char, modifiers), legacy_mode()),
            Some(vec![0x00]),
            "{key_char:?} {modifiers:?}"
        );
    }
}

#[test]
fn ctrl_digits_and_symbols_use_xterm_control_bytes() {
    let cases = [
        ("2", 0x00),
        ("3", 0x1b),
        ("4", 0x1c),
        ("5", 0x1d),
        ("6", 0x1e),
        ("7", 0x1f),
        ("8", 0x7f),
        ("/", 0x1f),
        ("-", 0x1f),
        ("?", 0x7f),
    ];

    for (key, expected) in cases {
        assert_eq!(
            encode(&keystroke(key, Some(key), control()), legacy_mode()),
            Some(vec![expected]),
            "ctrl+{key}"
        );
    }
    // Digits without an xterm mapping still send nothing.
    for key in ["1", "9", "0"] {
        assert_eq!(
            encode(&keystroke(key, Some(key), control()), legacy_mode()),
            None,
            "ctrl+{key}"
        );
    }
}

#[test]
fn kitty_modes_report_keypad_keys_as_the_main_keys_they_type() {
    for flags in [
        (false, true, false, false, false, false),
        (false, true, true, false, true, true),
    ] {
        let mode =
            TerminalKeyboardMode::from_flags(flags.0, flags.1, flags.2, flags.3, flags.4, flags.5)
                .with_application_keypad(true);
        for (keypad, main, text) in [
            ("kp_1", "1", "1"),
            ("kp_add", "+", "+"),
            ("kp_enter", "enter", "\r"),
        ] {
            assert_eq!(
                encode(&keystroke(keypad, Some(text), Modifiers::default()), mode),
                encode(&keystroke(main, Some(text), Modifiers::default()), mode),
                "{keypad} in {flags:?}"
            );
        }
    }
}
