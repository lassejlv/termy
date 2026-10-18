//! Visual styles of the horizontal tab strip.
//!
//! `Termy` is Termy's own chip strip. `Windows11` is what `tab_style = native`
//! means on Windows, which has no OS tab API: the same strip drawn in the
//! WinUI `TabView` look of Windows Terminal, Explorer and Notepad. Tabs sit
//! flush on the bottom edge of the titlebar, the selected tab takes the
//! terminal surface color and flares into the content below with concave
//! "ears", and unselected tabs are divided by thin separators.
//!
//! Both styles share the strip's behavior (drag reorder, rename, close,
//! overflow scrolling, animations); only geometry, palette and item
//! decoration differ.

use super::super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum HorizontalTabStyle {
    #[default]
    Termy,
    Windows11,
}

/// Geometry of tabs on the horizontal strip that differs between styles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HorizontalTabMetrics {
    /// Space before the first tab inside the scrollable tab lane.
    pub(crate) leading_padding: f32,
    /// Space between adjacent tabs.
    pub(crate) item_gap: f32,
    /// Height of the tab shell inside the `TABBAR_HEIGHT` row.
    pub(crate) item_height: f32,
    pub(crate) min_width: f32,
    pub(crate) max_width: f32,
    /// Horizontal padding inside a tab, on both sides.
    pub(crate) text_padding_x: f32,
    /// Width of the trailing close/pin/switch-hint slot.
    pub(crate) close_slot_width: f32,
}

impl HorizontalTabMetrics {
    pub(crate) const TERMY: Self = Self {
        leading_padding: TAB_HORIZONTAL_PADDING,
        item_gap: TAB_ITEM_GAP,
        item_height: TAB_ITEM_HEIGHT,
        min_width: TAB_MIN_WIDTH,
        max_width: TAB_MAX_WIDTH,
        text_padding_x: TAB_TEXT_PADDING_X,
        close_slot_width: TAB_CLOSE_SLOT_WIDTH,
    };

    /// WinUI `TabView` sizing: adjacent tabs (separators instead of gaps),
    /// 100-240 px wide, shrinking equally before the strip scrolls.
    pub(crate) const WINDOWS_11: Self = Self {
        // Leaves room for the selected first tab's leading ear.
        leading_padding: WINDOWS_TAB_EAR_SIZE,
        item_gap: 0.0,
        item_height: WINDOWS_TAB_ITEM_HEIGHT,
        min_width: 100.0,
        max_width: 240.0,
        text_padding_x: 8.0,
        close_slot_width: WINDOWS_TAB_CLOSE_BUTTON_SIZE,
    };
}

/// Size and placement of the action rail after the last tab.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TabStripActionRailMetrics {
    /// Full rail width.
    pub(crate) width: f32,
    /// Offset of the first button from the rail start.
    pub(crate) buttons_start: f32,
    /// Combined width of the rail's buttons.
    pub(crate) buttons_width: f32,
    pub(crate) button_height: f32,
    /// Vertical center of the buttons inside the `TABBAR_HEIGHT` row.
    pub(crate) button_center_y: f32,
}

impl TabStripActionRailMetrics {
    pub(crate) const TERMY: Self = Self {
        width: TABBAR_ACTION_RAIL_WIDTH,
        buttons_start: 0.0,
        buttons_width: TABBAR_NEW_TAB_BUTTON_SIZE,
        button_height: TABBAR_NEW_TAB_BUTTON_SIZE,
        button_center_y: TABBAR_HEIGHT * 0.5,
    };
}

// Windows 11 style geometry.
pub(crate) const WINDOWS_TAB_ITEM_HEIGHT: f32 = 28.0;
pub(crate) const WINDOWS_TAB_CORNER_RADIUS: f32 = 8.0;
/// Width and height of the concave curves flaring out at the selected tab's
/// bottom corners.
pub(crate) const WINDOWS_TAB_EAR_SIZE: f32 = 8.0;
pub(crate) const WINDOWS_TAB_SEPARATOR_HEIGHT: f32 = 16.0;
pub(crate) const WINDOWS_TAB_SEPARATOR_WIDTH: f32 = 1.0;
pub(crate) const WINDOWS_TAB_CLOSE_BUTTON_SIZE: f32 = 24.0;
pub(crate) const WINDOWS_TAB_CLOSE_BUTTON_RADIUS: f32 = 4.0;
pub(crate) const WINDOWS_TAB_CLOSE_GLYPH_SIZE: f32 = 9.0;
pub(crate) const WINDOWS_TAB_ADD_BUTTON_WIDTH: f32 = 32.0;
pub(crate) const WINDOWS_TAB_MENU_BUTTON_WIDTH: f32 = 24.0;
pub(crate) const WINDOWS_TAB_RAIL_BUTTON_HEIGHT: f32 = 24.0;
pub(crate) const WINDOWS_TAB_RAIL_BUTTON_RADIUS: f32 = 4.0;
pub(crate) const WINDOWS_TAB_RAIL_LEADING_GAP: f32 = 4.0;
pub(crate) const WINDOWS_TAB_ADD_GLYPH_SIZE: f32 = 12.0;
pub(crate) const WINDOWS_TAB_MENU_GLYPH_SIZE: f32 = 9.0;
// Segoe Fluent Icons / Segoe MDL2 Assets code points.
pub(crate) const WINDOWS_TAB_CLOSE_GLYPH: &str = "\u{E8BB}";
pub(crate) const WINDOWS_TAB_ADD_GLYPH: &str = "\u{E710}";
pub(crate) const WINDOWS_TAB_MENU_GLYPH: &str = "\u{E70D}";

