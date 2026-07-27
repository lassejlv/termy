use super::{
    TerminalView,
    command_palette::{CommandPaletteMode, style::CommandPaletteStyle},
};
use crate::commands;
use crate::text_input::{TextInputAlignment, TextInputElement, TextInputProvider, TextInputState};
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, AnyWindowHandle, App, AppContext, AsyncApp, ClipboardItem, Context, FocusHandle,
    Focusable, Font, FontWeight, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, ParentElement, Render, Rgba, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, WeakEntity, Window, div, px,
};
use std::{borrow::Cow, collections::BTreeMap};
use termy_plugin_runtime::{
    PluginContext, PluginRuntime, PluginUiAlignment, PluginUiButtonVariant, PluginUiGap,
    PluginUiNode, PluginUiTextVariant, PluginUiTone, PluginViewAction, PluginViewDescriptor,
    PluginViewRender, PluginViewTarget, PluginViewValue,
};

const PANEL_WIDTH: f32 = 620.0;
const PANEL_MAX_HEIGHT: f32 = 640.0;
const PANEL_MIN_WIDTH: f32 = 280.0;
const PANEL_MIN_HEIGHT: f32 = 180.0;
const MODAL_MARGIN: f32 = 16.0;
const PANEL_RADIUS: f32 = 12.0;
const CONTROL_RADIUS: f32 = 7.0;
const INPUT_HEIGHT: f32 = 36.0;
const SCRIM_ALPHA: f32 = 0.42;

struct ActiveInput {
    id: String,
    state: TextInputState,
    selecting: bool,
}

#[derive(Clone, Copy)]
struct PluginUiStyle {
    panel_bg: Rgba,
    panel_border: Rgba,
    primary_text: Rgba,
    muted_text: Rgba,
    input_selection: Rgba,
    control_bg: Rgba,
    control_hover: Rgba,
    accent: Rgba,
    accent_text: Rgba,
    success: Rgba,
    danger: Rgba,
}

pub(super) struct PluginUiView {
    parent: WeakEntity<TerminalView>,
    window_handle: AnyWindowHandle,
    runtime: PluginRuntime,
    descriptor: PluginViewDescriptor,
    revision: String,
    target: PluginViewTarget,
    nodes: Vec<PluginUiNode>,
    values: BTreeMap<String, PluginViewValue>,
    active_input: Option<ActiveInput>,
    focus_handle: FocusHandle,
    scroll_handle: ScrollHandle,
    loading: bool,
    busy: bool,
    error: Option<String>,
}

