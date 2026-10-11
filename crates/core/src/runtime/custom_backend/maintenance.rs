//! One sleeping timer handles synchronized-output deadlines and idle history compaction.
use super::*;

impl Shared {
    pub(super) fn schedule_maintenance(self: &Arc<Self>, state: &State) {
        let deadline = state.maintenance_deadline();
        let mut signal = self
            .sync_signal
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if signal.deadline == deadline {
            return;
        }
        let wake_timer = signal.deadline.is_none() || deadline < signal.deadline;
        signal.deadline = deadline;
        if wake_timer {
            self.sync_signal.changed.notify_one();
        }
        drop(signal);
        if deadline.is_none() || self.sync_watchdog_started.swap(true, Ordering::AcqRel) {
            return;
        }
        let shared = Arc::downgrade(self);
        let signal = self.sync_signal.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("termy-maintenance".into())
            .spawn(move || {
                loop {
                    let mut pending = signal
                        .state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    loop {
                        if pending.shutdown {
                            return;
                        }
                        let Some(deadline) = pending.deadline else {
                            pending = signal
                                .changed
                                .wait(pending)
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            continue;
                        };
                        let now = Instant::now();
                        if now < deadline {
                            let (next, _) = signal
                                .changed
                                .wait_timeout(pending, deadline - now)
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            pending = next;
                            continue;
                        }
                        pending.deadline = None;
                        drop(pending);
                        let Some(shared) = shared.upgrade() else {
                            return;
                        };
                        let mut state = shared.state();
                        if state.maintenance_deadline() != Some(deadline) {
                            break;
                        }
                        let now = Instant::now();
                        if state
                            .history_deadline
                            .is_some_and(|deadline| deadline <= now)
                        {
                            let quiet = state.history_quiet;
                            state.engine.compact_history_step(256, quiet);
                            // Output that keeps arriving restarts the cap, so a
                            // flood gets at most one forced step per interval.
                            state.history_pending_since = None;
                            state.history_deadline = state
                                .engine
                                .needs_history_compaction()
                                .then(|| now + std::time::Duration::from_millis(8));
                        }
                        let committed = state
                            .engine
                            .synchronized_update_deadline()
                            .is_some_and(|deadline| deadline <= now)
                            && state.engine.stop_synchronized_update();
                        let mut replies = Vec::new();
                        if committed {
                            if state.engine.take_history_activity() {
                                state.defer_history_compaction(now);
                            }
                            while let Some(event) = state.engine.pop_event() {
                                state.engine_event(event);
                            }
                            state.engine.drain_replies(&mut replies);
                            state.generation = state.generation.wrapping_add(1);
                        }
                        let tab_chrome_changed = committed && state.take_tab_chrome_changed();
                        shared.schedule_maintenance(&state);
                        drop(state);
                        if !replies.is_empty() {
                            let transport = shared
                                .transport
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .upgrade();
                            if let Some(transport) = transport {
                                if let Err(error) = transport.write_protocol_reply_owned(replies) {
                                    log::warn!("terminal timeout reply failed: {error}");
                                }
                            } else {
                                shared.state().append_replies(&replies);
                            }
                        }
                        if committed {
                            shared.notify();
                        }
                        if tab_chrome_changed {
                            shared.notify_tab_chrome();
                        }
                        break;
                    }
                }
            })
        {
            self.sync_watchdog_started.store(false, Ordering::Release);
            log::warn!("could not start terminal synchronization watchdog: {error}");
        }
    }
}
