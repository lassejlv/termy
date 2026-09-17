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
        for pane in self
            .session
            .tabs
            .iter()
            .chain(
                self.session
                    .workspaces
                    .iter()
                    .flat_map(|workspace| &workspace.tabs),
            )
            .flat_map(|tab| &tab.panes)
            .chain(
                self.session
                    .native_pane_zoom_snapshots
                    .values()
                    .flat_map(|zoom| &zoom.other_panes),
            )
        {
            pane.terminal.detach_session();
        }
        window.release();
    }
}