// Windows 11 style palette, as foreground alpha over the chrome.
const WINDOWS_TAB_CHROME_FOREGROUND_MIX: f32 = 0.06;
const WINDOWS_TAB_HOVER_ALPHA: f32 = 0.06;
const WINDOWS_TAB_PRESSED_ALPHA: f32 = 0.09;
const WINDOWS_TAB_SELECTED_TEXT_ALPHA: f32 = 1.0;
const WINDOWS_TAB_UNSELECTED_TEXT_ALPHA: f32 = 0.70;
const WINDOWS_TAB_SEPARATOR_ALPHA: f32 = 0.15;
const WINDOWS_TAB_CLOSE_HOVER_ALPHA: f32 = 0.08;
const WINDOWS_TAB_CLOSE_PRESSED_ALPHA: f32 = 0.05;

impl HorizontalTabStyle {
    pub(crate) fn metrics(self) -> HorizontalTabMetrics {
        match self {
            Self::Termy => HorizontalTabMetrics::TERMY,
            Self::Windows11 => HorizontalTabMetrics::WINDOWS_11,
        }
    }

    /// The Windows 11 rail holds "+" and, when the new-tab menu has choices,
    /// a separate chevron that opens it (like Windows Terminal).
    pub(crate) fn action_rail(self, menu_button: bool) -> TabStripActionRailMetrics {
        match self {
            Self::Termy => TabStripActionRailMetrics::TERMY,
            Self::Windows11 => {
                let buttons_width = WINDOWS_TAB_ADD_BUTTON_WIDTH
                    + if menu_button {
                        WINDOWS_TAB_MENU_BUTTON_WIDTH
                    } else {
                        0.0
                    };
                TabStripActionRailMetrics {
                    width: WINDOWS_TAB_RAIL_LEADING_GAP + buttons_width,
                    buttons_start: WINDOWS_TAB_RAIL_LEADING_GAP,
                    buttons_width,
                    button_height: WINDOWS_TAB_RAIL_BUTTON_HEIGHT,
                    button_center_y: TABBAR_HEIGHT - (WINDOWS_TAB_ITEM_HEIGHT * 0.5),
                }
            }
        }
    }

    pub(crate) fn is_windows_11(self) -> bool {
        self == Self::Windows11
    }
}

/// Whether the 1 px separator after tab `index` shows. Separators divide
/// adjacent unselected tabs and hide next to the selected or hovered tab,
/// whose fill already separates it.
pub(crate) fn windows_tab_separator_visible(
    index: usize,
    tab_count: usize,
    active_tab: usize,
    hovered_tab: Option<usize>,
) -> bool {
    let next = index.saturating_add(1);
    if next >= tab_count {
        return false;
    }
    let highlighted = |tab: usize| tab == active_tab || hovered_tab == Some(tab);
    !highlighted(index) && !highlighted(next)
}

/// Titlebar fill behind Windows 11 style tabs. Over a system backdrop (Mica)
/// the chrome stays transparent so the material shows; otherwise it is the
/// terminal background nudged toward the foreground, so the selected tab
/// (terminal surface color) stands out in light and dark themes alike.
pub(crate) fn windows_tab_chrome_background(
    background: gpui_kit::Rgba,
    foreground: gpui_kit::Rgba,
    surface_alpha: f32,
    system_backdrop: bool,
) -> gpui_kit::Rgba {
    let mut chrome = blend_rgba(background, foreground, WINDOWS_TAB_CHROME_FOREGROUND_MIX);
    chrome.a = if system_backdrop {
        0.0
    } else {
        surface_alpha.clamp(0.0, 1.0)
    };
    chrome
}

