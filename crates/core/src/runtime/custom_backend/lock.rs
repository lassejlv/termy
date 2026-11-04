//! Engine-lock hand-off between the PTY reader and every other caller.
//!
//! `std::sync::Mutex` is not fair: a reader thread that parses output in a
//! loop reacquires the lock microseconds after releasing it, so the UI can
//! wait many frames for a snapshot during `cat`-style floods. Other callers
//! announce themselves, and the reader yields to them before relocking.
use super::*;
use parking_lot::lock_api;
use std::sync::{
    PoisonError,
    atomic::{AtomicU8, AtomicUsize},
};

pub(super) type EngineMutex = lock_api::Mutex<RawEngineLock, State>;
pub(super) type EngineGuard<'a> = lock_api::MutexGuard<'a, RawEngineLock, State>;

const LOCKED: u8 = 1;
/// A caller sleeps, or is about to, until the lock is released.
const SLEEPING: u8 = 2;
/// Contended attempts before sleeping: a feeder holds the lock for one
/// slice at most, then yields to announced callers.
const SPINS: u32 = 32;
/// How late a sleeper wakes when a release missed it; see `unlock`.
const MISSED_WAKE_LIMIT: Duration = Duration::from_micros(200);

/// The engine lock. Hosts feeding a display terminal a few bytes at a time
/// take it once per write, so its uncontended release is a plain store
/// rather than a read-modify-write, which on x86 also waits for every store
/// the feed queued. Contended callers spin briefly, then sleep.
pub(super) struct RawEngineLock {
    state: AtomicU8,
    sleepers: Mutex<()>,
    wake: Condvar,
}

// SAFETY: `lock` and `try_lock` acquire only by setting `LOCKED` with an
// acquire exchange from a state without it, and only the holder clears it,
// with a release store.
unsafe impl lock_api::RawMutex for RawEngineLock {
    #[allow(clippy::declare_interior_mutable_const)]
    const INIT: Self = Self {
        state: AtomicU8::new(0),
        sleepers: Mutex::new(()),
        wake: Condvar::new(),
    };
    type GuardMarker = lock_api::GuardNoSend;

    #[inline]
    fn lock(&self) {
        if self
            .state
            .compare_exchange_weak(0, LOCKED, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            self.lock_contended();
        }
    }

    #[inline]
    fn try_lock(&self) -> bool {
        let mut state = self.state.load(Ordering::Relaxed);
        while state & LOCKED == 0 {
            match self.state.compare_exchange_weak(
                state,
                state | LOCKED,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(current) => state = current,
            }
        }
        false
    }

    #[inline]
    unsafe fn unlock(&self) {
        if self.state.load(Ordering::Relaxed) == LOCKED {
            // A caller that flags itself between the load and this store has
            // its flag cleared and sleeps until `MISSED_WAKE_LIMIT`, then
            // finds the lock free. Every other sleeper is woken below.
            self.state.store(0, Ordering::Release);
        } else {
            self.unlock_waking();
        }
    }

    #[inline]
    fn is_locked(&self) -> bool {
        self.state.load(Ordering::Relaxed) & LOCKED != 0
    }
}

impl RawEngineLock {
    #[cold]
    #[inline(never)]
    fn lock_contended(&self) {
        let mut spins = 0;
        loop {
            let state = self.state.load(Ordering::Relaxed);
            if state & LOCKED == 0 {
                if self
                    .state
                    .compare_exchange_weak(
                        state,
                        state | LOCKED,
                        Ordering::Acquire,
                        Ordering::Relaxed,
                    )
                    .is_ok()
                {
                    return;
                }
                continue;
            }
            if spins < SPINS {
                spins += 1;
                if spins <= SPINS / 2 {
                    std::hint::spin_loop();
                } else {
                    std::thread::yield_now();
                }
                continue;
            }
            let sleepers = self.sleepers.lock().unwrap_or_else(PoisonError::into_inner);
            // Flagged while holding `sleepers`, which a waking release takes
            // before notifying, so that release cannot notify before this
            // caller waits.
            if self
                .state
                .compare_exchange(
                    state,
                    state | SLEEPING,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                )
                .is_err()
            {
                continue;
            }
            drop(
                self.wake
                    .wait_timeout(sleepers, MISSED_WAKE_LIMIT)
                    .unwrap_or_else(PoisonError::into_inner),
            );
        }
    }

