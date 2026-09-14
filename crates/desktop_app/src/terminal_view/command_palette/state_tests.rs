use super::*;
use tempfile::tempdir;

fn command_item(title: &str, keywords: &str, action: CommandAction) -> CommandPaletteItem {
    CommandPaletteItem::command_with_state(title, keywords, action, true, None)
}

fn ranked_actions(items: &[CommandPaletteItem], query: &str) -> Vec<CommandAction> {
    ranked_actions_with_recents(items, query, &CommandPaletteRecents::default())
}

fn ranked_actions_with_recents(
    items: &[CommandPaletteItem],
    query: &str,
    recents: &CommandPaletteRecents,
) -> Vec<CommandAction> {
    rank_command_palette_items(items, query, recents, CommandPaletteRanking::ByScore)
        .into_iter()
        .filter_map(|matched| match items[matched.item_index].kind {
            CommandPaletteItemKind::Command(action) => Some(action),
            _ => None,
        })
        .collect()
}

#[test]
fn command_exists_in_path_finds_executable_file() {
    let dir = tempdir().expect("tempdir");
    let executable_path = dir.path().join("codex");
    std::fs::write(&executable_path, "#!/bin/sh\n").expect("write executable");
    #[cfg(unix)]
    std::fs::set_permissions(&executable_path, std::fs::Permissions::from_mode(0o755))
        .expect("chmod executable");

    let path = std::env::join_paths([dir.path()]).expect("join PATH");
    assert!(command_exists_in_path(
        "codex",
        path.to_string_lossy().as_ref()
    ));
}

#[cfg(unix)]
#[test]
fn command_exists_in_path_rejects_non_executable_file() {
    let dir = tempdir().expect("tempdir");
    let file_path = dir.path().join("codex");
    std::fs::write(&file_path, "#!/bin/sh\n").expect("write file");
    std::fs::set_permissions(&file_path, std::fs::Permissions::from_mode(0o644))
        .expect("chmod file");

    let path = std::env::join_paths([dir.path()]).expect("join PATH");
    assert!(!command_exists_in_path(
        "codex",
        path.to_string_lossy().as_ref()
    ));
}

#[test]
fn query_re_ranks_prefix_titles_first_and_hides_keyword_only_rows() {
    let items = vec![
        command_item("Close Tab", "remove tab", CommandAction::CloseTab),
        command_item("Rename Tab", "title name", CommandAction::RenameTab),
        command_item(
            "Restart App",
            "relaunch reopen restart",
            CommandAction::RestartApp,
        ),
        command_item("Reset Zoom", "font default", CommandAction::ZoomReset),
        command_item(
            "Check for Updates",
            "release version updater",
            CommandAction::CheckForUpdates,
        ),
    ];

    let actions = ranked_actions(&items, "re");

    // "Close Tab" only matches through its keywords, so it stays hidden
    // while titles match. "Check for Updates" matches as a scattered
    // subsequence and ranks below the three prefix matches.
    assert_eq!(
        actions,
        vec![
            CommandAction::RenameTab,
            CommandAction::RestartApp,
            CommandAction::ZoomReset,
            CommandAction::CheckForUpdates,
        ]
    );
}

#[test]
fn query_uses_keywords_when_no_titles_match() {
    let items = vec![
        command_item("Zoom In", "increase", CommandAction::ZoomIn),
        command_item("Zoom Out", "decrease", CommandAction::ZoomOut),
        command_item("Reset Zoom", "default", CommandAction::ZoomReset),
    ];

    let actions = ranked_actions(&items, "decrease");

    assert_eq!(actions, vec![CommandAction::ZoomOut]);
}

#[test]
fn query_splits_hyphenated_terms_on_non_alphanumeric_boundaries() {
    let items = vec![
        command_item("Tokyo Night", "theme", CommandAction::SwitchTheme),
        command_item("Tomorrow Night", "theme", CommandAction::SwitchTheme),
        command_item("Nord", "theme", CommandAction::SwitchTheme),
    ];

    let matches = rank_command_palette_items(
        &items,
        "tokyo-night",
        &CommandPaletteRecents::default(),
        CommandPaletteRanking::ByScore,
    );
    let titles: Vec<&str> = matches
        .iter()
        .map(|matched| items[matched.item_index].title.as_str())
        .collect();

    assert_eq!(titles, vec!["Tokyo Night"]);
}

#[test]
fn fuzzy_initials_match_and_rank_above_scattered_hits() {
    let items = vec![
        command_item("Close Tab", "close", CommandAction::CloseTab),
        command_item("New Tab", "new", CommandAction::NewTab),
        command_item("Copy Text", "copy", CommandAction::Copy),
    ];

    let actions = ranked_actions(&items, "nt");

    assert_eq!(actions.first(), Some(&CommandAction::NewTab));
}

