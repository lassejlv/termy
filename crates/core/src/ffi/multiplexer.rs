//! C ABI over the multiplexer client, so embedders keep terminals alive in a
//! session host: programs outlive the app, and relaunching attaches again.
//! A terminal created or attached here is an ordinary `TermyFfiTerminal`;
//! every terminal function works on it.

use super::*;
use crate::multiplexer::{
    PaneLaunch, SessionClient, connect_or_start, default_root, is_exited_pane, is_host_unavailable,
    is_missing_pane, is_version_mismatch,
};

/// A connection to one session host. Calls block on the host, so hosts should
/// make them off their UI thread; one handle may be used from one thread at a
/// time.
pub struct TermyFfiMultiplexer {
    client: SessionClient,
}

fn status_for(error: &anyhow::Error) -> TermyFfiStatus {
    if is_version_mismatch(error) {
        TermyFfiStatus::VersionMismatch
    } else if is_host_unavailable(error) {
        TermyFfiStatus::HostUnavailable
    } else if is_missing_pane(error) || is_exited_pane(error) {
        TermyFfiStatus::NotFound
    } else {
        log::warn!("multiplexer request failed: {error:#}");
        TermyFfiStatus::WriteFailed
    }
}

/// Connecting fails for reasons beyond a missing host too, such as a session
/// directory that isn't private; all of them mean no host to use.
fn connect_status_for(error: &anyhow::Error) -> TermyFfiStatus {
    match status_for(error) {
        TermyFfiStatus::VersionMismatch => TermyFfiStatus::VersionMismatch,
        _ => TermyFfiStatus::HostUnavailable,
    }
}

/// A terminal handle with a wake channel, around a pane view.
fn attached_terminal(
    attach: impl FnOnce(crate::TerminalWakeupNotifier) -> anyhow::Result<Terminal>,
) -> Result<TermyFfiTerminal, TermyFfiStatus> {
    let (wakeups_tx, wakeups_rx) = bounded(1);
    let sender = wakeups_tx.clone();
    let notifier = crate::TerminalWakeupNotifier::new(move || {
        let _ = sender.try_send(());
    });
    let terminal = attach(notifier).map_err(|error| status_for(&error))?;
    Ok(TermyFfiTerminal {
        terminal,
        wakeups_tx: Some(wakeups_tx),
        wakeups_rx: Some(wakeups_rx),
        kitty_png_exports: Vec::new(),
    })
}

/// Connect to the session host in `root`, starting one with `host_executable`
/// (the `termy-session-host` binary) when none runs. An empty `root` uses the
/// directory Termy and `termy mux` share. Starting a host can take seconds.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_multiplexer_connect(
    root_ptr: *const u8,
    root_len: usize,
    host_executable_ptr: *const u8,
    host_executable_len: usize,
    out_multiplexer: *mut *mut TermyFfiMultiplexer,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if out_multiplexer.is_null() {
            return TermyFfiStatus::Null;
        }
        let root = match unsafe { optional_utf8(root_ptr, root_len) } {
            Ok(Some(root)) if !root.is_empty() => PathBuf::from(root),
            Ok(_) => match default_root() {
                Some(root) => root,
                None => return TermyFfiStatus::HostUnavailable,
            },
            Err(status) => return status,
        };
        let executable = match unsafe { required_utf8(host_executable_ptr, host_executable_len) } {
            Ok(path) => PathBuf::from(path),
            Err(status) => return status,
        };
        match connect_or_start(&root, &executable) {
            Ok(client) => {
                unsafe {
                    *out_multiplexer = Box::into_raw(Box::new(TermyFfiMultiplexer { client }));
                };
                TermyFfiStatus::Ok
            }
            Err(error) => connect_status_for(&error),
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_multiplexer_free(
    multiplexer: *mut TermyFfiMultiplexer,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if multiplexer.is_null() {
            return TermyFfiStatus::Null;
        }
        drop(unsafe { Box::from_raw(multiplexer) });
        TermyFfiStatus::Ok
    })
}

