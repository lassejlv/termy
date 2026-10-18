//! Native macOS Settings window (SwiftUI, `native/macos/SettingsWindow.swift`).
//!
//! The SwiftUI side only draws: Rust builds the page model from the config
//! schema, sends it over as JSON, and applies the edits the window reports
//! back. The window draws General, Appearance, Terminal, Tabs and Colors;
//! its Keyboard Shortcuts, Themes, Plugins and SSH entries open those
//! sections in Termy's own Settings window.
//!
//! Like the native tabs, every AppKit call runs on the main queue after the
//! current GPUI update: showing a window moves key status, which makes AppKit
//! call back into GPUI right away.

mod model;

use crate::config;
use crate::settings_view::SettingsSection;
use gpui_kit::{App, AsyncApp, Global};
use model::{Event, Model, ModelSources, ThemeEntry};
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char};
use std::sync::OnceLock;

unsafe extern "C" {
    fn termy_native_settings_supported() -> bool;
    fn termy_native_settings_show(
        model: *const c_char,
        section: *const c_char,
        callback: extern "C" fn(*const c_char),
    );
    fn termy_native_settings_update(model: *const c_char);
    fn termy_native_settings_is_visible() -> bool;
    fn termy_native_settings_close();
}

/// Window events, from the AppKit callback to the GPUI task that applies them.
static EVENTS: OnceLock<flume::Sender<String>> = OnceLock::new();

/// What the window needs besides the config file.
struct NativeSettings {
    fonts: Vec<String>,
    /// The last failed edit per setting key, shown under that row.
    errors: HashMap<String, String>,
    /// Hash of the model last sent, so unchanged refreshes are skipped. One
    /// edit can bring two or three config change notifications.
    last_sent: Option<u64>,
}

impl Global for NativeSettings {}

/// Whether this macOS has the SwiftUI the window needs (macOS 13 and later).
pub(crate) fn supported() -> bool {
    // SAFETY: Plain query into the Swift library.
    unsafe { termy_native_settings_supported() }
}

/// Show the native window, on `section` when given. Returns false when the
/// window does not draw `section`, so the caller opens Termy's own window.
pub(crate) fn open(section: Option<SettingsSection>, cx: &mut App) -> bool {
    let page = match section {
        Some(section) => match model::page_for_section(section) {
            Some(page) => Some(page),
            None => return false,
        },
        None => None,
    };
    install(cx);
    // Errors belong to edits in the window's last session.
    cx.global_mut::<NativeSettings>().errors.clear();
    let Some(json) = model_json(cx) else {
        return false;
    };
    cx.global_mut::<NativeSettings>().last_sent = Some(hash(&json));
    let page = page.and_then(|page| CString::new(page).ok());
    on_main_queue(move || {
        let page = page.as_ref().map_or(std::ptr::null(), |page| page.as_ptr());
        // SAFETY: Both strings outlive the call, which copies them.
        unsafe { termy_native_settings_show(json.as_ptr(), page, on_event) }
    });
    true
}

pub(crate) fn close() {
    on_main_queue(|| {
        // SAFETY: Main-thread call into the Swift library.
        unsafe { termy_native_settings_close() }
    });
}

fn install(cx: &mut App) {
    if cx.has_global::<NativeSettings>() {
        return;
    }
    let fonts = crate::font_families::available_font_families(cx.text_system().all_font_names());
    cx.set_global(NativeSettings {
        fonts,
        errors: HashMap::new(),
        last_sent: None,
    });

    let (events_tx, events) = flume::unbounded();
    let _ = EVENTS.set(events_tx);
    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Ok(json) = events.recv_async().await {
            cx.update(|cx| handle_event(&json, cx));
        }
    })
    .detach();

    // Edits made elsewhere (the config file, the command palette, Termy's
    // own Settings window) show up in an open native window too.
    let changes = config::subscribe_config_changes();
    cx.spawn(async move |cx: &mut AsyncApp| {
        while changes.recv_async().await.is_ok() {
            cx.update(refresh);
        }
    })
    .detach();
}

extern "C" fn on_event(json: *const c_char) {
    if json.is_null() {
        return;
    }
    // SAFETY: The Swift side passes a NUL-terminated string that lives for
    // the duration of the call.
    let json = unsafe { CStr::from_ptr(json) }
        .to_string_lossy()
        .into_owned();
    if let Some(events) = EVENTS.get() {
        let _ = events.send(json);
    }
}

