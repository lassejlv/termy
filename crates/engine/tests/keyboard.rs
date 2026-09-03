use engine::{
    Key, KeyEvent, KeyEventKind, KeypadKey, MediaKey, ModifierKey, Modifiers, Terminal,
    TerminalConfig, TerminalEvent,
};

fn terminal() -> Terminal {
    Terminal::new(TerminalConfig {
        columns: 80,
        rows: 24,
        scrollback_limit: 100,
    })
}

fn event(key: Key, text: Option<&str>, modifiers: Modifiers) -> KeyEvent {
    let shifted_key = shifted_key(&key, text, modifiers);
    KeyEvent {
        key,
        text: text.map(str::to_owned),
        shifted_key,
        base_layout: None,
        modifiers,
        kind: KeyEventKind::Press,
    }
}

fn release(key: Key, text: Option<&str>, modifiers: Modifiers) -> KeyEvent {
    let shifted_key = shifted_key(&key, text, modifiers);
    KeyEvent {
        key,
        text: text.map(str::to_owned),
        shifted_key,
        base_layout: None,
        modifiers,
        kind: KeyEventKind::Release,
    }
}

fn repeat(key: Key, text: Option<&str>, modifiers: Modifiers) -> KeyEvent {
    let shifted_key = shifted_key(&key, text, modifiers);
    KeyEvent {
        key,
        text: text.map(str::to_owned),
        shifted_key,
        base_layout: None,
        modifiers,
        kind: KeyEventKind::Repeat,
    }
}

fn shifted_key(key: &Key, text: Option<&str>, modifiers: Modifiers) -> Option<u32> {
    let Key::Character(unshifted) = key else {
        return None;
    };
    modifiers
        .contains(Modifiers::SHIFT)
        .then(|| text?.chars().find(|character| !character.is_control()))
        .flatten()
        .filter(|shifted| shifted != unshifted)
        .map(u32::from)
}

#[test]
fn printable_text_uses_the_layout_result_and_complete_legacy_ctrl_table() {
    let terminal = terminal();
    assert_eq!(
        terminal.encode_key(&event(Key::Character('@'), Some("@"), Modifiers::empty())),
        b"@"
    );
    assert_eq!(
        terminal.encode_key(&event(Key::Character('2'), Some("@"), Modifiers::SHIFT)),
        b"@"
    );
    assert_eq!(
        terminal.encode_key(&event(Key::Character('a'), Some("æ"), Modifiers::empty())),
        "æ".as_bytes()
    );

    for (character, expected) in [
        ('2', 0),
        ('3', 27),
        ('4', 28),
        ('5', 29),
        ('6', 30),
        ('7', 31),
        ('8', 127),
    ] {
        assert_eq!(
            terminal.encode_key(&event(Key::Character(character), None, Modifiers::CONTROL,)),
            [expected]
        );
    }
    assert_eq!(
        terminal.encode_key(&event(
            Key::Character('2'),
            Some("@"),
            Modifiers::SHIFT | Modifiers::CONTROL,
        )),
        [0]
    );
}

#[test]
fn legacy_c0_and_backtab_encodings_match_terminal_conventions() {
    let terminal = terminal();
    assert_eq!(
        terminal.encode_key(&event(Key::Backtab, None, Modifiers::empty())),
        b"\x1b[Z"
    );
    assert_eq!(
        terminal.encode_key(&event(Key::Tab, None, Modifiers::SHIFT)),
        b"\x1b[Z"
    );
    assert_eq!(
        terminal.encode_key(&event(Key::Tab, None, Modifiers::ALT)),
        b"\x1b\t"
    );
    assert_eq!(
        terminal.encode_key(&event(Key::Enter, None, Modifiers::ALT)),
        b"\x1b\r"
    );
    assert_eq!(
        terminal.encode_key(&event(Key::Escape, None, Modifiers::ALT)),
        b"\x1b\x1b"
    );
    assert_eq!(
        terminal.encode_key(&event(Key::Backspace, None, Modifiers::CONTROL)),
        b"\x08"
    );
}