impl PluginUiView {
    fn new(
        parent: WeakEntity<TerminalView>,
        window_handle: AnyWindowHandle,
        runtime: PluginRuntime,
        descriptor: PluginViewDescriptor,
        revision: String,
        target: PluginViewTarget,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            parent,
            window_handle,
            runtime,
            descriptor,
            revision,
            target,
            nodes: Vec::new(),
            values: BTreeMap::new(),
            active_input: None,
            focus_handle: cx.focus_handle(),
            scroll_handle: ScrollHandle::new(),
            loading: true,
            busy: false,
            error: None,
        }
    }

    fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
    }

    pub(in crate::terminal_view) fn target(&self) -> PluginViewTarget {
        self.target
    }

    pub(in crate::terminal_view) fn title(&self) -> &str {
        &self.descriptor.title
    }

    fn current_context(
        &self,
        cx: &mut Context<Self>,
    ) -> Result<termy_plugin_runtime::PluginContext, String> {
        self.parent
            .update(cx, |view, cx| view.plugin_context(cx))
            .map_err(|_| "Plugin view lost its terminal window".to_string())
    }

    pub(super) fn load(&mut self, context: PluginContext, cx: &mut Context<Self>) {
        self.loading = true;
        self.error = None;
        let runtime = self.runtime.clone();
        let plugin_id = self.descriptor.plugin_id.clone();
        let view_id = self.descriptor.id.clone();
        let revision = self.revision.clone();
        let window_handle = self.window_handle;

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result = smol::unblock(move || {
                runtime.render_view(&plugin_id, &view_id, &revision, context)
            })
            .await;
            Self::finish_request(this, window_handle, result, cx);
        })
        .detach();
    }

    fn finish_request(
        this: WeakEntity<Self>,
        window_handle: AnyWindowHandle,
        result: Result<PluginViewRender, String>,
        cx: &mut AsyncApp,
    ) {
        cx.update(|cx| {
            let actions = this
                .update(cx, |view, cx| view.apply_render_result(result, cx))
                .ok()
                .flatten();
            let Some(actions) = actions else {
                return;
            };
            let Some(window_handle) = window_handle.downcast::<TerminalView>() else {
                return;
            };
            let _ = window_handle.update(cx, |view, window, cx| {
                if let Err(error) = view.apply_plugin_actions(actions, window, cx) {
                    log::error!("Plugin view action failed: {error}");
                    termy_toast::error(error);
                    view.notify_overlay(cx);
                }
            });
        });
    }

    fn apply_render_result(
        &mut self,
        result: Result<PluginViewRender, String>,
        cx: &mut Context<Self>,
    ) -> Option<Vec<termy_plugin_runtime::PluginAction>> {
        self.loading = false;
        self.busy = false;
        match result {
            Ok(render) => {
                if let Err(error) = self.validate_render_origin(&render) {
                    self.error = Some(error);
                    cx.notify();
                    return None;
                }
                self.error = None;
                self.set_document(render.nodes);
                cx.notify();
                Some(render.actions)
            }
            Err(error) => {
                log::error!("Plugin view failed: {error}");
                self.error = Some(error);
                cx.notify();
                None
            }
        }
    }

    fn validate_render_origin(&self, render: &PluginViewRender) -> Result<(), String> {
        if render.plugin_id != self.descriptor.plugin_id || render.revision != self.revision {
            return Err("Plugin view returned a response from the wrong revision".to_string());
        }
        let (_, current_revision) = self
            .runtime
            .view_with_revision(&self.descriptor.plugin_id, &self.descriptor.id)
            .ok_or_else(|| "Plugin view is no longer available".to_string())?;
        if current_revision != self.revision {
            return Err("Plugin changed while its view was running; reopen the view".to_string());
        }
        Ok(())
    }

    fn set_document(&mut self, nodes: Vec<PluginUiNode>) {
        let mut values = BTreeMap::new();
        Self::collect_values(&nodes, &mut values);
        self.nodes = nodes;
        self.values = values;
        self.active_input = None;
    }

    fn collect_values(nodes: &[PluginUiNode], values: &mut BTreeMap<String, PluginViewValue>) {
        for node in nodes {
            match node {
                PluginUiNode::TextInput { id, value, .. } => {
                    values.insert(id.clone(), PluginViewValue::Text(value.clone()));
                }
                PluginUiNode::Checkbox { id, checked, .. } => {
                    values.insert(id.clone(), PluginViewValue::Toggle(*checked));
                }
                _ => Self::collect_values(node.children(), values),
            }
        }
    }

    fn commit_active_input(&mut self) {
        if let Some(input) = self.active_input.as_ref() {
            self.values.insert(
                input.id.clone(),
                PluginViewValue::Text(input.state.text().to_string()),
            );
        }
    }

    fn activate_input(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if self
            .active_input
            .as_ref()
            .is_some_and(|input| input.id == id)
        {
            self.focus_handle.focus(window, cx);
            return;
        }
        self.commit_active_input();
        let value = self
            .values
            .get(id)
            .and_then(|value| match value {
                PluginViewValue::Text(value) => Some(value.clone()),
                PluginViewValue::Toggle(_) => None,
            })
            .unwrap_or_default();
        self.active_input = Some(ActiveInput {
            id: id.to_string(),
            state: TextInputState::new(value),
            selecting: false,
        });
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    fn active_input_limit(&self) -> usize {
        self.active_input
            .as_ref()
            .and_then(|input| Self::input_limit(&self.nodes, &input.id))
            .unwrap_or(4_096)
            .min(4_096)
    }

    fn bounded_input_text(text: &str, max_length: usize) -> Cow<'_, str> {
        for (count, character) in text.chars().enumerate() {
            if matches!(character, '\n' | '\r') || count == max_length {
                return Cow::Owned(
                    text.chars()
                        .filter(|character| !matches!(character, '\n' | '\r'))
                        .take(max_length)
                        .collect(),
                );
            }
        }
        Cow::Borrowed(text)
    }

    fn enforce_active_input_limit(&mut self) {
        let max_length = self.active_input_limit();
        let Some(input) = self.active_input.as_mut() else {
            return;
        };
        let bounded = Self::bounded_input_text(input.state.text(), max_length);
        if bounded.as_ref() != input.state.text() {
            input.state.set_text(bounded.into_owned());
        }
    }

    fn paste_active_input(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let text = Self::bounded_input_text(&text, self.active_input_limit());
        if let Some(input) = self.active_input.as_mut() {
            input.state.replace_text_in_range(None, text.as_ref());
            self.enforce_active_input_limit();
            cx.notify();
        }
    }

    fn copy_active_input(&self, cx: &mut Context<Self>) {
        let Some(input) = self.active_input.as_ref() else {
            return;
        };
        let range = input.state.selected_range();
        if !range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                input.state.text()[range].to_string(),
            ));
        }
    }

    fn select_all_active_input(&mut self, cx: &mut Context<Self>) {
        if let Some(input) = self.active_input.as_mut() {
            input.state.select_all();
            cx.notify();
        }
    }

    fn handle_input_mouse_down(
        &mut self,
        id: &str,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.activate_input(id, window, cx);
        let Some(input) = self.active_input.as_mut().filter(|input| input.id == id) else {
            return;
        };
        let index = input.state.character_index_for_point(event.position);
        if event.modifiers.shift {
            input.state.select_to_utf16(index);
        } else if event.click_count >= 3 {
            input.state.select_all();
        } else if event.click_count == 2 {
            input.state.select_token_at_utf16(index);
        } else {
            input.state.set_cursor_utf16(index);
        }
        input.selecting = event.click_count == 1;
        cx.stop_propagation();
        cx.notify();
    }

    fn handle_input_mouse_move(
        &mut self,
        id: &str,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = self
            .active_input
            .as_mut()
            .filter(|input| input.id == id && input.selecting)
        else {
            return;
        };
        if !event.dragging() {
            return;
        }
        let index = input.state.character_index_for_point(event.position);
        input.state.select_to_utf16(index);
        cx.notify();
    }

    fn handle_input_mouse_up(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(input) = self.active_input.as_mut().filter(|input| input.id == id) {
            input.selecting = false;
            cx.notify();
        }
    }

    fn input_limit(nodes: &[PluginUiNode], id: &str) -> Option<usize> {
        for node in nodes {
            if let PluginUiNode::TextInput {
                id: input_id,
                max_length,
                ..
            } = node
                && input_id == id
            {
                return Some(*max_length);
            }
            if let Some(limit) = Self::input_limit(node.children(), id) {
                return Some(limit);
            }
        }
        None
    }

    fn active_submit(nodes: &[PluginUiNode], id: &str) -> Option<String> {
        for node in nodes {
            if let PluginUiNode::TextInput {
                id: input_id,
                submit,
                disabled,
                ..
            } = node
                && input_id == id
                && !disabled
            {
                return submit.clone();
            }
            if let Some(action) = Self::active_submit(node.children(), id) {
                return Some(action);
            }
        }
        None
    }

    fn validate_values(&self) -> Result<(), String> {
        for (id, value) in &self.values {
            let PluginViewValue::Text(value) = value else {
                continue;
            };
            let Some(limit) = Self::input_limit(&self.nodes, id) else {
                continue;
            };
            if value.chars().count() > limit {
                return Err(format!("Input `{id}` must be at most {limit} characters"));
            }
        }
        Ok(())
    }

    fn dispatch(
        &mut self,
        id: String,
        control_id: String,
        payload: Option<String>,
        value: Option<PluginViewValue>,
        cx: &mut Context<Self>,
    ) {
        if self.busy || self.loading {
            return;
        }
        self.commit_active_input();
        if let Some(value) = value.as_ref() {
            self.values.insert(control_id.clone(), value.clone());
        }
        self.active_input = None;
        if let Err(error) = self.validate_values() {
            self.error = Some(error);
            cx.notify();
            return;
        }
        let context = match self.current_context(cx) {
            Ok(context) => context,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        self.busy = true;
        self.error = None;
        cx.notify();

        let runtime = self.runtime.clone();
        let plugin_id = self.descriptor.plugin_id.clone();
        let view_id = self.descriptor.id.clone();
        let revision = self.revision.clone();
        let values = self.values.clone();
        let action = PluginViewAction {
            id,
            control_id,
            payload,
            value,
        };
        let window_handle = self.window_handle;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result = smol::unblock(move || {
                runtime.invoke_view_action(&plugin_id, &view_id, &revision, action, values, context)
            })
            .await;
            Self::finish_request(this, window_handle, result, cx);
        })
        .detach();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        let Some(window_handle) = self.window_handle.downcast::<TerminalView>() else {
            return;
        };
        cx.defer(move |cx| {
            let _ = window_handle.update(cx, |view, window, cx| {
                view.close_plugin_ui(window, cx);
            });
        });
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let modifiers = event.keystroke.modifiers;
        let plain = !modifiers.modified();
        if plain && event.keystroke.key.eq_ignore_ascii_case("escape") {
            cx.stop_propagation();
            self.close(cx);
            return;
        }
        let Some(active_id) = self.active_input.as_ref().map(|input| input.id.clone()) else {
            return;
        };
        let secondary =
            modifiers.secondary() && !modifiers.alt && !modifiers.function && !modifiers.shift;
        let shift_only = modifiers.shift
            && !modifiers.control
            && !modifiers.alt
            && !modifiers.function
            && !modifiers.platform;
        let alt_only = modifiers.alt
            && !modifiers.control
            && !modifiers.shift
            && !modifiers.function
            && !modifiers.platform;
        if secondary && event.keystroke.key.eq_ignore_ascii_case("a") {
            cx.stop_propagation();
            self.select_all_active_input(cx);
            return;
        }
        if secondary && event.keystroke.key.eq_ignore_ascii_case("v") {
            cx.stop_propagation();
            self.paste_active_input(cx);
            return;
        }
        if secondary && event.keystroke.key.eq_ignore_ascii_case("c") {
            cx.stop_propagation();
            self.copy_active_input(cx);
            return;
        }
        let handled = match event.keystroke.key.as_str() {
            "enter" if plain => {
                if let Some(action) = Self::active_submit(&self.nodes, &active_id) {
                    let value = self
                        .active_input
                        .as_ref()
                        .map(|input| PluginViewValue::Text(input.state.text().to_string()));
                    self.dispatch(action, active_id.clone(), None, value, cx);
                }
                true
            }
            "backspace" if plain => {
                if let Some(input) = self.active_input.as_mut() {
                    input.state.delete_backward();
                }
                true
            }
            "delete" if plain => {
                if let Some(input) = self.active_input.as_mut() {
                    input.state.delete_forward();
                }
                true
            }
            "backspace" if alt_only => {
                if let Some(input) = self.active_input.as_mut() {
                    input.state.delete_word_backward();
                }
                true
            }
            "delete" if alt_only => {
                if let Some(input) = self.active_input.as_mut() {
                    input.state.delete_word_forward();
                }
                true
            }
            "backspace" if secondary => {
                if let Some(input) = self.active_input.as_mut() {
                    input.state.delete_to_start();
                }
                true
            }
            "delete" if secondary => {
                if let Some(input) = self.active_input.as_mut() {
                    input.state.delete_to_end();
                }
                true
            }
            "left" if plain => {
                if let Some(input) = self.active_input.as_mut() {
                    input.state.move_left();
                }
                true
            }
            "right" if plain => {
                if let Some(input) = self.active_input.as_mut() {
                    input.state.move_right();
                }
                true
            }
            "left" if shift_only => {
                if let Some(input) = self.active_input.as_mut() {
                    input.state.select_left();
                }
                true
            }
            "right" if shift_only => {
                if let Some(input) = self.active_input.as_mut() {
                    input.state.select_right();
                }
                true
            }
            "left" if secondary => {
                if let Some(input) = self.active_input.as_mut() {
                    input.state.move_to_start();
                }
                true
            }
            "right" if secondary => {
                if let Some(input) = self.active_input.as_mut() {
                    input.state.move_to_end();
                }
                true
            }
            "home" if plain => {
                if let Some(input) = self.active_input.as_mut() {
                    input.state.move_to_start();
                }
                true
            }
            "end" if plain => {
                if let Some(input) = self.active_input.as_mut() {
                    input.state.move_to_end();
                }
                true
            }
            _ => false,
        };
        if handled {
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn handle_copy_action(
        &mut self,
        _: &commands::Copy,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.copy_active_input(cx);
    }

    fn handle_paste_action(
        &mut self,
        _: &commands::Paste,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.paste_active_input(cx);
    }

    fn handle_select_all_action(
        &mut self,
        _: &commands::SelectAll,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_all_active_input(cx);
    }

    fn style(&self, cx: &App) -> Option<(PluginUiStyle, SharedString, SharedString, f32)> {
        let parent = self.parent.upgrade()?;
        let parent = parent.read(cx);
        let palette = CommandPaletteStyle::resolve(parent);
        let overlay = parent.overlay_style();
        let mut accent = parent.colors.cursor;
        accent.a = 0.92;
        let mut accent_text = parent.colors.background;
        accent_text.a = 1.0;
        Some((
            PluginUiStyle {
                panel_bg: palette.panel_bg,
                panel_border: palette.panel_border,
                primary_text: palette.primary_text,
                muted_text: palette.muted_text,
                input_selection: palette.input_selection,
                control_bg: overlay.chrome_panel_neutral(0.1),
                control_hover: overlay.chrome_panel_cursor(0.16),
                accent,
                accent_text,
                success: Rgba {
                    r: 0.24,
                    g: 0.78,
                    b: 0.48,
                    a: 1.0,
                },
                danger: Rgba {
                    r: 0.93,
                    g: 0.3,
                    b: 0.32,
                    a: 1.0,
                },
            },
            parent.ui_font_family.clone(),
            parent.font_family.clone(),
            parent.terminal_content_top_inset(),
        ))
    }

    fn panel_dimensions(available_width: f32, available_height: f32) -> (f32, f32) {
        let width = (available_width - MODAL_MARGIN * 2.0).max(1.0);
        let height = (available_height - MODAL_MARGIN * 2.0).max(1.0);
        (
            PANEL_WIDTH.min(width).max(PANEL_MIN_WIDTH.min(width)),
            PANEL_MAX_HEIGHT
                .min(height)
                .max(PANEL_MIN_HEIGHT.min(height)),
        )
    }

    fn gap(gap: PluginUiGap) -> f32 {
        match gap {
            PluginUiGap::None => 0.0,
            PluginUiGap::Small => 8.0,
            PluginUiGap::Medium => 12.0,
            PluginUiGap::Large => 20.0,
        }
    }

    fn align(element: gpui::Div, alignment: PluginUiAlignment) -> gpui::Div {
        match alignment {
            PluginUiAlignment::Start => element.items_start(),
            PluginUiAlignment::Center => element.items_center(),
            PluginUiAlignment::End => element.items_end(),
            PluginUiAlignment::Stretch => element,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_node(
        &self,
        node: &PluginUiNode,
        path: &str,
        style: PluginUiStyle,
        ui_font: &SharedString,
        terminal_font: &SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match node {
            PluginUiNode::Column {
                gap,
                align,
                children,
            } => {
                let children = children
                    .iter()
                    .enumerate()
                    .map(|(index, child)| {
                        self.render_node(
                            child,
                            &format!("{path}-{index}"),
                            style,
                            ui_font,
                            terminal_font,
                            cx,
                        )
                    })
                    .collect::<Vec<_>>();
                Self::align(
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap(px(Self::gap(*gap)))
                        .children(children),
                    *align,
                )
                .into_any_element()
            }
            PluginUiNode::Row {
                gap,
                align,
                children,
            } => {
                let children = children
                    .iter()
                    .enumerate()
                    .map(|(index, child)| {
                        self.render_node(
                            child,
                            &format!("{path}-{index}"),
                            style,
                            ui_font,
                            terminal_font,
                            cx,
                        )
                    })
                    .collect::<Vec<_>>();
                Self::align(
                    div()
                        .w_full()
                        .flex()
                        .gap(px(Self::gap(*gap)))
                        .children(children),
                    *align,
                )
                .into_any_element()
            }
            PluginUiNode::Text {
                text,
                variant,
                tone,
            } => {
                let color = match tone {
                    PluginUiTone::Default => style.primary_text,
                    PluginUiTone::Muted => style.muted_text,
                    PluginUiTone::Success => style.success,
                    PluginUiTone::Danger => style.danger,
                };
                let mut element = div()
                    .w_full()
                    .text_color(color)
                    .font_family(ui_font.clone());
                element = match variant {
                    PluginUiTextVariant::Heading => element
                        .text_size(px(18.0))
                        .font_weight(FontWeight::SEMIBOLD),
                    PluginUiTextVariant::Body => element.text_size(px(13.0)),
                    PluginUiTextVariant::Caption => element.text_size(px(11.0)),
                    PluginUiTextVariant::Code => element
                        .font_family(terminal_font.clone())
                        .text_size(px(12.0))
                        .px(px(8.0))
                        .py(px(6.0))
                        .rounded(px(CONTROL_RADIUS))
                        .bg(style.control_bg),
                };
                element.child(text.clone()).into_any_element()
            }
            PluginUiNode::TextInput {
                id,
                label,
                placeholder,
                disabled,
                ..
            } => {
                let active = self
                    .active_input
                    .as_ref()
                    .is_some_and(|input| input.id == *id);
                let value = if active {
                    self.active_input
                        .as_ref()
                        .map(|input| input.state.text().to_string())
                        .unwrap_or_default()
                } else {
                    self.values
                        .get(id)
                        .and_then(|value| match value {
                            PluginViewValue::Text(value) => Some(value.clone()),
                            PluginViewValue::Toggle(_) => None,
                        })
                        .unwrap_or_default()
                };
                let entity = cx.entity();
                let focus_handle = self.focus_handle.clone();
                let mouse_down_id = id.clone();
                let mouse_move_id = id.clone();
                let mouse_up_id = id.clone();
                let mouse_up_out_id = id.clone();
                let can_edit = !*disabled && !self.busy;
                let field = div()
                    .id(SharedString::from(format!(
                        "plugin-ui-{}-{}-{path}",
                        self.descriptor.plugin_id, self.descriptor.id
                    )))
                    .w_full()
                    .min_w(px(120.0))
                    .h(px(INPUT_HEIGHT))
                    .relative()
                    .flex()
                    .items_center()
                    .px(px(10.0))
                    .rounded(px(CONTROL_RADIUS))
                    .border_1()
                    .border_color(if active {
                        style.accent
                    } else {
                        style.panel_border
                    })
                    .bg(style.control_bg)
                    .when(can_edit, |element| {
                        element
                            .cursor_text()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                                    view.handle_input_mouse_down(&mouse_down_id, event, window, cx);
                                }),
                            )
                            .on_mouse_move(cx.listener(
                                move |view, event: &MouseMoveEvent, _window, cx| {
                                    view.handle_input_mouse_move(&mouse_move_id, event, cx);
                                },
                            ))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(move |view, _, _, cx| {
                                    view.handle_input_mouse_up(&mouse_up_id, cx);
                                }),
                            )
                            .on_mouse_up_out(
                                MouseButton::Left,
                                cx.listener(move |view, _, _, cx| {
                                    view.handle_input_mouse_up(&mouse_up_out_id, cx);
                                }),
                            )
                    })
                    .children((value.is_empty()).then(|| {
                        div()
                            .absolute()
                            .left(px(10.0))
                            .right(px(10.0))
                            .text_size(px(13.0))
                            .text_color(style.muted_text)
                            .child(placeholder.clone().unwrap_or_default())
                    }))
                    .child(
                        div()
                            .w_full()
                            .h(px(22.0))
                            .overflow_hidden()
                            .when(active, |element| {
                                element.child(TextInputElement::new(
                                    entity,
                                    focus_handle,
                                    Font {
                                        family: ui_font.clone(),
                                        ..Font::default()
                                    },
                                    px(13.0),
                                    style.primary_text.into(),
                                    style.input_selection.into(),
                                    TextInputAlignment::Left,
                                ))
                            })
                            .when(!active && !value.is_empty(), |element| {
                                element.child(
                                    div()
                                        .truncate()
                                        .text_size(px(13.0))
                                        .text_color(style.primary_text)
                                        .child(value),
                                )
                            }),
                    );
                div()
                    .flex_1()
                    .min_w(px(120.0))
                    .flex()
                    .flex_col()
                    .gap(px(5.0))
                    .children(label.as_ref().map(|label| {
                        div()
                            .text_size(px(11.0))
                            .text_color(style.muted_text)
                            .child(label.clone())
                    }))
                    .child(field)
                    .into_any_element()
            }
            PluginUiNode::Button {
                id,
                action,
                label,
                payload,
                variant,
                disabled,
            } => {
                let enabled = !*disabled && !self.busy;
                let (background, foreground) = match variant {
                    PluginUiButtonVariant::Secondary => (style.control_bg, style.primary_text),
                    PluginUiButtonVariant::Primary => (style.accent, style.accent_text),
                    PluginUiButtonVariant::Danger => (style.danger, style.accent_text),
                };
                let action_id = action.clone();
                let control_id = id.clone();
                let action_payload = payload.clone();
                div()
                    .id(SharedString::from(format!(
                        "plugin-ui-{}-{}-{path}",
                        self.descriptor.plugin_id, self.descriptor.id
                    )))
                    .flex_none()
                    .h(px(INPUT_HEIGHT))
                    .px(px(14.0))
                    .rounded(px(CONTROL_RADIUS))
                    .bg(background)
                    .text_size(px(12.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(foreground)
                    .flex()
                    .items_center()
                    .justify_center()
                    .opacity(if enabled { 1.0 } else { 0.55 })
                    .when(enabled, |element| {
                        element
                            .cursor_pointer()
                            .hover(move |element| element.bg(style.control_hover))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |view, _: &MouseDownEvent, _window, cx| {
                                    cx.stop_propagation();
                                    view.dispatch(
                                        action_id.clone(),
                                        control_id.clone(),
                                        action_payload.clone(),
                                        None,
                                        cx,
                                    );
                                }),
                            )
                    })
                    .child(label.clone())
                    .into_any_element()
            }
            PluginUiNode::Checkbox {
                id,
                action,
                label,
                payload,
                checked,
                disabled,
            } => {
                let enabled = !*disabled && !self.busy;
                let action_id = action.clone();
                let control_id = id.clone();
                let action_payload = payload.clone();
                let next_value = !*checked;
                div()
                    .id(SharedString::from(format!(
                        "plugin-ui-{}-{}-{path}",
                        self.descriptor.plugin_id, self.descriptor.id
                    )))
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .items_center()
                    .gap(px(9.0))
                    .opacity(if enabled { 1.0 } else { 0.55 })
                    .when(enabled, |element| {
                        element.cursor_pointer().on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |view, _: &MouseDownEvent, _window, cx| {
                                cx.stop_propagation();
                                view.dispatch(
                                    action_id.clone(),
                                    control_id.clone(),
                                    action_payload.clone(),
                                    Some(PluginViewValue::Toggle(next_value)),
                                    cx,
                                );
                            }),
                        )
                    })
                    .child(
                        div()
                            .flex_none()
                            .w(px(18.0))
                            .h(px(18.0))
                            .rounded(px(5.0))
                            .border_1()
                            .border_color(if *checked {
                                style.accent
                            } else {
                                style.panel_border
                            })
                            .bg(if *checked {
                                style.accent
                            } else {
                                style.control_bg
                            })
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(style.accent_text)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(if *checked { "✓" } else { "" }),
                    )
                    .child(
                        div()
                            .min_w(px(0.0))
                            .text_size(px(13.0))
                            .text_color(style.primary_text)
                            .child(label.clone()),
                    )
                    .into_any_element()
            }
            PluginUiNode::Divider => div()
                .w_full()
                .h(px(1.0))
                .bg(style.panel_border)
                .into_any_element(),
            PluginUiNode::Spacer { size } => div()
                .flex_none()
                .w(px(Self::gap(*size)))
                .h(px(Self::gap(*size)))
                .into_any_element(),
        }
    }
}

