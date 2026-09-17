//! Persistent workspace, tab, and pane structures shared by desktop and CLI.
use serde::{Deserialize, Serialize};
mod merge;
mod tab_edit;
mod tab_layout;
mod window_edit;
pub use merge::merge_session;
pub use tab_edit::TabEdit;
pub use window_edit::WindowEdit;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceTarget {
    pub window_id: String,
    pub index: usize,
    pub expected: StoredWorkspace,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum WorkspaceEdit {
    Rename { name: String },
    SetPinned { pinned: bool },
    SelectTab { index: usize },
    MoveTab { from: usize, to: usize },
}

impl WorkspaceEdit {
    pub fn apply(&self, workspace: &mut StoredWorkspace) -> Result<(), &'static str> {
        match self {
            Self::Rename { name } => {
                if name.trim().is_empty() || name.len() > 256 || name.chars().any(char::is_control)
                {
                    return Err(
                        "Workspace name must contain 1 to 256 bytes without control characters",
                    );
                }
                workspace.name = name.trim().to_owned();
            }
            Self::SetPinned { pinned } => workspace.pinned = *pinned,
            Self::SelectTab { index } => {
                if *index >= workspace.tabs.len() {
                    return Err("Tab does not exist");
                }
                workspace.active_tab = *index;
            }
            Self::MoveTab { from, to } => {
                if *from >= workspace.tabs.len() || *to >= workspace.tabs.len() {
                    return Err("Tab does not exist");
                }
                let tab = workspace.tabs.remove(*from);
                workspace.tabs.insert(*to, tab);
                workspace.active_tab = window_edit::moved_index(workspace.active_tab, *from, *to);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredPane {
    /// Live multiplexer layout only; not stored in the restart-snapshot database.
    #[serde(default)]
    pub session_id: Option<String>,
    pub left: u16,
    pub top: u16,
    pub width: u16,
    pub height: u16,
    pub buffer: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredTab {
    #[serde(default)]
    pub zoomed: bool,
    #[serde(default)]
    pub presentation: Option<StoredTabPresentation>,
    pub pinned: bool,
    pub manual_title: Option<String>,
    pub active_pane: usize,
    /// Pane split layout tree, serialized as JSON in the same shape the
    /// legacy file format used.
    pub layout_tree_json: Option<String>,
    pub panes: Vec<StoredPane>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredWorkspace {
    pub name: String,
    pub pinned: bool,
    pub active_tab: usize,
    pub tabs: Vec<StoredTab>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredSession {
    pub workspaces: Vec<StoredWorkspace>,
    pub active_workspace: usize,
}

/// UI title/status snapshot for live sessions; terminal events refresh it on attach.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredTabPresentation {
    pub title: String,
    pub explicit_title: Option<String>,
    pub explicit_title_is_prediction: bool,
    pub shell_title: Option<String>,
    pub current_command: Option<String>,
    pub last_prompt_cwd: Option<String>,
    pub running_process: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredWindow {
    pub id: String,
    pub session: StoredSession,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredMultiplexer {
    pub version: u32,
    pub windows: Vec<StoredWindow>,
}

mod layout;
mod layout_edit;
pub use layout::{
    NativeLayout, NativePaneLayoutNode, NativePaneLayoutTree, NativePaneRect, PaneResizeAxis,
    PaneResizeEdge, PaneResizeResult,
};

mod layout_persistence;
pub use layout_persistence::PersistedNativeLayoutNode;

mod layout_infer;
pub use layout_infer::NativePaneInfo;

pub use layout::{NATIVE_PANE_MIN_COLS, NATIVE_PANE_MIN_ROWS};
