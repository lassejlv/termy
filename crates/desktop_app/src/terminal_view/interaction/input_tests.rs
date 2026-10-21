use super::{
    CLIPBOARD_IMAGE_MAX_AGE, FileDropTarget, PathQuoteStyle, PendingKeyRelease,
    PendingKeyReleaseAction, classify_file_drop_target, clipboard_entries_to_terminal_paste_input,
    clipboard_item_to_terminal_paste_input, dropped_paths_to_terminal_paste_input, image_extension,
    is_compose_dead_key, kitty_png_clipboard_item, modifier_transition_events,
    normalize_paste_line_endings, shell_quote_paths, should_defer_altgr_key_down_to_ime,
    should_defer_key_down_to_ime, should_prepare_terminal_input_write, should_write_drop_to_target,
    take_deferred_ime_key_release, take_pending_key_release_action,
    terminal_modifier_transition_events, write_clipboard_image_in,
};
use gpui_kit::{Keystroke, Modifiers};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

fn keystroke(key: &str, key_char: Option<&str>, modifiers: Modifiers) -> Keystroke {
    Keystroke {
        modifiers,
        key: key.to_string(),
        key_char: key_char.map(str::to_string),
    }
}

#[test]
fn ime_defers_plain_printable_text() {
    assert!(should_defer_key_down_to_ime(&keystroke(
        "a",
        Some("a"),
        Modifiers::default(),
    )));
}

#[test]
fn ime_keeps_shifted_printable_text_on_ime_path() {
    let modifiers = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    assert!(should_defer_key_down_to_ime(&keystroke(
        "A",
        Some("A"),
        modifiers
    )));
}

#[test]
fn ime_does_not_defer_special_keys_or_modified_shortcuts() {
    for key in &["enter", "tab", "space", "backspace", "escape", "delete"] {
        assert!(
            !should_defer_key_down_to_ime(&keystroke(key, Some(key), Modifiers::default())),
            "{key} should not be deferred to IME"
        );
    }

    let control = Modifiers {
        control: true,
        ..Modifiers::default()
    };
    assert!(!should_defer_key_down_to_ime(&keystroke(
        "c",
        Some("c"),
        control
    )));
}

#[test]
fn dead_keys_starting_a_compose_sequence_are_not_forwarded() {
    // GPUI on Linux: the dead key's KeyDown has no key_char and a guessed key.
    let dead_key = keystroke("}", None, Modifiers::default());
    assert!(is_compose_dead_key(&dead_key, true));
    assert!(is_compose_dead_key(
        &keystroke("dead_tilde", None, Modifiers::default()),
        true
    ));
    // Without marked text it is an ordinary key without text.
    assert!(!is_compose_dead_key(&dead_key, false));
    // The composed character arrives with key_char and goes to the IME path.
    assert!(!is_compose_dead_key(
        &keystroke("asciitilde", Some("~"), Modifiers::default()),
        true
    ));
    // Named keys and shortcuts are still forwarded while composing.
    assert!(!is_compose_dead_key(
        &keystroke("escape", None, Modifiers::default()),
        true
    ));
    let control = Modifiers {
        control: true,
        ..Modifiers::default()
    };
    assert!(!is_compose_dead_key(&keystroke("c", None, control), true));
}

#[test]
fn altgr_text_defers_to_the_platform_text_path_on_windows() {
    let altgr = Modifiers {
        control: true,
        alt: true,
        ..Modifiers::default()
    };
    assert_eq!(
        should_defer_altgr_key_down_to_ime(&keystroke("7", Some("{"), altgr), true),
        cfg!(target_os = "windows")
    );
    // A real Ctrl+Alt shortcut produces no text and is not flagged.
    assert!(!should_defer_altgr_key_down_to_ime(
        &keystroke("t", None, altgr),
        false
    ));
    assert!(!should_defer_altgr_key_down_to_ime(
        &keystroke("7", Some("{"), altgr),
        false
    ));
    let control = Modifiers {
        control: true,
        ..Modifiers::default()
    };
    assert!(!should_defer_altgr_key_down_to_ime(
        &keystroke("c", Some("c"), control),
        true
    ));
}

#[test]
fn ime_does_not_defer_keys_without_key_char() {
    assert!(!should_defer_key_down_to_ime(&keystroke(
        "f1",
        None,
        Modifiers::default(),
    )));
}

