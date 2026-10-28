//! The attached view's side of a pane: the state its subscription thread
//! publishes, and the [`RemoteTransport`] that the view's [`Terminal`] drives.

use super::*;

impl ClientState {
    pub(super) fn state(&self) -> Arc<RemoteState> {
        Arc::clone(&self.state.lock().unwrap())
    }

    pub(super) fn send_control(&self, request: &Request) -> anyhow::Result<()> {
        let stream = self.stream.lock().unwrap();
        let Some(stream) = stream.as_ref() else {
            bail!("terminal session closed");
        };
        write_message(&mut &*stream, request)
    }

    pub(super) fn publish_state(
        &self,
        state: Arc<RemoteState>,
        change: crate::remote::frame::FrameChange,
    ) {
        let mut claim = self.size_claim.lock().unwrap();
        claim.published(&state);
        let mut shown = self.state.lock().unwrap();
        // Recorded with the state, so a renderer always takes the damage
        // that leads to the state it reads.
        self.damage.lock().unwrap().record(change, &state);
        *shown = state;
        drop(shown);
        drop(claim);
        self.pending.lock().unwrap().wakeup = true;
        self.state_updated.notify_all();
    }

    pub(super) fn wait_for_generation(&self, generation: u64) -> anyhow::Result<()> {
        let (state, _) = self
            .state_updated
            .wait_timeout_while(
                self.state.lock().unwrap(),
                Duration::from_secs(5),
                |state| {
                    state.render.metadata.generation < generation
                        && !self.disconnected.load(Ordering::Acquire)
                },
            )
            .unwrap();
        ensure!(
            !self.disconnected.load(Ordering::Acquire),
            "terminal session disconnected"
        );
        ensure!(
            state.render.metadata.generation >= generation,
            "terminal viewport did not catch up"
        );
        Ok(())
    }

    pub(super) fn notify(&self) {
        if self.wakeup_enabled.load(Ordering::Acquire)
            && let Some(wakeup) = &self.wakeup
        {
            wakeup.notify();
        }
    }
    // Titles and program status drive tab chrome, which stays visible for
    // hidden tabs, so they wake the host even while render wakeups are suspended.
    pub(super) fn notify_tab_chrome(&self) {
        if let Some(wakeup) = &self.wakeup {
            wakeup.notify();
        }
    }

    pub(super) fn remember_setting(&self, command: &RemoteCommand) {
        if !replaces_queued(command) || matches!(command, RemoteCommand::NudgeResize) {
            return;
        }
        let kind = std::mem::discriminant(command);
        let mut settings = self.settings.lock().unwrap();
        settings.retain(|setting| std::mem::discriminant(setting) != kind);
        settings.push(command.clone());
    }

    /// A new subscription showed its first frame: the pane is back. Sends
    /// the view's size and options again, which the host may have missed.
    pub(super) fn resubscribed(&self, input: &InputQueue) {
        self.reconnected();
        let settings = self.settings.lock().unwrap().clone();
        for command in settings {
            if let RemoteCommand::Resize(size) = &command {
                let mut claim = self.size_claim.lock().unwrap();
                claim.requested(*size, &self.state.lock().unwrap());
            }
            input.push(Input {
                command,
                reply: None,
            });
        }
    }

    pub(super) fn input_dropped(&self) {
        let _ = self
            .pending
            .lock()
            .unwrap()
            .extend_events(vec![TerminalEvent::InputDropped]);
        self.notify();
    }

    /// Marks the stream lost, once per streak of failed attempts to
    /// re-subscribe, with a title on the tab that says so.
    pub(super) fn disconnect(&self, error: &anyhow::Error) {
        if self.disconnected.swap(true, Ordering::AcqRel) {
            return;
        }
        log::error!("terminal session connection lost: {error:#}");
        self.wake_waiters();
        let _ = self
            .pending
            .lock()
            .unwrap()
            .extend_events(vec![TerminalEvent::Title(
                "Session disconnected — reconnecting…".into(),
            )]);
        self.notify_tab_chrome();
    }

    /// The pane's host is gone, so the pane ended with it, though its
    /// program did not exit. The view keeps the tab, marked as ended
    /// (audit B12): it takes no input and does not reconnect.
    pub(super) fn host_lost(&self, error: &anyhow::Error) {
        log::warn!("terminal session ended with its host: {error:#}");
        self.disconnected.store(true, Ordering::Release);
        self.host_lost.store(true, Ordering::Release);
        self.wake_waiters();
        let _ = self
            .pending
            .lock()
            .unwrap()
            .extend_events(vec![TerminalEvent::Title(
                "Session ended — the multiplexer host stopped".into(),
            )]);
        self.notify_tab_chrome();
    }

