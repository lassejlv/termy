use std::{
    collections::{BTreeMap, VecDeque},
    ffi::{CString, c_char, c_int, c_short, c_ulong, c_void},
    ffi::{OsStr, OsString},
    fs::File,
    io::{self, ErrorKind, Read, Write},
    os::fd::AsRawFd,
    os::unix::net::UnixStream,
    os::unix::{ffi::OsStrExt, fs::PermissionsExt},
    path::{Path, PathBuf},
    ptr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use super::limits::{
    MAX_PROTOCOL_REPLY_BACKLOG_BYTES, MAX_PROTOCOL_REPLY_BACKLOG_ENTRIES,
    MAX_WRITE_BACKLOG_BYTES as MAX_WRITER_BACKLOG_BYTES,
    MAX_WRITE_BACKLOG_ENTRIES as MAX_WRITER_BACKLOG_ENTRIES,
    MAX_WRITE_CHUNK_BYTES as MAX_WRITER_WRITE_CHUNK,
};
use super::{ChildExit, PtySize, SpawnConfig};

// Platform values come from libc: several differ across Linux architectures
// (for example O_NONBLOCK and SIGCHLD on MIPS).
const F_GETFL: c_int = libc::F_GETFL;
const F_SETFD: c_int = libc::F_SETFD;
const F_SETFL: c_int = libc::F_SETFL;
const FD_CLOEXEC: c_int = libc::FD_CLOEXEC;
const O_NONBLOCK: c_int = libc::O_NONBLOCK;
const EINTR: c_int = libc::EINTR;
const SIGHUP: c_int = libc::SIGHUP;
const SIGKILL: c_int = libc::SIGKILL;
#[cfg(test)]
const SIGPIPE: c_int = libc::SIGPIPE;
#[cfg(test)]
const SIGTERM: c_int = libc::SIGTERM;
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
const SIGNAL_DEFAULT: usize = 0;
const POLLIN: c_short = libc::POLLIN;
const POLLOUT: c_short = libc::POLLOUT;
const POLLERR: c_short = libc::POLLERR;
const POLLHUP: c_short = libc::POLLHUP;
const POLLNVAL: c_short = libc::POLLNVAL;
const CHILD_SPAWN_ERROR_LENGTH: usize = 5;
const EXIT_DRAIN_TIME_BUDGET: Duration = Duration::from_millis(25);
// Give conventional terminal shutdown handlers a brief chance to flush and exit
// before escalating. The watcher remains responsible for reaping the child and
// waking the reader so final output keeps its normal ordering.
const DROP_SHUTDOWN_GRACE: Duration = Duration::from_millis(250);
// Resize, close and protocol replies wake a blocked writer through a socket;
// the timeout is only a backstop against a lost wake.
const WRITER_POLL_TIMEOUT_MS: c_int = 1000;

const FIONREAD: c_ulong = libc::FIONREAD as c_ulong;

#[cfg(any(target_os = "linux", target_os = "android"))]
type PollCount = c_ulong;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
type PollCount = u32;

#[repr(C)]
struct PollFd {
    fd: c_int,
    events: c_short,
    revents: c_short,
}

#[cfg(any(target_os = "linux", target_os = "android"))]
type TerminalInputFlags = u32;
#[cfg(target_os = "macos")]
type TerminalInputFlags = c_ulong;

#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
const IUTF8: TerminalInputFlags = libc::IUTF8 as TerminalInputFlags;
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
const TCSANOW: c_int = 0;
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
const STDIN_FILENO: c_int = 0;

/// Opaque, over-allocated storage for the platform's `struct termios`.
///
/// POSIX places `c_iflag` first, but Darwin and Linux use different flag widths
/// and full structure layouts. Keeping the remainder opaque avoids encoding
/// libc-specific padding while preserving enough size and alignment for both.
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
#[repr(C)]
struct TermiosStorage {
    input_flags: TerminalInputFlags,
    opaque: [u8; 256],
}

const TIOCSWINSZ: c_ulong = libc::TIOCSWINSZ as c_ulong;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WinSize {
    rows: u16,
    cols: u16,
    pixel_width: u16,
    pixel_height: u16,
}

impl From<PtySize> for WinSize {
    fn from(size: PtySize) -> Self {
        Self {
            rows: size.rows,
            cols: size.cols,
            pixel_width: window_pixels(size.cell_width, size.cols),
            pixel_height: window_pixels(size.cell_height, size.rows),
        }
    }
}

/// Pixel extent for `TIOCSWINSZ`; zero tells programs the cell size is unknown.
fn window_pixels(cell: f32, cells: u16) -> u16 {
    if cell > 0.0 {
        cell.mul_add(f32::from(cells), 0.0)
            .round()
            .clamp(1.0, f32::from(u16::MAX)) as u16
    } else {
        0
    }
}

unsafe extern "C" {
    fn chdir(path: *const c_char) -> c_int;
    fn close(fd: c_int) -> c_int;
    fn execve(
        file: *const c_char,
        argv: *const *const c_char,
        environment: *const *const c_char,
    ) -> c_int;
    fn fcntl(fd: c_int, command: c_int, ...) -> c_int;
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
    fn kill(pid: c_int, signal: c_int) -> c_int;
    fn poll(descriptors: *mut PollFd, count: PollCount, timeout: c_int) -> c_int;
    fn tcgetpgrp(fd: c_int) -> c_int;
    #[cfg(target_os = "macos")]
    fn proc_name(pid: c_int, buffer: *mut c_void, size: u32) -> c_int;
    #[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
    fn signal(signal: c_int, handler: usize) -> usize;
    #[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
    fn tcgetattr(fd: c_int, termios: *mut TermiosStorage) -> c_int;
    #[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
    fn tcsetattr(fd: c_int, actions: c_int, termios: *const TermiosStorage) -> c_int;
    fn waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
    #[link_name = "write"]
    fn write_fd(fd: c_int, buffer: *const c_void, count: usize) -> isize;
    fn _exit(status: c_int) -> !;

    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn __errno_location() -> *mut c_int;
    #[cfg(target_os = "macos")]
    fn __error() -> *mut c_int;
}

#[derive(Clone, Copy)]
#[repr(u8)]
enum ChildSpawnStage {
    ChangeDirectory = 1,
    Execute = 2,
    ControllingTerminal = 3,
}

impl ChildSpawnStage {
    fn from_byte(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::ChangeDirectory),
            2 => Some(Self::Execute),
            3 => Some(Self::ControllingTerminal),
            _ => None,
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::ChangeDirectory => "change terminal working directory",
            Self::Execute => "execute terminal program",
            Self::ControllingTerminal => "attach terminal as controlling tty",
        }
    }
}

