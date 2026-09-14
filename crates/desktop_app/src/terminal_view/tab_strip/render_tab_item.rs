use super::super::*;
use super::render_palette::TabStripPalette;
use super::state::{TabDropMarkerSide, TabStripOrientation};
use termy_core::ProgressState;

pub(super) struct TabItemRenderInput {
    pub(super) orientation: TabStripOrientation,
    pub(super) index: usize,
    // Extent along the strip's primary axis (width for the horizontal strip,
    // row height for the vertical sidebar); cross extent is the other axis.
    pub(super) tab_primary_extent: f32,
    pub(super) tab_cross_extent: f32,
    pub(super) label: String,
    pub(super) switch_hint_label: Option<String>,
    pub(super) is_active: bool,
    pub(super) is_drag_source: bool,
    pub(super) is_pane_drop_target: bool,
    pub(super) is_renaming: bool,
    pub(super) show_tab_close: bool,
    pub(super) show_tab_pin: bool,
    pub(super) close_slot_width: f32,
    pub(super) text_padding_x: f32,
    pub(super) label_centered: bool,
    pub(super) drop_marker_side: Option<TabDropMarkerSide>,
    pub(super) open_anim_progress: Option<f32>,
    pub(super) hover_progress: f32,
    pub(super) progress_state: ProgressState,
}

#[cfg(test)]
const _: () = {
    assert!(TAB_CLOSE_CHIP_WIDTH < TAB_CLOSE_SLOT_WIDTH);
    assert!(TAB_CLOSE_CHIP_HEIGHT < TAB_CLOSE_HITBOX);
};

impl TerminalView {
    fn tab_accessory_visible(input: &TabItemRenderInput) -> bool {
        !input.is_renaming && input.close_slot_width > 0.0
    }