/// Segoe UI Variable is the Windows 11 UI face; Windows 10 has Segoe UI.
pub(crate) fn windows_tab_title_font_family() -> &'static str {
    #[cfg(target_os = "windows")]
    if crate::native_sdk::win32_window::is_windows_11() {
        return "Segoe UI Variable Text";
    }
    "Segoe UI"
}

/// Segoe Fluent Icons replaces Segoe MDL2 Assets on Windows 11; both carry
/// the glyphs used here at the same code points.
pub(crate) fn windows_tab_icon_font_family() -> &'static str {
    #[cfg(target_os = "windows")]
    if crate::native_sdk::win32_window::is_windows_11() {
        return "Segoe Fluent Icons";
    }
    "Segoe MDL2 Assets"
}

impl TerminalView {
    pub(crate) fn horizontal_tab_style(&self) -> HorizontalTabStyle {
        if self.windows_native_tab_style {
            HorizontalTabStyle::Windows11
        } else {
            HorizontalTabStyle::Termy
        }
    }

    pub(crate) fn horizontal_tab_metrics(&self) -> HorizontalTabMetrics {
        self.horizontal_tab_style().metrics()
    }

    pub(crate) fn horizontal_tab_action_rail(&self) -> TabStripActionRailMetrics {
        let style = self.horizontal_tab_style();
        style.action_rail(style.is_windows_11() && self.new_tab_menu_has_choices())
    }

    /// The horizontal strip is on screen in the Windows 11 style, so the
    /// titlebar row takes the chrome fill and only the content below it
    /// paints the terminal surface.
    pub(crate) fn windows_tab_chrome_active(&self) -> bool {
        self.horizontal_tab_style().is_windows_11()
            && self.tab_strip_orientation() == super::state::TabStripOrientation::Horizontal
            && self.should_render_tab_strip_chrome()
    }

    pub(crate) fn windows_tab_chrome_background_for(
        &self,
        colors: &TerminalColors,
    ) -> gpui_kit::Rgba {
        let system_backdrop = matches!(
            self.last_window_background_appearance,
            Some(
                WindowBackgroundAppearance::Blurred
                    | WindowBackgroundAppearance::MicaBackdrop
                    | WindowBackgroundAppearance::MicaAltBackdrop
            )
        );
        windows_tab_chrome_background(
            colors.background,
            colors.foreground,
            self.scaled_background_alpha(colors.background.a),
            system_backdrop,
        )
    }

    /// Windows 11 style colors on top of the shared strip palette. The
    /// selected tab takes `surface_bg`, the color the terminal content uses.
    pub(super) fn apply_windows_tab_palette(
        palette: &mut super::render_palette::TabStripPalette,
        colors: &TerminalColors,
        surface_bg: gpui_kit::Rgba,
    ) {
        let with_alpha = |mut color: gpui_kit::Rgba, alpha: f32| {
            color.a = alpha;
            color
        };
        let fg = colors.foreground;
        palette.active_tab_bg = surface_bg;
        palette.inactive_tab_bg = with_alpha(fg, 0.0);
        palette.hovered_tab_bg = with_alpha(fg, WINDOWS_TAB_HOVER_ALPHA);
        palette.active_tab_text = with_alpha(fg, WINDOWS_TAB_SELECTED_TEXT_ALPHA);
        palette.inactive_tab_text = with_alpha(fg, WINDOWS_TAB_UNSELECTED_TEXT_ALPHA);
        palette.close_button_bg = with_alpha(fg, 0.0);
        palette.close_button_hover_bg = with_alpha(fg, WINDOWS_TAB_CLOSE_HOVER_ALPHA);
        palette.close_button_danger_hover_bg = palette.close_button_hover_bg;
        palette.close_button_hover_text = with_alpha(fg, 1.0);
        palette.close_button_pressed_bg = with_alpha(fg, WINDOWS_TAB_CLOSE_PRESSED_ALPHA);
        palette.pressed_tab_bg = with_alpha(fg, WINDOWS_TAB_PRESSED_ALPHA);
        palette.separator_color = with_alpha(fg, WINDOWS_TAB_SEPARATOR_ALPHA);
        palette.style = HorizontalTabStyle::Windows11;
    }

