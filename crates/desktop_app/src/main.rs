#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod app_actions;
mod app_icon;
mod asset_source;
mod chrome_style;
mod cli_delegate;
mod colors;
mod commands;
mod config;
mod crash_log;
mod deeplink;
#[cfg(target_os = "macos")]
mod default_terminal;
mod font_families;
mod instance;
mod keybindings;
mod launch_probe;
#[cfg(target_os = "macos")]
mod macos_titlebar_drag;
mod menus;
mod settings_view;
mod ssh;
mod startup;
mod terminal_view;
mod text_editing;
mod text_input;
mod theme_store;
mod ui;
mod workspace_store;

use commands::{OpenConfig, OpenSettings};
use deeplink::{DeepLinkArgument, DeepLinkRoute};
use flume::Receiver;
use gpui::{
    App, Application, AsyncApp, Bounds, Pixels, WindowBounds, WindowHandle, WindowKind,
    WindowOptions, prelude::*, px, size,
};
use startup::StartupBlocker;
use terminal_view::{TerminalView, initial_window_background_appearance};
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
use termy_terminal_ui::TmuxClient;

pub(crate) const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub(crate) const APP_ID: &str = "termy";

const MIN_WINDOW_WIDTH: f32 = 480.0;
const MIN_WINDOW_HEIGHT: f32 = 320.0;
#[cfg(target_os = "windows")]
const LEGACY_DEFAULT_WINDOW_WIDTH: f32 = 1100.0;
#[cfg(target_os = "windows")]
const LEGACY_DEFAULT_WINDOW_HEIGHT: f32 = 720.0;
#[cfg(target_os = "windows")]
const WINDOWS_DEFAULT_WINDOW_WIDTH: f32 = 1280.0;
#[cfg(target_os = "windows")]
const WINDOWS_DEFAULT_WINDOW_HEIGHT: f32 = 820.0;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct StartupArguments {
    working_dir: Option<String>,
    deeplinks: Vec<String>,
}

fn parse_startup_arguments<I, S>(args: I) -> StartupArguments
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut startup = StartupArguments::default();
    let mut args = args.into_iter().map(Into::into).peekable();

    while let Some(arg) = args.next() {
        if arg.starts_with("termy://") {
            startup.deeplinks.push(arg);
        } else if let Some(value) = arg.strip_prefix("--working-directory=") {
            startup.working_dir = non_empty_arg_value(value);
        } else if arg == "--working-directory" {
            startup.working_dir = args.next().and_then(|value| non_empty_arg_value(&value));
        } else if arg == "--" {
            if let Some(value) = args.next() {
                startup.working_dir = non_empty_arg_value(&value);
            }
            break;
        } else if !arg.starts_with('-') && startup.working_dir.is_none() {
            startup.working_dir = non_empty_arg_value(&arg);
        }
    }

    startup
}