    /// The pane will not come back: it was closed while the stream was
    /// lost. The view handles it like the pane's own exit.
    pub(super) fn end(&self, error: &anyhow::Error) {
        log::warn!("terminal session ended: {error:#}");
        self.disconnected.store(true, Ordering::Release);
        self.wake_waiters();
        let _ = self
            .pending
            .lock()
            .unwrap()
            .extend_events(vec![TerminalEvent::Exit]);
        self.notify_tab_chrome();
    }

    pub(super) fn reconnected(&self) {
        if !self.disconnected.swap(false, Ordering::AcqRel) {
            return;
        }
        log::info!("terminal session reconnected");
        // Clears the disconnected title. The new subscription's replay then
        // restores the pane's own title, which replaces this.
        let _ = self
            .pending
            .lock()
            .unwrap()
            .extend_events(vec![TerminalEvent::ResetTitle]);
        self.notify_tab_chrome();
    }

    /// Ends viewport waits early; they check `disconnected`.
    pub(super) fn wake_waiters(&self) {
        // Pair the notification with the state mutex used by scroll waiters.
        let state = self.state.lock().unwrap();
        self.state_updated.notify_all();
        drop(state);
    }

    /// Subscribes again after the stream was lost (audit B9), retrying with
    /// a `delay` that grows up to `max_delay`. Returns the new stream, or
    /// `None` once the pane or its host is gone or the view closed.
    pub(super) fn reattach(
        &self,
        subscription: &Subscription,
        error: &anyhow::Error,
        delay: &mut Duration,
        max_delay: Duration,
    ) -> Option<TcpStream> {
        log::warn!("terminal session stream lost, subscribing again: {error:#}");
        loop {
            if !self.pause_unless_closing(*delay) {
                return None;
            }
            *delay = (*delay * 2).clamp(REATTACH_MIN_DELAY, max_delay);
            let opened = subscription.open().and_then(|stream| {
                let handle = control_handle(&stream)?;
                Ok((stream, handle))
            });
            match opened {
                Ok((stream, handle)) => {
                    // A new subscription sends frames until told otherwise:
                    // a hidden view pauses it again (audit I7).
                    let paused = self.paused.lock().unwrap();
                    let mut slot = self.stream.lock().unwrap();
                    if self.closing.load(Ordering::Acquire) {
                        let _ = stream.shutdown(Shutdown::Both);
                        return None;
                    }
                    self.frames_paused.store(false, Ordering::Release);
                    if *paused
                        && let Err(error) = write_message(&mut &handle, &Request::SetPaused(true))
                    {
                        log::warn!("cannot pause terminal session frames: {error:#}");
                    }
                    *slot = Some(handle);
                    return Some(stream);
                }
                Err(error) if error.chain().any(|cause| cause.is::<PaneGone>()) => {
                    self.end(&error);
                    return None;
                }
                Err(error) if error.chain().any(|cause| cause.is::<HostGone>()) => {
                    self.host_lost(&error);
                    return None;
                }
                Err(error) => self.disconnect(&error),
            }
        }
    }

    /// Waits for `delay`; false when the view closed first.
    pub(super) fn pause_unless_closing(&self, delay: Duration) -> bool {
        let slot = self.stream.lock().unwrap();
        let (_slot, _) = self
            .closed
            .wait_timeout_while(slot, delay, |_| !self.closing.load(Ordering::Acquire))
            .unwrap();
        !self.closing.load(Ordering::Acquire)
    }

    pub(super) fn close(&self) {
        let mut slot = self.stream.lock().unwrap();
        self.closing.store(true, Ordering::Release);
        if let Some(stream) = slot.take() {
            let _ = stream.shutdown(Shutdown::Both);
        }
        drop(slot);
        self.closed.notify_all();
    }
}

impl Drop for ClientTerminal {
    fn drop(&mut self) {
        self.shared.close();
        self.input.close();
    }
}

impl ClientTerminal {
    // The host publishes the new frame before replying, so selection waits for
    // one subscription update rather than attaching for a second full state.
    fn viewport_request(&self, command: RemoteCommand) -> anyhow::Result<RemoteReply> {
        let Response::Viewport {
            changed,
            generation,
        } = self.rpc.request(Request::ViewportCommand {
            pane: self.id.clone(),
            command,
        })?
        else {
            bail!("invalid terminal viewport response");
        };
        // A paused view gets no frames until it resumes, which brings this
        // one too.
        if changed && !*self.shared.paused.lock().unwrap() {
            self.shared.wait_for_generation(generation)?;
        }
        Ok(RemoteReply::Changed(changed))
    }