/// Start a pane in the host and attach a terminal to it. Options work as for
/// `termy_terminal_new_with_options`; the working directory is resolved here
/// and sent absolute. `out_pane_id` receives the pane's ID (free it with
/// `termy_buffer_free`); keep it to attach again after a relaunch.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_multiplexer_create_terminal(
    multiplexer: *const TermyFfiMultiplexer,
    size: TermyFfiSize,
    options: *const TermyFfiTerminalOptions,
    out_pane_id: *mut TermyFfiBytes,
    out_terminal: *mut *mut TermyFfiTerminal,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if multiplexer.is_null()
            || options.is_null()
            || out_pane_id.is_null()
            || out_terminal.is_null()
        {
            return TermyFfiStatus::Null;
        }
        let client = unsafe { &(*multiplexer).client };
        let options = unsafe { *options };
        let (mut config, shell_integration) = if options.config.is_null() {
            (TerminalRuntimeConfig::default(), None)
        } else {
            let loaded = unsafe { &(*options.config).loaded };
            (
                loaded.runtime_config.clone(),
                Some(tab_title_shell_integration_from_config(&loaded.app_config)),
            )
        };
        let working_directory = match unsafe {
            optional_utf8(options.working_directory_ptr, options.working_directory_len)
        } {
            Ok(value) => value,
            Err(status) => return status,
        };
        let command = match unsafe {
            optional_utf8(options.startup_command_ptr, options.startup_command_len)
        } {
            Ok(value) => value.filter(|command| !command.trim().is_empty()),
            Err(status) => return status,
        };
        match unsafe { env_vars_from_ffi(options.env_vars_ptr, options.env_vars_len) } {
            Ok(environment) => config.environment.extend(environment),
            Err(status) => return status,
        }
        let launch = PaneLaunch {
            size: size.into(),
            working_directory: PaneLaunch::client_working_directory(
                working_directory,
                crate::WorkingDirFallback::Home,
            ),
            shell_integration,
            config,
            launch: command.map(|command| crate::TerminalLaunch::ShellCommand(command.to_owned())),
            environment: Some(PaneLaunch::client_environment()),
        };
        let mut pane_id = None;
        let terminal = match attached_terminal(|notifier| {
            let (id, terminal) = client.create(launch, Some(notifier))?;
            pane_id = Some(id);
            Ok(terminal)
        }) {
            Ok(terminal) => terminal,
            Err(status) => return status,
        };
        unsafe {
            *out_pane_id = ffi_bytes_from_string(pane_id.unwrap_or_default());
            *out_terminal = Box::into_raw(Box::new(terminal));
        }
        TermyFfiStatus::Ok
    })
}

/// Attach a terminal to a running pane, such as one created before the app
/// last quit. Fails with `TERMY_FFI_NOT_FOUND` for a pane that is gone.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_multiplexer_attach_terminal(
    multiplexer: *const TermyFfiMultiplexer,
    pane_id_ptr: *const u8,
    pane_id_len: usize,
    out_terminal: *mut *mut TermyFfiTerminal,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if multiplexer.is_null() || out_terminal.is_null() {
            return TermyFfiStatus::Null;
        }
        let client = unsafe { &(*multiplexer).client };
        let id = match unsafe { required_utf8(pane_id_ptr, pane_id_len) } {
            Ok(id) => id,
            Err(status) => return status,
        };
        match attached_terminal(|notifier| client.attach(id, Some(notifier))) {
            Ok(terminal) => {
                unsafe { *out_terminal = Box::into_raw(Box::new(terminal)) };
                TermyFfiStatus::Ok
            }
            Err(status) => status,
        }
    })
}