#[test]
fn legacy_cursor_and_function_keys_follow_xterm_256color_families() {
    let mut terminal = terminal();
    assert_eq!(
        terminal.encode_key(&event(Key::Up, None, Modifiers::empty())),
        b"\x1b[A"
    );
    terminal.feed(b"\x1b[?1h");
    assert_eq!(
        terminal.encode_key(&event(Key::Up, None, Modifiers::empty())),
        b"\x1bOA"
    );
    assert_eq!(
        terminal.encode_key(&event(Key::Up, None, Modifiers::SHIFT)),
        b"\x1b[1;2A"
    );

    for (number, expected) in [
        (1, b"\x1bOP".as_slice()),
        (12, b"\x1b[24~".as_slice()),
        (13, b"\x1b[1;2P".as_slice()),
        (24, b"\x1b[24;2~".as_slice()),
        (25, b"\x1b[1;5P".as_slice()),
        (35, b"\x1b[23;5~".as_slice()),
    ] {
        assert_eq!(
            terminal.encode_key(&event(Key::Function(number), None, Modifiers::empty())),
            expected
        );
    }
}

#[test]
fn xterm_modify_other_keys_set_reset_disable_and_query() {
    let mut terminal = terminal();
    terminal.feed(b"\x1b[>4;1m");
    assert_eq!(
        terminal.encode_key(&event(Key::Tab, None, Modifiers::ALT)),
        b"\x1b[27;3;9~"
    );
    assert_eq!(
        terminal.encode_key(&event(Key::Backtab, None, Modifiers::empty())),
        b"\x1b[Z"
    );
    assert_eq!(
        terminal.encode_key(&event(Key::Enter, None, Modifiers::SHIFT)),
        b"\x1b[27;2;13~"
    );
    assert_eq!(
        terminal.encode_key(&event(Key::Tab, None, Modifiers::CONTROL)),
        b"\x1b[27;5;9~"
    );
    assert_eq!(
        terminal.encode_key(&event(
            Key::Character('a'),
            Some("A"),
            Modifiers::CONTROL | Modifiers::SHIFT,
        )),
        b"\x1b[27;6;65~"
    );
    assert_eq!(
        terminal.encode_key(&event(Key::Character('.'), Some("."), Modifiers::CONTROL,)),
        b"\x1b[27;5;46~"
    );
    assert_eq!(
        terminal.encode_key(&event(Key::Character('a'), None, Modifiers::CONTROL,)),
        b"\x01"
    );
    assert_eq!(
        terminal.encode_key(&event(
            Key::Character('2'),
            Some("@"),
            Modifiers::SHIFT | Modifiers::ALT,
        )),
        b"\x1b[27;4;64~"
    );

    terminal.feed(b"\x1b[?4m");
    assert_eq!(
        terminal.drain_events(),
        vec![TerminalEvent::Reply(b"\x1b[>4;1m".to_vec())]
    );
    terminal.feed(b"\x1b[>4m");
    assert_eq!(
        terminal.encode_key(&event(Key::Backtab, None, Modifiers::empty())),
        b"\x1b[Z"
    );
    terminal.feed(b"\x1b[>4;2m\x1b[>4n");
    assert_eq!(
        terminal.encode_key(&event(Key::Character('a'), Some("A"), Modifiers::SHIFT,)),
        b"A"
    );
    terminal.feed(b"\x1b[?4m");
    assert_eq!(
        terminal.drain_events(),
        vec![TerminalEvent::Reply(b"\x1b[>4n".to_vec())]
    );
}

#[test]
fn xterm_modify_other_keys_levels_mask_and_format_are_honored() {
    let mut terminal = terminal();
    terminal.feed(b"\x1b[>4;2m");
    assert_eq!(
        terminal.encode_key(&event(Key::Backtab, None, Modifiers::empty())),
        b"\x1b[27;2;9~"
    );
    assert_eq!(
        terminal.encode_key(&event(Key::Character('a'), Some("A"), Modifiers::SHIFT,)),
        b"\x1b[27;2;65~"
    );
    assert_eq!(
        terminal.encode_key(&event(Key::Character('a'), None, Modifiers::CONTROL,)),
        b"\x1b[27;5;97~"
    );
    terminal.feed(b"\x1b[>4:1m");
    assert_eq!(
        terminal.encode_key(&event(
            Key::Character('a'),
            Some("A"),
            Modifiers::SHIFT | Modifiers::ALT,
        )),
        b"\x1b[27;4;65~"
    );
    assert_eq!(
        terminal.encode_key(&event(
            Key::Character('a'),
            Some("A"),
            Modifiers::SHIFT | Modifiers::CONTROL,
        )),
        b"\x1b[27;6;65~"
    );

    terminal.feed(b"\x1b[>4;1f");
    assert_eq!(
        terminal.encode_key(&event(Key::Character('a'), None, Modifiers::CONTROL,)),
        b"\x1b[97;5u"
    );
    terminal.feed(b"\x1b[?4g");
    assert_eq!(
        terminal.drain_events(),
        vec![TerminalEvent::Reply(b"\x1b[>4;1f".to_vec())]
    );
    terminal.feed(b"\x1b[>4f\x1b[>4;3m");
    assert_eq!(
        terminal.encode_key(&event(Key::Character(' '), Some(" "), Modifiers::empty())),
        b"\x1b[27;1;32~"
    );
}

