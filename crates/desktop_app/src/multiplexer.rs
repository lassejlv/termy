//! Desktop window/workspace bookkeeping for the built-in session host.
use crate::{
    config::AppConfig,
    workspace_store::{StoredPane, StoredSession, StoredTab, StoredWorkspace},
};
use gpui_kit::{App, Global};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::Path,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use termy_core::multiplexer::{
    LayoutWatch, PaneInfo, SessionClient, connect_or_start, default_root,
};

use termy_core::session_model::{StoredMultiplexer as SavedState, StoredWindow as SavedWindow};

struct State {
    /// Per window, the session it last synced with the host: the merge base
    /// for its next save and for remote changes.
    saved: SavedState,
    /// Per window, the newest layout revision it already reflects. Older
    /// notifications are stale and skipped.
    floors: HashMap<String, u64>,
    pending: VecDeque<String>,
    claimed: HashSet<String>,
}

struct Manager {
    client: SessionClient,
    state: Mutex<State>,
    write_gate: Mutex<()>,
    /// Set once this app run has marked the SQLite session store as imported
    /// into the host (see `TerminalView::record_multiplexer_import`).
    import_recorded: AtomicBool,
    /// Open windows waiting for layout changes (see [`WindowSession::layout_changes`]).
    listeners: Mutex<Vec<flume::Sender<Arc<RemoteLayout>>>>,
}

/// The host's layout at one revision, as the layout watcher read it.
pub(crate) struct RemoteLayout {
    pub(crate) revision: u64,
    pub(crate) layout: SavedState,
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
    /// The layout as reconciled and published (see [`reconcile_on_host`]).
    saved: SavedState,
    watch: Option<LayoutWatch>,
}