    #[cold]
    #[inline(never)]
    fn unlock_waking(&self) {
        // Woken sleepers that lose the lock again flag themselves again.
        self.state.store(0, Ordering::Release);
        let _sleepers = self.sleepers.lock().unwrap_or_else(PoisonError::into_inner);
        self.wake.notify_all();
    }
}

/// Output fed under one lock hold; the reader yields between slices.
const FEED_SLICE: usize = 2 * PARSE_BATCH;
/// Upper bound on one yield, so a stuck waiter cannot stall output.
const READER_YIELD_LIMIT: Duration = Duration::from_millis(2);

#[derive(Default)]
pub(super) struct LockWaiters(AtomicUsize);

/// Announces a waiting caller for as long as it lives, so a panic while
/// locking cannot leave the feeder yielding to a waiter that is gone.
struct WaiterAnnouncement<'a>(&'a AtomicUsize);

impl<'a> WaiterAnnouncement<'a> {
    fn new(waiters: &'a LockWaiters) -> Self {
        waiters.0.fetch_add(1, Ordering::AcqRel);
        Self(&waiters.0)
    }
}

impl Drop for WaiterAnnouncement<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Shared {
    pub(super) fn state(&self) -> EngineGuard<'_> {
        let _announcement = WaiterAnnouncement::new(&self.lock_waiters);
        self.lock_state()
    }

    pub(super) fn live_transport(&self) -> Option<Arc<Transport>> {
        self.transport.get().and_then(Weak::upgrade)
    }

    /// Queue replies to the child. Callers hold the engine lock, so replies
    /// from different feeders (reader slices, the synchronized-update
    /// watchdog) reach the child in the order the engine produced them.
    pub(super) fn write_replies_locked(transport: &Transport, replies: Vec<u8>) {
        if replies.is_empty() {
            return;
        }
        if let Err(error) = transport.write_protocol_reply_owned(replies) {
            log::warn!("dropping terminal protocol reply: {error}");
        }
    }

    fn lock_state(&self) -> EngineGuard<'_> {
        self.state.lock()
    }

    /// Lock for feeding output, after any announced waiter got its turn.
    fn feeder_state(&self) -> EngineGuard<'_> {
        if self.lock_waiters.0.load(Ordering::Acquire) != 0 {
            let deadline = Instant::now() + READER_YIELD_LIMIT;
            while self.lock_waiters.0.load(Ordering::Acquire) != 0 && Instant::now() < deadline {
                std::thread::yield_now();
            }
        }
        self.lock_state()
    }

    /// Feed output in bounded slices so waiting callers can interleave.
    /// Hydration replays stay atomic: they finish synchronized updates.
    pub(super) fn feed(
        self: &Arc<Self>,
        bytes: &[u8],
        hydrate: bool,
        buffer_replies: bool,
    ) -> Vec<u8> {
        // Hosts feeding a display terminal (tmux `%output`, embedders) send
        // many small reads: one slice, one lock hold, replies buffered.
        if buffer_replies && !hydrate && bytes.len() <= PARSE_BATCH {
            let rest = self.feed_display_slice(bytes);
            if rest.is_empty() {
                return Vec::new();
            }
            return self.feed_slices(rest, false, true);
        }
        self.feed_slices(bytes, hydrate, buffer_replies)
    }

    /// One iteration of `feed_slices` for buffered, non-hydrating output
    /// that fits a single slice. Returns what an upload or animation pause
    /// left unparsed, for `feed_slices` to continue.
    #[inline]
    fn feed_display_slice<'a>(self: &Arc<Self>, bytes: &'a [u8]) -> &'a [u8] {
        let mut state = self.feeder_state();
        let before_generation = state.engine.generation();
        let consumed = state.engine.feed_deferring_uploads(bytes);
        // Events and replies are rare; skip draining when there are none.
        if state.engine.has_output() {
            state.drain_engine_output();
        }
        // Output buffered by a synchronized update changes nothing yet.
        if state.engine.generation() != before_generation {
            state.generation = state.generation.wrapping_add(1);
        }
        // A synchronized update can hold several uploads: committing one
        // replays the update up to the next.
        while let Some(upload) = state.engine.take_deferred_upload() {
            drop(state);
            let prepared = upload.prepare();
            state = self.feeder_state();
            let replies = state.commit_upload(prepared);
            state.append_replies(&replies);
        }
        if state.engine.take_history_activity() {
            state.defer_history_compaction(Instant::now());
        }
        let notify = !state.engine.modes().synchronized_update || !state.events.is_empty();
        let tab_chrome_changed = state.take_tab_chrome_changed();
        self.schedule_maintenance(&mut state);
        drop(state);
        if notify {
            self.notify();
        }
        if tab_chrome_changed {
            self.notify_tab_chrome();
        }
        &bytes[consumed..]
    }

    #[inline(never)]
    fn feed_slices(self: &Arc<Self>, bytes: &[u8], hydrate: bool, buffer_replies: bool) -> Vec<u8> {
        let slice = if hydrate {
            bytes.len().max(1)
        } else {
            FEED_SLICE
        };
        let mut replies = Vec::new();
        // PTY output queues each slice's replies before the lock is released.
        // Holding them until the whole read is parsed would let the watchdog
        // send a later reply first while this read waits behind a UI lock.
        let transport = (!buffer_replies && !hydrate)
            .then(|| self.live_transport())
            .flatten();
        let (mut should_notify, mut tab_chrome_changed) = (false, false);
        let mut rest = bytes;
        while !rest.is_empty() {
            let mut state = self.feeder_state();
            let (mut slice_replies, consumed) = state.feed(&rest[..rest.len().min(slice)], hydrate);
            rest = &rest[consumed..];
            while let Some(upload) = state.engine.take_deferred_upload() {
                // Decode the image (base64, zlib, PNG, file reads) unlocked.
                // Output has one feeder at a time (the PTY reader, or the
                // host for display terminals), so the upload still lands
                // between the bytes around it. Committing an upload from a
                // synchronized update replays it up to its next upload.
                drop(state);
                let prepared = upload.prepare();
                state = self.feeder_state();
                slice_replies.append(&mut state.commit_upload(prepared));
            }
            if buffer_replies {
                state.append_replies(&slice_replies);
            } else if let Some(transport) = &transport {
                Self::write_replies_locked(transport, slice_replies);
            } else {
                replies.append(&mut slice_replies);
            }
            if state.engine.take_history_activity() {
                state.defer_history_compaction(Instant::now());
            }
            should_notify |= !state.engine.modes().synchronized_update || !state.events.is_empty();
            tab_chrome_changed |= state.take_tab_chrome_changed();
            // Publish while the engine lock still protects this deadline: a later
            // feed must not have its timer replaced by an earlier feed's deadline.
            self.schedule_maintenance(&mut state);
        }
        if should_notify {
            self.notify();
        }
        if tab_chrome_changed {
            self.notify_tab_chrome();
        }
        replies
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panicking_waiter_withdraws_its_announcement() {
        let waiters = LockWaiters::default();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _announcement = WaiterAnnouncement::new(&waiters);
            assert_eq!(waiters.0.load(Ordering::Acquire), 1);
            panic!("lock panicked");
        }));
        assert!(result.is_err());
        assert_eq!(waiters.0.load(Ordering::Acquire), 0);
    }

    type TestMutex<T> = lock_api::Mutex<RawEngineLock, T>;

    #[test]
    fn the_engine_lock_excludes_holders_and_reports_its_state() {
        let mutex = TestMutex::new(0);
        let guard = mutex.lock();
        assert!(mutex.is_locked());
        assert!(mutex.try_lock().is_none());
        drop(guard);
        assert!(!mutex.is_locked());
        // Holders that sleep send the others through the spinning and
        // sleeping paths; no two may ever hold the lock at once.
        let mutex = Arc::new(TestMutex::new((0, false)));
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let mutex = Arc::clone(&mutex);
                std::thread::spawn(move || {
                    for round in 0..2000 {
                        let mut guard = mutex.lock();
                        assert!(!guard.1, "two holders");
                        guard.1 = true;
                        guard.0 += 1;
                        if round % 200 == 0 {
                            std::thread::sleep(Duration::from_micros(500));
                        }
                        guard.1 = false;
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(mutex.lock().0, 8000);
    }

    #[test]
    fn releasing_the_engine_lock_wakes_a_sleeping_caller() {
        let mutex = Arc::new(TestMutex::new(()));
        let guard = mutex.lock();
        let waiter = {
            let mutex = Arc::clone(&mutex);
            std::thread::spawn(move || drop(mutex.lock()))
        };
        // Wait until the caller has flagged itself to sleep.
        // SAFETY: the raw lock is only read, never locked or unlocked.
        let raw = unsafe { mutex.raw() };
        while raw.state.load(Ordering::Acquire) & SLEEPING == 0 {
            std::thread::yield_now();
        }
        drop(guard);
        waiter.join().unwrap();
        assert_eq!(raw.state.load(Ordering::Acquire), 0);
    }
}
