use super::*;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::atomic::{AtomicU64, Ordering},
};
use termy_core::plugin_runtime::{
    PluginAction, PluginCommand, PluginCommandPlacement, PluginContext, PluginEvent,
    PluginEventDispatch, PluginIcon, PluginInput, PluginOriginContext, PluginPaneContext,
    PluginPaneKind, PluginRuntimeKind, PluginSelectOption, PluginTabContext, PluginTerminalLaunch,
    PluginTerminalOpenLocation, PluginTerminalTarget, PluginTerminalTargetKind, PluginToastLevel,
};

const PLUGIN_SELECTED_TEXT_MAX_BYTES: usize = 64 * 1024;
const MAX_PENDING_PLUGIN_EVENTS: usize = 64;
static NEXT_PLUGIN_WINDOW_ID: AtomicU64 = AtomicU64::new(1);

struct PendingPluginEvent {
    event: PluginEvent,
    context: PluginContext,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct PluginLifecycleSnapshot {
    active_tab_id: Option<String>,
    active_tab_index: Option<usize>,
    active_pane_id: Option<String>,
    tab_ids: BTreeSet<String>,
    working_directory: Option<String>,
    active_command: Option<String>,
}

#[derive(Default)]
struct PluginLifecycleTracker {
    snapshot: PluginLifecycleSnapshot,
    command_started_at: Option<Instant>,
}

impl PluginLifecycleTracker {
    fn reset(&mut self, snapshot: PluginLifecycleSnapshot) {
        self.command_started_at = snapshot.active_command.as_ref().map(|_| Instant::now());
        self.snapshot = snapshot;
    }

