use super::*;
use termy_core::plugin_runtime::PluginSelectOption;

fn lifecycle_snapshot(
    tab_id: u64,
    tab_index: usize,
    working_directory: &str,
    active_command: Option<&str>,
) -> PluginLifecycleSnapshot {
    PluginLifecycleSnapshot {
        active_tab_id: Some(tab_id.to_string()),
        active_tab_index: Some(tab_index),
        active_pane_id: Some(format!("pane-{tab_id}")),
        tab_ids: BTreeSet::from([tab_id.to_string()]),
        working_directory: Some(working_directory.to_string()),
        active_command: active_command.map(str::to_string),
    }
}

fn command(inputs: Vec<PluginInput>) -> PluginCommand {
    PluginCommand {
        plugin_id: "test".to_string(),
        plugin_name: "Test".to_string(),
        id: "run".to_string(),
        title: "Test: Run".to_string(),
        placements: vec![PluginCommandPlacement::CommandPalette],
        keywords: Vec::new(),
        status: None,
        disabled_reason: None,
        icon: PluginIcon::Command,
        inputs,
        when: termy_core::plugin_runtime::PluginCommandWhen::default(),
        timeout_ms: 10_000,
    }
}

#[test]
fn plugin_placements_filter_and_sort_context_menu_commands() {
    let placed = |plugin_name: &str,
                  id: &str,
                  title: &str,
                  placements: Vec<PluginCommandPlacement>,
                  disabled_reason: Option<&str>| {
        let mut command = command(Vec::new());
        command.plugin_id = plugin_name.to_lowercase();
        command.plugin_name = plugin_name.to_string();
        command.id = id.to_string();
        command.title = title.to_string();
        command.placements = placements;
        command.disabled_reason = disabled_reason.map(str::to_string);
        command
    };
    let commands = vec![
        placed(
            "Zulu",
            "palette",
            "Palette only",
            vec![PluginCommandPlacement::CommandPalette],
            None,
        ),
        placed(
            "Beta",
            "later",
            "Later",
            vec![PluginCommandPlacement::TerminalContextMenu],
            None,
        ),
        placed(
            "alpha",
            "disabled",
            "Disabled but visible",
            vec![PluginCommandPlacement::TerminalContextMenu],
            Some("Unavailable"),
        ),
        placed("Hidden", "hidden", "Hidden", Vec::new(), None),
    ];

    let visible = placed_plugin_commands(commands, PluginCommandPlacement::TerminalContextMenu);

    assert_eq!(
        visible
            .iter()
            .map(|command| command.id.as_str())
            .collect::<Vec<_>>(),
        vec!["disabled", "later"]
    );
    assert_eq!(
        visible[0].disabled_reason.as_deref(),
        Some("Unavailable"),
        "disabled commands remain visible so the menu can explain their state"
    );
}

#[test]
fn input_session_uses_defaults_and_tracks_progress() {
    let session = PluginInputSession::new(
        command(vec![
            PluginInput::Text {
                id: "name".to_string(),
                label: "Name".to_string(),
                placeholder: None,
                default_value: Some("Termy".to_string()),
                required: true,
                max_length: 80,
            },
            PluginInput::Confirm {
                id: "confirm".to_string(),
                label: "Continue?".to_string(),
                default_value: true,
            },
        ]),
        "test-revision".to_string(),
    );
    assert_eq!(session.input_prefill(), "Termy");
    assert_eq!(session.progress_label(), "1 of 2");
    assert!(!session.is_last_input());
    assert!(!session.can_go_back());
}

#[test]
fn next_plugin_input_resets_selection_to_the_default_row() {
    let mut state = CommandPaletteState::new(false);
    let option = |label: &str, value: bool| CommandPaletteItem {
        title: label.to_string(),
        keywords: String::new(),
        enabled: true,
        status_hint: None,
        tmux_status_hint: None,
        kind: CommandPaletteItemKind::PluginInputOption {
            value: Value::Bool(value),
            icon: PluginIcon::Command,
        },
    };
    state.set_items(vec![option("Yes", true), option("No", false)]);
    assert!(state.set_selected_filtered_index(1));

    state.reset_for_next_plugin_input();

    assert_eq!(state.selected_filtered_index(), Some(0));
}

