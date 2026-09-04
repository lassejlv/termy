//! Native pseudo-terminal process lifecycle and coalesced asynchronous I/O.

mod process;

use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io::{Read, Write},
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
};

#[cfg(unix)]
use std::os::fd::{AsFd, OwnedFd};

use anyhow::{Context, Result, anyhow, bail};
#[cfg(unix)]
use nix::{
    errno::Errno,
    poll::{PollFd, PollFlags, PollTimeout, poll},
    unistd::{pipe, write},
};
pub use process::{
    Child, Command, ExitStatus, ProcessGroup, PtyMaster, PtySize, SpawnedPty, spawn,
};

/// Maximum terminal output retained in the shared userspace queue.
///
/// The reader also owns one fixed 32 KiB staging buffer. At the engine's measured parser
/// throughput, the queue is roughly one frame of work. When the app is slower than the child, the
/// reader waits and lets the operating system apply lossless PTY backpressure instead of allowing
/// resident memory to grow without bound.
pub const PTY_OUTPUT_BUFFER_LIMIT: usize = 512 * 1024;

const PTY_OUTPUT_INITIAL_CAPACITY: usize = 64 * 1024;
const PTY_READ_CHUNK_SIZE: usize = 32 * 1024;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PtyBufferMetrics {
    pub pending_bytes: usize,
    pub pending_capacity_bytes: usize,
    pub high_water_bytes: usize,
    pub bytes_buffered: u64,
    pub bytes_drained: u64,
    pub drain_calls: u64,
    pub producer_waits: u64,
    pub buffer_growths: u64,
    pub wake_events: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PtyIoMetrics {
    pub resize_requests: u64,
    pub resize_ioctls: u64,
    pub resize_suppressed: u64,
}

#[derive(Debug)]
struct OutputState {
    pending: Vec<u8>,
    closed: bool,
    high_water_bytes: usize,
    bytes_buffered: u64,
    bytes_drained: u64,
    drain_calls: u64,
    producer_waits: u64,
    buffer_growths: u64,
    wake_events: u64,
}

impl OutputState {
    fn new() -> Self {
        Self {
            pending: Vec::with_capacity(PTY_OUTPUT_INITIAL_CAPACITY),
            closed: false,
            high_water_bytes: 0,
            bytes_buffered: 0,
            bytes_drained: 0,
            drain_calls: 0,
            producer_waits: 0,
            buffer_growths: 0,
            wake_events: 0,
        }
    }

    fn metrics(&self) -> PtyBufferMetrics {
        PtyBufferMetrics {
            pending_bytes: self.pending.len(),
            pending_capacity_bytes: self.pending.capacity(),
            high_water_bytes: self.high_water_bytes,
            bytes_buffered: self.bytes_buffered,
            bytes_drained: self.bytes_drained,
            drain_calls: self.drain_calls,
            producer_waits: self.producer_waits,
            buffer_growths: self.buffer_growths,
            wake_events: self.wake_events,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AppendResult {
    Buffered { notify: bool },
    Closed,
}

#[derive(Debug)]
struct OutputBuffer {
    state: Mutex<OutputState>,
    space_available: Condvar,
    wake_queued: AtomicBool,
}

impl OutputBuffer {
    fn new() -> Self {
        Self {
            state: Mutex::new(OutputState::new()),
            space_available: Condvar::new(),
            wake_queued: AtomicBool::new(false),
        }
    }

    fn append(&self, bytes: &[u8]) -> Result<AppendResult> {
        if bytes.is_empty() {
            return Ok(AppendResult::Buffered { notify: false });
        }
        if bytes.len() > PTY_OUTPUT_BUFFER_LIMIT {
            bail!("PTY read chunk exceeds bounded output buffer");
        }

        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow!("PTY output buffer lock poisoned"))?;
        while !state.closed
            && state.pending.len() > PTY_OUTPUT_BUFFER_LIMIT.saturating_sub(bytes.len())
        {
            state.producer_waits = state.producer_waits.saturating_add(1);
            state = self
                .space_available
                .wait(state)
                .map_err(|_| anyhow!("PTY output buffer lock poisoned while waiting"))?;
        }
        if state.closed {
            return Ok(AppendResult::Closed);
        }

        let required_capacity = state.pending.len() + bytes.len();
        if state.pending.capacity() < required_capacity {
            let new_capacity = required_capacity
                .next_power_of_two()
                .clamp(PTY_OUTPUT_INITIAL_CAPACITY, PTY_OUTPUT_BUFFER_LIMIT);
            let mut grown = Vec::with_capacity(new_capacity);
            grown.extend_from_slice(&state.pending);
            state.pending = grown;
            state.buffer_growths = state.buffer_growths.saturating_add(1);
        }
        state.pending.extend_from_slice(bytes);
        debug_assert!(state.pending.capacity() <= PTY_OUTPUT_BUFFER_LIMIT);
        state.high_water_bytes = state.high_water_bytes.max(state.pending.len());
        state.bytes_buffered = state
            .bytes_buffered
            .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        let notify = !self.wake_queued.swap(true, Ordering::AcqRel);
        if notify {
            state.wake_events = state.wake_events.saturating_add(1);
        }
        Ok(AppendResult::Buffered { notify })
    }

    fn drain_into(&self, destination: &mut Vec<u8>) -> Result<usize> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow!("PTY output buffer lock poisoned"))?;
        destination.clear();
        let drained = state.pending.len();
        if drained > 0 {
            if destination.capacity() <= PTY_OUTPUT_BUFFER_LIMIT {
                std::mem::swap(destination, &mut state.pending);
            } else {
                // Do not let an arbitrary public caller inject a multi-megabyte allocation into
                // the bounded producer queue. Oversized caller storage stays caller-owned.
                destination.extend_from_slice(&state.pending);
                state.pending.clear();
            }
            debug_assert!(state.pending.capacity() <= PTY_OUTPUT_BUFFER_LIMIT);
            state.bytes_drained = state
                .bytes_drained
                .saturating_add(u64::try_from(drained).unwrap_or(u64::MAX));
        }
        state.drain_calls = state.drain_calls.saturating_add(1);
        // Rearm while holding the same lock used by the producer. A producer that appends after
        // this point must observe `false` and queue the next wakeup.
        self.wake_queued.store(false, Ordering::Release);
        drop(state);
        self.space_available.notify_one();
        Ok(drained)
    }

    fn metrics(&self) -> Result<PtyBufferMetrics> {
        self.state
            .lock()
            .map(|state| state.metrics())
            .map_err(|_| anyhow!("PTY output buffer lock poisoned"))
    }

    fn close(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.closed = true;
        drop(state);
        self.space_available.notify_all();
    }

    fn is_closed(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .closed
    }
}

#[derive(Clone, Debug)]
pub struct PtyCommand {
    pub program: OsString,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
}

impl PtyCommand {
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            arguments: Vec::new(),
            working_directory: None,
        }
    }

    #[must_use]
    pub fn with_arguments<I, S>(mut self, arguments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.arguments = arguments
            .into_iter()
            .map(|argument| argument.as_ref().to_owned())
            .collect();
        self
    }

    #[must_use]
    pub fn with_working_directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.working_directory = Some(directory.into());
        self
    }
}

