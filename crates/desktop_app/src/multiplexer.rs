//! Desktop window/workspace bookkeeping for the built-in session host.
use crate::{
    config::AppConfig,
    workspace_store::{StoredPane, StoredSession, StoredTab, StoredWorkspace},
};
use gpui_kit::{App, Global};
use std::{
    collections::{HashSet, VecDeque},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use termy_core::multiplexer::{PaneInfo, SessionClient, connect_or_start, default_root};

use termy_core::session_model::{StoredMultiplexer as SavedState, StoredWindow as SavedWindow};

struct State {
    saved: SavedState,
    pending: VecDeque<String>,
    claimed: HashSet<String>,
    published_windows: HashSet<String>,
}

struct Manager {
    client: SessionClient,
    state: Mutex<State>,
    write_gate: Mutex<()>,
    /// Set once this app run has marked the SQLite session store as imported
    /// into the host (see `TerminalView::record_multiplexer_import`).
    import_recorded: AtomicBool,
}

struct AppSessions(Option<Arc<Manager>>);
impl Global for AppSessions {}

/// Connects to the session host once per app run, starting it if needed. A
/// failure never blocks launch: the app falls back to regular terminals and
/// shows a toast that says why.
pub(crate) fn initialize(config: &AppConfig, cx: &mut App) {
    if cx.try_global::<AppSessions>().is_some() {
        return;
    }
    if !config.multiplexer_enabled {
        cx.set_global(AppSessions(None));
        return;
    }
    let result = (|| {
        crate::config::ensure_config_file().map_err(anyhow::Error::msg)?;
        // Shared with `termy mux`, so the CLI finds the host this window starts.
        let root = default_root().ok_or_else(|| anyhow::anyhow!("its directory is unavailable"))?;
        Ok((root, std::env::current_exe()?))
    })();
    match result {
        Ok((root, executable)) => start(&root, &executable, cx),
        Err(error) => fall_back_to_local(&error, None, cx),
    }
}

/// How long launch waits for the session host. Starting a new host takes up
/// to about five seconds; a hung host would otherwise hold the first window
/// for the sum of every request timeout.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(8);

/// What start-up reads from the host before the first window opens.
struct Connected {
    client: SessionClient,
    layout: Option<String>,
    panes: Vec<PaneInfo>,
}

fn load(client: SessionClient) -> anyhow::Result<Connected> {
    Ok(Connected {
        layout: client.layout()?,
        panes: client.list()?,
        client,
    })
}

fn start(root: &Path, executable: &Path, cx: &mut App) {
    let (root, executable) = (root.to_owned(), executable.to_owned());
    let log = root.join("host.log");
    start_within(
        STARTUP_TIMEOUT,
        move || connect_or_start(&root, &executable).and_then(load),
        &log,
        cx,
    );
}

/// Runs `connect` on a thread and gives up on it after `timeout`; the thread
/// then finishes, or keeps waiting on a hung host, on its own.
fn start_within(
    timeout: Duration,
    connect: impl FnOnce() -> anyhow::Result<Connected> + Send + 'static,
    log: &Path,
    cx: &mut App,
) {
    let (sender, receiver) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("termy-multiplexer-start".into())
        .spawn(move || {
            let _ = sender.send(connect());
        });
    let result = match spawned {
        Ok(_) => match receiver.recv_timeout(timeout) {
            Ok(result) => result,
            Err(_) => Err(anyhow::Error::new(StartupTimedOut(timeout))),
        },
        Err(error) => Err(anyhow::Error::new(error).context("start the multiplexer thread")),
    };
    let result = result.and_then(|connected| install(connected, cx).map_err(anyhow::Error::msg));
    if let Err(error) = result {
        fall_back_to_local(&error, Some(log), cx);
    }
}

#[derive(Debug)]
struct StartupTimedOut(Duration);

impl std::fmt::Display for StartupTimedOut {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            formatter,
            "the session host did not respond within {} s",
            self.0.as_secs()
        )
    }
}

impl std::error::Error for StartupTimedOut {}

