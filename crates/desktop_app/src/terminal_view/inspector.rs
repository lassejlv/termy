//! Ghostty-style developer inspector: a bottom pane (toggled with the
//! `toggle_inspector` command, default `secondary-alt-i`) showing live
//! terminal state, a keyboard event log, and render statistics.

use super::*;
use std::collections::VecDeque;
use termy_core::terminal_ui_render_metrics_snapshot;

const INSPECTOR_DEFAULT_HEIGHT: f32 = 280.0;
const INSPECTOR_MIN_HEIGHT: f32 = 140.0;
const INSPECTOR_MAX_HEIGHT: f32 = 640.0;
// The terminal always keeps at least this share of the viewport height.
const INSPECTOR_MAX_VIEWPORT_RATIO: f32 = 0.7;
const INSPECTOR_RESIZE_HANDLE_HEIGHT: f32 = 6.0;
const INSPECTOR_TAB_BAR_HEIGHT: f32 = 34.0;
const INSPECTOR_KEY_LOG_CAPACITY: usize = 200;
const INSPECTOR_TEXT_SIZE: f32 = 11.5;
const INSPECTOR_ROW_HEIGHT: f32 = 22.0;
const INSPECTOR_LABEL_WIDTH: f32 = 250.0;
const INSPECTOR_TAB_RADIUS: f32 = 5.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InspectorTab {
    Terminal,
    Runtime,
    Input,
    Config,
    Keyboard,
    Render,
}

impl InspectorTab {
    const ALL: [Self; 6] = [
        Self::Terminal,
        Self::Runtime,
        Self::Input,
        Self::Config,
        Self::Keyboard,
        Self::Render,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Terminal => "Terminal",
            Self::Runtime => "Runtime",
            Self::Input => "Input",
            Self::Config => "Config",
            Self::Keyboard => "Keyboard",
            Self::Render => "Render",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_mode_label_summarizes_enabled_features() {
        let label = mouse_mode_label(TerminalMouseMode {
            enabled: true,
            report_click: true,
            report_drag: true,
            report_motion: false,
            sgr_encoding: true,
            utf8_encoding: false,
        });

        assert_eq!(label, "click, drag, sgr");
        assert_eq!(mouse_mode_label(TerminalMouseMode::default()), "disabled");
    }

    #[test]
    fn cursor_state_label_reports_hidden_or_position() {
        assert_eq!(cursor_state_label(None), "hidden");
        assert_eq!(
            cursor_state_label(Some(TerminalCursorState {
                col: 4,
                row: 2,
                style: TerminalCursorStyle::Line,
            })),
            "row 2 col 4 Line"
        );
    }

    #[test]
    fn mouse_target_label_reports_pane_and_cell() {
        let target = MouseReportTargetCell {
            pane_id: "%1".to_string(),
            col: 9,
            row: 3,
        };

        assert_eq!(mouse_target_label(Some(&target)), "%1 row 3 col 9");
        assert_eq!(mouse_target_label(None), "-");
    }
}

pub(super) struct InspectorKeyLogEntry {
    seq: u64,
    key: String,
    modifiers: String,
    key_char: Option<String>,
    route: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct InspectorResizeDrag {
    start_window_y: f32,
    start_height: f32,
}

pub(super) struct InspectorState {
    pub(super) open: bool,
    pub(super) tab: InspectorTab,
    height: f32,
    resize_drag: Option<InspectorResizeDrag>,
    key_log: VecDeque<InspectorKeyLogEntry>,
    next_seq: u64,
}

impl InspectorState {
    pub(super) fn new(configured_height: f32) -> Self {
        Self {
            open: false,
            tab: InspectorTab::Terminal,
            height: Self::clamp_height(configured_height),
            resize_drag: None,
            key_log: VecDeque::new(),
            next_seq: 1,
        }
    }