fn non_empty_arg_value(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn fold_startup_new_tab_into_working_dir(startup: &mut StartupArguments) {
    if startup.working_dir.is_some() {
        return;
    }
    if startup.deeplinks.len() != 1 {
        return;
    }
    let Ok((DeepLinkRoute::NewTab, argument)) = DeepLinkRoute::parse(&startup.deeplinks[0]) else {
        return;
    };
    let Some(DeepLinkArgument::NewTab(payload)) = argument else {
        return;
    };
    let Some(dir) = payload.dir else {
        return;
    };
    startup.working_dir = Some(dir);
    startup.deeplinks.clear();
}

fn absorb_pending_open_urls(startup: &mut StartupArguments, urls: Vec<String>) {
    for raw_url in urls {
        if let Some(dir) = deeplink::directory_from_open_target(&raw_url) {
            if startup.working_dir.is_none() {
                startup.working_dir = Some(dir);
            } else {
                startup
                    .deeplinks
                    .push(deeplink::new_tab_deeplink_for_dir(&dir));
            }
            continue;
        }
        if raw_url.starts_with("termy://") {
            startup.deeplinks.push(raw_url);
        }
    }
    fold_startup_new_tab_into_working_dir(startup);
}

fn current_executable() -> Option<std::path::PathBuf> {
    std::env::current_exe()
        .ok()
        .map(|path| path.canonicalize().unwrap_or(path))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn preflight_tmux_runtime(config: &config::AppConfig) -> Result<(), StartupBlocker> {
    if !config.tmux_enabled {
        return Ok(());
    }

    TmuxClient::verify_tmux_version(
        &config.tmux_command_prefix_argv(),
        config.tmux_binary.as_str(),
        3,
        3,
    )
    .map_err(|error| StartupBlocker::TmuxPreflight(format!("tmux preflight failed: {error}")))
}

#[cfg(target_os = "windows")]
fn preflight_tmux_runtime(config: &config::AppConfig) -> Result<(), StartupBlocker> {
    let command_prefix = config.tmux_command_prefix_argv();
    // Without a command prefix the runtime silently stays native on Windows,
    // so there is nothing to preflight.
    if !config.tmux_enabled || command_prefix.is_empty() {
        return Ok(());
    }

    TmuxClient::verify_tmux_version(&command_prefix, config.tmux_binary.as_str(), 3, 3)
        .map_err(|error| StartupBlocker::TmuxPreflight(format!("tmux preflight failed: {error}")))
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn preflight_tmux_runtime(config: &config::AppConfig) -> Result<(), StartupBlocker> {
    if !config.tmux_enabled {
        return Ok(());
    }

    Err(StartupBlocker::TmuxPreflight(
        "tmux runtime is unsupported on this platform".to_string(),
    ))
}

fn guard_tmux_startup(config: &mut config::AppConfig) -> Option<String> {
    let blocker = preflight_tmux_runtime(config).err()?;
    config.tmux_enabled = false;
    Some(blocker.tmux_fallback_message())
}

fn normalized_startup_window_size(startup_config: &config::AppConfig) -> gpui::Size<Pixels> {
    let window_width = startup_config.window_width;
    let window_height = startup_config.window_height;

    #[cfg(target_os = "windows")]
    let (window_width, window_height) = if (window_width - LEGACY_DEFAULT_WINDOW_WIDTH).abs()
        < f32::EPSILON
        && (window_height - LEGACY_DEFAULT_WINDOW_HEIGHT).abs() < f32::EPSILON
    {
        (WINDOWS_DEFAULT_WINDOW_WIDTH, WINDOWS_DEFAULT_WINDOW_HEIGHT)
    } else {
        (window_width, window_height)
    };

    size(
        px(window_width.max(MIN_WINDOW_WIDTH)),
        px(window_height.max(MIN_WINDOW_HEIGHT)),
    )
}

#[cfg(target_os = "windows")]
fn should_apply_windows_startup_resize(
    current: gpui::Size<Pixels>,
    desired: gpui::Size<Pixels>,
) -> bool {
    const WINDOW_SIZE_EPSILON: f32 = 0.5;

    (f32::from(current.width) - f32::from(desired.width)).abs() > WINDOW_SIZE_EPSILON
        || (f32::from(current.height) - f32::from(desired.height)).abs() > WINDOW_SIZE_EPSILON
}

fn open_main_window(
    cx: &mut App,
    startup_config: config::AppConfig,
) -> Result<WindowHandle<TerminalView>, String> {
    let window_background = initial_window_background_appearance(&startup_config);
    let startup_window_size = normalized_startup_window_size(&startup_config);
    let bounds = Bounds::centered(None, startup_window_size, cx);
    let benchmark_mode = std::env::var_os("TERMY_BENCHMARK_COMMAND").is_some();

    #[cfg(target_os = "macos")]
    let titlebar = Some(gpui::TitlebarOptions {
        title: Some("Termy".into()),
        appears_transparent: true,
        traffic_light_position: Some(gpui::point(px(12.0), px(10.0))),
    });
    #[cfg(target_os = "windows")]
    let titlebar = Some(gpui::TitlebarOptions {
        title: Some("Termy".into()),
        appears_transparent: false,
        traffic_light_position: None,
    });
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let titlebar = Some(gpui::TitlebarOptions {
        title: Some("Termy".into()),
        appears_transparent: false,
        traffic_light_position: None,
    });

    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar,
            window_background,
            app_id: Some(APP_ID.to_string()),
            // Let the Linux compositor/window manager own the titlebar,
            // controls, borders, and resize affordances.
            #[cfg(target_os = "linux")]
            window_decorations: Some(gpui::WindowDecorations::Server),
            // Keep both sides of the xctrace comparison visible even when the
            // benchmark is launched from an IDE or another frontmost app.
            // Normal product windows retain the standard level.
            kind: if benchmark_mode {
                WindowKind::Floating
            } else {
                WindowKind::Normal
            },
            // Keep the NSWindow movable so macOS preserves normal zoom and
            // Dock-overlay behavior. The macOS content-view bridge below
            // disables AppKit-owned titlebar dragging without changing the
            // window's native management semantics.
            is_movable: cfg!(target_os = "macos"),
            is_resizable: true,
            window_min_size: Some(size(px(MIN_WINDOW_WIDTH), px(MIN_WINDOW_HEIGHT))),
            ..Default::default()
        },
        move |window, cx| {
            #[cfg(target_os = "linux")]
            window.set_window_title("Termy");
            if benchmark_mode {
                #[cfg(target_os = "macos")]
                if let Err(error) =
                    macos_titlebar_drag::keep_benchmark_panel_visible_when_inactive(window)
                {
                    log::error!(
                        "Failed to keep the macOS benchmark panel visible while inactive: {error}"
                    );
                }
                #[cfg(not(target_os = "macos"))]
                {
                    cx.activate(true);
                    window.activate_window();
                }
            }
            #[cfg(target_os = "windows")]
            {
                let startup_window_size = startup_window_size;
                window.defer(cx, move |window, _cx| {
                    if should_apply_windows_startup_resize(
                        window.viewport_size(),
                        startup_window_size,
                    ) {
                        window.resize(startup_window_size);
                    }
                });
            }

            let view = cx.new({
                let startup_config = startup_config;
                |cx| TerminalView::new(window, cx, startup_config)
            });
            let view_handle = view.downgrade();

            #[cfg(target_os = "macos")]
            {
                if let Err(error) =
                    macos_titlebar_drag::disable_automatic_content_view_window_drag(window)
                {
                    log::error!("Failed to disable automatic macOS titlebar dragging: {error}");
                }
                let (native_drop_tx, native_drop_rx) = flume::unbounded();
                match terminal_view::install_native_file_drop(window, native_drop_tx) {
                    Ok(()) => {
                        view.update(cx, |view, cx| {
                            view.set_native_file_drop_enabled(true);
                            cx.notify();
                        });
                        let native_drop_view = view.downgrade();
                        cx.spawn(async move |cx: &mut AsyncApp| {
                            while let Ok(result) = native_drop_rx.recv_async().await {
                                let _ = cx.update(|cx| {
                                    let _ = native_drop_view.update(cx, |view, cx| {
                                        view.handle_native_file_drop_result(result, cx);
                                    });
                                });
                            }
                        })
                        .detach();
                    }
                    Err(error) => {
                        log::error!("Failed to install native macOS file drop bridge: {error}");
                        crate::ui::toast::error(error.to_string());
                        view.update(cx, |view, cx| {
                            view.set_native_file_drop_enabled(false);
                            cx.notify();
                        });
                    }
                }
            }

            window.on_window_should_close(cx, move |window, cx| {
                view_handle
                    .update(cx, |view, cx| {
                        view.handle_window_should_close_request(window, cx)
                    })
                    .unwrap_or(true)
            });
            view
        },
    )
    .map_err(|error| format!("Failed to open main window: {error}"))
}

fn reopen_if_no_windows(cx: &mut App, mut reopen: impl FnMut(&mut App)) -> bool {
    if !cx.windows().is_empty() {
        return false;
    }

    reopen(cx);
    true
}

fn reopen_main_window(cx: &mut App) {
    let _ = open_main_window_with_runtime_config(cx);
}

pub(crate) fn open_main_window_with_runtime_config(
    cx: &mut App,
) -> Result<WindowHandle<TerminalView>, String> {
    open_main_window_with_runtime_config_overrides(cx, None)
}

pub(crate) fn open_main_window_with_runtime_config_overrides(
    cx: &mut App,
    working_dir: Option<String>,
) -> Result<WindowHandle<TerminalView>, String> {
    let mut reopen_config_error = None;
    let reopen_load =
        config::load_runtime_config(&mut reopen_config_error, "Failed to load config");
    let mut reopen_config = reopen_load.config;
    if let Some(working_dir) = working_dir {
        reopen_config.working_dir = Some(working_dir);
    }
    if let Some(message) = guard_tmux_startup(&mut reopen_config) {
        log::warn!("{message}");
        crate::ui::toast::warning(message);
    }

    open_main_window(cx, reopen_config).inspect_err(|error| {
        log::error!("{error}");
        crate::ui::toast::error(error.clone());
    })
}

fn focus_or_open_main_window<V: 'static>(
    cx: &mut App,
    mut open_window: impl FnMut(&mut App),
) -> bool {
    if app_actions::focus_existing_window::<V>(cx) {
        return false;
    }

    if app_actions::has_window::<V>(cx) {
        return false;
    }

    open_window(cx);
    true
}

