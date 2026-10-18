//! `tab_style = native` on Windows: the horizontal strip is a real WinUI
//! `TabView` (see `native_sdk::winui_tab_strip`) laid over the tab lane.
//!
//! Termy keeps the tab model. Every render pushes the tabs, lane and colors
//! to the strip, and its clicks come back as events that run the same tab
//! commands the drawn strip uses. The strip covers the whole tab lane and
//! moves the window itself from its empty space, like a titlebar.
//! When the XAML runtime is unavailable (termy.exe without its manifest),
//! the drawn Windows 11 style strip renders instead.

use super::super::*;
use super::render_shared::TabStripRenderState;
use crate::native_sdk::winui_tab_strip::{
    TabStripEvent, TabStripLane, TabStripTheme, WinUiTabStrip, argb,
};
use std::sync::atomic::{AtomicBool, Ordering};

/// Set once creating a strip fails; the cause (the exe manifest or a missing
/// Microsoft.UI.Xaml.dll) applies to every window.
static WINUI_UNAVAILABLE: AtomicBool = AtomicBool::new(false);

/// Hover and pressed tabs, over the strip background.
const HOVER_MIX: f32 = 0.06;
const PRESSED_MIX: f32 = 0.09;
const INACTIVE_TEXT_ALPHA: f32 = 0.70;

pub(crate) struct WinUiTabStripHost {
    strip: WinUiTabStrip,
    /// Left edge of the lane in window coordinates (logical pixels), to
    /// place menus the strip asks for.
    lane_x: f32,
    window_width: f32,
}

impl TerminalView {
    /// Shows the WinUI strip over the tab lane. False when it is unavailable,
    /// and the drawn strip should render instead.
    pub(super) fn sync_winui_tab_strip(
        &mut self,
        window: &Window,
        state: &TabStripRenderState,
        colors: &TerminalColors,
        tabbar_bg: gpui_kit::Rgba,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.ensure_winui_tab_strip(window, cx) {
            return false;
        }
        let items: Vec<(u64, &str)> = self
            .session
            .tabs
            .iter()
            .map(|tab| (tab.id, tab.title.as_str()))
            .collect();
        let selected = (!items.is_empty()).then_some(self.session.active_tab);
        let theme = Self::winui_tab_strip_theme(
            self.windows_tab_chrome_background_for(colors),
            tabbar_bg,
            colors.foreground,
        );
        let menu_button = self.new_tab_menu_has_choices();
        let geometry = state.geometry;
        let scale = window.scale_factor();
        let physical = |value: f32| (value * scale).round() as i32;
        let lane = TabStripLane {
            x: physical(geometry.row_start_x),
            y: physical(TOP_STRIP_CONTENT_OFFSET_Y),
            width: physical(Self::winui_tab_lane_width(&geometry)),
            height: physical(TABBAR_HEIGHT),
        };

        let Some(host) = self.winui_tab_strip.as_mut() else {
            return false;
        };
        host.lane_x = geometry.row_start_x;
        host.window_width = geometry.window_width;
        host.strip.set_theme(theme);
        host.strip.set_menu_button(menu_button);
        host.strip.set_items(&items, selected);
        host.strip.set_lane(lane, true);
        true
    }

    /// Renames tab `index` in place on the WinUI strip. False when the strip
    /// is not showing, and Termy's own inline rename applies.
    pub(crate) fn begin_winui_tab_rename(&mut self, index: usize) -> bool {
        let Some(tab_id) = self.session.tabs.get(index).map(|tab| tab.id) else {
            return false;
        };
        match self.winui_tab_strip.as_mut() {
            Some(host) if host.strip.is_visible() => {
                host.strip.begin_rename(tab_id);
                true
            }
            _ => false,
        }
    }

    pub(crate) fn hide_winui_tab_strip(&mut self) {
        if let Some(host) = self.winui_tab_strip.as_mut() {
            host.strip.hide();
        }
    }

