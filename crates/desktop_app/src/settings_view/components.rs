use super::*;

impl SettingsWindow {
    fn masked_secret_value(text: &str) -> String {
        if text.is_empty() || text.eq_ignore_ascii_case("not configured") {
            "Not configured".to_string()
        } else {
            "••••••••".to_string()
        }
    }

    pub(super) fn wrap_setting_with_scroll_anchor(
        &self,
        setting_key: &'static str,
        content: AnyElement,
    ) -> AnyElement {
        // Anchor wrappers must participate in width layout; otherwise `w_full()` row
        // children can resolve against content size and render far beyond the viewport.
        div()
            .id(SharedString::from(format!("setting-{setting_key}")))
            .w_full()
            .min_w(px(0.0))
            .anchor_scroll(self.setting_scroll_anchors.get(setting_key).cloned())
            .child(content)
            .into_any_element()
    }

    pub(super) fn render_section_header(
        &self,
        title: &'static str,
        _legacy_subtitle: &'static str,
        section: SettingsSection,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let hover_bg = self.bg_hover();
        let text_primary = self.text_primary();
        let text_muted = self.text_muted();
        let subtitle = Self::settings_section_subtitle(section);
        // Quiet ghost button, rendered only while the section actually has
        // something to reset — native settings keep destructive affordances
        // out of sight until they apply.
        let can_reset = self.section_has_non_default_values(section);

        let reset_button = can_reset.then(|| {
            let is_section_hovered = self.hovered_reset_section == Some(section);
            let tooltip_bg = self.bg_elevated();
            let tooltip_border = self.border_color();
            let tooltip_fg = self.text_primary();
            div()
                .id(SharedString::from(format!("reset-section-{section:?}")))
                .relative()
                .px_2()
                .py(px(5.0))
                .rounded(px(SETTINGS_BUTTON_RADIUS))
                .text_xs()
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(text_muted)
                .cursor_pointer()
                .hover(move |s| s.bg(hover_bg).text_color(text_primary))
                .on_hover(cx.listener(move |view, hovering: &bool, _window, cx| {
                    if *hovering {
                        view.hovered_reset_section = Some(section);
                    } else if view.hovered_reset_section == Some(section) {
                        view.hovered_reset_section = None;
                    }
                    cx.notify();
                }))
                .child("Reset section")
                .on_click(cx.listener(move |view, _, _, cx| {
                    view.confirm_reset_section_to_defaults(section, cx);
                }))
                .when(is_section_hovered, move |s| {
                    s.child(
                        deferred(
                            div()
                                .absolute()
                                .top_full()
                                .right_0()
                                .mt(px(6.0))
                                .px(px(8.0))
                                .py(px(4.0))
                                .rounded(px(6.0))
                                .bg(tooltip_bg)
                                .border_1()
                                .border_color(tooltip_border)
                                .text_size(px(11.0))
                                .text_color(tooltip_fg)
                                .whitespace_nowrap()
                                .child("Reset all settings in this section"),
                        )
                        .with_priority(20),
                    )
                })
        });

        // Title and subtitle come from the design system; the reset affordance
        // keeps its hover, tooltip, and confirmation flow here because that is
        // app behavior, not presentation.
        let mut header = termy_ui::SectionHeader::new(title).subtitle(subtitle);
        if let Some(reset_button) = reset_button {
            header = header.action(reset_button);
        }
        header
    }

    /// Group caption for the few surfaces that build their own card instead of
    /// going through `render_settings_group`. Styled to match the label
    /// `termy_ui::SettingsGroup` draws, so both kinds of group read the same.
    pub(super) fn render_group_header(&self, title: impl Into<SharedString>) -> impl IntoElement {
        div()
            .text_size(px(GROUP_TITLE_SIZE))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(self.text_muted())
            .child(title.into())
    }