/// The host's panes as a JSON array of objects with `id`, `child_pid`,
/// `title`, `working_directory`, `exited` and `shell`. Free with
/// `termy_buffer_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_multiplexer_list_json(
    multiplexer: *const TermyFfiMultiplexer,
    out_json: *mut TermyFfiBytes,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if multiplexer.is_null() || out_json.is_null() {
            return TermyFfiStatus::Null;
        }
        let panes = match unsafe { (*multiplexer).client.list() } {
            Ok(panes) => panes,
            Err(error) => return status_for(&error),
        };
        match serde_json::to_vec(&panes) {
            Ok(json) => {
                unsafe { *out_json = ffi_bytes_from_vec(json) };
                TermyFfiStatus::Ok
            }
            Err(_) => TermyFfiStatus::SerializeFailed,
        }
    })
}

/// End a pane's program and remove the pane. Detaching only, so the program
/// keeps running, is freeing the terminal handle instead.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_multiplexer_close(
    multiplexer: *const TermyFfiMultiplexer,
    pane_id_ptr: *const u8,
    pane_id_len: usize,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if multiplexer.is_null() {
            return TermyFfiStatus::Null;
        }
        let id = match unsafe { required_utf8(pane_id_ptr, pane_id_len) } {
            Ok(id) => id,
            Err(status) => return status,
        };
        match unsafe { (*multiplexer).client.close(id) } {
            Ok(()) => TermyFfiStatus::Ok,
            Err(error) => status_for(&error),
        }
    })
}

/// End every pane and stop the host.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_multiplexer_shutdown(
    multiplexer: *const TermyFfiMultiplexer,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if multiplexer.is_null() {
            return TermyFfiStatus::Null;
        }
        match unsafe { (*multiplexer).client.shutdown() } {
            Ok(()) => TermyFfiStatus::Ok,
            Err(error) => status_for(&error),
        }
    })
}

// ---------------------------------------------------------------------------
// Shared layout
// ---------------------------------------------------------------------------
//
// The host owns the window, workspace, tab and split layout that Termy and
// `termy mux` share. Embedders read and write it as a plain tree of pane IDs
// (`AppLayout`), and libtermy keeps the stored form (cell rectangles and the
// persisted split tree) in step, so `termy mux split`, `tab`, `window` and
// `layout` work on an embedder's panes too.

use crate::session_model::{
    NativePaneLayoutNode, NativePaneRect, PaneResizeAxis, StoredMultiplexer, StoredPane,
    StoredSession, StoredTab, StoredWindow, StoredWorkspace,
};
use std::collections::BTreeMap;

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
struct AppLayout {
    #[serde(default)]
    revision: u64,
    windows: Vec<AppWindow>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
struct AppWindow {
    id: String,
    #[serde(default)]
    active_workspace: usize,
    workspaces: Vec<AppWorkspace>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
struct AppWorkspace {
    name: String,
    #[serde(default)]
    pinned: bool,
    #[serde(default)]
    active_tab: usize,
    tabs: Vec<AppTab>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
struct AppTab {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    pinned: bool,
    #[serde(default)]
    zoomed: bool,
    /// The focused pane's ID.
    #[serde(default)]
    active_pane: Option<String>,
    /// Font zoom steps of panes that are zoomed.
    #[serde(default)]
    zoom: BTreeMap<String, i16>,
    /// The tab's size in cells, which the stored pane rectangles divide.
    #[serde(default)]
    cols: Option<u16>,
    #[serde(default)]
    rows: Option<u16>,
    tree: AppNode,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq, Clone)]
#[serde(untagged)]
enum AppNode {
    Leaf {
        leaf: String,
    },
    Split {
        axis: PaneResizeAxis,
        ratio: f32,
        first: Box<AppNode>,
        second: Box<AppNode>,
    },
}

impl AppNode {
    fn from_native(node: &NativePaneLayoutNode) -> Self {
        match node {
            NativePaneLayoutNode::Leaf { pane_id } => Self::Leaf {
                leaf: pane_id.clone(),
            },
            NativePaneLayoutNode::Split {
                axis,
                ratio,
                first,
                second,
            } => Self::Split {
                axis: *axis,
                ratio: *ratio,
                first: Box::new(Self::from_native(first)),
                second: Box::new(Self::from_native(second)),
            },
        }
    }

