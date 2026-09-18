use crate::multiplexer::{
    discovery::{self, Endpoint},
    pane::Pane,
    protocol::*,
};
use anyhow::{bail, ensure};
use fs4::fs_std::FileExt;
use rand_core::{OsRng, RngCore};
use std::{
    collections::HashMap,
    io::Write,
    net::{SocketAddr, TcpListener, TcpStream},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

struct Server {
    panes: Mutex<HashMap<String, Pane>>,
    layout: Mutex<Option<String>>,
    token: String,
    address: SocketAddr,
    stopping: AtomicBool,
}

pub fn serve(root: &Path) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        // Launchers can pass down the inheritable "ignore Ctrl+C" attribute.
        // Clear it before creating ConPTY shells so their foreground commands
        // can be interrupted even when this host started from a background job.
        // SAFETY: a null handler changes only the current process attribute;
        // no callback or pointer is registered.
        if unsafe { windows_sys::Win32::System::Console::SetConsoleCtrlHandler(None, 0) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    discovery::prepare_root(root)?;
    let lock = discovery::private_file(&root.join("host.lock"))?;
    ensure!(
        lock.try_lock_exclusive()?,
        "a session host is already running"
    );
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let address = listener.local_addr()?;
    let mut bytes = [0; 32];
    OsRng.fill_bytes(&mut bytes);
    let token: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let endpoint = Endpoint {
        conditional_layout_updates: true,
        graphics_stream: true,
        version: VERSION,
        port: address.port(),
        token: token.clone(),
    };
    let path = root.join("endpoint.json");
    let temp = root.join("endpoint.tmp");
    let mut file = discovery::private_file(&temp)?;
    file.set_len(0)?;
    file.write_all(&serde_json::to_vec(&endpoint)?)?;
    file.sync_all()?;
    std::fs::rename(temp, &path)?;
    let server = Arc::new(Server {
        panes: Mutex::new(HashMap::new()),
        layout: Mutex::new(None),
        token,
        address,
        stopping: AtomicBool::new(false),
    });
    for stream in listener.incoming() {
        if server.stopping.load(Ordering::Acquire) {
            break;
        }
        let stream = stream?;
        let server = Arc::clone(&server);
        std::thread::Builder::new()
            .name("termy-session-client".into())
            .spawn(move || {
                if let Err(error) = serve_client(stream, &server) {
                    log::debug!("session client disconnected: {error}");
                }
            })?;
    }
    for pane in server.panes.lock().unwrap().values() {
        pane.close();
    }
    std::fs::remove_file(path)?;
    Ok(())
}

impl Server {
    fn pane(&self, id: &str) -> anyhow::Result<Pane> {
        self.panes
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("terminal session no longer exists: {id}"))
    }

    fn execute(&self, request: Request) -> anyhow::Result<Response> {
        Ok(match request {
            Request::Create(launch) => {
                let pane = Pane::create(*launch)?;
                let info = pane.info();
                let state = pane.refresh()?;
                self.panes.lock().unwrap().insert(info.id.clone(), pane);
                Response::Attached(info, state)
            }
            Request::Attach(id) => {
                let pane = self.pane(&id)?;
                Response::Attached(pane.info(), pane.refresh()?)
            }
            Request::Command { pane, command } => {
                Response::Reply(self.pane(&pane)?.command(command)?)
            }
            Request::HostReply { pane, id, reply } => {
                self.pane(&pane)?.host_reply(id, reply);
                Response::Ok
            }
            Request::List => Response::Panes(
                self.panes
                    .lock()
                    .unwrap()
                    .values()
                    .map(Pane::info)
                    .collect(),
            ),
            Request::Close(id) => {
                if let Some(pane) = self.panes.lock().unwrap().remove(&id) {
                    pane.close();
                }
                Response::Ok
            }
            Request::GetLayout => Response::Layout(self.layout.lock().unwrap().clone()),
            Request::SetLayout(layout) => {
                ensure!(layout.len() <= 4 * 1024 * 1024, "session layout too large");
                *self.layout.lock().unwrap() = Some(layout);
                Response::Ok
            }
            Request::CompareAndSetLayout {
                expected,
                replacement,
            } => {
                ensure!(
                    replacement.len() <= 4 * 1024 * 1024,
                    "session layout too large"
                );
                let mut layout = self.layout.lock().unwrap();
                if *layout == expected {
                    *layout = Some(replacement);
                    Response::LayoutUpdated(true)
                } else {
                    Response::LayoutUpdated(false)
                }
            }
            Request::Shutdown
            | Request::Hello { .. }
            | Request::Subscribe(_)
            | Request::SubscribeGraphics(_) => {
                bail!("invalid session request")
            }
        })
    }
}

fn serve_client(mut stream: TcpStream, server: &Server) -> anyhow::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let Request::Hello { version, token } = read_message_limited(&mut stream, 4096)? else {
        bail!("session authentication required");
    };
    ensure!(
        version == VERSION && token == server.token,
        "invalid session authentication"
    );
    write_message(&mut stream, &Response::Ok)?;
    stream.set_read_timeout(None)?;
    loop {
        let request = read_message(&mut stream)?;
        if matches!(request, Request::Shutdown) {
            write_message(&mut stream, &Response::Ok)?;
            server.stopping.store(true, Ordering::Release);
            let _ = TcpStream::connect(server.address);
            return Ok(());
        }
        let graphics_stream = matches!(&request, Request::SubscribeGraphics(_));
        if let Request::Subscribe(id) | Request::SubscribeGraphics(id) = request {
            let subscription = server.pane(&id)?.subscribe()?;
            write_message(&mut stream, &Response::Ok)?;
            let mut reader = stream.try_clone()?;
            let monitor = Arc::clone(&subscription);
            std::thread::Builder::new()
                .name("termy-session-detach".into())
                .spawn(move || {
                    use std::io::Read;
                    let _ = reader.read(&mut [0u8]);
                    monitor.close();
                })?;
            let mut graphics = crate::remote::graphics::GraphicsEncoder::default();
            while let Some(mut update) = subscription.next() {
                if graphics_stream && let Update::State(state) = update {
                    let images = graphics.encode(state.graphics.as_deref().unwrap_or_default());
                    update = Update::GraphicsState(state, images);
                }
                if let Err(error) = write_message(&mut stream, &update) {
                    subscription.close();
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                    return Err(error);
                }
            }
            let _ = stream.shutdown(std::net::Shutdown::Both);
            return Ok(());
        }
        let response = server
            .execute(request)
            .unwrap_or_else(|error| Response::Error(error.to_string()));
        write_message(&mut stream, &response)?;
    }
}