/// Records why, and leaves this run on regular terminals. Callers that read
/// `tmux_enabled` must apply [`effective_config`], so the fallback never
/// starts tmux in place of the multiplexer.
fn fall_back_to_local(error: &anyhow::Error, log: Option<&Path>, cx: &mut App) {
    log::error!("Cannot start the built-in multiplexer: {error:#}");
    cx.set_global(AppSessions(None));
    cx.set_global(FellBackToLocal);
    let log = log.filter(|log| log.exists());
    crate::ui::toast::enqueue_toast(
        crate::ui::toast::ToastKind::Error,
        startup_failure_message(error, log),
        Some(Duration::from_secs(20)),
    );
}

/// A short toast: the first line of the error, and where the details are.
/// The whole error, with the host's last log lines, goes to the app log.
fn startup_failure_message(error: &anyhow::Error, log: Option<&Path>) -> String {
    const STOP: &str = "Run `termy mux shutdown` to stop it (this ends its terminals), then restart Termy. Using regular terminals for now.";
    if termy_core::multiplexer::is_version_mismatch(error) {
        return format!(
            "Built-in multiplexer unavailable: a session host from another Termy version is still running. {STOP}"
        );
    }
    if error.chain().any(|cause| cause.is::<StartupTimedOut>()) {
        return "Built-in multiplexer unavailable: the session host did not respond. If it is hung, `termy mux shutdown --force` stops it (this ends its terminals). Using regular terminals for now.".into();
    }
    let full = format!("{error:#}");
    let first_line = full.lines().next().unwrap_or_default();
    let mut message =
        format!("Built-in multiplexer unavailable, using regular terminals: {first_line}");
    if let Some(log) = log {
        message.push_str(&format!(" (see {})", log.display()));
    }
    message
}

/// The configuration this run actually uses. When the multiplexer was
/// configured but could not start, the run falls back to plain local
/// terminals: neither the multiplexer nor tmux, whatever `tmux_enabled` says,
/// so it matches the startup keybindings and the fallback toast.
pub(crate) fn effective_config(config: &mut AppConfig, cx: &App) {
    // The host started at launch, or not, applies until restart.
    config.multiplexer_enabled = enabled(cx);
    if cx.try_global::<FellBackToLocal>().is_some() {
        config.tmux_enabled = false;
    }
}

/// Set when the configured multiplexer could not start.
struct FellBackToLocal;
impl Global for FellBackToLocal {}

fn install(connected: Connected, cx: &mut App) -> Result<(), String> {
    let Connected {
        client,
        layout,
        panes,
    } = connected;
    let mut saved = match layout {
        Some(json) => serde_json::from_str::<SavedState>(&json)
            .map_err(|error| format!("Cannot restore multiplexer layout: {error}"))?,
        None => SavedState {
            version: 1,
            windows: Vec::new(),
        },
    };
    if saved.version != 1 {
        return Err("Unsupported multiplexer layout version".into());
    }
    let published_windows = saved
        .windows
        .iter()
        .map(|window| window.id.clone())
        .collect();
    reconcile(&mut saved, &panes);
    let pending = saved
        .windows
        .iter()
        .map(|window| window.id.clone())
        .collect();
    cx.set_global(AppSessions(Some(Arc::new(Manager {
        client,
        state: Mutex::new(State {
            saved,
            pending,
            claimed: HashSet::new(),
            published_windows,
        }),
        write_gate: Mutex::new(()),
        import_recorded: AtomicBool::new(false),
    }))));
    Ok(())
}

pub(crate) fn enabled(cx: &App) -> bool {
    cx.try_global::<AppSessions>()
        .is_some_and(|app| app.0.is_some())
}

pub(crate) fn pending_windows(cx: &App) -> usize {
    cx.try_global::<AppSessions>()
        .and_then(|app| app.0.as_ref())
        .map_or(0, |manager| manager.state.lock().unwrap().pending.len())
}

/// Saved windows that no open window shows and that still have a running
/// terminal in the host: a window closed while others stayed open, or one
/// not reopened yet. A window whose terminals all ended or exited is left
/// out, so reopening never brings back (and then saves over) a dead window.
fn detached_window_ids(state: &State, live: &[PaneInfo]) -> Vec<String> {
    let running: HashSet<&str> = live
        .iter()
        .filter(|pane| !pane.exited)
        .map(|pane| pane.id.as_str())
        .collect();
    state
        .saved
        .windows
        .iter()
        .filter(|window| !state.claimed.contains(&window.id))
        .filter(|window| session_ids(&window.session).any(|id| running.contains(id)))
        .map(|window| window.id.clone())
        .collect()
}