    fn clamp_height(height: f32) -> f32 {
        if height.is_finite() {
            height.clamp(INSPECTOR_MIN_HEIGHT, INSPECTOR_MAX_HEIGHT)
        } else {
            INSPECTOR_DEFAULT_HEIGHT
        }
    }
}

fn keystroke_modifiers_label(modifiers: gpui::Modifiers) -> String {
    let mut label = String::new();
    if modifiers.control {
        label.push('⌃');
    }
    if modifiers.alt {
        label.push('⌥');
    }
    if modifiers.shift {
        label.push('⇧');
    }
    if modifiers.platform {
        label.push('⌘');
    }
    if modifiers.function {
        label.push_str("fn");
    }
    if label.is_empty() {
        label.push('-');
    }
    label
}

struct InspectorPaneSnapshot {
    id: String,
    is_active: bool,
    cols: u16,
    rows: u16,
    cell_geometry: (u16, u16, u16, u16),
    display_offset: usize,
    history_size: usize,
    alternate_screen: bool,
    bracketed_paste: bool,
    mouse_mode: TerminalMouseMode,
    keyboard_mode: TerminalKeyboardMode,
    cursor_state: Option<TerminalCursorState>,
    child_pid: Option<u32>,
    zoom_steps: i16,
    degraded: bool,
    progress: ProgressState,
}

struct InspectorTerminalState {
    size: TerminalSize,
    display_offset: usize,
    history_size: usize,
    alternate_screen: bool,
    bracketed_paste: bool,
    mouse_mode: TerminalMouseMode,
    keyboard_mode: TerminalKeyboardMode,
    cursor_state: Option<TerminalCursorState>,
    child_pid: Option<u32>,
}

impl Terminal {
    fn inspector_state_snapshot(&self) -> InspectorTerminalState {
        let size = self.size();
        let (display_offset, history_size) = self.scroll_state();
        InspectorTerminalState {
            size,
            display_offset,
            history_size,
            alternate_screen: self.alternate_screen_mode(),
            bracketed_paste: self.bracketed_paste_mode(),
            mouse_mode: self.mouse_mode(),
            keyboard_mode: self.keyboard_mode(),
            cursor_state: self.cursor_state(),
            child_pid: self.child_pid(),
        }
    }
}

struct InspectorActiveTabSnapshot {
    title: String,
    window_id: String,
    pane_count: usize,
    progress: ProgressState,
    running_process: bool,
    pinned: bool,
    manual_title: Option<String>,
    explicit_title: Option<String>,
    shell_title: Option<String>,
    current_command: Option<String>,
    last_prompt_cwd: Option<String>,
    active_pane_id: Option<String>,
}

fn option_label(value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("-")
        .to_string()
}

fn bool_label(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn selection_pos_label(position: SelectionPos) -> String {
    format!("line {} col {}", position.line, position.col)
}

fn cell_pos_label(position: CellPos) -> String {
    format!("row {} col {}", position.row, position.col)
}

fn mouse_target_label(target: Option<&MouseReportTargetCell>) -> String {
    target.map_or_else(
        || "-".to_string(),
        |target| format!("{} row {} col {}", target.pane_id, target.row, target.col),
    )
}

fn mouse_mode_label(mode: TerminalMouseMode) -> String {
    if !mode.enabled {
        return "disabled".to_string();
    }
    let mut parts = Vec::new();
    if mode.report_click {
        parts.push("click");
    }
    if mode.report_drag {
        parts.push("drag");
    }
    if mode.report_motion {
        parts.push("motion");
    }
    if mode.sgr_encoding {
        parts.push("sgr");
    }
    if mode.utf8_encoding {
        parts.push("utf8");
    }
    if parts.is_empty() {
        "enabled".to_string()
    } else {
        parts.join(", ")
    }
}

fn cursor_state_label(cursor: Option<TerminalCursorState>) -> String {
    cursor.map_or_else(
        || "hidden".to_string(),
        |cursor| format!("row {} col {} {:?}", cursor.row, cursor.col, cursor.style),
    )
}

impl TerminalView {
    pub(super) fn toggle_inspector(&mut self, cx: &mut Context<Self>) {
        self.inspector.open = !self.inspector.open;
        cx.notify();
    }

    /// Height reserved at the bottom of the terminal area while the inspector
    /// is open; feeds PTY grid sizing and pane content bounds.
    pub(super) fn inspector_bottom_inset(&self) -> f32 {
        if !self.inspector.open {
            return 0.0;
        }
        let viewport_cap = self
            .last_viewport_size_px
            .map_or(INSPECTOR_MAX_HEIGHT, |(_, viewport_height)| {
                viewport_height as f32 * INSPECTOR_MAX_VIEWPORT_RATIO
            });
        self.inspector
            .height
            .min(viewport_cap)
            .max(INSPECTOR_MIN_HEIGHT)
    }

    pub(super) fn inspector_resize_drag_active(&self) -> bool {
        self.inspector.open && self.inspector.resize_drag.is_some()
    }

    pub(super) fn update_inspector_resize_drag(&mut self, window_y: f32) -> bool {
        let Some(drag) = self.inspector.resize_drag else {
            return false;
        };
        // Dragging the handle upward grows the panel.
        let next_height =
            InspectorState::clamp_height(drag.start_height + (drag.start_window_y - window_y));
        if (next_height - self.inspector.height).abs() < f32::EPSILON {
            return false;
        }
        self.inspector.height = next_height;
        true
    }

    pub(super) fn finish_inspector_resize_drag(&mut self) -> bool {
        let Some(drag) = self.inspector.resize_drag.take() else {
            return false;
        };
        // Remember the final height across restarts. Skip the write when the
        // drag ended where it started.
        if (self.inspector.height - drag.start_height).abs() >= 1.0
            && let Err(error) = crate::config::set_root_setting(
                termy_core::config_core::RootSettingId::InspectorHeight,
                &format!("{:.0}", self.inspector.height),
            )
        {
            log::error!("Failed to persist inspector height: {error}");
        }
        true
    }

    /// Adopt an externally edited config value unless a resize drag owns the
    /// height right now.
    pub(super) fn sync_inspector_height_from_config(&mut self, configured_height: f32) -> bool {
        if self.inspector.resize_drag.is_some() {
            return false;
        }
        let next = InspectorState::clamp_height(configured_height);
        if (next - self.inspector.height).abs() < f32::EPSILON {
            return false;
        }
        self.inspector.height = next;
        true
    }

    pub(super) fn inspector_collects_render_stats(&self) -> bool {
        self.inspector.open && self.inspector.tab == InspectorTab::Render
    }

    pub(super) fn record_inspector_key_event(
        &mut self,
        event: &gpui::KeyDownEvent,
        route: &'static str,
        cx: &mut Context<Self>,
    ) {
        if !self.inspector.open {
            return;
        }
        let seq = self.inspector.next_seq;
        self.inspector.next_seq += 1;
        self.inspector.key_log.push_front(InspectorKeyLogEntry {
            seq,
            key: event.keystroke.key.clone(),
            modifiers: keystroke_modifiers_label(event.keystroke.modifiers),
            key_char: event.keystroke.key_char.clone(),
            route,
        });
        self.inspector.key_log.truncate(INSPECTOR_KEY_LOG_CAPACITY);
        if self.inspector.tab == InspectorTab::Keyboard {
            cx.notify();
        }
    }

    fn inspector_pane_snapshots(&self) -> Vec<InspectorPaneSnapshot> {
        let Some(tab) = self.session.tabs.get(self.session.active_tab) else {
            return Vec::new();
        };
        let active_pane_id = tab.active_pane_id.as_str();
        tab.panes
            .iter()
            .map(|pane| {
                let terminal = pane.terminal();
                let state = terminal.inspector_state_snapshot();
                InspectorPaneSnapshot {
                    id: pane.id.clone(),
                    is_active: pane.id == active_pane_id,
                    cols: state.size.cols,
                    rows: state.size.rows,
                    cell_geometry: (pane.left, pane.top, pane.width, pane.height),
                    display_offset: state.display_offset,
                    history_size: state.history_size,
                    alternate_screen: state.alternate_screen,
                    bracketed_paste: state.bracketed_paste,
                    mouse_mode: state.mouse_mode,
                    keyboard_mode: state.keyboard_mode,
                    cursor_state: state.cursor_state,
                    child_pid: state.child_pid,
                    zoom_steps: pane.pane_zoom_steps,
                    degraded: pane.degraded,
                    progress: pane.progress_state,
                }
            })
            .collect()
    }

    fn inspector_active_tab_snapshot(&self) -> Option<InspectorActiveTabSnapshot> {
        self.session
            .tabs
            .get(self.session.active_tab)
            .map(|tab| InspectorActiveTabSnapshot {
                title: tab.title.clone(),
                window_id: tab.window_id.clone(),
                pane_count: tab.panes.len(),
                progress: tab.aggregate_progress_state(),
                running_process: tab.running_process,
                pinned: tab.pinned,
                manual_title: tab.manual_title.clone(),
                explicit_title: tab.explicit_title.clone(),
                shell_title: tab.shell_title.clone(),
                current_command: tab.current_command.clone(),
                last_prompt_cwd: tab.last_prompt_cwd.clone(),
                active_pane_id: tab.active_pane_id().map(ToOwned::to_owned),
            })
    }

    fn inspector_row(
        label: &'static str,
        value: String,
        label_color: gpui::Rgba,
        value_color: gpui::Rgba,
    ) -> AnyElement {
        div()
            .flex_none()
            .h(px(INSPECTOR_ROW_HEIGHT))
            .flex()
            .items_center()
            .gap(px(12.0))
            .child(
                div()
                    .flex_none()
                    .w(px(INSPECTOR_LABEL_WIDTH))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .truncate()
                    .text_color(label_color)
                    .child(label),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .truncate()
                    .text_color(value_color)
                    .child(value),
            )
            .into_any_element()
    }

    fn render_inspector_terminal_tab(
        &self,
        text_primary: gpui::Rgba,
        text_muted: gpui::Rgba,
        accent: gpui::Rgba,
    ) -> AnyElement {
        let runtime_label = match self.runtime_kind() {
            RuntimeKind::Native => "native",
            RuntimeKind::Tmux => "tmux",
        };
        let engine_label = terminal_engine_label(self.active_terminal());
        let tab_summary = self.inspector_active_tab_snapshot();
        let panes = self.inspector_pane_snapshots();
        let font_size: f32 = self.font_size.into();

        let mut content = div().flex().flex_col();
        content = content.child(Self::inspector_row(
            "Runtime",
            runtime_label.to_string(),
            text_muted,
            text_primary,
        ));
        content = content.child(Self::inspector_row(
            "Engine",
            engine_label.to_string(),
            text_muted,
            text_primary,
        ));
        content = content.child(Self::inspector_row(
            "Font Size",
            format!("{font_size:.1}px"),
            text_muted,
            text_primary,
        ));
        content = content.child(Self::inspector_row(
            "Shell Integration",
            if self.shell_integration_enabled {
                "enabled".to_string()
            } else {
                "disabled".to_string()
            },
            text_muted,
            text_primary,
        ));
        if let Some(tab) = tab_summary {
            content = content.child(Self::inspector_row(
                "Active Tab",
                format!(
                    "{}  ({}, {} pane(s))",
                    tab.title, tab.window_id, tab.pane_count
                ),
                text_muted,
                text_primary,
            ));
            content = content.child(Self::inspector_row(
                "Tab Progress",
                format!("{:?}", tab.progress),
                text_muted,
                text_primary,
            ));
            content = content.child(Self::inspector_row(
                "Tab State",
                format!(
                    "running {} / pinned {} / active pane {}",
                    bool_label(tab.running_process),
                    bool_label(tab.pinned),
                    option_label(tab.active_pane_id.as_deref())
                ),
                text_muted,
                text_primary,
            ));
            content = content.child(Self::inspector_row(
                "Current Command",
                option_label(tab.current_command.as_deref()),
                text_muted,
                text_primary,
            ));
            content = content.child(Self::inspector_row(
                "Manual Title",
                option_label(tab.manual_title.as_deref()),
                text_muted,
                text_primary,
            ));
            content = content.child(Self::inspector_row(
                "Explicit Title",
                option_label(tab.explicit_title.as_deref()),
                text_muted,
                text_primary,
            ));
            content = content.child(Self::inspector_row(
                "Shell Title",
                option_label(tab.shell_title.as_deref()),
                text_muted,
                text_primary,
            ));
            content = content.child(Self::inspector_row(
                "Prompt CWD",
                option_label(tab.last_prompt_cwd.as_deref()),
                text_muted,
                text_primary,
            ));
        }

        for pane in panes {
            let header_color = if pane.is_active { accent } else { text_muted };
            let header = format!(
                "Pane {}{}",
                pane.id,
                if pane.is_active { "  (active)" } else { "" }
            );
            content = content.child(
                div()
                    .flex_none()
                    .h(px(INSPECTOR_ROW_HEIGHT))
                    .mt(px(8.0))
                    .flex()
                    .items_center()
                    .text_color(header_color)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(header),
            );
            let (left, top, width, height) = pane.cell_geometry;
            content = content
                .child(Self::inspector_row(
                    "Grid",
                    format!("{} × {} cells", pane.cols, pane.rows),
                    text_muted,
                    text_primary,
                ))
                .child(Self::inspector_row(
                    "Cell Rect",
                    format!("x {left}  y {top}  w {width}  h {height}"),
                    text_muted,
                    text_primary,
                ))
                .child(Self::inspector_row(
                    "Scroll",
                    format!(
                        "offset {} / history {}",
                        pane.display_offset, pane.history_size
                    ),
                    text_muted,
                    text_primary,
                ))
                .child(Self::inspector_row(
                    "Screen",
                    format!(
                        "{} / bracketed paste {}",
                        if pane.alternate_screen {
                            "alternate"
                        } else {
                            "primary"
                        },
                        bool_label(pane.bracketed_paste)
                    ),
                    text_muted,
                    text_primary,
                ))
                .child(Self::inspector_row(
                    "Cursor",
                    cursor_state_label(pane.cursor_state),
                    text_muted,
                    text_primary,
                ))
                .child(Self::inspector_row(
                    "Child PID",
                    pane.child_pid
                        .map_or_else(|| "-".to_string(), |pid| pid.to_string()),
                    text_muted,
                    text_primary,
                ))
                .child(Self::inspector_row(
                    "Mouse Mode",
                    mouse_mode_label(pane.mouse_mode),
                    text_muted,
                    text_primary,
                ))
                .child(Self::inspector_row(
                    "Keyboard Mode",
                    format!("{:?}", pane.keyboard_mode),
                    text_muted,
                    text_primary,
                ))
                .child(Self::inspector_row(
                    "Zoom / Degraded",
                    format!("{} steps / {}", pane.zoom_steps, pane.degraded),
                    text_muted,
                    text_primary,
                ))
                .child(Self::inspector_row(
                    "Progress",
                    format!("{:?}", pane.progress),
                    text_muted,
                    text_primary,
                ));
        }

        content.into_any_element()
    }

    fn render_inspector_runtime_tab(
        &self,
        text_primary: gpui::Rgba,
        text_muted: gpui::Rgba,
    ) -> AnyElement {
        let mut content = div().flex().flex_col();
        content = content
            .child(Self::inspector_row(
                "Runtime Kind",
                format!("{:?}", self.runtime_kind()),
                text_muted,
                text_primary,
            ))
            .child(Self::inspector_row(
                "Config tmux_enabled",
                bool_label(self.tmux_enabled_config).to_string(),
                text_muted,
                text_primary,
            ))
            .child(Self::inspector_row(
                "Configured Working Dir",
                option_label(self.configured_working_dir.as_deref()),
                text_muted,
                text_primary,
            ))
            .child(Self::inspector_row(
                "Config Path",
                self.config_path
                    .as_ref()
                    .map_or_else(|| "-".to_string(), |path| path.display().to_string()),
                text_muted,
                text_primary,
            ))
            .child(Self::inspector_row(
                "Last Config Error",
                option_label(self.last_config_error_message.as_deref()),
                text_muted,
                text_primary,
            ));

        match self.runtime.as_tmux() {
            Some(runtime) => {
                content = content
                    .child(Self::inspector_row(
                        "tmux Session",
                        runtime.client.session_name().to_string(),
                        text_muted,
                        text_primary,
                    ))
                    .child(Self::inspector_row(
                        "tmux Binary",
                        runtime.config.binary.clone(),
                        text_muted,
                        text_primary,
                    ))
                    .child(Self::inspector_row(
                        "tmux Launch",
                        format!("{:?}", runtime.config.launch),
                        text_muted,
                        text_primary,
                    ))
                    .child(Self::inspector_row(
                        "tmux Client Grid",
                        format!("{} × {}", runtime.client_cols, runtime.client_rows),
                        text_muted,
                        text_primary,
                    ))
                    .child(Self::inspector_row(
                        "tmux Preferred CWD",
                        option_label(runtime.preferred_cwd.as_deref()),
                        text_muted,
                        text_primary,
                    ))
                    .child(Self::inspector_row(
                        "tmux Resize Work",
                        format!(
                            "{} / wake scheduled {}",
                            bool_label(runtime.resize_scheduler.has_work()),
                            bool_label(runtime.resize_wakeup_scheduled)
                        ),
                        text_muted,
                        text_primary,
                    ))
                    .child(Self::inspector_row(
                        "tmux Title Refresh",
                        format!(
                            "deadline {} / wake scheduled {}",
                            bool_label(runtime.title_refresh_deadline.is_some()),
                            bool_label(runtime.title_refresh_wakeup_scheduled)
                        ),
                        text_muted,
                        text_primary,
                    ))
                    .child(Self::inspector_row(
                        "tmux Active Border",
                        bool_label(runtime.config.show_active_pane_border).to_string(),
                        text_muted,
                        text_primary,
                    ));
            }
            None => {
                content = content.child(Self::inspector_row(
                    "Native Runtime",
                    "active".to_string(),
                    text_muted,
                    text_primary,
                ));
            }
        }

        content.into_any_element()
    }

    fn render_inspector_input_tab(
        &self,
        text_primary: gpui::Rgba,
        text_muted: gpui::Rgba,
    ) -> AnyElement {
        let active_overlay = if self.is_command_palette_open() {
            "command palette"
        } else if self.release_notes_open() {
            "release notes"
        } else if self.search_open {
            "search"
        } else if self.renaming_tab.is_some() {
            "tab rename"
        } else if self.renaming_workspace.is_some() {
            "workspace rename"
        } else if self.terminal_context_menu.is_some() {
            "terminal context menu"
        } else if self.tab_context_menu.is_some() {
            "tab context menu"
        } else {
            "-"
        };
        let selection = match (self.selection_anchor, self.selection_head) {
            (Some(anchor), Some(head)) => format!(
                "{} -> {} / dragging {} / moved {}",
                selection_pos_label(anchor),
                selection_pos_label(head),
                bool_label(self.selection_dragging),
                bool_label(self.selection_moved)
            ),
            _ => "-".to_string(),
        };
        let hovered_link = self.hovered_link.as_ref().map_or_else(
            || "-".to_string(),
            |link| {
                format!(
                    "rows {}:{}-{}:{} -> {}",
                    link.start_row, link.start_col, link.end_row, link.end_col, link.target
                )
            },
        );
        let hovered_divider = self.hovered_pane_divider.as_ref().map_or_else(
            || "-".to_string(),
            |divider| format!("{} {:?} {:?}", divider.pane_id, divider.axis, divider.edge),
        );
        let pane_resize = self.pane_resize_drag.as_ref().map_or_else(
            || "-".to_string(),
            |drag| {
                format!(
                    "{} {:?} {:?} steps {}",
                    drag.pane_id, drag.axis, drag.edge, drag.applied_steps
                )
            },
        );
        let pending_cursor = self.pending_cursor_move_click.as_ref().map_or_else(
            || "-".to_string(),
            |pending| {
                format!(
                    "{} {} -> {}",
                    pending.pane_id,
                    cell_pos_label(pending.start_cell),
                    cell_pos_label(pending.target)
                )
            },
        );
        let cursor_preview = self.pending_cursor_move_preview.as_ref().map_or_else(
            || "-".to_string(),
            |preview| {
                format!(
                    "{} {} {:?}",
                    preview.pane_id,
                    cell_pos_label(preview.target),
                    preview.style
                )
            },
        );
        let active_mouse_mode = self
            .session
            .tabs
            .get(self.session.active_tab)
            .and_then(TerminalTab::active_terminal)
            .map_or_else(
                || "-".to_string(),
                |terminal| mouse_mode_label(terminal.mouse_mode()),
            );

        let rows: Vec<(&'static str, String)> = vec![
            ("Active Overlay", active_overlay.to_string()),
            ("Selection", selection),
            ("Hovered Link", hovered_link),
            ("Hovered Pane Divider", hovered_divider),
            ("Pane Resize Drag", pane_resize),
            ("Pending Cursor Move", pending_cursor),
            ("Cursor Move Preview", cursor_preview),
            ("Active Mouse Mode", active_mouse_mode),
            (
                "Mouse Press Targets",
                format!(
                    "L {} / M {} / R {}",
                    mouse_target_label(self.mouse_reporting.left_button.as_ref()),
                    mouse_target_label(self.mouse_reporting.middle_button.as_ref()),
                    mouse_target_label(self.mouse_reporting.right_button.as_ref())
                ),
            ),
            (
                "Mouse Hover Target",
                mouse_target_label(self.mouse_reporting.hover_target.as_ref()),
            ),
            (
                "Scroll Accumulators",
                format!(
                    "terminal {:.2} / report x {:.2} y {:.2}",
                    self.terminal_scroll_accumulator_y,
                    self.mouse_reporting.scroll_accumulator_x,
                    self.mouse_reporting.scroll_accumulator_y
                ),
            ),
            (
                "Pending Key Releases",
                format!(
                    "{} terminal / {} ime",
                    self.pending_key_releases.len(),
                    self.deferred_ime_key_releases.len()
                ),
            ),
        ];

        let mut content = div().flex().flex_col();
        for (label, value) in rows {
            content = content.child(Self::inspector_row(label, value, text_muted, text_primary));
        }
        content.into_any_element()
    }

    fn render_inspector_config_tab(
        &self,
        text_primary: gpui::Rgba,
        text_muted: gpui::Rgba,
    ) -> AnyElement {
        let rows: Vec<(&'static str, String)> = vec![
            ("Theme", self.theme_id.clone()),
            ("Theme Mode", format!("{:?}", self.theme_mode)),
            (
                "Manual / Light / Dark Theme",
                format!(
                    "{} / {} / {}",
                    self.manual_theme, self.light_theme, self.dark_theme
                ),
            ),
            ("Font", self.font_family.to_string()),
            ("UI Font", self.ui_font_family.to_string()),
            ("Base Font Size", format!("{:.1}px", self.base_font_size)),
            (
                "Effective Font Size",
                format!("{:.1}px", f32::from(self.font_size)),
            ),
            ("Line Height", format!("{:.2}", self.line_height)),
            (
                "Padding",
                format!("{:.1} × {:.1}", self.padding_x, self.padding_y),
            ),
            ("Cursor Style", format!("{:?}", self.cursor_style)),
            ("Cursor Blink", bool_label(self.cursor_blink).to_string()),
            (
                "Background",
                format!(
                    "opacity {:.2} / blur {} / cells {}",
                    self.background_opacity,
                    bool_label(self.background_blur),
                    bool_label(self.background_opacity_cells)
                ),
            ),
            (
                "Chrome Contrast",
                bool_label(self.chrome_contrast).to_string(),
            ),
            ("Tab Bar Position", format!("{:?}", self.tab_bar_position)),
            (
                "Tab Bar Visibility",
                format!("{:?}", self.tab_bar_visibility),
            ),
            (
                "Sidebar Collapsed",
                bool_label(self.sidebar_collapsed).to_string(),
            ),
            (
                "Auto Hide Tab Bar",
                bool_label(self.auto_hide_tabbar).to_string(),
            ),
            ("Simple Mode", bool_label(self.simple_mode).to_string()),
            (
                "Command Palette Keybinds",
                bool_label(self.command_palette.show_keybinds()).to_string(),
            ),
            (
                "Shell Integration",
                bool_label(self.shell_integration_enabled).to_string(),
            ),
            (
                "Progress Indicator",
                bool_label(self.progress_indicator_enabled).to_string(),
            ),
            (
                "Native Persistence",
                format!(
                    "tabs {} / layouts {} / buffers {}",
                    bool_label(self.native_tab_persistence),
                    bool_label(self.native_layout_autosave),
                    bool_label(self.native_buffer_persistence)
                ),
            ),
            (
                "Quit Warnings",
                format!(
                    "quit {} / running process {}",
                    bool_label(self.warn_on_quit),
                    bool_label(self.warn_on_quit_with_running_process)
                ),
            ),
            ("Tasks", self.tasks.len().to_string()),
            (
                "Current Layout",
                option_label(self.current_named_layout.as_deref()),
            ),
        ];

        let mut content = div().flex().flex_col();
        for (label, value) in rows {
            content = content.child(Self::inspector_row(label, value, text_muted, text_primary));
        }
        content.into_any_element()
    }

    fn render_inspector_keyboard_tab(
        &self,
        text_primary: gpui::Rgba,
        text_muted: gpui::Rgba,
    ) -> AnyElement {
        let header = div()
            .flex_none()
            .h(px(INSPECTOR_ROW_HEIGHT))
            .flex()
            .items_center()
            .gap(px(12.0))
            .text_color(text_muted)
            .font_weight(FontWeight::SEMIBOLD)
            .child(div().flex_none().w(px(48.0)).child("#"))
            .child(div().flex_none().w(px(140.0)).child("Key"))
            .child(div().flex_none().w(px(80.0)).child("Mods"))
            .child(div().flex_none().w(px(80.0)).child("Char"))
            .child(div().flex_1().child("Routed To"));

        let mut list = div().flex().flex_col().child(header);
        if self.inspector.key_log.is_empty() {
            list = list.child(
                div()
                    .flex_none()
                    .h(px(INSPECTOR_ROW_HEIGHT))
                    .flex()
                    .items_center()
                    .text_color(text_muted)
                    .child("Press keys to record events…"),
            );
        }
        for entry in &self.inspector.key_log {
            list = list.child(
                div()
                    .flex_none()
                    .h(px(INSPECTOR_ROW_HEIGHT))
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .text_color(text_primary)
                    .child(
                        div()
                            .flex_none()
                            .w(px(48.0))
                            .text_color(text_muted)
                            .child(entry.seq.to_string()),
                    )
                    .child(div().flex_none().w(px(140.0)).child(entry.key.clone()))
                    .child(div().flex_none().w(px(80.0)).child(entry.modifiers.clone()))
                    .child(
                        div()
                            .flex_none()
                            .w(px(80.0))
                            .text_color(text_muted)
                            .child(entry.key_char.clone().unwrap_or_else(|| "-".to_string())),
                    )
                    .child(div().flex_1().text_color(text_muted).child(entry.route)),
            );
        }
        list.into_any_element()
    }

    fn render_inspector_render_tab(
        &self,
        text_primary: gpui::Rgba,
        text_muted: gpui::Rgba,
    ) -> AnyElement {
        let stats = &self.debug_overlay_stats;
        let terminal_ui = terminal_ui_render_metrics_snapshot();
        let base_rows: Vec<(&'static str, String)> = vec![
            (
                "Render callbacks / sec",
                format!("{:.1}", stats.render_callbacks_per_second),
            ),
            (
                "Callback interval p50 / p95 / p99",
                format!(
                    "{:.2} ms / {:.2} ms / {:.2} ms",
                    stats.render_callback_interval_p50_ms,
                    stats.render_callback_interval_p95_ms,
                    stats.render_callback_interval_p99_ms
                ),
            ),
            (
                "CPU view build p50 / p95 / p99",
                format!(
                    "{:.2} ms / {:.2} ms / {:.2} ms",
                    stats.view_build_p50_ms, stats.view_build_p95_ms, stats.view_build_p99_ms
                ),
            ),
            ("CPU", format!("{:.1}%", stats.cpu_percent)),
            (
                "Process RSS (not total GPU memory)",
                self.debug_overlay_memory_label(),
            ),
            ("View Wake Signals", stats.view_wake_signals.to_string()),
            (
                "Event Drain Passes",
                stats.terminal_event_drain_passes.to_string(),
            ),
            ("Terminal Redraws", stats.terminal_redraws.to_string()),
            (
                "Alt-Screen Fallback Redraws",
                stats.alt_screen_fallback_redraws.to_string(),
            ),
            (
                "Spans (damage/rebuild/shape/paint)",
                format!(
                    "{:.2} / {:.2} / {:.2} / {:.2} ms",
                    stats.span_damage_ms,
                    stats.span_rebuild_ms,
                    stats.span_shaping_ms,
                    stats.span_paint_ms
                ),
            ),
            ("Grid Paints", terminal_ui.grid_paint_count.to_string()),
            (
                "Shape Calls / Hits / Misses",
                format!(
                    "{} / {} / {}",
                    terminal_ui.shape_line_calls,
                    terminal_ui.shaped_line_cache_hits,
                    terminal_ui.shaped_line_cache_misses
                ),
            ),
            (
                "Runtime Wakeups",
                terminal_ui.runtime_wakeup_count.to_string(),
            ),
            (
                "Span Totals (damage/rebuild/shape/paint)",
                format!(
                    "{} / {} / {} / {} us",
                    terminal_ui.span_damage_compute_us,
                    terminal_ui.span_row_ops_rebuild_us,
                    terminal_ui.span_text_shaping_us,
                    terminal_ui.span_grid_paint_us
                ),
            ),
        ];

        let rows = {
            #[cfg(debug_assertions)]
            let rows = {
                let mut rows = base_rows;
                let counters = self.render_metrics.counters;
                let delta = counters.saturating_sub(self.render_metrics.last_emit_counters);
                rows.extend([
                    (
                        "Render Metrics Enabled",
                        bool_label(self.render_metrics.enabled).to_string(),
                    ),
                    (
                        "Cache Full / Partial / Reuse",
                        format!(
                            "{} / {} / {}",
                            counters.cache_full_count,
                            counters.cache_partial_count,
                            counters.cache_reuse_count
                        ),
                    ),
                    (
                        "Dirty Spans / Patched Cells",
                        format!(
                            "{} / {}",
                            counters.dirty_span_count, counters.patched_cell_count
                        ),
                    ),
                    (
                        "Since Last Log: Render / Full / Partial / Reuse",
                        format!(
                            "{} / {} / {} / {}",
                            delta.render_count,
                            delta.cache_full_count,
                            delta.cache_partial_count,
                            delta.cache_reuse_count
                        ),
                    ),
                    (
                        "Since Last Log: Dirty / Patched",
                        format!("{} / {}", delta.dirty_span_count, delta.patched_cell_count),
                    ),
                ]);
                rows
            };

            #[cfg(not(debug_assertions))]
            let rows = base_rows;

            rows
        };

        let mut content = div().flex().flex_col();
        for (label, value) in rows {
            content = content.child(Self::inspector_row(label, value, text_muted, text_primary));
        }
        #[cfg(not(debug_assertions))]
        {
            content = content.child(
                div()
                    .flex_none()
                    .mt(px(8.0))
                    .text_color(text_muted)
                    .child("Cache and dirty-span counters require a debug build."),
            );
        }
        content = content.child(
            div()
                .flex_none()
                .mt(px(8.0))
                .text_color(text_muted)
                .child(
                    "Callback interval is idle/activity cadence, not latency; ~500 ms can be healthy while idle. CPU view build excludes GPUI layout/paint, GPU work, and presentation.",
                ),
        );
        content.into_any_element()
    }

    pub(super) fn render_inspector_panel(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.inspector.open {
            return None;
        }

        let colors = self.colors.clone();
        let mut text_primary = colors.foreground;
        text_primary.a = 0.92;
        let mut text_muted = colors.foreground;
        text_muted.a = 0.58;
        let accent = colors.cursor;
        let stroke = resolve_chrome_stroke_color(
            colors.background,
            colors.foreground,
            self.chrome_contrast_profile().stroke_mix,
        );
        let mut panel_bg = colors.foreground;
        panel_bg.a = self.scaled_chrome_surface_alpha(0.025);
        let mut active_tab_bg = colors.foreground;
        active_tab_bg.a = self.scaled_chrome_surface_alpha(0.11);
        let mut hover_tab_bg = colors.foreground;
        hover_tab_bg.a = self.scaled_chrome_surface_alpha(0.05);
        let mut indicator = colors.cursor;
        indicator.a = self.scaled_chrome_accent_alpha(0.90);

        let active_tab = self.inspector.tab;
        let mut tab_bar = div()
            .flex_none()
            .h(px(INSPECTOR_TAB_BAR_HEIGHT))
            .px(px(8.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .border_b_1()
            .border_color(stroke);

        for tab in InspectorTab::ALL {
            let is_active = tab == active_tab;
            let chip_text = if is_active { text_primary } else { text_muted };
            tab_bar = tab_bar.child(
                div()
                    .id(SharedString::from(format!("inspector-tab-{}", tab.label())))
                    .relative()
                    .h(px(24.0))
                    .px(px(10.0))
                    .rounded(px(INSPECTOR_TAB_RADIUS))
                    .flex()
                    .items_center()
                    .bg(if is_active {
                        active_tab_bg
                    } else {
                        gpui::transparent_black().into()
                    })
                    .hover(move |s| s.bg(hover_tab_bg))
                    .cursor_pointer()
                    .text_size(px(INSPECTOR_TEXT_SIZE))
                    .text_color(chip_text)
                    .children(is_active.then(|| {
                        div()
                            .absolute()
                            .left_0()
                            .top(px(6.0))
                            .w(px(2.0))
                            .h(px(12.0))
                            .rounded_full()
                            .bg(indicator)
                    }))
                    .child(tab.label())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event: &MouseDownEvent, _window, cx| {
                            cx.stop_propagation();
                            if this.inspector.tab != tab {
                                this.inspector.tab = tab;
                                cx.notify();
                            }
                        }),
                    ),
            );
        }

        tab_bar = tab_bar.child(div().flex_1());

        if active_tab == InspectorTab::Keyboard {
            tab_bar = tab_bar.child(
                div()
                    .id("inspector-clear-key-log")
                    .h(px(22.0))
                    .px(px(8.0))
                    .rounded(px(INSPECTOR_TAB_RADIUS))
                    .flex()
                    .items_center()
                    .text_size(px(INSPECTOR_TEXT_SIZE))
                    .text_color(text_muted)
                    .hover(move |s| s.bg(hover_tab_bg).text_color(text_primary))
                    .cursor_pointer()
                    .child("Clear")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event: &MouseDownEvent, _window, cx| {
                            cx.stop_propagation();
                            this.inspector.key_log.clear();
                            cx.notify();
                        }),
                    ),
            );
        }

        tab_bar = tab_bar.child(
            div()
                .id("inspector-close")
                .h(px(22.0))
                .w(px(22.0))
                .rounded(px(INSPECTOR_TAB_RADIUS))
                .flex()
                .items_center()
                .justify_center()
                .text_color(text_muted)
                .hover(move |s| s.bg(hover_tab_bg).text_color(text_primary))
                .cursor_pointer()
                .child(
                    gpui::svg()
                        .path(gpui::SharedString::from("icons/tab_strip/x.svg"))
                        .size(px(10.0))
                        .text_color(text_muted),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event: &MouseDownEvent, _window, cx| {
                        cx.stop_propagation();
                        this.toggle_inspector(cx);
                    }),
                ),
        );