#[test]
fn shell_quote_paths_escapes_single_quotes() {
    let paths = vec![
        PathBuf::from("/tmp/normal.png"),
        PathBuf::from("/tmp/quote's test.png"),
    ];

    assert_eq!(
        shell_quote_paths(&paths, PathQuoteStyle::Posix),
        "'/tmp/normal.png' '/tmp/quote'\\''s test.png'"
    );
}

#[test]
fn cmd_paths_use_double_quotes() {
    let paths = vec![
        PathBuf::from(r"C:\Users\me\a & b.txt"),
        PathBuf::from(r"C:\it's.txt"),
    ];
    assert_eq!(
        shell_quote_paths(&paths, PathQuoteStyle::Cmd),
        r#""C:\Users\me\a & b.txt" "C:\it's.txt""#
    );
}

#[test]
fn powershell_paths_double_every_single_quote() {
    let paths = vec![
        PathBuf::from(r"C:\it's $HOME.txt"),
        PathBuf::from("C:\\curly\u{2019}s.txt"),
    ];
    assert_eq!(
        shell_quote_paths(&paths, PathQuoteStyle::PowerShell),
        "'C:\\it''s $HOME.txt' 'C:\\curly\u{2019}\u{2019}s.txt'"
    );
}

#[test]
fn path_quote_style_follows_the_shell_program() {
    for (program, style) in [
        (r"C:\Windows\System32\cmd.exe", PathQuoteStyle::Cmd),
        ("CMD.EXE", PathQuoteStyle::Cmd),
        (
            r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
            PathQuoteStyle::PowerShell,
        ),
        (
            r"C:\Program Files\PowerShell\7\pwsh.exe",
            PathQuoteStyle::PowerShell,
        ),
        ("pwsh", PathQuoteStyle::PowerShell),
        (r"C:\Program Files\Git\bin\bash.exe", PathQuoteStyle::Posix),
        (r"C:\Windows\System32\wsl.exe", PathQuoteStyle::Posix),
        ("/bin/zsh", PathQuoteStyle::Posix),
    ] {
        assert_eq!(
            PathQuoteStyle::for_shell_program(program),
            style,
            "{program}"
        );
    }
}

#[test]
fn paste_line_endings_become_carriage_returns() {
    assert_eq!(
        &*normalize_paste_line_endings(b"one\r\ntwo\nthree\rfour"),
        b"one\rtwo\rthree\rfour"
    );
    assert_eq!(&*normalize_paste_line_endings(b"\n\n"), b"\r\r");
    assert!(matches!(
        normalize_paste_line_endings(b"no newline"),
        std::borrow::Cow::Borrowed(_)
    ));
}

#[test]
fn dropped_paths_add_a_trailing_space() {
    let paths = vec![PathBuf::from("/tmp/file with space.png")];
    let input = dropped_paths_to_terminal_paste_input(&paths, PathQuoteStyle::Posix)
        .expect("drop should serialize");
    assert_eq!(
        String::from_utf8(input).expect("drop input should be utf8"),
        "'/tmp/file with space.png' "
    );
}

#[test]
fn dropped_paths_rejects_empty_input() {
    assert!(dropped_paths_to_terminal_paste_input(&[], PathQuoteStyle::Posix).is_none());
}

#[test]
fn file_drop_writes_when_target_is_already_active() {
    assert!(should_write_drop_to_target(true, false));
}

#[test]
fn file_drop_writes_after_inactive_target_is_focused() {
    assert!(should_write_drop_to_target(false, true));
}

#[test]
fn file_drop_does_not_fall_back_when_target_focus_fails() {
    assert!(!should_write_drop_to_target(false, false));
}

#[test]
fn file_drop_classifies_active_and_inactive_terminal_targets() {
    assert_eq!(
        classify_file_drop_target(Some("%left".to_string()), Some("%left"), true),
        Some(FileDropTarget {
            pane_id: "%left".to_string(),
            is_active: true,
        })
    );
    assert_eq!(
        classify_file_drop_target(Some("%right".to_string()), Some("%left"), true),
        Some(FileDropTarget {
            pane_id: "%right".to_string(),
            is_active: false,
        })
    );
}

#[test]
fn file_drop_rejects_missing_and_non_terminal_targets() {
    assert_eq!(classify_file_drop_target(None, Some("%left"), true), None);
    assert_eq!(
        classify_file_drop_target(Some("%pane".to_string()), Some("%left"), false),
        None
    );
}