struct ChildSpawnFailure {
    stage: ChildSpawnStage,
    errno: c_int,
}

pub(crate) struct Transport {
    writer: WriterHandle,
    child_pid: c_int,
    alive: Arc<ChildLiveness>,
    /// A CLOEXEC duplicate of the PTY master, used only for foreground queries.
    control: File,
}

#[derive(Clone)]
struct WriterHandle {
    sender: mpsc::Sender<WriterCommand>,
    control: Arc<WriterControl>,
    protocol_replies: Arc<ProtocolReplyQueue>,
}

struct WriterControl {
    close_requested: AtomicBool,
    wake_pending: AtomicBool,
    write_bytes: AtomicUsize,
    write_entries: AtomicUsize,
    limits: WriterLimits,
    protocol_reply_bytes: AtomicUsize,
    protocol_reply_entries: AtomicUsize,
    protocol_reply_limits: WriterLimits,
    pending_resize: Mutex<Option<WinSize>>,
    // Write end of the socket a writer blocked on POLLOUT also polls.
    poll_wake: std::sync::OnceLock<UnixStream>,
}

#[derive(Clone, Copy)]
struct WriterLimits {
    max_bytes: usize,
    max_entries: usize,
}

enum WriterCommand {
    Write(ReservedWrite),
    Wake(WakeReservation),
}

struct ReservedWrite {
    bytes: Vec<u8>,
    offset: usize,
    _reservation: WriteReservation,
}

struct ProtocolReplyQueue {
    pending: Mutex<VecDeque<ReservedWrite>>,
}

#[derive(Clone, Copy)]
enum WriteClass {
    Normal,
    ProtocolReply,
    /// Mouse motion shares the normal budget but may fill only half of it, so
    /// reports to a stalled program leave room for keys and pastes.
    Motion,
}

struct WriteReservation {
    control: Arc<WriterControl>,
    bytes: usize,
    class: WriteClass,
}

struct WakeReservation {
    control: Arc<WriterControl>,
}

impl WriterHandle {
    fn channel() -> (Self, mpsc::Receiver<WriterCommand>) {
        Self::channel_with_limits(WriterLimits {
            max_bytes: MAX_WRITER_BACKLOG_BYTES,
            max_entries: MAX_WRITER_BACKLOG_ENTRIES,
        })
    }

    fn channel_with_limits(limits: WriterLimits) -> (Self, mpsc::Receiver<WriterCommand>) {
        Self::channel_with_all_limits(
            limits,
            WriterLimits {
                max_bytes: MAX_PROTOCOL_REPLY_BACKLOG_BYTES,
                max_entries: MAX_PROTOCOL_REPLY_BACKLOG_ENTRIES,
            },
        )
    }

    fn channel_with_all_limits(
        limits: WriterLimits,
        protocol_reply_limits: WriterLimits,
    ) -> (Self, mpsc::Receiver<WriterCommand>) {
        let (sender, receiver) = mpsc::channel();
        (
            Self {
                sender,
                control: Arc::new(WriterControl {
                    close_requested: AtomicBool::new(false),
                    wake_pending: AtomicBool::new(false),
                    write_bytes: AtomicUsize::new(0),
                    write_entries: AtomicUsize::new(0),
                    limits,
                    protocol_reply_bytes: AtomicUsize::new(0),
                    protocol_reply_entries: AtomicUsize::new(0),
                    protocol_reply_limits,
                    pending_resize: Mutex::new(None),
                    poll_wake: std::sync::OnceLock::new(),
                }),
                protocol_replies: Arc::new(ProtocolReplyQueue {
                    pending: Mutex::new(VecDeque::new()),
                }),
            },
            receiver,
        )
    }