fn start_theme_install_from_deeplink(cx: &mut App, slug: String) {
    let loading_id = crate::ui::toast::loading(format!("Fetching theme \"{slug}\"..."));

    cx.spawn(async move |cx: &mut AsyncApp| {
        let fetch_result = cx
            .background_executor()
            .spawn(async move { theme_store::fetch_theme_for_deeplink_blocking(&slug) })
            .await;

        crate::ui::toast::dismiss_toast(loading_id);

        match fetch_result {
            Ok(theme) => {
                let title = "Install Theme";
                let message = format!(
                    "Install theme \"{}\" into your local theme library?",
                    theme.name
                );
                if !termy_native_sdk::confirm(title, &message) {
                    return;
                }

                let install_loading_id =
                    crate::ui::toast::loading(format!("Installing {}...", theme.name));
                let install_result = cx
                    .background_executor()
                    .spawn(async move { theme_store::install_theme_from_store_blocking(theme) })
                    .await;
                crate::ui::toast::dismiss_toast(install_loading_id);

                let _ = cx.update(|cx| match install_result {
                    Ok(installed_theme) => {
                        crate::ui::toast::success(installed_theme.message.clone());
                        app_actions::update_open_settings_windows(cx, |view, settings_cx| {
                            view.apply_theme_store_install(
                                &installed_theme.slug,
                                &installed_theme.version,
                                settings_cx,
                            );
                        });
                        app_actions::refresh_open_terminal_theme_assets(cx);
                    }
                    Err(error) => {
                        log::error!("Failed to install theme from deeplink: {error}");
                        crate::ui::toast::error(error);
                    }
                });
            }
            Err(error) => {
                log::error!("Failed to fetch theme from deeplink: {error}");
                crate::ui::toast::error(error);
            }
        }
    })
    .detach();
}

