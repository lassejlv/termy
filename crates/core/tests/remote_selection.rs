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

struct DelayedHost {
    terminal: Mutex<Terminal>,
    displayed: Arc<RemoteState>,
    requests: AtomicUsize,
}

impl RemoteTransport for DelayedHost {
    fn state(&self) -> Arc<RemoteState> {
        Arc::clone(&self.displayed)
    }
    fn request(&self, command: RemoteCommand) -> anyhow::Result<RemoteReply> {
        self.requests.fetch_add(1, Ordering::Relaxed);
        Ok(remote::execute(&mut self.terminal.lock().unwrap(), command))
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

fn fixture() -> (Arc<DelayedHost>, Terminal) {
    let terminal = Terminal::new_display(
        TerminalSize {
            cols: 12,
            rows: 3,
            ..TerminalSize::default()
        },
        None,
    );
    terminal.feed_output(b"selected\r\nsecond\r\nthird");
    let host = Arc::new(DelayedHost {
        displayed: Arc::new(RemoteState::capture(&terminal)),
        terminal: Mutex::new(terminal),
        requests: AtomicUsize::new(0),
    });
    (Arc::clone(&host), Terminal::from_remote(host))
}

#[test]
fn selection_reads_the_displayed_viewport_while_host_output_is_pending() {
    let (host, remote) = fixture();
    // Output has scrolled on the host, but its next frame has not reached the UI.
    host.terminal.lock().unwrap().feed_output(b"\r\nfourth");
    let mut selected = String::new();
    remote.visit_line_cells(0, 0, |_, _, _, cell| selected.push_str(&cell.text));
    assert_eq!(selected.trim_end(), "selected");
    assert_eq!(
        host.requests.load(Ordering::Relaxed),
        0,
        "visible selection must not block on IPC"
    );
}

#[test]
fn empty_selection_bounds_query_does_not_contact_host() {
    let (host, remote) = fixture();
    assert_eq!(
        remote.visit_line_cells(1, 0, |_, _, _, _| panic!("empty range")),
        (0, 2, 12)
    );
    assert_eq!(host.requests.load(Ordering::Relaxed), 0);
}

#[test]
fn selecting_plain_text_does_not_query_hyperlinks_over_ipc() {
    let (host, remote) = fixture();
    for col in 0..12 {
        assert!(remote.hyperlink_at(0, col).is_none());
    }
    assert_eq!(host.requests.load(Ordering::Relaxed), 0);
}

/// Audit L19: a click resolves against the frame the user clicked, even
/// when output scrolled the host's viewport since that frame was shown.
#[test]
fn links_resolve_against_the_clicked_frame_after_output_scrolls() {
    let terminal = Terminal::new_display(
        TerminalSize {
            cols: 30,
            rows: 3,
            ..TerminalSize::default()
        },
        None,
    );
    terminal.feed_output(
        b"\x1b]8;;https://one.example\x1b\\one\x1b]8;;\x1b\\\r\n\
          \x1b]8;;https://two.example\x1b\\two\x1b]8;;\x1b\\\r\n\
          see https://three.example",
    );
    let host = Arc::new(DelayedHost {
        displayed: Arc::new(RemoteState::capture(&terminal)),
        terminal: Mutex::new(terminal),
        requests: AtomicUsize::new(0),
    });
    let remote = Terminal::from_remote(host.clone());
    // A line of output moves every row up before the click arrives.
    host.terminal.lock().unwrap().feed_output(b"\r\nfour");
    let hyperlink = remote.hyperlink_at(1, 1).expect("the clicked hyperlink");
    assert_eq!(hyperlink.target, "https://two.example");
    let link = remote.link_at(2, 8).expect("the clicked link");
    assert_eq!(link.target, "https://three.example");
    // Rows are reported in the clicked frame's numbering.
    assert_eq!((link.start_row, link.end_row), (2, 2));
    // The first row has scrolled off the host's screen: no stale answer.
    assert!(remote.hyperlink_at(0, 1).is_none());
}

/// Audit L19: on the alternate screen nothing scrolls into history, so the
/// host recognizes a moved row by its text and rejects a stale click
/// instead of resolving a neighbouring line.
#[test]
fn alternate_screen_scrolling_rejects_clicks_on_moved_rows() {
    let terminal = Terminal::new_display(
        TerminalSize {
            cols: 30,
            rows: 3,
            ..TerminalSize::default()
        },
        None,
    );
    terminal.feed_output(
        b"\x1b[?1049h\x1b[H\
          \x1b]8;;https://one.example\x1b\\one\x1b]8;;\x1b\\\r\n\
          \x1b]8;;https://two.example\x1b\\two\x1b]8;;\x1b\\",
    );
    let host = Arc::new(DelayedHost {
        displayed: Arc::new(RemoteState::capture(&terminal)),
        terminal: Mutex::new(terminal),
        requests: AtomicUsize::new(0),
    });
    let remote = Terminal::from_remote(host.clone());
    // A pager scrolls its screen up by one line (SU) before the click.
    host.terminal.lock().unwrap().feed_output(b"\x1b[S");
    assert!(
        remote.hyperlink_at(0, 1).is_none(),
        "a click on the moved row resolved the line that took its place"
    );
    // Output that leaves the clicked row alone keeps the click valid.
    host.terminal
        .lock()
        .unwrap()
        .feed_output(b"\x1b[H\x1b]8;;https://one.example\x1b\\one\x1b]8;;\x1b\\\x1b[3;1Hthree");
    let hyperlink = remote.hyperlink_at(0, 1).expect("the unchanged row");
    assert_eq!(hyperlink.target, "https://one.example");
}

/// History outside the viewport is read from the host, numbered like the
/// frame on screen even when output scrolled since (audit B13).
#[test]
fn selection_outside_the_viewport_reads_host_history_as_shown() {
    let terminal = Terminal::new_display(
        TerminalSize {
            cols: 12,
            rows: 3,
            ..TerminalSize::default()
        },
        None,
    );
    // "selected" scrolls into history before the frame on screen.
    terminal.feed_output(b"selected\r\nsecond\r\nthird\r\nfourth");
    let host = Arc::new(DelayedHost {
        displayed: Arc::new(RemoteState::capture(&terminal)),
        terminal: Mutex::new(terminal),
        requests: AtomicUsize::new(0),
    });
    let remote = Terminal::from_remote(host.clone());
    // More output scrolls "second" into history before the copy.
    host.terminal.lock().unwrap().feed_output(b"\r\nfifth");
    let mut selected = String::new();
    remote.visit_line_cells(-1, -1, |_, _, _, cell| selected.push_str(&cell.text));
    assert_eq!(selected.trim_end(), "selected");
    assert_eq!(host.requests.load(Ordering::Relaxed), 1);
}