    fn update(
        &mut self,
        next: PluginLifecycleSnapshot,
        infer_command_finished: bool,
        now: Instant,
    ) -> Vec<PluginEvent> {
        let mut events = Vec::new();
        for tab_id in next.tab_ids.difference(&self.snapshot.tab_ids) {
            events.push(PluginEvent::TabCreated {
                tab_id: tab_id.clone(),
            });
        }
        for tab_id in self.snapshot.tab_ids.difference(&next.tab_ids) {
            events.push(PluginEvent::TabClosed {
                tab_id: tab_id.clone(),
            });
        }
        if next.active_tab_id != self.snapshot.active_tab_id {
            events.push(PluginEvent::TabActivated {
                previous_tab_index: self.snapshot.active_tab_index,
                previous_tab_id: self.snapshot.active_tab_id.clone(),
            });
        }
        if next.active_pane_id != self.snapshot.active_pane_id {
            events.push(PluginEvent::PaneActivated {
                previous_pane_id: self.snapshot.active_pane_id.clone(),
            });
        }
        if next.working_directory != self.snapshot.working_directory {
            events.push(PluginEvent::WorkingDirectoryChanged {
                previous_working_directory: self.snapshot.working_directory.clone(),
                working_directory: next.working_directory.clone(),
            });
        }
        if infer_command_finished
            && self.snapshot.active_command.is_some()
            && next.active_command != self.snapshot.active_command
        {
            events.push(PluginEvent::CommandFinished {
                command: self.snapshot.active_command.clone(),
                exit_code: None,
                duration_ms: self
                    .command_started_at
                    .map(|started| duration_millis(now.saturating_duration_since(started))),
            });
        }
        if next.active_command != self.snapshot.active_command {
            if let Some(command) = next.active_command.clone() {
                events.push(PluginEvent::CommandStarted {
                    command: Some(command),
                });
            }
            self.command_started_at = next.active_command.as_ref().map(|_| now);
        }
        self.snapshot = next;
        events
    }
}

pub(in crate::terminal_view) struct PluginLifecycleState {
    window_handle: gpui::AnyWindowHandle,
    window_id: String,
    terminal_ready_emitted: bool,
    tracker: PluginLifecycleTracker,
    pending: VecDeque<PendingPluginEvent>,
    dispatch_in_flight: bool,
    overflow_warned: bool,
}

impl PluginLifecycleState {
    pub(in crate::terminal_view) fn new(window_handle: gpui::AnyWindowHandle) -> Self {
        Self {
            window_handle,
            window_id: format!(
                "window-{}",
                NEXT_PLUGIN_WINDOW_ID.fetch_add(1, Ordering::Relaxed)
            ),
            terminal_ready_emitted: false,
            tracker: PluginLifecycleTracker::default(),
            pending: VecDeque::new(),
            dispatch_in_flight: false,
            overflow_warned: false,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct PluginInputSession {
    pub(super) command: PluginCommand,
    pub(super) revision: String,
    pub(super) input_index: usize,
    pub(super) values: BTreeMap<String, Value>,
    pick_input_id: Option<String>,
    pick_query: String,
    pick_options: Vec<PluginSelectOption>,
    pick_loading: bool,
    pick_error: Option<String>,
    pick_generation: u64,
}

impl PluginInputSession {
    fn new(command: PluginCommand, revision: String) -> Self {
        Self {
            command,
            revision,
            input_index: 0,
            values: BTreeMap::new(),
            pick_input_id: None,
            pick_query: String::new(),
            pick_options: Vec::new(),
            pick_loading: false,
            pick_error: None,
            pick_generation: 0,
        }
    }

    pub(super) fn current_input(&self) -> Option<&PluginInput> {
        self.command.inputs.get(self.input_index)
    }

    pub(super) fn is_last_input(&self) -> bool {
        self.input_index + 1 >= self.command.inputs.len()
    }

    pub(super) fn progress_label(&self) -> String {
        format!("{} of {}", self.input_index + 1, self.command.inputs.len())
    }

    pub(super) fn can_go_back(&self) -> bool {
        self.input_index > 0
    }

    pub(super) fn placeholder(&self) -> String {
        let Some(input) = self.current_input() else {
            return String::new();
        };
        if let Some(placeholder) = input.placeholder() {
            return placeholder.to_string();
        }
        match input {
            PluginInput::Text { label, .. } => label.clone(),
            PluginInput::Select { label, .. } => label.clone(),
            PluginInput::Pick { label, .. } => label.clone(),
            PluginInput::Confirm { .. } => "Choose Yes or No".to_string(),
        }
    }

    fn input_prefill(&self) -> String {
        match self.current_input() {
            Some(PluginInput::Text {
                id, default_value, ..
            }) => self
                .values
                .get(id)
                .and_then(Value::as_str)
                .map_or_else(|| default_value.clone().unwrap_or_default(), str::to_string),
            Some(
                PluginInput::Select { .. } | PluginInput::Pick { .. } | PluginInput::Confirm { .. },
            )
            | None => String::new(),
        }
    }

    fn move_back(&mut self) -> Option<String> {
        if self.input_index == 0 {
            return None;
        }
        self.input_index -= 1;
        self.reset_pick();
        self.current_input()?;
        Some(self.input_prefill())
    }

    fn reset_pick(&mut self) {
        self.pick_input_id = None;
        self.pick_query.clear();
        self.pick_options.clear();
        self.pick_loading = false;
        self.pick_error = None;
        self.pick_generation = self.pick_generation.wrapping_add(1);
    }
}

fn promote_stored_plugin_input_value(
    items: &mut Vec<CommandPaletteItem>,
    stored_value: Option<&Value>,
) {
    let Some(stored_value) = stored_value else {
        return;
    };
    let Some(index) = items.iter().position(|item| {
        matches!(
            &item.kind,
            CommandPaletteItemKind::PluginInputOption { value, .. } if value == stored_value
        )
    }) else {
        return;
    };
    let stored_item = items.remove(index);
    items.insert(0, stored_item);
}

fn preferred_pick_value(stored: Option<&Value>, default_value: Option<&str>) -> Option<Value> {
    stored
        .cloned()
        .or_else(|| default_value.map(|value| Value::String(value.to_string())))
}

fn plugin_selected_text(mut text: Option<String>) -> (Option<String>, bool) {
    let Some(value) = text.as_mut() else {
        return (None, false);
    };
    if value.len() <= PLUGIN_SELECTED_TEXT_MAX_BYTES {
        return (text, false);
    }

    let mut boundary = PLUGIN_SELECTED_TEXT_MAX_BYTES;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
    (text, true)
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn placed_plugin_commands(
    mut commands: Vec<PluginCommand>,
    placement: PluginCommandPlacement,
) -> Vec<PluginCommand> {
    commands.retain(|command| command.placements.contains(&placement));
    commands.sort_by(|left, right| {
        left.plugin_name
            .to_lowercase()
            .cmp(&right.plugin_name.to_lowercase())
            .then_with(|| left.title.to_lowercase().cmp(&right.title.to_lowercase()))
            .then_with(|| left.qualified_id().cmp(&right.qualified_id()))
    });
    commands
}

impl TerminalView {
    pub(in crate::terminal_view) fn plugin_context(
        &mut self,
        cx: &mut Context<Self>,
    ) -> PluginContext {
        let working_directory = self.preferred_working_dir_for_new_session(None, cx);
        let (selected_text, selected_text_truncated) = plugin_selected_text(self.selected_text());
        let active_tab =
            self.session
                .tabs
                .get(self.session.active_tab)
                .map(|tab| PluginTabContext {
                    id: tab.id.to_string(),
                    index: self.session.active_tab,
                    title: tab.title.clone(),
                    pane_count: tab.panes.len(),
                });
        let active_pane = self
            .session
            .tabs
            .get(self.session.active_tab)
            .and_then(|tab| {
                let index = tab.active_pane_index()?;
                let pane = tab.panes.get(index)?;
                Some(PluginPaneContext {
                    id: pane.id.clone(),
                    index,
                    kind: PluginPaneKind::Terminal,
                })
            });
        PluginContext {
            origin: PluginOriginContext {
                window_id: self.plugin_lifecycle.window_id.clone(),
                tab_id: active_tab.as_ref().map(|tab| tab.id.clone()),
                pane_id: active_pane.as_ref().map(|pane| pane.id.clone()),
            },
            working_directory,
            active_command: self.active_current_command().map(str::to_string),
            selected_text,
            selected_text_truncated,
            shell: self.terminal_runtime.resolved_shell_program(),
            runtime: match self.runtime_kind() {
                RuntimeKind::Native => PluginRuntimeKind::Native,
                RuntimeKind::Tmux => PluginRuntimeKind::Tmux,
            },
            active_tab,
            active_pane,
            platform: std::env::consts::OS.to_string(),
            app_version: crate::APP_VERSION.to_string(),
            settings: BTreeMap::new(),
        }
    }

    fn plugin_lifecycle_snapshot(&self) -> PluginLifecycleSnapshot {
        let active_tab = self.session.tabs.get(self.session.active_tab);
        PluginLifecycleSnapshot {
            active_tab_id: active_tab.map(|tab| tab.id.to_string()),
            active_tab_index: active_tab.map(|_| self.session.active_tab),
            active_pane_id: active_tab
                .and_then(TerminalTab::active_pane_id)
                .map(str::to_string),
            tab_ids: self
                .session
                .tabs
                .iter()
                .map(|tab| tab.id.to_string())
                .collect(),
            working_directory: active_tab.and_then(|tab| tab.last_prompt_cwd.clone()),
            active_command: self.active_current_command().map(str::to_string),
        }
    }

    fn reset_plugin_lifecycle_baseline(&mut self) {
        let snapshot = self.plugin_lifecycle_snapshot();
        self.plugin_lifecycle.tracker.reset(snapshot);
    }

    fn ensure_plugin_terminal_ready(&mut self, cx: &mut Context<Self>) {
        if self.plugin_lifecycle.terminal_ready_emitted {
            return;
        }
        self.plugin_lifecycle.terminal_ready_emitted = true;
        self.reset_plugin_lifecycle_baseline();
        self.enqueue_plugin_event(PluginEvent::TerminalReady, cx);
    }

    pub(in super::super) fn sync_plugin_lifecycle_state(
        &mut self,
        infer_command_finished: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.plugin_lifecycle.terminal_ready_emitted {
            return;
        }

        let snapshot = self.plugin_lifecycle_snapshot();
        let events =
            self.plugin_lifecycle
                .tracker
                .update(snapshot, infer_command_finished, Instant::now());

        for event in events {
            self.enqueue_plugin_event(event, cx);
        }
    }

    pub(in super::super) fn enqueue_plugin_event(
        &mut self,
        event: PluginEvent,
        cx: &mut Context<Self>,
    ) {
        if !self.plugin_runtime.has_event_subscribers(event.kind()) {
            return;
        }
        if self.plugin_lifecycle.pending.len() >= MAX_PENDING_PLUGIN_EVENTS {
            if !self.plugin_lifecycle.overflow_warned {
                self.plugin_lifecycle.overflow_warned = true;
                log::warn!("Plugin lifecycle event queue is full; dropping events");
                crate::ui::toast::warning("Plugin events are falling behind");
                self.notify_overlay(cx);
            }
            return;
        }
        let context = self.plugin_context(cx);
        self.plugin_lifecycle
            .pending
            .push_back(PendingPluginEvent { event, context });
        self.dispatch_next_plugin_event(cx);
    }

    fn dispatch_next_plugin_event(&mut self, cx: &mut Context<Self>) {
        if self.plugin_lifecycle.dispatch_in_flight {
            return;
        }
        let Some(pending) = self.plugin_lifecycle.pending.pop_front() else {
            self.plugin_lifecycle.overflow_warned = false;
            return;
        };
        self.plugin_lifecycle.dispatch_in_flight = true;
        let runtime = self.plugin_runtime.clone();
        let window_handle = self.plugin_lifecycle.window_handle;
        cx.spawn(async move |_this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let dispatch =
                smol::unblock(move || runtime.dispatch_event(pending.event, pending.context)).await;
            let _ = cx.update(|cx| {
                let Some(window_handle) = window_handle.downcast::<Self>() else {
                    return;
                };
                let _ = window_handle.update(cx, |view, window, cx| {
                    view.plugin_lifecycle.dispatch_in_flight = false;
                    view.apply_plugin_event_dispatch(dispatch, window, cx);
                    view.dispatch_next_plugin_event(cx);
                });
            });
        })
        .detach();
    }

    fn apply_plugin_event_dispatch(
        &mut self,
        dispatch: PluginEventDispatch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !dispatch.errors.is_empty() {
            let message = dispatch.errors.join("; ");
            log::error!("Plugin event failed: {message}");
            crate::ui::toast::error(format!("Plugin event failed: {message}"));
        }
        if let Err(error) = self.apply_plugin_actions(dispatch.actions, window, cx) {
            log::error!("Plugin event action failed: {error}");
            crate::ui::toast::error(error);
        }
        self.notify_overlay(cx);
    }

    fn plugin_refresh_error_message(errors: &[String]) -> Option<String> {
        if errors.is_empty() {
            return None;
        }
        let mut message = errors
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join("; ");
        if errors.len() > 3 {
            message.push_str(&format!("; and {} more", errors.len() - 3));
        }
        Some(message)
    }

    fn update_plugin_refresh_error(&mut self, errors: &[String], cx: &mut Context<Self>) {
        let error_message = Self::plugin_refresh_error_message(errors);
        if error_message == self.plugin_last_error {
            return;
        }
        if let Some(error) = error_message.as_deref() {
            log::error!("Plugin refresh failed: {error}");
            crate::ui::toast::error(format!("Plugin error: {error}"));
            self.notify_overlay(cx);
        }
        self.plugin_last_error = error_message;
    }

    pub(in super::super) fn schedule_plugin_refresh(&mut self, cx: &mut Context<Self>) {
        if self.plugin_refresh_in_flight {
            return;
        }
        self.plugin_refresh_in_flight = true;
        let runtime = self.plugin_runtime.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let refresh = smol::unblock(move || runtime.refresh_if_changed()).await;
            let _ = cx.update(|cx| {
                this.update(cx, |view, cx| {
                    view.plugin_refresh_in_flight = false;
                    view.update_plugin_refresh_error(&refresh.errors, cx);
                    view.ensure_plugin_terminal_ready(cx);
                    view.plugin_runtime.suspend_if_eventless();
                    if refresh.changed
                        && view.is_command_palette_open()
                        && view.command_palette.mode() == CommandPaletteMode::Commands
                    {
                        view.apply_command_palette_mode_setup(
                            CommandPaletteMode::Commands,
                            false,
                            CommandPaletteNotifyEvent::InteractionOnly,
                            cx,
                        );
                    }
                    if refresh.changed
                        && (view.terminal_context_menu.is_some() || view.tab_context_menu.is_some())
                    {
                        view.notify_overlay(cx);
                    }
                })
            });
        })
        .detach();
    }

    #[cfg(not(test))]
    pub(in super::super) fn schedule_initial_plugin_refresh(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            // Keep Bun discovery and TypeScript loading out of the first-paint
            // window. Explicit plugin entry points still refresh on demand.
            smol::Timer::after(Duration::from_millis(250)).await;
            let _ = cx.update(|cx| this.update(cx, |view, cx| view.schedule_plugin_refresh(cx)));
        })
        .detach();
    }

