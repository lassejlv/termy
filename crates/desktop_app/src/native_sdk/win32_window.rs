//! Win32 glue for Termy's GPUI windows.
//!
//! GPUI does not hand out a window's HWND (and `raw-window-handle` is not a
//! Windows dependency here), so every window registers its handle once while
//! it opens. Everything else looks the handle up by GPUI window id: DWM frame
//! theming, native moves started from the client-side titlebar, and owners for
//! modal dialogs.

use gpui_kit::Window;
use std::sync::{Mutex, MutexGuard, OnceLock};
use windows::Win32::Foundation::{COLORREF, ERROR_SUCCESS, HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetForegroundWindow, GetWindowThreadProcessId, HTCAPTION, IsWindow,
    IsWindowVisible, PostMessageW, SC_MAXIMIZE, SC_MOVE, SC_RESTORE, WM_SYSCOMMAND,
};
use windows::core::{BOOL, PCWSTR, w};

/// First Windows 11 build. Segoe Fluent Icons ships from here on.
pub const WINDOWS_11_FIRST_BUILD: u32 = 22000;
/// Windows 11 22H2, the first build with `DWMWA_SYSTEMBACKDROP_TYPE`.
pub const WINDOWS_11_SYSTEM_BACKDROP_BUILD: u32 = 22621;

#[derive(Clone, Copy)]
struct RegisteredWindow {
    window_id: u64,
    /// `HWND` is not `Send`, so the registry keeps the raw value.
    hwnd: isize,
}

static WINDOWS: Mutex<Vec<RegisteredWindow>> = Mutex::new(Vec::new());

fn registered_windows() -> MutexGuard<'static, Vec<RegisteredWindow>> {
    WINDOWS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn hwnd_from_raw(raw: isize) -> HWND {
    HWND(raw as *mut core::ffi::c_void)
}

fn is_live_window(hwnd: HWND) -> bool {
    !hwnd.is_invalid() && unsafe { IsWindow(Some(hwnd)) }.as_bool()
}

fn belongs_to_current_process(hwnd: HWND) -> bool {
    if hwnd.is_invalid() {
        return false;
    }
    let mut process_id = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut process_id)) };
    process_id == std::process::id()
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Records the HWND behind `window` and applies `title`.
///
/// The window is briefly retitled with a marker unique to this process and
/// window, which `FindWindowW` then locates. `SetWindowTextW` is synchronous
/// and nothing repaints in between, so the marker is never visible.
pub fn register_window(window: &mut Window, title: &str) {
    let window_id = window.window_handle().window_id().as_u64();
    let marker = format!("termy-window-probe-{}-{window_id}", std::process::id());
    window.set_window_title(&marker);
    let marker = wide(&marker);
    let found = unsafe { FindWindowW(PCWSTR::null(), PCWSTR(marker.as_ptr())) };
    window.set_window_title(title);

    let hwnd = match found {
        Ok(hwnd) if belongs_to_current_process(hwnd) => hwnd,
        _ => {
            log::warn!("Could not resolve the native handle of window {window_id}");
            return;
        }
    };

    let mut windows = registered_windows();
    windows
        .retain(|entry| entry.window_id != window_id && is_live_window(hwnd_from_raw(entry.hwnd)));
    windows.push(RegisteredWindow {
        window_id,
        hwnd: hwnd.0 as isize,
    });
}

/// The HWND registered for the GPUI window with `window_id`.
pub fn hwnd_for_window_id(window_id: u64) -> Option<HWND> {
    registered_windows()
        .iter()
        .find(|entry| entry.window_id == window_id)
        .map(|entry| hwnd_from_raw(entry.hwnd))
        .filter(|hwnd| is_live_window(*hwnd))
}

/// The HWND behind `window`, falling back to the foreground window when it is
/// one of ours (true for any window that just received a click).
pub fn hwnd_for_window(window: &Window) -> Option<HWND> {
    hwnd_for_window_id(window.window_handle().window_id().as_u64())
        .or_else(foreground_window_of_current_process)
}

fn foreground_window_of_current_process() -> Option<HWND> {
    let hwnd = unsafe { GetForegroundWindow() };
    belongs_to_current_process(hwnd).then_some(hwnd)
}