impl TextInputProvider for PluginUiView {
    fn text_input_state(&self) -> Option<&TextInputState> {
        (!self.busy)
            .then_some(self.active_input.as_ref())
            .flatten()
            .map(|input| &input.state)
    }

    fn text_input_state_mut(&mut self) -> Option<&mut TextInputState> {
        if self.busy {
            return None;
        }
        self.active_input.as_mut().map(|input| &mut input.state)
    }
}

impl gpui::EntityInputHandler for PluginUiView {
    fn text_for_range(
        &mut self,
        range: std::ops::Range<usize>,
        adjusted_range: &mut Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let state = self.text_input_state()?;
        Some(state.text_for_range(range, adjusted_range))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<gpui::UTF16Selection> {
        Some(self.text_input_state()?.selected_text_range())
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        self.text_input_state()?.marked_text_range_utf16()
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        if let Some(state) = self.text_input_state_mut() {
            state.unmark_text();
        }
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<std::ops::Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = Self::bounded_input_text(text, self.active_input_limit());
        if let Some(state) = self.text_input_state_mut() {
            state.replace_text_in_range(range, text.as_ref());
            self.enforce_active_input_limit();
            cx.notify();
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<std::ops::Range<usize>>,
        new_text: &str,
        new_selected_range: Option<std::ops::Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = Self::bounded_input_text(new_text, self.active_input_limit());
        let selected_range = (text.as_ref() == new_text)
            .then_some(new_selected_range)
            .flatten();
        if let Some(state) = self.text_input_state_mut() {
            state.replace_and_mark_text_in_range(range, text.as_ref(), selected_range);
            self.enforce_active_input_limit();
            cx.notify();
        }
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: std::ops::Range<usize>,
        element_bounds: gpui::Bounds<gpui::Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<gpui::Bounds<gpui::Pixels>> {
        Some(
            self.text_input_state()?
                .bounds_for_range(range_utf16, element_bounds),
        )
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<gpui::Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.text_input_state()?.character_index_for_point(point))
    }

    fn accepts_text_input(&self, _window: &mut Window, _cx: &mut Context<Self>) -> bool {
        self.text_input_state().is_some()
    }
}

impl Focusable for PluginUiView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PluginUiView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some((style, ui_font, terminal_font, chrome_height)) = self.style(cx) else {
            return div().size_full().into_any_element();
        };
        let viewport = window.viewport_size();
        let viewport_width: f32 = viewport.width.into();
        let viewport_height: f32 = viewport.height.into();
        let available_height = (viewport_height - chrome_height).max(1.0);
        let (panel_width, panel_max_height) =
            Self::panel_dimensions(viewport_width, available_height);
        let rendered_nodes = self
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                self.render_node(
                    node,
                    &index.to_string(),
                    style,
                    &ui_font,
                    &terminal_font,
                    cx,
                )
            })
            .collect::<Vec<_>>();
        let content = if self.loading {
            div()
                .w_full()
                .py(px(36.0))
                .text_center()
                .text_size(px(13.0))
                .text_color(style.muted_text)
                .child("Loading view…")
                .into_any_element()
        } else {
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .children(rendered_nodes)
                .into_any_element()
        };
        if self.target == PluginViewTarget::CommandPalette {
            return div()
                .id("plugin-ui-command-palette-content")
                .size_full()
                .min_h(px(0.0))
                .overflow_hidden()
                .track_focus(&self.focus_handle)
                .key_context("PluginUI")
                .on_action(cx.listener(Self::handle_copy_action))
                .on_action(cx.listener(Self::handle_paste_action))
                .on_action(cx.listener(Self::handle_select_all_action))
                .on_key_down(cx.listener(Self::handle_key_down))
                .child(
                    div()
                        .id("plugin-ui-content")
                        .size_full()
                        .min_h(px(0.0))
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll_handle)
                        .px(px(14.0))
                        .py(px(12.0))
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        .children(self.error.as_ref().map(|error| {
                            div()
                                .w_full()
                                .px(px(10.0))
                                .py(px(8.0))
                                .rounded(px(CONTROL_RADIUS))
                                .bg(style.control_bg)
                                .text_size(px(12.0))
                                .text_color(style.danger)
                                .child(error.clone())
                        }))
                        .child(content)
                        .children(self.busy.then(|| {
                            div()
                                .text_size(px(11.0))
                                .text_color(style.muted_text)
                                .child("Working…")
                        })),
                )
                .into_any_element();
        }
        let title = self.descriptor.title.clone();
        let close_button = div()
            .id("plugin-ui-close")
            .w(px(28.0))
            .h(px(28.0))
            .rounded(px(6.0))
            .text_size(px(18.0))
            .text_color(style.muted_text)
            .cursor_pointer()
            .hover(move |element| {
                element
                    .bg(style.control_hover)
                    .text_color(style.primary_text)
            })
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    view.close(cx);
                }),
            )
            .child("×");
        let panel = div()
            .id("plugin-ui-panel")
            .w(px(panel_width))
            .max_h(px(panel_max_height))
            .rounded(px(PANEL_RADIUS))
            .bg(style.panel_bg)
            .border_1()
            .border_color(style.panel_border)
            .shadow_lg()
            .overflow_hidden()
            .flex()
            .flex_col()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_view, _: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                }),
            )
            .child(
                div()
                    .w_full()
                    .h(px(52.0))
                    .px(px(18.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(style.panel_border)
                    .child(
                        div()
                            .min_w(px(0.0))
                            .truncate()
                            .text_size(px(14.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(style.primary_text)
                            .child(title),
                    )
                    .child(close_button),
            )
            .child(
                div()
                    .id("plugin-ui-content")
                    .w_full()
                    .max_h(px((panel_max_height - 52.0).max(1.0)))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll_handle)
                    .px(px(20.0))
                    .py(px(18.0))
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .children(self.error.as_ref().map(|error| {
                        div()
                            .w_full()
                            .px(px(10.0))
                            .py(px(8.0))
                            .rounded(px(CONTROL_RADIUS))
                            .bg(style.control_bg)
                            .text_size(px(12.0))
                            .text_color(style.danger)
                            .child(error.clone())
                    }))
                    .child(content)
                    .children(self.busy.then(|| {
                        div()
                            .text_size(px(11.0))
                            .text_color(style.muted_text)
                            .child("Working…")
                    })),
            );

        let scrim = Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: SCRIM_ALPHA,
        };
        div()
            .id("plugin-ui-modal")
            .size_full()
            .absolute()
            .top_0()
            .left_0()
            .occlude()
            .bg(scrim)
            .track_focus(&self.focus_handle)
            .key_context("PluginUI")
            .on_action(cx.listener(Self::handle_copy_action))
            .on_action(cx.listener(Self::handle_paste_action))
            .on_action(cx.listener(Self::handle_select_all_action))
            .on_key_down(cx.listener(Self::handle_key_down))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _: &MouseDownEvent, _window, cx| {
                    view.close(cx);
                    cx.stop_propagation();
                }),
            )
            .child(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p(px(MODAL_MARGIN))
                    .child(panel),
            )
            .into_any_element()
    }
}