    pub(super) fn render_reset_setting_button(
        &self,
        setting_key: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let hover_bg = self.bg_hover();
        let text_primary = self.text_primary();
        // Settings at their default value render an empty slot of the same size
        // so controls stay column-aligned while the reset affordance stays out
        // of sight until it applies.
        let can_reset = !self.is_setting_at_default(setting_key);
        let is_hovered = self.hovered_reset_setting == Some(setting_key);
        let tooltip_bg = self.bg_elevated();
        let tooltip_border = self.border_color();
        let tooltip_fg = self.text_primary();

        div()
            .id(SharedString::from(format!("reset-setting-{setting_key}")))
            .w(px(22.0))
            .h(px(22.0))
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(SETTINGS_INPUT_RADIUS))
            .when(can_reset, |s| {
                s.cursor_pointer()
                    .hover(move |s| s.bg(hover_bg))
                    .on_hover(cx.listener(move |view, hovering: &bool, _window, cx| {
                        if *hovering {
                            view.hovered_reset_setting = Some(setting_key);
                        } else if view.hovered_reset_setting == Some(setting_key) {
                            view.hovered_reset_setting = None;
                        }
                        cx.notify();
                    }))
            })
            .children(can_reset.then(|| {
                svg()
                    .path(SharedString::from("icons/settings/reset.svg"))
                    .size(px(13.0))
                    .text_color(text_primary)
            }))
            .when(can_reset, |s| {
                s.on_click(cx.listener(move |view, _, _, cx| {
                    view.confirm_reset_setting_to_default(setting_key, cx);
                }))
            })
            .when(is_hovered && can_reset, |s| {
                s.child(
                    deferred(
                        div()
                            .absolute()
                            .bottom(px(28.0))
                            .right(px(-4.0))
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(6.0))
                            .bg(tooltip_bg)
                            .border_1()
                            .border_color(tooltip_border)
                            .text_size(px(11.0))
                            .text_color(tooltip_fg)
                            .whitespace_nowrap()
                            .child("Reset to default"),
                    )
                    .with_priority(20),
                )
            })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_setting_row(
        &mut self,
        search_key: &'static str,
        id: &'static str,
        title: &'static str,
        description: &'static str,
        checked: bool,
        cx: &mut Context<Self>,
        on_toggle: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        let is_search_match = self.setting_matches_sidebar_query(search_key);
        let match_stripe = self.accent_with_alpha(0.85);
        let match_bg = self.accent_with_alpha(0.06);

        let row = div()
            .w_full()
            .relative()
            .flex()
            .items_center()
            .gap_4()
            .py(px(CARD_ROW_PADDING_Y))
            .px(px(CARD_ROW_PADDING_X))
            .when(is_search_match, |s| s.bg(match_bg))
            .when(is_search_match, |s| {
                s.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(6.0))
                        .bottom(px(6.0))
                        .w(px(2.0))
                        .bg(match_stripe),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex_col()
                    .gap(px(1.0))
                    .child(div().text_sm().text_color(self.text_primary()).child(title))
                    .child(
                        div()
                            .text_xs()
                            .text_color(self.text_muted())
                            .line_height(px(16.0))
                            .child(description),
                    ),
            )
            .child(
                div()
                    .ml_auto()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(self.render_switch(id, checked, cx, on_toggle))
                    .child(self.render_reset_setting_button(search_key, cx)),
            );

        self.wrap_setting_with_scroll_anchor(search_key, row.into_any_element())
    }

    pub(super) fn render_root_bool_setting_row(
        &mut self,
        setting_key: &'static str,
        toggle_id: &'static str,
        setting: RootSettingId,
        checked: bool,
        success_message: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let metadata = Self::setting_metadata_or_fallback(setting_key);
        self.render_setting_row(
            setting_key,
            toggle_id,
            metadata.title,
            metadata.description,
            checked,
            cx,
            move |view, window, cx| {
                let next = !checked;
                match config::set_root_setting(setting, &next.to_string()) {
                    Ok(()) => {
                        let _ = view.reload_config_if_changed(cx);
                        crate::ui::toast::success(success_message);
                        if setting == RootSettingId::SimpleMode && next {
                            window.remove_window();
                        }
                        cx.notify();
                    }
                    Err(error) => crate::ui::toast::error(error),
                }
            },
        )
    }

    pub(super) fn render_switch(
        &self,
        id: impl Into<SharedString>,
        checked: bool,
        cx: &mut Context<Self>,
        on_toggle: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        let accent = self.accent_with_alpha(0.95);
        let mut bg_off = self.colors.foreground;
        bg_off.a = 0.28;
        let track_color = if checked { accent } else { bg_off };
        let knob_color = self.contrasting_text_for_fill(track_color, self.bg_card());
        let knob_top = (SETTINGS_SWITCH_HEIGHT - SETTINGS_SWITCH_KNOB_SIZE) * 0.5;
        let knob_left = if checked {
            SETTINGS_SWITCH_WIDTH - SETTINGS_SWITCH_KNOB_SIZE - knob_top
        } else {
            knob_top
        };

        div()
            .id(id.into())
            .w(px(SETTINGS_SWITCH_WIDTH))
            .h(px(SETTINGS_SWITCH_HEIGHT))
            .rounded(px(SETTINGS_SWITCH_RADIUS))
            .bg(track_color)
            .cursor_pointer()
            .relative()
            .child(
                div()
                    .absolute()
                    .top(px(knob_top))
                    .left(px(knob_left))
                    .w(px(SETTINGS_SWITCH_KNOB_SIZE))
                    .h(px(SETTINGS_SWITCH_KNOB_SIZE))
                    .rounded_full()
                    .bg(knob_color)
                    .shadow_sm(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, _event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    on_toggle(view, window, cx);
                    cx.notify();
                }),
            )
    }

    pub(super) fn active_dropdown_options(
        &self,
        field: EditableField,
        is_active: bool,
        uses_dropdown: bool,
    ) -> Vec<DropdownOption> {
        if !uses_dropdown || !is_active {
            return Vec::new();
        }
        let query = self
            .active_input
            .as_ref()
            .map_or("", |input| input.state.text());
        if Self::field_uses_click_only_dropdown(field) {
            self.dropdown_options_for_field(field, "")
        } else {
            self.dropdown_options_for_field(field, query)
        }
    }

    pub(super) fn editable_dropdown_overlay(
        &self,
        field: EditableField,
        options: Vec<DropdownOption>,
        text_secondary: Rgba,
        hover_bg: Rgba,
        border_color: Rgba,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if options.is_empty() {
            return None;
        }

        let mut list = div().flex().flex_col().py_1();
        for (index, option) in options.into_iter().enumerate() {
            let option_label = option.display_text();
            let option_value = option.value.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!(
                        "dropdown-option-{field:?}-{index}"
                    )))
                    .px_3()
                    .py_1()
                    .text_sm()
                    .text_color(text_secondary)
                    .cursor_pointer()
                    .hover(|this| this.bg(hover_bg))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _event: &MouseDownEvent, _window, cx| {
                            cx.stop_propagation();
                            view.apply_dropdown_selection(field, &option_value, cx);
                        }),
                    )
                    .child(option_label),
            );
        }

        let mut dropdown_bg = self.colors.background;
        dropdown_bg.a = 1.0;
        Some(
            deferred(
                div()
                    .id(SharedString::from(format!(
                        "dropdown-suggestions-{field:?}"
                    )))
                    .occlude()
                    .absolute()
                    .top(px(SETTINGS_CONTROL_HEIGHT + 2.0))
                    .left_0()
                    .right_0()
                    .max_h(if field == EditableField::Theme {
                        px(180.0)
                    } else {
                        px(240.0)
                    })
                    .overflow_scroll()
                    .overflow_x_hidden()
                    .rounded(px(SETTINGS_BUTTON_RADIUS))
                    .bg(dropdown_bg)
                    .border_1()
                    .border_color(border_color)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_view, _event: &MouseDownEvent, _window, cx| {
                            cx.stop_propagation();
                        }),
                    )
                    .on_scroll_wheel(cx.listener(
                        |_view, _event: &ScrollWheelEvent, _window, cx| {
                            cx.stop_propagation();
                        },
                    ))
                    .child(list),
            )
            .with_priority(10)
            .into_any_element(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn editable_row_value_element(
        &self,
        field: EditableField,
        is_numeric: bool,
        is_active: bool,
        uses_text_input: bool,
        uses_dropdown: bool,
        display_value: String,
        text_secondary: Rgba,
        bg_card: Rgba,
        text_primary: Rgba,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_secret_field = Self::is_secret_field(field);
        let readonly_display_value = if !is_active && uses_dropdown {
            self.dropdown_display_value(field, &display_value)
        } else if !is_active && is_secret_field {
            Self::masked_secret_value(&display_value)
        } else {
            display_value.clone()
        };

        if is_numeric {
            let chevron_size = px(11.0);
            let stepper_w = px(NUMERIC_STEP_BUTTON_SIZE);
            let stepper_h = px(NUMERIC_STEP_BUTTON_SIZE * 0.5);
            return div()
                .h_full()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .text_color(text_secondary)
                        .child(display_value),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .w(stepper_w)
                        .child(
                            div()
                                .id(SharedString::from(format!("inc-{field:?}")))
                                .w(stepper_w)
                                .h(stepper_h)
                                .cursor_pointer()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_t(px(SETTINGS_INPUT_RADIUS - 2.0))
                                .text_color(text_primary)
                                .hover(|s| s.bg(bg_card))
                                .child(
                                    svg()
                                        .path(SharedString::from("icons/settings/chevron-up.svg"))
                                        .size(chevron_size)
                                        .text_color(text_primary),
                                )
                                .on_click(cx.listener(move |view, _, _, cx| {
                                    cx.stop_propagation();
                                    view.step_numeric_field(field, 1, cx);
                                })),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("dec-{field:?}")))
                                .w(stepper_w)
                                .h(stepper_h)
                                .cursor_pointer()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_b(px(SETTINGS_INPUT_RADIUS - 2.0))
                                .text_color(text_primary)
                                .hover(|s| s.bg(bg_card))
                                .child(
                                    svg()
                                        .path(SharedString::from("icons/settings/chevron-down.svg"))
                                        .size(chevron_size)
                                        .text_color(text_primary),
                                )
                                .on_click(cx.listener(move |view, _, _, cx| {
                                    cx.stop_propagation();
                                    view.step_numeric_field(field, -1, cx);
                                })),
                        ),
                )
                .into_any_element();
        }

        if is_active && uses_text_input {
            if is_secret_field {
                let mut hidden_text = text_secondary;
                hidden_text.a = 0.0;
                let mut hidden_selection = self.accent_with_alpha(0.3);
                hidden_selection.a = 0.0;
                let masked_active_value = self
                    .active_input
                    .as_ref()
                    .filter(|input| input.field == field)
                    .map_or_else(
                        || Self::masked_secret_value(&display_value),
                        |input| Self::masked_secret_value(input.state.text()),
                    );

                let font = Font {
                    family: self.config.ui_font_family.clone().into(),
                    ..gpui::font("")
                };

                return div()
                    .relative()
                    .size_full()
                    .child(TextInputElement::new(
                        cx.entity(),
                        self.focus_handle.clone(),
                        font,
                        px(SETTINGS_INPUT_TEXT_SIZE),
                        hidden_text.into(),
                        hidden_selection.into(),
                        TextInputAlignment::Left,
                    ))
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .h_full()
                            .flex()
                            .items_center()
                            .text_size(px(SETTINGS_INPUT_TEXT_SIZE))
                            .text_color(text_secondary)
                            .child(masked_active_value),
                    )
                    .into_any_element();
            }
            let font = Font {
                family: self.config.ui_font_family.clone().into(),
                ..gpui::font("")
            };
            return TextInputElement::new(
                cx.entity(),
                self.focus_handle.clone(),
                font,
                px(SETTINGS_INPUT_TEXT_SIZE),
                text_secondary.into(),
                self.accent_with_alpha(0.3).into(),
                TextInputAlignment::Left,
            )
            .into_any_element();
        }

        let mut readonly = div()
            .h_full()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .text_size(px(SETTINGS_INPUT_TEXT_SIZE))
                    .text_color(text_secondary)
                    .child(readonly_display_value),
            );
        if field == EditableField::Theme {
            readonly = readonly.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().w(px(10.0)).h(px(10.0)).bg(self.colors.background))
                    .child(div().w(px(10.0)).h(px(10.0)).bg(self.colors.foreground))
                    .child(div().w(px(10.0)).h(px(10.0)).bg(self.colors.cursor)),
            );
        } else if field == EditableField::FontFamily {
            readonly = readonly.child(
                div()
                    .text_xs()
                    .text_color(self.text_muted())
                    .font_family(self.config.font_family.clone())
                    .child("Ag"),
            );
        } else if field == EditableField::UiFontFamily {
            readonly = readonly.child(
                div()
                    .text_xs()
                    .text_color(self.text_muted())
                    .font_family(self.config.ui_font_family.clone())
                    .child("Ag"),
            );
        }
        if uses_dropdown {
            let chevron_path = if is_active {
                "icons/settings/chevron-up.svg"
            } else {
                "icons/settings/chevron-down.svg"
            };
            readonly = readonly.child(
                svg()
                    .path(SharedString::from(chevron_path))
                    .size(px(12.0))
                    .text_color(self.text_muted()),
            );
        }
        readonly.into_any_element()
    }

    pub(super) fn handle_editable_row_mouse_down(
        &mut self,
        field: EditableField,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        if !self
            .active_input
            .as_ref()
            .is_some_and(|input| input.field == field)
        {
            self.begin_editing_field(field, window, cx);
        }
        if let Some(input) = self.active_input.as_mut() {
            if Self::uses_text_input_for_field(field) {
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
            } else {
                input.selecting = false;
            }
        }
        self.focus_handle.focus(window);
        cx.notify();
    }

    pub(super) fn handle_editable_row_mouse_move(
        &mut self,
        field: EditableField,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = self.active_input.as_mut() else {
            return;
        };
        if input.field != field || !input.selecting || !event.dragging() {
            return;
        }
        let index = input.state.character_index_for_point(event.position);
        input.state.select_to_utf16(index);
        cx.notify();
    }

    pub(super) fn handle_editable_row_mouse_up(
        &mut self,
        field: EditableField,
        cx: &mut Context<Self>,
    ) {
        if let Some(input) = self.active_input.as_mut()
            && input.field == field
        {
            input.selecting = false;
            cx.notify();
        }
    }

    pub(super) fn render_editable_row(
        &mut self,
        search_key: &'static str,
        field: EditableField,
        title: &'static str,
        description: &'static str,
        display_value: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if field == EditableField::BackgroundOpacity {
            return self.render_background_opacity_row(search_key, title, description, cx);
        }
        let is_numeric = Self::is_numeric_field(field);
        let uses_text_input = Self::uses_text_input_for_field(field);
        let is_active = self
            .active_input
            .as_ref()
            .is_some_and(|input| input.field == field);
        let uses_dropdown = Self::field_uses_dropdown(field);
        let text_secondary = self.text_secondary();
        let hover_bg = self.bg_hover();
        let row_hover_bg = self.bg_elevated();
        let input_bg = self.bg_input();
        let border_color = self.border_color();
        let idle_border = self.card_border_color();
        let accent = self.accent();
        let bg_card = self.bg_card();
        let text_primary = self.text_primary();
        let text_muted = self.text_muted();
        let is_search_match = self.setting_matches_sidebar_query(search_key);
        let match_stripe = self.accent_with_alpha(0.85);
        let match_bg = self.accent_with_alpha(0.06);
        let dropdown_options = self.active_dropdown_options(field, is_active, uses_dropdown);
        let _dropdown_open = is_active && uses_dropdown && !dropdown_options.is_empty();
        let dropdown = self.editable_dropdown_overlay(
            field,
            dropdown_options,
            text_secondary,
            hover_bg,
            border_color,
            cx,
        );
        let value_element = self.editable_row_value_element(
            field,
            is_numeric,
            is_active,
            uses_text_input,
            uses_dropdown,
            display_value,
            text_secondary,
            bg_card,
            text_primary,
            cx,
        );

        let row = div()
            .id(SharedString::from(format!("editable-row-{field:?}")))
            .w_full()
            .relative()
            .flex()
            .items_center()
            .gap_4()
            .py(px(CARD_ROW_PADDING_Y))
            .px(px(CARD_ROW_PADDING_X))
            .when(is_search_match, |s| s.bg(match_bg))
            .when(is_search_match, |s| {
                s.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(6.0))
                        .bottom(px(6.0))
                        .w(px(2.0))
                        .bg(match_stripe),
                )
            })
            .cursor_pointer()
            .hover(move |s| s.bg(row_hover_bg))
            .when(!is_numeric, |s| {
                s.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                        view.handle_editable_row_mouse_down(field, event, window, cx);
                    }),
                )
                .on_mouse_move(
                    cx.listener(move |view, event: &MouseMoveEvent, _window, cx| {
                        view.handle_editable_row_mouse_move(field, event, cx);
                    }),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(move |view, _event: &MouseUpEvent, _window, cx| {
                        view.handle_editable_row_mouse_up(field, cx);
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(move |view, _event: &MouseUpEvent, _window, cx| {
                        view.handle_editable_row_mouse_up(field, cx);
                    }),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(text_primary)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(text_muted)
                            .line_height(px(17.0))
                            .child(description),
                    ),
            )
            .child(
                div()
                    .ml_auto()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w(px(SETTINGS_CONTROL_WIDTH))
                            .relative()
                            .h(px(SETTINGS_CONTROL_HEIGHT))
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .h_full()
                                    .px_2()
                                    .rounded(px(SETTINGS_INPUT_RADIUS))
                                    .bg(input_bg)
                                    .border_1()
                                    .border_color(if is_active { accent } else { idle_border })
                                    .overflow_hidden()
                                    .child(value_element),
                            )
                            .when_some(dropdown, |s, dropdown| s.child(dropdown)),
                    )
                    .child(self.render_reset_setting_button(search_key, cx)),
            );
        self.wrap_setting_with_scroll_anchor(search_key, row.into_any_element())
    }

    pub(super) fn background_opacity_slider_width() -> f32 {
        let fixed = SETTINGS_SLIDER_VALUE_WIDTH + (NUMERIC_STEP_BUTTON_SIZE * 2.0);
        let gaps = SETTINGS_OPACITY_CONTROL_GAP * 3.0;
        (SETTINGS_CONTROL_WIDTH - (SETTINGS_CONTROL_INNER_PADDING * 2.0) - fixed - gaps).max(80.0)
    }

    pub(super) fn quantize_background_opacity_ratio(ratio: f32) -> f32 {
        let step = SETTINGS_OPACITY_STEP_RATIO;
        ((ratio.clamp(0.0, 1.0) / step).round() * step).clamp(0.0, 1.0)
    }

    pub(super) fn background_opacity_ratio_from_local_x(local_x: f32, slider_width: f32) -> f32 {
        (local_x / slider_width.max(1.0)).clamp(0.0, 1.0)
    }

    pub(super) fn background_opacity_local_x_from_window_x(
        window_x: f32,
        slider_left: f32,
        slider_width: f32,
    ) -> f32 {
        (window_x - slider_left).clamp(0.0, slider_width.max(1.0))
    }

    pub(super) fn set_background_opacity_preview(&mut self, ratio: f32) -> bool {
        let ratio = Self::quantize_background_opacity_ratio(ratio);
        if (self.effective_background_opacity() - ratio).abs() < f32::EPSILON {
            return false;
        }
        let preview = config::BackgroundOpacityPreview {
            owner_id: self.background_opacity_preview_owner_id,
            opacity: ratio,
        };
        self.preview_background_opacity = Some(preview);
        config::publish_background_opacity_preview(Some(preview));
        true
    }

    pub(super) fn persist_background_opacity(&mut self, ratio: f32) -> Result<(), String> {
        let ratio = Self::quantize_background_opacity_ratio(ratio);
        let previous = self.config.background_opacity;
        self.config.background_opacity = ratio;
        if let Err(error) =
            config::set_root_setting(RootSettingId::BackgroundOpacity, &format!("{ratio:.3}"))
        {
            self.config.background_opacity = previous;
            return Err(error);
        }
        Ok(())
    }

    fn background_opacity_slider_local_x(&self, window_x: f32) -> Option<f32> {
        let bounds = self.background_opacity_slider_bounds?;
        let slider_left: f32 = bounds.left().into();
        let slider_width: f32 = bounds.size.width.into();
        Some(Self::background_opacity_local_x_from_window_x(
            window_x,
            slider_left,
            slider_width,
        ))
    }

    pub(super) fn begin_background_opacity_drag(&mut self, local_x: f32) {
        self.background_opacity_drag_state = Some(BackgroundOpacityDragState {
            start_local_x: local_x,
            start_ratio: self.effective_background_opacity(),
        });
    }

    pub(super) fn update_background_opacity_drag(
        &mut self,
        window_x: f32,
        slider_width: f32,
    ) -> bool {
        let Some(drag_state) = self.background_opacity_drag_state else {
            return false;
        };
        let Some(local_x) = self.background_opacity_slider_local_x(window_x) else {
            return false;
        };
        let delta_ratio = (local_x - drag_state.start_local_x) / slider_width.max(1.0);
        self.set_background_opacity_preview(drag_state.start_ratio + delta_ratio)
    }

    pub(super) fn set_background_opacity_from_slider_position(
        &mut self,
        window_x: f32,
        slider_width: f32,
    ) -> bool {
        let Some(local_x) = self.background_opacity_slider_local_x(window_x) else {
            return false;
        };
        let ratio = Self::background_opacity_ratio_from_local_x(local_x, slider_width);
        self.set_background_opacity_preview(ratio)
    }

    pub(super) fn finish_background_opacity_drag(&mut self) -> Result<bool, String> {
        let Some(_drag_state) = self.background_opacity_drag_state.take() else {
            return Ok(false);
        };
        let saved_ratio = self.config.background_opacity;
        let ratio = self.effective_background_opacity();
        if (ratio - saved_ratio).abs() < f32::EPSILON {
            self.clear_background_opacity_preview();
            return Ok(false);
        }
        if let Err(error) = self.persist_background_opacity(ratio) {
            self.clear_background_opacity_preview();
            return Err(error);
        }
        self.clear_background_opacity_preview();
        Ok((ratio - saved_ratio).abs() >= f32::EPSILON)
    }

    pub(super) fn step_background_opacity(&mut self, delta: i32) -> Result<bool, String> {
        let next = self.config.background_opacity + (delta as f32 * SETTINGS_OPACITY_STEP_RATIO);
        let next = Self::quantize_background_opacity_ratio(next);
        if (self.config.background_opacity - next).abs() < f32::EPSILON {
            return Ok(false);
        }
        self.clear_background_opacity_preview();
        self.persist_background_opacity(next)?;
        Ok(true)
    }

    pub(super) fn background_opacity_step_button(
        &self,
        id: &'static str,
        label: &'static str,
        delta: i32,
        border_color: Rgba,
        text_primary: Rgba,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(id)
            .w(px(NUMERIC_STEP_BUTTON_SIZE))
            .h(px(NUMERIC_STEP_BUTTON_SIZE))
            .rounded(px(SETTINGS_BUTTON_RADIUS))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(self.bg_input())
            .border_1()
            .border_color(border_color)
            .text_color(text_primary)
            .font_weight(gpui::FontWeight::BOLD)
            .text_sm()
            .child(label)
            .on_click(cx.listener(move |view, _, _, cx| {
                match view.step_background_opacity(delta) {
                    Ok(true) => crate::ui::toast::success("Saved"),
                    Ok(false) => {}
                    Err(error) => crate::ui::toast::error(error),
                }
                cx.notify();
            }))
            .into_any_element()
    }

    pub(super) fn background_opacity_slider(
        &self,
        slider_width: f32,
        slider_fill_width: f32,
        slider_thumb_left: f32,
        slider_track: Rgba,
        slider_fill: Rgba,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id("background-opacity-slider")
            .relative()
            .w(px(slider_width))
            .h(px(SETTINGS_SWITCH_KNOB_SIZE))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    let window_x: f32 = event.position.x.into();
                    if let Some(local_x) = view.background_opacity_slider_local_x(window_x) {
                        view.set_background_opacity_from_slider_position(window_x, slider_width);
                        view.begin_background_opacity_drag(local_x);
                        cx.notify();
                    }
                }),
            )
            .on_mouse_move(
                cx.listener(move |view, event: &MouseMoveEvent, _window, cx| {
                    if !event.dragging() {
                        return;
                    }
                    cx.stop_propagation();
                    let window_x: f32 = event.position.x.into();
                    view.update_background_opacity_drag(window_x, slider_width);
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|view, _event: &MouseUpEvent, _window, cx| {
                    cx.stop_propagation();
                    match view.finish_background_opacity_drag() {
                        Ok(true) => crate::ui::toast::success("Saved"),
                        Ok(false) => {}
                        Err(error) => crate::ui::toast::error(error),
                    }
                    cx.notify();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|view, _event: &MouseUpEvent, _window, cx| {
                    if view.background_opacity_drag_state.is_none() {
                        return;
                    }
                    cx.stop_propagation();
                    match view.finish_background_opacity_drag() {
                        Ok(true) => crate::ui::toast::success("Saved"),
                        Ok(false) => {}
                        Err(error) => crate::ui::toast::error(error),
                    }
                    cx.notify();
                }),
            )
            .child(
                canvas(
                    {
                        let this = cx.entity();
                        move |bounds, _, cx| {
                            this.update(cx, |view, _| {
                                view.background_opacity_slider_bounds = Some(bounds);
                            });
                        }
                    },
                    move |_bounds, _, _window, _cx| {},
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .top(px(7.0))
                    .left_0()
                    .w(px(slider_width))
                    .h(px(4.0))
                    .rounded(px(2.0))
                    .bg(slider_track),
            )
            .child(
                div()
                    .absolute()
                    .top(px(7.0))
                    .left_0()
                    .w(px(slider_fill_width))
                    .h(px(4.0))
                    .rounded(px(2.0))
                    .bg(slider_fill),
            )
            .child(
                div()
                    .absolute()
                    .top(px(2.0))
                    .left(px(slider_thumb_left))
                    .w(px(14.0))
                    .h(px(14.0))
                    .rounded_full()
                    .bg(self.accent())
                    .shadow_sm(),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn background_opacity_controls(
        &self,
        slider_width: f32,
        slider_fill_width: f32,
        slider_thumb_left: f32,
        slider_track: Rgba,
        slider_fill: Rgba,
        percentage: String,
        border_color: Rgba,
        text_primary: Rgba,
        text_secondary: Rgba,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .h_full()
            .flex()
            .items_center()
            .gap(px(SETTINGS_OPACITY_CONTROL_GAP))
            .child(self.background_opacity_slider(
                slider_width,
                slider_fill_width,
                slider_thumb_left,
                slider_track,
                slider_fill,
                cx,
            ))
            .child(self.background_opacity_step_button(
                "background-opacity-dec",
                "-",
                -1,
                border_color,
                text_primary,
                cx,
            ))
            .child(
                div()
                    .w(px(SETTINGS_SLIDER_VALUE_WIDTH))
                    .text_sm()
                    .text_color(text_secondary)
                    .text_align(TextAlign::Center)
                    .child(percentage),
            )
            .child(self.background_opacity_step_button(
                "background-opacity-inc",
                "+",
                1,
                border_color,
                text_primary,
                cx,
            ))
            .into_any_element()
    }

    pub(super) fn render_background_opacity_row(
        &mut self,
        search_key: &'static str,
        title: &'static str,
        description: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_search_match = self.setting_matches_sidebar_query(search_key);
        let match_stripe = self.accent_with_alpha(0.85);
        let match_bg = self.accent_with_alpha(0.06);
        let border_color = self.card_border_color();
        let input_bg = self.bg_input();
        let text_primary = self.text_primary();
        let text_secondary = self.text_secondary();
        let text_muted = self.text_muted();
        let slider_track = self.bg_hover();
        let slider_fill = self.accent_with_alpha(0.9);
        let slider_width = Self::background_opacity_slider_width();
        let slider_ratio = self.effective_background_opacity();
        let slider_fill_width = slider_ratio * slider_width;
        let slider_thumb_left = (slider_fill_width - 7.0).clamp(0.0, slider_width - 14.0);
        let percentage = format!("{}%", (slider_ratio * 100.0).round() as i32);
        let controls = self.background_opacity_controls(
            slider_width,
            slider_fill_width,
            slider_thumb_left,
            slider_track,
            slider_fill,
            percentage,
            border_color,
            text_primary,
            text_secondary,
            cx,
        );

        let row = div()
            .id("editable-row-background-opacity")
            .w_full()
            .relative()
            .flex()
            .items_center()
            .gap_4()
            .py(px(CARD_ROW_PADDING_Y))
            .px(px(CARD_ROW_PADDING_X))
            .when(is_search_match, |s| s.bg(match_bg))
            .when(is_search_match, |s| {
                s.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(6.0))
                        .bottom(px(6.0))
                        .w(px(2.0))
                        .bg(match_stripe),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(text_primary)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(text_muted)
                            .line_height(px(17.0))
                            .child(description),
                    ),
            )
            .child(
                div()
                    .ml_auto()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w(px(SETTINGS_CONTROL_WIDTH))
                            .h(px(SETTINGS_CONTROL_HEIGHT))
                            .px(px(SETTINGS_CONTROL_INNER_PADDING))
                            .rounded(px(SETTINGS_BUTTON_RADIUS))
                            .bg(input_bg)
                            .border_1()
                            .border_color(border_color)
                            .child(controls),
                    )
                    .child(self.render_reset_setting_button(search_key, cx)),
            );

        self.wrap_setting_with_scroll_anchor(search_key, row.into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::SettingsWindow;

    #[test]
    fn background_opacity_ratio_from_local_x_clamps() {
        assert_eq!(
            SettingsWindow::background_opacity_ratio_from_local_x(-10.0, 120.0),
            0.0
        );
        assert_eq!(
            SettingsWindow::background_opacity_ratio_from_local_x(60.0, 120.0),
            0.5
        );
        assert_eq!(
            SettingsWindow::background_opacity_ratio_from_local_x(200.0, 120.0),
            1.0
        );
    }

    #[test]
    fn background_opacity_local_x_from_window_x_accounts_for_slider_offset() {
        assert_eq!(
            SettingsWindow::background_opacity_local_x_from_window_x(260.0, 200.0, 120.0),
            60.0
        );
        assert_eq!(
            SettingsWindow::background_opacity_local_x_from_window_x(500.0, 200.0, 120.0),
            120.0
        );
    }

    #[test]
    fn drag_delta_uses_slider_local_coordinates() {
        let slider_width = 120.0;
        let slider_left = 200.0;
        let start_window_x = 260.0;
        let current_window_x = 296.0;
        let start_local_x = SettingsWindow::background_opacity_local_x_from_window_x(
            start_window_x,
            slider_left,
            slider_width,
        );
        let current_local_x = SettingsWindow::background_opacity_local_x_from_window_x(
            current_window_x,
            slider_left,
            slider_width,
        );

        let delta_ratio = (current_local_x - start_local_x) / slider_width;
        let next_ratio = SettingsWindow::quantize_background_opacity_ratio(0.5 + delta_ratio);

        assert_eq!(start_local_x, 60.0);
        assert_eq!(current_local_x, 96.0);
        assert_eq!(next_ratio, 0.8);
    }
}