    /// Concave curve flaring out from a bottom corner of the selected tab.
    /// `leading` puts it left of the tab. The fill is the outside of a
    /// quarter circle: a ring (rounded border, inner radius = ear size)
    /// centered on the ear's outer top corner, clipped to the ear box.
    pub(super) fn render_windows_tab_ear(leading: bool, fill: gpui_kit::Rgba) -> AnyElement {
        let size = WINDOWS_TAB_EAR_SIZE;
        let ring_width = size;
        let outer_radius = size + ring_width;
        let ring_extent = outer_radius * 2.0;
        let ring_left = if leading {
            -outer_radius
        } else {
            size - outer_radius
        };
        let ear = div()
            .absolute()
            .bottom_0()
            .w(px(size))
            .h(px(size))
            .overflow_hidden();
        let ear = if leading {
            ear.left(px(-size))
        } else {
            ear.right(px(-size))
        };
        ear.child(
            div()
                .absolute()
                .left(px(ring_left))
                .top(px(-outer_radius))
                .w(px(ring_extent))
                .h(px(ring_extent))
                .rounded(px(outer_radius))
                .border(px(ring_width))
                .border_color(fill),
        )
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn termy_metrics_match_the_existing_strip_constants() {
        let metrics = HorizontalTabStyle::Termy.metrics();
        assert_eq!(metrics.leading_padding, TAB_HORIZONTAL_PADDING);
        assert_eq!(metrics.item_gap, TAB_ITEM_GAP);
        assert_eq!(metrics.item_height, TAB_ITEM_HEIGHT);
        assert_eq!(metrics.min_width, TAB_MIN_WIDTH);
        assert_eq!(metrics.max_width, TAB_MAX_WIDTH);
        assert_eq!(metrics.text_padding_x, TAB_TEXT_PADDING_X);
        assert_eq!(metrics.close_slot_width, TAB_CLOSE_SLOT_WIDTH);
        assert_eq!(
            HorizontalTabStyle::Termy.action_rail(true),
            TabStripActionRailMetrics::TERMY
        );
    }

    #[test]
    fn windows_metrics_follow_winui_tab_view_sizing() {
        let metrics = HorizontalTabStyle::Windows11.metrics();
        assert_eq!(metrics.item_gap, 0.0);
        assert_eq!(metrics.min_width, 100.0);
        assert_eq!(metrics.max_width, 240.0);
        assert!(metrics.item_height <= TABBAR_HEIGHT);
        assert!(metrics.leading_padding >= WINDOWS_TAB_EAR_SIZE);
        assert!(WINDOWS_TAB_CLOSE_BUTTON_SIZE <= metrics.item_height);
    }

    #[test]
    fn windows_action_rail_adds_menu_chevron_only_when_requested() {
        let plain = HorizontalTabStyle::Windows11.action_rail(false);
        let with_menu = HorizontalTabStyle::Windows11.action_rail(true);
        assert_eq!(plain.buttons_width, WINDOWS_TAB_ADD_BUTTON_WIDTH);
        assert_eq!(
            with_menu.buttons_width,
            WINDOWS_TAB_ADD_BUTTON_WIDTH + WINDOWS_TAB_MENU_BUTTON_WIDTH
        );
        assert_eq!(
            with_menu.width,
            with_menu.buttons_start + with_menu.buttons_width
        );
        // Buttons center on the tabs, which sit on the row's bottom edge.
        assert_eq!(
            with_menu.button_center_y + WINDOWS_TAB_ITEM_HEIGHT * 0.5,
            TABBAR_HEIGHT
        );
    }

    #[test]
    fn separators_divide_adjacent_unselected_tabs() {
        // Tabs 0..4, tab 2 selected, nothing hovered.
        assert!(windows_tab_separator_visible(0, 5, 2, None));
        assert!(!windows_tab_separator_visible(1, 5, 2, None));
        assert!(!windows_tab_separator_visible(2, 5, 2, None));
        assert!(windows_tab_separator_visible(3, 5, 2, None));
    }

    #[test]
    fn separators_hide_after_the_last_tab() {
        assert!(!windows_tab_separator_visible(4, 5, 0, None));
        assert!(!windows_tab_separator_visible(0, 1, 0, None));
        assert!(!windows_tab_separator_visible(0, 0, 0, None));
    }

    #[test]
    fn separators_hide_next_to_the_hovered_tab() {
        assert!(!windows_tab_separator_visible(2, 5, 0, Some(3)));
        assert!(!windows_tab_separator_visible(3, 5, 0, Some(3)));
        assert!(windows_tab_separator_visible(1, 5, 0, Some(3)));
    }

    #[test]
    fn chrome_background_is_transparent_over_a_system_backdrop() {
        let background = gpui_kit::Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        };
        let foreground = gpui_kit::Rgba {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        };
        let backdrop = windows_tab_chrome_background(background, foreground, 0.8, true);
        assert_eq!(backdrop.a, 0.0);

        let opaque = windows_tab_chrome_background(background, foreground, 1.0, false);
        assert_eq!(opaque.a, 1.0);
        assert!(opaque.r > background.r && opaque.r < foreground.r);

        let translucent = windows_tab_chrome_background(background, foreground, 0.6, false);
        assert_eq!(translucent.a, 0.6);
    }
}
