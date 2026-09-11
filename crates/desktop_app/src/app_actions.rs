use crate::APP_ID;
use crate::config;
use crate::settings_view::{SettingsSection, SettingsWindow};
use crate::terminal_view::TerminalView;
use crate::terminal_view::initial_window_background_appearance;
use gpui::{App, AppContext, Bounds, WindowBounds, WindowOptions, px, size};

pub(crate) fn open_config_file() -> Result<(), String> {
    config::open_config_file().map_err(|error| error.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsActionTarget {
    SettingsWindow,
    ConfigFile,
}

pub(crate) fn settings_action_target(config: &config::AppConfig) -> SettingsActionTarget {
    if config.simple_mode {
        SettingsActionTarget::ConfigFile
    } else {
        SettingsActionTarget::SettingsWindow
    }
}

pub(crate) fn open_settings_or_config_file(cx: &mut App) -> Result<(), String> {
    let mut settings_config_error = None;
    let settings_load = config::load_runtime_config(
        &mut settings_config_error,
        "Failed to load config before opening settings",
    );

    match settings_action_target(&settings_load.config) {
        SettingsActionTarget::SettingsWindow => open_settings_window(cx),
        SettingsActionTarget::ConfigFile => open_config_file(),
    }
}

pub(crate) fn focus_existing_window<V: 'static>(cx: &mut App) -> bool {
    if let Some(window_handle) = cx
        .windows()
        .into_iter()
        .find_map(|handle| handle.downcast::<V>())
    {
        window_handle
            .update(cx, |_view, window, _cx| {
                window.activate_window();
            })
            .is_ok()
    } else {
        false
    }
}

pub(crate) fn has_window<V: 'static>(cx: &App) -> bool {
    cx.windows()
        .into_iter()
        .any(|handle| handle.downcast::<V>().is_some())
}

pub(crate) fn update_open_settings_windows(
    cx: &mut App,
    mut update: impl FnMut(&mut SettingsWindow, &mut gpui::Context<SettingsWindow>),
) {
    for settings_window in cx
        .windows()
        .into_iter()
        .filter_map(|handle| handle.downcast::<SettingsWindow>())
    {
        let _ = settings_window.update(cx, |view, _window, cx| update(view, cx));
    }
}

pub(crate) fn refresh_open_terminal_theme_assets(cx: &mut App) {
    for terminal_window in cx
        .windows()
        .into_iter()
        .filter_map(|handle| handle.downcast::<TerminalView>())
    {
        let _ = terminal_window.update(cx, |view, _window, cx| {
            view.reload_theme_assets(cx);
        });
    }
}

pub(crate) fn close_settings_windows(cx: &mut App) {
    for settings_window in cx
        .windows()
        .into_iter()
        .filter_map(|handle| handle.downcast::<SettingsWindow>())
    {
        let _ = settings_window.update(cx, |_view, window, _cx| window.remove_window());
    }
}

pub(crate) fn open_new_tab_in_main_window(
    cx: &mut App,
    command: Option<String>,
    dir: Option<String>,
) -> Result<(), String> {
    let Some(main_window) = cx
        .windows()
        .into_iter()
        .find_map(|handle| handle.downcast::<TerminalView>())
    else {
        return Err("No main window available for new tab deeplink".to_string());
    };

    main_window
        .update(cx, |view, window, cx| {
            view.open_new_tab_from_deeplink(command.as_deref(), dir.as_deref(), window, cx);
        })
        .map_err(|error| format!("Failed to open new tab from deeplink: {error}"))?;

    Ok(())
}

pub(crate) fn open_settings_window(cx: &mut App) -> Result<(), String> {
    open_settings_window_with_section(None, cx)
}

pub(crate) fn open_settings_section(section: SettingsSection, cx: &mut App) -> Result<(), String> {
    open_settings_window_with_section(Some(section), cx)
}