fn manager(cx: &App) -> Option<&Arc<Manager>> {
    cx.try_global::<AppSessions>()
        .and_then(|app| app.0.as_ref())
}

pub(crate) fn detached_windows(cx: &App) -> usize {
    let Some(manager) = manager(cx) else {
        return 0;
    };
    let Ok(live) = manager.client.list() else {
        return 0;
    };
    detached_window_ids(&manager.state.lock().unwrap(), &live).len()
}

/// Queue a detached window so the next window opened claims it instead of
/// starting empty. Returns `false` when there is nothing to reopen.
pub(crate) fn queue_detached_window(cx: &App) -> bool {
    let Some(manager) = manager(cx) else {
        return false;
    };
    let Ok(live) = manager.client.list() else {
        return false;
    };
    let mut state = manager.state.lock().unwrap();
    let Some(id) = detached_window_ids(&state, &live).into_iter().next() else {
        return false;
    };
    // Panes that ended since the window was saved cannot be reattached.
    let live_ids: HashSet<&str> = live.iter().map(|pane| pane.id.as_str()).collect();
    if let Some(window) = state
        .saved
        .windows
        .iter_mut()
        .find(|window| window.id == id)
    {
        prune_session(&mut window.session, &live_ids);
    }
    if !state.pending.contains(&id) {
        state.pending.push_front(id);
    }
    true
}

pub(crate) struct WindowSession {
    manager: Arc<Manager>,
    id: String,
    released: AtomicBool,
}

pub(crate) fn claim_window(
    cx: &App,
    reopen: bool,
) -> Option<(Arc<WindowSession>, Option<StoredSession>)> {
    let manager = Arc::clone(cx.try_global::<AppSessions>()?.0.as_ref()?);
    let mut state = manager.state.lock().unwrap();
    if reopen && state.claimed.is_empty() && state.pending.is_empty() {
        if let Ok(panes) = manager.client.list() {
            reconcile(&mut state.saved, &panes);
        }
        state.pending = state
            .saved
            .windows
            .iter()
            .map(|window| window.id.clone())
            .collect();
    }
    let saved = state.pending.pop_front().and_then(|id| {
        state
            .saved
            .windows
            .iter()
            .find(|window| window.id == id)
            .cloned()
    });
    let id = saved.as_ref().map_or_else(
        || uuid::Uuid::new_v4().to_string(),
        |window| window.id.clone(),
    );
    state.claimed.insert(id.clone());
    drop(state);
    Some((
        Arc::new(WindowSession {
            manager,
            id,
            released: AtomicBool::new(false),
        }),
        saved.map(|window| window.session),
    ))
}

impl WindowSession {
    pub(crate) fn client(&self) -> &SessionClient {
        &self.manager.client
    }

    pub(crate) fn import_recorded(&self) -> bool {
        self.manager.import_recorded.load(Ordering::Acquire)
    }

    pub(crate) fn set_import_recorded(&self) {
        self.manager.import_recorded.store(true, Ordering::Release);
    }

    pub(crate) fn is_released(&self) -> bool {
        self.released.load(Ordering::Acquire)
    }

    pub(crate) fn release(&self) {
        if !self.released.swap(true, Ordering::AcqRel) {
            self.manager.state.lock().unwrap().claimed.remove(&self.id);
        }
    }

    /// Release this window and remove it from the host layout, for a window
    /// closed with "Terminate": its terminals end, so there is nothing left
    /// to reopen. A layout that keeps changing is left for the next start's
    /// reconcile, which drops windows whose terminals are gone.
    pub(crate) fn forget(&self) -> Result<(), String> {
        let _gate = self.manager.write_gate.lock().unwrap();
        if self.released.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        {
            let mut state = self.manager.state.lock().unwrap();
            state.claimed.remove(&self.id);
            state.pending.retain(|id| id != &self.id);
            state.saved.windows.retain(|window| window.id != self.id);
        }
        for _ in 0..8 {
            let expected = self
                .manager
                .client
                .layout()
                .map_err(|error| error.to_string())?;
            let Some(json) = &expected else {
                return Ok(());
            };
            let mut remote =
                serde_json::from_str::<SavedState>(json).map_err(|error| error.to_string())?;
            let before = remote.windows.len();
            remote.windows.retain(|window| window.id != self.id);
            if remote.windows.len() == before {
                return Ok(());
            }
            let replacement = serde_json::to_string(&remote).map_err(|error| error.to_string())?;
            if self
                .manager
                .client
                .compare_and_set_layout(expected, replacement)
                .map_err(|error| error.to_string())?
            {
                return Ok(());
            }
        }
        Err("The session layout is changing".into())
    }