    pub(super) fn command_palette_plugin_items(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Vec<CommandPaletteItem> {
        self.plugin_commands_for_placement(PluginCommandPlacement::CommandPalette, cx)
            .into_iter()
            .map(|command| {
                let enabled = command.disabled_reason.is_none();
                let mut keywords = vec![
                    "plugin".to_string(),
                    command.plugin_id.replace(['-', '_', '.'], " "),
                    command.plugin_name.clone(),
                    command.id.replace(['-', '_', '.'], " "),
                ];
                keywords.extend(command.keywords.iter().cloned());
                CommandPaletteItem {
                    title: command.title,
                    keywords: keywords.join(" "),
                    enabled,
                    status_hint: command.disabled_reason.or(command.status),
                    tmux_status_hint: None,
                    kind: CommandPaletteItemKind::PluginCommand {
                        plugin_id: command.plugin_id,
                        command_id: command.id,
                        icon: command.icon,
                    },
                }
            })
            .collect()
    }

    pub(in crate::terminal_view) fn plugin_commands_for_placement(
        &mut self,
        placement: PluginCommandPlacement,
        cx: &mut Context<Self>,
    ) -> Vec<PluginCommand> {
        let context = self.plugin_context(cx);
        let mut commands = placed_plugin_commands(self.plugin_runtime.commands(), placement);
        commands.retain(|command| command.when.matches(&context));
        commands
    }

    pub(super) fn plugin_input_uses_free_text(&self) -> bool {
        matches!(
            self.command_palette
                .plugin_input_session
                .as_ref()
                .and_then(PluginInputSession::current_input),
            Some(PluginInput::Text { .. } | PluginInput::Pick { .. })
        )
    }

    pub(super) fn command_palette_plugin_input_items(&self) -> Vec<CommandPaletteItem> {
        let Some(session) = self.command_palette.plugin_input_session.as_ref() else {
            return Vec::new();
        };
        let Some(input) = session.current_input() else {
            return Vec::new();
        };
        match input {
            PluginInput::Text {
                required,
                max_length,
                ..
            } => {
                let value = self.command_palette.input().text();
                let char_count = value.chars().count();
                let (enabled, status_hint) = if *required && value.trim().is_empty() {
                    (false, Some("Required".to_string()))
                } else if char_count > *max_length {
                    (false, Some(format!("Max {max_length} characters")))
                } else {
                    (true, None)
                };
                vec![CommandPaletteItem {
                    title: if session.is_last_input() {
                        format!("Run {}", session.command.title)
                    } else {
                        "Continue".to_string()
                    },
                    keywords: String::new(),
                    enabled,
                    status_hint,
                    tmux_status_hint: None,
                    kind: CommandPaletteItemKind::PluginInputSubmit {
                        icon: session.command.icon,
                    },
                }]
            }
            PluginInput::Select {
                required,
                default_value,
                options,
                ..
            } => {
                let mut options = options.clone();
                if let Some(default_value) = default_value
                    && let Some(index) = options
                        .iter()
                        .position(|option| option.value == *default_value)
                {
                    let default = options.remove(index);
                    options.insert(0, default);
                }
                let mut items = options
                    .into_iter()
                    .map(|option| CommandPaletteItem {
                        title: option.label.clone(),
                        keywords: std::iter::once(option.value.clone())
                            .chain(option.keywords)
                            .collect::<Vec<_>>()
                            .join(" "),
                        enabled: true,
                        status_hint: option.status,
                        tmux_status_hint: None,
                        kind: CommandPaletteItemKind::PluginInputOption {
                            value: Value::String(option.value),
                            icon: session.command.icon,
                        },
                    })
                    .collect::<Vec<_>>();
                if !required {
                    items.push(CommandPaletteItem {
                        title: "Skip".to_string(),
                        keywords: "none skip optional".to_string(),
                        enabled: true,
                        status_hint: Some("Optional".to_string()),
                        tmux_status_hint: None,
                        kind: CommandPaletteItemKind::PluginInputOption {
                            value: Value::Null,
                            icon: session.command.icon,
                        },
                    });
                }
                promote_stored_plugin_input_value(&mut items, session.values.get(input.id()));
                items
            }
            PluginInput::Pick {
                required,
                default_value,
                ..
            } => {
                if session.pick_loading {
                    return vec![CommandPaletteItem {
                        title: "Loading options…".to_string(),
                        keywords: String::new(),
                        enabled: false,
                        status_hint: None,
                        tmux_status_hint: None,
                        kind: CommandPaletteItemKind::PluginInputOption {
                            value: Value::Null,
                            icon: session.command.icon,
                        },
                    }];
                }
                if let Some(error) = session.pick_error.as_ref() {
                    return vec![CommandPaletteItem {
                        title: "Couldn’t load options".to_string(),
                        keywords: String::new(),
                        enabled: false,
                        status_hint: Some(error.clone()),
                        tmux_status_hint: None,
                        kind: CommandPaletteItemKind::PluginInputOption {
                            value: Value::Null,
                            icon: session.command.icon,
                        },
                    }];
                }
                let mut items = session
                    .pick_options
                    .iter()
                    .map(|option| CommandPaletteItem {
                        title: option.label.clone(),
                        keywords: std::iter::once(option.value.clone())
                            .chain(option.keywords.iter().cloned())
                            .collect::<Vec<_>>()
                            .join(" "),
                        enabled: true,
                        status_hint: option.status.clone(),
                        tmux_status_hint: None,
                        kind: CommandPaletteItemKind::PluginInputOption {
                            value: Value::String(option.value.clone()),
                            icon: session.command.icon,
                        },
                    })
                    .collect::<Vec<_>>();
                if !required {
                    items.push(CommandPaletteItem {
                        title: "Skip".to_string(),
                        keywords: "none skip optional".to_string(),
                        enabled: true,
                        status_hint: Some("Optional".to_string()),
                        tmux_status_hint: None,
                        kind: CommandPaletteItemKind::PluginInputOption {
                            value: Value::Null,
                            icon: session.command.icon,
                        },
                    });
                }
                if items.is_empty() {
                    items.push(CommandPaletteItem {
                        title: "No matching options".to_string(),
                        keywords: String::new(),
                        enabled: false,
                        status_hint: None,
                        tmux_status_hint: None,
                        kind: CommandPaletteItemKind::PluginInputOption {
                            value: Value::Null,
                            icon: session.command.icon,
                        },
                    });
                }
                let selected_value =
                    preferred_pick_value(session.values.get(input.id()), default_value.as_deref());
                promote_stored_plugin_input_value(&mut items, selected_value.as_ref());
                items
            }
            PluginInput::Confirm { default_value, .. } => {
                let values = if *default_value {
                    [("Yes", true), ("No", false)]
                } else {
                    [("No", false), ("Yes", true)]
                };
                let mut items = values
                    .into_iter()
                    .map(|(label, value)| CommandPaletteItem {
                        title: label.to_string(),
                        keywords: if value { "yes confirm" } else { "no cancel" }.to_string(),
                        enabled: true,
                        status_hint: None,
                        tmux_status_hint: None,
                        kind: CommandPaletteItemKind::PluginInputOption {
                            value: Value::Bool(value),
                            icon: session.command.icon,
                        },
                    })
                    .collect::<Vec<_>>();
                promote_stored_plugin_input_value(&mut items, session.values.get(input.id()));
                items
            }
        }
    }

    pub(super) fn schedule_plugin_pick_options(&mut self, cx: &mut Context<Self>) {
        let query = self.command_palette.input().text().to_string();
        let request = {
            let Some(session) = self.command_palette.plugin_input_session.as_mut() else {
                return;
            };
            let Some(PluginInput::Pick { id, .. }) = session.current_input().cloned() else {
                return;
            };
            if session.pick_input_id.as_deref() == Some(id.as_str()) && session.pick_query == query
            {
                return;
            }
            session.pick_generation = session.pick_generation.wrapping_add(1);
            session.pick_input_id = Some(id.clone());
            session.pick_query = query.clone();
            session.pick_options.clear();
            session.pick_loading = true;
            session.pick_error = None;
            (
                session.command.plugin_id.clone(),
                session.command.id.clone(),
                id,
                session.revision.clone(),
                session.pick_generation,
            )
        };
        let context = self.plugin_context(cx);
        let runtime = self.plugin_runtime.clone();
        let window_handle = self.plugin_lifecycle.window_handle;
        cx.spawn(async move |_this: WeakEntity<Self>, cx: &mut AsyncApp| {
            smol::Timer::after(Duration::from_millis(150)).await;
            let (plugin_id, command_id, input_id, revision, generation) = request;
            let completed_input_id = input_id.clone();
            let result = smol::unblock(move || {
                runtime.resolve_pick_options(
                    &plugin_id,
                    &command_id,
                    &input_id,
                    &revision,
                    &query,
                    context,
                )
            })
            .await;
            let _ = cx.update(|cx| {
                let Some(window_handle) = window_handle.downcast::<Self>() else {
                    return;
                };
                let _ = window_handle.update(cx, |view, _window, cx| {
                    let Some(session) = view.command_palette.plugin_input_session.as_mut() else {
                        return;
                    };
                    if session.pick_generation != generation
                        || session.pick_input_id.as_deref() != Some(completed_input_id.as_str())
                    {
                        return;
                    }
                    session.pick_loading = false;
                    match result {
                        Ok(options) => {
                            session.pick_options = options;
                            session.pick_error = None;
                        }
                        Err(error) => {
                            session.pick_options.clear();
                            session.pick_error = Some(error);
                        }
                    }
                    view.refresh_command_palette_matches(false, cx);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub(super) fn plugin_input_mode_title(&self) -> String {
        self.command_palette
            .plugin_input_session
            .as_ref()
            .map_or_else(
                || "Plugin input".to_string(),
                |session| session.command.title.clone(),
            )
    }

    pub(super) fn plugin_input_progress_label(&self) -> String {
        self.command_palette
            .plugin_input_session
            .as_ref()
            .map_or_else(String::new, PluginInputSession::progress_label)
    }

    pub(super) fn plugin_input_placeholder(&self) -> String {
        self.command_palette
            .plugin_input_session
            .as_ref()
            .map_or_else(String::new, PluginInputSession::placeholder)
    }

    pub(super) fn plugin_input_can_go_back(&self) -> bool {
        self.command_palette
            .plugin_input_session
            .as_ref()
            .is_some_and(PluginInputSession::can_go_back)
    }

    pub(super) fn plugin_input_is_last(&self) -> bool {
        self.command_palette
            .plugin_input_session
            .as_ref()
            .is_some_and(PluginInputSession::is_last_input)
    }

    pub(in crate::terminal_view) fn start_plugin_command(
        &mut self,
        plugin_id: &str,
        command_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((command, revision)) = self
            .plugin_runtime
            .command_with_revision(plugin_id, command_id)
        else {
            crate::ui::toast::error(format!(
                "Plugin command {plugin_id}.{command_id} is unavailable"
            ));
            self.notify_overlay(cx);
            return;
        };
        if let Some(reason) = command.disabled_reason.as_deref() {
            crate::ui::toast::info(reason);
            self.notify_overlay(cx);
            return;
        }
        let context = self.plugin_context(cx);
        if !command.when.matches(&context) {
            crate::ui::toast::info("That plugin command is not available in this terminal context");
            self.notify_overlay(cx);
            return;
        }
        if command.inputs.is_empty() {
            self.invoke_plugin_command(command, revision, BTreeMap::new(), window, cx);
            return;
        }

        let was_open = self.command_palette.is_open();
        let session = PluginInputSession::new(command, revision);
        let prefill = session.input_prefill();
        self.command_palette.begin_plugin_inputs(session);
        self.command_palette.input_mut().set_text(prefill);
        self.apply_command_palette_mode_setup(
            CommandPaletteMode::PluginInputs,
            false,
            if was_open {
                CommandPaletteNotifyEvent::InteractionOnly
            } else {
                CommandPaletteNotifyEvent::OpenCloseTransition
            },
            cx,
        );
    }

    pub(in super::super) fn run_plugin_command_from_keybinding(
        &mut self,
        plugin_id: &str,
        command_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(window_handle) = window.window_handle().downcast::<Self>() else {
            crate::ui::toast::error("Plugin keybinding lost its Termy window");
            self.notify_overlay(cx);
            return;
        };
        let runtime = self.plugin_runtime.clone();
        let plugin_id = plugin_id.to_string();
        let command_id = command_id.to_string();
        cx.spawn(async move |_this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let refresh = smol::unblock(move || runtime.refresh_if_changed()).await;
            let _ = cx.update(|cx| {
                let _ = window_handle.update(cx, |view, window, cx| {
                    view.update_plugin_refresh_error(&refresh.errors, cx);
                    view.start_plugin_command(&plugin_id, &command_id, window, cx);
                });
            });
        })
        .detach();
    }

    pub(super) fn submit_plugin_text_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = Value::String(self.command_palette.input().text().to_string());
        self.submit_plugin_input_value(value, window, cx);
    }

    pub(super) fn submit_plugin_input_value(
        &mut self,
        value: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut completed = None;
        let mut next_prefill = None;
        {
            let Some(session) = self.command_palette.plugin_input_session.as_mut() else {
                return;
            };
            let Some(input) = session.current_input().cloned() else {
                return;
            };
            if let PluginInput::Text {
                required,
                max_length,
                ..
            } = &input
            {
                let text = value.as_str().unwrap_or_default();
                if *required && text.trim().is_empty() {
                    crate::ui::toast::info(format!("{} is required", input.label()));
                    self.notify_overlay(cx);
                    return;
                }
                if text.chars().count() > *max_length {
                    crate::ui::toast::info(format!(
                        "{} must be at most {max_length} characters",
                        input.label()
                    ));
                    self.notify_overlay(cx);
                    return;
                }
            }
            if let PluginInput::Pick { required, .. } = &input
                && *required
                && value.as_str().is_none_or(|value| value.trim().is_empty())
            {
                crate::ui::toast::info(format!("{} is required", input.label()));
                self.notify_overlay(cx);
                return;
            }
            if value.is_null() {
                session.values.remove(input.id());
            } else {
                session.values.insert(input.id().to_string(), value);
            }
            if session.is_last_input() {
                completed = Some((
                    session.command.clone(),
                    session.revision.clone(),
                    session.values.clone(),
                ));
            } else {
                session.input_index += 1;
                session.reset_pick();
                next_prefill = Some(session.input_prefill());
            }
        }

        if let Some((command, revision, values)) = completed {
            self.invoke_plugin_command(command, revision, values, window, cx);
            return;
        }
        self.command_palette
            .input_mut()
            .set_text(next_prefill.unwrap_or_default());
        self.command_palette.reset_for_next_plugin_input();
        self.apply_command_palette_mode_setup(
            CommandPaletteMode::PluginInputs,
            false,
            CommandPaletteNotifyEvent::InteractionOnly,
            cx,
        );
    }

    pub(super) fn back_from_plugin_input(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(prefill) = self
            .command_palette
            .plugin_input_session
            .as_mut()
            .and_then(PluginInputSession::move_back)
        else {
            return false;
        };
        self.command_palette.input_mut().set_text(prefill);
        self.command_palette.reset_for_next_plugin_input();
        self.apply_command_palette_mode_setup(
            CommandPaletteMode::PluginInputs,
            false,
            CommandPaletteNotifyEvent::InteractionOnly,
            cx,
        );
        true
    }

    fn invoke_plugin_command(
        &mut self,
        command: PluginCommand,
        revision: String,
        inputs: BTreeMap<String, Value>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let context = self.plugin_context(cx);
        let runtime = self.plugin_runtime.clone();
        let plugin_id = command.plugin_id.clone();
        let command_id = command.id.clone();
        let title = command.title;
        let Some(window_handle) = window.window_handle().downcast::<Self>() else {
            crate::ui::toast::error("Plugin command lost its Termy window");
            self.notify_overlay(cx);
            return;
        };
        let loading_id = crate::ui::toast::enqueue_actionable_toast_with_id(
            crate::ui::toast::ToastKind::Loading,
            format!("Running {title}…"),
            Some(Duration::from_secs(60 * 60)),
            Some("Cancel".to_string()),
        );
        let (progress_tx, progress_rx) = flume::bounded(32);
        let control = termy_core::plugin_runtime::PluginInvocationControl::with_progress_handler(
            move |progress| {
                let _ = progress_tx.try_send(progress);
            },
        );
        self.plugin_invocations.insert(loading_id, control.clone());
        let progress_window_handle = window_handle;
        let progress_title = title;
        cx.spawn(async move |_this: WeakEntity<Self>, cx: &mut AsyncApp| {
            while let Ok(progress) = progress_rx.recv_async().await {
                let detail = match (progress.message, progress.percentage) {
                    (Some(message), Some(percentage)) => {
                        format!("{progress_title}: {message} ({percentage}%)")
                    }
                    (Some(message), None) => format!("{progress_title}: {message}"),
                    (None, Some(percentage)) => format!("{progress_title} ({percentage}%)"),
                    (None, None) => format!("Running {progress_title}…"),
                };
                let _ = cx.update(|cx| {
                    crate::ui::toast::update_toast(
                        loading_id,
                        crate::ui::toast::ToastKind::Loading,
                        detail,
                    );
                    let _ = progress_window_handle.update(cx, |view, _window, cx| {
                        view.notify_overlay(cx);
                    });
                });
            }
        })
        .detach();
        self.close_command_palette(cx);
        self.notify_overlay(cx);

        cx.spawn(async move |_this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result = smol::unblock(move || {
                let result = runtime.invoke_with_control(
                    &plugin_id,
                    &command_id,
                    &revision,
                    inputs,
                    context,
                    &control,
                );
                let opens_view = result.as_ref().is_ok_and(|actions| {
                    actions
                        .iter()
                        .any(|action| matches!(action, PluginAction::ViewOpen { .. }))
                });
                if !opens_view {
                    runtime.suspend_if_eventless();
                }
                result
            })
            .await;
            let _ = cx.update(|cx| {
                crate::ui::toast::dismiss_toast(loading_id);
                let _ = window_handle.update(cx, |view, window, cx| {
                    view.plugin_invocations.remove(&loading_id);
                    match result {
                        Ok(actions) => {
                            if let Err(error) = view.apply_plugin_actions(actions, window, cx) {
                                log::error!("Plugin action failed: {error}");
                                crate::ui::toast::error(error);
                            }
                        }
                        Err(error) => {
                            log::error!("Plugin command failed: {error}");
                            crate::ui::toast::error(error);
                        }
                    }
                    view.notify_overlay(cx);
                });
            });
        })
        .detach();
    }

    pub(in crate::terminal_view) fn cancel_plugin_invocation(
        &mut self,
        toast_id: u64,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(control) = self.plugin_invocations.get(&toast_id) else {
            return false;
        };
        control.cancel();
        crate::ui::toast::update_toast(
            toast_id,
            crate::ui::toast::ToastKind::Loading,
            "Cancelling plugin command…",
        );
        self.notify_overlay(cx);
        true
    }

    pub(in crate::terminal_view) fn apply_plugin_actions(
        &mut self,
        actions: Vec<PluginAction>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        for action in &actions {
            if let PluginAction::TermyCommand { command } = action
                && CommandAction::from_config_name(command).is_none()
            {
                return Err(format!("Plugin returned unknown Termy command `{command}`"));
            }
        }

        for action in actions {
            match action {
                PluginAction::TerminalRun {
                    command,
                    working_directory,
                } => {
                    let launch = TerminalLaunch::ShellCommand(command);
                    if !self.add_tab_with_launch(working_directory.as_deref(), Some(&launch), cx) {
                        return Err(
                            "Plugin command stopped because its terminal could not be created"
                                .to_string(),
                        );
                    }
                    cx.notify();
                }
                PluginAction::TerminalSendText {
                    text,
                    submit,
                    target,
                } => {
                    let (_, pane_id) = self.resolve_plugin_terminal_target(&target)?;
                    let mut input = text.into_bytes();
                    if submit {
                        input.push(b'\n');
                    }
                    if !self.send_input_to_pane(&pane_id, &input) {
                        return Err("Plugin terminal target stopped accepting input".to_string());
                    }
                }
                PluginAction::TerminalOpen {
                    location,
                    working_directory,
                    launch,
                    target,
                    focus,
                } => {
                    let previous_tab_id = self
                        .session
                        .tabs
                        .get(self.session.active_tab)
                        .map(|tab| tab.id);
                    let previous_pane_id = self.active_pane_id().map(str::to_string);
                    let launch = launch.map(Self::terminal_launch);
                    if self.runtime_kind().uses_tmux()
                        && matches!(launch, Some(TerminalLaunch::Program { .. }))
                    {
                        return Err(
                            "Plugin structured program launches require the native runtime"
                                .to_string(),
                        );
                    }
                    let _ = self.focus_plugin_terminal_target(&target, cx)?;
                    let opened = match location {
                        PluginTerminalOpenLocation::Tab => self.add_tab_with_launch(
                            working_directory.as_deref(),
                            launch.as_ref(),
                            cx,
                        ),
                        PluginTerminalOpenLocation::SplitRight => self
                            .split_active_pane_vertical_with_launch(
                                working_directory.as_deref(),
                                launch.as_ref(),
                                cx,
                            ),
                        PluginTerminalOpenLocation::SplitDown => self
                            .split_active_pane_horizontal_with_launch(
                                working_directory.as_deref(),
                                launch.as_ref(),
                                cx,
                            ),
                        PluginTerminalOpenLocation::Window => {
                            let window_handle =
                                crate::open_main_window_with_runtime_config_overrides(
                                    cx,
                                    working_directory.clone(),
                                )?;
                            if let Some(launch) = launch.clone() {
                                window_handle
                                    .update(cx, |view, _window, cx| match launch {
                                        TerminalLaunch::ShellCommand(command) => {
                                            let mut input = command.into_bytes();
                                            input.push(b'\n');
                                            view.send_input_to_active_pane(&input)
                                        }
                                        program @ TerminalLaunch::Program { .. } => {
                                            if !view.add_tab_with_launch(
                                                working_directory.as_deref(),
                                                Some(&program),
                                                cx,
                                            ) {
                                                return false;
                                            }
                                            view.close_tab(0, cx);
                                            true
                                        }
                                    })
                                    .map_err(|error| {
                                        format!(
                                            "Plugin terminal window became unavailable: {error}"
                                        )
                                    })?
                            } else {
                                true
                            }
                        }
                    };
                    if !opened {
                        return Err("Plugin terminal could not be opened".to_string());
                    }
                    if !focus && location != PluginTerminalOpenLocation::Window {
                        self.restore_plugin_terminal_focus(previous_tab_id, previous_pane_id, cx);
                    }
                }
                PluginAction::TermyCommand { command } => {
                    let action = CommandAction::from_config_name(&command)
                        .ok_or_else(|| format!("Unknown Termy command `{command}`"))?;
                    self.execute_command_action(action, false, window, cx);
                }
                PluginAction::ClipboardWrite { text } => {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
                PluginAction::UrlOpen { url } => {
                    webbrowser::open(&url)
                        .map_err(|error| format!("Failed to open plugin URL: {error}"))?;
                }
                PluginAction::Toast { level, message } => match level {
                    PluginToastLevel::Info => crate::ui::toast::info(message),
                    PluginToastLevel::Success => crate::ui::toast::success(message),
                    PluginToastLevel::Warning => crate::ui::toast::warning(message),
                    PluginToastLevel::Error => crate::ui::toast::error(message),
                },
                PluginAction::ViewOpen {
                    view,
                    target,
                    params,
                    plugin_id,
                    revision,
                } => {
                    self.open_plugin_ui(&plugin_id, &view, &revision, target, params, window, cx)?;
                }
                PluginAction::ViewReplace {
                    view,
                    params,
                    plugin_id,
                    revision,
                } => {
                    self.replace_plugin_ui(&plugin_id, &view, &revision, params, cx)?;
                }
                PluginAction::ViewClose {
                    plugin_id,
                    revision,
                } => {
                    let plugin_ui = self.plugin_ui.as_ref().ok_or_else(|| {
                        "Plugin returned view.close without an open view".to_string()
                    })?;
                    if !plugin_ui.read(cx).belongs_to(&plugin_id, &revision) {
                        return Err("Plugin cannot close another plugin's view".to_string());
                    }
                    self.close_plugin_ui(window, cx);
                }
            }
        }
        Ok(())
    }

    fn resolve_plugin_terminal_target(
        &self,
        target: &PluginTerminalTarget,
    ) -> Result<(usize, String), String> {
        let (tab_id, pane_id) = match target {
            PluginTerminalTarget::Named(PluginTerminalTargetKind::Active) => (None, None),
            PluginTerminalTarget::Named(PluginTerminalTargetKind::Origin) => {
                return Err("Plugin terminal origin was not resolved by the runtime".to_string());
            }
            PluginTerminalTarget::Exact {
                window_id,
                tab_id,
                pane_id,
            } => {
                if window_id != &self.plugin_lifecycle.window_id {
                    return Err("Plugin terminal target belongs to another window".to_string());
                }
                (tab_id.as_deref(), pane_id.as_deref())
            }
        };
        let tab_index = if let Some(tab_id) = tab_id {
            self.session
                .tabs
                .iter()
                .position(|tab| tab.id.to_string() == tab_id)
                .ok_or_else(|| "Plugin terminal target tab is no longer available".to_string())?
        } else {
            self.session.active_tab
        };
        let tab = self
            .session
            .tabs
            .get(tab_index)
            .ok_or_else(|| "Plugin terminal target tab is unavailable".to_string())?;
        let pane_id = pane_id
            .map(str::to_string)
            .or_else(|| tab.active_pane_id().map(str::to_string))
            .ok_or_else(|| "Plugin terminal target pane is unavailable".to_string())?;
        if !tab.panes.iter().any(|pane| pane.id == pane_id) {
            return Err("Plugin terminal target pane is no longer available".to_string());
        }
        Ok((tab_index, pane_id))
    }

    fn focus_plugin_terminal_target(
        &mut self,
        target: &PluginTerminalTarget,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        let (tab_index, pane_id) = self.resolve_plugin_terminal_target(target)?;
        if tab_index != self.session.active_tab {
            self.switch_tab(tab_index, cx);
        }
        if self.active_pane_id() != Some(pane_id.as_str()) {
            self.focus_pane_target(&pane_id, cx);
        }
        Ok(pane_id)
    }

    fn terminal_launch(launch: PluginTerminalLaunch) -> TerminalLaunch {
        match launch {
            PluginTerminalLaunch::Shell { command } => TerminalLaunch::ShellCommand(command),
            PluginTerminalLaunch::Program { program, args } => {
                TerminalLaunch::Program { program, args }
            }
        }
    }

    fn restore_plugin_terminal_focus(
        &mut self,
        tab_id: Option<TabId>,
        pane_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(tab_id) = tab_id
            && let Some(index) = self.session.tabs.iter().position(|tab| tab.id == tab_id)
        {
            self.switch_tab(index, cx);
        }
        if let Some(pane_id) = pane_id {
            self.focus_pane_target(&pane_id, cx);
        }
    }
}

pub(super) fn plugin_icon_path(icon: PluginIcon) -> &'static str {
    match icon {
        PluginIcon::Command => "icons/command_palette/command.svg",
        PluginIcon::Play => "icons/command_palette/play.svg",
        PluginIcon::Terminal => "icons/settings/terminal.svg",
        PluginIcon::Folder => "icons/command_palette/folder.svg",
        PluginIcon::Link => "icons/command_palette/link.svg",
        PluginIcon::Clipboard => "icons/command_palette/clipboard.svg",
        PluginIcon::Settings => "icons/settings/advanced.svg",
        PluginIcon::Info => "icons/command_palette/info.svg",
    }
}

#[cfg(test)]
#[path = "plugins_tests.rs"]
mod tests;