fn dispatch_deeplink(
    cx: &mut App,
    route: DeepLinkRoute,
    route_argument: Option<DeepLinkArgument>,
) -> Result<(), String> {
    match route {
        DeepLinkRoute::Activate => Ok(()),
        DeepLinkRoute::NewTab => {
            let (command, dir) = match route_argument {
                Some(DeepLinkArgument::NewTab(payload)) => (payload.command, payload.dir),
                Some(DeepLinkArgument::Value(_)) | None => (None, None),
            };
            app_actions::open_new_tab_in_main_window(cx, command, dir)
        }
        DeepLinkRoute::Settings => app_actions::open_settings_or_config_file(cx),
        DeepLinkRoute::OpenConfig => app_actions::open_config_file(),
        DeepLinkRoute::ThemeInstall => {
            let slug = route_argument
                .and_then(|argument| match argument {
                    DeepLinkArgument::Value(value) => Some(value),
                    DeepLinkArgument::NewTab(_) => None,
                })
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| "Theme install deeplink requires a slug".to_string())?;
            start_theme_install_from_deeplink(cx, slug);
            Ok(())
        }
    }
}

fn handle_open_urls_with_main_window<V: 'static>(
    cx: &mut App,
    urls: &[String],
    mut open_window: impl FnMut(&mut App),
    mut dispatch: impl FnMut(&mut App, DeepLinkRoute, Option<DeepLinkArgument>) -> Result<(), String>,
) {
    for raw_url in urls {
        if let Some(dir) = deeplink::directory_from_open_target(raw_url) {
            log::info!("Handling folder open: {raw_url}");
            let argument = Some(DeepLinkArgument::NewTab(deeplink::NewTabDeepLink {
                command: None,
                dir: Some(dir),
            }));
            let _ = focus_or_open_main_window::<V>(cx, &mut open_window);
            if let Err(error) = dispatch(cx, DeepLinkRoute::NewTab, argument) {
                log::error!("Failed to open folder {raw_url}: {error}");
                crate::ui::toast::error(error);
            }
            continue;
        }
        match DeepLinkRoute::parse(raw_url) {
            Ok((route, route_argument)) => {
                log::info!("Handling deeplink: {raw_url}");
                let _ = focus_or_open_main_window::<V>(cx, &mut open_window);
                if let Err(error) = dispatch(cx, route, route_argument) {
                    log::error!("Failed to handle deeplink {raw_url}: {error}");
                    crate::ui::toast::error(error);
                }
            }
            Err(error) => {
                log::warn!("Rejected deeplink {raw_url}: {error}");
                crate::ui::toast::error(error);
            }
        }
    }
}

fn handle_open_urls(cx: &mut App, urls: &[String]) {
    handle_open_urls_with_main_window::<TerminalView>(
        cx,
        urls,
        reopen_main_window,
        dispatch_deeplink,
    );
}

fn spawn_deeplink_listener(cx: &mut App, deeplink_rx: Receiver<Vec<String>>) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Ok(urls) = deeplink_rx.recv_async().await {
            let _ = cx.update(|cx| handle_open_urls(cx, &urls));
        }
    })
    .detach();
}

