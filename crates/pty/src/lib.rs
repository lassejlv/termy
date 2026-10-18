//! Node-API bindings for `termy_core::pty`, published as `@termysh/pty`.
//!
//! The JavaScript wrapper in `packages/pty` provides the node-pty-compatible
//! API; this crate exposes one small native class. Reads collect in a bounded
//! buffer that one threadsafe-function call drains, so a burst of 1 KiB reads
//! reaches JavaScript as one Buffer, output always precedes the exit event,
//! and a full buffer blocks the PTY reader instead of growing without limit.

use std::{
    path::PathBuf,
    sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, Weak},
};

use napi::{
    bindgen_prelude::{Buffer, FnArgs, Function},
    threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode},
};
use napi_derive::napi;
use termy_core::pty::{Pty, PtyCommand, PtyDimensions, PtyExit};

/// At most one flush is queued at a time; the bound is only a safety net.
const EVENT_QUEUE_SIZE: usize = 4;
/// Output buffered for JavaScript before the reader stops reading.
const MAX_BUFFERED_OUTPUT: usize = 1024 * 1024;

const EVENT_DATA: u32 = 0;
const EVENT_EXIT: u32 = 1;
/// A flush that found nothing to deliver (after a pause); JavaScript ignores it.
const EVENT_NONE: u32 = 2;

/// Deliver buffered output, or the exit once the output is gone.
struct Flush;

/// `(event, data, exitCode, signal)`, as the JavaScript callback receives it.
type EventArgs = FnArgs<(u32, Option<Buffer>, Option<i64>, Option<i32>)>;
type EventCallback =
    ThreadsafeFunction<Flush, (), EventArgs, napi::Status, false, false, EVENT_QUEUE_SIZE>;

#[napi(object)]
pub struct NativeSpawnOptions {
    pub file: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    /// Flattened `[name, value, name, value, ...]`: the child's whole environment.
    pub env: Vec<String>,
    pub cols: u32,
    pub rows: u32,
}

#[derive(Default)]
struct OutputState {
    bytes: Vec<u8>,
    exit: Option<PtyExit>,
    /// A flush is queued for JavaScript.
    scheduled: bool,
    paused: bool,
    closed: bool,
}

/// Output and exit waiting for JavaScript.
#[derive(Default)]
struct Output {
    state: Mutex<OutputState>,
    drained: Condvar,
    events: OnceLock<Weak<EventCallback>>,
}

impl Output {
    fn lock(&self) -> MutexGuard<'_, OutputState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Claim the single queued flush if there is something to deliver.
    fn claim_flush(state: &mut OutputState) -> bool {
        let claim = !state.scheduled
            && !state.paused
            && !state.closed
            && (!state.bytes.is_empty() || state.exit.is_some());
        state.scheduled |= claim;
        claim
    }

    fn flush(&self, mode: ThreadsafeFunctionCallMode) {
        let queued = self
            .events
            .get()
            .and_then(Weak::upgrade)
            .is_some_and(|events| events.call(Flush, mode) == napi::Status::Ok);
        // A flush that never reached JavaScript must not block later ones.
        if !queued {
            self.lock().scheduled = false;
        }
    }

    /// Reader thread: buffer output, blocking while the buffer is full.
    fn push(&self, bytes: &[u8]) {
        let state = self.lock();
        let mut state = self
            .drained
            .wait_while(state, |state| {
                state.bytes.len() >= MAX_BUFFERED_OUTPUT && !state.closed
            })
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.closed {
            return;
        }
        state.bytes.extend_from_slice(bytes);
        if Self::claim_flush(&mut state) {
            drop(state);
            self.flush(ThreadsafeFunctionCallMode::Blocking);
        }
    }

    fn exit(&self, exit: PtyExit) {
        let mut state = self.lock();
        state.exit = Some(exit);
        if Self::claim_flush(&mut state) {
            drop(state);
            self.flush(ThreadsafeFunctionCallMode::Blocking);
        }
    }

    /// JavaScript thread: hand over everything buffered, then the exit.
    /// A flush queued before `pause()` delivers nothing; `resume()` queues
    /// another.
    fn take(&self) -> (u32, Option<Buffer>, Option<i64>, Option<i32>) {
        let mut state = self.lock();
        state.scheduled = false;
        let event = if state.paused {
            (EVENT_NONE, None, None, None)
        } else if !state.bytes.is_empty() {
            let bytes = std::mem::take(&mut state.bytes);
            self.drained.notify_all();
            (EVENT_DATA, Some(Buffer::from(bytes)), None, None)
        } else if let Some(exit) = state.exit.take() {
            (EVENT_EXIT, None, exit.code, exit.signal)
        } else {
            (EVENT_NONE, None, None, None)
        };
        // An exit that arrived with this output follows in the next flush.
        if Self::claim_flush(&mut state) {
            drop(state);
            self.flush(ThreadsafeFunctionCallMode::NonBlocking);
        }
        event
    }

    /// JavaScript thread.
    fn set_paused(&self, paused: bool) {
        let mut state = self.lock();
        state.paused = paused;
        if Self::claim_flush(&mut state) {
            drop(state);
            self.flush(ThreadsafeFunctionCallMode::NonBlocking);
        }
    }

    /// Stop buffering and release a reader waiting for room.
    fn close(&self) {
        self.lock().closed = true;
        self.drained.notify_all();
    }
}

#[napi]
pub struct NativePty {
    pty: Option<Pty>,
    output: Arc<Output>,
    // Kept until `destroy` so a `resume` after the child exited can still
    // deliver the buffered output and the exit.
    events: Option<Arc<EventCallback>>,
    pid: u32,
}