        let body = match active_tab {
            InspectorTab::Terminal => {
                self.render_inspector_terminal_tab(text_primary, text_muted, accent)
            }
            InspectorTab::Runtime => self.render_inspector_runtime_tab(text_primary, text_muted),
            InspectorTab::Input => self.render_inspector_input_tab(text_primary, text_muted),
            InspectorTab::Config => self.render_inspector_config_tab(text_primary, text_muted),
            InspectorTab::Keyboard => self.render_inspector_keyboard_tab(text_primary, text_muted),
            InspectorTab::Render => self.render_inspector_render_tab(text_primary, text_muted),
        };

        let resize_handle = div()
            .id("inspector-resize-handle")
            .absolute()
            .top(px(-(INSPECTOR_RESIZE_HANDLE_HEIGHT * 0.5)))
            .left_0()
            .right_0()
            .h(px(INSPECTOR_RESIZE_HANDLE_HEIGHT))
            .cursor_row_resize()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    this.inspector.resize_drag = Some(InspectorResizeDrag {
                        start_window_y: event.position.y.into(),
                        start_height: this.inspector_bottom_inset(),
                    });
                    cx.notify();
                }),
            );

        Some(
            div()
                .id("terminal-inspector")
                .relative()
                .flex_none()
                .w_full()
                .h(px(self.inspector_bottom_inset()))
                .flex()
                .flex_col()
                .bg(panel_bg)
                .border_t_1()
                .border_color(stroke)
                .font_family(self.ui_font_family.clone())
                .text_size(px(INSPECTOR_TEXT_SIZE))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_this, _event: &MouseDownEvent, _window, cx| {
                        cx.stop_propagation();
                    }),
                )
                .child(tab_bar)
                .child(
                    div()
                        .id("inspector-body")
                        .flex_1()
                        .min_h(px(0.0))
                        .overflow_y_scroll()
                        .px(px(12.0))
                        .py(px(8.0))
                        .child(body),
                )
                .child(resize_handle)
                .into_any_element(),
        )
    }
}