    /// Requests fail at once while the stream is lost, rather than each
    /// reconnecting to a host that is not there (audit I3).
    fn ensure_connected(&self) -> anyhow::Result<()> {
        ensure!(
            !self.shared.disconnected.load(Ordering::Acquire),
            "terminal session disconnected"
        );
        Ok(())
    }
}

impl RemoteTransport for ClientTerminal {
    fn state(&self) -> Arc<RemoteState> {
        self.shared.state()
    }
    fn state_with_damage(&self) -> (Arc<RemoteState>, Option<RemoteDamage>) {
        let state = self.shared.state.lock().unwrap();
        let damage = self.shared.damage.lock().unwrap().take(&state);
        (Arc::clone(&state), damage)
    }
    // Errors fail only this request: the caller logs them, and the pane
    // stays usable (audit B9).
    fn request(&self, command: RemoteCommand) -> anyhow::Result<RemoteReply> {
        self.ensure_connected()?;
        let changes_viewport = matches!(
            command,
            RemoteCommand::Scroll(_)
                | RemoteCommand::ScrollToBottom
                | RemoteCommand::ClearScrollback
        );
        if changes_viewport {
            return self.viewport_request(command);
        }
        match self.rpc.request(Request::Command {
            pane: self.id.clone(),
            command,
        })? {
            Response::Reply(reply) => Ok(reply),
            _ => bail!("invalid terminal response"),
        }
    }
    fn send(&self, command: RemoteCommand) {
        self.shared.remember_setting(&command);
        if let RemoteCommand::Resize(size) = &command {
            let mut claim = self.shared.size_claim.lock().unwrap();
            claim.requested(*size, &self.shared.state.lock().unwrap());
        }
        let input = Input {
            command,
            reply: None,
        };
        if self.input.push(input) == Queued::Dropped {
            self.shared.input_dropped();
        }
    }
    fn send_and_wait(&self, command: RemoteCommand) -> anyhow::Result<Option<RemoteReply>> {
        self.ensure_connected()?;
        let (tx, rx) = flume::bounded(1);
        let input = Input {
            command,
            reply: Some(tx),
        };
        let deadline = Instant::now() + RESPONSE_TIMEOUT;
        self.input.push_until(input, deadline)?;
        // Queued: the host may run it whatever happens next, so a failure
        // from here on is not a reason to send it another way. Its reply may
        // be `Written` or `InputRejected` for a write.
        Ok(rx.recv_deadline(deadline).ok().and_then(Result::ok))
    }
    fn take_events(&self) -> Vec<TerminalEvent> {
        let mut pending = self.shared.pending.lock().unwrap();
        let mut events = std::mem::take(&mut pending.events);
        let wakeup = std::mem::take(&mut pending.wakeup);
        drop(pending);
        if wakeup {
            events.push(TerminalEvent::Wakeup);
        }
        events
    }
    fn take_host_requests(&self) -> Vec<(u64, RemoteHostRequest)> {
        self.shared
            .pending
            .lock()
            .unwrap()
            .take_host_requests(Instant::now())
    }
    fn reply_to_host(&self, id: u64, reply: RemoteHostReply) {
        if let Err(error) = self.host.request(Request::HostReply {
            pane: self.id.clone(),
            id,
            reply,
        }) {
            // The program was or will be told it was denied.
            log::warn!("cannot answer a terminal clipboard request: {error:#}");
        }
    }
    fn has_pending_events(&self) -> bool {
        let pending = self.shared.pending.lock().unwrap();
        pending.wakeup || !pending.events.is_empty() || !pending.hosts.is_empty()
    }
    fn set_wakeup_enabled(&self, enabled: bool) {
        // A hidden view needs no frames, only events (audit I7). A new
        // subscription is paused again when it opens (see `reattach`).
        let mut paused = self.shared.paused.lock().unwrap();
        if *paused == enabled {
            *paused = !enabled;
            if !self.shared.disconnected.load(Ordering::Acquire)
                && let Err(error) = self.shared.send_control(&Request::SetPaused(!enabled))
            {
                log::warn!("cannot pause terminal session frames: {error:#}");
            }
        }
        drop(paused);
        self.shared.wakeup_enabled.store(enabled, Ordering::Release);
        if enabled && self.has_pending_events() {
            self.shared.notify();
        }
    }
    fn host_size_overridden(&self) -> bool {
        let claim = self.shared.size_claim.lock().unwrap();
        claim.overridden(&self.shared.state.lock().unwrap())
    }
    fn frames_paused(&self) -> bool {
        self.shared.frames_paused.load(Ordering::Acquire)
    }
    fn host_lost(&self) -> bool {
        self.shared.host_lost.load(Ordering::Acquire)
    }
}