#[test]
fn kitty_disambiguate_keeps_c0_fallbacks_and_canonicalizes_other_keys() {
    let mut terminal = terminal();
    terminal.feed(b"\x1b[?1h\x1b[=1u");
    assert_eq!(
        terminal.encode_key(&event(Key::Character('a'), Some("a"), Modifiers::CONTROL,)),
        b"\x1b[97;5u"
    );
    assert_eq!(
        terminal.encode_key(&event(
            Key::Character('a'),
            Some("a"),
            Modifiers::CONTROL | Modifiers::CAPS_LOCK,
        )),
        b"\x1b[97;69u"
    );
    assert_eq!(
        terminal.encode_key(&event(Key::Escape, None, Modifiers::empty())),
        b"\x1b[27u"
    );
    assert_eq!(
        terminal.encode_key(&event(Key::Up, None, Modifiers::empty())),
        b"\x1b[A"
    );
    assert_eq!(
        terminal.encode_key(&event(Key::Enter, None, Modifiers::empty())),
        b"\r"
    );
    assert_eq!(
        terminal.encode_key(&event(Key::Tab, None, Modifiers::empty())),
        b"\t"
    );
    assert_eq!(
        terminal.encode_key(&event(Key::Backspace, None, Modifiers::empty())),
        b"\x7f"
    );
    assert!(
        terminal
            .encode_key(&release(Key::Enter, None, Modifiers::empty()))
            .is_empty()
    );
}

#[test]
fn kitty_pure_enhancements_do_not_change_legacy_shapes() {
    let mut terminal = terminal();
    terminal.feed(b"\x1b[?1h\x1b[=4u");
    assert_eq!(
        terminal.encode_key(&event(Key::Up, None, Modifiers::empty())),
        b"\x1bOA"
    );
    assert_eq!(
        terminal.encode_key(&event(Key::Function(13), None, Modifiers::empty())),
        b"\x1b[1;2P"
    );
}

#[test]
fn kitty_event_reporting_handles_functional_releases_but_not_c0_releases() {
    let mut terminal = terminal();
    terminal.feed(b"\x1b[=2u");
    assert_eq!(
        terminal.encode_key(&release(Key::Up, None, Modifiers::empty())),
        b"\x1b[1;1:3A"
    );
    assert!(
        terminal
            .encode_key(&release(Key::Enter, None, Modifiers::empty()))
            .is_empty()
    );
    assert_eq!(
        terminal.encode_key(&event(Key::Enter, None, Modifiers::empty())),
        b"\r"
    );
}

#[test]
fn kitty_all_keys_alternate_keys_and_associated_text_are_canonical() {
    let mut terminal = terminal();
    terminal.feed(b"\x1b[=24u");
    assert_eq!(
        terminal.encode_key(&event(
            Key::Character('a'),
            Some("a\u{301}"),
            Modifiers::empty(),
        )),
        b"\x1b[97;;97:769u"
    );
    assert_eq!(
        terminal.encode_key(&event(Key::Backtab, None, Modifiers::empty())),
        b"\x1b[9;2u"
    );

    terminal.feed(b"\x1b[=5u");
    assert_eq!(
        terminal.encode_key(&event(
            Key::Character('='),
            Some("+"),
            Modifiers::SHIFT | Modifiers::CONTROL,
        )),
        b"\x1b[61:43;6u"
    );
    assert_eq!(
        terminal.encode_key(&event(Key::Function(3), None, Modifiers::empty())),
        b"\x1b[13~"
    );
    assert_eq!(
        terminal.encode_key(&event(Key::Function(13), None, Modifiers::empty())),
        b"\x1b[57376u"
    );
}