fn main() {
    launch_probe::mark_process_start();
    let cli_args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(status) = ssh::run_askpass_if_requested(&cli_args) {
        std::process::exit(status);
    }
    if cli_delegate::should_delegate_to_cli(&cli_args) {
        cli_delegate::delegate_to_cli_or_exit(cli_args);
    }

    env_logger::init();
    crash_log::install_panic_hook();

    let mut startup_arguments = parse_startup_arguments(cli_args);
    let (deeplink_tx, deeplink_rx) = flume::unbounded::<Vec<String>>();
    match instance::claim_or_forward(&instance::urls_to_forward(&startup_arguments)) {
        Ok(instance::InstanceClaim::Forwarded) => {
            eprintln!("Termy is already running; activating the existing window.");
            std::process::exit(0);
        }
        Ok(instance::InstanceClaim::Primary(guard)) => {
            instance::spawn_listener(guard, deeplink_tx.clone());
        }
        Err(error) => log::warn!("Termy instance handoff unavailable: {error}"),
    }

    let application = Application::new().with_assets(crate::asset_source::EmbeddedAssets);
    launch_probe::record_stage("platform_created");

    application.on_reopen(|cx| {
        let _ = reopen_if_no_windows(cx, reopen_main_window);
    });
    application.on_open_urls({
        let deeplink_tx_urls = deeplink_tx.clone();
        move |urls| {
            if let Err(error) = deeplink_tx_urls.send(urls) {
                log::error!("Failed to enqueue deeplink event: {error}");
            }
        }
    });

    application.run(move |cx: &mut App| {
        launch_probe::record_stage("application_running");

        let mut pending_urls = Vec::new();
        while let Ok(urls) = deeplink_rx.try_recv() {
            pending_urls.extend(urls);
        }
        absorb_pending_open_urls(&mut startup_arguments, pending_urls);
        fold_startup_new_tab_into_working_dir(&mut startup_arguments);
        let leftover_deeplinks = startup_arguments.deeplinks.clone();

        cx.on_action(|_: &OpenConfig, _cx| {
            if let Err(error) = app_actions::open_config_file() {
                log::error!("Failed to open config file: {error}");
                crate::ui::toast::error(error);
            }
        });
        cx.on_action(|_: &OpenSettings, cx| {
            if let Err(error) = app_actions::open_settings_or_config_file(cx) {
                log::error!("{error}");
                crate::ui::toast::error(error);
            }
        });

        let mut startup_config_error = None;
        let startup_load =
            config::load_runtime_config(&mut startup_config_error, "Failed to load config");
        let mut app_config = startup_load.config;
        launch_probe::record_stage("config_loaded");
        if let Some(working_dir) = startup_arguments.working_dir {
            app_config.working_dir = Some(working_dir);
        }
        app_icon::apply_at_startup(&app_config);
        launch_probe::record_stage("icon_applied");
        if let Some(message) = guard_tmux_startup(&mut app_config) {
            log::warn!("{message}");
            crate::ui::toast::warning(message);
        }
        // Keep startup menus/keybinds aligned with the active runtime capability set.
        let tmux_runtime_active = if cfg!(target_os = "windows") {
            false
        } else {
            app_config.tmux_enabled
        };
        keybindings::install_keybindings(cx, &app_config, tmux_runtime_active);
        launch_probe::record_stage("keybindings_installed");
        let startup_config = app_config;

        if let Err(error) = open_main_window(cx, startup_config) {
            log::error!("{error}");
            StartupBlocker::MainWindowOpen(error).present_alert_and_exit();
        }

        if !leftover_deeplinks.is_empty()
            && let Err(error) = deeplink_tx.send(leftover_deeplinks)
        {
            log::error!("Failed to enqueue leftover startup deeplink: {error}");
        }
        spawn_deeplink_listener(cx, deeplink_rx);
        if let Some(executable) = current_executable() {
            let open_tab_tx = deeplink_tx.clone();
            if let Err(error) =
                termy_native_sdk::register_open_tab_here(&executable, move |directory| {
                    let url = deeplink::new_tab_deeplink_for_dir(&directory.to_string_lossy());
                    if let Err(error) = open_tab_tx.send(vec![url]) {
                        log::error!("Failed to enqueue Finder/file-manager tab: {error}");
                    }
                })
            {
                log::warn!("File manager integration was not registered: {error}");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{
        DeepLinkArgument, DeepLinkRoute, MIN_WINDOW_HEIGHT, MIN_WINDOW_WIDTH, StartupArguments,
        absorb_pending_open_urls, focus_or_open_main_window, fold_startup_new_tab_into_working_dir,
        guard_tmux_startup, handle_open_urls_with_main_window, normalized_startup_window_size,
        parse_startup_arguments, reopen_if_no_windows,
    };
    #[cfg(target_os = "windows")]
    use super::{
        LEGACY_DEFAULT_WINDOW_HEIGHT, LEGACY_DEFAULT_WINDOW_WIDTH, WINDOWS_DEFAULT_WINDOW_HEIGHT,
        WINDOWS_DEFAULT_WINDOW_WIDTH, should_apply_windows_startup_resize,
    };
    use crate::app_actions;
    use crate::config::AppConfig;
    use crate::deeplink::NewTabDeepLink;
    use gpui::{
        App, AppContext, Context, IntoElement, Render, TestAppContext, Window, WindowOptions, div,
        px, size,
    };
    use std::cell::RefCell;

    struct ReopenTestView;

    impl Render for ReopenTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    fn open_test_window(cx: &mut App) {
        cx.open_window(WindowOptions::default(), |_window, cx| {
            cx.new(|_cx| ReopenTestView)
        })
        .expect("test window should open");
    }

    #[test]
    fn startup_arguments_parse_positional_working_directory() {
        let parsed = parse_startup_arguments(["/tmp/project"]);
        assert_eq!(parsed.working_dir.as_deref(), Some("/tmp/project"));
        assert!(parsed.deeplinks.is_empty());
    }

    #[test]
    fn startup_arguments_parse_working_directory_flag() {
        let parsed = parse_startup_arguments(["--working-directory", "/tmp/project"]);
        assert_eq!(parsed.working_dir.as_deref(), Some("/tmp/project"));
    }

    #[test]
    fn startup_arguments_keep_deeplinks() {
        let parsed = parse_startup_arguments(["termy://new?dir=%2Ftmp%2Fproject"]);
        assert_eq!(parsed.working_dir, None);
        assert_eq!(parsed.deeplinks, vec!["termy://new?dir=%2Ftmp%2Fproject"]);
    }

    #[test]
    fn fold_single_new_tab_deeplink_into_first_window_working_dir() {
        let mut startup = parse_startup_arguments(["termy://new?dir=%2Ftmp%2Fproject"]);
        fold_startup_new_tab_into_working_dir(&mut startup);
        assert_eq!(startup.working_dir.as_deref(), Some("/tmp/project"));
        assert!(startup.deeplinks.is_empty());
    }

    #[test]
    fn fold_leaves_settings_deeplinks_for_later_dispatch() {
        let mut startup = parse_startup_arguments(["termy://settings"]);
        fold_startup_new_tab_into_working_dir(&mut startup);
        assert_eq!(startup.working_dir, None);
        assert_eq!(startup.deeplinks, vec!["termy://settings"]);
    }

    #[cfg(unix)]
    #[test]
    fn absorb_open_urls_uses_the_first_folder_as_working_dir() {
        let mut startup = StartupArguments::default();
        absorb_pending_open_urls(
            &mut startup,
            vec![
                "file:///tmp/first".to_string(),
                "file:///tmp/second".to_string(),
            ],
        );
        assert_eq!(startup.working_dir.as_deref(), Some("/tmp/first"));
        assert_eq!(
            startup.deeplinks,
            vec!["termy://new?dir=%2Ftmp%2Fsecond".to_string()]
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn tmux_startup_guard_falls_back_to_native_when_binary_is_missing() {
        let mut config = AppConfig {
            tmux_enabled: true,
            tmux_binary: "/termy-test-bin/tmux-does-not-exist".to_string(),
            ..AppConfig::default()
        };

        let warning = guard_tmux_startup(&mut config).expect("missing tmux should be guarded");

        assert!(!config.tmux_enabled);
        assert!(warning.contains("starting in native mode"));
        assert!(warning.contains("tmux preflight failed"));
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn tmux_startup_guard_falls_back_in_bare_environment() {
        let mut config = AppConfig {
            tmux_enabled: true,
            tmux_binary: "tmux".to_string(),
            tmux_command_prefix: Some("env PATH=/termy-test-bin".to_string()),
            ..AppConfig::default()
        };

        let warning = guard_tmux_startup(&mut config).expect("bare PATH should not contain tmux");

        assert!(!config.tmux_enabled);
        assert!(warning.contains("starting in native mode"));
    }

    #[gpui::test]
    fn reopen_if_no_windows_opens_a_window(cx: &mut TestAppContext) {
        assert!(cx.windows().is_empty(), "expected no windows at test start");

        let reopened = cx.update(|app| reopen_if_no_windows(app, open_test_window));

        assert!(
            reopened,
            "expected reopen hook to run when no windows exist"
        );
        assert_eq!(cx.windows().len(), 1);
    }

    #[gpui::test]
    fn reopen_if_no_windows_does_not_open_when_window_exists(cx: &mut TestAppContext) {
        cx.update(open_test_window);
        assert_eq!(cx.windows().len(), 1);

        let reopened = cx.update(|app| reopen_if_no_windows(app, open_test_window));

        assert!(
            !reopened,
            "expected reopen hook to be skipped when a window already exists"
        );
        assert_eq!(cx.windows().len(), 1);
    }

    #[gpui::test]
    fn focus_or_open_main_window_opens_when_missing(cx: &mut TestAppContext) {
        assert_eq!(cx.windows().len(), 0);

        let opened =
            cx.update(|app| focus_or_open_main_window::<ReopenTestView>(app, open_test_window));

        assert!(opened, "expected missing window to be opened");
        assert_eq!(cx.windows().len(), 1);
    }

    #[gpui::test]
    fn focus_or_open_main_window_reuses_existing_window(cx: &mut TestAppContext) {
        cx.update(open_test_window);
        assert_eq!(cx.windows().len(), 1);

        let opened =
            cx.update(|app| focus_or_open_main_window::<ReopenTestView>(app, open_test_window));

        assert!(
            !opened,
            "expected existing main window to be reused without opening another"
        );
        assert_eq!(cx.windows().len(), 1);
    }

    #[gpui::test]
    fn handle_open_urls_opens_window_before_dispatch(cx: &mut TestAppContext) {
        let handled = RefCell::new(Vec::new());

        cx.update(|app| {
            handle_open_urls_with_main_window::<ReopenTestView>(
                app,
                &[String::from("termy://settings")],
                open_test_window,
                |_, route, route_argument| {
                    handled.borrow_mut().push((route, route_argument));
                    Ok(())
                },
            );
        });

        assert_eq!(cx.windows().len(), 1);
        assert_eq!(*handled.borrow(), vec![(DeepLinkRoute::Settings, None)]);
    }

    #[gpui::test]
    fn bare_deeplink_opens_window_without_error_route(cx: &mut TestAppContext) {
        let handled = RefCell::new(Vec::new());

        cx.update(|app| {
            handle_open_urls_with_main_window::<ReopenTestView>(
                app,
                &[String::from("termy://")],
                open_test_window,
                |_, route, route_argument| {
                    handled.borrow_mut().push((route, route_argument));
                    Ok(())
                },
            );
        });

        assert_eq!(cx.windows().len(), 1);
        assert_eq!(*handled.borrow(), vec![(DeepLinkRoute::Activate, None)]);
    }

    #[cfg(unix)]
    #[gpui::test]
    fn folder_open_target_dispatches_a_new_tab(cx: &mut TestAppContext) {
        let handled = RefCell::new(Vec::new());

        cx.update(|app| {
            handle_open_urls_with_main_window::<ReopenTestView>(
                app,
                &[String::from("file:///tmp/demo")],
                open_test_window,
                |_, route, route_argument| {
                    handled.borrow_mut().push((route, route_argument));
                    Ok(())
                },
            );
        });

        assert_eq!(cx.windows().len(), 1);
        assert_eq!(
            *handled.borrow(),
            vec![(
                DeepLinkRoute::NewTab,
                Some(DeepLinkArgument::NewTab(NewTabDeepLink {
                    command: None,
                    dir: Some("/tmp/demo".to_string()),
                }))
            )]
        );
    }

    #[gpui::test]
    fn finder_service_reuses_open_window_for_selected_folders(cx: &mut TestAppContext) {
        let handled = RefCell::new(Vec::new());
        let directory = "/tmp/it's a folder & another";
        let url = url::Url::from_directory_path(directory)
            .unwrap()
            .to_string();

        cx.update(|app| {
            open_test_window(app);
            handle_open_urls_with_main_window::<ReopenTestView>(
                app,
                &[url.clone(), url],
                |_| panic!("Finder should reuse the existing main window"),
                |_, route, argument| {
                    handled.borrow_mut().push((route, argument));
                    Ok(())
                },
            );
        });

        assert_eq!(cx.windows().len(), 1);
        let expected = (
            DeepLinkRoute::NewTab,
            Some(DeepLinkArgument::NewTab(NewTabDeepLink {
                command: None,
                dir: Some(format!("{directory}/")),
            })),
        );
        assert_eq!(*handled.borrow(), vec![expected.clone(), expected]);
    }

    #[gpui::test]
    fn new_tab_deeplink_passes_route_without_argument(cx: &mut TestAppContext) {
        let handled = RefCell::new(Vec::new());

        cx.update(|app| {
            handle_open_urls_with_main_window::<ReopenTestView>(
                app,
                &[String::from("termy://new")],
                open_test_window,
                |_, route, route_argument| {
                    handled.borrow_mut().push((route, route_argument));
                    Ok(())
                },
            );
        });

        assert_eq!(cx.windows().len(), 1);
        assert_eq!(*handled.borrow(), vec![(DeepLinkRoute::NewTab, None)]);
    }

    #[test]
    fn normalized_startup_window_size_uses_default_config_values() {
        let config = AppConfig::default();

        assert_eq!(
            normalized_startup_window_size(&config),
            size(px(1280.0), px(820.0))
        );
    }

    #[test]
    fn normalized_startup_window_size_clamps_to_minimums() {
        let config = AppConfig {
            window_width: 100.0,
            window_height: 200.0,
            ..AppConfig::default()
        };

        assert_eq!(
            normalized_startup_window_size(&config),
            size(px(MIN_WINDOW_WIDTH), px(MIN_WINDOW_HEIGHT))
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn normalized_startup_window_size_migrates_legacy_windows_default() {
        let mut config = AppConfig::default();
        config.window_width = LEGACY_DEFAULT_WINDOW_WIDTH;
        config.window_height = LEGACY_DEFAULT_WINDOW_HEIGHT;

        assert_eq!(
            normalized_startup_window_size(&config),
            size(
                px(WINDOWS_DEFAULT_WINDOW_WIDTH),
                px(WINDOWS_DEFAULT_WINDOW_HEIGHT)
            )
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_startup_resize_correction_skips_matching_sizes() {
        let target = size(px(1280.0), px(820.0));

        assert!(!should_apply_windows_startup_resize(target, target));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_startup_resize_correction_detects_size_mismatch() {
        let current = size(px(900.0), px(700.0));
        let target = size(px(1280.0), px(820.0));

        assert!(should_apply_windows_startup_resize(current, target));
    }

    #[gpui::test]
    fn new_tab_deeplink_ignores_optional_command(cx: &mut TestAppContext) {
        let handled = RefCell::new(Vec::new());

        cx.update(|app| {
            handle_open_urls_with_main_window::<ReopenTestView>(
                app,
                &[String::from("termy://new?cmd=git%20status")],
                open_test_window,
                |_, route, route_argument| {
                    handled.borrow_mut().push((route, route_argument));
                    Ok(())
                },
            );
        });

        assert_eq!(cx.windows().len(), 1);
        assert_eq!(*handled.borrow(), vec![(DeepLinkRoute::NewTab, None)]);
    }

    #[gpui::test]
    fn new_tab_deeplink_ignores_optional_command_and_passes_dir(cx: &mut TestAppContext) {
        let handled = RefCell::new(Vec::new());

        cx.update(|app| {
            handle_open_urls_with_main_window::<ReopenTestView>(
                app,
                &[String::from(
                    "termy://new?cmd=git%20status&dir=%2Ftmp%2Fdemo",
                )],
                open_test_window,
                |_, route, route_argument| {
                    handled.borrow_mut().push((route, route_argument));
                    Ok(())
                },
            );
        });

        assert_eq!(cx.windows().len(), 1);
        assert_eq!(
            *handled.borrow(),
            vec![(
                DeepLinkRoute::NewTab,
                Some(DeepLinkArgument::NewTab(NewTabDeepLink {
                    command: None,
                    dir: Some("/tmp/demo".to_string()),
                }))
            )]
        );
    }

    #[gpui::test]
    fn theme_install_deeplink_passes_slug_argument(cx: &mut TestAppContext) {
        let handled = RefCell::new(Vec::new());

        cx.update(|app| {
            handle_open_urls_with_main_window::<ReopenTestView>(
                app,
                &[String::from(
                    "termy://store/theme-install?slug=catppuccin-mocha",
                )],
                open_test_window,
                |_, route, route_argument| {
                    handled.borrow_mut().push((route, route_argument));
                    Ok(())
                },
            );
        });

        assert_eq!(cx.windows().len(), 1);
        assert_eq!(
            *handled.borrow(),
            vec![(
                DeepLinkRoute::ThemeInstall,
                Some(DeepLinkArgument::Value("catppuccin-mocha".to_string()))
            )]
        );
    }

    #[gpui::test]
    fn settings_deeplink_reuses_existing_settings_window(cx: &mut TestAppContext) {
        cx.update(|app| {
            app_actions::open_settings_window(app).expect("settings window should open");
            handle_open_urls_with_main_window::<ReopenTestView>(
                app,
                &[String::from("termy://settings")],
                open_test_window,
                super::dispatch_deeplink,
            );
        });

        let settings_count = cx
            .windows()
            .into_iter()
            .filter(|handle| {
                handle
                    .downcast::<crate::settings_view::SettingsWindow>()
                    .is_some()
            })
            .count();

        assert_eq!(settings_count, 1);
    }
}
