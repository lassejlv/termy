use crate::multiplexer::{discovery, protocol::*};
use crate::{remote::*, *};
use anyhow::{Context, bail, ensure};
use flume::Sender;
use std::{
    net::{Shutdown, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

/// Longer than the host's own per-request deadline, so a slow host answers
/// with its error before the client gives up on the reply.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

struct Connection {
    root: PathBuf,
    /// `None` after a failed exchange; the next request reconnects.
    stream: Mutex<Option<TcpStream>>,
    conditional_layout_updates: bool,
    viewport_replies: bool,
}

impl Connection {
    fn connect(root: &Path) -> anyhow::Result<Self> {
        let (stream, capabilities) = open_rpc_stream(root)?;
        Ok(Self {
            root: root.to_owned(),
            stream: Mutex::new(Some(stream)),
            conditional_layout_updates: capabilities.conditional_layout_updates,
            viewport_replies: capabilities.viewport_replies,
        })
    }
    fn request(&self, request: Request) -> anyhow::Result<Response> {
        let mut slot = self.stream.lock().unwrap();
        let stream = match slot.as_mut() {
            Some(stream) => stream,
            None => slot.insert(open_rpc_stream(&self.root)?.0),
        };
        let response = match write_message(stream, &request).and_then(|()| read_message(stream)) {
            Ok(response) => response,
            Err(error) => {
                // A timed-out or broken exchange leaves the stream mid-reply:
                // its late answer would be read as the next request's reply.
                *slot = None;
                return Err(error);
            }
        };
        drop(slot);
        match response {
            Response::Error(error) => bail!(error),
            response => Ok(response),
        }
    }
}

fn open_rpc_stream(root: &Path) -> anyhow::Result<(TcpStream, discovery::Endpoint)> {
    let (stream, capabilities) = discovery::connect_with_capabilities(root)?;
    stream.set_read_timeout(Some(RESPONSE_TIMEOUT))?;
    Ok((stream, capabilities))
}

#[derive(Clone)]
pub struct SessionClient {
    root: PathBuf,
    connection: Arc<Connection>,
}

impl SessionClient {
    /// Apply a workspace edit without replacing another client's layout edits.
    pub fn edit_workspace(
        &self,
        window_id: &str,
        index: usize,
        expected: &crate::session_model::StoredWorkspace,
        edit: &crate::session_model::WorkspaceEdit,
    ) -> anyhow::Result<crate::session_model::StoredMultiplexer> {
        for _ in 0..8 {
            let serialized = self
                .layout()?
                .ok_or_else(|| anyhow::anyhow!("No saved session layout"))?;
            let mut layout: crate::session_model::StoredMultiplexer =
                serde_json::from_str(&serialized)?;
            ensure!(layout.version == 1, "Unsupported session layout version");
            let workspace = layout
                .windows
                .iter_mut()
                .find(|window| window.id == window_id)
                .and_then(|window| window.session.workspaces.get_mut(index))
                .ok_or_else(|| anyhow::anyhow!("Workspace no longer exists"))?;
            ensure!(
                workspace == expected,
                "Workspace changed in another client; refresh before editing"
            );
            edit.apply(workspace)
                .map_err(|error| anyhow::anyhow!(error))?;
            let replacement = serde_json::to_string(&layout)?;
            if self.compare_and_set_layout(Some(serialized), replacement)? {
                return Ok(layout);
            }
        }
        bail!("Session layout is changing; retry the edit")
    }
    pub fn supports_conditional_layout_updates(&self) -> bool {
        self.connection.conditional_layout_updates
    }
    pub fn connect(root: &Path) -> anyhow::Result<Self> {
        Ok(Self {
            root: root.to_owned(),
            connection: Arc::new(Connection::connect(root)?),
        })
    }

    pub fn create(
        &self,
        launch: PaneLaunch,
        wakeup: Option<TerminalWakeupNotifier>,
    ) -> anyhow::Result<(String, Terminal)> {
        let Response::Attached(info, state) =
            self.connection.request(Request::Create(Box::new(launch)))?
        else {
            bail!("invalid create response");
        };
        let id = info.id;
        let terminal = self.attach_state(&id, state, wakeup)?;
        Ok((id, terminal))
    }

    pub fn attach(
        &self,
        id: &str,
        wakeup: Option<TerminalWakeupNotifier>,
    ) -> anyhow::Result<Terminal> {
        let Response::Attached(_, state) =
            self.connection.request(Request::Attach(id.to_owned()))?
        else {
            bail!("invalid attach response");
        };
        self.attach_state(id, state, wakeup)
    }

    fn attach_state(
        &self,
        id: &str,
        state: Arc<RemoteState>,
        wakeup: Option<TerminalWakeupNotifier>,
    ) -> anyhow::Result<Terminal> {
        validate_state(&state)?;
        let rpc = Connection::connect(&self.root)?;
        let input = Connection::connect(&self.root)?;
        let (mut stream, capabilities) = discovery::connect_with_capabilities(&self.root)?;
        let subscription = if capabilities.graphics_stream {
            Request::SubscribeGraphics(id.to_owned())
        } else {
            Request::Subscribe(id.to_owned())
        };
        write_message(&mut stream, &subscription)?;
        ensure!(
            matches!(read_message(&mut stream)?, Response::Ok),
            "cannot subscribe to terminal session"
        );
        stream.set_read_timeout(None)?;
        let close_stream = stream.try_clone()?;
        let (input_tx, input_rx) = flume::bounded(256);
        let initial_state = awaiting_graphics((*state).clone());
        let shared = Arc::new(ClientState {
            state: Mutex::new(Arc::new(initial_state)),
            state_updated: Condvar::new(),
            pending: Mutex::new(Pending::default()),
            wakeup,
            wakeup_enabled: AtomicBool::new(true),
            disconnected: AtomicBool::new(false),
            closing: AtomicBool::new(false),
        });
        let reader_shared = Arc::clone(&shared);
        let legacy_graphics = if capabilities.graphics_stream {
            None
        } else {
            Some(Connection::connect(&self.root)?)
        };
        let reader_pane_id = id.to_owned();
        std::thread::Builder::new()
            .name("termy-session-read".into())
            .spawn(move || {
                let mut graphics = graphics::GraphicsDecoder::default();
                let mut legacy_cache = LegacyGraphicsCache::default();
                let result = (|| -> anyhow::Result<()> {
                    loop {
                        let mut tab_chrome = false;
                        match read_message(&mut stream)? {
                            Update::State(mut state) => {
                                validate_state(&state)?;
                                let connection = legacy_graphics
                                    .as_ref()
                                    .context("unexpected legacy terminal update")?;
                                legacy_cache.update(
                                    Arc::make_mut(&mut state),
                                    || match connection.request(Request::Command {
                                        pane: reader_pane_id.clone(),
                                        command: RemoteCommand::Graphics,
                                    })? {
                                        Response::Reply(RemoteReply::Graphics(
                                            revision,
                                            placements,
                                        )) => Ok((revision, placements)),
                                        _ => bail!("invalid terminal graphics response"),
                                    },
                                )?;
                                reader_shared.publish_state(state);
                            }
                            Update::GraphicsState(mut state, update) => {
                                validate_state(&state)?;
                                Arc::make_mut(&mut state).graphics = Some(graphics.decode(update)?);
                                reader_shared.publish_state(state);
                            }
                            Update::Events(events) => {
                                tab_chrome = events.iter().any(|event| {
                                    matches!(
                                        event,
                                        TerminalEvent::ProgramStatus(_)
                                            | TerminalEvent::Title(_)
                                            | TerminalEvent::ResetTitle
                                    )
                                });
                                reader_shared
                                    .pending
                                    .lock()
                                    .unwrap()
                                    .extend_events(events)?;
                            }
                            Update::Host { id, request } => reader_shared
                                .pending
                                .lock()
                                .unwrap()
                                .hosts
                                .push((id, request)),
                        }
                        if tab_chrome {
                            reader_shared.notify_tab_chrome();
                        } else {
                            reader_shared.notify();
                        }
                    }
                })();
                if let Err(error) = result {
                    reader_shared.fail(&error);
                }
            })?;
        let input_shared = Arc::clone(&shared);
        let pane_id = id.to_owned();
        std::thread::Builder::new()
            .name("termy-session-input".into())
            .spawn(move || {
                while let Ok(command) = input_rx.recv() {
                    if let Err(error) = input.request(Request::Command {
                        pane: pane_id.clone(),
                        command,
                    }) {
                        input_shared.fail(&error);
                        break;
                    }
                }
            })?;
        Ok(Terminal::from_remote(Arc::new(ClientTerminal {
            id: id.to_owned(),
            rpc,
            host: Arc::clone(&self.connection),
            shared,
            input: input_tx,
            close_stream,
        })))
    }

    pub fn list(&self) -> anyhow::Result<Vec<PaneInfo>> {
        match self.connection.request(Request::List)? {
            Response::Panes(panes) => Ok(panes),
            _ => bail!("invalid session list"),
        }
    }
    pub fn close(&self, id: &str) -> anyhow::Result<()> {
        self.connection.request(Request::Close(id.to_owned()))?;
        Ok(())
    }
    /// Wait for the host to accept input before returning. Unlike a view's
    /// asynchronous input queue, this is safe for a short-lived CLI process.
    pub fn write(&self, id: &str, bytes: Vec<u8>) -> anyhow::Result<()> {
        self.send_command(id, RemoteCommand::Write(bytes))
    }
    /// The cell metrics (pixels) the attached UI last applied to `id`.
    pub(crate) fn cell_size(&self, id: &str) -> anyhow::Result<(f32, f32)> {
        let Response::Attached(_, state) =
            self.connection.request(Request::Attach(id.to_owned()))?
        else {
            bail!("invalid attach response");
        };
        Ok((state.size.cell_width, state.size.cell_height))
    }
    pub fn resize(&self, id: &str, size: TerminalSize) -> anyhow::Result<()> {
        self.send_command(id, RemoteCommand::Resize(size))
    }
    fn send_command(&self, id: &str, command: RemoteCommand) -> anyhow::Result<()> {
        match self.connection.request(Request::Command {
            pane: id.to_owned(),
            command,
        })? {
            Response::Reply(_) => Ok(()),
            _ => bail!("invalid terminal command response"),
        }
    }
    pub fn layout(&self) -> anyhow::Result<Option<String>> {
        match self.connection.request(Request::GetLayout)? {
            Response::Layout(layout) => Ok(layout),
            _ => bail!("invalid session layout"),
        }
    }
    pub fn set_layout(&self, layout: String) -> anyhow::Result<()> {
        self.connection.request(Request::SetLayout(layout))?;
        Ok(())
    }
    /// Publish a layout only if no other client changed the snapshot we read.
    /// A false result leaves the host's layout untouched.
    pub fn compare_and_set_layout(
        &self,
        expected: Option<String>,
        replacement: String,
    ) -> anyhow::Result<bool> {
        ensure!(
            self.supports_conditional_layout_updates(),
            "This running multiplexer predates CLI layout editing; keep its terminals attached or restart it after closing its sessions to enable layout editing"
        );
        match self.connection.request(Request::CompareAndSetLayout {
            expected,
            replacement,
        })? {
            Response::LayoutUpdated(updated) => Ok(updated),
            _ => bail!("multiplexer host does not support conditional layout updates"),
        }
    }
    /// Explicitly terminate all hosted terminals and the background host.
    pub fn shutdown(&self) -> anyhow::Result<()> {
        self.connection.request(Request::Shutdown)?;
        Ok(())
    }
}

fn awaiting_graphics(mut state: RemoteState) -> RemoteState {
    // Keep painting local while the subscription loads its first image snapshot.
    // Do not label this empty placeholder with the completed host revision:
    // the renderer must invalidate it when those first streamed images arrive.
    state.graphics_revision = 0;
    state.graphics = Some(Vec::new());
    state
}

fn validate_state(state: &RemoteState) -> anyhow::Result<()> {
    let metadata = state.render.metadata;
    ensure!(
        metadata.cols > 0
            && metadata.rows > 0
            && metadata.cols == state.size.cols
            && metadata.rows == state.size.rows,
        "invalid remote terminal dimensions"
    );
    ensure!(
        state.render.cells.len() == usize::from(metadata.cols) * usize::from(metadata.rows),
        "incomplete remote terminal viewport"
    );
    Ok(())
}

#[derive(Default)]
struct Pending {
    wakeup: bool,
    events: Vec<TerminalEvent>,
    hosts: Vec<(u64, RemoteHostRequest)>,
}

impl Pending {
    fn extend_events(&mut self, events: Vec<TerminalEvent>) -> anyhow::Result<()> {
        for event in events {
            // A suspended UI keeps only the latest state and one of each
            // repeated notification, like the host's queue for slow clients.
            if !coalesce_event(&mut self.events, &event) {
                continue;
            }
            ensure!(self.events.len() < 1024, "session event queue overflow");
            self.events.push(event);
        }
        Ok(())
    }
}

struct ClientState {
    state: Mutex<Arc<RemoteState>>,
    state_updated: Condvar,
    pending: Mutex<Pending>,
    wakeup: Option<TerminalWakeupNotifier>,
    wakeup_enabled: AtomicBool,
    disconnected: AtomicBool,
    closing: AtomicBool,
}

impl ClientState {
    fn publish_state(&self, state: Arc<RemoteState>) {
        *self.state.lock().unwrap() = state;
        self.pending.lock().unwrap().wakeup = true;
        self.state_updated.notify_all();
    }

    fn wait_for_generation(&self, generation: u64) -> anyhow::Result<()> {
        let (state, _) = self
            .state_updated
            .wait_timeout_while(
                self.state.lock().unwrap(),
                Duration::from_secs(5),
                |state| {
                    state.render.metadata.generation < generation
                        && !self.disconnected.load(Ordering::Acquire)
                },
            )
            .unwrap();
        ensure!(
            !self.disconnected.load(Ordering::Acquire),
            "terminal session disconnected"
        );
        ensure!(
            state.render.metadata.generation >= generation,
            "terminal viewport did not catch up"
        );
        Ok(())
    }

    fn notify(&self) {
        if self.wakeup_enabled.load(Ordering::Acquire)
            && let Some(wakeup) = &self.wakeup
        {
            wakeup.notify();
        }
    }
    // Titles and program status drive tab chrome, which stays visible for
    // hidden tabs, so they wake the host even while render wakeups are suspended.
    fn notify_tab_chrome(&self) {
        if let Some(wakeup) = &self.wakeup {
            wakeup.notify();
        }
    }
    fn fail(&self, error: &anyhow::Error) {
        if self.closing.load(Ordering::Acquire) || self.disconnected.swap(true, Ordering::AcqRel) {
            return;
        }
        log::error!("terminal session connection lost: {error}");
        // Pair the notification with the state mutex used by scroll waiters.
        let state = self.state.lock().unwrap();
        self.state_updated.notify_all();
        drop(state);
        self.pending
            .lock()
            .unwrap()
            .events
            .push(TerminalEvent::Title(
                "Session disconnected — reopen Termy to reconnect".into(),
            ));
        self.notify();
    }
}

struct ClientTerminal {
    id: String,
    rpc: Connection,
    host: Arc<Connection>,
    shared: Arc<ClientState>,
    input: Sender<RemoteCommand>,
    close_stream: TcpStream,
}

impl Drop for ClientTerminal {
    fn drop(&mut self) {
        self.shared.closing.store(true, Ordering::Release);
        let _ = self.close_stream.shutdown(Shutdown::Both);
    }
}

impl ClientTerminal {
    // The host publishes the new frame before replying, so selection waits for
    // one subscription update rather than attaching for a second full state.
    fn viewport_request(&self, command: RemoteCommand) -> anyhow::Result<RemoteReply> {
        let result = (|| {
            let Response::Viewport {
                changed,
                generation,
            } = self.rpc.request(Request::ViewportCommand {
                pane: self.id.clone(),
                command,
            })?
            else {
                bail!("invalid terminal viewport response");
            };
            if changed {
                self.shared.wait_for_generation(generation)?;
            }
            Ok(RemoteReply::Changed(changed))
        })();
        if let Err(error) = &result {
            self.shared.fail(error);
        }
        result
    }
}

impl RemoteTransport for ClientTerminal {
    fn state(&self) -> Arc<RemoteState> {
        Arc::clone(&self.shared.state.lock().unwrap())
    }
    fn request(&self, command: RemoteCommand) -> anyhow::Result<RemoteReply> {
        let changes_viewport = matches!(
            command,
            RemoteCommand::Scroll(_)
                | RemoteCommand::ScrollToBottom
                | RemoteCommand::ClearScrollback
        );
        if changes_viewport && self.rpc.viewport_replies {
            return self.viewport_request(command);
        }
        match self.rpc.request(Request::Command {
            pane: self.id.clone(),
            command,
        }) {
            Ok(Response::Reply(reply)) => {
                if changes_viewport && matches!(reply, RemoteReply::Changed(true)) {
                    // Attach refreshes/publishes immediately on every supported
                    // host. Wait for that generation on the ordered subscription
                    // (including its graphics), rather than letting selection
                    // record the old offset during the 16ms frame interval.
                    let result = (|| {
                        let Response::Attached(_, state) =
                            self.rpc.request(Request::Attach(self.id.clone()))?
                        else {
                            bail!("invalid terminal refresh response");
                        };
                        self.shared
                            .wait_for_generation(state.render.metadata.generation)
                    })();
                    if let Err(error) = result {
                        self.shared.fail(&error);
                        return Err(error);
                    }
                }
                Ok(reply)
            }
            Ok(_) => bail!("invalid terminal response"),
            Err(error) => {
                self.shared.fail(&error);
                Err(error)
            }
        }
    }
    fn send(&self, command: RemoteCommand) {
        if self.shared.disconnected.load(Ordering::Acquire) {
            return;
        }
        if let Err(error) = self.input.send_timeout(command, Duration::from_millis(100)) {
            self.shared.fail(&anyhow::anyhow!(error.to_string()));
        }
    }
    fn take_events(&self) -> Vec<TerminalEvent> {
        let mut pending = self.shared.pending.lock().unwrap();
        let mut events = std::mem::take(&mut pending.events);
        let wakeup = std::mem::take(&mut pending.wakeup);
        drop(pending);
        if wakeup {
            events.push(TerminalEvent::Wakeup);
        }
        events
    }
    fn take_host_requests(&self) -> Vec<(u64, RemoteHostRequest)> {
        std::mem::take(&mut self.shared.pending.lock().unwrap().hosts)
    }
    fn reply_to_host(&self, id: u64, reply: RemoteHostReply) {
        if let Err(error) = self.host.request(Request::HostReply {
            pane: self.id.clone(),
            id,
            reply,
        }) {
            self.shared.fail(&error);
        }
    }
    fn has_pending_events(&self) -> bool {
        let pending = self.shared.pending.lock().unwrap();
        pending.wakeup || !pending.events.is_empty() || !pending.hosts.is_empty()
    }
    fn set_wakeup_enabled(&self, enabled: bool) {
        self.shared.wakeup_enabled.store(enabled, Ordering::Release);
        if enabled && self.has_pending_events() {
            self.shared.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_streamed_images_invalidate_the_pending_graphics_snapshot() {
        let terminal = Terminal::new_display(crate::TerminalSize::default(), None);
        terminal.feed_output(b"\x1b_Ga=T,i=1,f=32,s=1,v=1,C=1,q=2;AQID/w==\x1b\\");
        let ready = RemoteState::capture(&terminal);
        assert_eq!(ready.graphics.as_ref().unwrap().len(), 1);
        let pending = awaiting_graphics(ready.clone());
        assert!(pending.graphics.as_ref().unwrap().is_empty());
        // A renderer may cache the initial empty snapshot before the reader
        // publishes these same host placements. Its revision must then change
        // even when there is no further PTY output or window resize.
        assert_ne!(pending.graphics_revision, ready.graphics_revision);
    }

    #[test]
    fn program_status_snapshots_do_not_accumulate_for_slow_clients() {
        let mut pending = Pending::default();
        pending.extend_events(vec![TerminalEvent::Bell]).unwrap();
        for _ in 0..2048 {
            pending
                .extend_events(vec![TerminalEvent::ProgramStatus(vec![])])
                .unwrap();
        }
        assert_eq!(pending.events.len(), 2);
        assert!(matches!(pending.events[0], TerminalEvent::Bell));
        assert!(matches!(pending.events[1], TerminalEvent::ProgramStatus(_)));
    }

    #[test]
    fn late_reply_after_timeout_is_not_returned_for_the_next_request() {
        use std::net::TcpListener;
        let temp = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::fs::write(
            temp.path().join("endpoint.json"),
            serde_json::json!({ "version": VERSION, "port": port, "token": "token" }).to_string(),
        )
        .unwrap();
        let host = std::thread::spawn(move || {
            let accept = || {
                let (mut stream, _) = listener.accept().unwrap();
                let _: Request = read_message(&mut stream).unwrap();
                write_message(&mut stream, &Response::Ok).unwrap();
                let _: Request = read_message(&mut stream).unwrap();
                stream
            };
            // The first request is answered only after the client gave up.
            let mut slow = accept();
            std::thread::sleep(Duration::from_millis(300));
            let _ = write_message(&mut slow, &Response::Layout(Some("first".into())));
            let mut next = accept();
            write_message(&mut next, &Response::Layout(Some("second".into()))).unwrap();
        });

        let connection = Connection::connect(temp.path()).unwrap();
        let short_timeout = Some(Duration::from_millis(100));
        let stream = connection.stream.lock().unwrap();
        stream
            .as_ref()
            .unwrap()
            .set_read_timeout(short_timeout)
            .unwrap();
        drop(stream);
        assert!(connection.request(Request::GetLayout).is_err());
        let Response::Layout(layout) = connection.request(Request::GetLayout).unwrap() else {
            panic!("expected a layout reply");
        };
        assert_eq!(layout.as_deref(), Some("second"));
        host.join().unwrap();
    }

    #[test]
    fn suspended_ui_queue_coalesces_bells_progress_and_clipboard_writes() {
        let mut pending = Pending::default();
        for round in 0..2048 {
            pending
                .extend_events(vec![
                    TerminalEvent::Bell,
                    TerminalEvent::Progress(ProgressState::InProgress((round % 100) as u8)),
                    TerminalEvent::ClipboardStore(format!("copy {round}")),
                ])
                .unwrap();
        }
        assert_eq!(pending.events.len(), 3);
        assert!(matches!(pending.events[0], TerminalEvent::Bell));
        assert!(matches!(
            pending.events[1],
            TerminalEvent::Progress(ProgressState::InProgress(47))
        ));
        assert!(
            matches!(&pending.events[2], TerminalEvent::ClipboardStore(text) if text == "copy 2047")
        );
    }
}