#[test]
fn moving_back_preserves_select_and_confirm_values() {
    let mut session = PluginInputSession::new(
        command(vec![
            PluginInput::Select {
                id: "target".to_string(),
                label: "Target".to_string(),
                placeholder: None,
                default_value: Some("debug".to_string()),
                required: true,
                options: vec![
                    PluginSelectOption {
                        value: "debug".to_string(),
                        label: "Debug".to_string(),
                        keywords: Vec::new(),
                        status: None,
                    },
                    PluginSelectOption {
                        value: "release".to_string(),
                        label: "Release".to_string(),
                        keywords: Vec::new(),
                        status: None,
                    },
                ],
            },
            PluginInput::Confirm {
                id: "confirmed".to_string(),
                label: "Continue?".to_string(),
                default_value: true,
            },
            PluginInput::Text {
                id: "name".to_string(),
                label: "Name".to_string(),
                placeholder: None,
                default_value: None,
                required: false,
                max_length: 80,
            },
        ]),
        "test-revision".to_string(),
    );
    assert_eq!(session.placeholder(), "Target");
    assert_eq!(session.progress_label(), "1 of 3");
    session
        .values
        .insert("target".to_string(), Value::String("release".to_string()));
    session
        .values
        .insert("confirmed".to_string(), Value::Bool(false));
    session.input_index = 2;

    assert_eq!(session.move_back().as_deref(), Some(""));
    assert_eq!(
        session.current_input().map(PluginInput::id),
        Some("confirmed")
    );
    assert_eq!(
        session.values.get("confirmed").and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(session.move_back().as_deref(), Some(""));
    assert_eq!(session.current_input().map(PluginInput::id), Some("target"));
    assert_eq!(
        session.values.get("target").and_then(Value::as_str),
        Some("release")
    );
}

#[test]
fn stored_plugin_input_value_is_promoted_ahead_of_the_default() {
    let option = |label: &str, value: Value| CommandPaletteItem {
        title: label.to_string(),
        keywords: String::new(),
        enabled: true,
        status_hint: None,
        tmux_status_hint: None,
        kind: CommandPaletteItemKind::PluginInputOption {
            value,
            icon: PluginIcon::Command,
        },
    };
    let cases = [
        (
            vec![
                option("Debug", Value::String("debug".to_string())),
                option("Release", Value::String("release".to_string())),
            ],
            Value::String("release".to_string()),
        ),
        (
            vec![
                option("Yes", Value::Bool(true)),
                option("No", Value::Bool(false)),
            ],
            Value::Bool(false),
        ),
    ];

    for (mut items, stored_value) in cases {
        promote_stored_plugin_input_value(&mut items, Some(&stored_value));

        let CommandPaletteItemKind::PluginInputOption { value, .. } = &items[0].kind else {
            panic!("stored plugin input was not promoted")
        };
        assert_eq!(value, &stored_value);
    }
}

#[test]
fn picker_prefers_stored_value_then_falls_back_to_default() {
    let stored = Value::String("stored".to_string());
    assert_eq!(
        preferred_pick_value(Some(&stored), Some("default")),
        Some(stored)
    );
    assert_eq!(
        preferred_pick_value(None, Some("default")),
        Some(Value::String("default".to_string()))
    );
    assert_eq!(preferred_pick_value(None, None), None);
}

#[test]
fn beginning_plugin_inputs_opens_the_palette_for_keybinding_invocation() {
    let mut palette = CommandPaletteState::new(false);
    assert!(!palette.is_open());

    palette.begin_plugin_inputs(PluginInputSession::new(
        command(vec![PluginInput::Confirm {
            id: "confirm".to_string(),
            label: "Continue?".to_string(),
            default_value: false,
        }]),
        "revision".to_string(),
    ));

    assert!(palette.is_open());
    assert_eq!(palette.mode(), CommandPaletteMode::PluginInputs);
}

#[test]
fn plugin_selected_text_is_utf8_safe_and_bounded() {
    let text = format!("{}😀", "a".repeat(PLUGIN_SELECTED_TEXT_MAX_BYTES - 1));
    let (selected_text, truncated) = plugin_selected_text(Some(text));

    assert!(truncated);
    let selected_text = selected_text.expect("selected text");
    assert!(selected_text.len() <= PLUGIN_SELECTED_TEXT_MAX_BYTES);
    assert_eq!(
        selected_text,
        "a".repeat(PLUGIN_SELECTED_TEXT_MAX_BYTES - 1)
    );
}

#[test]
fn lifecycle_tracker_emits_working_directory_changes_once() {
    let now = Instant::now();
    let initial = lifecycle_snapshot(7, 0, "/old", None);
    let changed = lifecycle_snapshot(7, 0, "/new", None);
    let mut tracker = PluginLifecycleTracker {
        snapshot: initial,
        command_started_at: None,
    };

    assert_eq!(
        tracker.update(changed.clone(), false, now),
        vec![PluginEvent::WorkingDirectoryChanged {
            previous_working_directory: Some("/old".to_string()),
            working_directory: Some("/new".to_string()),
        }]
    );
    assert!(tracker.update(changed, false, now).is_empty());
}

#[test]
fn lifecycle_tracker_infers_tmux_command_completion() {
    let started = Instant::now();
    let mut tracker = PluginLifecycleTracker {
        snapshot: lifecycle_snapshot(7, 0, "/tmp", Some("cargo test")),
        command_started_at: Some(started),
    };

    assert_eq!(
        tracker.update(
            lifecycle_snapshot(7, 0, "/tmp", None),
            true,
            started + Duration::from_millis(150),
        ),
        vec![PluginEvent::CommandFinished {
            command: Some("cargo test".to_string()),
            exit_code: None,
            duration_ms: Some(150),
        }]
    );
}

#[test]
fn lifecycle_tracker_treats_tab_activation_as_a_new_baseline() {
    let now = Instant::now();
    let mut tracker = PluginLifecycleTracker {
        snapshot: lifecycle_snapshot(7, 2, "/old", Some("cargo test")),
        command_started_at: Some(now - Duration::from_secs(1)),
    };

    assert_eq!(
        tracker.update(
            lifecycle_snapshot(9, 4, "/new", Some("bun test")),
            true,
            now,
        ),
        vec![
            PluginEvent::TabCreated {
                tab_id: "9".to_string(),
            },
            PluginEvent::TabClosed {
                tab_id: "7".to_string(),
            },
            PluginEvent::TabActivated {
                previous_tab_index: Some(2),
                previous_tab_id: Some("7".to_string()),
            },
            PluginEvent::PaneActivated {
                previous_pane_id: Some("pane-7".to_string()),
            },
            PluginEvent::WorkingDirectoryChanged {
                previous_working_directory: Some("/old".to_string()),
                working_directory: Some("/new".to_string()),
            },
            PluginEvent::CommandFinished {
                command: Some("cargo test".to_string()),
                exit_code: None,
                duration_ms: Some(1_000),
            },
            PluginEvent::CommandStarted {
                command: Some("bun test".to_string()),
            },
        ]
    );
    assert_eq!(tracker.snapshot.active_tab_id, Some("9".to_string()));
    assert_eq!(tracker.command_started_at, Some(now));
}
