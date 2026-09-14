use super::super::*;
use super::hints::TabSwitchHintState;
use super::render_palette::TabStripPalette;
use super::render_tab_item::TabItemRenderInput;
use super::state::TabStripOrientation;

impl TerminalView {
    /// Toggle the right-side tab sidebar between expanded and collapsed (a thin
    /// rail). Marks the layout dirty so the terminal grid re-sizes to reclaim /
    /// yield the freed width on the next frame.
    pub(crate) fn toggle_sidebar_collapsed(&mut self, cx: &mut Context<Self>) {
        self.sidebar_collapsed = !self.sidebar_collapsed;
        self.mark_tab_strip_layout_dirty();
        cx.notify();
    }

    /// Render the vertical tab strip as a right-side sidebar column. Used when
    /// `tab_bar_position == Right`.
    pub(crate) fn render_tab_sidebar(
        &mut self,
        window: &Window,
        colors: &TerminalColors,
        font_family: &SharedString,
        sidebar_bg: gpui::Rgba,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.resolve_tab_strip_palette(colors, sidebar_bg);

        if self.sidebar_collapsed {
            return self.render_collapsed_sidebar_rail(&palette, cx);
        }

        // The scrollable tab region sits below the titlebar inset and the header.
        // Record its height so vertical scroll / scroll-into-view math is correct.
        let viewport_height: f32 = window.viewport_size().height.into();
        let tabs_viewport_height =
            (viewport_height - self.terminal_content_top_inset() - SIDEBAR_HEADER_HEIGHT).max(0.0);
        self.tab_strip.vertical_layout_last_synced_viewport_height = tabs_viewport_height;
        self.scroll_active_tab_into_view(TabStripOrientation::Vertical);

        let header = self.render_sidebar_header(&palette, cx);
        let tabs = self.build_sidebar_tabs_content(window, &palette, font_family, colors, cx);

        div()
            .id("tab-sidebar")
            .flex_none()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(palette.tab_stroke_color)
            .child(header)
            .child(
                div()
                    .id("sidebar-tabs-viewport")
                    .flex_1()
                    // min_h(0) lets the flex child shrink so overflow_y_scroll
                    // has a bounded height to scroll within.
                    .min_h(px(0.0))
                    .w_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.tab_strip.vertical_scroll_handle)
                    .child(tabs),
            )
            .into_any_element()
    }

    fn render_collapsed_sidebar_rail(
        &mut self,
        palette: &TabStripPalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id("tab-sidebar-collapsed")
            .flex_none()
            .w(px(SIDEBAR_COLLAPSED_WIDTH))
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .border_l_1()
            .border_color(palette.tab_stroke_color)
            .child(self.sidebar_icon_button(
                "sidebar-expand-button",
                "icons/sidebar/expand.svg",
                palette,
                cx,
                |this, cx| this.toggle_sidebar_collapsed(cx),
            ))
            .into_any_element()
    }

    fn render_sidebar_header(
        &mut self,
        palette: &TabStripPalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id("sidebar-header")
            .flex_none()
            .w_full()
            .h(px(SIDEBAR_HEADER_HEIGHT))
            .flex()
            .items_center()
            .justify_between()
            .px(px(6.0))
            .border_b_1()
            .border_color(palette.tab_stroke_color)
            .child(self.sidebar_icon_button(
                "sidebar-collapse-button",
                "icons/sidebar/collapse.svg",
                palette,
                cx,
                |this, cx| this.toggle_sidebar_collapsed(cx),
            ))
            .child({
                // Anchor the new-tab kind dropdown just below the header,
                // right-aligned within the sidebar column.
                let menu_anchor = (
                    (self.last_viewport_width - NEW_TAB_MENU_WIDTH - 8.0).max(4.0),
                    self.terminal_content_top_inset() + SIDEBAR_HEADER_HEIGHT + 2.0,
                );
                self.sidebar_icon_button(
                    "sidebar-new-tab-button",
                    "icons/tab_strip/plus.svg",
                    palette,
                    cx,
                    move |this, cx| this.handle_new_tab_button(menu_anchor, cx),
                )
            })
            .into_any_element()
    }

    fn sidebar_icon_button(
        &self,
        id: &'static str,
        icon_path: &'static str,
        palette: &TabStripPalette,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        let mut button_bg = palette.hovered_tab_bg;
        button_bg.a = 0.0;
        let mut button_hover_bg = palette.hovered_tab_bg;
        button_hover_bg.a = (button_hover_bg.a * 1.45).min(1.0);
        let mut icon_color = palette.inactive_tab_text;
        icon_color.a = icon_color.a.max(0.70);

        div()
            .id(id)
            .w(px(TABBAR_NEW_TAB_BUTTON_SIZE.min(SIDEBAR_HEADER_HEIGHT)))
            .h(px(TABBAR_NEW_TAB_BUTTON_SIZE.min(SIDEBAR_HEADER_HEIGHT)))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(TAB_ITEM_RADIUS))
            .bg(button_bg)
            .text_color(icon_color)
            .hover(move |style| style.bg(button_hover_bg))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _event: &MouseDownEvent, window, cx| {
                    window.prevent_default();
                    on_click(this, cx);
                    cx.stop_propagation();
                }),
            )
            .child(
                gpui::svg()
                    .path(gpui::SharedString::from(icon_path))
                    .size(px(13.0))
                    .text_color(icon_color),
            )
            .into_any_element()
    }

    fn build_sidebar_tabs_content(
        &mut self,
        window: &Window,
        palette: &TabStripPalette,
        font_family: &SharedString,
        colors: &TerminalColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let font_family_key = font_family.as_ref();
        let now = Instant::now();
        let tab_row_width = SIDEBAR_WIDTH - (SIDEBAR_TAB_MARGIN_X * 2.0);
        let mut content = div()
            .id("sidebar-tabs-content")
            .flex()
            .flex_col()
            .w_full()
            .px(px(SIDEBAR_TAB_MARGIN_X))
            .py(px(SIDEBAR_TAB_PADDING_Y))
            .gap(px(SIDEBAR_TAB_ROW_GAP))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                this.on_tabs_content_mouse_move(TabStripOrientation::Vertical, event, window, cx);
            }));

        for index in 0..self.session.tabs.len() {
            let (tab_title, pinned, progress_state) = {
                let tab = &self.session.tabs[index];
                (
                    tab.title.clone(),
                    tab.pinned,
                    tab.aggregate_progress_state(),
                )
            };
            let is_active = index == self.session.active_tab;
            let is_drag_source = self
                .tab_strip
                .drag
                .is_some_and(|drag| drag.source_index == index);
            let show_close_button = Self::tab_shows_close(
                self.tab_close_visibility,
                is_active,
                self.tab_strip.hovered_tab,
                self.tab_strip.hovered_tab_close,
                index,
            );
            let is_renaming = self.renaming_tab == Some(index);
            let show_switch_hint = self.tab_strip.switch_hints.should_render(
                index,
                is_renaming,
                self.tab_switch_hints_blocked(),
                now,
            );
            let switch_hint_label = show_switch_hint
                .then(|| TabSwitchHintState::label_for_index(index))
                .flatten();
            let show_tab_close = !is_renaming && show_close_button && switch_hint_label.is_none();
            let show_tab_pin = !is_renaming && pinned && switch_hint_label.is_none();
            let close_slot_width = if !is_renaming
                && (show_tab_close || show_tab_pin || switch_hint_label.is_some())
            {
                TAB_CLOSE_SLOT_WIDTH
            } else {
                0.0
            };
            let icon_slot_width = if is_renaming {
                0.0
            } else {
                TAB_LEADING_SLOT_WIDTH
            };
            let reserves_close_slot = Self::tab_reserves_close_slot_for_layout(
                self.tab_width_mode,
                self.tab_close_visibility,
                is_active,
                pinned,
            );
            let text_reserve_slot_width = if is_renaming {
                0.0
            } else if reserves_close_slot {
                TAB_CLOSE_SLOT_WIDTH
            } else {
                close_slot_width
            };
            let available_text_px = Self::tab_title_text_area_width(
                tab_row_width,
                text_reserve_slot_width + icon_slot_width,
            );
            let label = Self::format_tab_label_for_render_measured(
                &tab_title,
                available_text_px,
                |candidate| {
                    self.measure_tab_title_width(window, font_family, font_family_key, candidate)
                },
            );

            let tab_item = self.render_tab_item(
                TabItemRenderInput {
                    orientation: TabStripOrientation::Vertical,
                    index,
                    tab_primary_extent: SIDEBAR_TAB_ROW_HEIGHT,
                    tab_cross_extent: tab_row_width,
                    label,
                    switch_hint_label,
                    is_active,
                    is_drag_source,
                    is_pane_drop_target: self.pane_move_targets_tab(self.session.tabs[index].id),
                    is_renaming,
                    show_tab_close,
                    show_tab_pin,
                    close_slot_width,
                    text_padding_x: TAB_TEXT_PADDING_X,
                    label_centered: false,
                    drop_marker_side: self.tab_drop_marker_side(index),
                    open_anim_progress: None,
                    hover_progress: self.tab_strip.hover_progress(index, now),
                    progress_state,
                },
                font_family,
                colors,
                palette,
                cx,
            );

            content = content.child(tab_item);
        }

        content.into_any_element()
    }
}
