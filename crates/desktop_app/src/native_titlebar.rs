//! Keeps the window manager's titlebar in step with the Termy theme.
//!
//! Linux windows use server-side decorations, so the window manager draws the
//! titlebar in the desktop theme. On X11 (including XWayland) we hint the
//! theme colors through window properties:
//! - `_GTK_THEME_VARIANT` asks GNOME/Mutter and GTK-aware window managers
//!   for the dark or light titlebar variant.
//! - `_KDE_NET_WM_COLOR_SCHEME` points KWin at a generated color scheme with
//!   the exact theme colors.
//!
//! Native Wayland exposes neither hint through GPUI.
//!
//! On Windows DWM draws the window frame (and the caption of windows that keep
//! the OS titlebar). It follows the system light/dark setting by default; we
//! switch it to match the Termy theme instead and paint the caption in the
//! theme background. The window title also tracks the active tab there, so
//! the taskbar and Alt+Tab show something useful.
//!
//! macOS draws its own titlebar, so there this is a no-op.

use crate::colors::TerminalColors;
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
use gpui_kit::Window;

#[cfg(target_os = "linux")]
pub(crate) use linux::NativeTitlebarTheme;
#[cfg(target_os = "windows")]
pub(crate) use windows_frame::NativeTitlebarTheme;

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub(crate) struct NativeTitlebarTheme;

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
impl NativeTitlebarTheme {
    pub(crate) fn for_window(_window: &Window) -> Self {
        Self
    }

    pub(crate) fn sync(&mut self, _colors: &TerminalColors) {}
}

#[cfg(any(target_os = "linux", target_os = "windows", test))]
/// `r,g,b` bytes of an opaque titlebar color.
type Rgb8 = [u8; 3];

#[cfg(any(target_os = "linux", target_os = "windows", test))]
fn rgb8(color: gpui_kit::Rgba) -> Rgb8 {
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    [channel(color.r), channel(color.g), channel(color.b)]
}

#[cfg(any(target_os = "linux", target_os = "windows", test))]
fn is_dark([r, g, b]: Rgb8) -> bool {
    // Rec. 709 luma is enough to choose between the two variants.
    let luma = 0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b);
    luma < 128.0
}

