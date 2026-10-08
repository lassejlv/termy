//! Node-API bindings for `termy_core::pty`, published as `@termysh/pty`.
//!
//! The JavaScript wrapper in `packages/pty` provides the node-pty-compatible
//! API; this crate exposes one small native class. Output and exit events go
//! through a single bounded threadsafe function, so they reach JavaScript in
//! order, and a full queue blocks the PTY reader instead of buffering without
//! limit.

use std::{
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
};

use napi::{
    bindgen_prelude::{Buffer, FnArgs, Function},
    threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode},
};
use napi_derive::napi;
use termy_core::pty::{Pty, PtyCommand, PtyDimensions, PtyExit};

/// Output chunks waiting for JavaScript before the reader stops reading.
const EVENT_QUEUE_SIZE: usize = 64;

const EVENT_DATA: u32 = 0;
const EVENT_EXIT: u32 = 1;

enum Event {
    Data(Vec<u8>),
    Exit(PtyExit),
}

/// `(event, data, exitCode, signal)`, as the JavaScript callback receives it.
type EventArgs = FnArgs<(u32, Option<Buffer>, Option<i64>, Option<i32>)>;
type EventCallback =
    ThreadsafeFunction<Event, (), EventArgs, napi::Status, false, false, EVENT_QUEUE_SIZE>;

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

/// Pauses the reader thread while JavaScript has asked for no more output.
#[derive(Default)]
struct Flow {
    paused: Mutex<bool>,
    resumed: Condvar,
}

impl Flow {
    fn set_paused(&self, paused: bool) {
        *self
            .paused
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = paused;
        if !paused {
            self.resumed.notify_all();
        }
    }

    fn wait_while_paused(&self) {
        let paused = self
            .paused
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _resumed = self
            .resumed
            .wait_while(paused, |paused| *paused)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
}

#[napi]
pub struct NativePty {
    pty: Option<Pty>,
    flow: Arc<Flow>,
    pid: u32,
}

#[napi]
impl NativePty {
    /// Start a program. `callback(event, data, exitCode, signal)` receives
    /// `0` with a Buffer of output, then `1` once with the exit status.
    #[napi(constructor)]
    pub fn new(
        options: NativeSpawnOptions,
        callback: Function<'_, EventArgs, ()>,
    ) -> napi::Result<Self> {
        let events: EventCallback = callback
            .build_threadsafe_function::<Event>()
            .max_queue_size::<EVENT_QUEUE_SIZE>()
            .build_callback(|context| {
                Ok(match context.value {
                    Event::Data(bytes) => (EVENT_DATA, Some(Buffer::from(bytes)), None, None),
                    Event::Exit(exit) => (EVENT_EXIT, None, exit.code, exit.signal),
                }
                .into())
            })?;

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

        let flow = Arc::new(Flow::default());
        let reader_flow = flow.clone();
        let output_events = Arc::new(events);
        let exit_events = output_events.clone();
        let pty = Pty::spawn(
            command,
            dimensions,
            move |bytes| {
                reader_flow.wait_while_paused();
                // Blocking applies backpressure: a full queue stops the reader.
                output_events.call(
                    Event::Data(bytes.to_vec()),
                    ThreadsafeFunctionCallMode::Blocking,
                );
            },
            move |exit| {
                exit_events.call(Event::Exit(exit), ThreadsafeFunctionCallMode::Blocking);
            },
        )
        .map_err(|error| napi::Error::from_reason(error.to_string()))?;
        let pid = pty.pid();
        Ok(Self {
            pty: Some(pty),
            flow,
            pid,
        })
    }

    #[napi(getter)]
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Foreground process name, or null when unknown (always on Windows).
    #[napi(getter)]
    pub fn process(&self) -> Option<String> {
        self.pty.as_ref()?.foreground_process_name()
    }

    /// Queue input. Input after the program has exited is dropped, as with a
    /// closed terminal; a full 8 MiB input backlog is an error.
    #[napi]
    pub fn write(&self, data: Buffer) -> napi::Result<()> {
        let Some(pty) = &self.pty else {
            return Ok(());
        };
        match pty.write(&data) {
            Err(error) if error.kind() != std::io::ErrorKind::BrokenPipe => {
                Err(napi::Error::from_reason(error.to_string()))
            }
            _ => Ok(()),
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
        self.flow.set_paused(false);
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

    #[napi]
    pub fn pause(&self) {
        self.flow.set_paused(true);
    }

    #[napi]
    pub fn resume(&self) {
        self.flow.set_paused(false);
    }

    /// Hang up and release the PTY. Output that is still queued is delivered;
    /// the exit event follows.
    #[napi]
    pub fn destroy(&mut self) {
        self.flow.set_paused(false);
        self.pty.take();
    }
}

impl Drop for NativePty {
    fn drop(&mut self) {
        self.flow.set_paused(false);
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