    fn ensure_winui_tab_strip(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
        if self.winui_tab_strip.is_some() {
            return true;
        }
        if !crate::native_window_tabs::winui_tab_strip_platform()
            || WINUI_UNAVAILABLE.load(Ordering::Relaxed)
        {
            return false;
        }
        let window_id = window.window_handle().window_id().as_u64();
        let Some(parent) = crate::native_sdk::win32_window::hwnd_for_window_id(window_id) else {
            return false;
        };
        let (strip, events) = match WinUiTabStrip::create(parent) {
            Ok(created) => created,
            Err(error) => {
                WINUI_UNAVAILABLE.store(true, Ordering::Relaxed);
                log::warn!(
                    "WinUI tab strip unavailable ({error}); drawing Windows 11 style tabs instead. \
                     termy.exe needs termy.manifest embedded (scripts/windows/embed-manifest.ps1)."
                );
                return false;
            }
        };
        self.winui_tab_strip = Some(WinUiTabStripHost {
            strip,
            lane_x: 0.0,
            window_width: 0.0,
        });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            while let Ok(event) = events.recv_async().await {
                let handled = this.update(cx, |view, cx| view.handle_winui_tab_event(event, cx));
                if handled.is_err() {
                    break;
                }
            }
        })
        .detach();
        true
    }

    fn handle_winui_tab_event(&mut self, event: TabStripEvent, cx: &mut Context<Self>) {
        let Some(host) = self.winui_tab_strip.as_ref() else {
            return;
        };
        let lane_x = host.lane_x;
        let window_width = host.window_width;
        // Menus open under the strip: GPUI cannot draw over the island.
        let menu_y = TOP_STRIP_CONTENT_OFFSET_Y + TABBAR_HEIGHT + 2.0;
        let menu_x = |x: f32| {
            (lane_x + x)
                .min(window_width - NEW_TAB_MENU_WIDTH - 4.0)
                .max(4.0)
        };
        match event {
            // GPUI never sees presses on the strip, so close Termy's menus
            // the way a click elsewhere would.
            TabStripEvent::PointerDown => {
                self.close_new_tab_menu(cx);
                let _ = self.close_tab_context_menu(cx);
                let _ = self.close_terminal_context_menu(cx);
                return;
            }
            TabStripEvent::Select { tab_id } => {
                if let Some(index) = self.tab_index_by_id(tab_id) {
                    self.switch_tab(index, cx);
                }
            }
            TabStripEvent::Close { tab_id } => {
                if let Some(index) = self.tab_index_by_id(tab_id) {
                    self.close_tab(index, cx);
                }
            }
            TabStripEvent::NewTab => {
                self.close_new_tab_menu(cx);
                self.add_tab(cx);
            }
            TabStripEvent::NewTabMenu { x, .. } => {
                self.handle_new_tab_menu_button((menu_x(x), menu_y), cx);
            }
            TabStripEvent::ContextMenu { tab_id, x, .. } => {
                if let Some(index) = self.tab_index_by_id(tab_id) {
                    let position = gpui_kit::point(px(lane_x + x), px(menu_y));
                    self.open_tab_context_menu(index, position, cx);
                }
            }
            TabStripEvent::Reorder { tab_id, index } => {
                if let Some(from) = self.tab_index_by_id(tab_id) {
                    self.reorder_tab(from, index, cx);
                }
            }
            TabStripEvent::DroppedOutside { tab_id, .. } => {
                self.move_tab_out_of_window(tab_id, cx);
            }
            TabStripEvent::Rename { tab_id, title } => {
                if let Some(index) = self.tab_index_by_id(tab_id) {
                    self.renaming_tab = Some(index);
                    self.rename_input.set_text(title);
                    self.commit_rename_tab(cx);
                }
            }
            TabStripEvent::RenameCancelled { .. } => self.cancel_rename_tab(cx),
        }
        // The strip may have moved ahead of the model (a refused close or
        // switch); the next render puts it back in step.
        if let Some(host) = self.winui_tab_strip.as_mut() {
            host.strip.invalidate_items();
        }
        cx.notify();
    }

    /// The selected tab takes the terminal surface color, so it runs into the
    /// content below like Windows Terminal's. XAML Islands cannot show the
    /// window backdrop, so translucent colors are made opaque.
    fn winui_tab_strip_theme(
        chrome: gpui_kit::Rgba,
        surface: gpui_kit::Rgba,
        foreground: gpui_kit::Rgba,
    ) -> TabStripTheme {
        let opaque = |mut color: gpui_kit::Rgba| {
            color.a = 1.0;
            color
        };
        let chrome = opaque(chrome);
        let surface = opaque(surface);
        let text = opaque(foreground);
        let mut inactive_text = text;
        inactive_text.a = INACTIVE_TEXT_ALPHA;
        let luminance = 0.2126 * chrome.r + 0.7152 * chrome.g + 0.0722 * chrome.b;
        TabStripTheme {
            dark: i32::from(luminance < 0.5),
            strip_background: argb(chrome),
            selected_background: argb(surface),
            hover_background: argb(blend_rgba(chrome, text, HOVER_MIX)),
            pressed_background: argb(blend_rgba(chrome, text, PRESSED_MIX)),
            selected_foreground: argb(text),
            foreground: argb(inactive_text),
        }
    }
}
