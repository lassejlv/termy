use super::*;
use crate::terminal_ui::TmuxNotification;
use crate::terminal_view::runtime::TMUX_MOUSE_MODE_SUBSCRIPTION_NAME;
use std::time::{Duration, Instant};

fn parse_tmux_mouse_mode_subscription(value: &str) -> Option<TmuxPaneMouseMode> {
    let [standard, button, any, sgr, utf8] = value.as_bytes() else {
        return None;
    };
    let flag = |value| match value {
        b'0' => Some(false),
        b'1' => Some(true),
        _ => None,
    };

    Some(TmuxPaneMouseMode {
        standard: flag(*standard)?,
        button: flag(*button)?,
        any: flag(*any)?,
        sgr: flag(*sgr)?,
        utf8: flag(*utf8)?,
    })
}

impl TerminalView {
    pub(in crate::terminal_view) fn request_tmux_resize_convergence(
        &mut self,
        cols: u16,
        rows: u16,
    ) {
        self.tmux_runtime_mut()
            .resize_scheduler
            .request_resize(cols, rows);
        let _ = self.event_wakeup_tx.try_send(());
    }

    pub(in crate::terminal_view) fn clear_tmux_resize_convergence(&mut self) {
        let runtime = self.tmux_runtime_mut();
        runtime.resize_scheduler.clear();
        runtime.resize_wakeup_scheduled = false;
    }

