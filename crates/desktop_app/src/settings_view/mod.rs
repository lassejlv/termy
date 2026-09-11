use crate::colors::TerminalColors;
use crate::config::{self, AppConfig, SystemAppearance, system_appearance_from_window};
use crate::text_input::{TextInputAlignment, TextInputElement, TextInputProvider, TextInputState};
use crate::theme_store::{self, ThemeStoreAuthSession, ThemeStoreAuthUser, ThemeStoreTheme};
use crate::ui::scrollbar::{self as ui_scrollbar, ScrollbarPaintStyle, ScrollbarRange};
use gpui::{
    AnyElement, AsyncApp, Bounds, Context, FocusHandle, Font, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit,
    ParentElement, Pixels, Render, Rgba, ScrollAnchor, ScrollHandle, ScrollWheelEvent,
    SharedString, StatefulInteractiveElement, Styled, StyledImage, TextAlign, WeakEntity, Window,
    WindowBackgroundAppearance, canvas, deferred, div, img, point, prelude::FluentBuilder, px, svg,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{
    LazyLock,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};
use termy_command_core::CommandId;
use termy_config_core::{
    RootSettingId, RootSettingValueKind, SettingsSection as CoreSettingsSection,
    color_setting_from_key, color_setting_specs, format_line_height, root_setting_default_value,
    root_setting_enum_choices, root_setting_from_key, root_setting_specs, root_setting_value_kind,
};
use termy_plugin_runtime::{InstalledPlugin, PluginRuntime, PluginSettingState};

mod colors;
mod components;
mod input_mode;
mod keybinds;
mod plugins;
mod search;
mod sections;
mod ssh;
mod state;
mod state_apply;
mod style;
#[cfg(test)]
mod test_utils;

use self::search::SearchableSetting;
use self::ssh::{ActiveSshInput, SshHostForm};
use self::state::{ActiveTextInput, DropdownOption, EditableField};
use input_mode::KeyInputMode;

const SIDEBAR_WIDTH: f32 = 208.0;
const SIDEBAR_ICON_SIZE: f32 = 18.0;
const SIDEBAR_ITEM_HEIGHT: f32 = 32.0;
const SIDEBAR_ITEM_RADIUS: f32 = 7.0;
// Accent bar marking the selected sidebar item; mirrors the active-tab and
// command-palette selection indicators so the chrome speaks one language.
const SIDEBAR_SELECTED_ACCENT_WIDTH: f32 = 2.0;
const SIDEBAR_SELECTED_ACCENT_INSET_Y: f32 = 8.0;
const SIDEBAR_GROUP_GAP: f32 = 16.0;
const SIDEBAR_GROUP_LABEL_SIZE: f32 = 11.0;
// Settings content is constrained and centered like native macOS System
// Settings so rows do not stretch label-to-control across wide windows.
const SETTINGS_CONTENT_MAX_WIDTH: f32 = 800.0;
const SETTINGS_CONTROL_WIDTH: f32 = 300.0;
const SETTINGS_CONTROL_HEIGHT: f32 = 30.0;
const NUMERIC_STEP_BUTTON_SIZE: f32 = 22.0;
const SETTINGS_INPUT_TEXT_SIZE: f32 = 13.0;
const SETTINGS_SEARCH_NAV_THROTTLE_MS: u64 = 70;
const SETTINGS_SCROLL_ANIMATION_DURATION_MS: u64 = 170;
const SETTINGS_SCROLL_ANIMATION_TICK_MS: u64 = 16;
const SETTINGS_SCROLLBAR_WIDTH: f32 = 8.0;
const SETTINGS_SCROLLBAR_MIN_THUMB_HEIGHT: f32 = 18.0;
const SETTINGS_SCROLLBAR_TRACK_ALPHA: f32 = 0.10;
const SETTINGS_SCROLLBAR_THUMB_ALPHA: f32 = 0.42;
const SETTINGS_SCROLLBAR_THUMB_ACTIVE_ALPHA: f32 = 0.58;
const SETTINGS_OVERLAY_PANEL_ALPHA_FLOOR_RATIO: f32 = 0.72;
const SETTINGS_SWITCH_WIDTH: f32 = 38.0;
const SETTINGS_SWITCH_HEIGHT: f32 = 22.0;
const SETTINGS_SWITCH_KNOB_SIZE: f32 = 18.0;
const SETTINGS_SEARCH_PREVIEW_LIMIT: usize = 6;
const SETTINGS_SLIDER_VALUE_WIDTH: f32 = 60.0;
const SETTINGS_OPACITY_STEP_RATIO: f32 = 0.05;
const SETTINGS_CONTROL_INNER_PADDING: f32 = 8.0;
const SETTINGS_OPACITY_CONTROL_GAP: f32 = 6.0;
const SETTINGS_CARD_RADIUS: f32 = 10.0;
const SETTINGS_INPUT_RADIUS: f32 = 6.0;
const SETTINGS_BUTTON_RADIUS: f32 = 6.0;
const SETTINGS_SWITCH_RADIUS: f32 = 11.0;
// Section title and subtitle sizes now live in `termy_ui::metrics`.
const GROUP_TITLE_SIZE: f32 = 11.0;
const CARD_GAP: f32 = 22.0;
const CARD_ROW_PADDING_X: f32 = 16.0;
const CARD_ROW_PADDING_Y: f32 = 11.0;
const CONTENT_GUTTER_X: f32 = 32.0;
const CONTENT_GUTTER_Y: f32 = 24.0;
static NEXT_BACKGROUND_OPACITY_PREVIEW_OWNER_ID: AtomicU64 = AtomicU64::new(1);
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SettingsSection {
    Appearance,
    Terminal,
    Ssh,
    Tabs,
    ThemeStore,
    Plugins,
    Advanced,
    Colors,
    Keybindings,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct BackgroundOpacityDragState {
    start_local_x: f32,
    start_ratio: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ScrollbarDragState {
    thumb_grab_offset: f32,
}

pub struct SettingsWindow {
    active_section: SettingsSection,
    config: AppConfig,
    config_path: Option<PathBuf>,
    config_fingerprint: Option<u64>,
    last_config_error_message: Option<String>,
    available_font_families: Vec<String>,
    focus_handle: FocusHandle,
    active_input: Option<ActiveTextInput>,
    ssh_input: Option<ActiveSshInput>,
    content_scroll_handle: ScrollHandle,
    setting_scroll_anchors: HashMap<&'static str, ScrollAnchor>,
    searchable_settings: Vec<SearchableSetting>,
    searchable_setting_indices: HashMap<&'static str, usize>,
    sidebar_search_state: TextInputState,
    sidebar_search_active: bool,
    sidebar_search_selecting: bool,
    sidebar_search_cache_query: String,
    sidebar_search_match_cache: HashSet<&'static str>,
    theme_store_search_state: TextInputState,
    theme_store_search_active: bool,
    theme_store_search_selecting: bool,
    search_navigation_last_target: Option<&'static str>,
    search_navigation_last_jump_at: Option<Instant>,
    capturing_action: Option<CommandId>,
    background_opacity_preview_owner_id: u64,
    preview_background_opacity: Option<config::BackgroundOpacityPreview>,
    background_opacity_drag_state: Option<BackgroundOpacityDragState>,
    background_opacity_slider_bounds: Option<Bounds<Pixels>>,
    scrollbar_drag_state: Option<ScrollbarDragState>,
    scrollbar_lane_bounds: Option<Bounds<Pixels>>,
    hovered_reset_setting: Option<&'static str>,
    hovered_reset_section: Option<SettingsSection>,
    scroll_animation_token: u64,
    colors: TerminalColors,
    system_appearance: SystemAppearance,
    appearance_subscription: Option<gpui::Subscription>,
    last_window_background_appearance: Option<WindowBackgroundAppearance>,
    theme_store_themes: Vec<ThemeStoreTheme>,
    theme_store_loaded: bool,
    theme_store_loading: bool,
    theme_store_error: Option<String>,
    theme_store_from_cache: bool,
    theme_store_auth_session: Option<ThemeStoreAuthSession>,
    theme_store_auth_loading: bool,
    theme_store_auth_error: Option<String>,
    theme_store_installed_versions: HashMap<String, String>,
    plugin_runtime: PluginRuntime,
    installed_plugins: Vec<InstalledPlugin>,
    plugin_settings: BTreeMap<String, Vec<PluginSettingState>>,
    plugin_setting_input: Option<plugins::ActivePluginSettingInput>,
    active_plugin_setting_select: Option<(String, String)>,
    plugin_inventory_errors: Vec<String>,
    plugin_bun_path: Option<PathBuf>,
    plugin_bun_error: Option<String>,
    plugin_operation_in_flight: bool,
    ssh_hosts: Vec<termy_ssh_core::SshHost>,
    ssh_hosts_error: Option<String>,
    ssh_form: Option<SshHostForm>,
}

impl SettingsWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut last_config_error_message = None;
        let loaded = config::load_runtime_config(
            &mut last_config_error_message,
            "Failed to load config for settings view",
        );
        let config = loaded.config;
        let config_path = loaded.path;
        let config_fingerprint = loaded.fingerprint;
        let config_change_rx = config::subscribe_config_changes();
        let background_opacity_preview_rx = config::subscribe_background_opacity_preview();
        #[cfg(test)]
        let _ = &config_change_rx;
        #[cfg(test)]
        let _ = &background_opacity_preview_rx;
        let available_font_families =
            crate::font_families::available_font_families(window.text_system().all_font_names());
        let system_appearance = system_appearance_from_window(window.appearance());
        let colors = TerminalColors::from_config(&config, system_appearance);
        let searchable_settings = Self::build_searchable_settings();
        let searchable_setting_indices =
            Self::build_searchable_setting_indices(&searchable_settings);
        let content_scroll_handle = ScrollHandle::new();
        let setting_scroll_anchors = Self::build_setting_scroll_anchors(&content_scroll_handle);
        let plugin_runtime = PluginRuntime::new(config_path.as_deref());
        let (installed_plugins, plugin_inventory_errors) = match plugin_runtime.installed_plugins()
        {
            Ok(inventory) => (inventory.plugins, inventory.errors),
            Err(error) => (Vec::new(), vec![error]),
        };
        let (plugin_bun_path, plugin_bun_error) = match plugin_runtime.bun_path() {
            Ok(path) => (path, None),
            Err(error) => (None, Some(error)),
        };
        let (ssh_hosts, ssh_hosts_error) = match crate::ssh::load_hosts(config_path.as_deref()) {
            Ok(hosts) => (hosts, None),
            Err(error) => (Vec::new(), Some(error)),
        };
        let mut view = Self {
            active_section: SettingsSection::Advanced,
            config,
            config_path,
            config_fingerprint,
            last_config_error_message,
            available_font_families,
            focus_handle: cx.focus_handle(),
            active_input: None,
            ssh_input: None,
            content_scroll_handle,
            setting_scroll_anchors,
            searchable_settings,
            searchable_setting_indices,
            sidebar_search_state: TextInputState::new(String::new()),
            sidebar_search_active: true,
            sidebar_search_selecting: false,
            sidebar_search_cache_query: String::new(),
            sidebar_search_match_cache: HashSet::new(),
            theme_store_search_state: TextInputState::new(String::new()),
            theme_store_search_active: false,
            theme_store_search_selecting: false,
            search_navigation_last_target: None,
            search_navigation_last_jump_at: None,
            capturing_action: None,
            background_opacity_preview_owner_id: NEXT_BACKGROUND_OPACITY_PREVIEW_OWNER_ID
                .fetch_add(1, Ordering::Relaxed),
            preview_background_opacity: config::current_background_opacity_preview(),
            background_opacity_drag_state: None,
            background_opacity_slider_bounds: None,
            scrollbar_drag_state: None,
            scrollbar_lane_bounds: None,
            hovered_reset_setting: None,
            hovered_reset_section: None,
            scroll_animation_token: 0,
            colors,
            system_appearance,
            appearance_subscription: None,
            last_window_background_appearance: None,
            theme_store_themes: Vec::new(),
            theme_store_loaded: false,
            theme_store_loading: false,
            theme_store_error: None,
            theme_store_from_cache: false,
            theme_store_auth_session: None,
            theme_store_auth_loading: false,
            theme_store_auth_error: None,
            theme_store_installed_versions: theme_store::load_installed_theme_versions(),
            plugin_runtime,
            installed_plugins,
            plugin_settings: BTreeMap::new(),
            plugin_setting_input: None,
            active_plugin_setting_select: None,
            plugin_inventory_errors,
            plugin_bun_path,
            plugin_bun_error,
            plugin_operation_in_flight: false,
            ssh_hosts,
            ssh_hosts_error,
            ssh_form: None,
        };
        view.focus_handle.focus(window);

        #[cfg(not(test))]
        {
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                loop {
                    if config_change_rx.recv_async().await.is_err() {
                        break;
                    }
                    while config_change_rx.try_recv().is_ok() {}
                    let result = cx.update(|cx| {
                        this.update(cx, |view, cx| {
                            if view.reload_config_if_changed(cx) {
                                cx.notify();
                            }
                        })
                    });
                    if result.is_err() {
                        break;
                    }
                }
            })
            .detach();
        }

        #[cfg(not(test))]
        {
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                loop {
                    let Ok(mut opacity) = background_opacity_preview_rx.recv_async().await else {
                        break;
                    };
                    while let Ok(next_opacity) = background_opacity_preview_rx.try_recv() {
                        opacity = next_opacity;
                    }
                    let result = cx.update(|cx| {
                        this.update(cx, |view, cx| {
                            if view.sync_background_opacity_preview(opacity) {
                                cx.notify();
                            }
                        })
                    });
                    if result.is_err() {
                        break;
                    }
                }
            })
            .detach();
        }

        view.appearance_subscription =
            Some(cx.observe_window_appearance(window, |view, window, cx| {
                view.handle_window_appearance_change(window.appearance(), cx);
            }));

        view
    }

    fn handle_window_appearance_change(
        &mut self,
        appearance: gpui::WindowAppearance,
        cx: &mut Context<Self>,
    ) {
        let next = system_appearance_from_window(appearance);
        if self.system_appearance == next {
            return;
        }
        self.system_appearance = next;
        if self.config.theme_mode != config::AppearanceMode::Manual {
            self.colors = TerminalColors::from_config(&self.config, self.system_appearance);
            cx.notify();
        }
    }

    fn theme_store_api_base_url() -> String {
        theme_store::theme_store_api_base_url()
    }

    fn theme_store_registry_url() -> String {
        theme_store::theme_store_registry_url()
    }

    fn ensure_theme_store_themes_loaded(&mut self, cx: &mut Context<Self>) {
        if self.theme_store_loaded || self.theme_store_loading {
            return;
        }
        self.load_cached_theme_store_themes();
        self.refresh_theme_store_themes(cx);
    }

    fn load_cached_theme_store_themes(&mut self) {
        let registry_url = Self::theme_store_registry_url();
        let Some(themes) = theme_store::load_cached_theme_store_themes(&registry_url) else {
            return;
        };
        self.theme_store_themes = themes;
        self.theme_store_loaded = true;
        self.theme_store_error = None;
        self.theme_store_from_cache = true;
    }

    fn refresh_theme_store_themes(&mut self, cx: &mut Context<Self>) {
        if self.theme_store_loading {
            return;
        }

        self.theme_store_loading = true;
        self.theme_store_error = None;
        self.theme_store_from_cache = false;
        let registry_url = Self::theme_store_registry_url();

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result = smol::unblock(move || {
                theme_store::fetch_theme_store_themes_blocking(&registry_url)
            })
            .await;

            let _ = cx.update(|cx| {
                this.update(cx, |view, cx| {
                    view.theme_store_loading = false;
                    match result {
                        Ok((themes, from_cache)) => {
                            view.theme_store_themes = themes;
                            view.theme_store_loaded = true;
                            view.theme_store_error = None;
                            view.theme_store_from_cache = from_cache;
                        }
                        Err(error) => {
                            view.theme_store_loaded = true;
                            if view.theme_store_themes.is_empty() {
                                view.theme_store_error = Some(error);
                                view.theme_store_from_cache = false;
                            } else {
                                log::warn!(
                                    "Theme registry refresh failed; keeping cached themes: {error}"
                                );
                                view.theme_store_error = None;
                                view.theme_store_from_cache = true;
                            }
                        }
                    }
                    cx.notify();
                })
            });
        })
        .detach();
    }

    fn install_theme_store_theme(&mut self, theme: ThemeStoreTheme, cx: &mut Context<Self>) {
        let installed_slug = theme.slug.trim().to_ascii_lowercase();
        let installed_version = theme.latest_version.clone().unwrap_or_default();
        let loading_id = crate::ui::toast::loading(format!("Installing {}...", theme.name));

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result =
                smol::unblock(move || theme_store::install_theme_from_store_blocking(theme)).await;

            crate::ui::toast::dismiss_toast(loading_id);

            let _ = cx.update(|cx| {
                this.update(cx, |view, cx| {
                    match result {
                        Ok(installed_theme) => {
                            crate::ui::toast::success(installed_theme.message);
                            view.theme_store_installed_versions
                                .insert(installed_slug.clone(), installed_version.clone());
                            if let Err(error) = theme_store::persist_installed_theme_versions(
                                &view.theme_store_installed_versions,
                            ) {
                                log::error!("Failed to persist installed theme state: {error}");
                            }
                            let _ = view.reload_config_if_changed(cx);
                            view.reload_theme_assets(cx);
                            crate::app_actions::refresh_open_terminal_theme_assets(cx);
                        }
                        Err(error) => crate::ui::toast::error(error),
                    }
                    cx.notify();
                })
            });
        })
        .detach();
    }

    fn confirm_install_theme_store_theme(
        &mut self,
        theme: ThemeStoreTheme,
        cx: &mut Context<Self>,
    ) {
        let title = "Install Theme";
        let message = format!(
            "Install theme \"{}\" into your local theme library?",
            theme.name
        );

        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let confirmed = termy_native_sdk::confirm(title, &message);
            if !confirmed {
                return;
            }

            let _ = cx.update(|cx| {
                this.update(cx, |view, cx| {
                    view.install_theme_store_theme(theme.clone(), cx);
                })
            });
        })
        .detach();
    }

    fn uninstall_theme_store_theme(&mut self, slug: &str, cx: &mut Context<Self>) {
        let key = slug.trim().to_ascii_lowercase();
        if key.is_empty() {
            return;
        }

        if termy_themes::builtin_theme(&key).is_none() {
            let reset = match config::reset_theme_references_in_config(&key) {
                Ok(reset) => reset,
                Err(error) => {
                    log::error!("Failed to reset theme references during uninstall: {error}");
                    crate::ui::toast::error("Failed to reset selected theme");
                    return;
                }
            };
            if reset.theme {
                self.config.theme = config::SHELL_DECIDE_THEME_ID.to_string();
            }
            if reset.theme_light {
                self.config.theme_light = "termy-light".to_string();
            }
            if reset.theme_dark {
                self.config.theme_dark = "termy".to_string();
            }
        }

        match theme_store::uninstall_installed_theme(&key) {
            Ok(true) => {
                self.theme_store_installed_versions.remove(&key);
                let _ = self.reload_config_if_changed(cx);
                self.reload_theme_assets(cx);
                crate::app_actions::refresh_open_terminal_theme_assets(cx);
                crate::ui::toast::success("Theme uninstalled");
            }
            Ok(false) => {
                self.reload_theme_assets(cx);
                crate::app_actions::refresh_open_terminal_theme_assets(cx);
                crate::ui::toast::info("Theme is not installed");
            }
            Err(error) => {
                log::error!("Failed to uninstall theme: {error}");
                crate::ui::toast::error(error);
            }
        }
    }

    pub(crate) fn apply_theme_store_install(
        &mut self,
        slug: &str,
        version: &str,
        cx: &mut Context<Self>,
    ) {
        self.theme_store_installed_versions
            .insert(slug.trim().to_ascii_lowercase(), version.to_string());
        self.reload_theme_assets(cx);
        cx.notify();
    }

    fn reload_theme_assets(&mut self, cx: &mut Context<Self>) {
        self.colors = TerminalColors::from_config(&self.config, self.system_appearance);
        cx.notify();
    }

    fn logout_theme_store_user(&mut self, cx: &mut Context<Self>) {
        if self.theme_store_auth_loading {
            return;
        }
        let Some(session) = self.theme_store_auth_session.clone() else {
            return;
        };

        self.theme_store_auth_loading = true;
        self.theme_store_auth_error = None;
        let api_base = Self::theme_store_api_base_url();

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let session_token = session.session_token.clone();
            let result = smol::unblock(move || {
                theme_store::logout_auth_session_blocking(&api_base, &session_token)?;
                theme_store::clear_auth_session()
            })
            .await;

            let _ = cx.update(|cx| {
                this.update(cx, |view, cx| {
                    view.theme_store_auth_loading = false;
                    match result {
                        Ok(()) => {
                            view.theme_store_auth_session = None;
                            view.theme_store_auth_error = None;
                            crate::ui::toast::success("Logged out from theme store");
                        }
                        Err(error) => {
                            log::error!("Failed to logout from theme store: {error}");
                            view.theme_store_auth_error = Some(error.clone());
                            crate::ui::toast::error(error);
                        }
                    }
                    cx.notify();
                })
            });
        })
        .detach();
    }

    fn theme_store_auth_display_name(user: &ThemeStoreAuthUser) -> String {
        user.name
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| format!("@{}", user.github_login))
    }

    fn theme_store_auth_avatar_fallback_label(user: &ThemeStoreAuthUser) -> String {
        user.github_login
            .chars()
            .next()
            .map_or_else(|| "?".to_string(), |ch| ch.to_ascii_uppercase().to_string())
    }

    pub(super) fn open_url(url: &str) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            if Command::new("open")
                .arg(url)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .is_ok()
            {
                return Ok(());
            }
        }
        #[cfg(target_os = "linux")]
        {
            if Command::new("xdg-open")
                .arg(url)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .is_ok()
            {
                return Ok(());
            }
        }
        #[cfg(target_os = "windows")]
        {
            if Command::new("cmd")
                .args(["/C", "start", "", url])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .is_ok()
            {
                return Ok(());
            }
        }

        if webbrowser::open(url).is_ok() {
            return Ok(());
        }

        Err(format!("Failed to open URL: {url}"))
    }

    fn apply_runtime_config(&mut self, config: AppConfig) -> bool {
        if self.config.app_icon != config.app_icon {
            crate::app_icon::apply(config.app_icon);
        }
        self.colors = TerminalColors::from_config(&config, self.system_appearance);
        self.config = config;
        let previous_preview = self.preview_background_opacity;
        let synced_preview = config::synced_background_opacity_preview(
            self.config.background_opacity,
            previous_preview,
        );
        if synced_preview != previous_preview {
            self.preview_background_opacity = synced_preview;
            if previous_preview
                .is_some_and(|preview| preview.owner_id == self.background_opacity_preview_owner_id)
                && self.background_opacity_drag_state.is_none()
            {
                config::publish_background_opacity_preview(None);
            }
        }
        true
    }

    fn effective_background_opacity(&self) -> f32 {
        config::effective_background_opacity(
            self.config.background_opacity,
            self.preview_background_opacity,
        )
    }

    fn sync_background_opacity_preview(
        &mut self,
        preview: Option<config::BackgroundOpacityPreview>,
    ) -> bool {
        let preview = preview.map(|preview| config::BackgroundOpacityPreview {
            opacity: preview.opacity.clamp(0.0, 1.0),
            ..preview
        });
        if self.preview_background_opacity == preview {
            return false;
        }
        self.preview_background_opacity = preview;
        true
    }

    fn clear_background_opacity_preview(&mut self) -> bool {
        if !self.sync_background_opacity_preview(None) {
            return false;
        }
        config::publish_background_opacity_preview(None);
        true
    }

    fn reload_config_if_changed(&mut self, _cx: &mut Context<Self>) -> bool {
        let Some(path) = self.config_path.clone() else {
            let loaded = config::load_runtime_config(
                &mut self.last_config_error_message,
                "Failed to reload config for settings view",
            );
            self.config_path = loaded.path;
            self.config_fingerprint = loaded.fingerprint;
            return if loaded.loaded_from_disk {
                self.apply_runtime_config(loaded.config)
            } else {
                false
            };
        };

        let Some(fingerprint) = config::config_fingerprint(&path) else {
            return false;
        };

        if self.config_fingerprint == Some(fingerprint) {
            return false;
        }

        let loaded = config::load_runtime_config(
            &mut self.last_config_error_message,
            "Failed to reload config for settings view",
        );
        self.config_path = loaded.path;
        self.config_fingerprint = loaded.fingerprint;
        if loaded.loaded_from_disk {
            self.apply_runtime_config(loaded.config)
        } else {
            false
        }
    }

    fn key_requires_secondary_only(event: &KeyDownEvent, key: &str) -> bool {
        (event.keystroke.modifiers.secondary() || event.keystroke.modifiers.control)
            && !event.keystroke.modifiers.alt
            && !event.keystroke.modifiers.function
            && event.keystroke.key.eq_ignore_ascii_case(key)
    }

    fn cmd_only(modifiers: gpui::Modifiers) -> bool {
        modifiers.secondary() && !modifiers.alt && !modifiers.function
    }

    fn is_plain_escape(event: &KeyDownEvent) -> bool {
        event.keystroke.key.eq_ignore_ascii_case("escape") && !event.keystroke.modifiers.modified()
    }

    fn handle_global_shortcuts(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if Self::key_requires_secondary_only(event, "q") {
            cx.quit();
            return true;
        }

        if Self::key_requires_secondary_only(event, "w") {
            window.remove_window();
            return true;
        }

        false
    }

    fn handle_section_cycle_shortcut(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.active_input.is_none()
            && self.plugin_setting_input.is_none()
            && self.ssh_input.is_none()
            && event.keystroke.key.eq_ignore_ascii_case("tab")
            && !event.keystroke.modifiers.alt
            && !event.keystroke.modifiers.control
            && !event.keystroke.modifiers.function
            && !event.keystroke.modifiers.platform
        {
            self.cycle_active_section(event.keystroke.modifiers.shift, window, cx);
            cx.notify();
            return true;
        }

        false
    }

    fn handle_sidebar_search_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.sidebar_search_active || self.active_input.is_some() {
            return false;
        }

        if Self::cmd_only(event.keystroke.modifiers)
            && event.keystroke.key.eq_ignore_ascii_case("a")
        {
            self.sidebar_search_state.select_all();
            cx.notify();
            return true;
        }

        match event.keystroke.key.as_str() {
            "enter" => self.jump_to_first_search_result(window, cx),
            "escape" => {
                self.blur_sidebar_search();
                cx.notify();
            }
            "backspace" => {
                self.sidebar_search_state.delete_backward();
                self.refresh_search_navigation(window, cx);
            }
            "delete" => {
                self.sidebar_search_state.delete_forward();
                self.refresh_search_navigation(window, cx);
            }
            "left" => {
                self.sidebar_search_state.move_left();
                cx.notify();
            }
            "right" => {
                self.sidebar_search_state.move_right();
                cx.notify();
            }
            "home" => {
                self.sidebar_search_state.move_to_start();
                cx.notify();
            }
            "end" => {
                self.sidebar_search_state.move_to_end();
                cx.notify();
            }
            _ => {}
        }

        true
    }

    fn handle_theme_store_search_key_down(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.theme_store_search_active || self.active_input.is_some() {
            return false;
        }

        if Self::cmd_only(event.keystroke.modifiers)
            && event.keystroke.key.eq_ignore_ascii_case("a")
        {
            self.theme_store_search_state.select_all();
            cx.notify();
            return true;
        }

        match event.keystroke.key.as_str() {
            "escape" => {
                self.theme_store_search_active = false;
                self.theme_store_search_selecting = false;
                cx.notify();
            }
            "backspace" => {
                self.theme_store_search_state.delete_backward();
                cx.notify();
            }
            "delete" => {
                self.theme_store_search_state.delete_forward();
                cx.notify();
            }
            "left" => {
                self.theme_store_search_state.move_left();
                cx.notify();
            }
            "right" => {
                self.theme_store_search_state.move_right();
                cx.notify();
            }
            "home" => {
                self.theme_store_search_state.move_to_start();
                cx.notify();
            }
            "end" => {
                self.theme_store_search_state.move_to_end();
                cx.notify();
            }
            _ => {}
        }

        true
    }

    fn handle_active_input_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if self.ssh_input.is_some() {
            self.handle_ssh_input_key_down(event, cx);
            return;
        }
        if self.plugin_setting_input.is_some() {
            self.handle_plugin_setting_input_key_down(event, cx);
            return;
        }
        let active_field = self.active_input.as_ref().map(|input| input.field);
        let active_input_query = self
            .active_input
            .as_ref()
            .map(|input| input.state.text().to_string())
            .unwrap_or_default();
        let allow_text_editing = active_field.is_some_and(Self::uses_text_input_for_field);

        if Self::cmd_only(event.keystroke.modifiers)
            && event.keystroke.key.eq_ignore_ascii_case("a")
            && let Some(input) = self.active_input.as_mut()
        {
            input.state.select_all();
            cx.notify();
            return;
        }

        // Handle Cmd+V (paste)
        if Self::cmd_only(event.keystroke.modifiers)
            && event.keystroke.key.eq_ignore_ascii_case("v")
            && allow_text_editing
        {
            if let Some(clipboard_text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                // Filter out newlines for single-line input
                let filtered_text: String = clipboard_text
                    .chars()
                    .filter(|c| *c != '\n' && *c != '\r')
                    .collect();
                if !filtered_text.is_empty()
                    && let Some(input) = self.active_input.as_mut()
                {
                    input.state.replace_text_in_range(None, &filtered_text);
                    cx.notify();
                }
            }
            return;
        }

        match event.keystroke.key.as_str() {
            "enter" => {
                if let Some(field) = active_field
                    && field == EditableField::Theme
                {
                    self.commit_active_input(cx);
                    return;
                }
                if let Some(field) = active_field
                    && self.commit_dropdown_selection(field, &active_input_query, cx)
                {
                    return;
                }
                self.commit_active_input(cx);
            }
            "escape" => self.cancel_active_input(cx),
            "tab" => {
                if let Some(field) = active_field
                    && self.commit_dropdown_selection(field, &active_input_query, cx)
                {
                }
            }
            "backspace" => {
                if allow_text_editing && let Some(input) = self.active_input.as_mut() {
                    input.state.delete_backward();
                }
                cx.notify();
            }
            "delete" => {
                if allow_text_editing && let Some(input) = self.active_input.as_mut() {
                    input.state.delete_forward();
                }
                cx.notify();
            }
            "left" => {
                if allow_text_editing && let Some(input) = self.active_input.as_mut() {
                    input.state.move_left();
                }
                cx.notify();
            }
            "right" => {
                if allow_text_editing && let Some(input) = self.active_input.as_mut() {
                    input.state.move_right();
                }
                cx.notify();
            }
            "home" => {
                if allow_text_editing && let Some(input) = self.active_input.as_mut() {
                    input.state.move_to_start();
                }
                cx.notify();
            }
            "end" => {
                if allow_text_editing && let Some(input) = self.active_input.as_mut() {
                    input.state.move_to_end();
                }
                cx.notify();
            }
            _ => {}
        }
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.key_input_mode() == KeyInputMode::CaptureAction {
            self.handle_keybind_capture(event, cx);
            return;
        }

        if Self::is_plain_escape(event)
            && self.active_input.is_none()
            && self.plugin_setting_input.is_none()
            && self.ssh_input.is_none()
        {
            window.remove_window();
            return;
        }

        if self.handle_global_shortcuts(event, window, cx)
            || self.handle_section_cycle_shortcut(event, window, cx)
        {
            return;
        }

        match self.key_input_mode() {
            KeyInputMode::CaptureAction => self.handle_keybind_capture(event, cx),
            KeyInputMode::SidebarSearch => {
                let _ = self.handle_sidebar_search_key_down(event, window, cx);
            }
            KeyInputMode::ThemeStoreSearch => {
                let _ = self.handle_theme_store_search_key_down(event, cx);
            }
            KeyInputMode::ActiveInput => self.handle_active_input_key_down(event, cx),
            KeyInputMode::Idle => {}
        }
    }
}