#[derive(Clone, Debug)]
pub enum PtyEvent {
    /// Output is available through [`PtySession::take_output`]. Multiple reads are coalesced into
    /// one wakeup until the consumer drains the pending bytes.
    Wake,
    Exit {
        code: u32,
        signal: Option<String>,
    },
    ReadError(String),
}

pub struct PtySession {
    master: Option<PtyMaster>,
    writer: Option<Arc<Mutex<File>>>,
    output: Arc<OutputBuffer>,
    worker: Option<JoinHandle<()>>,
    #[cfg(unix)]
    shutdown_writer: Option<OwnedFd>,
    #[cfg(unix)]
    process_group: ProcessGroup,
    last_size: Mutex<PtySize>,
    resize_requests: AtomicU64,
    resize_ioctls: AtomicU64,
    resize_suppressed: AtomicU64,
    child_pid: Option<u32>,
}

impl std::fmt::Debug for PtySession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PtySession")
            .field("child_pid", &self.child_pid)
            .finish_non_exhaustive()
    }
}

impl PtySession {
    /// Starts the requested child process in a native pseudo-terminal and launches one worker
    /// which reads output, applies bounded backpressure, waits for process exit, and calls
    /// `on_event`.
    ///
    /// # Errors
    ///
    /// Returns an error when the PTY, child process, I/O handles, or worker threads cannot be
    /// created.
    pub fn spawn(
        command: &PtyCommand,
        size: PtySize,
        on_event: impl Fn(PtyEvent) + Send + Sync + 'static,
    ) -> Result<Self> {
        let mut native_command = Command::new(&command.program).args(command.arguments.iter());
        if let Some(directory) = &command.working_directory {
            native_command = native_command.current_dir(directory);
        }
        // macOS ships xterm-256color terminfo, but not xterm-kitty. Advertising a missing
        // terminfo entry breaks `clear`, readline, and fullscreen TUIs before they can negotiate
        // optional protocols such as Kitty keyboard reporting.
        native_command = native_command
            .env("TERM", "xterm-256color")
            .env("COLORTERM", "truecolor")
            .env("TERM_PROGRAM", "Tmon")
            .env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));

        let SpawnedPty { master, mut child } =
            spawn(&native_command, size).context("opening and spawning native PTY")?;
        let mut reader = master.try_clone().context("cloning PTY reader")?;
        #[cfg(unix)]
        let (shutdown_reader, shutdown_writer) = pipe().context("creating PTY shutdown pipe")?;
        let writer = Arc::new(Mutex::new(
            master.try_clone().context("cloning PTY writer")?,
        ));
        let child_pid = Some(child.process_id());
        #[cfg(unix)]
        let process_group = child.process_group();
        let output = Arc::new(OutputBuffer::new());
        let on_event: Arc<dyn Fn(PtyEvent) + Send + Sync> = Arc::new(on_event);

        let worker_events = Arc::clone(&on_event);
        let worker_output = Arc::clone(&output);
        let worker = thread::Builder::new()
            .name("tmon-pty-reader".to_owned())
            .spawn(move || {
                let mut buffer = vec![0_u8; PTY_READ_CHUNK_SIZE];
                let mut read_error = None;
                loop {
                    #[cfg(unix)]
                    let next_read = read_interruptibly(
                        &mut reader,
                        &shutdown_reader,
                        &worker_output,
                        &mut buffer,
                    );
                    #[cfg(not(unix))]
                    let next_read = reader.read(&mut buffer).map(Some);

                    match next_read {
                        Ok(Some(0) | None) => break,
                        Ok(Some(length)) => match worker_output.append(&buffer[..length]) {
                            Ok(AppendResult::Buffered { notify: true }) => {
                                worker_events(PtyEvent::Wake);
                            }
                            Ok(AppendResult::Buffered { notify: false }) => {}
                            Ok(AppendResult::Closed) => break,
                            Err(error) => {
                                read_error = Some(error.to_string());
                                let _ = child.kill();
                                break;
                            }
                        },
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(error) => {
                            read_error = Some(error.to_string());
                            let _ = child.kill();
                            break;
                        }
                    }
                }
                if worker_output.is_closed() {
                    // Session teardown has already signalled the child from the owner thread.
                    let _ = child.wait();
                    return;
                }
                let status = child.wait();
                if let Some(error) = read_error {
                    worker_events(PtyEvent::ReadError(error));
                } else {
                    match status {
                        Ok(status) => worker_events(PtyEvent::Exit {
                            code: status.exit_code(),
                            signal: status.signal().map(str::to_owned),
                        }),
                        Err(error) => worker_events(PtyEvent::ReadError(error.to_string())),
                    }
                }
            })
            .inspect_err(|_| {
                output.close();
                #[cfg(unix)]
                let _ = process_group.terminate();
            })
            .context("spawning PTY reader thread")?;

        Ok(Self {
            master: Some(master),
            writer: Some(writer),
            output,
            worker: Some(worker),
            #[cfg(unix)]
            shutdown_writer: Some(shutdown_writer),
            #[cfg(unix)]
            process_group,
            last_size: Mutex::new(size),
            resize_requests: AtomicU64::new(0),
            resize_ioctls: AtomicU64::new(0),
            resize_suppressed: AtomicU64::new(0),
            child_pid,
        })
    }

    /// Drains all output currently buffered by the PTY reader.
    ///
    /// # Errors
    ///
    /// Returns an error if the shared output buffer lock is poisoned.
    pub fn take_output(&self) -> Result<Vec<u8>> {
        let mut output = Vec::new();
        self.drain_output_into(&mut output)?;
        Ok(output)
    }

    /// Drains buffered PTY output into caller-owned storage.
    ///
    /// `destination` is cleared first. Bounded allocations are swapped into the producer, so
    /// keeping one scratch vector per terminal avoids steady-state allocation and copying between
    /// the PTY worker and application. Oversized caller allocations are never retained by the
    /// engine. The returned byte count equals `destination.len()`.
    ///
    /// # Errors
    ///
    /// Returns an error if the output-buffer lock is poisoned.
    pub fn drain_output_into(&self, destination: &mut Vec<u8>) -> Result<usize> {
        self.output.drain_into(destination)
    }

    /// Returns bounded-buffer counters suitable for performance and stability gates.
    ///
    /// # Errors
    ///
    /// Returns an error if the output-buffer lock is poisoned.
    pub fn buffer_metrics(&self) -> Result<PtyBufferMetrics> {
        self.output.metrics()
    }

    #[must_use]
    pub fn io_metrics(&self) -> PtyIoMetrics {
        PtyIoMetrics {
            resize_requests: self.resize_requests.load(Ordering::Relaxed),
            resize_ioctls: self.resize_ioctls.load(Ordering::Relaxed),
            resize_suppressed: self.resize_suppressed.load(Ordering::Relaxed),
        }
    }

    /// Writes bytes to the child process through the PTY.
    ///
    /// # Errors
    ///
    /// Returns an error if the writer lock is poisoned or the PTY write/flush fails.
    pub fn write(&self, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let writer = self
            .writer
            .as_ref()
            .ok_or_else(|| anyhow!("PTY session is shutting down"))?;
        let mut writer = writer
            .lock()
            .map_err(|_| anyhow!("PTY writer lock poisoned"))?;
        writer.write_all(bytes).context("writing to PTY")?;
        writer.flush().context("flushing PTY writer")
    }

    /// Updates the PTY's character and pixel dimensions.
    ///
    /// # Errors
    ///
    /// Returns an error if the operating system rejects the resize request.
    pub fn resize(&self, size: PtySize) -> Result<()> {
        self.resize_requests.fetch_add(1, Ordering::Relaxed);
        let mut last_size = self
            .last_size
            .lock()
            .map_err(|_| anyhow!("PTY size lock poisoned"))?;
        if *last_size == size {
            self.resize_suppressed.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        let master = self
            .master
            .as_ref()
            .ok_or_else(|| anyhow!("PTY session is shutting down"))?;
        master.resize(size).context("resizing PTY")?;
        *last_size = size;
        self.resize_ioctls.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    #[must_use]
    pub const fn child_pid(&self) -> Option<u32> {
        self.child_pid
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        self.output.close();
        #[cfg(unix)]
        if let Some(shutdown_writer) = &self.shutdown_writer {
            let _ = write(shutdown_writer, &[1]);
        }
        #[cfg(unix)]
        // Hard process-group termination is deterministic and also covers descendants retaining
        // the slave descriptor.
        let _ = self.process_group.terminate();
        // Close session-owned descriptors before joining so a worker blocked in `read` observes
        // teardown promptly even when the child exits abnormally.
        self.writer.take();
        self.master.take();
        if let Some(worker) = self.worker.take()
            && worker.thread().id() != thread::current().id()
        {
            let _ = worker.join();
        }
        #[cfg(unix)]
        self.shutdown_writer.take();
    }
}

#[cfg(unix)]
fn read_interruptibly(
    reader: &mut File,
    shutdown_reader: &OwnedFd,
    output: &OutputBuffer,
    buffer: &mut [u8],
) -> std::io::Result<Option<usize>> {
    loop {
        if output.is_closed() {
            return Ok(None);
        }
        let mut descriptors = [
            PollFd::new(reader.as_fd(), PollFlags::POLLIN),
            PollFd::new(shutdown_reader.as_fd(), PollFlags::POLLIN),
        ];
        match poll(&mut descriptors, PollTimeout::NONE) {
            Ok(_) => {
                if descriptors[1].any().unwrap_or(true) {
                    return Ok(None);
                }
                if descriptors[0].any().unwrap_or(true) {
                    return reader.read(buffer).map(Some);
                }
            }
            Err(Errno::EINTR) => {}
            Err(error) => return Err(std::io::Error::from_raw_os_error(error as i32)),
        }
    }
}

#[must_use]
pub fn pty_size(columns: usize, rows: usize, cell_width: f32, cell_height: f32) -> PtySize {
    let columns = u16::try_from(columns).unwrap_or(u16::MAX).max(2);
    let rows = u16::try_from(rows).unwrap_or(u16::MAX).max(1);
    PtySize {
        cols: columns,
        rows,
        pixel_width: clamp_pixels(f32::from(columns) * cell_width),
        pixel_height: clamp_pixels(f32::from(rows) * cell_height),
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn clamp_pixels(value: f32) -> u16 {
    if value.is_finite() {
        value.round().clamp(0.0, f32::from(u16::MAX)) as u16
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::{AppendResult, OutputBuffer, PTY_OUTPUT_BUFFER_LIMIT};

    #[test]
    fn caller_scratch_capacity_is_recycled_back_to_the_producer() {
        let output = OutputBuffer::new();
        let bytes = vec![b'x'; 32 * 1024];
        assert_eq!(
            output.append(&bytes).expect("first append succeeds"),
            AppendResult::Buffered { notify: true }
        );

        let mut scratch = Vec::with_capacity(PTY_OUTPUT_BUFFER_LIMIT);
        assert_eq!(
            output
                .drain_into(&mut scratch)
                .expect("first drain succeeds"),
            bytes.len()
        );
        assert_eq!(scratch, bytes);
        let after_swap = output.metrics().expect("metrics read succeeds");
        assert!(after_swap.pending_capacity_bytes >= PTY_OUTPUT_BUFFER_LIMIT);
        let growths = after_swap.buffer_growths;

        assert_eq!(
            output.append(&bytes).expect("second append succeeds"),
            AppendResult::Buffered { notify: true }
        );
        let after_reuse = output.metrics().expect("metrics read succeeds");
        assert_eq!(
            after_reuse.buffer_growths, growths,
            "the producer should append into the caller's recycled allocation"
        );
        assert_eq!(after_reuse.high_water_bytes, bytes.len());
    }

    #[test]
    fn oversized_caller_scratch_is_never_retained_by_the_producer() {
        let output = OutputBuffer::new();
        let bytes = vec![b'x'; 32 * 1024];
        output.append(&bytes).expect("append succeeds");

        let mut oversized = Vec::with_capacity(PTY_OUTPUT_BUFFER_LIMIT * 16);
        assert_eq!(
            output
                .drain_into(&mut oversized)
                .expect("drain into oversized caller storage succeeds"),
            bytes.len()
        );
        assert_eq!(oversized, bytes);
        assert!(
            output
                .metrics()
                .expect("metrics read succeeds")
                .pending_capacity_bytes
                <= PTY_OUTPUT_BUFFER_LIMIT
        );

        output.append(&bytes).expect("second append succeeds");
        assert!(
            output
                .metrics()
                .expect("metrics read succeeds")
                .pending_capacity_bytes
                <= PTY_OUTPUT_BUFFER_LIMIT
        );
    }
}