fn handle_event(json: &str, cx: &mut App) {
    let event = match serde_json::from_str::<Event>(json) {
        Ok(event) => event,
        Err(error) => {
            log::error!("Invalid native settings event {json}: {error}");
            return;
        }
    };
    let (key, result) = match event {
        Event::OpenSection { section } => {
            if let Some(section) = model::external_section(&section)
                && let Err(error) = crate::app_actions::open_settings_section(section, cx)
            {
                crate::ui::toast::error(error);
            }
            return;
        }
        Event::Set { key, value } => {
            let fonts = &cx.global::<NativeSettings>().fonts;
            let result = model::apply_set(&key, &value, fonts);
            if result.is_ok() && key == "simple_mode" && value.trim() == "true" {
                // Simple mode opens the config file instead of Settings.
                close();
            }
            (key, result)
        }
        Event::Reset { key } => {
            let result = model::apply_reset(&key);
            (key, result)
        }
        Event::Refresh => {
            refresh(cx);
            return;
        }
    };
    let errors = &mut cx.global_mut::<NativeSettings>().errors;
    let changed = match result {
        Ok(()) => errors.remove(&key).is_some(),
        Err(error) => {
            errors.insert(key, error);
            true
        }
    };
    // A successful write refreshes through the config change notification.
    // A failed one leaves the config untouched, and clearing an error changes
    // nothing on disk, so those refresh here.
    if changed {
        refresh(cx);
    }
}

/// Send the current model to the window, when it is open and the model
/// changed since the last one sent.
fn refresh(cx: &mut App) {
    // SAFETY: Read-only query into the Swift library on the main thread.
    if !unsafe { termy_native_settings_is_visible() } {
        return;
    }
    let Some(json) = model_json(cx) else {
        return;
    };
    let digest = hash(&json);
    let state = cx.global_mut::<NativeSettings>();
    if state.last_sent == Some(digest) {
        return;
    }
    state.last_sent = Some(digest);
    on_main_queue(move || {
        // SAFETY: The string outlives the call, which copies it.
        unsafe { termy_native_settings_update(json.as_ptr()) }
    });
}

fn model_json(cx: &App) -> Option<CString> {
    let mut config_error = None;
    let config = config::load_runtime_config(
        &mut config_error,
        "Failed to load config for the Settings window",
    )
    .config;
    let state = cx.global::<NativeSettings>();
    let themes = theme_entries();
    let model = Model::build(
        &config,
        &ModelSources {
            fonts: &state.fonts,
            themes: &themes,
            appearance: config::system_appearance_from_window(cx.window_appearance()),
            errors: &state.errors,
        },
    );
    let json = serde_json::to_string(&model)
        .map_err(|error| log::error!("Failed to encode the native settings model: {error}"))
        .ok()?;
    CString::new(json).ok()
}

/// Termy's themes and the shell-decided theme, the rest of the built-in
/// collection, then local themes. A local theme file replaces the colors of
/// a built-in theme with the same id.
fn theme_entries() -> Vec<ThemeEntry> {
    let local_ids = crate::theme_library::local_theme_ids();
    let local_colors = |id: &str| {
        local_ids
            .iter()
            .any(|local| local == id)
            .then(|| crate::theme_library::theme_colors(id))
            .flatten()
    };
    let mut entries: Vec<ThemeEntry> = termy_core::themes::builtin_themes()
        .iter()
        .map(|theme| ThemeEntry {
            id: theme.id.to_string(),
            name: theme.name.to_string(),
            colors: local_colors(theme.id).unwrap_or(theme.colors),
        })
        .collect();
    entries.insert(
        termy_core::themes::TERMY_THEME_IDS.len(),
        ThemeEntry {
            id: config::SHELL_DECIDE_THEME_ID.to_string(),
            name: "Shell Decide".to_string(),
            colors: termy_core::themes::termy(),
        },
    );
    for id in &local_ids {
        if termy_core::themes::builtin_theme(id).is_none()
            && let Some(colors) = local_colors(id)
        {
            entries.push(ThemeEntry {
                id: id.clone(),
                name: model::theme_label(id),
                colors,
            });
        }
    }
    entries
}

fn hash(json: &CString) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    json.hash(&mut hasher);
    hasher.finish()
}

fn on_main_queue(work: impl FnOnce() + Send + 'static) {
    dispatch2::DispatchQueue::main().exec_async(work);
}