fn open_settings_window_with_section(
    section: Option<SettingsSection>,
    cx: &mut App,
) -> Result<(), String> {
    // Key-repeat and repeated action dispatch should raise the existing settings window,
    // not spawn duplicate windows.
    if section.is_none() && focus_existing_window::<SettingsWindow>(cx) {
        return Ok(());
    }
    if let Some(settings_window) = cx
        .windows()
        .into_iter()
        .find_map(|handle| handle.downcast::<SettingsWindow>())
    {
        let Some(section) = section else {
            // A failed focus can happen during a re-entrant settings update. The
            // existing window still owns the action, so do not create another.
            return Ok(());
        };
        return settings_window
            .update(cx, |view, window, cx| {
                view.set_active_section(section, window, cx);
                window.activate_window();
            })
            .map_err(|error| format!("Failed to focus settings window: {error}"));
    }
    // If a settings window still exists after a failed focus attempt (for example,
    // during a re-entrant update), do not open a duplicate.
    if has_window::<SettingsWindow>(cx) {
        return Ok(());
    }

    let initial_window_size = size(px(1080.0), px(675.0));
    let minimum_window_size = size(px(760.0), px(560.0));
    let bounds = Bounds::centered(None, initial_window_size, cx);
    let mut settings_config_error = None;
    let settings_load = config::load_runtime_config(
        &mut settings_config_error,
        "Failed to load config for settings window",
    );
    let window_background = initial_window_background_appearance(&settings_load.config);

    #[cfg(target_os = "macos")]
    let titlebar = Some(gpui::TitlebarOptions {
        title: Some("Settings".into()),
        appears_transparent: true,
        traffic_light_position: Some(gpui::point(px(12.0), px(10.0))),
    });
    #[cfg(target_os = "windows")]
    let titlebar = Some(gpui::TitlebarOptions {
        title: Some("Settings".into()),
        appears_transparent: false,
        traffic_light_position: None,
    });
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let titlebar = Some(gpui::TitlebarOptions {
        title: Some("Settings".into()),
        appears_transparent: false,
        traffic_light_position: None,
    });

    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar,
            window_background,
            app_id: Some(APP_ID.to_string()),
            // Match the terminal window: let KDE/the window manager own the frame.
            #[cfg(target_os = "linux")]
            window_decorations: Some(gpui::WindowDecorations::Server),
            is_resizable: true,
            window_min_size: Some(minimum_window_size),
            ..Default::default()
        },
        |window, cx| {
            #[cfg(target_os = "linux")]
            window.set_window_title("Settings");
            cx.new(|cx| {
                let mut view = SettingsWindow::new(window, cx);
                if let Some(section) = section {
                    view.set_active_section(section, window, cx);
                }
                view
            })
        },
    )
    .map(|_| ())
    .map_err(|error| format!("Failed to open settings window: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AnyWindowHandle, TestAppContext};

    fn settings_window_count(cx: &TestAppContext) -> usize {
        cx.windows()
            .into_iter()
            .filter(|handle| handle.downcast::<SettingsWindow>().is_some())
            .count()
    }

    #[gpui::test]
    fn open_settings_window_reuses_existing_window(cx: &mut TestAppContext) {
        assert_eq!(settings_window_count(cx), 0);

        cx.update(|app| {
            open_settings_window(app).expect("settings window should open");
        });
        assert_eq!(settings_window_count(cx), 1);

        cx.update(|app| {
            open_settings_window(app).expect("settings window should be reused");
            open_settings_window(app).expect("settings window should be reused");
        });
        assert_eq!(settings_window_count(cx), 1);
    }

    #[test]
    fn simple_mode_routes_settings_action_to_config_file() {
        let mut config = config::AppConfig::default();
        assert_eq!(
            settings_action_target(&config),
            SettingsActionTarget::SettingsWindow
        );

        config.simple_mode = true;
        assert_eq!(
            settings_action_target(&config),
            SettingsActionTarget::ConfigFile
        );
    }

    #[gpui::test]
    fn open_settings_window_does_not_duplicate_when_called_from_settings_update(
        cx: &mut TestAppContext,
    ) {
        cx.update(|app| {
            open_settings_window(app).expect("settings window should open");
        });
        assert_eq!(settings_window_count(cx), 1);

        let settings_window = cx
            .windows()
            .into_iter()
            .find_map(|handle| handle.downcast::<SettingsWindow>())
            .expect("settings window should exist");
        let settings_window_any: AnyWindowHandle = settings_window.into();

        settings_window_any
            .update(cx, |_view, _window, app| {
                open_settings_window(app).expect("settings window should be reused");
            })
            .expect("settings window update should succeed");

        assert_eq!(settings_window_count(cx), 1);
    }
}
