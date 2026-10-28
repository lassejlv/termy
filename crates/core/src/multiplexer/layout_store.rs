//! The host's copy of the desktop session layout (audit I4). The host owns
//! it: every accepted write is validated, bumps a revision and wakes layout
//! watchers, and panes the host closes are pruned from it.
use crate::session_model::StoredMultiplexer;
use anyhow::{Context, ensure};
use std::sync::{
    Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
};

pub(crate) const MAX_LAYOUT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Default)]
struct State {
    json: Option<String>,
    /// 0 while no layout was ever stored; every accepted write adds one.
    revision: u64,
}

#[derive(Default)]
pub(crate) struct LayoutStore {
    state: Mutex<State>,
    changed: Condvar,
}

impl LayoutStore {
    pub(crate) fn get(&self) -> (u64, Option<String>) {
        let state = self.state.lock().unwrap();
        (state.revision, state.json.clone())
    }

    pub(crate) fn revision(&self) -> u64 {
        self.state.lock().unwrap().revision
    }

    /// Replaces the layout unconditionally and returns the new revision.
    pub(crate) fn set(&self, json: String) -> anyhow::Result<u64> {
        validate(&json)?;
        let mut state = self.state.lock().unwrap();
        Ok(self.store(&mut state, json))
    }

    /// Replaces the layout only if it is still at revision `expected`.
    /// Returns whether it did, and the revision the layout is at now.
    pub(crate) fn compare_and_set(
        &self,
        expected: u64,
        json: String,
    ) -> anyhow::Result<(bool, u64)> {
        validate(&json)?;
        let mut state = self.state.lock().unwrap();
        if state.revision != expected {
            return Ok((false, state.revision));
        }
        Ok((true, self.store(&mut state, json)))
    }

    /// Removes a pane the host closed or that exited, collapsing its tab's
    /// split tree and dropping the tab once empty (workspaces stay).
    pub(crate) fn remove_pane(&self, id: &str) {
        let mut state = self.state.lock().unwrap();
        let mut layout = match StoredMultiplexer::parse(state.json.as_deref()) {
            Ok(layout) => layout,
            Err(error) => {
                log::error!("Cannot prune pane {id} from the session layout: {error}");
                return;
            }
        };
        if !layout.remove_pane(id) {
            return;
        }
        match serde_json::to_string(&layout) {
            Ok(json) => {
                self.store(&mut state, json);
            }
            Err(error) => log::error!("Cannot prune pane {id} from the session layout: {error}"),
        }
    }

    fn store(&self, state: &mut State, json: String) -> u64 {
        state.json = Some(json);
        state.revision += 1;
        self.changed.notify_all();
        state.revision
    }

    /// Blocks until the layout moves past revision `seen` and returns the new
    /// revision, or `None` once `closed` is set (see [`Self::wake_watchers`]).
    pub(crate) fn wait_for_change(&self, seen: u64, closed: &AtomicBool) -> Option<u64> {
        let state = self.state.lock().unwrap();
        let state = self
            .changed
            .wait_while(state, |state| {
                state.revision == seen && !closed.load(Ordering::Acquire)
            })
            .unwrap();
        (!closed.load(Ordering::Acquire)).then_some(state.revision)
    }

    pub(crate) fn wake_watchers(&self) {
        // Holding the lock orders this wakeup after a watcher's flag check.
        let _state = self.state.lock().unwrap();
        self.changed.notify_all();
    }
}

