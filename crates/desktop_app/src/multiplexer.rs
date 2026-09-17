//! Desktop window/workspace bookkeeping for the built-in session host.
use crate::{
    config::AppConfig,
    workspace_store::{StoredPane, StoredSession, StoredTab, StoredWorkspace},
};
use gpui::{App, Global};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashSet, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use termy_multiplexer::{SessionClient, connect_or_start};

#[derive(Clone, Serialize, Deserialize)]
struct SavedWindow {
    id: String,
    session: StoredSession,
}

#[derive(Clone, Serialize, Deserialize)]
struct SavedState {
    version: u32,
    windows: Vec<SavedWindow>,
}

struct State {
    saved: SavedState,
    pending: VecDeque<String>,
    claimed: HashSet<String>,
}

struct Manager {
    client: SessionClient,
    state: Mutex<State>,
    write_gate: Mutex<()>,
}

struct AppSessions(Option<Arc<Manager>>);
impl Global for AppSessions {}

pub(crate) fn initialize(config: &AppConfig, cx: &mut App) -> Result<(), String> {
    if cx.try_global::<AppSessions>().is_some() {
        return Ok(());
    }
    if !config.multiplexer_enabled {
        cx.set_global(AppSessions(None));
        return Ok(());
    }
    let path = crate::config::ensure_config_file().map_err(|error| error.to_string())?;
    let root = path
        .parent()
        .ok_or("config directory is missing")?
        .join("multiplexer");
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let client = connect_or_start(&root, &executable)
        .map_err(|error| format!("Cannot start the built-in multiplexer: {error}"))?;
    install(client, cx)
}

fn install(client: SessionClient, cx: &mut App) -> Result<(), String> {
    let mut saved = match client.layout().map_err(|error| error.to_string())? {
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
    let panes = client.list().map_err(|error| error.to_string())?;
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
        }),
        write_gate: Mutex::new(()),
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

    pub(crate) fn is_released(&self) -> bool {
        self.released.load(Ordering::Acquire)
    }

    pub(crate) fn release(&self) {
        if !self.released.swap(true, Ordering::AcqRel) {
            self.manager.state.lock().unwrap().claimed.remove(&self.id);
        }
    }

    pub(crate) fn save(&self, session: StoredSession) -> Result<(), String> {
        // Serialize writes from all windows so an older snapshot cannot
        // overwrite a newer tab transfer or a different window's changes.
        let _gate = self.manager.write_gate.lock().unwrap();
        if self.is_released() {
            return Ok(());
        }
        let json = {
            let mut state = self.manager.state.lock().unwrap();
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
            serde_json::to_string(&state.saved).map_err(|error| error.to_string())?
        };
        self.manager
            .client
            .set_layout(json)
            .map_err(|error| error.to_string())
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

fn reconcile(saved: &mut SavedState, live: &[termy_multiplexer::PaneInfo]) {
    let live_ids: HashSet<&str> = live.iter().map(|pane| pane.id.as_str()).collect();
    for window in &mut saved.windows {
        for workspace in &mut window.session.workspaces {
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
        window
            .session
            .workspaces
            .retain(|workspace| !workspace.tabs.is_empty());
        window.session.active_workspace = window
            .session
            .active_workspace
            .min(window.session.workspaces.len().saturating_sub(1));
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
                },
            });
        }
    }
}

#[cfg(test)]
pub(crate) fn install_for_test(client: SessionClient, cx: &mut App) -> Result<(), String> {
    install(client, cx)
}
