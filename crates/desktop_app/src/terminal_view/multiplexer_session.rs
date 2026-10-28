use super::*;

#[cfg(all(test, unix))]
mod tests;

impl Terminal {
    pub(super) fn adopt_session(&self) {
        if let Self::Native(native) = self
            && let Some(session) = &native.session
        {
            session.adopt();
        }
    }
}

impl TerminalView {
    pub(super) fn multiplexer_client(&self) -> Option<&termy_core::multiplexer::SessionClient> {
        self.multiplexer.as_ref().map(|window| window.client())
    }

    /// The multiplexer never writes the SQLite session store, so the session
    /// there is the one local mode saved last. A host without a layout imports
    /// it once (the first start with the multiplexer on). After that a host
    /// that starts empty, for example after a reboot, opens a fresh window
    /// instead of reviving tabs from before the multiplexer.
    pub(super) fn startup_session_for_multiplexer_import(
        mut startup: super::persistence::StartupNativeSession,
    ) -> super::persistence::StartupNativeSession {
        match startup.store.imported_into_multiplexer() {
            Ok(false) => {}
            Ok(true) => startup.session = None,
            Err(error) => {
                log::error!("Failed to read the session import state: {error}");
                startup.session = None;
            }
        }
        startup
    }

    /// Turning the multiplexer off does not stop its host: the shells in it
    /// keep running in the background until the host is shut down. Say so
    /// only when a host runs (`host_running`).
    pub(super) fn multiplexer_setting_toast(enabled: bool, host_running: bool) -> &'static str {
        if enabled || !host_running {
            "Built-in multiplexer setting saved. Restart Termy to apply it."
        } else {
            "Built-in multiplexer turned off. Restart Termy to apply it. Its background \
             sessions keep running until you run `termy mux shutdown`."
        }
    }

    pub(super) fn multiplexer_host_is_running() -> bool {
        termy_core::multiplexer::default_root()
            .is_some_and(|root| termy_core::multiplexer::host_is_running(&root))
    }

    /// `true` while the SQLite session store still counts as not imported
    /// into the host in this app run, and a window like this one would import
    /// it at start-up (see `load_startup_native_session`).
    pub(super) fn multiplexer_import_pending(&self) -> bool {
        self.multiplexer
            .as_ref()
            .is_some_and(|window| !window.import_recorded())
            && self.owns_persisted_session
            && (self.native_tab_persistence || self.workspace_sidebar_enabled)
    }

    /// Mark the session store as imported once the host holds the session:
    /// after this window imported it, or when the host already had a layout.
    /// Users who turned the multiplexer on before this marker existed get it
    /// here, so a host restart does not import their old local tabs again.
    /// A successful host save records it too (see the persistence writer).
    pub(super) fn record_multiplexer_import(&self) {
        if !self.multiplexer_import_pending() {
            return;
        }
        let Some(store) = self.workspace_store() else {
            return;
        };
        match store.mark_imported_into_multiplexer() {
            Ok(()) => {
                if let Some(window) = &self.multiplexer {
                    window.set_import_recorded();
                }
            }
            Err(error) => log::error!("Failed to record the session import: {error}"),
        }
    }

    /// Host terminals this window owns: live panes, including stashed
    /// workspaces and zoom-parked splits, then panes of workspaces that
    /// were never opened since the restore (they have no terminal yet).
    pub(super) fn multiplexer_window_session_ids(&self) -> Vec<String> {
        let mut ids = Vec::new();
        self.for_each_live_pane(|_, pane| {
            ids.extend(pane.terminal.session_id().map(str::to_owned));
        });
        ids.extend(
            self.session
                .workspaces
                .iter()
                .filter_map(|workspace| workspace.pending_restore.as_ref())
                .flat_map(|workspace| &workspace.tabs)
                .flat_map(|tab| &tab.panes)
                .filter_map(|pane| pane.session_id.clone()),
        );
        ids
    }

    /// "Terminate" when closing a window: end its terminals and remove it
    /// from the host layout, so nothing is left to reopen. Live panes close
    /// when the window drops them; cold ones are closed here.
    pub(super) fn prepare_multiplexer_terminate(&self) {
        let Some(window) = &self.multiplexer else {
            return;
        };
        let mut live = HashSet::new();
        self.for_each_live_pane(|_, pane| {
            pane.terminal.adopt_session();
            live.extend(pane.terminal.session_id().map(str::to_owned));
        });
        let cold = self
            .multiplexer_window_session_ids()
            .into_iter()
            .filter(|id| !live.contains(id))
            .collect::<Vec<_>>();
        if let Err(error) = window.forget() {
            log::error!("Failed to remove the closed window from the session layout: {error}");
        }
        for id in cold {
            if let Err(error) = window.client().close(&id) {
                log::error!("Failed to close multiplexer pane: {error}");
            }
        }
    }

    /// The window closes because nothing is left in it: its last shell
    /// exited (audit B2), or its last tab was closed or moved to another
    /// window. Ends any host panes it still shows and removes the window from
    /// the layout instead of saving it: a saved dead pane would be reattached
    /// on the next launch, replay its `Exit` and close the window again.
    /// Returns false, leaving the window to be saved and detached, outside
    /// multiplexer mode or while any of its panes still runs.
    pub(super) fn discard_multiplexer_window(&self) -> bool {
        let Some(window) = &self.multiplexer else {
            return false;
        };
        let ids = self.multiplexer_window_session_ids();
        let Ok(panes) = window.client().list() else {
            return false;
        };
        let running = ids
            .iter()
            .any(|id| panes.iter().any(|pane| &pane.id == id && !pane.exited));
        if running {
            return false;
        }
        for id in ids {
            if let Err(error) = window.client().close(&id) {
                log::error!("Failed to close exited multiplexer pane: {error:#}");
            }
        }
        if let Err(error) = window.forget() {
            log::error!("Failed to remove the closed window from the session layout: {error}");
        }
        true
    }

    /// A window owns the presentation; the host owns the running terminals.
    /// Explicit tab/pane deletion leaves the pane cleanup armed. Window/app
    /// removal first saves the layout, then detaches every owned terminal.
    pub(super) fn prepare_multiplexer_detach(&self) {
        let Some(window) = &self.multiplexer else {
            return;
        };
        if window.is_released() {
            return;
        }
        self.sync_persisted_native_workspace();
        self.for_each_live_pane(|_, pane| pane.terminal.detach_session());
        window.release();
    }
}