#[cfg(any(target_os = "linux", test))]
/// A KDE color scheme whose titlebar (`Header` set, and the legacy `WM`
/// group for older KWin) uses `background`/`foreground` in both the active and
/// inactive state.
fn kde_color_scheme(background: Rgb8, foreground: Rgb8) -> String {
    let format = |[r, g, b]: Rgb8| format!("{r},{g},{b}");
    let background = format(background);
    let foreground = format(foreground);
    format!(
        "[General]\n\
         ColorScheme=Termy\n\
         Name=Termy\n\
         \n\
         [Colors:Header]\n\
         BackgroundNormal={background}\n\
         ForegroundNormal={foreground}\n\
         \n\
         [Colors:Header][Inactive]\n\
         BackgroundNormal={background}\n\
         ForegroundNormal={foreground}\n\
         \n\
         [WM]\n\
         activeBackground={background}\n\
         activeBlend={background}\n\
         activeForeground={foreground}\n\
         inactiveBackground={background}\n\
         inactiveBlend={background}\n\
         inactiveForeground={foreground}\n"
    )
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{Rgb8, TerminalColors, is_dark, kde_color_scheme, rgb8};
    use gpui_kit::Window;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::io::Write as _;
    use std::path::PathBuf;
    use std::sync::OnceLock;
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{Atom, AtomEnum, ConnectionExt as _, PropMode};
    use x11rb::rust_connection::RustConnection;
    use x11rb::wrapper::ConnectionExt as _;

    pub(crate) struct NativeTitlebarTheme {
        /// `None` on native Wayland, where no hint is available.
        x11_window: Option<u32>,
        applied: Option<(Rgb8, Rgb8)>,
    }

    impl NativeTitlebarTheme {
        pub(crate) fn for_window(window: &Window) -> Self {
            let x11_window =
                match HasWindowHandle::window_handle(window).map(|handle| handle.as_raw()) {
                    Ok(RawWindowHandle::Xcb(handle)) => Some(handle.window.get()),
                    Ok(RawWindowHandle::Xlib(handle)) => u32::try_from(handle.window).ok(),
                    _ => None,
                };
            Self {
                x11_window,
                applied: None,
            }
        }

        /// Applies `colors` to the titlebar unless they are already applied.
        /// Cheap enough to call on every frame.
        pub(crate) fn sync(&mut self, colors: &TerminalColors) {
            let Some(x11_window) = self.x11_window else {
                return;
            };
            let next = (rgb8(colors.background), rgb8(colors.foreground));
            if self.applied == Some(next) {
                return;
            }
            self.applied = Some(next);
            if let Err(error) = apply(x11_window, next.0, next.1) {
                log::warn!("Failed to apply theme colors to the window titlebar: {error}");
            }
        }
    }

    struct X11 {
        connection: RustConnection,
        utf8_string: Atom,
        gtk_theme_variant: Atom,
        kde_color_scheme: Atom,
    }

    /// A separate client connection: properties set on GPUI's window from
    /// here reach the window manager the same way.
    fn x11() -> Option<&'static X11> {
        static X11: OnceLock<Option<X11>> = OnceLock::new();
        X11.get_or_init(|| match connect() {
            Ok(x11) => Some(x11),
            Err(error) => {
                log::warn!("Titlebar theme hints are unavailable: {error}");
                None
            }
        })
        .as_ref()
    }

    fn connect() -> Result<X11, Box<dyn std::error::Error>> {
        let (connection, _) = x11rb::connect(None)?;
        let utf8_string = connection.intern_atom(false, b"UTF8_STRING")?;
        let gtk_theme_variant = connection.intern_atom(false, b"_GTK_THEME_VARIANT")?;
        let kde_color_scheme = connection.intern_atom(false, b"_KDE_NET_WM_COLOR_SCHEME")?;
        Ok(X11 {
            utf8_string: utf8_string.reply()?.atom,
            gtk_theme_variant: gtk_theme_variant.reply()?.atom,
            kde_color_scheme: kde_color_scheme.reply()?.atom,
            connection,
        })
    }

    fn apply(
        window: u32,
        background: Rgb8,
        foreground: Rgb8,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let Some(x11) = x11() else {
            return Ok(());
        };
        let variant: &[u8] = if is_dark(background) {
            b"dark"
        } else {
            b"light"
        };
        x11.connection.change_property8(
            PropMode::REPLACE,
            window,
            x11.gtk_theme_variant,
            x11.utf8_string,
            variant,
        )?;
        match write_kde_color_scheme(background, foreground) {
            Ok(path) => {
                x11.connection.change_property8(
                    PropMode::REPLACE,
                    window,
                    x11.kde_color_scheme,
                    AtomEnum::STRING,
                    path.as_os_str().as_encoded_bytes(),
                )?;
            }
            Err(error) => log::warn!("Failed to write the KDE titlebar color scheme: {error}"),
        }
        x11.connection.flush()?;
        Ok(())
    }

    /// One file per color pair, so KWin never keeps a stale cached scheme
    /// for a path whose contents changed.
    fn write_kde_color_scheme(background: Rgb8, foreground: Rgb8) -> std::io::Result<PathBuf> {
        let directory = dirs::cache_dir()
            .ok_or_else(|| std::io::Error::other("no cache directory"))?
            .join("termy")
            .join("titlebar-color-schemes");
        let hex = |[r, g, b]: Rgb8| format!("{r:02x}{g:02x}{b:02x}");
        let path = directory.join(format!("{}-{}.colors", hex(background), hex(foreground)));
        if !path.exists() {
            std::fs::create_dir_all(&directory)?;
            let contents = kde_color_scheme(background, foreground);
            let mut temp = tempfile::NamedTempFile::new_in(&directory)?;
            temp.write_all(contents.as_bytes())?;
            temp.persist(&path).map_err(|error| error.error)?;
        }
        Ok(path)
    }
}