#[test]
fn kitty_alternate_keys_encode_shifted_and_pc101_base_fields() {
    let mut terminal = terminal();
    terminal.feed(b"\x1b[=12u");

    let mut localized = event(
        Key::Character('\u{0444}'),
        Some("\u{0444}"),
        Modifiers::empty(),
    );
    localized.base_layout = Some(u32::from('a'));
    assert_eq!(terminal.encode_key(&localized), b"\x1b[1092::97u");

    let mut shifted = event(Key::Character('='), Some("+"), Modifiers::SHIFT);
    shifted.base_layout = Some(u32::from(']'));
    assert_eq!(terminal.encode_key(&shifted), b"\x1b[61:43:93;2u");

    terminal.feed(b"\x1b[=14u");
    let mut shifted_release = release(Key::Character('='), None, Modifiers::SHIFT);
    shifted_release.shifted_key = Some(u32::from('+'));
    assert_eq!(terminal.encode_key(&shifted_release), b"\x1b[61:43;2:3u");
}

#[test]
fn kitty_functional_modifier_media_and_lock_events_use_canonical_codes() {
    let mut terminal = terminal();
    assert_eq!(
        terminal.encode_key(&event(Key::PrintScreen, None, Modifiers::empty())),
        b"\x1b[57361u"
    );
    assert_eq!(
        terminal.encode_key(&event(Key::PrintScreen, None, Modifiers::CAPS_LOCK)),
        b"\x1b[57361u"
    );
    assert_eq!(
        terminal.encode_key(&event(Key::Media(MediaKey::Play), None, Modifiers::empty(),)),
        b"\x1b[57428u"
    );
    assert!(
        terminal
            .encode_key(&event(
                Key::Modifier(ModifierKey::LeftShift),
                None,
                Modifiers::SHIFT,
            ))
            .is_empty()
    );

    terminal.feed(b"\x1b[=2u");
    assert_eq!(
        terminal.encode_key(&event(Key::CapsLock, None, Modifiers::CAPS_LOCK)),
        b"\x1b[57358;65:1u"
    );

    terminal.feed(b"\x1b[=10u");
    assert_eq!(
        terminal.encode_key(&event(
            Key::Modifier(ModifierKey::LeftShift),
            None,
            Modifiers::SHIFT,
        )),
        b"\x1b[57441;2:1u"
    );
    assert_eq!(
        terminal.encode_key(&repeat(
            Key::Modifier(ModifierKey::LeftShift),
            None,
            Modifiers::SHIFT,
        )),
        b"\x1b[57441;2:2u"
    );
    assert_eq!(
        terminal.encode_key(&release(
            Key::Modifier(ModifierKey::LeftShift),
            None,
            Modifiers::empty(),
        )),
        b"\x1b[57441;1:3u"
    );
    assert_eq!(
        terminal.encode_key(&event(Key::CapsLock, None, Modifiers::CAPS_LOCK)),
        b"\x1b[57358;65:1u"
    );
    assert_eq!(
        terminal.encode_key(&event(Key::NumLock, None, Modifiers::NUM_LOCK)),
        b"\x1b[57360;129:1u"
    );
}

#[test]
fn unsupported_legacy_super_and_hyper_chords_do_not_fall_through_as_text() {
    let terminal = terminal();
    assert_eq!(
        terminal.encode_key(&event(Key::Character('a'), Some("a"), Modifiers::SUPER,)),
        b"\x1b[97;9u"
    );
    assert_eq!(
        terminal.encode_key(&event(Key::Character('a'), Some("a"), Modifiers::HYPER,)),
        b"\x1b[97;17u"
    );
    assert_eq!(
        terminal.encode_key(&event(Key::Enter, None, Modifiers::SUPER)),
        b"\x1b[13;9u"
    );
}

#[test]
fn kitty_release_omits_associated_text() {
    let mut terminal = terminal();
    terminal.feed(b"\x1b[=26u");
    assert_eq!(
        terminal.encode_key(&release(Key::Character('a'), Some("a"), Modifiers::empty(),)),
        b"\x1b[97;1:3u"
    );
}

#[test]
fn committed_text_uses_keycode_zero_only_when_associated_text_is_negotiated() {
    let mut terminal = terminal();
    assert_eq!(terminal.encode_text("a\u{301}"), "a\u{301}".as_bytes());
    terminal.feed(b"\x1b[=8u");
    assert_eq!(terminal.encode_text("a\u{301}"), "a\u{301}".as_bytes());
    terminal.feed(b"\x1b[=24u");
    assert_eq!(terminal.encode_text("a\u{301}"), b"\x1b[0;;97:769u");
    assert_eq!(terminal.encode_text("a\n"), b"\x1b[0;;97u");
}

