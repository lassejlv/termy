//! Native window tabs (`tab_style = native`): when they apply, where a new
//! terminal window goes, and the AppKit hookup on macOS.

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
    use super::native_window_tabs_enabled;
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
}