/// Tests without a session host, so they run on every platform.
#[cfg(test)]
mod import_tests {
    use super::*;
    use crate::workspace_store::{StoredSession, WorkspaceStore};

    pub(super) fn local_session() -> StoredSession {
        StoredSession {
            active_workspace: 0,
            zoom_steps: 0,
            workspaces: vec![termy_core::session_model::StoredWorkspace {
                name: "Local".into(),
                pinned: false,
                active_tab: 0,
                tabs: Vec::new(),
            }],
        }
    }

    #[test]
    fn a_host_without_a_layout_imports_the_local_session_only_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(WorkspaceStore::open(&dir.path().join("workspaces.db")).unwrap());
        let session = local_session();
        let startup = |store: &Arc<WorkspaceStore>| {
            TerminalView::startup_session_for_multiplexer_import(
                super::super::persistence::StartupNativeSession {
                    store: Arc::clone(store),
                    session: Some(session.clone()),
                },
            )
            .session
        };
        store.save_session(&session).unwrap();
        assert_eq!(startup(&store), Some(session.clone()));
        store.mark_imported_into_multiplexer().unwrap();
        assert_eq!(startup(&store), None, "a restarted host must not revive it");
        // Switching back to regular mode saves a newer session, which the
        // next switch to the multiplexer imports once more.
        store.save_session(&session).unwrap();
        assert_eq!(startup(&store), Some(session.clone()));
    }

    #[test]
    fn turning_the_multiplexer_off_mentions_a_running_host_only() {
        let off = TerminalView::multiplexer_setting_toast(false, true);
        assert!(off.contains("keep running") && off.contains("termy mux shutdown"));
        assert!(!TerminalView::multiplexer_setting_toast(false, false).contains("shutdown"));
        assert!(!TerminalView::multiplexer_setting_toast(true, true).contains("shutdown"));
    }
}