#[test]
fn recent_commands_are_boosted_and_lead_the_unfiltered_list() {
    let items = vec![
        command_item("New Tab", "tab", CommandAction::NewTab),
        command_item("Close Tab", "tab", CommandAction::CloseTab),
        command_item("Rename Tab", "tab", CommandAction::RenameTab),
    ];

    let mut recents = CommandPaletteRecents::default();
    recents.push(
        CommandAction::RenameTab
            .to_command_id()
            .config_name()
            .into(),
    );

    assert_eq!(
        ranked_actions_with_recents(&items, "", &recents),
        vec![
            CommandAction::RenameTab,
            CommandAction::NewTab,
            CommandAction::CloseTab,
        ]
    );

    // The boost is a tiebreaker: a stronger title match still wins.
    assert_eq!(
        ranked_actions_with_recents(&items, "tab", &recents).first(),
        Some(&CommandAction::RenameTab)
    );
    assert_eq!(
        ranked_actions_with_recents(&items, "close", &recents).first(),
        Some(&CommandAction::CloseTab)
    );
}

#[test]
fn unavailable_rows_keep_builder_order_in_curated_modes() {
    let items = vec![
        CommandPaletteItem::command_with_state(
            "Install CLI",
            "cli",
            CommandAction::InstallCli,
            false,
            Some("Installed"),
        ),
        command_item("New Tab", "tab", CommandAction::NewTab),
    ];

    let matches = rank_command_palette_items(
        &items,
        "",
        &CommandPaletteRecents::default(),
        CommandPaletteRanking::PreserveOrder,
    );
    let order: Vec<usize> = matches.iter().map(|matched| matched.item_index).collect();

    assert_eq!(order, vec![0, 1]);
}

#[test]
fn title_highlights_cover_the_matched_characters() {
    let items = vec![command_item("New Tab", "tab", CommandAction::NewTab)];

    let matches = rank_command_palette_items(
        &items,
        "nt",
        &CommandPaletteRecents::default(),
        CommandPaletteRanking::ByScore,
    );

    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].title_highlights, vec![0..1, 4..5]);
}

#[test]
fn curated_modes_keep_builder_order() {
    let items = vec![
        command_item("Zoom Out", "zoom", CommandAction::ZoomOut),
        command_item("Zoom", "zoom", CommandAction::ZoomIn),
    ];

    let matches = rank_command_palette_items(
        &items,
        "zoom",
        &CommandPaletteRecents::default(),
        CommandPaletteRanking::PreserveOrder,
    );
    let order: Vec<usize> = matches.iter().map(|matched| matched.item_index).collect();

    assert_eq!(order, vec![0, 1]);
}

#[test]
fn filtered_index_selection_clamps_after_query_change() {
    let mut state = CommandPaletteState::new(true);
    state.set_items(vec![
        command_item("New Tab", "tab", CommandAction::NewTab),
        command_item("Close Tab", "tab", CommandAction::CloseTab),
        command_item("Switch Theme", "theme", CommandAction::SwitchTheme),
    ]);
    assert!(state.set_selected_filtered_index(2));

    state.input_mut().set_text("close".to_string());
    state.refilter_current_query();
    assert_eq!(state.filtered_len(), 1);
    assert_eq!(state.selected_filtered_index(), Some(0));

    state.input_mut().set_text(String::new());
    state.refilter_current_query();
    assert_eq!(state.filtered_len(), 3);
    assert_eq!(state.selected_filtered_index(), Some(0));
}

#[test]
fn move_selection_handles_empty_and_bounds_without_panics() {
    let mut state = CommandPaletteState::new(true);
    assert!(!state.move_selection_up());
    assert!(!state.move_selection_down());

    state.set_items(vec![
        command_item("New Tab", "tab", CommandAction::NewTab),
        command_item("Close Tab", "tab", CommandAction::CloseTab),
    ]);

    assert!(!state.move_selection_up());
    assert!(state.move_selection_down());
    assert!(!state.move_selection_down());
    assert!(state.move_selection_up());
}

#[test]
fn target_scroll_y_only_moves_when_selection_leaves_viewport() {
    let rows = COMMAND_PALETTE_MAX_ITEMS;
    assert_eq!(command_palette_target_scroll_y(0.0, 2, 12, rows), Some(0.0));
    assert_eq!(
        command_palette_target_scroll_y(0.0, 9, 12, rows),
        Some((10.0 * COMMAND_PALETTE_ROW_HEIGHT) - command_palette_viewport_height(rows))
    );
    assert_eq!(
        command_palette_target_scroll_y(90.0, 0, 12, rows),
        Some(0.0)
    );
    assert_eq!(command_palette_target_scroll_y(0.0, 0, 0, rows), None);
}