impl TerminalView {
    pub(in crate::terminal_view) fn open_plugin_ui(
        &mut self,
        plugin_id: &str,
        view_id: &str,
        revision: &str,
        target: PluginViewTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let (descriptor, current_revision) = self
            .plugin_runtime
            .view_with_revision(plugin_id, view_id)
            .ok_or_else(|| format!("Plugin view {plugin_id}.{view_id} is unavailable"))?;
        if current_revision != revision {
            return Err("Plugin changed before its view could open; try again".to_string());
        }
        let context = self.plugin_context(cx);
        match target {
            PluginViewTarget::Modal => self.close_command_palette(cx),
            PluginViewTarget::CommandPalette => {
                self.open_command_palette_in_mode(CommandPaletteMode::Commands, cx);
                self.command_palette_input_mut().clear();
            }
        }
        self.close_search(cx);
        self.cancel_rename_tab(cx);
        self.cancel_rename_workspace(cx);
        self.cancel_browser_url_edit(cx);
        let _ = self.close_terminal_context_menu(cx);
        let _ = self.close_tab_context_menu(cx);
        let _ = self.close_new_tab_menu(cx);
        let parent = cx.entity().downgrade();
        let window_handle = window.window_handle();
        let runtime = self.plugin_runtime.clone();
        let revision = revision.to_string();
        let plugin_ui = cx.new(|cx| {
            PluginUiView::new(
                parent,
                window_handle,
                runtime,
                descriptor,
                revision,
                target,
                cx,
            )
        });
        self.plugin_ui = Some(plugin_ui.clone());
        plugin_ui.update(cx, |view, cx| {
            if target == PluginViewTarget::Modal {
                view.focus(window, cx);
            }
            view.load(context, cx);
        });
        if target == PluginViewTarget::CommandPalette {
            self.focus_handle.focus(window, cx);
        }
        cx.notify();
        self.notify_overlay(cx);
        Ok(())
    }