    fn render_tab_accessory(
        &self,
        input: &TabItemRenderInput,
        palette: &TabStripPalette,
        close_text_color: gpui::Rgba,
        hover_tab_index: usize,
        close_tab_index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(label) = input.switch_hint_label.as_ref() {
            let mut accessory = div()
                .flex_none()
                .w(px(input.close_slot_width))
                .h(px(TAB_CLOSE_HITBOX))
                .flex()
                .items_center()
                .justify_center()
                .bg(palette.switch_hint_bg)
                .text_color(palette.switch_hint_text)
                .text_size(px(TAB_SWITCH_HINT_TEXT_SIZE))
                .font_weight(FontWeight::MEDIUM);

            if input.orientation == TabStripOrientation::Horizontal {
                accessory = accessory
                    .border_l_1()
                    .border_color(palette.switch_hint_border);
            } else {
                accessory = accessory
                    .border_1()
                    .border_color(palette.switch_hint_border);
            }

            return accessory.child(label.clone()).into_any_element();
        }

        if input.show_tab_pin {
            return div()
                .flex_none()
                .w(px(input.close_slot_width))
                .h(px(TAB_CLOSE_HITBOX))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .w(px(TAB_CLOSE_CHIP_WIDTH.min(input.close_slot_width)))
                        .h(px(TAB_CLOSE_CHIP_HEIGHT.min(TAB_CLOSE_HITBOX)))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(TAB_CLOSE_CHIP_RADIUS))
                        .bg(palette.close_button_bg)
                        .text_color(close_text_color)
                        .text_size(px(7.5))
                        .font_weight(FontWeight::BOLD)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _event: &MouseDownEvent, window, cx| {
                                window.prevent_default();
                                let _ = this.set_tab_pinned(close_tab_index, false, cx);
                                cx.stop_propagation();
                            }),
                        )
                        .on_mouse_move(cx.listener(
                            move |this, _event: &MouseMoveEvent, _window, cx| {
                                this.on_tab_close_mouse_move(hover_tab_index, cx);
                                cx.stop_propagation();
                            },
                        ))
                        .hover(move |style| {
                            style
                                .bg(palette.close_button_hover_bg)
                                .text_color(palette.close_button_hover_text)
                        })
                        .cursor_pointer()
                        .child(
                            gpui::svg()
                                .path(gpui::SharedString::from("icons/command_palette/pin.svg"))
                                .size(px(10.0))
                                .text_color(close_text_color),
                        ),
                )
                .into_any_element();
        }

        let close_font_size = if input.orientation == TabStripOrientation::Horizontal {
            TAB_HORIZONTAL_TITLE_FONT_SIZE
        } else {
            TAB_TITLE_FONT_SIZE
        };

        div()
            .flex_none()
            .w(px(input.close_slot_width))
            .h(px(TAB_CLOSE_HITBOX))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(TAB_CLOSE_CHIP_WIDTH.min(input.close_slot_width)))
                    .h(px(TAB_CLOSE_CHIP_HEIGHT.min(TAB_CLOSE_HITBOX)))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(TAB_CLOSE_CHIP_RADIUS))
                    .bg(palette.close_button_bg)
                    .text_color(close_text_color)
                    .text_size(px(close_font_size))
                    .font_weight(FontWeight::MEDIUM)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event: &MouseDownEvent, window, cx| {
                            window.prevent_default();
                            this.request_tab_close_by_index(close_tab_index, window, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .on_mouse_move(cx.listener(
                        move |this, _event: &MouseMoveEvent, _window, cx| {
                            this.on_tab_close_mouse_move(hover_tab_index, cx);
                            cx.stop_propagation();
                        },
                    ))
                    .hover(move |style| {
                        style
                            .bg(palette.close_button_hover_bg)
                            .text_color(palette.close_button_hover_text)
                    })
                    .cursor_pointer()
                    .child(
                        gpui::svg()
                            .path(gpui::SharedString::from("icons/tab_strip/x.svg"))
                            .size(px(10.0))
                            .text_color(close_text_color),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_tab_item(
        &mut self,
        input: TabItemRenderInput,
        font_family: &SharedString,
        colors: &TerminalColors,
        palette: &TabStripPalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let orientation = input.orientation;
        let switch_tab_index = input.index;
        let hover_tab_index = input.index;
        let close_tab_index = input.index;
        let tab_primary_extent = input.tab_primary_extent;

        let anim = input.open_anim_progress.unwrap_or(1.0);

        let mut rename_text_color = if input.is_active {
            palette.active_tab_text
        } else {
            palette.inactive_tab_text
        };
        rename_text_color.a *= anim;
        let mut rename_selection_color = colors.cursor;
        rename_selection_color.a = if input.is_active { 0.34 } else { 0.24 };
        rename_selection_color.a *= anim;

        let base_tab_bg = if input.is_active {
            palette.active_tab_bg
        } else {
            palette.inactive_tab_bg
        };
        let target_tab_bg = if input.is_active {
            palette.active_tab_bg
        } else {
            palette.hovered_tab_bg
        };
        let hover_progress = input.hover_progress.clamp(0.0, 1.0);
        let mut tab_bg = base_tab_bg;
        tab_bg.a = base_tab_bg.a + ((target_tab_bg.a - base_tab_bg.a) * hover_progress);
        if input.is_drag_source {
            tab_bg.a = (tab_bg.a + self.scaled_chrome_surface_alpha(0.06)).min(1.0);
        }
        if input.is_pane_drop_target {
            tab_bg = palette.hovered_tab_bg;
            tab_bg.a = (tab_bg.a * 1.8).max(self.scaled_chrome_surface_alpha(0.12));
        }
        tab_bg.a *= anim;

        let mut close_text_color = if input.is_active {
            palette.active_tab_text
        } else {
            palette.inactive_tab_text
        };
        close_text_color.a *= anim;
        if input.is_renaming || (!input.show_tab_close && !input.show_tab_pin) {
            close_text_color.a = 0.0;
        }

        let justify_label_center = input.label_centered;
        let title_font_size = if orientation == TabStripOrientation::Horizontal {
            TAB_HORIZONTAL_TITLE_FONT_SIZE
        } else {
            TAB_TITLE_FONT_SIZE
        };
        let mut hover_tab_bg = if input.is_active {
            palette.active_tab_bg
        } else {
            palette.hovered_tab_bg
        };
        if input.is_drag_source {
            hover_tab_bg.a = (hover_tab_bg.a + self.scaled_chrome_surface_alpha(0.06)).min(1.0);
        }
        let drag_offset_y = if input.is_drag_source { -1.0 } else { 0.0 };
        let visual_offset_y = if orientation == TabStripOrientation::Horizontal {
            drag_offset_y
        } else {
            0.0
        };
        // Primary extent runs along the strip axis: width for the horizontal
        // strip, height for the vertical sidebar.
        let (shell_width, shell_height) = match orientation {
            TabStripOrientation::Horizontal => (input.tab_primary_extent, input.tab_cross_extent),
            TabStripOrientation::Vertical => (input.tab_cross_extent, input.tab_primary_extent),
        };
        let mut tab_shell = div()
            .flex_none()
            .relative()
            .overflow_hidden()
            .rounded(px(TAB_ITEM_RADIUS))
            .bg(tab_bg)
            .hover(move |style| style.bg(hover_tab_bg))
            .w(px(shell_width))
            .h(px(shell_height))
            .mt(px(visual_offset_y))
            .px(px(input.text_padding_x))
            .flex()
            .items_center()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.prevent_default();
                    this.on_tab_mouse_down(
                        orientation,
                        switch_tab_index,
                        event.click_count,
                        window,
                        cx,
                    );
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.prevent_default();
                    this.open_tab_context_menu_for_window(
                        switch_tab_index,
                        event.position,
                        window,
                        cx,
                    );
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _event: &MouseDownEvent, window, cx| {
                    window.prevent_default();
                    this.request_tab_close_by_index(close_tab_index, window, cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_move(
                cx.listener(move |this, event: &MouseMoveEvent, window, cx| {
                    this.on_tab_mouse_move(orientation, hover_tab_index, event, window, cx);
                    cx.stop_propagation();
                }),
            );

        if input.is_pane_drop_target {
            tab_shell = tab_shell
                .border_1()
                .border_color(palette.active_tab_indicator);
        }

        if input.is_drag_source {
            tab_shell = tab_shell.shadow_md();
        }

        if input.is_active {
            let indicator_height = (shell_height - (TAB_ACTIVE_INDICATOR_INSET_Y * 2.0)).max(0.0);
            let mut indicator_color = palette.active_tab_indicator;
            indicator_color.a *= anim;
            tab_shell = tab_shell.child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(TAB_ACTIVE_INDICATOR_INSET_Y))
                    .w(px(TAB_ACTIVE_INDICATOR_WIDTH))
                    .h(px(indicator_height))
                    .rounded_full()
                    .bg(indicator_color),
            );
        }

        let drop_marker = input.drop_marker_side.map(|side| match orientation {
            TabStripOrientation::Horizontal => {
                let marker_x = match side {
                    TabDropMarkerSide::Leading => 0.0,
                    TabDropMarkerSide::Trailing => input.tab_primary_extent - TAB_DROP_MARKER_WIDTH,
                }
                .max(0.0);
                let marker_height =
                    (input.tab_cross_extent - (TAB_DROP_MARKER_INSET_Y * 2.0)).max(0.0);

                div()
                    .absolute()
                    .left(px(marker_x))
                    .top(px(TAB_DROP_MARKER_INSET_Y))
                    .w(px(TAB_DROP_MARKER_WIDTH))
                    .h(px(marker_height))
                    .rounded_full()
                    .bg(palette.tab_drop_marker_color)
            }
            TabStripOrientation::Vertical => {
                // Horizontal bar at the top (Leading) or bottom (Trailing) edge.
                let marker_y = match side {
                    TabDropMarkerSide::Leading => 0.0,
                    TabDropMarkerSide::Trailing => input.tab_primary_extent - TAB_DROP_MARKER_WIDTH,
                }
                .max(0.0);
                let marker_width =
                    (input.tab_cross_extent - (TAB_DROP_MARKER_INSET_Y * 2.0)).max(0.0);

                div()
                    .absolute()
                    .top(px(marker_y))
                    .left(px(TAB_DROP_MARKER_INSET_Y))
                    .h(px(TAB_DROP_MARKER_WIDTH))
                    .w(px(marker_width))
                    .rounded_full()
                    .bg(palette.tab_drop_marker_color)
            }
        });

        let leading_slot = if !input.is_renaming {
            Some(
                div()
                    .flex_none()
                    .w(px(TAB_LEADING_SLOT_WIDTH))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_start()
                    .children(Self::render_progress_dot(
                        &input.progress_state,
                        colors,
                        anim,
                    ))
                    .into_any_element(),
            )
        } else {
            None
        };

        let trailing_accessory = if Self::tab_accessory_visible(&input) {
            Some(self.render_tab_accessory(
                &input,
                palette,
                close_text_color,
                hover_tab_index,
                close_tab_index,
                cx,
            ))
        } else {
            None
        };

        let tab_shell =
            tab_shell
                .children(leading_slot)
                .child(div().flex_1().min_w(px(0.0)).h_full().relative().child(
                    if input.is_renaming {
                        let rename_alignment = if justify_label_center {
                            InlineInputAlignment::Center
                        } else {
                            InlineInputAlignment::Left
                        };
                        self.render_inline_input_layer(
                            Font {
                                family: font_family.clone(),
                                weight: FontWeight::NORMAL,
                                ..gpui::font("")
                            },
                            px(title_font_size),
                            rename_text_color.into(),
                            rename_selection_color.into(),
                            rename_alignment,
                            cx,
                        )
                    } else {
                        let mut title_text = div()
                            .size_full()
                            .flex()
                            .items_center()
                            .overflow_x_hidden()
                            .whitespace_nowrap()
                            .font_family(font_family.clone())
                            .text_color(rename_text_color)
                            .text_size(px(title_font_size))
                            .text_ellipsis();
                        if justify_label_center {
                            title_text = title_text.justify_center();
                        }
                        title_text.child(input.label).into_any_element()
                    },
                ))
                .children(trailing_accessory)
                .children(drop_marker);

        if orientation == TabStripOrientation::Horizontal {
            return div()
                .flex_none()
                .relative()
                .w(px(tab_primary_extent))
                .h(px(TABBAR_HEIGHT))
                .flex()
                .items_center()
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        window.prevent_default();
                        this.on_tab_mouse_down(
                            orientation,
                            switch_tab_index,
                            event.click_count,
                            window,
                            cx,
                        );
                        cx.stop_propagation();
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        window.prevent_default();
                        this.open_tab_context_menu_for_window(
                            switch_tab_index,
                            event.position,
                            window,
                            cx,
                        );
                        cx.stop_propagation();
                    }),
                )
                .on_mouse_move(
                    cx.listener(move |this, event: &MouseMoveEvent, window, cx| {
                        this.on_tab_mouse_move(orientation, hover_tab_index, event, window, cx);
                        cx.stop_propagation();
                    }),
                )
                .child(tab_shell)
                .into_any_element();
        }

        tab_shell.into_any_element()
    }

    fn render_progress_dot(
        state: &ProgressState,
        colors: &TerminalColors,
        anim: f32,
    ) -> Option<impl IntoElement> {
        if !state.is_active() {
            return None;
        }
        let mut bg_color = match state {
            ProgressState::InProgress(_) => colors.ansi[2], // green
            ProgressState::Error(_) => colors.ansi[1],      // red
            ProgressState::Warning(_) => colors.ansi[3],    // yellow
            ProgressState::Indeterminate => colors.ansi[4], // blue
            ProgressState::Clear => return None,
        };
        bg_color.a *= anim;
        Some(
            div()
                .flex_none()
                .w(px(TAB_PROGRESS_BADGE_SIZE))
                .h(px(TAB_PROGRESS_BADGE_SIZE))
                .rounded_full()
                .bg(bg_color),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab_item_input(is_renaming: bool, close_slot_width: f32) -> TabItemRenderInput {
        TabItemRenderInput {
            orientation: TabStripOrientation::Horizontal,
            index: 0,
            tab_primary_extent: TAB_MIN_WIDTH,
            tab_cross_extent: TAB_ITEM_HEIGHT,
            label: String::new(),
            switch_hint_label: None,
            is_active: true,
            is_drag_source: false,
            is_pane_drop_target: false,
            is_renaming,
            show_tab_close: true,
            show_tab_pin: false,
            close_slot_width,
            text_padding_x: TAB_TEXT_PADDING_X,
            label_centered: true,
            drop_marker_side: None,
            open_anim_progress: None,
            hover_progress: 0.0,
            progress_state: ProgressState::default(),
        }
    }

    #[test]
    fn tab_accessory_hides_while_renaming() {
        assert!(!TerminalView::tab_accessory_visible(&tab_item_input(
            true,
            TAB_CLOSE_SLOT_WIDTH,
        )));
    }

    #[test]
    fn tab_accessory_shows_with_slot_when_not_renaming() {
        assert!(TerminalView::tab_accessory_visible(&tab_item_input(
            false,
            TAB_CLOSE_SLOT_WIDTH,
        )));
    }
}