/// Owner for modal dialogs: the foreground Termy window, else the most
/// recently opened visible one. `None` before any window exists.
pub fn dialog_owner() -> Option<HWND> {
    foreground_window_of_current_process().or_else(|| {
        registered_windows()
            .iter()
            .rev()
            .map(|entry| hwnd_from_raw(entry.hwnd))
            .find(|hwnd| is_live_window(*hwnd) && unsafe { IsWindowVisible(*hwnd) }.as_bool())
    })
}

/// Starts an OS window move as if the caption had been pressed. Snap and
/// Aero Shake work as usual. Only meaningful while the left button is held.
pub fn start_window_drag(window: &Window) -> bool {
    let Some(hwnd) = hwnd_for_window(window) else {
        return false;
    };
    // SC_MOVE | HTCAPTION is the mouse-driven variant of SC_MOVE. Posting it
    // lets the current GPUI event finish before DefWindowProc enters the
    // modal move loop.
    unsafe {
        PostMessageW(
            Some(hwnd),
            WM_SYSCOMMAND,
            WPARAM((SC_MOVE | HTCAPTION) as usize),
            LPARAM(0),
        )
    }
    .is_ok()
}

/// Maximizes `window`, or restores it when it is already maximized. GPUI's
/// `zoom_window` only ever maximizes on Windows.
pub fn toggle_maximize(window: &Window) {
    let command = if window.is_maximized() {
        SC_RESTORE
    } else {
        SC_MAXIMIZE
    };
    let posted = hwnd_for_window(window).is_some_and(|hwnd| {
        unsafe {
            PostMessageW(
                Some(hwnd),
                WM_SYSCOMMAND,
                WPARAM(command as usize),
                LPARAM(0),
            )
        }
        .is_ok()
    });
    if !posted && command == SC_MAXIMIZE {
        window.zoom_window();
    }
}

/// Asks DWM for the dark or light frame (border, system menu and, when the OS
/// titlebar is shown, the caption itself).
pub fn set_immersive_dark_mode(hwnd: HWND, dark: bool) {
    let value = BOOL::from(dark);
    let result = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &value as *const BOOL as *const core::ffi::c_void,
            std::mem::size_of::<BOOL>() as u32,
        )
    };
    if let Err(error) = result {
        log::debug!("DWMWA_USE_IMMERSIVE_DARK_MODE failed: {error}");
    }
}

/// Paints the OS caption in `rgb`. Windows 10 rejects the attribute, which
/// leaves the default caption.
pub fn set_caption_color(hwnd: HWND, [r, g, b]: [u8; 3]) {
    let color = COLORREF(u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16));
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR,
            &color as *const COLORREF as *const core::ffi::c_void,
            std::mem::size_of::<COLORREF>() as u32,
        )
    };
}

fn read_registry_string(root: HKEY, subkey: PCWSTR, value: PCWSTR) -> Option<String> {
    let mut buffer = [0u16; 64];
    let mut size = std::mem::size_of_val(&buffer) as u32;
    let status = unsafe {
        RegGetValueW(
            root,
            subkey,
            value,
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast::<core::ffi::c_void>()),
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let len = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..len]))
}

fn read_registry_dword(root: HKEY, subkey: PCWSTR, value: PCWSTR) -> Option<u32> {
    let mut data = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            root,
            subkey,
            value,
            RRF_RT_REG_DWORD,
            None,
            Some((&mut data as *mut u32).cast::<core::ffi::c_void>()),
            Some(&mut size),
        )
    };
    (status == ERROR_SUCCESS).then_some(data)
}

/// The Windows build number, e.g. 22631. `None` when it cannot be read.
pub fn windows_build_number() -> Option<u32> {
    static BUILD: OnceLock<Option<u32>> = OnceLock::new();
    *BUILD.get_or_init(|| {
        read_registry_string(
            HKEY_LOCAL_MACHINE,
            w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"),
            w!("CurrentBuildNumber"),
        )
        .and_then(|build| build.trim().parse().ok())
    })
}

pub fn is_windows_11() -> bool {
    windows_build_number().is_some_and(|build| build >= WINDOWS_11_FIRST_BUILD)
}

/// Whether apps use the dark theme system-wide. GPUI re-applies this to the
/// DWM frame whenever it changes, so callers watch it to re-apply their own.
pub fn system_apps_use_dark_theme() -> Option<bool> {
    read_registry_dword(
        HKEY_CURRENT_USER,
        w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
        w!("AppsUseLightTheme"),
    )
    .map(|light| light == 0)
}
