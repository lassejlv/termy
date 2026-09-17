use std::{
    collections::{BTreeMap, VecDeque},
    ffi::{CString, c_char, c_int, c_short, c_ulong, c_void},
    ffi::{OsStr, OsString},
    fs::File,
    io::{self, ErrorKind, Read, Write},
    os::fd::{AsRawFd, FromRawFd},
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

use crate::tmon::Size;
use crate::tmon::pty_limits::{
    MAX_PROTOCOL_REPLY_BACKLOG_BYTES, MAX_PROTOCOL_REPLY_BACKLOG_ENTRIES,
    MAX_WRITE_BACKLOG_BYTES as MAX_WRITER_BACKLOG_BYTES,
    MAX_WRITE_BACKLOG_ENTRIES as MAX_WRITER_BACKLOG_ENTRIES,
    MAX_WRITE_CHUNK_BYTES as MAX_WRITER_WRITE_CHUNK,
};

const F_GETFL: c_int = 3;
const F_SETFD: c_int = 2;
const F_SETFL: c_int = 4;
const FD_CLOEXEC: c_int = 1;
#[cfg(any(target_os = "linux", target_os = "android"))]
const O_NONBLOCK: c_int = 0x0800;
#[cfg(target_os = "macos")]
const O_NONBLOCK: c_int = 0x0004;
const EINTR: c_int = 4;
const SIGHUP: c_int = 1;
const SIGKILL: c_int = 9;
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
const SIGINT: c_int = 2;
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
const SIGQUIT: c_int = 3;
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
const SIGALRM: c_int = 14;
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
const SIGTERM: c_int = 15;
#[cfg(any(target_os = "linux", target_os = "android"))]
const SIGCHLD: c_int = 17;
#[cfg(target_os = "macos")]
const SIGCHLD: c_int = 20;
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
const SIGNAL_DEFAULT: usize = 0;
const POLLIN: c_short = 0x0001;
const POLLOUT: c_short = 0x0004;
const POLLERR: c_short = 0x0008;
const POLLHUP: c_short = 0x0010;
const POLLNVAL: c_short = 0x0020;
const CHILD_SPAWN_ERROR_LENGTH: usize = 5;
const EXIT_DRAIN_TIME_BUDGET: Duration = Duration::from_millis(25);
// Give conventional terminal shutdown handlers a brief chance to flush and exit
// before escalating. The watcher remains responsible for reaping the child and
// waking the reader so final output keeps its normal ordering.
const DROP_SHUTDOWN_GRACE: Duration = Duration::from_millis(250);
const WRITER_POLL_TIMEOUT_MS: c_int = 10;

#[cfg(any(target_os = "linux", target_os = "android"))]
const FIONREAD: c_ulong = 0x541b;
#[cfg(target_os = "macos")]
const FIONREAD: c_ulong = 0x4004_667f;

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
const IUTF8: TerminalInputFlags = 0x0000_4000;
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

#[cfg(any(target_os = "linux", target_os = "android"))]
const TIOCSWINSZ: c_ulong = 0x5414;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
const TIOCSWINSZ: c_ulong = 0x8008_7467;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WinSize {
    rows: u16,
    cols: u16,
    pixel_width: u16,
    pixel_height: u16,
}

impl From<Size> for WinSize {
    fn from(size: Size) -> Self {
        Self {
            rows: size.rows,
            cols: size.cols,
            pixel_width: size
                .cell_width
                .mul_add(f32::from(size.cols), 0.0)
                .round()
                .clamp(1.0, f32::from(u16::MAX)) as u16,
            pixel_height: size
                .cell_height
                .mul_add(f32::from(size.rows), 0.0)
                .round()
                .clamp(1.0, f32::from(u16::MAX)) as u16,
        }
    }
}

#[cfg_attr(not(target_os = "android"), link(name = "util"))]
unsafe extern "C" {
    fn forkpty(
        master: *mut c_int,
        name: *mut c_char,
        termios: *const c_void,
        size: *const WinSize,
    ) -> c_int;
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
}

impl ChildSpawnStage {
    fn from_byte(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::ChangeDirectory),
            2 => Some(Self::Execute),
            _ => None,
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::ChangeDirectory => "change terminal working directory",
            Self::Execute => "execute terminal program",
        }
    }
}

struct ChildSpawnFailure {
    stage: ChildSpawnStage,
    errno: c_int,
}

#[derive(Clone, Debug)]
pub(crate) struct SpawnConfig {
    pub(crate) program: String,
    pub(crate) args: Vec<String>,
    pub(crate) working_directory: Option<PathBuf>,
    pub(crate) environment: Vec<(String, String)>,
}

