// Child liveness tracking, reaping and shutdown signaling for the Unix PTY
// child. Included into `unix.rs`, so it shares that module's imports and items.

/// Whether the child may still be signaled. The watcher clears `alive` under
/// `signaling` before it reaps the child, and senders signal under it only
/// while `alive` is set, so no signal reaches a PID reused after the reap.
/// Where the exit can be awaited without reaping (`wait_for_child_exit`),
/// the PID stays the child's zombie until then; elsewhere the reap precedes
/// the clear and a signal can still race it.
struct ChildLiveness {
    alive: AtomicBool,
    signaling: Mutex<()>,
}

impl ChildLiveness {
    fn new() -> Self {
        Self {
            alive: AtomicBool::new(true),
            signaling: Mutex::new(()),
        }
    }

    fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    /// Run `send` (which signals the child) unless it has exited.
    fn signal_if_alive<T>(&self, send: impl FnOnce() -> T) -> Option<T> {
        let _signaling = self
            .signaling
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.is_alive().then(send)
    }

    fn exited(&self) {
        let _signaling = self
            .signaling
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.alive.store(false, Ordering::Release);
    }
}

fn terminate_and_reap(pid: c_int) {
    // SAFETY: `pid` is the exact process and process group created by forkpty.
    unsafe {
        let _ = kill(-pid, SIGKILL);
        let _ = kill(pid, SIGKILL);
        let mut status = 0;
        while waitpid(pid, &mut status, 0) < 0
            && io::Error::last_os_error().kind() == ErrorKind::Interrupted
        {}
    }
}

/// Wait until the child has exited, leaving it unreaped so its PID can't be
/// reused yet. False when that wait isn't available or failed: the caller
/// then learns of the exit by reaping.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn wait_for_child_exit(pid: c_int) -> bool {
    // SAFETY: `info` is writable for the call; WNOWAIT leaves the child for
    // the watcher's `waitpid`, and the watcher is its sole waiter.
    unsafe {
        let mut info = std::mem::zeroed::<libc::siginfo_t>();
        loop {
            if libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            ) == 0
            {
                return true;
            }
            if io::Error::last_os_error().kind() != ErrorKind::Interrupted {
                return false;
            }
        }
    }
}

/// Elsewhere `waitid` with `WNOWAIT` is not relied on: if it returned before
/// the exit, the watcher would stop signaling a running child. The reap comes
/// first.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn wait_for_child_exit(_pid: c_int) -> bool {
    false
}

/// `None` when the status is unknown: a host that ignores SIGCHLD or reaps
/// with `waitpid(-1)` makes this wait fail with ECHILD.
fn wait_for_child(pid: c_int) -> Option<c_int> {
    // SAFETY: the dedicated watcher is the sole waiter for the exact child PID
    // returned by forkpty.
    unsafe {
        let mut status = 0;
        loop {
            if waitpid(pid, &mut status, 0) >= 0 {
                return Some(status);
            }
            if io::Error::last_os_error().kind() != ErrorKind::Interrupted {
                return None;
            }
        }
    }
}

fn signal_child_shutdown(pid: c_int) {
    // SAFETY: `pid` is the exact process and process group created by forkpty.
    unsafe {
        if kill(-pid, SIGHUP) < 0 {
            let _ = kill(pid, SIGHUP);
        }
    }
}

fn force_child_shutdown(pid: c_int) {
    // The watcher remains the sole waiter. SIGKILL guarantees its wait returns
    // even when a child ignores the conventional terminal SIGHUP.
    unsafe {
        let _ = kill(-pid, SIGKILL);
        let _ = kill(pid, SIGKILL);
    }
}

fn schedule_child_shutdown_escalation(pid: c_int, alive: Arc<ChildLiveness>, grace: Duration) {
    let fallback_alive = alive.clone();
    let escalation = thread::Builder::new()
        .name("termy-native-pty-shutdown".to_string())
        .spawn(move || {
            thread::sleep(grace);
            alive.signal_if_alive(|| force_child_shutdown(pid));
        });

    // A failed thread spawn must not restore the original unbounded shutdown.
    // Killing is non-blocking here; the dedicated watcher still performs waitpid.
    if escalation.is_err() {
        fallback_alive.signal_if_alive(|| force_child_shutdown(pid));
    }
}