    fn write_owned(&self, bytes: Vec<u8>) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        if self.control.is_closed() {
            return Err(writer_closed_error());
        }
        let reservation = self
            .control
            .reserve_write(bytes.capacity(), WriteClass::Normal)?;
        self.send_reserved(bytes, reservation)
    }

    fn write_motion_owned(&self, bytes: Vec<u8>) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        if self.control.is_closed() {
            return Err(writer_closed_error());
        }
        let reservation = self
            .control
            .reserve_write(bytes.capacity(), WriteClass::Motion)?;
        self.send_reserved(bytes, reservation)
    }

    fn write(&self, bytes: &[u8]) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        if self.control.is_closed() {
            return Err(writer_closed_error());
        }
        // Reserve before copying borrowed input so an oversized or currently
        // full backlog fails without allocating a payload that cannot queue.
        let mut reservation = self
            .control
            .reserve_write(bytes.len(), WriteClass::Normal)?;
        let owned = bytes.to_vec();
        reservation.grow_to(owned.capacity())?;
        self.send_reserved(owned, reservation)
    }

    fn write_protocol_reply_owned(&self, bytes: Vec<u8>) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        if self.control.is_closed() {
            return Err(writer_closed_error());
        }
        let reservation = self
            .control
            .reserve_write(bytes.capacity(), WriteClass::ProtocolReply)?;
        self.protocol_replies
            .push_if_open(
                &self.control,
                ReservedWrite {
                    bytes,
                    offset: 0,
                    _reservation: reservation,
                },
            )
            .map_err(|write| {
                drop(write);
                writer_closed_error()
            })?;
        self.wake()
    }

    fn send_reserved(&self, bytes: Vec<u8>, reservation: WriteReservation) -> io::Result<()> {
        let command = WriterCommand::Write(ReservedWrite {
            bytes,
            offset: 0,
            _reservation: reservation,
        });
        if let Err(error) = self.sender.send(command) {
            drop(error);
            self.protocol_replies.close(&self.control);
            return Err(writer_closed_error());
        }
        Ok(())
    }

    fn resize(&self, window_size: WinSize) -> io::Result<()> {
        if !self.control.request_resize(window_size) {
            return Err(writer_closed_error());
        }
        self.wake()
    }

    fn close(&self) {
        self.protocol_replies.close(&self.control);
        // The receiver may be sleeping with no writes queued. A wake command
        // makes the sticky close visible without waiting for channel teardown.
        let _ = self.wake();
    }

    fn wake(&self) -> io::Result<()> {
        // A writer blocked on a full PTY is not in recv, so its queued Wake
        // stays pending; the socket interrupts its poll for every request.
        if let Some(stream) = self.control.poll_wake.get() {
            send_signal_byte(stream);
        }
        let Some(reservation) = self.control.reserve_wake() else {
            return Ok(());
        };
        if let Err(error) = self.sender.send(WriterCommand::Wake(reservation)) {
            drop(error);
            self.protocol_replies.close(&self.control);
            return Err(writer_closed_error());
        }
        Ok(())
    }
}

impl WriterControl {
    fn is_closed(&self) -> bool {
        self.close_requested.load(Ordering::Acquire)
    }

    fn request_resize(&self, window_size: WinSize) -> bool {
        if self.is_closed() {
            return false;
        }

        let mut pending_resize = self
            .pending_resize
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.is_closed() {
            return false;
        }
        *pending_resize = Some(window_size);
        true
    }

    fn take_resize(&self) -> Option<WinSize> {
        self.pending_resize
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    fn request_close(&self) {
        self.close_requested.store(true, Ordering::Release);
        // Do not leave stale resize state behind after cancellation. The lock
        // is never held while the writer performs ioctl, write, or poll.
        self.pending_resize
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
    }

    fn reserve_write(
        self: &Arc<Self>,
        bytes: usize,
        class: WriteClass,
    ) -> io::Result<WriteReservation> {
        let (byte_counter, entry_counter, limits) = self.write_budget(class);
        if self.is_closed() || !try_reserve_counter(byte_counter, bytes, limits.max_bytes) {
            return if self.is_closed() {
                Err(writer_closed_error())
            } else {
                Err(self.backlog_full_error(class))
            };
        }
        if !try_reserve_counter(entry_counter, 1, limits.max_entries) {
            byte_counter.fetch_sub(bytes, Ordering::AcqRel);
            return Err(self.backlog_full_error(class));
        }
        if self.is_closed() {
            self.release_write(bytes, class);
            return Err(writer_closed_error());
        }
        Ok(WriteReservation {
            control: self.clone(),
            bytes,
            class,
        })
    }

    fn write_budget(&self, class: WriteClass) -> (&AtomicUsize, &AtomicUsize, WriterLimits) {
        match class {
            WriteClass::Normal => (&self.write_bytes, &self.write_entries, self.limits),
            WriteClass::Motion => (
                &self.write_bytes,
                &self.write_entries,
                WriterLimits {
                    max_bytes: self.limits.max_bytes / 2,
                    max_entries: self.limits.max_entries / 2,
                },
            ),
            WriteClass::ProtocolReply => (
                &self.protocol_reply_bytes,
                &self.protocol_reply_entries,
                self.protocol_reply_limits,
            ),
        }
    }

    fn grow_write_reservation(&self, class: WriteClass, additional: usize) -> io::Result<()> {
        if additional == 0 {
            return Ok(());
        }
        let (byte_counter, _, limits) = self.write_budget(class);
        if try_reserve_counter(byte_counter, additional, limits.max_bytes) {
            Ok(())
        } else {
            Err(self.backlog_full_error(class))
        }
    }

    fn reserve_wake(self: &Arc<Self>) -> Option<WakeReservation> {
        self.wake_pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| WakeReservation {
                control: self.clone(),
            })
    }

    fn release_write(&self, bytes: usize, class: WriteClass) {
        let (byte_counter, entry_counter, _) = self.write_budget(class);
        byte_counter.fetch_sub(bytes, Ordering::AcqRel);
        entry_counter.fetch_sub(1, Ordering::AcqRel);
    }

    fn backlog_full_error(&self, class: WriteClass) -> io::Error {
        let (byte_counter, entry_counter, limits) = self.write_budget(class);
        let bytes = byte_counter.load(Ordering::Acquire);
        let entries = entry_counter.load(Ordering::Acquire);
        let label = match class {
            WriteClass::Normal | WriteClass::Motion => "writer",
            WriteClass::ProtocolReply => "protocol-reply",
        };
        io::Error::new(
            ErrorKind::WouldBlock,
            format!(
                "Termy native PTY {label} backlog is full ({bytes} bytes in {entries} writes; limits are {} bytes and {} writes)",
                limits.max_bytes, limits.max_entries,
            ),
        )
    }