    pub(in crate::terminal_view) fn command_palette_plugin_ui(
        &self,
        cx: &App,
    ) -> Option<gpui::Entity<PluginUiView>> {
        self.plugin_ui
            .as_ref()
            .filter(|view| view.read(cx).target() == PluginViewTarget::CommandPalette)
            .cloned()
    }

    pub(in crate::terminal_view) fn modal_plugin_ui(
        &self,
        cx: &App,
    ) -> Option<gpui::Entity<PluginUiView>> {
        self.plugin_ui
            .as_ref()
            .filter(|view| view.read(cx).target() == PluginViewTarget::Modal)
            .cloned()
    }

    pub(in crate::terminal_view) fn dismiss_command_palette_plugin_ui(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.command_palette_plugin_ui(cx).is_none() {
            return false;
        }
        self.plugin_ui = None;
        cx.notify();
        self.notify_overlay(cx);
        true
    }

    fn close_plugin_ui(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.plugin_ui.take().is_none() {
            return;
        }
        self.focus_handle.focus(window, cx);
        cx.notify();
        self.notify_overlay(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_input_text_is_single_line_unicode_bounded() {
        assert_eq!(PluginUiView::bounded_input_text("a😀b", 2).as_ref(), "a😀");
        assert_eq!(
            PluginUiView::bounded_input_text("first\nsecond\r", 32).as_ref(),
            "firstsecond"
        );
    }

    #[test]
    fn plugin_panel_fits_inside_small_terminal_viewports() {
        let (width, height) = PluginUiView::panel_dimensions(480.0, 268.0);
        assert!(width + MODAL_MARGIN * 2.0 <= 480.0);
        assert!(height + MODAL_MARGIN * 2.0 <= 268.0);
        assert_eq!(width, 448.0);
        assert_eq!(height, 236.0);
    }
}