    fn to_native(&self) -> NativePaneLayoutNode {
        match self {
            Self::Leaf { leaf } => NativePaneLayoutNode::Leaf {
                pane_id: leaf.clone(),
            },
            Self::Split {
                axis,
                ratio,
                first,
                second,
            } => NativePaneLayoutNode::Split {
                axis: *axis,
                ratio: ratio.clamp(0.05, 0.95),
                first: Box::new(first.to_native()),
                second: Box::new(second.to_native()),
            },
        }
    }

    fn leaves(&self, into: &mut Vec<String>) {
        match self {
            Self::Leaf { leaf } => into.push(leaf.clone()),
            Self::Split { first, second, .. } => {
                first.leaves(into);
                second.leaves(into);
            }
        }
    }
}

impl AppTab {
    /// The tab as an embedder sees it, or `None` for a tab without live panes.
    fn from_stored(tab: &StoredTab) -> Option<Self> {
        let ids: Vec<String> = tab
            .panes
            .iter()
            .filter_map(|pane| pane.session_id.clone())
            .collect();
        let tree = match tab.layout_tree() {
            Ok(tree) => AppNode::from_native(&tree),
            // A tree that cannot be read: line the panes up side by side.
            Err(_) => ids.iter().skip(1).fold(
                AppNode::Leaf {
                    leaf: ids.first()?.clone(),
                },
                |tree, id| AppNode::Split {
                    axis: PaneResizeAxis::Horizontal,
                    ratio: 0.5,
                    first: Box::new(tree),
                    second: Box::new(AppNode::Leaf { leaf: id.clone() }),
                },
            ),
        };
        let bounds = tab.layout_bounds();
        Some(Self {
            title: tab.manual_title.clone(),
            pinned: tab.pinned,
            zoomed: tab.zoomed,
            active_pane: tab
                .panes
                .get(tab.active_pane)
                .and_then(|pane| pane.session_id.clone()),
            zoom: tab
                .panes
                .iter()
                .filter(|pane| pane.zoom_steps != 0)
                .filter_map(|pane| Some((pane.session_id.clone()?, pane.zoom_steps)))
                .collect(),
            cols: Some(bounds.width),
            rows: Some(bounds.height),
            tree,
        })
    }

