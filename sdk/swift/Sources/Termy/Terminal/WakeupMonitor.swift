import Foundation
internal import CTermy

/// A dedicated thread blocked in `termy_terminal_wait_for_wakeup`.
///
/// The wait/notify pair is the only part of the C API that may run
/// concurrently with other calls on the same handle. `stop()` wakes and joins
/// the thread, so the handle can be freed safely afterwards.
final class WakeupMonitor: @unchecked Sendable {
    private let handle: OpaquePointer
    private let onWake: @Sendable () -> Void
    private let lock = NSLock()
    private var running = false
    private var thread: Thread?
    private let exited = DispatchSemaphore(value: 0)

    init(handle: OpaquePointer, onWake: @escaping @Sendable () -> Void) {
        self.handle = handle
        self.onWake = onWake
    }

    var isRunning: Bool { lock.withLock { running } }

    func start() {
        let shouldStart = lock.withLock {
            guard !running else { return false }
            running = true
            return true
        }
        guard shouldStart else { return }
        let thread = Thread { [self] in run() }
        thread.name = "dev.termy.wakeup"
        thread.qualityOfService = .userInitiated
        lock.withLock { self.thread = thread }
        thread.start()
    }

    /// Wakes the waiting thread and blocks until it has exited.
    ///
    /// When `onWake` drops the last reference to the `Terminal`, its deinit
    /// calls this on the wakeup thread itself. Waiting there would deadlock,
    /// so it returns at once: the thread leaves its loop when `onWake` returns
    /// and no longer touches the handle.
    func stop() {
        let (wasRunning, thread) = lock.withLock {
            defer { running = false }
            return (running, self.thread)
        }
        guard wasRunning else { return }
        termy_terminal_notify_wakeup(handle)
        if let thread, Thread.current === thread { return }
        exited.wait()
    }

    private func run() {
        // The long timeout is a safety net; output and stop() wake immediately.
        while isRunning {
            var woke = false
            let status = termy_terminal_wait_for_wakeup(handle, 60_000, &woke)
            guard isRunning else { break }
            if status != TERMY_FFI_OK {
                Thread.sleep(forTimeInterval: 0.05)
            } else if woke {
                onWake()
            }
        }
        exited.signal()
    }
}