#[test]
fn clipboard_image_paste_materializes_a_quoted_temp_path() {
    let item = gpui_kit::ClipboardItem::new_image(&gpui_kit::Image::from_bytes(
        gpui_kit::ImageFormat::Png,
        vec![1, 2, 3, 4],
    ));

    let input = clipboard_item_to_terminal_paste_input(&item, PathQuoteStyle::Posix)
        .expect("clipboard image should serialize")
        .expect("clipboard image should produce paste input");
    let text = String::from_utf8(input).expect("path should be utf8");

    assert!(text.starts_with('\''));
    assert!(text.ends_with(".png'"));
    assert!(text.contains("clipboard-images"));
    let path = PathBuf::from(&text[1..text.len() - 1]);
    assert_eq!(std::fs::read(&path).unwrap(), [1, 2, 3, 4]);
    let _ = std::fs::remove_file(path);
}

#[test]
fn clipboard_images_get_private_unpredictable_files_and_expire() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("clipboard-images");
    let image = gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Png, vec![1, 2, 3]);
    let first = write_clipboard_image_in(&dir, &image).unwrap();
    let second = write_clipboard_image_in(&dir, &image).unwrap();
    assert_ne!(first, second, "names must not be derived from the image");
    assert_eq!(std::fs::read(&second).unwrap(), [1, 2, 3]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&second), 0o600);
    }

    let unrelated = dir.join("notes.txt");
    std::fs::write(&unrelated, b"keep").unwrap();
    let old = std::time::SystemTime::now() - CLIPBOARD_IMAGE_MAX_AGE * 2;
    for path in [&first, &unrelated] {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(old)
            .unwrap();
    }
    let third = write_clipboard_image_in(&dir, &image).unwrap();
    assert!(!first.exists(), "expired pastes are removed");
    assert!(second.exists() && third.exists() && unrelated.exists());
}

#[test]
fn clipboard_file_paths_are_separated_and_shell_quoted() {
    let paths = gpui_kit::ExternalPaths(
        [
            PathBuf::from("/tmp/a b.txt"),
            PathBuf::from("/tmp/it's.txt"),
        ]
        .into_iter()
        .collect(),
    );
    let item = gpui_kit::ClipboardEntry::ExternalPaths(paths).into();
    assert_eq!(
        clipboard_item_to_terminal_paste_input(&item, PathQuoteStyle::Posix).unwrap(),
        Some(b"'/tmp/a b.txt' '/tmp/it'\\''s.txt'".to_vec())
    );
}

#[test]
fn clipboard_file_paths_win_over_finder_display_names() {
    let paths = gpui_kit::ExternalPaths([PathBuf::from("/tmp/q7n4w2.kpx")].into_iter().collect());
    let entries = [
        gpui_kit::ClipboardEntry::ExternalPaths(paths),
        gpui_kit::ClipboardEntry::String("q7n4w2.kpx".to_string().into()),
    ];
    assert_eq!(
        clipboard_entries_to_terminal_paste_input(&entries, PathQuoteStyle::Posix).unwrap(),
        Some(b"'/tmp/q7n4w2.kpx'".to_vec())
    );
}

#[test]
fn clipboard_image_with_text_still_pastes_the_text() {
    let image = gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Png, vec![1, 2, 3, 4]);
    let entries = [
        gpui_kit::ClipboardEntry::String("screenshot".to_string().into()),
        gpui_kit::ClipboardEntry::Image(image),
    ];
    assert_eq!(
        clipboard_entries_to_terminal_paste_input(&entries, PathQuoteStyle::Posix).unwrap(),
        Some(b"screenshot".to_vec())
    );
}

#[test]
fn clipboard_text_is_pasted_without_shell_quoting() {
    let item = gpui_kit::ClipboardItem::new_string("echo hello\n".into());
    assert_eq!(
        clipboard_item_to_terminal_paste_input(&item, PathQuoteStyle::Posix).unwrap(),
        Some(b"echo hello\n".to_vec())
    );
}

#[test]
fn kitty_png_clipboard_item_preserves_png_bytes_and_format() {
    let png = [137, 80, 78, 71, 13, 10, 26, 10];
    let item = kitty_png_clipboard_item(&png);

    assert_eq!(item.entries().len(), 1);
    let gpui_kit::ClipboardEntry::Image(image) = &item.entries()[0] else {
        panic!("Kitty clipboard item should contain an image");
    };
    assert_eq!(image.format(), gpui_kit::ImageFormat::Png);
    assert_eq!(image.bytes(), png);
}