    fn ensure_tmux_resize_convergence_wakeup(&mut self, cx: &mut Context<Self>) {
        if !self.runtime_uses_tmux() || !self.tmux_runtime().resize_scheduler.has_work() {
            return;
        }

        match self
            .tmux_runtime()
            .resize_scheduler
            .next_wakeup(Instant::now())
        {
            TmuxResizeWakeup::None => {}
            TmuxResizeWakeup::Immediate => {
                let _ = self.event_wakeup_tx.try_send(());
            }
            TmuxResizeWakeup::Delayed(delay) => {
                if self.tmux_runtime().resize_wakeup_scheduled {
                    return;
                }

                self.tmux_runtime_mut().resize_wakeup_scheduled = true;
                let wakeup_tx = self.event_wakeup_tx.clone();
                cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                    smol::Timer::after(delay).await;
                    let _ = cx.update(|cx| {
                        this.update(cx, |view, _cx| {
                            // The delayed callback can outlive tmux runtime; guard before any
                            // tmux_runtime_mut() access to avoid panicking after detach/exit.
                            if !view.runtime_uses_tmux() {
                                return;
                            }
                            let has_work = {
                                let runtime = view.tmux_runtime_mut();
                                runtime.resize_wakeup_scheduled = false;
                                runtime.resize_scheduler.has_work()
                            };
                            if has_work {
                                let _ = wakeup_tx.try_send(());
                            };
                        })
                    });
                })
                .detach();
            }
        }
    }

    pub(in crate::terminal_view) fn drive_tmux_resize_convergence(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut should_redraw = false;
        if let Some(attempt) = self
            .tmux_runtime_mut()
            .resize_scheduler
            .claim_attempt(Instant::now())
        {
            match self.tmux_runtime().client.refresh_snapshot() {
                Ok(snapshot) => {
                    let converged =
                        Self::snapshot_matches_client_size(&snapshot, attempt.cols, attempt.rows);
                    self.apply_tmux_snapshot(snapshot);
                    should_redraw = true;
                    self.tmux_runtime_mut()
                        .resize_scheduler
                        .complete_attempt(Instant::now(), converged);
                }
                Err(error) => {
                    crate::ui::toast::error(format!("tmux sync failed: {error}"));
                    self.clear_tmux_resize_convergence();
                }
            }
        }

        self.ensure_tmux_resize_convergence_wakeup(cx);
        should_redraw
    }

    pub(in crate::terminal_view) fn schedule_tmux_title_refresh(&mut self) {
        self.tmux_runtime_mut().title_refresh_deadline =
            Some(Instant::now() + Duration::from_millis(TMUX_TITLE_REFRESH_DEBOUNCE_MS));
        let _ = self.event_wakeup_tx.try_send(());
    }

    fn ensure_tmux_title_refresh_wakeup(&mut self, cx: &mut Context<Self>) {
        if !self.runtime_uses_tmux()
            || self.tmux_runtime().title_refresh_deadline.is_none()
            || self.tmux_runtime().title_refresh_wakeup_scheduled
        {
            return;
        }

        self.tmux_runtime_mut().title_refresh_wakeup_scheduled = true;
        let wakeup_tx = self.event_wakeup_tx.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            smol::Timer::after(Duration::from_millis(TMUX_TITLE_REFRESH_DEBOUNCE_MS)).await;
            let _ = cx.update(|cx| {
                this.update(cx, |view, _cx| {
                    // Same safety rule as resize wakeups: this task can fire after runtime
                    // transitions back to native mode.
                    if !view.runtime_uses_tmux() {
                        return;
                    }
                    let has_deadline = {
                        let runtime = view.tmux_runtime_mut();
                        runtime.title_refresh_wakeup_scheduled = false;
                        runtime.title_refresh_deadline.is_some()
                    };
                    if has_deadline {
                        let _ = wakeup_tx.try_send(());
                    };
                })
            });
        })
        .detach();
    }

    fn tmux_snapshot_refresh_mode(
        needs_refresh: bool,
        title_refresh_deadline: Option<Instant>,
        now: Instant,
    ) -> TmuxSnapshotRefreshMode {
        if needs_refresh {
            return TmuxSnapshotRefreshMode::Immediate;
        }

        if title_refresh_deadline.is_some_and(|deadline| now >= deadline) {
            return TmuxSnapshotRefreshMode::Debounced;
        }

        TmuxSnapshotRefreshMode::None
    }

    pub(in crate::terminal_view) fn process_tmux_terminal_events(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut should_redraw = false;
        let mut needs_refresh = false;

        for notification in self.tmux_runtime().client.poll_notifications() {
            match notification {
                TmuxNotification::Output { pane_id, bytes } => {
                    if let Some(terminal) = self.pane_terminal_by_id(&pane_id) {
                        terminal.feed_output(&bytes);
                        let replies = {
                            let mut clipboard_text = ClipboardTextCache::default();
                            let mut reply_host =
                                GpuiClipboardReplyHost::new(cx, &mut clipboard_text);
                            terminal.drain_kitty_clipboard_events(&mut reply_host)
                        };
                        for reply in replies {
                            let _ = self.tmux_send_input_to_pane(&pane_id, &reply);
                        }
                        if self.is_active_pane_id(&pane_id) {
                            should_redraw = true;
                            self.schedule_tmux_title_refresh();
                        }
                    }
                }
                TmuxNotification::NeedsRefresh => {
                    needs_refresh = true;
                }
                TmuxNotification::Warning(message) => {
                    crate::ui::toast::warning(message);
                    should_redraw = true;
                }
                TmuxNotification::SubscriptionChanged {
                    name, pane, value, ..
                } if name == TMUX_MOUSE_MODE_SUBSCRIPTION_NAME => {
                    if let Some(mode) = parse_tmux_mouse_mode_subscription(&value)
                        && let Some(pane) = self
                            .session
                            .tabs
                            .iter_mut()
                            .flat_map(|tab| tab.panes.iter_mut())
                            .find(|candidate| candidate.id == pane)
                    {
                        pane.tmux_mouse_mode = Some(mode);
                    }
                }
                TmuxNotification::SubscriptionChanged { .. } => {}
                TmuxNotification::Exit(reason) => {
                    let reason =
                        Some(reason.unwrap_or_else(|| "tmux control mode exited".to_string()));
                    return self.recover_from_tmux_runtime_exit(reason, cx);
                }
            }
        }

        self.ensure_tmux_title_refresh_wakeup(cx);
        let now = Instant::now();
        match Self::tmux_snapshot_refresh_mode(
            needs_refresh,
            self.tmux_runtime().title_refresh_deadline,
            now,
        ) {
            TmuxSnapshotRefreshMode::Immediate | TmuxSnapshotRefreshMode::Debounced => {
                {
                    let runtime = self.tmux_runtime_mut();
                    runtime.title_refresh_deadline = None;
                    runtime.title_refresh_wakeup_scheduled = false;
                }
                if self.refresh_tmux_snapshot() {
                    should_redraw = true;
                }
            }
            TmuxSnapshotRefreshMode::None => {}
        }

        if self.drive_tmux_resize_convergence(cx) {
            should_redraw = true;
        }

        should_redraw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tmux_mouse_mode_subscription_flags() {
        assert_eq!(
            parse_tmux_mouse_mode_subscription("00110"),
            Some(TmuxPaneMouseMode {
                standard: false,
                button: false,
                any: true,
                sgr: true,
                utf8: false,
            })
        );
        assert_eq!(
            parse_tmux_mouse_mode_subscription("00000"),
            Some(Default::default())
        );
        assert_eq!(parse_tmux_mouse_mode_subscription("0011"), None);
        assert_eq!(parse_tmux_mouse_mode_subscription("00x10"), None);
    }

    #[test]
    fn tmux_snapshot_refresh_mode_is_debounced_when_deadline_has_elapsed() {
        let now = Instant::now();
        let mode = TerminalView::tmux_snapshot_refresh_mode(
            false,
            Some(now - Duration::from_millis(1)),
            now,
        );
        assert_eq!(mode, TmuxSnapshotRefreshMode::Debounced);
    }

    #[test]
    fn tmux_snapshot_refresh_mode_is_none_when_deadline_has_not_elapsed() {
        let now = Instant::now();
        let mode = TerminalView::tmux_snapshot_refresh_mode(
            false,
            Some(now + Duration::from_millis(5)),
            now,
        );
        assert_eq!(mode, TmuxSnapshotRefreshMode::None);
    }

    #[test]
    fn tmux_snapshot_refresh_mode_prioritizes_immediate_refresh_over_debounce() {
        let now = Instant::now();
        let mode = TerminalView::tmux_snapshot_refresh_mode(
            true,
            Some(now - Duration::from_millis(1)),
            now,
        );
        assert_eq!(mode, TmuxSnapshotRefreshMode::Immediate);
    }
}