pub(crate) struct Pty {
    writer: WriterHandle,
    child_pid: c_int,
    alive: Arc<AtomicBool>,
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
            WriteClass::Normal => "writer",
            WriteClass::ProtocolReply => "protocol-reply",
        };
        io::Error::new(
            ErrorKind::WouldBlock,
            format!(
                "tmon PTY {label} backlog is full ({bytes} bytes in {entries} writes; limits are {} bytes and {} writes)",
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
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            current
                .checked_add(amount)
                .filter(|reserved| *reserved <= limit)
        })
        .is_ok()
}

fn writer_closed_error() -> io::Error {
    io::Error::new(ErrorKind::BrokenPipe, "tmon PTY is closed")
}

impl Pty {
    pub(crate) fn spawn(
        config: SpawnConfig,
        size: Size,
        mut on_output: impl FnMut(&[u8]) -> Vec<u8> + Send + 'static,
        on_exit: impl FnOnce() + Send + 'static,
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
        let environment =
            child_environment(&config.environment, config.working_directory.as_deref())?;
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

        let mut master = -1;
        let window_size = WinSize::from(size);
        // SAFETY: all pointers refer to parent-owned allocations that remain valid across
        // fork. The child only invokes libc functions before replacing its image.
        let pid = unsafe { forkpty(&mut master, ptr::null_mut(), ptr::null(), &window_size) };
        if pid < 0 {
            return Err(io::Error::last_os_error());
        }

        if pid == 0 {
            // SAFETY: the child does not use the parent's status endpoint.
            unsafe {
                let _ = close(child_status_reader.as_raw_fd());
            }
            reset_child_process_state();
            if let Some(directory) = &working_directory {
                // SAFETY: `directory` is a valid, NUL-terminated C string.
                unsafe {
                    if chdir(directory.as_ptr()) < 0 {
                        report_child_spawn_failure(
                            child_status_writer.as_raw_fd(),
                            ChildSpawnStage::ChangeDirectory,
                        );
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
                report_child_spawn_failure(
                    child_status_writer.as_raw_fd(),
                    ChildSpawnStage::Execute,
                );
                _exit(127);
            }
        }

        drop(child_status_writer);
        // SAFETY: `master` is a new, owned file descriptor in the parent branch.
        let master_file = unsafe { File::from_raw_fd(master) };
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
        // SAFETY: fcntl is called with an owned, valid descriptor and an integer flag.
        if unsafe { fcntl(master, F_SETFD, FD_CLOEXEC) } < 0 {
            let error = io::Error::last_os_error();
            drop(master_file);
            terminate_and_reap(pid);
            return Err(error);
        }
        if let Err(error) = set_nonblocking(&master_file) {
            drop(master_file);
            terminate_and_reap(pid);
            return Err(error);
        }
        let mut reader = match master_file.try_clone() {
            Ok(reader) => reader,
            Err(error) => {
                drop(master_file);
                terminate_and_reap(pid);
                return Err(error);
            }
        };
        let (writer, writer_rx) = WriterHandle::channel();
        let writer_control = writer.control.clone();
        let writer_protocol_replies = writer.protocol_replies.clone();
        let writer_thread = thread::Builder::new()
            .name("tmon-pty-writer".to_string())
            .spawn(move || {
                run_pty_writer(
                    master_file,
                    writer_rx,
                    writer_control,
                    writer_protocol_replies,
                );
            });
        if let Err(error) = writer_thread {
            drop(reader);
            terminate_and_reap(pid);
            return Err(error);
        }

        let (mut shutdown_reader, mut shutdown_writer) = match UnixStream::pair() {
            Ok(pair) => pair,
            Err(error) => {
                writer.close();
                terminate_and_reap(pid);
                return Err(error);
            }
        };
        let reader_writer = writer.clone();
        let alive = Arc::new(AtomicBool::new(true));
        let reader_alive = alive.clone();

        let reader_thread = thread::Builder::new()
            .name("tmon-pty".to_string())
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
                            if !queue_reader_protocol_reply(&reader_writer, reply) {
                                force_child_shutdown(pid);
                                break;
                            }
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
                if !reader_alive.load(Ordering::Acquire) {
                    on_exit();
                }
            });
        if let Err(error) = reader_thread {
            writer.close();
            terminate_and_reap(pid);
            return Err(error);
        }

        let watcher_alive = alive.clone();
        let watcher_thread = thread::Builder::new()
            .name("tmon-pty-child".to_string())
            .spawn(move || {
                wait_for_child(pid);
                watcher_alive.store(false, Ordering::Release);
                let _ = shutdown_writer.write(&[1]);
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
        })
    }