    pub(crate) fn save(&self, session: StoredSession) -> Result<(), String> {
        // Serialize writes from all windows so an older snapshot cannot
        // overwrite a newer tab transfer or a different window's changes.
        let _gate = self.manager.write_gate.lock().unwrap();
        if self.is_released() {
            return Ok(());
        }
        let previous = self
            .manager
            .state
            .lock()
            .unwrap()
            .saved
            .windows
            .iter()
            .find(|window| window.id == self.id)
            .map(|window| window.session.clone());
        let mut published = false;
        for _ in 0..8 {
            let expected = self
                .manager
                .client
                .layout()
                .map_err(|error| error.to_string())?;
            let mut remote = match &expected {
                Some(json) => {
                    serde_json::from_str::<SavedState>(json).map_err(|error| error.to_string())?
                }
                None => SavedState {
                    version: 1,
                    windows: Vec::new(),
                },
            };
            if remote.version != 1 {
                return Err("Unsupported multiplexer layout version".into());
            }
            if let Some(window) = remote
                .windows
                .iter_mut()
                .find(|window| window.id == self.id)
            {
                window.session = match &previous {
                    Some(base) => {
                        termy_core::session_model::merge_session(base, &session, &window.session)
                            .map_err(str::to_owned)?
                    }
                    None if window.session == session => session.clone(),
                    None => return Err("This window was created by another client".into()),
                };
            } else {
                if self
                    .manager
                    .state
                    .lock()
                    .unwrap()
                    .published_windows
                    .contains(&self.id)
                    && expected.is_some()
                {
                    return Err("This window was removed by another client".into());
                }
                remote.windows.push(SavedWindow {
                    id: self.id.clone(),
                    session: session.clone(),
                });
            }
            let replacement = serde_json::to_string(&remote).map_err(|error| error.to_string())?;
            if self
                .manager
                .client
                .compare_and_set_layout(expected, replacement)
                .map_err(|error| error.to_string())?
            {
                published = true;
                break;
            }
        }
        if !published {
            return Err("The session layout is changing; try saving again".into());
        }
        // Retain what this view actually knows, not the merged remote state.
        // Otherwise its next unchanged save would undo remote-only edits.
        let mut state = self.manager.state.lock().unwrap();
        state.published_windows.insert(self.id.clone());
        if let Some(window) = state
            .saved
            .windows
            .iter_mut()
            .find(|window| window.id == self.id)
        {
            window.session = session;
        } else {
            state.saved.windows.push(SavedWindow {
                id: self.id.clone(),
                session,
            });
        }
        Ok(())
    }
}

impl Drop for WindowSession {
    fn drop(&mut self) {
        self.release();
    }
}

/// Desktop ownership differs from transport ownership: removing a tab/pane
/// closes its process; a window being detached disarms this cleanup first.
pub(crate) struct PaneSession {
    pub(crate) id: String,
    client: SessionClient,
    close_on_drop: AtomicBool,
}

impl PaneSession {
    pub(crate) fn new(id: String, client: SessionClient) -> Self {
        Self {
            id,
            client,
            close_on_drop: AtomicBool::new(true),
        }
    }
    pub(crate) fn detach(&self) {
        self.close_on_drop.store(false, Ordering::Release);
    }
    pub(crate) fn attached(id: String, client: SessionClient) -> Self {
        Self {
            id,
            client,
            close_on_drop: AtomicBool::new(false),
        }
    }
    pub(crate) fn adopt(&self) {
        self.close_on_drop.store(true, Ordering::Release);
    }
}