#[napi]
impl NativePty {
    /// Start a program. `callback(event, data, exitCode, signal)` receives
    /// `0` with a Buffer of output, then `1` once with the exit status; `2`
    /// carries nothing and is ignored.
    #[napi(constructor)]
    pub fn new(
        options: NativeSpawnOptions,
        callback: Function<'_, EventArgs, ()>,
    ) -> napi::Result<Self> {
        let output = Arc::new(Output::default());
        let flushed = output.clone();
        let events: EventCallback = callback
            .build_threadsafe_function::<Flush>()
            .max_queue_size::<EVENT_QUEUE_SIZE>()
            .build_callback(move |_| Ok(flushed.take().into()))?;
        // `Output` holds the function weakly; `destroy` releases the last
        // owner, so it stops keeping the event loop alive.
        let events = Arc::new(events);
        let _ = output.events.set(Arc::downgrade(&events));

        let dimensions = dimensions(options.cols, options.rows, 0, 0)?;
        let (pairs, remainder) = options.env.as_chunks::<2>();
        if !remainder.is_empty() {
            return Err(napi::Error::from_reason(
                "env must contain name and value pairs",
            ));
        }
        let environment = pairs
            .iter()
            .map(|[name, value]| (name.clone(), value.clone()))
            .collect();
        let command = PtyCommand {
            program: options.file,
            args: options.args,
            working_directory: options.cwd.map(PathBuf::from),
            environment,
            inherit_environment: false,
        };

        let (reader_output, exit_output) = (output.clone(), output.clone());
        let pty = Pty::spawn(
            command,
            dimensions,
            move |bytes| reader_output.push(bytes),
            move |exit| exit_output.exit(exit),
        )
        .map_err(|error| napi::Error::from_reason(error.to_string()))?;
        let pid = pty.pid();
        Ok(Self {
            pty: Some(pty),
            output,
            events: Some(events),
            pid,
        })
    }

    #[napi(getter)]
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Foreground process name, or null when unknown. On Windows it is the
    /// newest descendant of the program, with its extension (`vim.exe`).
    #[napi(getter)]
    pub fn process(&self) -> Option<String> {
        self.pty.as_ref()?.foreground_process_name()
    }

    /// Queue input. Returns false, without queueing, while the 8 MiB input
    /// backlog is full; the JavaScript wrapper retries later. Input after the
    /// program has exited is dropped, as with a closed terminal.
    #[napi]
    pub fn write(&self, data: Buffer) -> napi::Result<bool> {
        let Some(pty) = &self.pty else {
            return Ok(true);
        };
        match pty.write(&data) {
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(false),
            Err(error) if error.kind() != std::io::ErrorKind::BrokenPipe => {
                Err(napi::Error::from_reason(error.to_string()))
            }
            _ => Ok(true),
        }
    }

    #[napi]
    pub fn resize(
        &self,
        cols: u32,
        rows: u32,
        pixel_width: Option<u32>,
        pixel_height: Option<u32>,
    ) -> napi::Result<()> {
        let dimensions = dimensions(
            cols,
            rows,
            pixel_width.unwrap_or(0),
            pixel_height.unwrap_or(0),
        )?;
        let Some(pty) = &self.pty else {
            return Ok(());
        };
        match pty.resize(dimensions) {
            Err(error) if error.kind() != std::io::ErrorKind::BrokenPipe => {
                Err(napi::Error::from_reason(error.to_string()))
            }
            _ => Ok(()),
        }
    }

    /// Send a signal (Unix). Windows terminates the process; `signal` is ignored.
    #[napi]
    pub fn kill(&self, signal: Option<i32>) -> napi::Result<()> {
        let Some(pty) = &self.pty else {
            return Ok(());
        };
        // Signals such as SIGWINCH or SIGSTOP don't end the program, so a
        // `pause()` (or XOFF) stays in effect; the exit waits for `resume()`.
        #[cfg(not(windows))]
        let result = match signal {
            Some(signal) => pty.signal(signal),
            None => pty.kill(),
        };
        #[cfg(windows)]
        let result = {
            let _ = signal;
            pty.kill()
        };
        result.map_err(|error| napi::Error::from_reason(error.to_string()))
    }

    /// Stop delivering output. The reader keeps buffering up to 1 MiB, then
    /// waits; an exit is delivered after the buffered output, on `resume`.
    #[napi]
    pub fn pause(&self) {
        self.output.set_paused(true);
    }

    #[napi]
    pub fn resume(&self) {
        self.output.set_paused(false);
    }

    /// Hang up and release the PTY. Output that is still queued is delivered;
    /// the exit event follows.
    #[napi]
    pub fn destroy(&mut self) {
        self.output.set_paused(false);
        self.pty.take();
        self.events.take();
    }
}

impl Drop for NativePty {
    fn drop(&mut self) {
        self.output.close();
    }
}

/// Whether this platform can create PTYs (Windows needs ConPTY, 10 1809+).
#[napi]
pub fn available() -> bool {
    termy_core::pty::available()
}

fn dimensions(
    cols: u32,
    rows: u32,
    pixel_width: u32,
    pixel_height: u32,
) -> napi::Result<PtyDimensions> {
    let cells = |value: u32, name: &str| {
        u16::try_from(value)
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| {
                napi::Error::from_reason(format!("{name} must be between 1 and 65535, got {value}"))
            })
    };
    Ok(PtyDimensions {
        cols: cells(cols, "cols")?,
        rows: cells(rows, "rows")?,
        pixel_width,
        pixel_height,
    })
}
