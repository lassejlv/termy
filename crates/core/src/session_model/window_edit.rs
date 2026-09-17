use crate::session_model::{StoredSession, StoredWorkspace, WorkspaceEdit};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum WindowEdit {
    CreateWorkspace { name: String },
    SelectWorkspace { index: usize },
    MoveWorkspace { from: usize, to: usize },
    DeleteEmptyWorkspace { index: usize },
}

impl WindowEdit {
    pub fn apply(&self, session: &mut StoredSession) -> Result<(), &'static str> {
        match self {
            Self::CreateWorkspace { name } => {
                let mut workspace = StoredWorkspace {
                    name: String::new(),
                    pinned: false,
                    active_tab: 0,
                    tabs: Vec::new(),
                };
                WorkspaceEdit::Rename { name: name.clone() }.apply(&mut workspace)?;
                session.workspaces.push(workspace);
                session.active_workspace = session.workspaces.len() - 1;
            }
            Self::SelectWorkspace { index } => {
                if *index >= session.workspaces.len() {
                    return Err("Workspace does not exist");
                }
                session.active_workspace = *index;
            }
            Self::MoveWorkspace { from, to } => {
                if *from >= session.workspaces.len() || *to >= session.workspaces.len() {
                    return Err("Workspace does not exist");
                }
                let workspace = session.workspaces.remove(*from);
                session.workspaces.insert(*to, workspace);
                session.active_workspace = moved_index(session.active_workspace, *from, *to);
            }
            Self::DeleteEmptyWorkspace { index } => {
                let workspace = session
                    .workspaces
                    .get(*index)
                    .ok_or("Workspace does not exist")?;
                if !workspace.tabs.is_empty() {
                    return Err("Workspace still has tabs; close its panes first");
                }
                if session.workspaces.len() == 1 {
                    return Err("Cannot delete the last workspace");
                }
                session.workspaces.remove(*index);
                if session.active_workspace > *index {
                    session.active_workspace -= 1;
                }
                session.active_workspace =
                    session.active_workspace.min(session.workspaces.len() - 1);
            }
        }
        Ok(())
    }
}

pub(crate) fn moved_index(active: usize, from: usize, to: usize) -> usize {
    if active == from {
        to
    } else if from < active && active <= to {
        active - 1
    } else if to <= active && active < from {
        active + 1
    } else {
        active
    }
}