impl Drop for PaneSession {
    fn drop(&mut self) {
        if self.close_on_drop.load(Ordering::Acquire)
            && let Err(error) = self.client.close(&self.id)
        {
            log::error!("Failed to close multiplexer pane: {error}");
        }
    }
}

fn session_ids(session: &StoredSession) -> impl Iterator<Item = &str> {
    session
        .workspaces
        .iter()
        .flat_map(|workspace| &workspace.tabs)
        .flat_map(|tab| &tab.panes)
        .filter_map(|pane| pane.session_id.as_deref())
}

/// Drop the panes of `session` whose host terminals are gone.
fn prune_session(session: &mut StoredSession, live_ids: &HashSet<&str>) {
    for workspace in &mut session.workspaces {
        for tab in &mut workspace.tabs {
            let original = tab.panes.len();
            tab.panes.retain(|pane| {
                pane.session_id
                    .as_deref()
                    .is_some_and(|id| live_ids.contains(id))
            });
            if tab.panes.len() != original {
                tab.layout_tree_json = None;
                tab.zoomed = false;
            }
            tab.active_pane = tab.active_pane.min(tab.panes.len().saturating_sub(1));
        }
        workspace.tabs.retain(|tab| !tab.panes.is_empty());
        workspace.active_tab = workspace
            .active_tab
            .min(workspace.tabs.len().saturating_sub(1));
    }
    session
        .workspaces
        .retain(|workspace| !workspace.tabs.is_empty());
    session.active_workspace = session
        .active_workspace
        .min(session.workspaces.len().saturating_sub(1));
}

fn reconcile(saved: &mut SavedState, live: &[PaneInfo]) {
    let live_ids: HashSet<&str> = live.iter().map(|pane| pane.id.as_str()).collect();
    for window in &mut saved.windows {
        prune_session(&mut window.session, &live_ids);
    }
    saved
        .windows
        .retain(|window| !window.session.workspaces.is_empty());
    let referenced: HashSet<String> = saved
        .windows
        .iter()
        .flat_map(|window| session_ids(&window.session))
        .map(str::to_owned)
        .collect();
    let recovered: Vec<_> = live
        .iter()
        .filter(|pane| !referenced.contains(&pane.id))
        .map(|pane| StoredTab {
            zoomed: false,
            presentation: None,
            pinned: false,
            manual_title: None,
            active_pane: 0,
            layout_tree_json: None,
            panes: vec![StoredPane {
                session_id: Some(pane.id.clone()),
                left: 0,
                top: 0,
                width: 80,
                height: 24,
                buffer: None,
                zoom_steps: 0,
            }],
        })
        .collect();
    if !recovered.is_empty() {
        let workspace = StoredWorkspace {
            name: if saved.windows.is_empty() {
                String::new()
            } else {
                "Recovered sessions".into()
            },
            pinned: false,
            active_tab: 0,
            tabs: recovered,
        };
        if let Some(window) = saved.windows.first_mut() {
            window.session.workspaces.push(workspace);
        } else {
            saved.windows.push(SavedWindow {
                id: uuid::Uuid::new_v4().to_string(),
                session: StoredSession {
                    workspaces: vec![workspace],
                    active_workspace: 0,
                    zoom_steps: 0,
                },
            });
        }
    }
}

