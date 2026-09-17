use crate::{discovery, protocol::*};
use anyhow::{bail, ensure};
use flume::Sender;
use std::{
    net::{Shutdown, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use termy_core::{remote::*, *};

struct Connection {
    stream: Mutex<TcpStream>,
}

impl Connection {
    fn connect(root: &Path) -> anyhow::Result<Self> {
        Ok(Self {
            stream: Mutex::new(discovery::connect(root)?),
        })
    }
    fn request(&self, request: Request) -> anyhow::Result<Response> {
        let mut stream = self.stream.lock().unwrap();
        write_message(&mut *stream, &request)?;
        match read_message(&mut *stream)? {
            Response::Error(error) => bail!(error),
            response => Ok(response),
        }
    }
}

#[derive(Clone)]
pub struct SessionClient {
    root: PathBuf,
    connection: Arc<Connection>,
}

impl SessionClient {
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
        let mut stream = discovery::connect(&self.root)?;
        write_message(&mut stream, &Request::Subscribe(id.to_owned()))?;
        ensure!(
            matches!(read_message(&mut stream)?, Response::Ok),
            "cannot subscribe to terminal session"
        );
        stream.set_read_timeout(None)?;
        let close_stream = stream.try_clone()?;
        let (input_tx, input_rx) = flume::bounded(256);
        let shared = Arc::new(ClientState {
            state: Mutex::new(state),
            pending: Mutex::new(Pending::default()),
            wakeup,
            wakeup_enabled: AtomicBool::new(true),
            disconnected: AtomicBool::new(false),
            closing: AtomicBool::new(false),
        });
        let reader_shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("termy-session-read".into())
            .spawn(move || {
                let result = (|| -> anyhow::Result<()> {
                    loop {
                        match read_message(&mut stream)? {
                            Update::State(state) => {
                                validate_state(&state)?;
                                *reader_shared.state.lock().unwrap() = state;
                                reader_shared.pending.lock().unwrap().wakeup = true;
                            }
                            Update::Events(events) => {
                                let mut pending = reader_shared.pending.lock().unwrap();
                                ensure!(
                                    pending.events.len() + events.len() <= 1024,
                                    "session event queue overflow"
                                );
                                pending.events.extend(events);
                            }
                            Update::Host { id, request } => reader_shared
                                .pending
                                .lock()
                                .unwrap()
                                .hosts
                                .push((id, request)),
                        }
                        reader_shared.notify();
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
    /// Explicitly terminate all hosted terminals and the background host.
    pub fn shutdown(&self) -> anyhow::Result<()> {
        self.connection.request(Request::Shutdown)?;
        Ok(())
    }
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

struct ClientState {
    state: Mutex<Arc<RemoteState>>,
    pending: Mutex<Pending>,
    wakeup: Option<TerminalWakeupNotifier>,
    wakeup_enabled: AtomicBool,
    disconnected: AtomicBool,
    closing: AtomicBool,
}

impl ClientState {
    fn notify(&self) {
        if self.wakeup_enabled.load(Ordering::Acquire)
            && let Some(wakeup) = &self.wakeup
        {
            wakeup.notify();
        }
    }
    fn fail(&self, error: &anyhow::Error) {
        if self.closing.load(Ordering::Acquire) || self.disconnected.swap(true, Ordering::AcqRel) {
            return;
        }
        log::error!("terminal session connection lost: {error}");
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

impl RemoteTransport for ClientTerminal {
    fn state(&self) -> Arc<RemoteState> {
        Arc::clone(&self.shared.state.lock().unwrap())
    }
    fn request(&self, command: RemoteCommand) -> anyhow::Result<RemoteReply> {
        match self.rpc.request(Request::Command {
            pane: self.id.clone(),
            command,
        }) {
            Ok(Response::Reply(reply)) => Ok(reply),
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
        if std::mem::take(&mut pending.wakeup) {
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
