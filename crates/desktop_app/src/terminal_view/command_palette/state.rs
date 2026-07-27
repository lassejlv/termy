use super::super::*;
use super::fuzzy::{self, FuzzyMatch, FuzzyQuery};
use super::plugins::PluginInputSession;
use super::recents::CommandPaletteRecents;
use super::state_layouts::SavedLayoutIntent;
use super::state_tmux::{TmuxSessionIntent, TmuxSessionRow, TmuxSessionStatusHint};
use crate::config::SHELL_DECIDE_THEME_ID;
use gpui::{Pixels, Point, UniformListScrollHandle};
use std::collections::HashMap;
use std::ops::Range;
#[cfg(unix)]
#[cfg(test)]
use std::os::unix::fs::PermissionsExt;
use termy_plugin_runtime::PluginIcon;
use termy_terminal_ui::TmuxSocketTarget;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in super::super) enum CommandPaletteMode {
    Commands,
    Themes,
    TmuxSessions,
    Layouts,
    Tasks,
    PluginInputs,
    AppInfo,
}

impl CommandPaletteMode {
    /// Modes whose rows come from a flat catalog rank by match quality; modes
    /// that build curated lists (sessions, layouts, tasks, plugin options) keep
    /// the order their builder chose.
    fn ranking(self) -> CommandPaletteRanking {
        match self {
            Self::Commands | Self::Themes | Self::AppInfo => CommandPaletteRanking::ByScore,
            Self::TmuxSessions | Self::Layouts | Self::Tasks | Self::PluginInputs => {
                CommandPaletteRanking::PreserveOrder
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CommandPaletteScrollDirection {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in super::super) enum TaskIntent {
    Browse,
    CreateGlobalInput,
    CreateLayoutInput,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in super::super) enum CommandPaletteCommandIntent {
    Browse,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CommandPaletteItemKind {
    Command(CommandAction),
    PluginCommand {
        plugin_id: String,
        command_id: String,
        icon: PluginIcon,
    },
    PluginInputSubmit {
        icon: PluginIcon,
    },
    PluginInputOption {
        value: serde_json::Value,
        icon: PluginIcon,
    },
    Theme(String),
    TmuxSessionAttachOrSwitch {
        session_name: String,
        socket_target: TmuxSocketTarget,
    },
    TmuxSessionCreateAndAttach {
        session_name: String,
        socket_target: TmuxSocketTarget,
    },
    TmuxSessionDetachCurrent,
    TmuxSessionOpenRenameMode,
    TmuxSessionOpenKillMode,
    TmuxSessionRenameSelect {
        session_name: String,
        socket_target: TmuxSocketTarget,
    },
    TmuxSessionRenameApply {
        current_session_name: String,
        next_session_name: String,
        socket_target: TmuxSocketTarget,
    },
    TmuxSessionKill {
        session_name: String,
        socket_target: TmuxSocketTarget,
    },
    SavedLayoutOpen {
        layout_name: String,
    },
    SavedLayoutOpenTasksMode {
        layout_name: String,
    },
    SavedLayoutOpenSaveMode,
    SavedLayoutSaveAs {
        layout_name: String,
    },
    SavedLayoutOpenRenameMode,
    SavedLayoutRenameSelect {
        layout_name: String,
    },
    SavedLayoutRenameApply {
        current_layout_name: String,
        next_layout_name: String,
    },
    SavedLayoutOpenDeleteMode,
    SavedLayoutDelete {
        layout_name: String,
    },
    TaskOpenCreateGlobalMode,
    TaskOpenCreateLayoutMode {
        layout_name: String,
    },
    TaskOpenSaveCurrentCommandGlobalMode,
    TaskOpenSaveCurrentCommandLayoutMode {
        layout_name: String,
    },
    TaskCreate {
        task_name: String,
        command: String,
        layout_name: Option<String>,
    },
    Task {
        task_name: String,
        command: String,
        working_dir: Option<String>,
        layout_name: Option<String>,
    },
    AppInfoEntry {
        label: &'static str,
        value: String,
    },
    AppInfoCopyAll {
        payload: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CommandPaletteItem {
    pub(super) title: String,
    pub(super) keywords: String,
    pub(super) enabled: bool,
    pub(super) status_hint: Option<String>,
    pub(super) tmux_status_hint: Option<TmuxSessionStatusHint>,
    pub(super) kind: CommandPaletteItemKind,
}

impl CommandPaletteItem {
    pub(super) fn command_with_state(
        title: &str,
        keywords: &str,
        action: CommandAction,
        enabled: bool,
        status_hint: Option<&'static str>,
    ) -> Self {
        Self {
            title: title.to_string(),
            keywords: keywords.to_string(),
            enabled,
            status_hint: status_hint.map(str::to_string),
            tmux_status_hint: None,
            kind: CommandPaletteItemKind::Command(action),
        }
    }

    pub(super) fn app_info_entry(label: &'static str, value: String) -> Self {
        let truncated = if value.chars().count() > 60 {
            format!("{}…", value.chars().take(59).collect::<String>())
        } else {
            value.clone()
        };
        let keywords = format!("info {label} {value}").to_ascii_lowercase();
        Self {
            title: format!("{label}: {truncated}"),
            keywords,
            enabled: true,
            status_hint: Some("Copy".to_string()),
            tmux_status_hint: None,
            kind: CommandPaletteItemKind::AppInfoEntry { label, value },
        }
    }

    pub(super) fn app_info_copy_all(payload: String) -> Self {
        Self {
            title: "Copy all to clipboard".to_string(),
            keywords: "copy all info clipboard".to_string(),
            enabled: true,
            status_hint: Some("Copy".to_string()),
            tmux_status_hint: None,
            kind: CommandPaletteItemKind::AppInfoCopyAll { payload },
        }
    }

    pub(super) fn theme(theme_id: String, is_active: bool) -> Self {
        let title = if is_active {
            format!("\u{2713} {theme_id}")
        } else {
            theme_id.clone()
        };
        let keywords = format!("theme palette colors {}", theme_id.replace('-', " "));

        Self {
            title,
            keywords,
            enabled: true,
            status_hint: None,
            tmux_status_hint: None,
            kind: CommandPaletteItemKind::Theme(theme_id),
        }
    }

    pub(super) fn task(
        task_name: &str,
        command: &str,
        working_dir: Option<&str>,
        layout_name: Option<&str>,
    ) -> Self {
        let mut keywords = format!(
            "task run command {} {}",
            task_name.replace(['-', '_'], " "),
            command
        );
        if let Some(layout_name) = layout_name {
            keywords.push(' ');
            keywords.push_str(&layout_name.replace(['-', '_'], " "));
        }
        if let Some(working_dir) = working_dir {
            keywords.push(' ');
            keywords.push_str(working_dir);
        }

        let title = match layout_name {
            Some(layout_name) => format!("{task_name} [{layout_name}]"),
            None => task_name.to_string(),
        };

        Self {
            title,
            keywords,
            enabled: true,
            status_hint: None,
            tmux_status_hint: None,
            kind: CommandPaletteItemKind::Task {
                task_name: task_name.to_string(),
                command: command.to_string(),
                working_dir: working_dir.map(ToOwned::to_owned),
                layout_name: layout_name.map(ToOwned::to_owned),
            },
        }
    }
}

#[derive(Clone, Debug)]
pub(in super::super) struct CommandPaletteState {
    open: bool,
    mode: CommandPaletteMode,
    pub(super) command_intent: CommandPaletteCommandIntent,
    pub(super) tmux_session_intent: TmuxSessionIntent,
    pub(super) saved_layout_intent: SavedLayoutIntent,
    pub(super) task_intent: TaskIntent,
    pub(super) tmux_rename_source_session: Option<String>,
    pub(super) tmux_rename_source_socket: Option<TmuxSocketTarget>,
    pub(super) saved_layout_rename_source: Option<String>,
    input: InlineInputState,
    items: Vec<CommandPaletteItem>,
    filtered: Vec<CommandPaletteMatch>,
    recents: CommandPaletteRecents,
    selected_filtered_index: usize,
    /// Rows the list can show at once, recomputed from the window size.
    visible_rows: usize,
    /// Last pointer position seen over the list. Hover only takes over the
    /// selection once the pointer actually moves, so a list scrolling under a
    /// resting cursor cannot yank the keyboard selection back.
    pointer_position: Option<Point<Pixels>>,
    hover_locked: bool,
    scroll_handle: UniformListScrollHandle,
    scroll_target_y: Option<f32>,
    scroll_max_y: f32,
    scroll_animating: bool,
    scroll_last_tick: Option<Instant>,
    show_keybinds: bool,
    shortcut_cache: HashMap<CommandAction, Option<String>>,
    pub(super) tmux_session_rows: Vec<TmuxSessionRow>,
    pub(super) tmux_create_socket_target: TmuxSocketTarget,
    pub(super) saved_layout_names: Vec<String>,
    pub(super) saved_layout_live_name: Option<String>,
    pub(super) saved_layout_autosave_enabled: bool,
    pub(super) plugin_input_session: Option<PluginInputSession>,
}

impl CommandPaletteState {
    pub(in super::super) fn new(show_keybinds: bool) -> Self {
        Self {
            open: false,
            mode: CommandPaletteMode::Commands,
            command_intent: CommandPaletteCommandIntent::Browse,
            tmux_session_intent: TmuxSessionIntent::AttachOrSwitch,
            saved_layout_intent: SavedLayoutIntent::Browse,
            task_intent: TaskIntent::Browse,
            tmux_rename_source_session: None,
            tmux_rename_source_socket: None,
            saved_layout_rename_source: None,
            input: InlineInputState::new(String::new()),
            items: Vec::new(),
            filtered: Vec::new(),
            recents: CommandPaletteRecents::default(),
            selected_filtered_index: 0,
            visible_rows: COMMAND_PALETTE_MAX_ITEMS,
            pointer_position: None,
            hover_locked: false,
            scroll_handle: UniformListScrollHandle::new(),
            scroll_target_y: None,
            scroll_max_y: 0.0,
            scroll_animating: false,
            scroll_last_tick: None,
            show_keybinds,
            shortcut_cache: HashMap::new(),
            tmux_session_rows: Vec::new(),
            tmux_create_socket_target: TmuxSocketTarget::Default,
            saved_layout_names: Vec::new(),
            saved_layout_live_name: None,
            saved_layout_autosave_enabled: false,
            plugin_input_session: None,
        }
    }

    pub(super) fn is_open(&self) -> bool {
        self.open
    }

    pub(super) fn mode(&self) -> CommandPaletteMode {
        self.mode
    }

    pub(super) fn open(&mut self, mode: CommandPaletteMode) {
        self.open = true;
        self.set_mode(mode);
    }

    pub(super) fn close(&mut self) {
        self.open = false;
        self.mode = CommandPaletteMode::Commands;
        self.reset_for_mode();
    }

    pub(super) fn set_mode(&mut self, mode: CommandPaletteMode) {
        self.mode = mode;
        self.reset_for_mode();
    }

    pub(super) fn set_show_keybinds(&mut self, show_keybinds: bool) {
        self.show_keybinds = show_keybinds;
    }

    pub(in super::super) fn show_keybinds(&self) -> bool {
        self.show_keybinds
    }

    pub(super) fn input(&self) -> &InlineInputState {
        &self.input
    }

    pub(super) fn input_mut(&mut self) -> &mut InlineInputState {
        &mut self.input
    }

    pub(super) fn set_items(&mut self, items: Vec<CommandPaletteItem>) {
        self.items = items;
        self.refilter_current_query();
    }

    pub(super) fn set_items_unfiltered(&mut self, items: Vec<CommandPaletteItem>) {
        self.filtered = (0..items.len())
            .map(|item_index| CommandPaletteMatch {
                item_index,
                ..CommandPaletteMatch::default()
            })
            .collect();
        self.items = items;
        self.clamp_selection();
    }

    /// Re-reads the persisted most-recently-used commands so a palette opened
    /// in one window reflects commands run in another.
    pub(in super::super) fn reload_recents(&mut self) {
        self.recents = CommandPaletteRecents::load();
    }

    pub(super) fn record_recent_item(&mut self, item: &CommandPaletteItem) {
        let Some(key) = super::recents::recent_key_for_item(item) else {
            return;
        };
        self.recents.record(key);
    }

    pub(super) fn reset_for_next_plugin_input(&mut self) {
        self.selected_filtered_index = 0;
        self.scroll_handle = UniformListScrollHandle::new();
        self.reset_scroll_animation_state();
    }

    pub(super) fn begin_plugin_inputs(&mut self, session: PluginInputSession) {
        self.open = true;
        self.plugin_input_session = None;
        self.mode = CommandPaletteMode::PluginInputs;
        self.reset_for_mode();
        self.plugin_input_session = Some(session);
    }

    pub(super) fn command_intent(&self) -> CommandPaletteCommandIntent {
        self.command_intent
    }

    pub(super) fn task_intent(&self) -> TaskIntent {
        self.task_intent
    }

    pub(super) fn set_task_intent(&mut self, intent: TaskIntent) {
        self.task_intent = intent;
    }

    pub(super) fn cached_shortcut(&self, action: CommandAction) -> Option<Option<String>> {
        self.shortcut_cache.get(&action).cloned()
    }

    pub(super) fn cache_shortcut(&mut self, action: CommandAction, shortcut: Option<String>) {
        self.shortcut_cache.insert(action, shortcut);
    }

    pub(super) fn clear_shortcut_cache(&mut self) {
        self.shortcut_cache.clear();
    }

    pub(super) fn refilter_current_query(&mut self) {
        // Typing is keyboard intent: the row under a resting cursor must not
        // steal the selection when the list re-orders beneath it.
        self.lock_hover();
        self.filtered = rank_command_palette_items(
            &self.items,
            self.input.text(),
            &self.recents,
            self.mode.ranking(),
        );
        self.clamp_selection();
    }

    pub(super) fn filtered_len(&self) -> usize {
        self.filtered.len()
    }

    pub(super) fn filtered_item(&self, filtered_index: usize) -> Option<&CommandPaletteItem> {
        let matched = self.filtered.get(filtered_index)?;
        self.items.get(matched.item_index)
    }

    /// Title byte ranges that matched the current query, for highlighting.
    pub(super) fn filtered_title_highlights(&self, filtered_index: usize) -> Vec<Range<usize>> {
        self.filtered
            .get(filtered_index)
            .map(|matched| matched.title_highlights.clone())
            .unwrap_or_default()
    }

    pub(super) fn selected_filtered_index(&self) -> Option<usize> {
        let len = self.filtered_len();
        if len == 0 {
            None
        } else {
            Some(self.selected_filtered_index.min(len - 1))
        }
    }

    pub(super) fn set_selected_filtered_index(&mut self, index: usize) -> bool {
        let len = self.filtered_len();
        if len == 0 {
            self.selected_filtered_index = 0;
            return false;
        }

        let clamped = index.min(len - 1);
        let changed = self.selected_filtered_index != clamped;
        self.selected_filtered_index = clamped;
        changed
    }

    pub(super) fn move_selection_up(&mut self) -> bool {
        let Some(selected) = self.selected_filtered_index() else {
            return false;
        };
        self.lock_hover();
        if selected == 0 {
            return false;
        }
        self.set_selected_filtered_index(selected - 1)
    }

    pub(super) fn move_selection_down(&mut self) -> bool {
        let Some(selected) = self.selected_filtered_index() else {
            return false;
        };
        self.lock_hover();
        let len = self.filtered_len();
        if selected + 1 >= len {
            return false;
        }
        self.set_selected_filtered_index(selected + 1)
    }

    /// Moves by one screenful, stopping at the ends of the list.
    pub(super) fn move_selection_page(&mut self, direction: CommandPaletteScrollDirection) -> bool {
        let Some(selected) = self.selected_filtered_index() else {
            return false;
        };
        self.lock_hover();
        let page = self.visible_rows.max(1);
        let target = match direction {
            CommandPaletteScrollDirection::Up => selected.saturating_sub(page),
            CommandPaletteScrollDirection::Down => selected
                .saturating_add(page)
                .min(self.filtered_len().saturating_sub(1)),
        };
        self.set_selected_filtered_index(target)
    }

    pub(super) fn move_selection_to_edge(
        &mut self,
        direction: CommandPaletteScrollDirection,
    ) -> bool {
        if self.selected_filtered_index().is_none() {
            return false;
        }
        self.lock_hover();
        let target = match direction {
            CommandPaletteScrollDirection::Up => 0,
            CommandPaletteScrollDirection::Down => self.filtered_len().saturating_sub(1),
        };
        self.set_selected_filtered_index(target)
    }

    pub(super) fn visible_rows(&self) -> usize {
        self.visible_rows
    }

    pub(super) fn set_visible_rows(&mut self, visible_rows: usize) {
        self.visible_rows = visible_rows.max(1);
    }

    /// Suspends hover-to-select until the pointer physically moves again.
    fn lock_hover(&mut self) {
        self.hover_locked = true;
    }

    /// Records a pointer sample over the list and reports whether hover should
    /// drive the selection. Returns `false` for events that repeat the last
    /// position, which is what the platform emits when the list scrolls beneath
    /// a resting cursor.
    pub(super) fn accept_hover_at(&mut self, position: Point<Pixels>) -> bool {
        let moved = self.pointer_position != Some(position);
        self.pointer_position = Some(position);
        if moved {
            self.hover_locked = false;
        }
        !self.hover_locked
    }

    pub(super) fn base_scroll_handle(&self) -> gpui::ScrollHandle {
        self.scroll_handle.0.borrow().base_handle.clone()
    }

    pub(super) fn scroll_handle(&self) -> &UniformListScrollHandle {
        &self.scroll_handle
    }

    pub(super) fn scroll_target_y(&self) -> Option<f32> {
        self.scroll_target_y
    }

    pub(super) fn set_scroll_target_y(&mut self, target: f32) {
        self.scroll_target_y = Some(target);
    }

    pub(super) fn clear_scroll_target_y(&mut self) {
        self.scroll_target_y = None;
    }

    pub(super) fn scroll_max_y(&self) -> f32 {
        self.scroll_max_y
    }

    pub(super) fn set_scroll_max_y_for_count(&mut self, item_count: usize) {
        self.scroll_max_y = command_palette_max_scroll_for_count(item_count, self.visible_rows);
    }

    pub(super) fn is_scroll_animating(&self) -> bool {
        self.scroll_animating
    }

    pub(super) fn start_scroll_animation(&mut self, now: Instant) {
        self.scroll_animating = true;
        self.scroll_last_tick = Some(now);
    }

    pub(super) fn stop_scroll_animation(&mut self) {
        self.scroll_animating = false;
        self.scroll_last_tick = None;
    }

    pub(super) fn scroll_dt_seconds(&mut self, now: Instant) -> f32 {
        let dt = self
            .scroll_last_tick
            .map_or(1.0 / 60.0, |last| (now - last).as_secs_f32());
        self.scroll_last_tick = Some(now);
        dt
    }

    pub(super) fn reset_scroll_animation_state(&mut self) {
        self.clear_scroll_target_y();
        self.scroll_max_y = 0.0;
        self.stop_scroll_animation();
    }

    pub(super) fn clamp_selection(&mut self) {
        let len = self.filtered_len();
        if len == 0 {
            self.selected_filtered_index = 0;
        } else if self.selected_filtered_index >= len {
            self.selected_filtered_index = len - 1;
        }
    }

    fn reset_for_mode(&mut self) {
        self.input.clear();
        self.items.clear();
        self.filtered.clear();
        // A pointer sample from the previous mode must not decide whether the
        // first hover in this one counts as movement.
        self.pointer_position = None;
        self.hover_locked = false;
        self.selected_filtered_index = 0;
        self.scroll_handle = UniformListScrollHandle::new();
        self.shortcut_cache.clear();
        self.reset_scroll_animation_state();
        if self.mode != CommandPaletteMode::TmuxSessions {
            self.tmux_session_intent = TmuxSessionIntent::AttachOrSwitch;
            self.tmux_rename_source_session = None;
            self.tmux_rename_source_socket = None;
        }
        if self.mode != CommandPaletteMode::Commands {
            self.command_intent = CommandPaletteCommandIntent::Browse;
        }
        if self.mode != CommandPaletteMode::Layouts {
            self.saved_layout_intent = SavedLayoutIntent::Browse;
            self.saved_layout_rename_source = None;
        }
        if self.mode != CommandPaletteMode::Tasks {
            self.task_intent = TaskIntent::Browse;
        }
        if self.mode != CommandPaletteMode::PluginInputs {
            self.plugin_input_session = None;
        }
    }
}

pub(super) fn ordered_theme_ids_for_palette(
    mut theme_ids: Vec<String>,
    current_theme: &str,
) -> Vec<String> {
    if !theme_ids.iter().any(|theme| theme == SHELL_DECIDE_THEME_ID) {
        theme_ids.push(SHELL_DECIDE_THEME_ID.to_string());
    }

    if !theme_ids.iter().any(|theme| theme == current_theme) {
        theme_ids.push(current_theme.to_string());
    }

    theme_ids.sort_unstable();
    theme_ids.dedup();

    if let Some(current_index) = theme_ids.iter().position(|theme| theme == current_theme) {
        let current = theme_ids.remove(current_index);
        theme_ids.insert(0, current);
    }

    theme_ids
}

/// A row that survived filtering, together with the title ranges that matched
/// the query so the renderer can highlight them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct CommandPaletteMatch {
    pub(super) item_index: usize,
    /// Byte ranges into the item title, ascending and disjoint.
    pub(super) title_highlights: Vec<Range<usize>>,
    score: i32,
}

/// Whether a mode wants rows reordered by match quality, or presented in the
/// order its item builder produced (session lists, plugin option lists, and
/// other curated lists rely on their own ordering).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CommandPaletteRanking {
    ByScore,
    PreserveOrder,
}

/// Filters `items` by `query` and, for score-ranked modes, orders them by
/// relevance with recently executed commands boosted.
pub(super) fn rank_command_palette_items(
    items: &[CommandPaletteItem],
    query: &str,
    recents: &CommandPaletteRecents,
    ranking: CommandPaletteRanking,
) -> Vec<CommandPaletteMatch> {
    let Some(query) = FuzzyQuery::new(query) else {
        return unfiltered_matches(items, recents, ranking);
    };

    let title_matches: Vec<Option<FuzzyMatch>> = items
        .iter()
        .map(|item| fuzzy::match_text(&item.title, &query))
        .collect();
    // Keyword hits are a fallback only: when any title matches, keyword-only
    // rows stay hidden so a typed word does not drag in loosely related rows.
    let has_title_matches = title_matches.iter().any(Option::is_some);

    let mut matches: Vec<CommandPaletteMatch> = items
        .iter()
        .enumerate()
        .zip(title_matches)
        .filter_map(|((item_index, item), title_match)| {
            let (score, title_highlights) = match title_match {
                Some(matched) => (matched.score, matched.ranges),
                None if has_title_matches => return None,
                None => (fuzzy::match_text(&item.keywords, &query)?.score, Vec::new()),
            };

            Some(CommandPaletteMatch {
                item_index,
                title_highlights,
                score: score + recents.bonus_for_item(item),
            })
        })
        .collect();

    if ranking == CommandPaletteRanking::ByScore {
        // Unavailable rows sink below every runnable one, whatever they score:
        // they stay reachable (with their reason on the row) without pushing
        // runnable commands out of the first screenful. Stable sort keeps the
        // builder's order for equally scored rows.
        matches.sort_by_key(|matched| {
            (
                !items[matched.item_index].enabled,
                std::cmp::Reverse(matched.score),
            )
        });
    }

    matches
}

/// Empty-query ordering: recently executed rows first (most recent first),
/// everything else in its original order.
fn unfiltered_matches(
    items: &[CommandPaletteItem],
    recents: &CommandPaletteRecents,
    ranking: CommandPaletteRanking,
) -> Vec<CommandPaletteMatch> {
    let mut matches: Vec<CommandPaletteMatch> = (0..items.len())
        .map(|item_index| CommandPaletteMatch {
            item_index,
            title_highlights: Vec::new(),
            score: 0,
        })
        .collect();

    if ranking == CommandPaletteRanking::ByScore {
        matches.sort_by_key(|matched| {
            let item = &items[matched.item_index];
            (
                !item.enabled,
                recents.rank_for_item(item).unwrap_or(usize::MAX),
            )
        });
    }

    matches
}

/// Panel geometry for the window the palette is drawn in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct CommandPaletteLayout {
    pub(super) width: f32,
    pub(super) top_offset: f32,
    pub(super) visible_rows: usize,
}

/// Fits the panel to the space it actually has. The preferred width, top
/// offset, and row count are unchanged on a normal window; they only shrink
/// when the panel would otherwise overflow.
pub(super) fn command_palette_layout_for_viewport(
    viewport_width: f32,
    viewport_height: f32,
) -> CommandPaletteLayout {
    let width = (viewport_width - COMMAND_PALETTE_VIEWPORT_MARGIN_X * 2.0)
        .clamp(COMMAND_PALETTE_MIN_WIDTH, COMMAND_PALETTE_WIDTH)
        .min(viewport_width.max(1.0));

    let top_offset = COMMAND_PALETTE_TOP_OFFSET
        .min(viewport_height * COMMAND_PALETTE_TOP_OFFSET_RATIO)
        .max(COMMAND_PALETTE_MIN_TOP_OFFSET);

    // Everything above and below the list: the offset from the window top, the
    // input head, both dividers, the list padding, the footer, and a margin so
    // the panel never touches the bottom edge.
    let chrome_height = top_offset
        + COMMAND_PALETTE_INPUT_HEAD_HEIGHT
        + COMMAND_PALETTE_FOOTER_HEIGHT
        + COMMAND_PALETTE_LIST_PADDING_Y * 2.0
        + 2.0
        + COMMAND_PALETTE_VIEWPORT_MARGIN_Y;
    let rows_that_fit = ((viewport_height - chrome_height) / COMMAND_PALETTE_ROW_HEIGHT).floor();
    let visible_rows = if rows_that_fit.is_finite() && rows_that_fit > 0.0 {
        (rows_that_fit as usize).clamp(COMMAND_PALETTE_MIN_ITEMS, COMMAND_PALETTE_MAX_ITEMS)
    } else {
        COMMAND_PALETTE_MIN_ITEMS
    };

    CommandPaletteLayout {
        width,
        top_offset,
        visible_rows,
    }
}

pub(super) fn command_palette_viewport_height(visible_rows: usize) -> f32 {
    visible_rows as f32 * COMMAND_PALETTE_ROW_HEIGHT
}

pub(super) fn command_palette_max_scroll_for_count(item_count: usize, visible_rows: usize) -> f32 {
    (item_count as f32 * COMMAND_PALETTE_ROW_HEIGHT - command_palette_viewport_height(visible_rows))
        .max(0.0)
}

pub(super) fn command_palette_target_scroll_y(
    current_y: f32,
    selected_index: usize,
    item_count: usize,
    visible_rows: usize,
) -> Option<f32> {
    if item_count == 0 {
        return None;
    }

    let viewport_height = command_palette_viewport_height(visible_rows);
    let max_scroll = command_palette_max_scroll_for_count(item_count, visible_rows);
    let row_top = selected_index as f32 * COMMAND_PALETTE_ROW_HEIGHT;
    let row_bottom = row_top + COMMAND_PALETTE_ROW_HEIGHT;

    let target = if row_top < current_y {
        row_top
    } else if row_bottom > current_y + viewport_height {
        row_bottom - viewport_height
    } else {
        current_y
    };

    Some(target.clamp(0.0, max_scroll))
}

pub(super) fn command_palette_next_scroll_y(
    current_y: f32,
    target_y: f32,
    max_scroll: f32,
    dt_seconds: f32,
) -> f32 {
    let target_y = target_y.clamp(0.0, max_scroll);
    let delta = target_y - current_y;
    if delta.abs() <= 0.5 {
        return target_y;
    }

    let dt = dt_seconds.clamp(1.0 / 240.0, 0.05);
    let smoothing = 1.0 - (-18.0 * dt).exp();
    let desired_step = delta * smoothing;
    let max_step = 1800.0 * dt;
    let step = desired_step.clamp(-max_step, max_step);
    let next_y = (current_y + step).clamp(0.0, max_scroll);

    if (target_y - next_y).abs() <= 0.5 {
        target_y
    } else {
        next_y
    }
}

#[cfg(test)]
fn command_exists_in_path(command: &str, path_env: &str) -> bool {
    if command.is_empty() {
        return false;
    }

    std::env::split_paths(path_env)
        .filter(|entry| !entry.as_os_str().is_empty())
        .any(|entry| command_exists_in_dir(&entry, command))
}

#[cfg(test)]
fn command_exists_in_dir(dir: &std::path::Path, command: &str) -> bool {
    let candidate = dir.join(command);
    candidate_is_executable(&candidate)
}

#[cfg(test)]
fn candidate_is_executable(candidate: &std::path::Path) -> bool {
    let Ok(metadata) = std::fs::metadata(candidate) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        metadata.permissions().mode() & 0o111 != 0
    }

    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
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
    fn unavailable_rows_sink_below_runnable_ones() {
        let items = vec![
            CommandPaletteItem::command_with_state(
                "New Browser Tab",
                "tab",
                CommandAction::NewBrowserTab,
                false,
                Some("Disabled"),
            ),
            command_item("Rename Tab", "tab", CommandAction::RenameTab),
            command_item("New Tab", "tab", CommandAction::NewTab),
        ];

        // "New Browser Tab" is the strongest match for "new" but is disabled,
        // so it still lands last.
        assert_eq!(
            ranked_actions(&items, "new"),
            vec![CommandAction::NewTab, CommandAction::NewBrowserTab]
        );
        assert_eq!(
            ranked_actions(&items, "tab").last(),
            Some(&CommandAction::NewBrowserTab)
        );
        assert_eq!(
            ranked_actions(&items, "").last(),
            Some(&CommandAction::NewBrowserTab)
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
                .map(|index| {
                    command_item(&format!("Command {index}"), "cmd", CommandAction::NewTab)
                })
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
}
