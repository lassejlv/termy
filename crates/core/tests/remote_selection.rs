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

#[test]
fn selection_outside_the_viewport_still_reads_host_history() {
    let (host, remote) = fixture();
    host.terminal.lock().unwrap().feed_output(b"\r\nfourth");
    let mut selected = String::new();
    remote.visit_line_cells(-1, -1, |_, _, _, cell| selected.push_str(&cell.text));
    assert_eq!(selected.trim_end(), "selected");
    assert_eq!(host.requests.load(Ordering::Relaxed), 1);
}