#[test]
fn application_keypad_and_kitty_keypad_identity_are_preserved() {
    let mut terminal = terminal();
    let one = event(
        Key::Keypad(KeypadKey::Digit(1)),
        Some("1"),
        Modifiers::empty(),
    );
    let enter = event(Key::Keypad(KeypadKey::Enter), None, Modifiers::empty());
    assert_eq!(terminal.encode_key(&one), b"1");
    assert_eq!(terminal.encode_key(&enter), b"\r");

    terminal.feed(b"\x1b=");
    assert_eq!(terminal.encode_key(&one), b"\x1bOq");
    assert_eq!(terminal.encode_key(&enter), b"\x1bOM");
    terminal.feed(b"\x1b>");
    assert_eq!(terminal.encode_key(&one), b"1");

    terminal.feed(b"\x1b[?66h\x1b[?66$p");
    assert_eq!(terminal.encode_key(&one), b"\x1bOq");
    assert_eq!(
        terminal.drain_events(),
        vec![TerminalEvent::Reply(b"\x1b[?66;1$y".to_vec())]
    );
    terminal.feed(b"\x1b[?66l\x1b[=1u");
    assert_eq!(
        terminal.encode_key(&event(
            Key::Keypad(KeypadKey::Left),
            None,
            Modifiers::empty(),
        )),
        b"\x1b[57417u"
    );
    assert_eq!(terminal.encode_key(&one), b"1");

    terminal.feed(b"\x1b[=8u");
    assert_eq!(terminal.encode_key(&one), b"\x1b[57400u");
}

#[test]
fn keypad_text_identity_is_stable_across_press_and_release() {
    let mut terminal = terminal();
    terminal.feed(b"\x1b[=2u");
    assert_eq!(
        terminal.encode_key(&event(
            Key::Keypad(KeypadKey::Add),
            Some("+"),
            Modifiers::empty(),
        )),
        b"+"
    );
    assert!(
        terminal
            .encode_key(&release(
                Key::Keypad(KeypadKey::Add),
                None,
                Modifiers::empty(),
            ))
            .is_empty()
    );
}

#[test]
fn num_lock_overrides_deckpam_while_pc_keypad_navigation_honors_decckm() {
    let mut terminal = terminal();
    terminal.feed(b"\x1b=");
    assert_eq!(
        terminal.encode_key(&event(
            Key::Keypad(KeypadKey::Digit(1)),
            Some("1"),
            Modifiers::NUM_LOCK,
        )),
        b"1"
    );
    assert_eq!(
        terminal.encode_key(&event(
            Key::Keypad(KeypadKey::Decimal),
            Some("."),
            Modifiers::NUM_LOCK,
        )),
        b"."
    );
    assert_eq!(
        terminal.encode_key(&event(
            Key::Keypad(KeypadKey::Add),
            Some("+"),
            Modifiers::NUM_LOCK,
        )),
        b"+"
    );
    assert_eq!(
        terminal.encode_key(&event(
            Key::Keypad(KeypadKey::Enter),
            None,
            Modifiers::NUM_LOCK,
        )),
        b"\r"
    );
    assert_eq!(
        terminal.encode_key(&event(
            Key::Keypad(KeypadKey::End),
            None,
            Modifiers::empty(),
        )),
        b"\x1b[F"
    );
    assert_eq!(
        terminal.encode_key(&event(
            Key::Keypad(KeypadKey::Down),
            None,
            Modifiers::empty(),
        )),
        b"\x1b[B"
    );
    assert_eq!(
        terminal.encode_key(&event(
            Key::Keypad(KeypadKey::PageDown),
            None,
            Modifiers::empty(),
        )),
        b"\x1b[6~"
    );
    assert_eq!(
        terminal.encode_key(&event(
            Key::Keypad(KeypadKey::Multiply),
            Some("*"),
            Modifiers::empty(),
        )),
        b"\x1bOj"
    );

    terminal.feed(b"\x1b[?1h");
    assert_eq!(
        terminal.encode_key(&event(
            Key::Keypad(KeypadKey::Home),
            None,
            Modifiers::empty(),
        )),
        b"\x1bOH"
    );
    assert_eq!(
        terminal.encode_key(&event(
            Key::Keypad(KeypadKey::Begin),
            None,
            Modifiers::empty(),
        )),
        b"\x1bOE"
    );

    terminal.feed(b"\x1b[=1u");
    assert_eq!(
        terminal.encode_key(&event(
            Key::Keypad(KeypadKey::Digit(1)),
            Some("1"),
            Modifiers::NUM_LOCK,
        )),
        b"1"
    );
}
