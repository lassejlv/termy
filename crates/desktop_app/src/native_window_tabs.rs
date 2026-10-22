//! Native window tabs (`tab_style = native`): when they apply, where a new
//! terminal window goes, and the AppKit hookup on macOS. On Windows the same
//! setting selects the Windows 11 style of Termy's own tab strip.

use crate::config;
#[cfg(target_os = "macos")]
use crate::{macos_native_tabs, terminal_view::TerminalView};
#[cfg(target_os = "macos")]
use gpui_kit::AsyncApp;
use gpui_kit::{AnyWindowHandle, App};

/// Native tabs replace Termy's tab strip only where Termy does not need its
/// own tab model: tmux tabs, workspaces and saved native tabs all live in one
/// window.
pub(crate) fn native_window_tabs_enabled(config: &config::AppConfig) -> bool {
    cfg!(target_os = "macos")
        && config.tab_style == config::TabStyle::Native
        && !config.tmux_enabled
        && !config.sidebar_enabled
        && !config.native_tab_persistence
}

/// Windows has no OS tab API, so there `tab_style = native` keeps Termy's
/// tab model and draws its horizontal strip in the Windows 11 (WinUI
/// `TabView`) style. Unlike AppKit tabs it works with tmux, workspaces and
/// saved tabs.
pub(crate) fn windows_native_tab_style(config: &config::AppConfig) -> bool {
    cfg!(target_os = "windows") && config.tab_style == config::TabStyle::Native
}

/// Set once GPUI renders without DirectComposition, which the WinUI tab
/// strip needs (see [`prepare_windows_platform`]).
#[cfg(target_os = "windows")]
static WINUI_TAB_STRIP_PLATFORM: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Windows draws the `tab_style = native` strip with a WinUI `TabView` in a
/// XAML Island, a child window. GPUI's DirectComposition output sits above
/// child windows and takes their input, so for the strip GPUI renders to a
/// plain window swapchain instead. That rules out window transparency, so it
/// happens only for opaque windows. GPUI reads the switch once, when its
/// platform starts, so this runs before that and before any threads exist.
#[cfg(target_os = "windows")]
pub(crate) fn prepare_windows_platform(config: &config::AppConfig) {
    if !windows_native_tab_style(config) || config.background_opacity < 1.0 {
        return;
    }
    // SAFETY: called at the top of `main`, before any other thread starts.
    unsafe { std::env::set_var("GPUI_DISABLE_DIRECT_COMPOSITION", "1") };
    WINUI_TAB_STRIP_PLATFORM.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Whether this process can show the WinUI tab strip. Changing `tab_style`
/// or the background opacity takes effect after a restart.
#[cfg(target_os = "windows")]
pub(crate) fn winui_tab_strip_platform() -> bool {
    WINUI_TAB_STRIP_PLATFORM.load(std::sync::atomic::Ordering::Relaxed)
}

/// Where a new terminal window goes.
#[derive(Clone, Copy)]
pub(crate) enum TerminalWindowPlacement {
    Standalone,
    /// An AppKit tab directly after (right of) or before `host`.
    NativeTab {
        host: AnyWindowHandle,
        after: bool,
    },
}

/// Tab a freshly opened, still hidden window beside `host`.
pub(crate) fn attach(cx: &mut App, window: AnyWindowHandle, host: AnyWindowHandle, after: bool) {
    #[cfg(target_os = "macos")]
    {
        let host_window = host
            .update(cx, |_, window, _| macos_native_tabs::Retained::of(window))
            .ok()
            .flatten();
        let _ = window.update(cx, |_, window, _| {
            match (host_window, macos_native_tabs::Retained::of(window)) {
                (Some(host_window), Some(tab_window)) => {
                    macos_native_tabs::add_tab(host_window, tab_window, after);
                }
                // The host closed meanwhile; show the window on its own.
                _ => window.activate_window(),
            }
        });
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (host, after);
        let _ = window.update(cx, |_, window, _| window.activate_window());
    }
}

/// Restore AppKit's tab bar on GPUI windows and route its "+" button to a new
/// tab in the clicked window.
#[cfg(target_os = "macos")]
pub(crate) fn install(cx: &mut App) {
    macos_native_tabs::allow_automatic_window_tabbing();
    let Some(requests) = macos_native_tabs::install() else {
        return;
    };
    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Ok(ns_window) = requests.recv_async().await {
            cx.update(|cx| {
                let target = cx
                    .windows()
                    .into_iter()
                    .filter_map(|handle| handle.downcast::<TerminalView>())
                    .find(|handle| {
                        handle
                            .update(cx, |_, window, _| {
                                macos_native_tabs::ns_window_address(window) == Some(ns_window)
                            })
                            .unwrap_or(false)
                    });
                if let Some(target) = target {
                    let _ = target.update(cx, |view, _, cx| view.add_tab(cx));
                }
            });
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::{native_window_tabs_enabled, windows_native_tab_style};
    use crate::config::{AppConfig, TabStyle};

    #[test]
    fn native_window_tabs_only_apply_on_macos_without_a_single_window_tab_model() {
        let config = AppConfig::default();
        assert_eq!(
            native_window_tabs_enabled(&config),
            cfg!(target_os = "macos")
        );

        let mut custom = config.clone();
        custom.tab_style = TabStyle::Custom;
        assert!(!native_window_tabs_enabled(&custom));

        for disable in [
            |config: &mut AppConfig| config.tmux_enabled = true,
            |config: &mut AppConfig| config.sidebar_enabled = true,
            |config: &mut AppConfig| config.native_tab_persistence = true,
        ] {
            let mut config = config.clone();
            disable(&mut config);
            assert!(!native_window_tabs_enabled(&config));
        }
    }

    #[test]
    fn windows_native_tab_style_follows_tab_style_on_windows_only() {
        let config = AppConfig::default();
        assert_eq!(
            windows_native_tab_style(&config),
            cfg!(target_os = "windows")
        );

        let mut custom = config.clone();
        custom.tab_style = TabStyle::Custom;
        assert!(!windows_native_tab_style(&custom));

        // Termy keeps its own tab model, so tmux and workspaces still apply.
        let mut tmux_and_sidebar = config;
        tmux_and_sidebar.tmux_enabled = true;
        tmux_and_sidebar.sidebar_enabled = true;
        assert_eq!(
            windows_native_tab_style(&tmux_and_sidebar),
            cfg!(target_os = "windows")
        );
    }
}
