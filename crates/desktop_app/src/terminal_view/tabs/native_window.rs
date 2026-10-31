use super::*;

impl TerminalView {
    /// Tab commands for native tab windows. Every tab is its own window, so
    /// navigation acts on the AppKit tab group instead of `session.tabs`.
    /// `None` leaves the command to the regular tab handling.
    pub(in super::super) fn execute_native_window_tab_action(
        &mut self,
        action: CommandAction,
        window: &mut Window,
    ) -> Option<bool> {
        if !self.native_window_tabs {
            return None;
        }
        #[cfg(target_os = "macos")]
        {
            use crate::macos_native_tabs as native;
            let (index, count) = native::tab_position(window).unwrap_or((0, 1));
            let handled = match action {
                CommandAction::SwitchTabLeft => {
                    if let Some(previous) = index.checked_sub(1) {
                        native::select_tab(window, previous);
                    }
                    true
                }
                CommandAction::SwitchTabRight => {
                    native::select_tab(window, index + 1);
                    true
                }
                CommandAction::CycleTabs => {
                    count > 1 && native::select_tab(window, (index + 1) % count)
                }
                CommandAction::MoveTabLeft => {
                    native::move_tab(window, false);
                    true
                }
                CommandAction::MoveTabRight => {
                    native::move_tab(window, true);
                    true
                }
                CommandAction::RenameTab => {
                    crate::ui::toast::info("Native tabs take their name from the window title");
                    true
                }
                action => {
                    let position = Self::switch_to_tab_position_for_action(action)?;
                    native::select_tab(window, position - 1)
                }
            };
            Some(handled)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (action, window);
            None
        }
    }

    #[cfg(target_os = "macos")]
    fn switch_to_tab_position_for_action(action: CommandAction) -> Option<usize> {
        Some(match action {
            CommandAction::SwitchToTab1 => 1,
            CommandAction::SwitchToTab2 => 2,
            CommandAction::SwitchToTab3 => 3,
            CommandAction::SwitchToTab4 => 4,
            CommandAction::SwitchToTab5 => 5,
            CommandAction::SwitchToTab6 => 6,
            CommandAction::SwitchToTab7 => 7,
            CommandAction::SwitchToTab8 => 8,
            CommandAction::SwitchToTab9 => 9,
            _ => return None,
        })
    }

    /// Show or hide AppKit's tab bar. Returns false outside native tab windows.
    pub(in super::super) fn toggle_native_window_tab_bar(&self, window: &Window) -> bool {
        if !self.native_window_tabs {
            return false;
        }
        #[cfg(target_os = "macos")]
        crate::macos_native_tabs::toggle_tab_bar(window);
        #[cfg(not(target_os = "macos"))]
        let _ = window;
        true
    }

    /// Keep AppKit's chrome in step with the shown tab: the window title is
    /// the tab label, and the titlebar takes the terminal background. Runs on
    /// every notify because background tabs never render.
    pub(in super::super) fn sync_native_window_chrome(&mut self, cx: &mut Context<Self>) {
        if !self.native_window_tabs {
            return;
        }
        let Some(tab) = self.session.tabs.get(self.session.active_tab) else {
            return;
        };
        let background = self.colors.background;
        let foreground = self.colors.foreground;
        let chrome = NativeWindowChrome {
            title: tab.title.clone(),
            background: [
                f64::from(background.r),
                f64::from(background.g),
                f64::from(background.b),
                f64::from(self.effective_background_opacity()),
            ],
            foreground: [
                f64::from(foreground.r),
                f64::from(foreground.g),
                f64::from(foreground.b),
                f64::from(foreground.a),
            ],
        };
        if self.native_window_chrome.as_ref() == Some(&chrome) {
            return;
        }
        #[cfg(target_os = "macos")]
        {
            let [red, green, blue, _] = chrome.background;
            let dark = 0.2126 * red + 0.7152 * green + 0.0722 * blue < 0.5;
            let (title, background) = (chrome.title.clone(), chrome.background);
            let tab_bar = native_tab_bar_colors(chrome.background, chrome.foreground);
            let window = self.window_handle;
            App::defer(cx, move |cx| {
                let _ = window.update(cx, |_, window, _| {
                    crate::macos_native_tabs::set_window_chrome(
                        window, title, background, dark, tab_bar,
                    );
                });
            });
        }
        #[cfg(not(target_os = "macos"))]
        let _ = cx;
        self.native_window_chrome = Some(chrome);
    }

    /// A native tab window shows one tab. Any extra tab (a new tab, or a
    /// restored window that had several) moves into its own native tab once
    /// the current update finishes.
    pub(in super::super) fn schedule_native_window_tab_split(&mut self, cx: &mut Context<Self>) {
        if !self.native_window_tabs || self.native_tab_split_pending || self.session.tabs.len() <= 1
        {
            return;
        }
        self.native_tab_split_pending = true;
        let view = cx.entity().downgrade();
        let window = self.window_handle;
        App::defer(cx, move |cx| {
            Self::split_native_window_tabs(view, window, cx);
        });
    }
}

/// What a native tab window last showed in AppKit's chrome.
#[derive(Clone, PartialEq)]
pub(in super::super) struct NativeWindowChrome {
    title: String,
    background: [f64; 4],
    foreground: [f64; 4],
}

/// How far the tab bar's glass moves from the terminal background toward
/// the foreground: a faint track, and a clearly lifted selected tab.
#[cfg(target_os = "macos")]
const NATIVE_TAB_TRACK_FOREGROUND_MIX: f64 = 0.06;
#[cfg(target_os = "macos")]
const NATIVE_TAB_SELECTED_FOREGROUND_MIX: f64 = 0.16;

/// AppKit tab bar colors from the theme, as the custom strip does it: tints
/// of the terminal background toward the foreground, titles in the foreground.
/// The track keeps the background opacity so translucent windows stay so.
#[cfg(target_os = "macos")]
fn native_tab_bar_colors(
    background: [f64; 4],
    foreground: [f64; 4],
) -> crate::macos_native_tabs::TabBarColors {
    let mix = |amount: f64| {
        let channel =
            |index: usize| background[index] + (foreground[index] - background[index]) * amount;
        [channel(0), channel(1), channel(2), 1.0]
    };
    let [red, green, blue, _] = mix(NATIVE_TAB_TRACK_FOREGROUND_MIX);
    crate::macos_native_tabs::TabBarColors {
        track: [red, green, blue, background[3]],
        selected: mix(NATIVE_TAB_SELECTED_FOREGROUND_MIX),
        text: foreground,
    }
}