#[test]
fn image_extension_matches_expected_file_suffixes() {
    assert_eq!(image_extension(gpui_kit::ImageFormat::Gif), "gif");
    assert_eq!(image_extension(gpui_kit::ImageFormat::Png), "png");
    assert_eq!(image_extension(gpui_kit::ImageFormat::Jpeg), "jpg");
    assert_eq!(image_extension(gpui_kit::ImageFormat::Webp), "webp");
    assert_eq!(image_extension(gpui_kit::ImageFormat::Bmp), "bmp");
    assert_eq!(image_extension(gpui_kit::ImageFormat::Tiff), "tiff");
    assert_eq!(image_extension(gpui_kit::ImageFormat::Svg), "svg");
}

#[test]
fn modifier_transitions_synthesize_shift_press_with_super_held() {
    let previous = Modifiers {
        platform: true,
        ..Modifiers::default()
    };
    let current = Modifiers {
        platform: true,
        shift: true,
        ..Modifiers::default()
    };

    let events = modifier_transition_events(previous, current);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0.key, "shift");
    assert_eq!(events[0].1, termy_core::TerminalKeyEventKind::Press);
    assert!(events[0].0.modifiers.platform);
    assert!(events[0].0.modifiers.shift);
}

#[test]
fn overlay_owned_modifier_changes_skip_terminal_events() {
    let previous = Modifiers {
        platform: true,
        ..Modifiers::default()
    };

    assert!(terminal_modifier_transition_events(previous, Modifiers::default(), true).is_empty());
}

#[test]
fn non_active_pane_writes_skip_terminal_input_prepare() {
    assert!(should_prepare_terminal_input_write(
        Some("%pane-1"),
        "%pane-1"
    ));
    assert!(!should_prepare_terminal_input_write(
        Some("%pane-1"),
        "%pane-2"
    ));
    assert!(!should_prepare_terminal_input_write(None, "%pane-1"));
}

#[test]
fn deferred_ime_key_release_is_cleared_without_forwarding() {
    let mut deferred = HashSet::from(["a".to_string()]);

    assert!(take_deferred_ime_key_release(&mut deferred, "a"));
    assert!(!take_deferred_ime_key_release(&mut deferred, "a"));
    assert!(deferred.is_empty());
}

#[test]
fn consumed_key_release_is_dropped_after_transient_input_closes() {
    let mut pending = HashMap::from([("enter".to_string(), PendingKeyRelease::Consumed)]);

    assert_eq!(
        take_pending_key_release_action(&mut pending, "enter"),
        PendingKeyReleaseAction::Drop
    );
    assert!(pending.is_empty());
}

#[test]
fn release_without_a_forwarded_press_is_dropped() {
    // A key binding handled the press, so the terminal never received it.
    let mut pending = HashMap::new();

    assert_eq!(
        take_pending_key_release_action(&mut pending, "t"),
        PendingKeyReleaseAction::Drop
    );
}

#[test]
fn forwarded_key_release_routes_to_original_pane() {
    let mut pending = HashMap::from([(
        "enter".to_string(),
        PendingKeyRelease::Terminal {
            pane_id: "%pane-1".to_string(),
        },
    )]);

    assert_eq!(
        take_pending_key_release_action(&mut pending, "enter"),
        PendingKeyReleaseAction::ForwardToPane("%pane-1".to_string())
    );
    assert!(pending.is_empty());
}

#[test]
fn modifier_releases_keep_their_original_panes() {
    let mut pending = HashMap::from([
        (
            "shift".to_string(),
            PendingKeyRelease::Terminal {
                pane_id: "%left".to_string(),
            },
        ),
        (
            "super".to_string(),
            PendingKeyRelease::Terminal {
                pane_id: "%right".to_string(),
            },
        ),
    ]);

    assert_eq!(
        take_pending_key_release_action(&mut pending, "shift"),
        PendingKeyReleaseAction::ForwardToPane("%left".to_string())
    );
    assert_eq!(
        take_pending_key_release_action(&mut pending, "super"),
        PendingKeyReleaseAction::ForwardToPane("%right".to_string())
    );
    assert!(pending.is_empty());
}