#[test]
fn target_scroll_y_follows_a_shrunken_viewport() {
    // With only three rows visible, selecting row 5 has to scroll further
    // than it would in the full-height list.
    let short = command_palette_target_scroll_y(0.0, 5, 12, 3).expect("target");
    let tall = command_palette_target_scroll_y(0.0, 5, 12, 8).expect("target");
    assert!(short > tall, "short {short} should scroll past tall {tall}");
    assert_eq!(
        short,
        6.0 * COMMAND_PALETTE_ROW_HEIGHT - 3.0 * COMMAND_PALETTE_ROW_HEIGHT
    );
}

#[test]
fn hover_is_ignored_until_the_pointer_actually_moves() {
    let mut state = CommandPaletteState::new(true);
    state.set_items(vec![
        command_item("New Tab", "tab", CommandAction::NewTab),
        command_item("Close Tab", "tab", CommandAction::CloseTab),
        command_item("Rename Tab", "tab", CommandAction::RenameTab),
    ]);

    let resting = point(px(10.0), px(20.0));
    assert!(
        state.accept_hover_at(resting),
        "first sample is a real move"
    );

    // Keyboard navigation locks hover; the platform then re-sends the same
    // pointer position as the list scrolls beneath the cursor.
    assert!(state.move_selection_down());
    assert_eq!(state.selected_filtered_index(), Some(1));
    assert!(!state.accept_hover_at(resting));
    assert_eq!(state.selected_filtered_index(), Some(1));

    // A genuine move re-enables hover.
    assert!(state.accept_hover_at(point(px(10.0), px(48.0))));
}

#[test]
fn typing_locks_hover_so_results_do_not_jump_to_the_cursor() {
    let mut state = CommandPaletteState::new(true);
    state.set_items(vec![
        command_item("New Tab", "tab", CommandAction::NewTab),
        command_item("Close Tab", "tab", CommandAction::CloseTab),
    ]);

    let resting = point(px(10.0), px(20.0));
    assert!(state.accept_hover_at(resting));

    state.input_mut().set_text("tab".to_string());
    state.refilter_current_query();

    assert!(!state.accept_hover_at(resting));
}

#[test]
fn page_and_edge_moves_respect_the_visible_row_count() {
    let mut state = CommandPaletteState::new(true);
    state.set_visible_rows(3);
    state.set_items(
        (0..10)
            .map(|index| command_item(&format!("Command {index}"), "cmd", CommandAction::NewTab))
            .collect(),
    );

    assert!(state.move_selection_page(CommandPaletteScrollDirection::Down));
    assert_eq!(state.selected_filtered_index(), Some(3));
    assert!(state.move_selection_page(CommandPaletteScrollDirection::Down));
    assert_eq!(state.selected_filtered_index(), Some(6));
    assert!(state.move_selection_page(CommandPaletteScrollDirection::Up));
    assert_eq!(state.selected_filtered_index(), Some(3));

    assert!(state.move_selection_to_edge(CommandPaletteScrollDirection::Down));
    assert_eq!(state.selected_filtered_index(), Some(9));
    assert!(state.move_selection_to_edge(CommandPaletteScrollDirection::Up));
    assert_eq!(state.selected_filtered_index(), Some(0));

    // Already at the edge: no change reported, no panic.
    assert!(!state.move_selection_page(CommandPaletteScrollDirection::Up));
    assert!(!state.move_selection_to_edge(CommandPaletteScrollDirection::Up));
}

#[test]
fn page_moves_on_an_empty_list_do_nothing() {
    let mut state = CommandPaletteState::new(true);
    assert!(!state.move_selection_page(CommandPaletteScrollDirection::Down));
    assert!(!state.move_selection_to_edge(CommandPaletteScrollDirection::Down));
}

#[test]
fn layout_keeps_preferred_geometry_on_a_roomy_window() {
    let layout = command_palette_layout_for_viewport(1440.0, 900.0);

    assert_eq!(layout.width, COMMAND_PALETTE_WIDTH);
    assert_eq!(layout.top_offset, COMMAND_PALETTE_TOP_OFFSET);
    assert_eq!(layout.visible_rows, COMMAND_PALETTE_MAX_ITEMS);
}