#[cfg(all(test, unix))]
pub(crate) fn install_for_test(client: SessionClient, cx: &mut App) -> Result<(), String> {
    install(load(client).map_err(|error| error.to_string())?, cx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_failure_toast_says_how_to_stop_an_old_host() {
        let mismatch = termy_core::multiplexer::VersionMismatch { running: 3 }.into();
        let message = startup_failure_message(&mismatch, None);
        assert!(message.contains("termy mux shutdown"), "{message}");
        assert!(message.contains("regular terminals"), "{message}");
        let message = startup_failure_message(&anyhow::anyhow!("layout is corrupt"), None);
        assert!(message.contains("layout is corrupt"), "{message}");
        assert!(message.contains("regular terminals"), "{message}");
        let timeout = anyhow::Error::new(StartupTimedOut(STARTUP_TIMEOUT));
        let message = startup_failure_message(&timeout, None);
        assert!(message.contains("termy mux shutdown --force"), "{message}");
    }

    /// Finding 9: the host's log lines go to the app log, not the toast.
    #[test]
    fn startup_failure_toast_shows_one_line_and_the_log_path() {
        let error = anyhow::anyhow!(
            "background terminal host exited: exit status: 3\nLast lines of host.log:\nfirst\nsecond"
        );
        let log = Path::new("/sessions/host.log");
        let message = startup_failure_message(&error, Some(log));
        assert!(message.contains("exit status: 3"), "{message}");
        assert!(message.contains("see /sessions/host.log"), "{message}");
        assert!(!message.contains('\n'), "{message}");
        assert!(!message.contains("second"), "{message}");
    }

    #[cfg(unix)]
    fn private_root(temp: &tempfile::TempDir) -> std::path::PathBuf {
        use std::os::unix::fs::DirBuilderExt;
        let root = temp.path().join("host");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap();
        root
    }

    fn multiplexer_and_tmux_config() -> AppConfig {
        AppConfig {
            multiplexer_enabled: true,
            tmux_enabled: true,
            ..AppConfig::default()
        }
    }

    /// Audit B1: a host that is running but cannot be used (here, one of an
    /// older protocol) must not stop the app from launching. Finding 3: the
    /// fallback uses plain local terminals, not tmux.
    #[cfg(unix)]
    #[gpui_kit::test]
    fn unusable_host_falls_back_to_local_terminals(cx: &mut gpui_kit::TestAppContext) {
        let temp = tempfile::tempdir().unwrap();
        let root = private_root(&temp);
        std::fs::write(
            root.join("endpoint.json"),
            r#"{"version":3,"port":1,"token":"old","mutual_auth":true}"#,
        )
        .unwrap();
        let lock = std::fs::File::create(root.join("host.lock")).unwrap();
        lock.try_lock().unwrap();
        cx.update(|cx| {
            start(&root, Path::new("/nonexistent/termy"), cx);
            assert!(!enabled(cx));
            assert!(claim_window(cx, true).is_none());
            let mut config = multiplexer_and_tmux_config();
            effective_config(&mut config, cx);
            assert!(!config.multiplexer_enabled);
            assert!(!config.tmux_enabled);
            assert!(!crate::terminal_view::tmux_runtime_requested(&config));
        });
    }

    /// Without a fallback, `tmux_enabled` is left alone.
    #[gpui_kit::test]
    fn disabled_multiplexer_keeps_the_tmux_setting(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            let mut config = AppConfig {
                multiplexer_enabled: false,
                tmux_enabled: true,
                ..AppConfig::default()
            };
            initialize(&config, cx);
            effective_config(&mut config, cx);
            assert!(config.tmux_enabled);
        });
    }

    /// A layout the app cannot parse falls back instead of failing launch.
    #[cfg(unix)]
    #[gpui_kit::test]
    fn unparsable_layout_falls_back_to_local_terminals(cx: &mut gpui_kit::TestAppContext) {
        let temp = tempfile::tempdir().unwrap();
        let root = private_root(&temp);
        let host = {
            let root = root.clone();
            std::thread::spawn(move || termy_core::multiplexer::serve(&root))
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let client = loop {
            match SessionClient::connect(&root) {
                Ok(client) => break client,
                Err(error) => {
                    assert!(std::time::Instant::now() < deadline, "{error:#}");
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        };
        client.set_layout("{ not a layout".into()).unwrap();
        cx.update(|cx| {
            start(&root, Path::new("/nonexistent/termy"), cx);
            assert!(!enabled(cx));
            let mut config = multiplexer_and_tmux_config();
            effective_config(&mut config, cx);
            assert!(!config.tmux_enabled);
        });
        termy_core::multiplexer::shutdown_host(&root).unwrap();
        host.join().unwrap().unwrap();
    }

    /// Finding 4: a host that never answers holds launch for at most the
    /// start-up timeout.
    #[gpui_kit::test]
    fn hung_host_falls_back_within_the_startup_timeout(cx: &mut gpui_kit::TestAppContext) {
        let started = std::time::Instant::now();
        cx.update(|cx| {
            start_within(
                Duration::from_millis(200),
                || {
                    std::thread::sleep(Duration::from_secs(30));
                    anyhow::bail!("unreachable")
                },
                Path::new("/nonexistent/host.log"),
                cx,
            );
            assert!(!enabled(cx));
        });
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