    pub(crate) fn write(&self, input: &[u8]) -> io::Result<()> {
        self.writer.write(input)
    }

    pub(crate) fn write_owned(&self, input: Vec<u8>) -> io::Result<()> {
        self.writer.write_owned(input)
    }

    pub(crate) fn write_protocol_reply_owned(&self, input: Vec<u8>) -> io::Result<()> {
        match self.writer.write_protocol_reply_owned(input) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.writer.close();
                if self.alive.load(Ordering::Acquire) {
                    force_child_shutdown(self.child_pid);
                }
                Err(error)
            }
        }
    }

    pub(crate) fn resize(&self, size: Size) -> io::Result<()> {
        self.writer.resize(WinSize::from(size))
    }

    pub(crate) fn child_pid(&self) -> u32 {
        self.child_pid as u32
    }
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
) {
    let _ = run_pty_writer_loop(
        master,
        receiver,
        control,
        protocol_replies,
        wait_for_pty_writable,
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
        if !matches!(
            service_writer_control(&writer, &control, &mut resize),
            Ok(true)
        ) {
            break;
        }

        if current_protocol_reply.is_none() {
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
            if !matches!(
                service_writer_control(&writer, &control, &mut resize),
                Ok(true)
            ) {
                break;
            }
        }

        // A reply is taken again after recv because the Wake that made the
        // receiver runnable can race with the first priority-queue check.
        if current_protocol_reply.is_none() {
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
                // poll is capped at 10ms in production; service controls as
                // soon as it returns, whether for readiness or timeout.
                if !matches!(
                    service_writer_control(&writer, &control, &mut resize),
                    Ok(true)
                ) {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    protocol_replies.close(&control);
    writer
}

fn service_writer_control<W>(
    writer: &W,
    control: &WriterControl,
    resize: &mut impl FnMut(&W, WinSize) -> io::Result<()>,
) -> io::Result<bool> {
    if control.is_closed() {
        return Ok(false);
    }

    let pending_resize = control.take_resize();
    if control.is_closed() {
        return Ok(false);
    }
    if let Some(window_size) = pending_resize {
        resize(writer, window_size)?;
    }
    Ok(!control.is_closed())
}

fn wait_for_pty_writable(master: &File) -> io::Result<()> {
    let mut descriptor = PollFd {
        fd: master.as_raw_fd(),
        events: POLLOUT,
        revents: 0,
    };
    // The timeout bounds Resize/Close latency because std's channel and poll
    // cannot be waited on together. Readiness still wakes immediately when the
    // child drains input, avoiding a fixed delay between partial writes.
    // SAFETY: `descriptor` contains the master's live fd and exactly one entry
    // is passed to poll for the duration of the call.
    let ready = unsafe { poll(&mut descriptor, 1 as PollCount, WRITER_POLL_TIMEOUT_MS) };
    if ready < 0 {
        let error = io::Error::last_os_error();
        if error.kind() != ErrorKind::Interrupted {
            return Err(error);
        }
    }
    if descriptor.revents & POLLNVAL != 0 {
        return Err(io::Error::new(
            ErrorKind::BrokenPipe,
            "tmon PTY writer descriptor is invalid",
        ));
    }
    Ok(())
}

fn resize_pty_master(master: &File, window_size: WinSize) -> io::Result<()> {
    loop {
        // SAFETY: `master` owns a live PTY descriptor and `window_size` has the
        // platform winsize layout for the duration of the ioctl call.
        if unsafe { ioctl(master.as_raw_fd(), TIOCSWINSZ, &window_size) } >= 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != ErrorKind::Interrupted {
            return Err(error);
        }
    }
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

fn queue_reader_protocol_reply(writer: &WriterHandle, reply: Vec<u8>) -> bool {
    if reply.is_empty() {
        return true;
    }
    if writer.write_protocol_reply_owned(reply).is_ok() {
        true
    } else {
        // A missing protocol response can leave a child waiting forever. The
        // caller tears down the child after this sticky writer shutdown.
        writer.close();
        false
    }
}

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
    // Match the native Alacritty PTY path. `signal`, `tcgetattr`, and
    // `tcsetattr` are async-signal-safe, and this code performs no allocation
    // or locking between forkpty and execve.
    unsafe {
        for signal_number in [SIGCHLD, SIGHUP, SIGINT, SIGQUIT, SIGTERM, SIGALRM] {
            let _ = signal(signal_number, SIGNAL_DEFAULT);
        }

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

fn wait_for_child(pid: c_int) {
    // SAFETY: the dedicated watcher is the sole waiter for the exact child PID
    // returned by forkpty.
    unsafe {
        let mut status = 0;
        while waitpid(pid, &mut status, 0) < 0
            && io::Error::last_os_error().kind() == ErrorKind::Interrupted
        {}
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

fn schedule_child_shutdown_escalation(pid: c_int, alive: Arc<AtomicBool>) {
    let fallback_alive = alive.clone();
    let escalation = thread::Builder::new()
        .name("tmon-pty-shutdown".to_string())
        .spawn(move || {
            thread::sleep(DROP_SHUTDOWN_GRACE);
            if alive.load(Ordering::Acquire) {
                force_child_shutdown(pid);
            }
        });

    // A failed thread spawn must not restore the original unbounded shutdown.
    // Killing is non-blocking here; the dedicated watcher still performs waitpid.
    if escalation.is_err() && fallback_alive.load(Ordering::Acquire) {
        force_child_shutdown(pid);
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        let child_was_alive = self.alive.load(Ordering::Acquire);
        if child_was_alive {
            // SIGHUP mirrors closing a conventional terminal session.
            signal_child_shutdown(self.child_pid);
        }
        self.writer.close();
        if child_was_alive {
            schedule_child_shutdown_escalation(self.child_pid, self.alive.clone());
        }
    }
}

fn c_string(value: &str, label: &str) -> io::Result<CString> {
    CString::new(value).map_err(|_| {
        io::Error::new(
            ErrorKind::InvalidInput,
            format!("{label} cannot contain NUL bytes"),
        )
    })
}

fn c_string_os(value: &OsStr, label: &str) -> io::Result<CString> {
    CString::new(value.as_bytes()).map_err(|_| {
        io::Error::new(
            ErrorKind::InvalidInput,
            format!("{label} cannot contain NUL bytes"),
        )
    })
}

fn child_environment(
    overrides: &[(String, String)],
    working_directory: Option<&Path>,
) -> io::Result<Vec<CString>> {
    let mut environment = std::env::vars_os().collect::<BTreeMap<OsString, OsString>>();
    for (name, value) in overrides {
        if name.is_empty() || name.as_bytes().contains(&b'=') {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "terminal environment names cannot be empty or contain '='",
            ));
        }
        environment.insert(OsString::from(name), OsString::from(value));
    }
    if let Some(working_directory) = working_directory {
        environment.insert(
            OsString::from("PWD"),
            working_directory.as_os_str().to_os_string(),
        );
    }

    environment
        .into_iter()
        .map(|(name, value)| {
            let mut entry = Vec::with_capacity(name.as_bytes().len() + value.as_bytes().len() + 1);
            entry.extend_from_slice(name.as_bytes());
            entry.push(b'=');
            entry.extend_from_slice(value.as_bytes());
            CString::new(entry).map_err(|_| {
                io::Error::new(
                    ErrorKind::InvalidInput,
                    "terminal environment cannot contain NUL bytes",
                )
            })
        })
        .collect()
}

fn resolve_program(config: &SpawnConfig) -> io::Result<PathBuf> {
    let program = PathBuf::from(&config.program);
    if config.program.as_bytes().contains(&b'/') {
        return Ok(program);
    }

    let child_directory = match config.working_directory.as_deref() {
        Some(directory) if directory.is_absolute() => directory.to_path_buf(),
        Some(directory) => std::env::current_dir()?.join(directory),
        None => std::env::current_dir()?,
    };

    let path = config
        .environment
        .iter()
        .rev()
        .find(|(name, _)| name == "PATH")
        .map(|(_, value)| OsString::from(value))
        .or_else(|| std::env::var_os("PATH"))
        .unwrap_or_default();
    for directory in std::env::split_paths(&path) {
        let directory = if directory.is_absolute() {
            directory
        } else {
            child_directory.join(directory)
        };
        let Ok(candidate) = directory.join(&program).canonicalize() else {
            continue;
        };
        if candidate
            .metadata()
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        {
            return Ok(candidate);
        }
    }

    Err(io::Error::new(
        ErrorKind::NotFound,
        format!(
            "terminal program '{}' was not found in PATH",
            config.program
        ),
    ))
}

#[cfg(test)]
#[path = "pty_tests/mod.rs"]
mod tests;