fn load(client: SessionClient) -> anyhow::Result<Connected> {
    let saved = reconcile_on_host(&client).map_err(anyhow::Error::msg)?;
    let watch = client
        .watch_layout()
        // Windows still save; they only miss other clients' edits until the
        // next launch.
        .inspect_err(|error| log::error!("Cannot watch the multiplexer layout: {error:#}"))
        .ok();
    Ok(Connected {
        client,
        saved,
        watch,
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
    let result = result.map(|connected| install(connected, cx));
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

fn install(connected: Connected, cx: &mut App) {
    let Connected {
        client,
        saved,
        watch,
    } = connected;
    let pending = saved
        .windows
        .iter()
        .map(|window| window.id.clone())
        .collect();
    let manager = Arc::new(Manager {
        client,
        state: Mutex::new(State {
            saved,
            floors: HashMap::new(),
            pending,
            claimed: HashSet::new(),
        }),
        write_gate: Mutex::new(()),
        import_recorded: AtomicBool::new(false),
        listeners: Mutex::new(Vec::new()),
    });
    if let Some(watch) = watch {
        spawn_layout_watcher(&manager, watch);
    }
    cx.set_global(AppSessions(Some(manager)));
}

/// Closes panes whose process exited, reconciles the host's layout with the
/// panes that are still alive and publishes the result (audit B2, B4).
fn reconcile_on_host(client: &SessionClient) -> Result<SavedState, String> {
    let panes = client.list().map_err(|error| error.to_string())?;
    let (exited, live): (Vec<_>, Vec<_>) = panes.into_iter().partition(|pane| pane.exited);
    for pane in exited {
        // Reattaching would replay `Exit` and close the window again.
        if let Err(error) = client.close(&pane.id) {
            log::warn!(
                "Cannot close exited multiplexer pane {}: {error:#}",
                pane.id
            );
        }
    }
    client
        .update_layout(|layout| {
            reconcile(layout, &live);
            Ok(())
        })
        .map(|update| update.layout)
        .map_err(|error| format!("Cannot restore multiplexer layout: {error:#}"))
}

/// Reads the layout after every revision the host reports and hands it to
/// each open window. A lost watch is reopened with backoff (the host may
/// have restarted); the thread ends with the manager.
fn spawn_layout_watcher(manager: &Arc<Manager>, watch: LayoutWatch) {
    const MAX_BACKOFF: Duration = Duration::from_secs(30);
    let manager: Weak<Manager> = Arc::downgrade(manager);
    let spawned = std::thread::Builder::new()
        .name("termy-layout-watch".into())
        .spawn(move || {
            let mut watch = Some(watch);
            let mut backoff = Duration::from_millis(250);
            loop {
                let Some(current) = watch.as_mut() else {
                    std::thread::sleep(backoff);
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    let Some(manager) = manager.upgrade() else {
                        return;
                    };
                    match manager.client.watch_layout() {
                        Ok(reopened) => watch = Some(reopened),
                        Err(error) => log::debug!("Cannot watch the multiplexer layout: {error:#}"),
                    }
                    continue;
                };
                if let Err(error) = current.recv() {
                    log::warn!("Lost the multiplexer layout watch: {error:#}");
                    watch = None;
                    continue;
                }
                backoff = Duration::from_millis(250);
                let Some(manager) = manager.upgrade() else {
                    return;
                };
                let remote = manager
                    .client
                    .layout_revision()
                    .map_err(|error| error.to_string())
                    .and_then(|(revision, json)| {
                        Ok(RemoteLayout {
                            revision,
                            layout: SavedState::parse(json.as_deref())?,
                        })
                    });
                match remote {
                    Ok(remote) => {
                        let remote = Arc::new(remote);
                        manager
                            .listeners
                            .lock()
                            .unwrap()
                            .retain(|listener| listener.send(Arc::clone(&remote)).is_ok());
                    }
                    Err(error) => log::warn!("Cannot read the multiplexer layout: {error}"),
                }
            }
        });
    if let Err(error) = spawned {
        log::error!("Cannot watch the multiplexer layout: {error}");
    }
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
        .filter(|window| window.session.session_ids().any(|id| running.contains(id)))
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
    let (id, gone) = {
        let state = manager.state.lock().unwrap();
        let Some(id) = detached_window_ids(&state, &live).into_iter().next() else {
            return false;
        };
        let running: HashSet<&str> = live
            .iter()
            .filter(|pane| !pane.exited)
            .map(|pane| pane.id.as_str())
            .collect();
        let gone: Vec<String> = state
            .saved
            .windows
            .iter()
            .filter(|window| window.id == id)
            .flat_map(|window| window.session.session_ids())
            .filter(|pane| !running.contains(pane))
            .map(str::to_owned)
            .collect();
        (id, gone)
    };
    // Panes that ended since the window was saved cannot be reattached:
    // close the exited ones and drop them from the layout, collapsing their
    // tabs as a close would, before the window claims it.
    for pane in live
        .iter()
        .filter(|pane| pane.exited && gone.contains(&pane.id))
    {
        if let Err(error) = manager.client.close(&pane.id) {
            log::warn!(
                "Cannot close exited multiplexer pane {}: {error:#}",
                pane.id
            );
        }
    }
    let published = manager.client.update_layout(|layout| {
        if let Some(window) = layout.windows.iter_mut().find(|window| window.id == id) {
            for pane in &gone {
                window.session.remove_pane(pane);
            }
        }
        Ok(())
    });
    let mut state = manager.state.lock().unwrap();
    match published {
        Ok(update) => {
            if let Some(window) = update
                .layout
                .windows
                .into_iter()
                .find(|window| window.id == id)
            {
                set_base(&mut state, &id, window.session);
            }
        }
        Err(error) => {
            log::warn!("Cannot prune the detached window's layout: {error:#}");
            if let Some(window) = state
                .saved
                .windows
                .iter_mut()
                .find(|window| window.id == id)
            {
                for pane in &gone {
                    window.session.remove_pane(pane);
                }
            }
        }
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
    /// Set by a failed save, cleared by a successful one: failures are shown
    /// once per streak rather than on every debounced save.
    save_failing: AtomicBool,
}

pub(crate) fn claim_window(
    cx: &App,
    reopen: bool,
) -> Option<(Arc<WindowSession>, Option<StoredSession>)> {
    let manager = Arc::clone(cx.try_global::<AppSessions>()?.0.as_ref()?);
    let mut state = manager.state.lock().unwrap();
    if reopen && state.claimed.is_empty() && state.pending.is_empty() {
        // Other clients may have changed the layout since launch: reconcile
        // what the host has now, not this process's copy (audit B16).
        match reconcile_on_host(&manager.client) {
            Ok(saved) => {
                state.saved = saved;
                state.floors.clear();
            }
            Err(error) => log::error!("{error}"),
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
            save_failing: AtomicBool::new(false),
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

    /// Release this window and remove it from the host layout: it closed
    /// for good, because it was closed with "Terminate", its last shell
    /// exited (audit B2) or its last tab went. Nothing is left to reopen.
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
            state.floors.remove(&self.id);
        }
        self.manager
            .client
            .update_layout(|layout| {
                layout.windows.retain(|window| window.id != self.id);
                Ok(())
            })
            .map(drop)
            .map_err(|error| format!("{error:#}"))
    }

    /// Every layout the host publishes from now on (audit B5).
    pub(crate) fn layout_changes(&self) -> flume::Receiver<Arc<RemoteLayout>> {
        let (sender, receiver) = flume::unbounded();
        self.manager.listeners.lock().unwrap().push(sender);
        receiver
    }

    /// This window's `(base, remote)` sessions when `remote` changed it since
    /// it last synced. `None` for stale revisions and unchanged windows.
    pub(crate) fn remote_change(
        &self,
        remote: &RemoteLayout,
    ) -> Option<(StoredSession, StoredSession)> {
        if self.is_released() {
            return None;
        }
        let state = self.manager.state.lock().unwrap();
        if state.floors.get(&self.id).copied().unwrap_or(0) > remote.revision {
            return None;
        }
        let session = &remote
            .layout
            .windows
            .iter()
            .find(|window| window.id == self.id)?
            .session;
        let base = &state
            .saved
            .windows
            .iter()
            .find(|window| window.id == self.id)?
            .session;
        (base != session).then(|| (base.clone(), session.clone()))
    }

    /// Records that the view now reflects `session` at `revision`.
    pub(crate) fn adopt_remote(&self, revision: u64, session: StoredSession) {
        let mut state = self.manager.state.lock().unwrap();
        set_base(&mut state, &self.id, session);
        let floor = state.floors.entry(self.id.clone()).or_default();
        *floor = (*floor).max(revision);
    }

    /// The session this window last synced with the host.
    pub(crate) fn base(&self) -> Option<StoredSession> {
        self.manager
            .state
            .lock()
            .unwrap()
            .saved
            .windows
            .iter()
            .find(|window| window.id == self.id)
            .map(|window| window.session.clone())
    }

    /// Publishes `session`, the view's state, merged onto the host's copy of
    /// this window. `base` must be [`Self::base`] as read together with
    /// `session` on the foreground: a remote change adopted while this save
    /// waited (see [`Self::adopt_remote`]) is not in `session`, and merging
    /// against the newer base would read its additions as local closures.
    pub(crate) fn save(
        &self,
        session: StoredSession,
        base: Option<StoredSession>,
    ) -> Result<(), String> {
        // Serialize writes from all windows so an older snapshot cannot
        // overwrite a newer tab transfer or a different window's changes.
        let _gate = self.manager.write_gate.lock().unwrap();
        if self.is_released() {
            return Ok(());
        }
        let result = self.manager.client.update_layout(|remote| {
            match remote
                .windows
                .iter_mut()
                .find(|window| window.id == self.id)
            {
                Some(window) => {
                    let base = base.as_ref().unwrap_or(&window.session);
                    window.session =
                        termy_core::session_model::rebase_session(base, &session, &window.session);
                }
                // A window that is open exists, even if another client
                // removed it from the layout.
                None => remote.windows.push(SavedWindow {
                    id: self.id.clone(),
                    session: session.clone(),
                }),
            }
            Ok(())
        });
        match result {
            Ok(update) => {
                // Retain what this view actually knows, not the merged remote
                // state. Otherwise its next unchanged save would undo
                // remote-only edits. A remote change adopted meanwhile is what
                // the view shows now; keep that base instead. A failed save
                // leaves the base alone: its local edits were not published,
                // so the next save must still see them as edits (audit B3).
                let mut state = self.manager.state.lock().unwrap();
                let current = state
                    .saved
                    .windows
                    .iter()
                    .find(|window| window.id == self.id)
                    .map(|window| &window.session);
                if current == base.as_ref() {
                    set_base(&mut state, &self.id, session);
                }
                let floor = state.floors.entry(self.id.clone()).or_default();
                *floor = (*floor).max(update.revision);
                self.save_failing.store(false, Ordering::Release);
                Ok(())
            }
            Err(error) => {
                let error = format!("{error:#}");
                if !self.save_failing.swap(true, Ordering::AcqRel) {
                    crate::ui::toast::error(format!(
                        "Could not save this window's multiplexer layout: {error}"
                    ));
                }
                Err(error)
            }
        }
    }
}

fn set_base(state: &mut State, id: &str, session: StoredSession) {
    if let Some(window) = state
        .saved
        .windows
        .iter_mut()
        .find(|window| window.id == id)
    {
        window.session = session;
    } else {
        state.saved.windows.push(SavedWindow {
            id: id.to_owned(),
            session,
        });
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

/// Brings a saved layout in line with the host's live panes: dead panes leave
/// their tabs the way a close would (collapsing split trees, following the
/// active tab and pane), empty workspaces stay, and live panes no window
/// lists are collected in a "Recovered sessions" workspace.
fn reconcile(saved: &mut SavedState, live: &[PaneInfo]) {
    let live_ids: HashSet<&str> = live.iter().map(|pane| pane.id.as_str()).collect();
    let dead: Vec<String> = saved
        .session_ids()
        .filter(|id| !live_ids.contains(id))
        .map(str::to_owned)
        .collect();
    for id in dead {
        saved.remove_pane(&id);
    }
    for workspace in saved
        .windows
        .iter_mut()
        .flat_map(|window| &mut window.session.workspaces)
    {
        // Live layouts always carry session IDs; drop anything else.
        for tab in &mut workspace.tabs {
            if tab.panes.iter().any(|pane| pane.session_id.is_none()) {
                tab.panes.retain(|pane| pane.session_id.is_some());
                tab.layout_tree_json = None;
                tab.zoomed = false;
                tab.active_pane = tab.active_pane.min(tab.panes.len().saturating_sub(1));
            }
        }
        workspace.tabs.retain(|tab| !tab.panes.is_empty());
        workspace.active_tab = workspace
            .active_tab
            .min(workspace.tabs.len().saturating_sub(1));
    }
    // A window with no tab anywhere has nothing to show: its panes were all
    // closed or exited. Opening it would only add a blank window. An empty
    // workspace beside live tabs stays (`mux window W create-workspace`).
    saved.windows.retain(|window| {
        window
            .session
            .workspaces
            .iter()
            .any(|workspace| !workspace.tabs.is_empty())
    });
    let referenced: HashSet<String> = saved.session_ids().map(str::to_owned).collect();
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
    install(load(client).map_err(|error| error.to_string())?, cx);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: &str, left: u16, top: u16, width: u16, height: u16) -> StoredPane {
        StoredPane {
            session_id: Some(id.into()),
            left,
            top,
            width,
            height,
            buffer: None,
            zoom_steps: 0,
        }
    }

    fn tab(panes: Vec<StoredPane>) -> StoredTab {
        StoredTab {
            zoomed: false,
            presentation: None,
            pinned: false,
            manual_title: None,
            active_pane: 0,
            layout_tree_json: None,
            panes,
        }
    }

    fn workspace(name: &str, tabs: Vec<StoredTab>, active_tab: usize) -> StoredWorkspace {
        StoredWorkspace {
            name: name.into(),
            pinned: false,
            active_tab,
            tabs,
        }
    }

    fn live(ids: &[&str]) -> Vec<PaneInfo> {
        ids.iter()
            .map(|id| PaneInfo {
                id: (*id).into(),
                child_pid: None,
                title: None,
                working_directory: None,
                exited: false,
                shell: None,
            })
            .collect()
    }

    fn saved(workspaces: Vec<StoredWorkspace>, active_workspace: usize) -> SavedState {
        SavedState {
            version: 1,
            windows: vec![SavedWindow {
                id: "window".into(),
                session: StoredSession {
                    workspaces,
                    active_workspace,
                    zoom_steps: 0,
                },
            }],
        }
    }

    /// Audit B4: an empty workspace (`mux window W create-workspace`) and the
    /// active workspace selection survive a relaunch.
    #[test]
    fn reconcile_keeps_empty_workspaces() {
        let mut layout = saved(
            vec![
                workspace("Work", vec![tab(vec![pane("a", 0, 0, 80, 24)])], 0),
                workspace("Review", Vec::new(), 0),
            ],
            1,
        );
        let before = layout.clone();
        reconcile(&mut layout, &live(&["a"]));
        assert_eq!(layout, before);
    }

    /// Audit B16: removing a dead pane collapses the split tree, so the tab
    /// has no hole and later CLI edits can infer it.
    #[test]
    fn reconcile_collapses_split_trees() {
        let mut layout = saved(
            vec![workspace(
                "Work",
                vec![tab(vec![
                    pane("a", 0, 0, 40, 24),
                    pane("b", 40, 0, 40, 12),
                    pane("c", 40, 12, 40, 12),
                ])],
                0,
            )],
            0,
        );
        reconcile(&mut layout, &live(&["a", "b"]));
        let tab = &layout.windows[0].session.workspaces[0].tabs[0];
        assert_eq!(
            tab.panes,
            vec![pane("a", 0, 0, 40, 24), pane("b", 40, 0, 40, 24)]
        );
        assert!(tab.layout_tree().is_ok());
    }

    /// A window whose panes all went away is not reopened as a blank window.
    #[test]
    fn reconcile_drops_windows_without_tabs() {
        let mut layout = saved(
            vec![
                workspace("Work", vec![tab(vec![pane("gone", 0, 0, 80, 24)])], 0),
                workspace("Review", Vec::new(), 0),
            ],
            0,
        );
        reconcile(&mut layout, &[]);
        assert!(layout.windows.is_empty());
    }

    /// Audit L5: tabs [A, B, C] with B active and A dead keep B selected.
    #[test]
    fn reconcile_follows_the_active_tab_by_identity() {
        let mut layout = saved(
            vec![workspace(
                "Work",
                vec![
                    tab(vec![pane("a", 0, 0, 80, 24)]),
                    tab(vec![pane("b", 0, 0, 80, 24)]),
                    tab(vec![pane("c", 0, 0, 80, 24)]),
                ],
                1,
            )],
            0,
        );
        reconcile(&mut layout, &live(&["b", "c", "orphan"]));
        let session = &layout.windows[0].session;
        let workspace = &session.workspaces[0];
        assert_eq!(workspace.tabs.len(), 2);
        assert_eq!(
            workspace.tabs[workspace.active_tab].panes[0]
                .session_id
                .as_deref(),
            Some("b")
        );
        assert_eq!(session.workspaces[1].name, "Recovered sessions");
        assert_eq!(
            session.workspaces[1].tabs[0].panes[0].session_id.as_deref(),
            Some("orphan")
        );
    }

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

    /// The host refuses a layout the app cannot parse (audit I4), so such a
    /// layout can no longer stop the multiplexer from starting.
    #[cfg(unix)]
    #[gpui_kit::test]
    fn unparsable_layouts_never_reach_the_app(cx: &mut gpui_kit::TestAppContext) {
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
        assert!(client.set_layout("{ not a layout".into()).is_err());
        cx.update(|cx| {
            start(&root, Path::new("/nonexistent/termy"), cx);
            assert!(enabled(cx));
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