    #[cfg(test)]
    fn backlog(&self) -> (usize, usize) {
        (
            self.write_bytes.load(Ordering::Acquire),
            self.write_entries.load(Ordering::Acquire),
        )
    }

    #[cfg(test)]
    fn protocol_reply_backlog(&self) -> (usize, usize) {
        (
            self.protocol_reply_bytes.load(Ordering::Acquire),
            self.protocol_reply_entries.load(Ordering::Acquire),
        )
    }
}

impl ProtocolReplyQueue {
    fn push_if_open(
        &self,
        control: &WriterControl,
        write: ReservedWrite,
    ) -> Result<(), ReservedWrite> {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if control.is_closed() {
            return Err(write);
        }
        pending.push_back(write);
        Ok(())
    }

    fn pop(&self) -> Option<ReservedWrite> {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop_front()
    }

    fn close(&self, control: &WriterControl) {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        control.request_close();
        pending.clear();
    }
}

impl WriteReservation {
    fn grow_to(&mut self, bytes: usize) -> io::Result<()> {
        let additional = bytes.saturating_sub(self.bytes);
        self.control
            .grow_write_reservation(self.class, additional)?;
        self.bytes += additional;
        Ok(())
    }
}

impl Drop for WriteReservation {
    fn drop(&mut self) {
        self.control.release_write(self.bytes, self.class);
    }
}

impl Drop for WakeReservation {
    fn drop(&mut self) {
        self.control.wake_pending.store(false, Ordering::Release);
    }
}

fn try_reserve_counter(counter: &AtomicUsize, amount: usize, limit: usize) -> bool {
    counter
        .try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            current
                .checked_add(amount)
                .filter(|reserved| *reserved <= limit)
        })
        .is_ok()
}

fn writer_closed_error() -> io::Error {
    io::Error::new(ErrorKind::BrokenPipe, "Termy native PTY is closed")
}