    /// The stored form, keeping what embedders don't edit (presentation,
    /// buffers) from `previous`, the same tab as stored before.
    fn to_stored(&self, previous: Option<&StoredTab>) -> Result<StoredTab, String> {
        let mut ids = Vec::new();
        self.tree.leaves(&mut ids);
        if ids.is_empty() {
            return Err("Tab has no panes".into());
        }
        let previous_bounds = previous.map(StoredTab::layout_bounds);
        let minimum = (ids.len() as u16).saturating_mul(4).max(20);
        let bounds = NativePaneRect {
            left: 0,
            top: 0,
            width: self
                .cols
                .or(previous_bounds.map(|b| b.width))
                .unwrap_or(160)
                .max(minimum),
            height: self
                .rows
                .or(previous_bounds.map(|b| b.height))
                .unwrap_or(48)
                .max(minimum / 2),
        };
        let mut tab = StoredTab {
            zoomed: self.zoomed,
            presentation: previous.and_then(|tab| tab.presentation.clone()),
            pinned: self.pinned,
            manual_title: self.title.clone(),
            active_pane: self
                .active_pane
                .as_ref()
                .and_then(|active| ids.iter().position(|id| id == active))
                .unwrap_or(0),
            layout_tree_json: None,
            panes: ids
                .iter()
                .map(|id| StoredPane {
                    session_id: Some(id.clone()),
                    left: 0,
                    top: 0,
                    width: 1,
                    height: 1,
                    buffer: previous
                        .and_then(|tab| {
                            tab.panes
                                .iter()
                                .find(|pane| pane.session_id.as_ref() == Some(id))
                        })
                        .and_then(|pane| pane.buffer.clone()),
                    zoom_steps: self.zoom.get(id).copied().unwrap_or(0),
                })
                .collect(),
        };
        tab.apply_layout_tree(&self.tree.to_native(), bounds)?;
        Ok(tab)
    }
}

impl AppLayout {
    fn from_stored(layout: &StoredMultiplexer, revision: u64) -> Self {
        Self {
            revision,
            windows: layout
                .windows
                .iter()
                .map(|window| AppWindow {
                    id: window.id.clone(),
                    active_workspace: window.session.active_workspace,
                    workspaces: window
                        .session
                        .workspaces
                        .iter()
                        .map(|workspace| AppWorkspace {
                            name: workspace.name.clone(),
                            pinned: workspace.pinned,
                            active_tab: workspace.active_tab,
                            tabs: workspace
                                .tabs
                                .iter()
                                .filter_map(AppTab::from_stored)
                                .collect(),
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    fn to_stored(&self, previous: &StoredMultiplexer) -> Result<StoredMultiplexer, String> {
        // Tabs as stored before, by any of their panes.
        let mut stored_tabs: BTreeMap<&str, &StoredTab> = BTreeMap::new();
        let mut stored_windows: BTreeMap<&str, &StoredWindow> = BTreeMap::new();
        for window in &previous.windows {
            stored_windows.insert(window.id.as_str(), window);
            for workspace in &window.session.workspaces {
                for tab in &workspace.tabs {
                    for id in tab
                        .panes
                        .iter()
                        .filter_map(|pane| pane.session_id.as_deref())
                    {
                        stored_tabs.insert(id, tab);
                    }
                }
            }
        }
        let windows = self
            .windows
            .iter()
            .map(|window| {
                let workspaces = window
                    .workspaces
                    .iter()
                    .map(|workspace| {
                        let tabs = workspace
                            .tabs
                            .iter()
                            .map(|tab| {
                                let mut ids = Vec::new();
                                tab.tree.leaves(&mut ids);
                                let previous = ids
                                    .iter()
                                    .find_map(|id| stored_tabs.get(id.as_str()).copied());
                                tab.to_stored(previous)
                            })
                            .collect::<Result<Vec<_>, String>>()?;
                        Ok(StoredWorkspace {
                            name: workspace.name.clone(),
                            pinned: workspace.pinned,
                            active_tab: workspace.active_tab.min(tabs.len().saturating_sub(1)),
                            tabs,
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                Ok(StoredWindow {
                    id: window.id.clone(),
                    session: StoredSession {
                        active_workspace: window
                            .active_workspace
                            .min(workspaces.len().saturating_sub(1)),
                        workspaces,
                        zoom_steps: stored_windows
                            .get(window.id.as_str())
                            .map_or(0, |stored| stored.session.zoom_steps),
                    },
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(StoredMultiplexer {
            version: previous.version,
            windows,
        })
    }
}

/// The shared layout as JSON: `revision`, then `windows`, each with `id`,
/// `active_workspace` and `workspaces` of `name`, `pinned`, `active_tab` and
/// `tabs`. A tab has `title`, `pinned`, `zoomed`, `active_pane`, `zoom` (pane
/// ID to zoom steps), its size as `cols` and `rows`, and a split `tree`: a
/// leaf is `{"leaf": "<pane ID>"}`, a split `{"axis": "horizontal" (side by
/// side) or "vertical", "ratio", "first", "second"}`. Free with
/// `termy_buffer_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_multiplexer_layout_json(
    multiplexer: *const TermyFfiMultiplexer,
    out_json: *mut TermyFfiBytes,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if multiplexer.is_null() || out_json.is_null() {
            return TermyFfiStatus::Null;
        }
        let (revision, json) = match unsafe { (*multiplexer).client.layout_revision() } {
            Ok(layout) => layout,
            Err(error) => return status_for(&error),
        };
        let Ok(stored) = StoredMultiplexer::parse(json.as_deref()) else {
            return TermyFfiStatus::WriteFailed;
        };
        match serde_json::to_vec(&AppLayout::from_stored(&stored, revision)) {
            Ok(json) => {
                unsafe { *out_json = ffi_bytes_from_vec(json) };
                TermyFfiStatus::Ok
            }
            Err(_) => TermyFfiStatus::SerializeFailed,
        }
    })
}

/// Replaces the shared layout with `json` (as `termy_multiplexer_layout_json`
/// writes it) if it is still at `expected_revision`. `out_updated` says
/// whether it was written; `out_revision` is the layout's revision after the
/// call, the new one or the newer one another client wrote, which the caller
/// should read and apply before writing again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_multiplexer_set_layout_json(
    multiplexer: *const TermyFfiMultiplexer,
    json_ptr: *const u8,
    json_len: usize,
    expected_revision: u64,
    out_revision: *mut u64,
    out_updated: *mut bool,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if multiplexer.is_null() || out_revision.is_null() || out_updated.is_null() {
            return TermyFfiStatus::Null;
        }
        let json = match unsafe { required_utf8(json_ptr, json_len) } {
            Ok(json) => json,
            Err(status) => return status,
        };
        let Ok(app) = serde_json::from_str::<AppLayout>(json) else {
            return TermyFfiStatus::InvalidArgument;
        };
        let client = unsafe { &(*multiplexer).client };
        let (revision, current) = match client.layout_revision() {
            Ok(layout) => layout,
            Err(error) => return status_for(&error),
        };
        unsafe {
            *out_updated = false;
            *out_revision = revision;
        }
        if revision != expected_revision {
            return TermyFfiStatus::Ok;
        }
        let Ok(previous) = StoredMultiplexer::parse(current.as_deref()) else {
            return TermyFfiStatus::WriteFailed;
        };
        let stored = match app.to_stored(&previous) {
            Ok(stored) => stored,
            Err(error) => {
                log::warn!("rejected app layout: {error}");
                return TermyFfiStatus::InvalidArgument;
            }
        };
        if stored == previous {
            unsafe { *out_updated = true };
            return TermyFfiStatus::Ok;
        }
        let Ok(replacement) = serde_json::to_string(&stored) else {
            return TermyFfiStatus::SerializeFailed;
        };
        match client.compare_and_set_layout(expected_revision, replacement) {
            Ok(Some(revision)) => {
                unsafe {
                    *out_updated = true;
                    *out_revision = revision;
                }
                TermyFfiStatus::Ok
            }
            Ok(None) => {
                if let Ok((revision, _)) = client.layout_revision() {
                    unsafe { *out_revision = revision };
                }
                TermyFfiStatus::Ok
            }
            Err(error) => status_for(&error),
        }
    })
}

/// Called on a background thread with each new layout revision, the current
/// one first; `u64::MAX` when the watch ended because the host went away.
pub type TermyFfiLayoutChangedCallback = unsafe extern "C" fn(context: *mut c_void, revision: u64);

/// A running layout watch; free it to stop.
pub struct TermyFfiLayoutWatch {
    close: Box<dyn Fn() + Send + Sync>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// Calls `callback` with `context` whenever the shared layout changes, from
/// a thread of its own, until the watch is freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_multiplexer_watch_layout(
    multiplexer: *const TermyFfiMultiplexer,
    callback: Option<TermyFfiLayoutChangedCallback>,
    context: TermyFfiUserData,
    out_watch: *mut *mut TermyFfiLayoutWatch,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        let Some(callback) = callback else {
            return TermyFfiStatus::Null;
        };
        if multiplexer.is_null() || out_watch.is_null() {
            return TermyFfiStatus::Null;
        }
        let mut watch = match unsafe { (*multiplexer).client.watch_layout() } {
            Ok(watch) => watch,
            Err(error) => return status_for(&error),
        };
        let close = match watch.closer() {
            Ok(close) => close,
            Err(error) => return status_for(&error),
        };
        // The pointer crosses to the watch thread; the caller keeps it valid
        // until the watch is freed.
        let context = context as usize;
        let thread = std::thread::Builder::new()
            .name("termy-layout-watch".into())
            .spawn(move || {
                while let Ok(revision) = watch.recv() {
                    unsafe { callback(context as *mut c_void, revision) };
                }
                unsafe { callback(context as *mut c_void, u64::MAX) };
            });
        let Ok(thread) = thread else {
            return TermyFfiStatus::WriteFailed;
        };
        let watch = TermyFfiLayoutWatch {
            close: Box::new(close),
            thread: Some(thread),
        };
        unsafe { *out_watch = Box::into_raw(Box::new(watch)) };
        TermyFfiStatus::Ok
    })
}

/// Stops a layout watch and waits for its thread; the callback is not
/// called after this returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_layout_watch_free(
    watch: *mut TermyFfiLayoutWatch,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if watch.is_null() {
            return TermyFfiStatus::Null;
        }
        let mut watch = unsafe { Box::from_raw(watch) };
        (watch.close)();
        if let Some(thread) = watch.thread.take() {
            let _ = thread.join();
        }
        TermyFfiStatus::Ok
    })
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    fn tab(tree: AppNode) -> AppTab {
        AppTab {
            title: Some("build".into()),
            pinned: false,
            zoomed: false,
            active_pane: Some("b".into()),
            zoom: BTreeMap::from([("b".into(), 2)]),
            cols: Some(120),
            rows: Some(40),
            tree,
        }
    }

    #[test]
    fn app_layout_round_trips_through_the_stored_form() {
        let tree = AppNode::Split {
            axis: PaneResizeAxis::Horizontal,
            ratio: 0.5,
            first: Box::new(AppNode::Leaf { leaf: "a".into() }),
            second: Box::new(AppNode::Split {
                axis: PaneResizeAxis::Vertical,
                ratio: 0.5,
                first: Box::new(AppNode::Leaf { leaf: "b".into() }),
                second: Box::new(AppNode::Leaf { leaf: "c".into() }),
            }),
        };
        let app = AppLayout {
            revision: 0,
            windows: vec![AppWindow {
                id: "w1".into(),
                active_workspace: 0,
                workspaces: vec![AppWorkspace {
                    name: "keen-river".into(),
                    pinned: false,
                    active_tab: 0,
                    tabs: vec![tab(tree.clone())],
                }],
            }],
        };
        let stored = app.to_stored(&StoredMultiplexer::empty()).expect("stored");
        let stored_tab = &stored.windows[0].session.workspaces[0].tabs[0];
        assert_eq!(stored_tab.panes.len(), 3);
        assert_eq!(stored_tab.active_pane, 1);
        assert_eq!(stored_tab.panes[1].zoom_steps, 2);
        assert_eq!(stored_tab.panes[0].width + stored_tab.panes[1].width, 120);
        // The stored form decodes the way the host and `termy mux` read it.
        let json = serde_json::to_string(&stored).unwrap();
        let parsed = StoredMultiplexer::parse(Some(&json)).expect("host accepts it");
        let back = AppLayout::from_stored(&parsed, 7);
        assert_eq!(back.revision, 7);
        assert_eq!(back.windows[0].workspaces[0].tabs[0].tree, tree);
        assert_eq!(
            back.windows[0].workspaces[0].tabs[0].title.as_deref(),
            Some("build")
        );
        assert_eq!(
            back.windows[0].workspaces[0].tabs[0].active_pane.as_deref(),
            Some("b")
        );
    }

    #[test]
    fn app_layout_json_uses_the_documented_shape() {
        let json = r#"{"windows":[{"id":"w","workspaces":[{"name":"n","tabs":[
            {"tree":{"axis":"vertical","ratio":0.25,"first":{"leaf":"x"},"second":{"leaf":"y"}}}]}]}]}"#;
        let app: AppLayout = serde_json::from_str(json).expect("decodes");
        let stored = app.to_stored(&StoredMultiplexer::empty()).expect("stored");
        let panes = &stored.windows[0].session.workspaces[0].tabs[0].panes;
        assert!(panes[0].height < panes[1].height);
    }
}
