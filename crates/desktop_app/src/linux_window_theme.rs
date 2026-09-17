//! Keep system-managed X11/XWayland titlebars in the desktop's theme variant.

use gpui::WindowAppearance;
use raw_window_handle::RawWindowHandle;
use x11rb::{
    connection::Connection,
    protocol::xproto::{Atom, ConnectionExt, PropMode},
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

// Also type-check the native entry point on test hosts. GPUI test windows
// have no raw handles, so only production windows call it.
#[cfg_attr(test, allow(dead_code))]
pub(crate) fn install(window: &mut gpui::Window) {
    use raw_window_handle::HasWindowHandle;

    let result = HasWindowHandle::window_handle(window)
        .map(|handle| handle.as_raw())
        .map_err(|error| anyhow::anyhow!("Could not access the native window handle: {error:?}"))
        .and_then(|handle| install_for_handle(window, handle));
    if let Err(error) = result {
        log::warn!("Failed to initialize Linux titlebar theme: {error}");
    }
}

fn install_for_handle(window: &mut gpui::Window, handle: RawWindowHandle) -> anyhow::Result<()> {
    let Some(window_id) = x11_window_id(handle) else {
        // Wayland server-side decorations are themed by the compositor.
        return Ok(());
    };
    // GPUI's X11 backend also connects to DISPLAY. Keep this connection for
    // the window's lifetime, rather than reconnecting on appearance changes.
    let (connection, _) = RustConnection::connect(None)?;
    let hint = X11ThemeHint::new(connection, window_id)?;
    hint.apply(window.appearance())?;
    window
        .observe_window_appearance(move |window, _| {
            if let Err(error) = hint.apply(window.appearance()) {
                log::warn!("Failed to update Linux titlebar theme: {error}");
            }
        })
        .detach();
    Ok(())
}

fn x11_window_id(handle: RawWindowHandle) -> Option<u32> {
    match handle {
        RawWindowHandle::Xcb(handle) => Some(handle.window.get()),
        RawWindowHandle::Xlib(handle) => u32::try_from(handle.window).ok(),
        _ => None,
    }
}

struct X11ThemeHint {
    connection: RustConnection,
    window: u32,
    theme_variant: Atom,
    utf8_string: Atom,
}

impl X11ThemeHint {
    fn new(connection: RustConnection, window: u32) -> anyhow::Result<Self> {
        let theme_variant = connection
            .intern_atom(false, b"_GTK_THEME_VARIANT")?
            .reply()?
            .atom;
        let utf8_string = connection.intern_atom(false, b"UTF8_STRING")?.reply()?.atom;
        Ok(Self {
            connection,
            window,
            theme_variant,
            utf8_string,
        })
    }

    fn apply(&self, appearance: WindowAppearance) -> anyhow::Result<()> {
        // GTK exports this hint for non-client-decorated windows. GPUI 0.2.2
        // observes the desktop appearance but does not publish the hint itself.
        // https://docs.gtk.org/gdk3-x11/method.X11Window.set_theme_variant.html
        if matches!(
            appearance,
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        ) {
            self.connection
                .change_property8(
                    PropMode::REPLACE,
                    self.window,
                    self.theme_variant,
                    self.utf8_string,
                    b"dark",
                )?
                .check()?;
        } else {
            // Remove our override so switching back to light restores the
            // window manager's default variant of the user's selected theme.
            self.connection
                .delete_property(self.window, self.theme_variant)?
                .check()?;
        }
        self.connection.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use raw_window_handle::{WaylandWindowHandle, XcbWindowHandle, XlibWindowHandle};
    use std::{num::NonZeroU32, ptr::NonNull};

    #[test]
    fn theme_hint_targets_x11_windows_only() {
        assert_eq!(
            x11_window_id(RawWindowHandle::Xcb(XcbWindowHandle::new(
                NonZeroU32::new(42).unwrap()
            ))),
            Some(42)
        );
        assert_eq!(
            x11_window_id(RawWindowHandle::Xlib(XlibWindowHandle::new(43))),
            Some(43)
        );
        assert_eq!(
            x11_window_id(RawWindowHandle::Wayland(WaylandWindowHandle::new(
                NonNull::dangling()
            ))),
            None
        );
    }

    #[gpui::test]
    fn wayland_does_not_connect_to_x11(cx: &mut gpui::TestAppContext) {
        struct TestView;
        impl gpui::Render for TestView {
            fn render(
                &mut self,
                _: &mut gpui::Window,
                _: &mut gpui::Context<Self>,
            ) -> impl gpui::IntoElement {
                gpui::div()
            }
        }
        cx.add_window_view(|window, _| {
            let handle = RawWindowHandle::Wayland(WaylandWindowHandle::new(NonNull::dangling()));
            install_for_handle(window, handle).unwrap();
            TestView
        });
    }

    #[cfg(unix)]
    #[test]
    fn x11_property_round_trip_tracks_dark_light_and_window_id() {
        use std::{
            io::{Read, Write},
            os::unix::net::UnixStream,
            thread,
            time::Duration,
        };
        use x11rb::{
            protocol::xproto::{Screen, Setup},
            rust_connection::DefaultStream,
            x11_utils::Serialize,
        };

        // Exercise the real x11rb connection against a bounded protocol peer,
        // so this regression also runs on macOS without an X server installed.
        let (client, mut server) = UnixStream::pair().unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let peer = thread::spawn(move || {
            let mut handshake = [0u8; 12];
            server.read_exact(&mut handshake).unwrap();
            let mut setup = Setup {
                status: 1,
                protocol_major_version: 11,
                resource_id_base: 0x200000,
                resource_id_mask: 0x1fffff,
                maximum_request_length: u16::MAX,
                roots: vec![Screen::default()],
                ..Default::default()
            }
            .serialize();
            let length = ((setup.len() - 8) / 4) as u16;
            setup[6..8].copy_from_slice(&length.to_ne_bytes());
            server.write_all(&setup).unwrap();
            let mut sequence = 0u16;
            let mut updates = Vec::new();
            loop {
                let mut header = [0u8; 4];
                match server.read_exact(&mut header) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
                    Err(error) => panic!("X11 peer read failed: {error}"),
                }
                sequence += 1;
                let length = usize::from(u16::from_ne_bytes([header[2], header[3]])) * 4;
                let mut request = vec![0u8; length];
                request[..4].copy_from_slice(&header);
                server.read_exact(&mut request[4..]).unwrap();
                let word =
                    |offset| u32::from_ne_bytes(request[offset..offset + 4].try_into().unwrap());
                let mut reply = [0u8; 32];
                reply[0] = 1;
                reply[2..4].copy_from_slice(&sequence.to_ne_bytes());
                match header[0] {
                    16 => {
                        // InternAtom
                        let name_len = usize::from(u16::from_ne_bytes([request[4], request[5]]));
                        let atom: u32 = match &request[8..8 + name_len] {
                            b"_GTK_THEME_VARIANT" => 100,
                            b"UTF8_STRING" => 101,
                            name => panic!("unexpected atom: {name:?}"),
                        };
                        reply[8..12].copy_from_slice(&atom.to_ne_bytes());
                        server.write_all(&reply).unwrap();
                    }
                    18 => {
                        // ChangeProperty
                        assert_eq!(header[1], 0, "replace, do not append");
                        assert_eq!(word(8), 100);
                        assert_eq!(word(12), 101, "UTF8_STRING, not STRING");
                        assert_eq!(request[16], 8);
                        assert_eq!(word(20), 4);
                        updates.push((word(4), Some(request[24..28].to_vec())));
                    }
                    19 => {
                        // DeleteProperty
                        assert_eq!(word(8), 100);
                        updates.push((word(4), None));
                    }
                    43 => {
                        // GetInputFocus: x11rb's checked-request barrier
                        server.write_all(&reply).unwrap();
                    }
                    opcode => panic!("unexpected X11 request {opcode}"),
                }
            }
            updates
        });
        let (stream, _) = DefaultStream::from_unix_stream(client).unwrap();
        let connection = RustConnection::connect_to_stream(stream, 0).unwrap();
        let mut hint = X11ThemeHint::new(connection, 42).unwrap();
        hint.apply(WindowAppearance::Light).unwrap();
        hint.apply(WindowAppearance::Dark).unwrap();
        hint.apply(WindowAppearance::VibrantLight).unwrap();
        hint.window = 43;
        hint.apply(WindowAppearance::VibrantDark).unwrap();
        drop(hint);
        assert_eq!(
            peer.join().unwrap(),
            vec![
                (42, None),
                (42, Some(b"dark".to_vec())),
                (42, None),
                (43, Some(b"dark".to_vec()))
            ]
        );
    }
}
