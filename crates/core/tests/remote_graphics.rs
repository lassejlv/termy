use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use termy_core::{
    Terminal, TerminalEvent, TerminalSize,
    remote::{
        self, RemoteCommand, RemoteHostReply, RemoteHostRequest, RemoteReply, RemoteState,
        RemoteTransport,
    },
};

struct Host(Mutex<Terminal>, AtomicUsize);

impl RemoteTransport for Host {
    fn state(&self) -> Arc<RemoteState> {
        Arc::new(RemoteState::capture(&self.0.lock().unwrap()))
    }

    fn request(&self, command: RemoteCommand) -> anyhow::Result<RemoteReply> {
        let reply = remote::execute(&mut self.0.lock().unwrap(), command);
        // Exercise the same image serialization boundary as the session host.
        let bytes = bincode::serialize(&reply)?;
        self.1.fetch_add(bytes.len(), Ordering::Relaxed);
        Ok(bincode::deserialize(&bytes)?)
    }

    fn send(&self, command: RemoteCommand) {
        self.request(command).unwrap();
    }
    fn take_events(&self) -> Vec<TerminalEvent> {
        Vec::new()
    }
    fn take_host_requests(&self) -> Vec<(u64, RemoteHostRequest)> {
        Vec::new()
    }
    fn reply_to_host(&self, _: u64, _: RemoteHostReply) {}
    fn has_pending_events(&self) -> bool {
        false
    }
    fn set_wakeup_enabled(&self, _: bool) {}
}

fn terminal() -> (Arc<Host>, Terminal) {
    let host = Arc::new(Host(
        Mutex::new(Terminal::new_display(
            TerminalSize {
                cols: 20,
                rows: 6,
                cell_width: 10.0,
                cell_height: 20.0,
            },
            None,
        )),
        AtomicUsize::new(0),
    ));
    let remote = Terminal::from_remote(host.clone());
    (host, remote)
}

#[track_caller]
fn assert_placements_match(host: &Host, remote: &Terminal) {
    let geometry = |terminal: &Terminal| {
        terminal
            .kitty_graphics_placements()
            .iter()
            .map(|p| {
                (
                    p.image_id,
                    p.viewport_row,
                    p.col,
                    p.virtual_cell,
                    p.clip_top_rows,
                    p.clip_bottom_rows,
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(geometry(remote), geometry(&host.0.lock().unwrap()));
}

#[test]
fn remote_images_follow_output_scrolling_and_scrollback() {
    let (host, remote) = terminal();
    remote.feed_output(b"\x1b[3;1H\x1b_Ga=T,i=1,f=32,s=1,v=1,c=2,r=2,C=1;AQID/w==\x1b\\");
    assert_placements_match(&host, &remote);
    remote.feed_output(b"\x1b[6;1H\r\n");
    assert_placements_match(&host, &remote);
    remote.feed_output(b"\r\n\r\n\r\n\r\n");
    assert_placements_match(&host, &remote);
    remote.scroll_display(4);
    assert_placements_match(&host, &remote);
    remote.scroll_to_bottom();
    assert_placements_match(&host, &remote);
}

#[test]
fn remote_images_follow_unicode_placeholder_redraws() {
    let (host, remote) = terminal();
    remote.feed_output(b"\x1b_Ga=T,i=1,U=1,f=32,s=1,v=1,c=2,r=2;AQID/w==\x1b\\");
    remote.feed_output("\x1b[3;2H\x1b[38;2;0;0;1m\u{10eeee}\u{305}\u{305}\x1b[0m".as_bytes());
    assert_placements_match(&host, &remote);
    assert_eq!(remote.kitty_graphics_placements().len(), 1);
    remote.feed_output(
        "\x1b[3;2H \x1b[2;2H\x1b[38;2;0;0;1m\u{10eeee}\u{305}\u{305}\x1b[0m".as_bytes(),
    );
    assert_placements_match(&host, &remote);
    remote.feed_output(b"\x1b[2;2H ");
    assert_placements_match(&host, &remote);
    assert!(remote.kitty_graphics_placements().is_empty());
}

#[test]
fn remote_resize_does_not_retransfer_unchanged_image_pixels() {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    let (host, mut remote) = terminal();
    // Incompressible pixels expose the cost hidden by one-pixel fixtures.
    let mut seed = 1u32;
    let pixels: Vec<u8> = (0..1024 * 1024 * 4)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as u8
        })
        .collect();
    remote.feed_output(
        format!(
            "\x1b_Ga=T,i=1,f=32,s=1024,v=1024,c=2,r=2,C=1;{}\x1b\\",
            STANDARD.encode(pixels)
        )
        .as_bytes(),
    );
    assert_eq!(remote.kitty_graphics_placements().len(), 1);
    host.1.store(0, Ordering::Relaxed);
    let start = std::time::Instant::now();
    for cols in 21..33 {
        remote.resize(TerminalSize {
            cols,
            rows: 6,
            cell_width: 10.0,
            cell_height: 20.0,
        });
        assert_placements_match(&host, &remote);
    }
    let bytes = host.1.load(Ordering::Relaxed);
    eprintln!(
        "12 resize/render cycles: {:?}, {bytes} reply bytes",
        start.elapsed()
    );
    assert!(
        bytes < 64 * 1024,
        "resizing retransferred unchanged image pixels: {bytes} bytes"
    );
}