#[test]
fn layout_shrinks_to_fit_a_small_window() {
    let layout = command_palette_layout_for_viewport(420.0, 320.0);

    assert_eq!(
        layout.width,
        420.0 - COMMAND_PALETTE_VIEWPORT_MARGIN_X * 2.0
    );
    assert!(layout.top_offset < COMMAND_PALETTE_TOP_OFFSET);
    assert!(layout.visible_rows < COMMAND_PALETTE_MAX_ITEMS);
    assert!(layout.visible_rows >= COMMAND_PALETTE_MIN_ITEMS);
}

#[test]
fn layout_never_exceeds_a_tiny_viewport() {
    let layout = command_palette_layout_for_viewport(200.0, 120.0);

    assert!(layout.width <= 200.0);
    assert_eq!(layout.visible_rows, COMMAND_PALETTE_MIN_ITEMS);
    assert!(layout.top_offset >= COMMAND_PALETTE_MIN_TOP_OFFSET);
}

#[test]
fn next_scroll_y_is_dt_based_and_respects_bounds() {
    let slow = command_palette_next_scroll_y(0.0, 120.0, 300.0, 1.0 / 240.0);
    let fast = command_palette_next_scroll_y(0.0, 120.0, 300.0, 0.05);
    assert!(fast > slow);
    assert!(fast <= 300.0);

    let snapped = command_palette_next_scroll_y(59.7, 60.0, 300.0, 1.0 / 60.0);
    assert_eq!(snapped, 60.0);

    let clamped = command_palette_next_scroll_y(280.0, 400.0, 300.0, 0.05);
    assert!(clamped <= 300.0);
}

#[test]
fn ordered_theme_ids_pin_current_theme_first() {
    let ordered = ordered_theme_ids_for_palette(
        vec![
            "nord".to_string(),
            "termy".to_string(),
            "dracula".to_string(),
            "nord".to_string(),
        ],
        "termy",
    );

    assert_eq!(
        ordered,
        vec!["termy", "dracula", "nord", SHELL_DECIDE_THEME_ID]
    );

    let ordered_with_missing_current = ordered_theme_ids_for_palette(
        vec!["nord".to_string(), "dracula".to_string()],
        "tokyo-night",
    );

    assert_eq!(
        ordered_with_missing_current,
        vec!["tokyo-night", "dracula", "nord", SHELL_DECIDE_THEME_ID]
    );
}

#[test]
fn close_resets_to_command_mode_and_clears_transient_state() {
    let mut state = CommandPaletteState::new(false);
    state.open(CommandPaletteMode::Themes);
    state.input_mut().set_text("theme".to_string());
    state.set_items(vec![CommandPaletteItem::command_with_state(
        "New Tab",
        "tab",
        CommandAction::NewTab,
        true,
        None,
    )]);
    state.set_selected_filtered_index(999);
    state.set_scroll_target_y(12.0);
    state.set_scroll_max_y_for_count(12);
    state.start_scroll_animation(Instant::now());

    state.close();

    assert!(!state.is_open());
    assert_eq!(state.mode(), CommandPaletteMode::Commands);
    assert!(state.input().text().is_empty());
    assert_eq!(state.filtered_len(), 0);
    assert!(state.scroll_target_y().is_none());
    assert_eq!(state.scroll_max_y(), 0.0);
    assert!(!state.is_scroll_animating());
}

#[test]
fn app_info_entry_truncates_non_ascii_without_panicking() {
    let value = "ø".repeat(61);
    let item = CommandPaletteItem::app_info_entry("CPU", value.clone());

    assert!(item.title.contains('…'));
    assert_eq!(
        item.kind,
        CommandPaletteItemKind::AppInfoEntry {
            label: "CPU",
            value
        }
    );
}

#[test]
fn empty_query_hides_tasks_in_scored_lists_but_keeps_them_in_browsers() {
    let items = vec![
        command_item("New Tab", "tab", CommandAction::NewTab),
        CommandPaletteItem::task("build", "cargo build", None, None),
    ];
    let recents = CommandPaletteRecents::default();

    for query in ["", "   "] {
        let scored =
            rank_command_palette_items(&items, query, &recents, CommandPaletteRanking::ByScore);
        assert_eq!(
            scored.len(),
            1,
            "query {query:?} should hide tasks in the root list"
        );
        assert!(matches!(
            items[scored[0].item_index].kind,
            CommandPaletteItemKind::Command(_)
        ));
    }

    let preserved =
        rank_command_palette_items(&items, "", &recents, CommandPaletteRanking::PreserveOrder);
    assert_eq!(
        preserved.len(),
        2,
        "the Tasks browser still lists tasks with an empty query"
    );
}
