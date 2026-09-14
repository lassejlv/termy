use super::super::*;
use super::hints::TabSwitchHintState;
use super::render_palette::{TabStripPalette, resolve_branding_text_color};
use super::render_shared::TabStripRenderState;
use super::render_tab_item::TabItemRenderInput;
use super::state::{TabStripOrientation, TabStripOverflowState};

impl TerminalView {
    fn render_inset_lane(id: &'static str, width: f32, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id(id)
            .relative()
            .flex_none()
            .w(px(width))
            .h_full()
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                this.on_action_rail_mouse_move(event, window, cx);
            }))
            .into_any_element()
    }

    pub(super) fn render_termy_branding(
        font_family: &SharedString,
        termy_branding_slot_start_x: f32,
        termy_branding_slot_width: f32,
        termy_branding_text_color: gpui::Rgba,
    ) -> Option<AnyElement> {
        (termy_branding_slot_width > f32::EPSILON).then(|| {
            div()
                .id("tabbar-termy-branding")
                .absolute()
                .left(px(termy_branding_slot_start_x.max(0.0)))
                .top_0()
                .bottom_0()
                .w(px(termy_branding_slot_width.max(0.0)))
                .overflow_hidden()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .font_family(font_family.clone())
                        .text_size(px(TOP_STRIP_TERMY_BRANDING_FONT_SIZE))
                        .text_color(termy_branding_text_color)
                        .child(TOP_STRIP_TERMY_BRANDING_TEXT),
                )
                .into_any_element()
        })
    }

    pub(crate) fn render_titlebar_branding(
        &mut self,
        window: &Window,
        colors: &TerminalColors,
        font_family: &SharedString,
        tabbar_bg: gpui::Rgba,
        _show_sidebar_chrome: bool,
        _cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let font_family_key = font_family.as_ref();
        let branding_width =
            self.termy_branding_reserved_width(window, font_family, font_family_key);
        if branding_width <= f32::EPSILON {
            return None;
        }

        let leading_inset_width = Self::titlebar_left_padding_for_platform();
        let lane_width = leading_inset_width + branding_width + TOP_STRIP_TERMY_BRANDING_TAB_GAP;
        let palette = self.resolve_tab_strip_palette(colors, tabbar_bg);
        let branding_text_color = resolve_branding_text_color(&palette);

        Some(
            div()
                .id("titlebar-branding-slot")
                .relative()
                .flex_none()
                .w(px(lane_width))
                .h(px(TABBAR_HEIGHT))
                .children(Self::render_termy_branding(
                    font_family,
                    leading_inset_width,
                    branding_width,
                    branding_text_color,
                ))
                .into_any_element(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_left_inset_lane(
        width: f32,
        font_family: &SharedString,
        termy_branding_slot_start_x: f32,
        termy_branding_slot_width: f32,
        termy_branding_text_color: gpui::Rgba,
        workspace_actions: Option<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id("tabbar-left-inset")
            .relative()
            .flex_none()
            .w(px(width))
            .h_full()
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                this.on_action_rail_mouse_move(event, window, cx);
            }))
            .children(Self::render_termy_branding(
                font_family,
                termy_branding_slot_start_x,
                termy_branding_slot_width,
                termy_branding_text_color,
            ))
            .children(workspace_actions)
            .into_any_element()
    }

    fn render_gutter_lane(gutter_width: f32, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("tabbar-action-gutter")
            .relative()
            .flex_none()
            .w(px(gutter_width))
            .h_full()
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                this.on_action_rail_mouse_move(event, window, cx);
            }))
            .into_any_element()
    }

    fn should_render_left_inset_divider(overflow: TabStripOverflowState) -> bool {
        overflow.left
    }

    fn horizontal_tab_render_width(display_width: f32, anim_progress: Option<f32>) -> f32 {
        let stable_width = if display_width.is_finite() {
            display_width.max(TAB_MIN_WIDTH)
        } else {
            TAB_MIN_WIDTH
        };

        anim_progress.map_or(stable_width, |progress| {
            (stable_width * progress.clamp(0.0, 1.0)).max(TAB_MIN_WIDTH)
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn build_tabs_scroll_content(
        &mut self,
        window: &Window,
        state: &TabStripRenderState,
        palette: &TabStripPalette,
        font_family: &SharedString,
        font_family_key: &str,
        colors: &TerminalColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let now = Instant::now();
        let new_tab_anim = self.new_tab_animation_progress(now);
        let mut tabs_scroll_content = div()
            .id("tabs-scroll-content")
            .flex_none()
            .w(px(state.content_width))
            .min_w(px(state.content_width))
            .h(px(TABBAR_HEIGHT))
            .flex()
            .relative()
            .items_center()
            .gap(px(TAB_ITEM_GAP))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                this.on_tabs_content_mouse_move(TabStripOrientation::Horizontal, event, window, cx);
            }));

        tabs_scroll_content = tabs_scroll_content.child(
            div()
                .id("tabs-left-padding-spacer")
                .flex_none()
                .w(px(TAB_HORIZONTAL_PADDING))
                .h(px(TABBAR_HEIGHT)),
        );

        for index in 0..self.session.tabs.len() {
            let (display_width, tab_title, pinned, progress_state) = {
                let tab = &self.session.tabs[index];
                (
                    tab.display_width,
                    tab.title.clone(),
                    tab.pinned,
                    tab.aggregate_progress_state(),
                )
            };
            let anim_progress = new_tab_anim
                .filter(|(anim_index, _)| *anim_index == index)
                .map(|(_, p)| p);
            let tab_width = Self::horizontal_tab_render_width(display_width, anim_progress);
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
            // Reserve the close-slot width in the TEXT area whenever the layout
            // already reserved it (Uniform/Stable modes / pinned), independent of
            // hover. Otherwise the title re-truncates and visibly shifts when the
            // close button fades in on hover even though the tab width is fixed.
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
                tab_width,
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
                    orientation: TabStripOrientation::Horizontal,
                    index,
                    tab_primary_extent: tab_width,
                    tab_cross_extent: TAB_ITEM_HEIGHT,
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
                    open_anim_progress: anim_progress,
                    hover_progress: self.tab_strip.hover_progress(index, now),
                    progress_state,
                },
                font_family,
                colors,
                palette,
                cx,
            );

            tabs_scroll_content = tabs_scroll_content.child(tab_item);
        }

        tabs_scroll_content.into_any_element()
    }

    fn render_action_rail(
        &mut self,
        state: &TabStripRenderState,
        palette: &TabStripPalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut button_bg = palette.hovered_tab_bg;
        button_bg.a = 0.0;
        let mut button_hover_bg = palette.hovered_tab_bg;
        button_hover_bg.a = (button_hover_bg.a * 1.45).min(1.0);
        let mut icon_color = palette.inactive_tab_text;
        icon_color.a = icon_color.a.max(0.70);

        // Anchor for the new-tab kind dropdown: right-aligned under the "+"
        // so it never overflows the window edge.
        let menu_anchor = (
            (state.geometry.button_end_x - NEW_TAB_MENU_WIDTH).max(4.0),
            TABBAR_HEIGHT + 2.0,
        );

        div()
            .id("tabbar-action-rail")
            .relative()
            .flex_none()
            .w(px(state.geometry.action_rail_width))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .on_scroll_wheel(cx.listener(Self::handle_tab_strip_action_rail_scroll_wheel))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                this.on_action_rail_mouse_move(event, window, cx);
            }))
            .child(
                div()
                    .id("tabbar-new-tab-button")
                    .w_full()
                    .h(px(TABBAR_NEW_TAB_BUTTON_SIZE.min(TABBAR_HEIGHT)))
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
                            this.handle_new_tab_button(menu_anchor, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .child(
                        gpui::svg()
                            .path(gpui::SharedString::from("icons/tab_strip/plus.svg"))
                            .size(px(13.0))
                            .text_color(icon_color),
                    ),
            )
            .into_any_element()
    }

    pub(crate) fn render_tab_strip(
        &mut self,
        window: &Window,
        colors: &TerminalColors,
        font_family: &SharedString,
        tabbar_bg: gpui::Rgba,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let font_family_key = font_family.as_ref();
        let measured_title_widths =
            self.measure_tab_title_widths(window, font_family, font_family_key);
        self.sync_tab_title_text_widths(&measured_title_widths);

        let base_left_inset_width = self.tab_strip_base_left_inset();
        let workspace_sidebar_visible = self.workspace_sidebar_visible();
        // With the workspace sidebar, the inset lane hosts its actions and
        // the first tab stays flush with the sidebar edge, so the titlebar
        // branding is suppressed.
        let termy_branding_reserved_width = if workspace_sidebar_visible {
            0.0
        } else {
            self.termy_branding_reserved_width(window, font_family, font_family_key)
        };
        let termy_branding_tab_gap = if termy_branding_reserved_width > f32::EPSILON {
            TOP_STRIP_TERMY_BRANDING_TAB_GAP
        } else {
            0.0
        };
        let state = self.build_tab_strip_render_state(
            window,
            base_left_inset_width + termy_branding_reserved_width + termy_branding_tab_gap,
        );
        let palette = self.resolve_tab_strip_palette(colors, tabbar_bg);
        let termy_branding_slot_start_x =
            base_left_inset_width.min(state.geometry.left_inset_width);
        let termy_branding_slot_width = (state.geometry.left_inset_width
            - termy_branding_slot_start_x)
            .max(0.0)
            .min(termy_branding_reserved_width.max(0.0));
        let termy_branding_text_color =
            super::render_palette::resolve_branding_text_color(&palette);
        let tabs_scroll_content = self.build_tabs_scroll_content(
            window,
            &state,
            &palette,
            font_family,
            font_family_key,
            colors,
            cx,
        );
        let show_left_inset_divider = Self::should_render_left_inset_divider(state.overflow_state);
        let workspace_actions = workspace_sidebar_visible
            .then(|| self.render_workspace_sidebar_titlebar_actions(&palette, cx));

        div()
            .w_full()
            .h(px(TABBAR_HEIGHT))
            .flex()
            .border_b_1()
            .border_color(palette.tab_stroke_color)
            .children((state.geometry.left_inset_width > 0.0).then(|| {
                Self::render_left_inset_lane(
                    state.geometry.left_inset_width,
                    font_family,
                    termy_branding_slot_start_x,
                    termy_branding_slot_width,
                    termy_branding_text_color,
                    workspace_actions,
                    cx,
                )
            }))
            .child(
                div()
                    .id("tabs-scroll-viewport-lane")
                    .flex_none()
                    .w(px(state.geometry.tabs_viewport_width))
                    .min_w(px(0.0))
                    .h_full()
                    .relative()
                    .child(
                        div()
                            .id("tabs-scroll-viewport")
                            .absolute()
                            .left_0()
                            .top_0()
                            .right_0()
                            .bottom_0()
                            // Native overflow scrolling drives the shared scroll
                            // handle (and maps vertical wheel -> horizontal). Do NOT
                            // also attach a manual on_scroll_wheel here: it would
                            // double-apply the delta against the same handle and the
                            // opposing sign conventions cancel out, making the strip
                            // appear unscrollable. The "+" action rail keeps its own
                            // manual handler since it sits outside this viewport.
                            .overflow_x_scroll()
                            .track_scroll(&self.tab_strip.horizontal_scroll_handle)
                            .child(tabs_scroll_content),
                    )
                    .children(show_left_inset_divider.then(|| {
                        div()
                            .absolute()
                            .left_0()
                            .top(px(TAB_STROKE_THICKNESS))
                            .bottom_0()
                            .w(px(TAB_STROKE_THICKNESS))
                            .bg(palette.tab_stroke_color)
                    })),
            )
            .children(
                (state.geometry.gutter_width > 0.0)
                    .then(|| Self::render_gutter_lane(state.geometry.gutter_width, cx)),
            )
            .children(
                (state.geometry.action_rail_width > 0.0)
                    .then(|| self.render_action_rail(&state, &palette, cx)),
            )
            .children((state.geometry.right_inset_width > 0.0).then(|| {
                Self::render_inset_lane("tabbar-right-inset", state.geometry.right_inset_width, cx)
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn left_inset_divider_hides_without_left_overflow() {
        assert!(!TerminalView::should_render_left_inset_divider(
            TabStripOverflowState {
                left: false,
                right: true,
            },
        ));
        assert!(!TerminalView::should_render_left_inset_divider(
            TabStripOverflowState {
                left: false,
                right: false,
            },
        ));
    }

    #[test]
    fn left_inset_divider_shows_when_left_overflow_exists() {
        assert!(TerminalView::should_render_left_inset_divider(
            TabStripOverflowState {
                left: true,
                right: true,
            },
        ));
        assert!(TerminalView::should_render_left_inset_divider(
            TabStripOverflowState {
                left: true,
                right: false,
            },
        ));
    }

    #[test]
    fn horizontal_tab_render_width_never_drops_below_minimum() {
        assert_eq!(
            TerminalView::horizontal_tab_render_width(TAB_MAX_WIDTH, Some(0.0)),
            TAB_MIN_WIDTH
        );
        assert_eq!(
            TerminalView::horizontal_tab_render_width(TAB_MAX_WIDTH, Some(0.1)),
            TAB_MIN_WIDTH
        );
        assert_eq!(
            TerminalView::horizontal_tab_render_width(12.0, None),
            TAB_MIN_WIDTH
        );
    }

    #[test]
    fn horizontal_tab_render_width_keeps_full_width_without_animation() {
        assert_eq!(
            TerminalView::horizontal_tab_render_width(TAB_MAX_WIDTH, None),
            TAB_MAX_WIDTH
        );
    }
}
