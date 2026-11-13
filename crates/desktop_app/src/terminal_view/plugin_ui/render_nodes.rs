use super::*;

impl PluginUiView {
    fn gap(gap: PluginUiGap) -> f32 {
        match gap {
            PluginUiGap::None => 0.0,
            PluginUiGap::Small => 8.0,
            PluginUiGap::Medium => 12.0,
            PluginUiGap::Large => 20.0,
        }
    }

    fn align(element: gpui_kit::Div, alignment: PluginUiAlignment) -> gpui_kit::Div {
        match alignment {
            PluginUiAlignment::Start => element.items_start(),
            PluginUiAlignment::Center => element.items_center(),
            PluginUiAlignment::End => element.items_end(),
            PluginUiAlignment::Stretch => element,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_node(
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
                        .map_or_default(|input| input.state.text().to_string())
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
                                        ..gpui_kit::font("")
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
            PluginUiNode::TextArea {
                id,
                label,
                placeholder,
                rows,
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
                        .map_or_default(|input| input.state.text().to_string())
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
                let height = (*rows as f32 * 20.0 + 16.0).max(56.0);
                let field = div()
                    .id(SharedString::from(format!(
                        "plugin-ui-{}-{}-{path}",
                        self.descriptor.plugin_id, self.descriptor.id
                    )))
                    .w_full()
                    .min_w(px(120.0))
                    .h(px(height))
                    .relative()
                    .px(px(10.0))
                    .py(px(8.0))
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
                            .top(px(8.0))
                            .text_size(px(13.0))
                            .text_color(style.muted_text)
                            .child(placeholder.clone().unwrap_or_default())
                    }))
                    .when(active, |element| {
                        element.child(MultilineTextInputElement::new(
                            entity,
                            focus_handle,
                            Font {
                                family: ui_font.clone(),
                                ..gpui_kit::font("")
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
                                .flex()
                                .flex_col()
                                .text_size(px(13.0))
                                .text_color(style.primary_text)
                                .children(
                                    value.split('\n').map(|line| div().child(line.to_string())),
                                ),
                        )
                    });
                div()
                    .w_full()
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
            PluginUiNode::Select {
                id,
                label,
                placeholder,
                options,
                action,
                disabled,
                ..
            } => {
                let selected = self
                    .values
                    .get(id)
                    .and_then(|value| match value {
                        PluginViewValue::Text(value) => Some(value.as_str()),
                        PluginViewValue::Toggle(_) => None,
                    })
                    .unwrap_or_default();
                let selected_label = options
                    .iter()
                    .find(|option| option.value == selected)
                    .map(|option| option.label.clone())
                    .or_else(|| placeholder.clone())
                    .unwrap_or_else(|| "Select…".to_string());
                let enabled = !*disabled && !self.busy;
                let is_open = self.open_select.as_deref() == Some(id.as_str());
                let focused = self.focused_control.as_deref() == Some(id.as_str());
                let select_id = id.clone();
                let rows =
                    is_open.then(|| {
                        options
                            .iter()
                            .map(|option| {
                                let option_value = option.value.clone();
                                let option_label = option.label.clone();
                                let option_status = option.status.clone();
                                let control_id = id.clone();
                                let action_id = action.clone();
                                div()
                                    .w_full()
                                    .px(px(10.0))
                                    .py(px(7.0))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .text_size(px(12.0))
                                    .text_color(style.primary_text)
                                    .cursor_pointer()
                                    .hover(move |element| element.bg(style.control_hover))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |view, _event, _window, cx| {
                                            cx.stop_propagation();
                                            view.open_select = None;
                                            let value = PluginViewValue::Text(option_value.clone());
                                            if let Some(action_id) = action_id.clone() {
                                                view.dispatch(
                                                    action_id,
                                                    control_id.clone(),
                                                    None,
                                                    Some(value),
                                                    cx,
                                                );
                                            } else {
                                                view.focused_control = Some(control_id.clone());
                                                view.values.insert(control_id.clone(), value);
                                                cx.notify();
                                            }
                                        }),
                                    )
                                    .child(option_label)
                                    .children(option_status.map(|status| {
                                        div().text_color(style.muted_text).child(status)
                                    }))
                            })
                            .collect::<Vec<_>>()
                    });
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(5.0))
                    .children(label.as_ref().map(|label| {
                        div()
                            .text_size(px(11.0))
                            .text_color(style.muted_text)
                            .child(label.clone())
                    }))
                    .child(
                        div()
                            .w_full()
                            .h(px(INPUT_HEIGHT))
                            .px(px(10.0))
                            .rounded(px(CONTROL_RADIUS))
                            .border_1()
                            .border_color(if is_open || focused {
                                style.accent
                            } else {
                                style.panel_border
                            })
                            .bg(style.control_bg)
                            .flex()
                            .items_center()
                            .justify_between()
                            .opacity(if enabled { 1.0 } else { 0.55 })
                            .when(enabled, |element| {
                                element.cursor_pointer().on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |view, _event, _window, cx| {
                                        cx.stop_propagation();
                                        view.focused_control = Some(select_id.clone());
                                        view.open_select = (view.open_select.as_deref()
                                            != Some(select_id.as_str()))
                                        .then_some(select_id.clone());
                                        cx.notify();
                                    }),
                                )
                            })
                            .child(selected_label)
                            .child("⌄"),
                    )
                    .children(rows.map(|rows| {
                        div()
                            .w_full()
                            .rounded(px(CONTROL_RADIUS))
                            .border_1()
                            .border_color(style.panel_border)
                            .bg(style.control_bg)
                            .flex()
                            .flex_col()
                            .children(rows)
                    }))
                    .into_any_element()
            }
            PluginUiNode::List {
                id,
                action,
                selected_id: _,
                search_placeholder,
                filtering,
                is_loading,
                children,
            } => {
                let selected = self
                    .values
                    .get(id)
                    .and_then(|value| match value {
                        PluginViewValue::Text(value) => Some(value.as_str()),
                        PluginViewValue::Toggle(_) => None,
                    })
                    .unwrap_or_default();
                let query = self.list_queries.get(id).cloned().unwrap_or_default();
                let normalized_query = query.to_lowercase();
                let list_focused = self.focused_control.as_deref() == Some(id.as_str());
                let mut list = div().w_full().flex().flex_col().gap(px(4.0));
                if *filtering {
                    let list_id = id.clone();
                    list = list.child(
                        div()
                            .w_full()
                            .h(px(32.0))
                            .px(px(10.0))
                            .rounded(px(CONTROL_RADIUS))
                            .bg(style.control_bg)
                            .border_1()
                            .border_color(if list_focused {
                                style.accent
                            } else {
                                style.panel_border
                            })
                            .text_size(px(12.0))
                            .text_color(style.muted_text)
                            .flex()
                            .items_center()
                            .cursor_text()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |view, _event, window, cx| {
                                    cx.stop_propagation();
                                    view.commit_active_input();
                                    view.active_input = None;
                                    view.focused_control = Some(list_id.clone());
                                    view.focus_handle.focus(window, cx);
                                    cx.notify();
                                }),
                            )
                            .child(if query.is_empty() {
                                search_placeholder
                                    .clone()
                                    .unwrap_or_else(|| "Type to filter…".to_string())
                            } else {
                                query
                            }),
                    );
                }
                if *is_loading {
                    list = list.child(
                        div()
                            .w_full()
                            .py(px(12.0))
                            .text_size(px(12.0))
                            .text_color(style.muted_text)
                            .child("Loading…"),
                    );
                }
                let rows = children.iter().filter_map(|child| {
                    let PluginUiNode::ListItem {
                        id: item_id,
                        title,
                        subtitle,
                        keywords,
                        status,
                        payload,
                        action: item_action,
                        disabled,
                        ..
                    } = child
                    else {
                        return None;
                    };
                    if !normalized_query.is_empty()
                        && !title.to_lowercase().contains(&normalized_query)
                        && !subtitle
                            .as_ref()
                            .is_some_and(|value| value.to_lowercase().contains(&normalized_query))
                        && !keywords
                            .iter()
                            .any(|value| value.to_lowercase().contains(&normalized_query))
                    {
                        return None;
                    }
                    let enabled = !*disabled && !self.busy;
                    let selected = selected == item_id;
                    let list_id = id.clone();
                    let item_id_for_action = item_id.clone();
                    let action_id = item_action.clone().or_else(|| action.clone());
                    let action_payload = payload.clone();
                    Some(
                        div()
                            .w_full()
                            .px(px(10.0))
                            .py(px(8.0))
                            .rounded(px(CONTROL_RADIUS))
                            .bg(if selected {
                                style.control_hover
                            } else {
                                style.control_bg
                            })
                            .opacity(if enabled { 1.0 } else { 0.55 })
                            .flex()
                            .items_center()
                            .justify_between()
                            .when(enabled, |element| {
                                element.cursor_pointer().on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |view, _event, _window, cx| {
                                        cx.stop_propagation();
                                        let value =
                                            PluginViewValue::Text(item_id_for_action.clone());
                                        if let Some(action_id) = action_id.clone() {
                                            view.dispatch(
                                                action_id,
                                                list_id.clone(),
                                                action_payload.clone(),
                                                Some(value),
                                                cx,
                                            );
                                        } else {
                                            view.focused_control = Some(list_id.clone());
                                            view.values.insert(list_id.clone(), value);
                                            cx.notify();
                                        }
                                    }),
                                )
                            })
                            .child(
                                div()
                                    .min_w(px(0.0))
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_size(px(13.0))
                                            .text_color(style.primary_text)
                                            .child(title.clone()),
                                    )
                                    .children(subtitle.clone().map(|subtitle| {
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(style.muted_text)
                                            .child(subtitle)
                                    })),
                            )
                            .children(status.clone().map(|status| {
                                div()
                                    .text_size(px(11.0))
                                    .text_color(style.muted_text)
                                    .child(status)
                            })),
                    )
                });
                list.children(rows).into_any_element()
            }
            PluginUiNode::ListItem {
                id,
                title,
                subtitle,
                status,
                payload,
                action,
                disabled,
                ..
            } => {
                let enabled = !*disabled && !self.busy && action.is_some();
                let action_id = action.clone();
                let control_id = id.clone();
                let action_payload = payload.clone();
                div()
                    .w_full()
                    .px(px(10.0))
                    .py(px(8.0))
                    .rounded(px(CONTROL_RADIUS))
                    .bg(style.control_bg)
                    .opacity(if *disabled { 0.55 } else { 1.0 })
                    .when(enabled, |element| {
                        element.cursor_pointer().on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |view, _event, _window, cx| {
                                cx.stop_propagation();
                                if let Some(action_id) = action_id.clone() {
                                    view.dispatch(
                                        action_id,
                                        control_id.clone(),
                                        action_payload.clone(),
                                        Some(PluginViewValue::Text(control_id.clone())),
                                        cx,
                                    );
                                }
                            }),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(title.clone())
                            .children(subtitle.clone().map(|subtitle| {
                                div()
                                    .text_size(px(11.0))
                                    .text_color(style.muted_text)
                                    .child(subtitle)
                            })),
                    )
                    .children(
                        status
                            .clone()
                            .map(|status| div().text_color(style.muted_text).child(status)),
                    )
                    .into_any_element()
            }
            PluginUiNode::EmptyState { title, description } => div()
                .w_full()
                .py(px(24.0))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .text_size(px(14.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(style.primary_text)
                        .child(title.clone()),
                )
                .children(description.clone().map(|description| {
                    div()
                        .text_size(px(12.0))
                        .text_color(style.muted_text)
                        .child(description)
                }))
                .into_any_element(),
            PluginUiNode::Progress { label, value } => {
                let detail = value.map(|value| format!("{value}%"));
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(5.0))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_size(px(11.0))
                            .text_color(style.muted_text)
                            .children(label.clone())
                            .children(detail),
                    )
                    .child(
                        div()
                            .w_full()
                            .h(px(5.0))
                            .rounded(px(3.0))
                            .bg(style.control_bg)
                            .child(
                                div()
                                    .h_full()
                                    .w(gpui_kit::relative(
                                        value.map_or(0.35, |value| f32::from(value) / 100.0),
                                    ))
                                    .rounded(px(3.0))
                                    .bg(style.accent)
                                    .opacity(value.map_or(0.65, |_| 1.0)),
                            ),
                    )
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
                let focused = self.focused_control.as_deref() == Some(id.as_str());
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
                    .border_1()
                    .border_color(if focused { style.accent } else { background })
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
                let focused = self.focused_control.as_deref() == Some(id.as_str());
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
                            .border_color(if *checked || focused {
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