#[cfg(target_os = "windows")]
mod windows_frame {
    use super::{Rgb8, TerminalColors, is_dark, rgb8};
    use crate::native_sdk::win32_window;
    use gpui_kit::Window;
    use std::time::{Duration, Instant};

    /// GPUI resets the DWM frame to the system theme whenever the system
    /// setting changes, so that setting is polled (at most this often) to
    /// notice and re-apply the Termy theme.
    const SYSTEM_THEME_POLL_INTERVAL: Duration = Duration::from_secs(1);

    pub(crate) struct NativeTitlebarTheme {
        window_id: u64,
        applied: Option<Rgb8>,
        system_dark: Option<bool>,
        system_checked_at: Option<Instant>,
        window_title: Option<String>,
    }

    impl NativeTitlebarTheme {
        pub(crate) fn for_window(window: &Window) -> Self {
            Self {
                window_id: window.window_handle().window_id().as_u64(),
                applied: None,
                system_dark: None,
                system_checked_at: None,
                window_title: None,
            }
        }

        /// Applies the theme background to the DWM frame unless it is already
        /// applied. Cheap enough to call on every frame.
        pub(crate) fn sync(&mut self, colors: &TerminalColors) {
            let now = Instant::now();
            if self.system_checked_at.is_none_or(|checked_at| {
                now.duration_since(checked_at) >= SYSTEM_THEME_POLL_INTERVAL
            }) {
                self.system_checked_at = Some(now);
                let system_dark = win32_window::system_apps_use_dark_theme();
                if system_dark != self.system_dark {
                    self.system_dark = system_dark;
                    self.applied = None;
                }
            }

            let background = rgb8(colors.background);
            if self.applied == Some(background) {
                return;
            }
            // The handle is registered while the window opens; until it is
            // known, keep retrying on later frames.
            let Some(hwnd) = win32_window::hwnd_for_window_id(self.window_id) else {
                return;
            };
            self.applied = Some(background);
            win32_window::set_immersive_dark_mode(hwnd, is_dark(background));
            win32_window::set_caption_color(hwnd, background);
        }

        /// Sets the OS window title (taskbar, Alt+Tab) unless it already is
        /// `title`.
        pub(crate) fn sync_window_title(&mut self, window: &mut Window, title: &str) {
            if self.window_title.as_deref() == Some(title) {
                return;
            }
            window.set_window_title(title);
            self.window_title = Some(title.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb8_rounds_and_clamps_channels() {
        let color = gpui_kit::Rgba {
            r: 1.2,
            g: 0.5,
            b: -0.1,
            a: 1.0,
        };
        assert_eq!(rgb8(color), [255, 128, 0]);
    }

    #[test]
    fn dark_variant_follows_background_luma() {
        assert!(is_dark([0x1e, 0x1e, 0x2e]));
        assert!(is_dark([0x00, 0x00, 0xff]));
        assert!(!is_dark([0xff, 0xff, 0xff]));
        assert!(!is_dark([0xee, 0xe8, 0xd5]));
    }

    #[test]
    fn kde_color_scheme_sets_header_and_wm_colors() {
        let scheme = kde_color_scheme([30, 30, 46], [205, 214, 244]);
        assert!(scheme.contains("[Colors:Header]\nBackgroundNormal=30,30,46\n"));
        assert!(scheme.contains("[Colors:Header][Inactive]\nBackgroundNormal=30,30,46\n"));
        assert!(scheme.contains("activeBackground=30,30,46\n"));
        assert!(scheme.contains("inactiveForeground=205,214,244\n"));
    }
}