/// The host accepts only layouts this build can read back.
fn validate(json: &str) -> anyhow::Result<()> {
    ensure!(json.len() <= MAX_LAYOUT_BYTES, "session layout too large");
    StoredMultiplexer::parse(Some(json))
        .map_err(anyhow::Error::msg)
        .context("session layout rejected")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_model::*;
    use std::sync::Arc;

    fn layout(tabs: &[&[&str]]) -> String {
        let tabs = tabs
            .iter()
            .map(|ids| StoredTab {
                zoomed: false,
                presentation: None,
                pinned: false,
                manual_title: None,
                active_pane: 0,
                layout_tree_json: None,
                panes: ids
                    .iter()
                    .enumerate()
                    .map(|(index, id)| StoredPane {
                        session_id: Some((*id).into()),
                        left: index as u16 * 40,
                        top: 0,
                        width: 40,
                        height: 24,
                        buffer: None,
                        zoom_steps: 0,
                    })
                    .collect(),
            })
            .collect();
        serde_json::to_string(&StoredMultiplexer {
            version: STORED_MULTIPLEXER_VERSION,
            windows: vec![StoredWindow {
                id: "window".into(),
                session: StoredSession {
                    workspaces: vec![StoredWorkspace {
                        name: "Work".into(),
                        pinned: false,
                        active_tab: 1,
                        tabs,
                    }],
                    active_workspace: 0,
                    zoom_steps: 0,
                },
            }],
        })
        .unwrap()
    }

    #[test]
    fn writes_bump_the_revision_and_compare_against_it() {
        let store = LayoutStore::default();
        assert_eq!(store.get(), (0, None));
        let first = layout(&[&["a"]]);
        assert_eq!(store.compare_and_set(0, first.clone()).unwrap(), (true, 1));
        assert_eq!(store.compare_and_set(0, layout(&[])).unwrap(), (false, 1));
        assert_eq!(store.get(), (1, Some(first)));
        assert_eq!(store.set(layout(&[])).unwrap(), 2);
    }

    #[test]
    fn invalid_layouts_are_rejected_without_a_new_revision() {
        let store = LayoutStore::default();
        for invalid in [
            "not json".to_owned(),
            r#"{"tabs":[]}"#.to_owned(),
            r#"{"version":2,"windows":[]}"#.to_owned(),
            format!(
                r#"{{"version":1,"windows":[],"padding":"{}"}}"#,
                "x".repeat(MAX_LAYOUT_BYTES)
            ),
        ] {
            assert!(store.set(invalid.clone()).is_err(), "{invalid:.40}");
            assert!(store.compare_and_set(0, invalid).is_err());
        }
        assert_eq!(store.get(), (0, None));
    }

    #[test]
    fn pruning_collapses_tabs_and_follows_the_active_tab() {
        let store = LayoutStore::default();
        store.set(layout(&[&["a"], &["b", "c"]])).unwrap();
        store.remove_pane("missing");
        assert_eq!(store.revision(), 1, "no change, no revision");
        store.remove_pane("a");
        store.remove_pane("c");
        let (revision, json) = store.get();
        assert_eq!(revision, 3);
        let pruned = StoredMultiplexer::parse(json.as_deref()).unwrap();
        let workspace = &pruned.windows[0].session.workspaces[0];
        assert_eq!(workspace.tabs.len(), 1);
        assert_eq!(workspace.active_tab, 0);
        assert_eq!(workspace.tabs[0].panes[0].width, 80);
        store.remove_pane("b");
        let pruned = StoredMultiplexer::parse(store.get().1.as_deref()).unwrap();
        assert!(pruned.windows[0].session.workspaces[0].tabs.is_empty());
    }

    #[test]
    fn watchers_wake_on_writes_and_on_close() {
        let store = Arc::new(LayoutStore::default());
        let closed = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let watcher = {
            let (store, closed) = (Arc::clone(&store), Arc::clone(&closed));
            std::thread::spawn(move || {
                let first = store.wait_for_change(0, &closed).unwrap();
                tx.send(first).unwrap();
                store.wait_for_change(first, &closed)
            })
        };
        store.set(layout(&[])).unwrap();
        assert_eq!(rx.recv().unwrap(), 1);
        closed.store(true, Ordering::Release);
        store.wake_watchers();
        assert_eq!(watcher.join().unwrap(), None);
    }
}