impl Transport {
    // Hosts that answer terminal queries use `spawn_answering_queries`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn spawn(
        config: SpawnConfig,
        size: PtySize,
        on_output: impl FnMut(&[u8]) -> Vec<u8> + Send + 'static,
        on_exit: impl FnOnce() + Send + 'static,
    ) -> io::Result<Self> {
        Self::spawn_with_exit(config, size, true, on_output, move |_| on_exit())
    }

    /// Spawn like [`Self::spawn`] for a host whose `on_output` answers
    /// terminal queries. A Unix PTY needs nothing extra for that. Without
    /// `inherit_environment`, the child sees only `config.environment`.
    pub(crate) fn spawn_answering_queries(
        config: SpawnConfig,
        size: PtySize,
        inherit_environment: bool,
        on_output: impl FnMut(&[u8]) -> Vec<u8> + Send + 'static,
        on_exit: impl FnOnce() + Send + 'static,
    ) -> io::Result<Self> {
        Self::spawn_with_exit(config, size, inherit_environment, on_output, move |_| {
            on_exit();
        })
    }

    /// Spawn like [`Self::spawn`], reporting how the child ended. Without
    /// `inherit_environment`, the child sees only `config.environment`.
    pub(crate) fn spawn_with_exit(
        config: SpawnConfig,
        size: PtySize,
        inherit_environment: bool,
        mut on_output: impl FnMut(&[u8]) -> Vec<u8> + Send + 'static,
        on_exit: impl FnOnce(ChildExit) + Send + 'static,
    ) -> io::Result<Self> {
        let program_path = resolve_program(&config)?;
        let program = c_string_os(program_path.as_os_str(), "terminal program")?;
        let mut arguments = Vec::with_capacity(config.args.len() + 1);
        arguments.push(c_string(&config.program, "terminal program")?);
        for argument in &config.args {
            arguments.push(c_string(argument, "terminal argument")?);
        }
        let mut argv = arguments
            .iter()
            .map(|argument| argument.as_ptr())
            .collect::<Vec<_>>();
        argv.push(ptr::null());

        let working_directory = config
            .working_directory
            .as_ref()
            .map(|path| c_string_os(path.as_os_str(), "working directory"))
            .transpose()?;
        let environment = child_environment(
            &config.environment,
            config.working_directory.as_deref(),
            inherit_environment,
        )?;
        let mut environment_pointers = environment
            .iter()
            .map(|entry| entry.as_ptr())
            .collect::<Vec<_>>();
        environment_pointers.push(ptr::null());

        let (mut child_status_reader, child_status_writer) = UnixStream::pair()?;
        // A successful exec closes the child endpoint and reports EOF to the
        // parent. Failures write a small stage + errno record before _exit.
        if unsafe { fcntl(child_status_writer.as_raw_fd(), F_SETFD, FD_CLOEXEC) } < 0 {
            return Err(io::Error::last_os_error());
        }

        let (master_file, user) = open_pty_pair(WinSize::from(size))?;
        let mut descriptors = InheritedDescriptors::new();
        // SAFETY: all pointers refer to parent-owned allocations that remain valid across
        // fork. The child only invokes libc functions before replacing its image.
        // Signals stay blocked across fork so no parent handler runs in the
        // child before its dispositions are reset; the child clears the mask.
        let parent_mask = block_all_signals();
        let pid = unsafe { libc::fork() };
        if pid != 0 {
            restore_signal_mask(&parent_mask);
        }
        if pid < 0 {
            return Err(io::Error::last_os_error());
        }

        if pid == 0 {
            // SAFETY: the child does not use the parent's status endpoint.
            unsafe {
                let _ = close(child_status_reader.as_raw_fd());
            }
            // SAFETY: the child owns these descriptors and only issues
            // async-signal-safe syscalls before exec.
            let status_fd = match unsafe {
                attach_child_terminal(
                    user.as_raw_fd(),
                    child_status_writer.as_raw_fd(),
                    &mut descriptors,
                )
            } {
                Ok(fd) => fd,
                Err(fd) => unsafe {
                    report_child_spawn_failure(fd, ChildSpawnStage::ControllingTerminal);
                    _exit(126);
                },
            };
            reset_child_process_state();
            if let Some(directory) = &working_directory {
                // SAFETY: `directory` is a valid, NUL-terminated C string.
                unsafe {
                    if chdir(directory.as_ptr()) < 0 {
                        report_child_spawn_failure(status_fd, ChildSpawnStage::ChangeDirectory);
                        _exit(126);
                    }
                }
            }
            // SAFETY: `argv` and `environment_pointers` are NUL-terminated and all
            // pointed-to strings remain alive. execve is async-signal-safe after fork.
            unsafe {
                let _ = execve(
                    program.as_ptr(),
                    argv.as_ptr(),
                    environment_pointers.as_ptr(),
                );
                report_child_spawn_failure(status_fd, ChildSpawnStage::Execute);
                _exit(127);
            }
        }

        drop(child_status_writer);
        drop(user);
        match read_child_spawn_status(&mut child_status_reader) {
            Ok(None) => {}
            Ok(Some(failure)) => {
                drop(master_file);
                wait_for_child(pid);
                return Err(failure.into_io_error());
            }
            Err(error) => {
                drop(master_file);
                terminate_and_reap(pid);
                return Err(error);
            }
        }
        drop(child_status_reader);
        if let Err(error) = set_nonblocking(&master_file) {
            drop(master_file);
            terminate_and_reap(pid);
            return Err(error);
        }
        let (mut reader, control) = match master_file
            .try_clone()
            .and_then(|reader| Ok((reader, master_file.try_clone()?)))
        {
            Ok(files) => files,
            Err(error) => {
                drop(master_file);
                terminate_and_reap(pid);
                return Err(error);
            }
        };
        let (writer, writer_rx) = WriterHandle::channel();
        let writer_control = writer.control.clone();
        let writer_protocol_replies = writer.protocol_replies.clone();
        let (writer_wake, wake_sender) = match signal_socket_pair().and_then(|(reader, sender)| {
            reader.set_nonblocking(true)?;
            sender.set_nonblocking(true)?;
            Ok((reader, sender))
        }) {
            Ok(pair) => pair,
            Err(error) => {
                drop(reader);
                terminate_and_reap(pid);
                return Err(error);
            }
        };
        let _ = writer.control.poll_wake.set(wake_sender);
        let writer_thread = thread::Builder::new()
            .name("termy-native-pty-writer".to_string())
            .spawn(move || {
                run_pty_writer(
                    master_file,
                    writer_rx,
                    writer_control,
                    writer_protocol_replies,
                    writer_wake,
                );
            });
        if let Err(error) = writer_thread {
            drop(reader);
            terminate_and_reap(pid);
            return Err(error);
        }

        let (mut shutdown_reader, shutdown_writer) = match signal_socket_pair() {
            Ok(pair) => pair,
            Err(error) => {
                writer.close();
                terminate_and_reap(pid);
                return Err(error);
            }
        };
        let reader_writer = writer.clone();
        let alive = Arc::new(ChildLiveness::new());
        let reader_alive = alive.clone();
        let exit_status = Arc::new(Mutex::new(None::<ChildExit>));
        let reader_exit_status = exit_status.clone();

        let reader_thread = thread::Builder::new()
            .name("termy-native-pty".to_string())
            .spawn(move || {
                let mut buffer = [0u8; 32 * 1024];
                let mut child_wakeup = false;
                let mut descriptors = [
                    PollFd {
                        fd: reader.as_raw_fd(),
                        events: POLLIN,
                        revents: 0,
                    },
                    PollFd {
                        fd: shutdown_reader.as_raw_fd(),
                        events: POLLIN,
                        revents: 0,
                    },
                ];
                loop {
                    descriptors[0].revents = 0;
                    descriptors[1].revents = 0;
                    // SAFETY: both descriptors remain owned by this thread for the
                    // duration of the call, and the array has exactly two entries.
                    let ready = unsafe {
                        poll(descriptors.as_mut_ptr(), descriptors.len() as PollCount, -1)
                    };
                    if ready < 0 {
                        if io::Error::last_os_error().kind() == ErrorKind::Interrupted {
                            continue;
                        }
                        break;
                    }

                    let shutdown_events = descriptors[1].revents;
                    if shutdown_events & (POLLIN | POLLERR | POLLHUP | POLLNVAL) != 0 {
                        child_wakeup = true;
                        drain_ready_pty_output(&mut reader, &mut buffer, &mut on_output);
                        break;
                    }

                    let reader_events = descriptors[0].revents;
                    if reader_events & POLLIN == 0 {
                        if reader_events & (POLLERR | POLLHUP | POLLNVAL) != 0 {
                            drain_ready_pty_output(&mut reader, &mut buffer, &mut on_output);
                            break;
                        }
                        continue;
                    }
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(read) => {
                            let reply = on_output(&buffer[..read]);
                            queue_reader_protocol_reply(&reader_writer, reply);
                        }
                        Err(error) if error.kind() == ErrorKind::Interrupted => {}
                        Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                        // Linux PTYs commonly report EIO when the slave closes.
                        Err(error) if error.raw_os_error() == Some(5) => break,
                        Err(_) => break,
                    }
                }
                drop(reader);
                reader_writer.close();
                if !child_wakeup {
                    let mut wake = [0u8; 1];
                    while shutdown_reader
                        .read(&mut wake)
                        .is_err_and(|error| error.kind() == ErrorKind::Interrupted)
                    {
                    }
                }
                if !reader_alive.is_alive() {
                    let exit = reader_exit_status
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take()
                        .unwrap_or_default();
                    on_exit(exit);
                }
            });
        if let Err(error) = reader_thread {
            writer.close();
            terminate_and_reap(pid);
            return Err(error);
        }

        let watcher_alive = alive.clone();
        let watcher_thread = thread::Builder::new()
            .name("termy-native-pty-child".to_string())
            .spawn(move || {
                // The child is known to be gone, but not yet reaped, before
                // it stops taking signals; see `ChildLiveness`.
                let exited = wait_for_child_exit(pid);
                if exited {
                    watcher_alive.exited();
                }
                let status = wait_for_child(pid);
                *exit_status
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                    Some(status.map(child_exit_from_wait_status).unwrap_or_default());
                if !exited {
                    watcher_alive.exited();
                }
                send_signal_byte(&shutdown_writer);
            });
        if let Err(error) = watcher_thread {
            writer.close();
            terminate_and_reap(pid);
            return Err(error);
        }

        Ok(Self {
            writer,
            child_pid: pid,
            alive,
            control,
        })
    }

    pub(crate) fn write(&self, input: &[u8]) -> io::Result<()> {
        self.writer.write(input)
    }

    pub(crate) fn write_owned(&self, input: Vec<u8>) -> io::Result<()> {
        self.writer.write_owned(input)
    }

    /// Queue a mouse-motion report only while the input backlog is at most
    /// half full; `WouldBlock` means it was dropped (the next report carries
    /// the newer position).
    pub(crate) fn write_motion_owned(&self, input: Vec<u8>) -> io::Result<()> {
        self.writer.write_motion_owned(input)
    }

    pub(crate) fn write_protocol_reply_owned(&self, input: Vec<u8>) -> io::Result<()> {
        match self.writer.write_protocol_reply_owned(input) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::WouldBlock => Err(error),
            // As for the reader's replies: drop's SIGHUP grace and escalation
            // own the child's shutdown, so a reply the closed writer rejects
            // never kills it outright.
            Err(error) => {
                self.writer.close();
                Err(error)
            }
        }
    }

    pub(crate) fn resize(&self, size: PtySize) -> io::Result<()> {
        self.writer.resize(WinSize::from(size))
    }

    pub(crate) fn child_pid(&self) -> u32 {
        self.child_pid as u32
    }

    /// Ends the child like dropping the transport does, but waits for it:
    /// hangs up its process group, kills the group if the child still runs
    /// after [`DROP_SHUTDOWN_GRACE`], and returns once the watcher has reaped
    /// it (or after a further second). For a process about to exit, whose
    /// escalation timer would die with it.
    pub(crate) fn terminate_and_wait(&self) {
        let pid = self.child_pid;
        if self
            .alive
            .signal_if_alive(|| signal_child_shutdown(pid))
            .is_none()
        {
            return;
        }
        let wait = |timeout: Duration| {
            let deadline = Instant::now() + timeout;
            while self.alive.is_alive() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(5));
            }
        };
        wait(DROP_SHUTDOWN_GRACE);
        self.alive.signal_if_alive(|| force_child_shutdown(pid));
        wait(Duration::from_secs(1));
    }

    /// Send `signal` to the child process while it is running.
    pub(crate) fn signal(&self, signal: i32) -> io::Result<()> {
        let sent = self.alive.signal_if_alive(|| {
            // SAFETY: `child_pid` is the exact child created by forkpty; the
            // watcher has not reaped it while it is alive (see `ChildLiveness`).
            if unsafe { kill(self.child_pid, signal) } < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
        match sent {
            // ESRCH: the child exited before the watcher noticed.
            Some(Err(error)) if error.raw_os_error() != Some(3) => Err(error),
            _ => Ok(()),
        }
    }

    /// Name of the PTY's foreground process group leader, such as `vim`.
    pub(crate) fn foreground_process_name(&self) -> Option<String> {
        // SAFETY: `control` is an owned, open descriptor for the PTY master.
        let group = unsafe { tcgetpgrp(self.control.as_raw_fd()) };
        if group <= 0 {
            return None;
        }
        process_name(group)
    }
}

/// Decode a `waitpid` status. Stopped children are not reported because the
/// watcher waits without `WUNTRACED`.
fn child_exit_from_wait_status(status: c_int) -> ChildExit {
    let signal = status & 0x7f;
    if signal == 0 {
        ChildExit {
            code: Some((status >> 8) & 0xff),
            signal: None,
        }
    } else {
        ChildExit {
            code: None,
            signal: Some(signal),
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn process_name(pid: c_int) -> Option<String> {
    let name = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let name = name.trim_end_matches('\n');
    (!name.is_empty()).then(|| name.to_owned())
}

#[cfg(target_os = "macos")]
fn process_name(pid: c_int) -> Option<String> {
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer is writable for its full, stated length.
    let length = unsafe { proc_name(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    let length = usize::try_from(length).ok().filter(|length| *length > 0)?;
    Some(String::from_utf8_lossy(&buffer[..length.min(buffer.len())]).into_owned())
}

fn set_nonblocking(file: &File) -> io::Result<()> {
    // SAFETY: `file` owns a live descriptor and F_GETFL takes no variadic argument.
    let flags = unsafe { fcntl(file.as_raw_fd(), F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }

    // O_NONBLOCK is an open-file status flag, so the reader clone intentionally
    // shares it with this descriptor. Both read and write paths handle WouldBlock.
    // SAFETY: `file` owns a live descriptor and `flags | O_NONBLOCK` preserves
    // all existing open-file status flags.
    if unsafe { fcntl(file.as_raw_fd(), F_SETFL, flags | O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn run_pty_writer(
    master: File,
    receiver: mpsc::Receiver<WriterCommand>,
    control: Arc<WriterControl>,
    protocol_replies: Arc<ProtocolReplyQueue>,
    wake: UnixStream,
) {
    let _ = run_pty_writer_loop(
        master,
        receiver,
        control,
        protocol_replies,
        |master: &File| wait_for_pty_writable(master, &wake),
        resize_pty_master,
    );
}

fn run_pty_writer_loop<W, WaitWritable, Resize>(
    mut writer: W,
    receiver: mpsc::Receiver<WriterCommand>,
    control: Arc<WriterControl>,
    protocol_replies: Arc<ProtocolReplyQueue>,
    mut wait_writable: WaitWritable,
    mut resize: Resize,
) -> W
where
    W: Write,
    WaitWritable: FnMut(&W) -> io::Result<()>,
    Resize: FnMut(&W, WinSize) -> io::Result<()>,
{
    // Keep exactly one reserved write outside the channel. All later accepted
    // writes remain in std's FIFO, while their admission reservations bound
    // both memory and entry count without making control work scan the backlog.
    let mut current_write: Option<ReservedWrite> = None;
    let mut current_protocol_reply: Option<ReservedWrite> = None;

    loop {
        if !service_writer_control(&writer, &control, &mut resize) {
            break;
        }

        // Replies go between writes, never inside one: a write is a whole
        // key sequence or bracketed paste that must reach the child intact.
        if current_protocol_reply.is_none()
            && current_write.as_ref().is_none_or(|write| write.offset == 0)
        {
            current_protocol_reply = protocol_replies.pop();
        }

        if current_protocol_reply.is_none() && current_write.is_none() {
            let Ok(command) = receiver.recv() else {
                break;
            };
            match command {
                WriterCommand::Write(write) => current_write = Some(write),
                // Dropping the token before servicing control permits a resize
                // published concurrently with this service pass to queue the
                // next (and only next) wake instead of being stranded.
                WriterCommand::Wake(reservation) => drop(reservation),
            }

            // Resize and Close are published before their Wake command. Check
            // shared control state before writing even when recv returned an
            // older Write that was already ahead of that wake in the channel.
            if !service_writer_control(&writer, &control, &mut resize) {
                break;
            }
        }

        // A reply is taken again after recv because the Wake that made the
        // receiver runnable can race with the first priority-queue check.
        if current_protocol_reply.is_none()
            && current_write.as_ref().is_none_or(|write| write.offset == 0)
        {
            current_protocol_reply = protocol_replies.pop();
        }
        let writing_protocol_reply = current_protocol_reply.is_some();
        let Some(write) = current_protocol_reply.as_mut().or(current_write.as_mut()) else {
            continue;
        };
        let end = write
            .offset
            .saturating_add(MAX_WRITER_WRITE_CHUNK)
            .min(write.bytes.len());
        match writer.write(&write.bytes[write.offset..end]) {
            Ok(0) => break,
            Ok(written) => {
                write.offset += written;
                if write.offset == write.bytes.len() {
                    if writing_protocol_reply {
                        current_protocol_reply = None;
                    } else {
                        current_write = None;
                    }
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                if wait_writable(&writer).is_err() {
                    break;
                }
                // poll returns for readiness, a wake, or its backstop timeout;
                // service controls as soon as it does.
                if !service_writer_control(&writer, &control, &mut resize) {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    protocol_replies.close(&control);
    writer
}

/// Apply a pending resize; false once the writer is closed.
fn service_writer_control<W>(
    writer: &W,
    control: &WriterControl,
    resize: &mut impl FnMut(&W, WinSize) -> io::Result<()>,
) -> bool {
    if control.is_closed() {
        return false;
    }

    let pending_resize = control.take_resize();
    if control.is_closed() {
        return false;
    }
    // A size the PTY rejects keeps the old one; input must still flow.
    if let Some(window_size) = pending_resize
        && let Err(error) = resize(writer, window_size)
    {
        log::warn!("failed to resize the terminal PTY: {error}");
    }
    !control.is_closed()
}

impl ChildSpawnFailure {
    fn into_io_error(self) -> io::Error {
        let source = io::Error::from_raw_os_error(self.errno);
        io::Error::new(
            source.kind(),
            format!("failed to {}: {source}", self.stage.description()),
        )
    }
}

fn read_child_spawn_status(
    status_reader: &mut UnixStream,
) -> io::Result<Option<ChildSpawnFailure>> {
    let mut record = [0u8; CHILD_SPAWN_ERROR_LENGTH];
    let mut filled = 0;
    loop {
        match status_reader.read(&mut record[filled..]) {
            Ok(0) if filled == 0 => return Ok(None),
            Ok(0) => {
                return Err(io::Error::new(
                    ErrorKind::UnexpectedEof,
                    "terminal child returned an incomplete spawn status",
                ));
            }
            Ok(read) => {
                filled += read;
                if filled == record.len() {
                    break;
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }

    let stage = ChildSpawnStage::from_byte(record[0]).ok_or_else(|| {
        io::Error::new(
            ErrorKind::InvalidData,
            "terminal child returned an invalid spawn stage",
        )
    })?;
    let errno = c_int::from_ne_bytes([record[1], record[2], record[3], record[4]]);
    Ok(Some(ChildSpawnFailure { stage, errno }))
}

fn report_child_spawn_failure(status_fd: c_int, stage: ChildSpawnStage) {
    let errno = current_errno();
    let mut record = [0u8; CHILD_SPAWN_ERROR_LENGTH];
    record[0] = stage as u8;
    record[1..].copy_from_slice(&errno.to_ne_bytes());

    let mut written = 0;
    while written < record.len() {
        // SAFETY: `status_fd` is the child endpoint of the status socket, and
        // the remaining record bytes stay valid for the duration of the call.
        let result = unsafe {
            write_fd(
                status_fd,
                record.as_ptr().add(written).cast::<c_void>(),
                record.len() - written,
            )
        };
        if result > 0 {
            written += result as usize;
        } else if result == 0 || current_errno() != EINTR {
            break;
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn current_errno() -> c_int {
    // SAFETY: libc returns a valid pointer to this thread's errno slot.
    unsafe { *__errno_location() }
}

#[cfg(target_os = "macos")]
fn current_errno() -> c_int {
    // SAFETY: libc returns a valid pointer to this thread's errno slot.
    unsafe { *__error() }
}

fn drain_ready_pty_output(
    reader: &mut File,
    buffer: &mut [u8],
    on_output: &mut impl FnMut(&[u8]) -> Vec<u8>,
) {
    // Snapshot the queue at direct-child exit. Consuming at least this many
    // bytes preserves everything that was already buffered even when the
    // callback itself is slow. After that snapshot is consumed, a wall-clock
    // budget prevents a descendant that keeps writing from delaying exit
    // forever. Every readiness check remains nonblocking.
    let mut buffered_at_exit = readable_byte_count(reader).unwrap_or(0);
    let drain_started = Instant::now();
    loop {
        if buffered_at_exit == 0 && drain_started.elapsed() >= EXIT_DRAIN_TIME_BUDGET {
            return;
        }

        let mut descriptor = PollFd {
            fd: reader.as_raw_fd(),
            events: POLLIN,
            revents: 0,
        };
        // SAFETY: `descriptor` contains the reader's live fd and one entry is
        // passed to poll. A zero timeout never waits for descendant activity.
        let ready = unsafe { poll(&mut descriptor, 1 as PollCount, 0) };
        if ready <= 0
            || descriptor.revents & POLLNVAL != 0
            || descriptor.revents & (POLLIN | POLLERR | POLLHUP) == 0
        {
            return;
        }

        match reader.read(buffer) {
            Ok(0) => return,
            Ok(read) => {
                buffered_at_exit = buffered_at_exit.saturating_sub(read);
                // The direct child has already exited, so there is no peer to
                // consume a generated response. Keep draining its final output.
                drop(on_output(&buffer[..read]));
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) if error.kind() == ErrorKind::WouldBlock => return,
            // Linux PTYs commonly report EIO when the slave closes.
            Err(error) if error.raw_os_error() == Some(5) => return,
            Err(_) => return,
        }
    }
}

fn queue_reader_protocol_reply(writer: &WriterHandle, reply: Vec<u8>) {
    if reply.is_empty() {
        return;
    }
    match writer.write_protocol_reply_owned(reply) {
        Ok(()) => {}
        // A full backlog means the child is not reading its input, e.g. `cat`
        // of a file full of queries. Drop the reply instead of the session.
        Err(error) if error.kind() == ErrorKind::WouldBlock => {
            log::warn!("dropping terminal protocol reply: {error}");
        }
        // The writer only closes during drop or after the child is gone. Drop's
        // SIGHUP grace and escalation own the shutdown, so a query printed by
        // a HUP handler must not turn into an immediate SIGKILL.
        Err(_) => {}
    }
}

/// A hint only: the macOS master always reports 0, and Linux counts just the
/// line-discipline buffer. The exit drain stays correct through its time
/// budget, and macOS blocks the exiting child in `close()` until its output is
/// read, so nothing is lost.
fn readable_byte_count(reader: &File) -> Option<usize> {
    let mut readable: c_int = 0;
    // SAFETY: `reader` owns a live descriptor and `readable` is writable for
    // the platform's FIONREAD integer result for the duration of this call.
    if unsafe { ioctl(reader.as_raw_fd(), FIONREAD, &mut readable) } < 0 || readable < 0 {
        None
    } else {
        Some(readable as usize)
    }
}

#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
fn reset_child_process_state() {
    // Reset inherited process state for the native PTY child. `signal`, `tcgetattr`, and
    // `tcsetattr` are async-signal-safe, and this code performs no allocation
    // or locking between forkpty and execve.
    unsafe {
        // Ignored signals survive exec (e.g. SIGTSTP/SIGTTOU from a job-control
        // parent, or SIGHUP under nohup), so reset every disposition first.
        for signal_number in 1..=MAX_RESET_SIGNAL {
            if signal_number != SIGKILL && signal_number != libc::SIGSTOP {
                let _ = signal(signal_number, SIGNAL_DEFAULT);
            }
        }
        // The forking thread's blocked mask survives exec; shells expect none.
        let mut unblocked = std::mem::zeroed::<libc::sigset_t>();
        libc::sigemptyset(&mut unblocked);
        let _ = libc::sigprocmask(libc::SIG_SETMASK, &unblocked, ptr::null_mut());

        let mut termios = TermiosStorage {
            input_flags: 0,
            opaque: [0; 256],
        };
        if tcgetattr(STDIN_FILENO, &mut termios) == 0 {
            termios.input_flags |= IUTF8;
            let _ = tcsetattr(STDIN_FILENO, TCSANOW, &termios);
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
fn reset_child_process_state() {}

impl Drop for Transport {
    fn drop(&mut self) {
        // SIGHUP mirrors closing a conventional terminal session.
        let child_was_alive = self
            .alive
            .signal_if_alive(|| signal_child_shutdown(self.child_pid))
            .is_some();
        self.writer.close();
        if child_was_alive {
            schedule_child_shutdown_escalation(self.child_pid, self.alive.clone());
        }
    }
}

include!("unix/launch.rs");
include!("unix/child.rs");
include!("unix/master.rs");
include!("unix/shutdown.rs");

#[cfg(test)]
#[path = "unix_tests/mod.rs"]
mod tests;