impl TextInputProvider for SettingsWindow {
    fn text_input_state(&self) -> Option<&TextInputState> {
        if let Some(input) = self.ssh_input.as_ref() {
            return Some(&input.state);
        }
        if let Some(input) = self.plugin_setting_input.as_ref() {
            return Some(&input.state);
        }
        let settings_input = self
            .active_input
            .as_ref()
            .and_then(|input| Self::uses_text_input_for_field(input.field).then_some(&input.state));

        settings_input.or({
            if self.sidebar_search_active {
                Some(&self.sidebar_search_state)
            } else if self.theme_store_search_active {
                Some(&self.theme_store_search_state)
            } else {
                None
            }
        })
    }

    fn text_input_state_mut(&mut self) -> Option<&mut TextInputState> {
        if let Some(input) = self.ssh_input.as_mut() {
            return Some(&mut input.state);
        }
        if let Some(input) = self.plugin_setting_input.as_mut() {
            return Some(&mut input.state);
        }
        let settings_input = self.active_input.as_mut().and_then(|input| {
            Self::uses_text_input_for_field(input.field).then_some(&mut input.state)
        });

        if settings_input.is_some() {
            settings_input
        } else if self.sidebar_search_active {
            Some(&mut self.sidebar_search_state)
        } else if self.theme_store_search_active {
            Some(&mut self.theme_store_search_state)
        } else {
            None
        }
    }
}

