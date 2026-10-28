//! Client-side titlebar pieces for Windows.
//!
//! The Termy window hides the OS titlebar there (`appears_transparent`), so
//! the tab strip doubles as the caption. GPUI reports elements tagged with a
//! `WindowControlArea` to Windows' non-client hit test: empty strip space
//! answers `HTCAPTION` (native drag, Aero Snap, double-click maximize/restore,
//! system menu) and the buttons drawn here answer `HTMINBUTTON`/`HTMAXBUTTON`/
//! `HTCLOSE`, which also brings the Windows 11 snap layout flyout to the
//! maximize button.

use super::super::*;
#[cfg(any(target_os = "windows", test))]
use super::layout::TabStripGeometry;

/// What the client-side titlebar shows at a point, as far as the OS move
/// logic is concerned.
#[cfg(any(target_os = "windows", test))]
#[derive(Clone, Copy, Debug)]
pub(crate) struct WindowsTitlebarDragInput {
    /// The horizontal tab strip is rendered in the titlebar row. Otherwise
    /// the row holds only the full-width drag fill and the caption buttons.
    pub(crate) tabs_row_rendered: bool,
    /// The left inset is a drag surface (it holds no workspace actions).
    pub(crate) left_inset_draggable: bool,
    pub(crate) caption_buttons_width: f32,
}

impl TerminalView {
    /// Whether a titlebar press at `x`/`y` lands on an element tagged
    /// `WindowControlArea::Drag`. Windows then moves the window itself, so
    /// Termy must let the press through instead of handling it.
    #[cfg(any(target_os = "windows", test))]
    pub(crate) fn windows_titlebar_os_drag_hit_test(
        x: f32,
        y: f32,
        input: WindowsTitlebarDragInput,
        geometry: TabStripGeometry,
    ) -> bool {
        if x < 0.0 || x >= geometry.window_width - input.caption_buttons_width {
            return false;
        }
        if !input.tabs_row_rendered {
            return true;
        }
        if !(TOP_STRIP_CONTENT_OFFSET_Y..TOP_STRIP_CONTENT_OFFSET_Y + TABBAR_HEIGHT).contains(&y) {
            return false;
        }
        x >= geometry.row_end_x || (input.left_inset_draggable && x < geometry.row_start_x)
    }

    #[cfg(target_os = "windows")]
    pub(crate) fn windows_titlebar_drag_input(&self, window: &Window) -> WindowsTitlebarDragInput {
        let geometry = self.tab_strip_geometry(window);
        WindowsTitlebarDragInput {
            tabs_row_rendered: self.should_render_tab_strip_chrome()
                && self.tab_strip_orientation()
                    == crate::terminal_view::tab_strip::state::TabStripOrientation::Horizontal,
            left_inset_draggable: !self.workspace_sidebar_visible(),
            caption_buttons_width: geometry
                .right_inset_width
                .min(Self::titlebar_right_padding_for_platform()),
        }
    }

    #[cfg(target_os = "windows")]
    #[allow(clippy::too_many_arguments)]
    fn render_windows_caption_button(
        id: &'static str,
        glyph: &'static str,
        area: gpui_kit::WindowControlArea,
        font_family: &'static str,
        glyph_color: gpui_kit::Rgba,
        hover_bg: gpui_kit::Rgba,
        pressed_bg: gpui_kit::Rgba,
        hover_glyph_color: gpui_kit::Rgba,
    ) -> AnyElement {
        div()
            .id(id)
            .flex_none()
            .w(px(WINDOWS_CAPTION_BUTTON_WIDTH))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .font_family(font_family)
            .text_size(px(WINDOWS_CAPTION_BUTTON_GLYPH_SIZE))
            .text_color(glyph_color)
            .window_control_area(area)
            .hover(move |style| style.bg(hover_bg).text_color(hover_glyph_color))
            .active(move |style| style.bg(pressed_bg).text_color(hover_glyph_color))
            .child(glyph)
            .into_any_element()
    }

    /// Minimize, maximize/restore and close, right-aligned in `width`. The OS
    /// performs the actions; GPUI only reports the hit areas.
    #[cfg(target_os = "windows")]
    pub(super) fn render_windows_caption_buttons(
        window: &Window,
        foreground: gpui_kit::Rgba,
        width: f32,
    ) -> AnyElement {
        use gpui_kit::WindowControlArea;

        // Segoe Fluent Icons replaces Segoe MDL2 Assets on Windows 11; both
        // carry the Chrome* caption glyphs at the same code points.
        let font_family = if crate::native_sdk::win32_window::is_windows_11() {
            "Segoe Fluent Icons"
        } else {
            "Segoe MDL2 Assets"
        };
        let mut glyph_color = foreground;
        glyph_color.a = if window.is_window_active() {
            0.92
        } else {
            0.46
        };
        let mut hover_bg = foreground;
        hover_bg.a = 0.09;
        let mut pressed_bg = foreground;
        pressed_bg.a = 0.06;
        let mut hover_glyph_color = foreground;
        hover_glyph_color.a = 1.0;
        let close_hover_bg = gpui_kit::rgb(0xC42B1C);
        let close_pressed_bg = gpui_kit::rgba(0xC42B1CE6);
        let close_hover_glyph_color = gpui_kit::rgb(0xFFFFFF);
        let maximize_glyph = if window.is_maximized() {
            "\u{E923}"
        } else {
            "\u{E922}"
        };

        div()
            .id("windows-caption-buttons")
            .flex_none()
            .w(px(width.max(0.0)))
            .h_full()
            .overflow_hidden()
            .flex()
            .justify_end()
            .child(Self::render_windows_caption_button(
                "windows-caption-minimize",
                "\u{E921}",
                WindowControlArea::Min,
                font_family,
                glyph_color,
                hover_bg,
                pressed_bg,
                hover_glyph_color,
            ))
            .child(Self::render_windows_caption_button(
                "windows-caption-maximize",
                maximize_glyph,
                WindowControlArea::Max,
                font_family,
                glyph_color,
                hover_bg,
                pressed_bg,
                hover_glyph_color,
            ))
            .child(Self::render_windows_caption_button(
                "windows-caption-close",
                "\u{E8BB}",
                WindowControlArea::Close,
                font_family,
                glyph_color,
                close_hover_bg,
                close_pressed_bg,
                close_hover_glyph_color,
            ))
            .into_any_element()
    }

