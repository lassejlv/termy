use super::*;
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};
use termy_plugin_runtime::{
    PluginAction, PluginCommand, PluginCommandPlacement, PluginContext, PluginEvent,
    PluginEventDispatch, PluginIcon, PluginInput, PluginPaneContext, PluginPaneKind,
    PluginRuntimeKind, PluginTabContext, PluginToastLevel,
};

const PLUGIN_SELECTED_TEXT_MAX_BYTES: usize = 64 * 1024;
const MAX_PENDING_PLUGIN_EVENTS: usize = 64;

struct PendingPluginEvent {
    event: PluginEvent,
    context: PluginContext,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct PluginLifecycleSnapshot {
    active_tab_id: Option<TabId>,
    active_tab_index: Option<usize>,
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
        if next.active_tab_id != self.snapshot.active_tab_id {
            let previous_tab_index = self.snapshot.active_tab_index;
            self.command_started_at = next.active_command.as_ref().map(|_| now);
            self.snapshot = next;
            return vec![PluginEvent::TabActivated { previous_tab_index }];
        }

        let mut events = Vec::new();
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
            self.command_started_at = next.active_command.as_ref().map(|_| now);
        }
        self.snapshot = next;
        events
    }
}

pub(in crate::terminal_view) struct PluginLifecycleState {
    window_handle: gpui::AnyWindowHandle,
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
}

impl PluginInputSession {
    fn new(command: PluginCommand, revision: String) -> Self {
        Self {
            command,
            revision,
            input_index: 0,
            values: BTreeMap::new(),
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
            Some(PluginInput::Select { .. } | PluginInput::Confirm { .. }) | None => String::new(),
        }
    }

    fn move_back(&mut self) -> Option<String> {
        if self.input_index == 0 {
            return None;
        }
        self.input_index -= 1;
        self.current_input()?;
        Some(self.input_prefill())
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
        let active_tab = self.tabs.get(self.active_tab).map(|tab| PluginTabContext {
            index: self.active_tab,
            title: tab.title.clone(),
            pane_count: tab.panes.len(),
        });
        let active_pane = self.tabs.get(self.active_tab).and_then(|tab| {
            let index = tab.active_pane_index()?;
            let pane = tab.panes.get(index)?;
            Some(PluginPaneContext {
                index,
                kind: if pane.is_browser() {
                    PluginPaneKind::Browser
                } else {
                    PluginPaneKind::Terminal
                },
            })
        });
        PluginContext {
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
        let active_tab = self.tabs.get(self.active_tab);
        PluginLifecycleSnapshot {
            active_tab_id: active_tab.map(|tab| tab.id),
            active_tab_index: active_tab.map(|_| self.active_tab),
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
                termy_toast::warning("Plugin events are falling behind");
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
            cx.update(|cx| {
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
            termy_toast::error(format!("Plugin event failed: {message}"));
        }
        if let Err(error) = self.apply_plugin_actions(dispatch.actions, window, cx) {
            log::error!("Plugin event action failed: {error}");
            termy_toast::error(error);
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
            termy_toast::error(format!("Plugin error: {error}"));
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

    pub(super) fn command_palette_plugin_items(&self) -> Vec<CommandPaletteItem> {
        self.plugin_commands_for_placement(PluginCommandPlacement::CommandPalette)
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
        &self,
        placement: PluginCommandPlacement,
    ) -> Vec<PluginCommand> {
        placed_plugin_commands(self.plugin_runtime.commands(), placement)
    }

    pub(super) fn plugin_input_uses_free_text(&self) -> bool {
        matches!(
            self.command_palette
                .plugin_input_session
                .as_ref()
                .and_then(PluginInputSession::current_input),
            Some(PluginInput::Text { .. })
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
            termy_toast::error(format!(
                "Plugin command {plugin_id}.{command_id} is unavailable"
            ));
            self.notify_overlay(cx);
            return;
        };
        if let Some(reason) = command.disabled_reason.as_deref() {
            termy_toast::info(reason);
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
            termy_toast::error("Plugin keybinding lost its Termy window");
            self.notify_overlay(cx);
            return;
        };
        let runtime = self.plugin_runtime.clone();
        let plugin_id = plugin_id.to_string();
        let command_id = command_id.to_string();
        cx.spawn(async move |_this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let refresh = smol::unblock(move || runtime.refresh_if_changed()).await;
            cx.update(|cx| {
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
                    termy_toast::info(format!("{} is required", input.label()));
                    self.notify_overlay(cx);
                    return;
                }
                if text.chars().count() > *max_length {
                    termy_toast::info(format!(
                        "{} must be at most {max_length} characters",
                        input.label()
                    ));
                    self.notify_overlay(cx);
                    return;
                }
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
            termy_toast::error("Plugin command lost its Termy window");
            self.notify_overlay(cx);
            return;
        };
        let loading_id = termy_toast::loading(format!("Running {title}…"));
        self.close_command_palette(cx);
        self.notify_overlay(cx);

        cx.spawn(async move |_this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result = smol::unblock(move || {
                runtime.invoke(&plugin_id, &command_id, &revision, inputs, context)
            })
            .await;
            cx.update(|cx| {
                termy_toast::dismiss_toast(loading_id);
                let _ = window_handle.update(cx, |view, window, cx| {
                    match result {
                        Ok(actions) => {
                            if let Err(error) = view.apply_plugin_actions(actions, window, cx) {
                                log::error!("Plugin action failed: {error}");
                                termy_toast::error(error);
                            }
                        }
                        Err(error) => {
                            log::error!("Plugin command failed: {error}");
                            termy_toast::error(error);
                        }
                    }
                    view.notify_overlay(cx);
                });
            });
        })
        .detach();
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
                    mut command,
                    working_directory,
                } => {
                    if !command.ends_with('\n') {
                        command.push('\n');
                    }
                    if !self.add_tab_with_working_dir(working_directory.as_deref(), cx) {
                        return Err(
                            "Plugin command stopped because its terminal could not be created"
                                .to_string(),
                        );
                    }
                    let terminal = self
                        .tabs
                        .get(self.active_tab)
                        .and_then(TerminalTab::active_terminal)
                        .ok_or_else(|| {
                            "Plugin command stopped because the new terminal is unavailable"
                                .to_string()
                        })?;
                    terminal.write_input(command.as_bytes());
                    cx.notify();
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
                    PluginToastLevel::Info => termy_toast::info(message),
                    PluginToastLevel::Success => termy_toast::success(message),
                    PluginToastLevel::Warning => termy_toast::warning(message),
                    PluginToastLevel::Error => termy_toast::error(message),
                },
                PluginAction::ViewOpen {
                    view,
                    target,
                    plugin_id,
                    revision,
                } => {
                    self.open_plugin_ui(&plugin_id, &view, &revision, target, window, cx)?;
                }
            }
        }
        Ok(())
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
mod tests {
    use super::*;
    use termy_plugin_runtime::PluginSelectOption;

    fn lifecycle_snapshot(
        tab_id: u64,
        tab_index: usize,
        working_directory: &str,
        active_command: Option<&str>,
    ) -> PluginLifecycleSnapshot {
        PluginLifecycleSnapshot {
            active_tab_id: Some(tab_id),
            active_tab_index: Some(tab_index),
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
            vec![PluginEvent::TabActivated {
                previous_tab_index: Some(2),
            }]
        );
        assert_eq!(tracker.snapshot.active_tab_id, Some(9));
        assert_eq!(tracker.command_started_at, Some(now));
    }
}