impl gpui::EntityInputHandler for SettingsWindow {
    fn text_for_range(
        &mut self,
        range: std::ops::Range<usize>,
        adjusted_range: &mut Option<std::ops::Range<usize>>,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> Option<String> {
        let state = TextInputProvider::text_input_state(self)?;
        Some(state.text_for_range(range, adjusted_range))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> Option<gpui::UTF16Selection> {
        let state = TextInputProvider::text_input_state(self)?;
        Some(state.selected_text_range())
    }

    fn marked_text_range(
        &self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        let state = TextInputProvider::text_input_state(self)?;
        state.marked_text_range_utf16()
    }

    fn unmark_text(&mut self, _window: &mut gpui::Window, _cx: &mut gpui::Context<Self>) {
        if let Some(state) = TextInputProvider::text_input_state_mut(self) {
            state.unmark_text();
        }
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<std::ops::Range<usize>>,
        text: &str,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let mut changed = false;
        if let Some(state) = TextInputProvider::text_input_state_mut(self) {
            state.replace_text_in_range(range, text);
            changed = true;
        }

        if changed {
            self.sync_active_ssh_input();
            self.refresh_search_navigation(window, cx);
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<std::ops::Range<usize>>,
        new_text: &str,
        new_selected_range: Option<std::ops::Range<usize>>,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let mut changed = false;
        if let Some(state) = TextInputProvider::text_input_state_mut(self) {
            state.replace_and_mark_text_in_range(range, new_text, new_selected_range);
            changed = true;
        }

        if changed {
            self.sync_active_ssh_input();
            self.refresh_search_navigation(window, cx);
        }
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: std::ops::Range<usize>,
        element_bounds: gpui::Bounds<gpui::Pixels>,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> Option<gpui::Bounds<gpui::Pixels>> {
        let state = TextInputProvider::text_input_state(self)?;
        Some(state.bounds_for_range(range_utf16, element_bounds))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<gpui::Pixels>,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> Option<usize> {
        let state = TextInputProvider::text_input_state(self)?;
        Some(state.character_index_for_point(point))
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_window_background_appearance(window);
        // Components from `termy_ui` read their colors from a global, so refresh
        // it before building this frame's element tree.
        self.sync_ui_tokens(cx);
        let bg = self.bg_primary();
        let settings_scrollbar_metrics = self.settings_scrollbar_metrics(window);
        let scrollbar_thumb_active = self.scrollbar_drag_state.is_some();
        let settings_scrollbar_lane = {
            let bounds_entity = cx.entity();
            div()
                .id("settings-scrollbar-lane")
                .flex_none()
                .w(px(SETTINGS_SCROLLBAR_WIDTH + 4.0))
                .min_w(px(SETTINGS_SCROLLBAR_WIDTH + 4.0))
                .max_w(px(SETTINGS_SCROLLBAR_WIDTH + 4.0))
                .h_full()
                .pl(px(2.0))
                .pr(px(2.0))
                .cursor_pointer()
                .child(
                    canvas(
                        move |bounds, _, cx| {
                            bounds_entity.update(cx, |view, _| {
                                view.scrollbar_lane_bounds = Some(bounds);
                            });
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full(),
                )
                .when_some(settings_scrollbar_metrics, |s, metrics| {
                    s.child(ui_scrollbar::render_vertical(
                        "settings-content-scrollbar",
                        metrics,
                        self.settings_scrollbar_style(),
                        scrollbar_thumb_active,
                        &[],
                        None,
                        0.0,
                    ))
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|view, event: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        let y: f32 = event.position.y.into();
                        view.handle_scrollbar_mouse_down(y, window, cx);
                    }),
                )
        };
        div()
            .id("settings-root")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::handle_key_down))
            .on_any_mouse_down(cx.listener(|view, _event: &MouseDownEvent, _window, cx| {
                if view.active_input.is_some()
                    || view.ssh_input.is_some()
                    || view.sidebar_search_active
                    || view.theme_store_search_active
                    || view.capturing_action.is_some()
                {
                    view.active_input = None;
                    view.ssh_input = None;
                    view.capturing_action = None;
                    view.blur_sidebar_search();
                    view.theme_store_search_active = false;
                    view.theme_store_search_selecting = false;
                    cx.notify();
                }
            }))
            .when(self.background_opacity_drag_state.is_some(), |s| {
                s.on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, _window, cx| {
                    if !event.dragging() {
                        return;
                    }
                    let x: f32 = event.position.x.into();
                    view.update_background_opacity_drag(x, Self::background_opacity_slider_width());
                    cx.notify();
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|view, _event: &MouseUpEvent, _window, cx| {
                        match view.finish_background_opacity_drag() {
                            Ok(true) => crate::ui::toast::success("Saved"),
                            Ok(false) => {}
                            Err(error) => crate::ui::toast::error(error),
                        }
                        cx.notify();
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|view, _event: &MouseUpEvent, _window, cx| {
                        match view.finish_background_opacity_drag() {
                            Ok(true) => crate::ui::toast::success("Saved"),
                            Ok(false) => {}
                            Err(error) => crate::ui::toast::error(error),
                        }
                        cx.notify();
                    }),
                )
            })
            .when(self.scrollbar_drag_state.is_some(), |s| {
                s.on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, window, cx| {
                    if !event.dragging() {
                        return;
                    }
                    let y: f32 = event.position.y.into();
                    view.handle_scrollbar_drag(y, window, cx);
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|view, _event: &MouseUpEvent, _window, cx| {
                        if view.finish_scrollbar_drag() {
                            cx.notify();
                        }
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|view, _event: &MouseUpEvent, _window, cx| {
                        if view.finish_scrollbar_drag() {
                            cx.notify();
                        }
                    }),
                )
            })
            .flex()
            .size_full()
            .bg(bg)
            .font_family(self.config.ui_font_family.clone())
            .child(self.render_sidebar(cx))
            .child(
                // Keep the shared content pane shrink-safe so wide rows cannot
                // push controls or the scrollbar lane off-canvas.
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .h_full()
                    .flex()
                    .items_start()
                    .child(
                        div()
                            .id("settings-content-scroll")
                            .flex_1()
                            .min_w(px(0.0))
                            .h_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.content_scroll_handle)
                            .overflow_x_hidden()
                            .px(px(CONTENT_GUTTER_X))
                            .py(px(CONTENT_GUTTER_Y))
                            .child(
                                div().w_full().flex().justify_center().child(
                                    div()
                                        .w_full()
                                        .max_w(px(SETTINGS_CONTENT_MAX_WIDTH))
                                        .child(self.render_content(cx)),
                                ),
                            ),
                    )
                    .child(settings_scrollbar_lane),
            )
    }
}

impl Drop for SettingsWindow {
    fn drop(&mut self) {
        if config::synced_background_opacity_preview(
            self.config.background_opacity,
            self.preview_background_opacity,
        )
        .is_some_and(|preview| preview.owner_id == self.background_opacity_preview_owner_id)
        {
            config::publish_background_opacity_preview(None);
        }
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