    /// Trailing lane of the horizontal strip on Windows: free space that drags
    /// the window, then the caption buttons.
    #[cfg(target_os = "windows")]
    pub(super) fn render_windows_right_inset_lane(
        window: &Window,
        foreground: gpui_kit::Rgba,
        width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let caption_buttons_width = width.min(Self::titlebar_right_padding_for_platform());
        div()
            .id("tabbar-right-inset")
            .relative()
            .flex_none()
            .w(px(width))
            .h_full()
            .flex()
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                this.on_action_rail_mouse_move(event, window, cx);
            }))
            .child(
                div()
                    .id("tabbar-right-drag-surface")
                    .flex_1()
                    .h_full()
                    .window_control_area(gpui_kit::WindowControlArea::Drag),
            )
            .child(Self::render_windows_caption_buttons(
                window,
                foreground,
                caption_buttons_width,
            ))
            .into_any_element()
    }

    /// Caption controls for the titlebar row when it does not hold the
    /// horizontal tab strip (vertical tabs, or the strip is hidden): a
    /// full-width drag surface plus the caption buttons, laid over the row.
    /// `None` on other platforms, which do not draw their own caption.
    #[cfg_attr(target_os = "windows", allow(clippy::unnecessary_wraps))]
    pub(crate) fn render_windows_titlebar_controls(
        &self,
        window: &Window,
        colors: &TerminalColors,
    ) -> Option<AnyElement> {
        #[cfg(target_os = "windows")]
        {
            Some(
                div()
                    .id("windows-titlebar-controls")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .child(
                        div()
                            .id("windows-titlebar-drag-surface")
                            .flex_1()
                            .h_full()
                            .window_control_area(gpui_kit::WindowControlArea::Drag),
                    )
                    .child(Self::render_windows_caption_buttons(
                        window,
                        colors.foreground,
                        Self::titlebar_right_padding_for_platform(),
                    ))
                    .into_any_element(),
            )
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (window, colors);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAPTION_WIDTH: f32 = 138.0;

    fn geometry() -> TabStripGeometry {
        TerminalView::tab_strip_layout_for_viewport_with_left_inset_and_content_width(
            1280.0,
            TerminalView::titlebar_left_padding_for_platform(),
            192.0,
        )
        .geometry
    }

    fn input(tabs_row_rendered: bool, left_inset_draggable: bool) -> WindowsTitlebarDragInput {
        WindowsTitlebarDragInput {
            tabs_row_rendered,
            left_inset_draggable,
            caption_buttons_width: CAPTION_WIDTH,
        }
    }

    fn strip_y() -> f32 {
        TOP_STRIP_CONTENT_OFFSET_Y + TABBAR_HEIGHT * 0.5
    }

    #[test]
    fn trailing_free_space_is_an_os_drag_surface() {
        let geometry = geometry();
        let x = (geometry.row_end_x + geometry.window_width - CAPTION_WIDTH) * 0.5;
        assert!(TerminalView::windows_titlebar_os_drag_hit_test(
            x,
            strip_y(),
            input(true, true),
            geometry,
        ));
    }

    #[test]
    fn caption_buttons_are_not_drag_surfaces() {
        let geometry = geometry();
        let x = geometry.window_width - CAPTION_WIDTH * 0.5;
        assert!(!TerminalView::windows_titlebar_os_drag_hit_test(
            x,
            strip_y(),
            input(true, true),
            geometry,
        ));
        assert!(!TerminalView::windows_titlebar_os_drag_hit_test(
            x,
            strip_y(),
            input(false, true),
            geometry,
        ));
    }

    #[test]
    fn tabs_and_action_rail_stay_with_termy() {
        let geometry = geometry();
        for x in [
            geometry.row_start_x + 1.0,
            (geometry.button_start_x + geometry.button_end_x) * 0.5,
        ] {
            assert!(!TerminalView::windows_titlebar_os_drag_hit_test(
                x,
                strip_y(),
                input(true, true),
                geometry,
            ));
        }
    }

    #[test]
    fn left_inset_drags_only_without_workspace_actions() {
        let geometry = geometry();
        let x = geometry.row_start_x * 0.5;
        assert!(TerminalView::windows_titlebar_os_drag_hit_test(
            x,
            strip_y(),
            input(true, true),
            geometry,
        ));
        assert!(!TerminalView::windows_titlebar_os_drag_hit_test(
            x,
            strip_y(),
            input(true, false),
            geometry,
        ));
    }

    #[test]
    fn titlebar_without_tab_row_drags_everywhere_but_the_caption() {
        let geometry = geometry();
        assert!(TerminalView::windows_titlebar_os_drag_hit_test(
            geometry.row_start_x + 1.0,
            strip_y(),
            input(false, false),
            geometry,
        ));
    }
}
