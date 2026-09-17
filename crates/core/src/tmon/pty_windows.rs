use std::{
    cmp::Ordering as CmpOrdering,
    collections::VecDeque,
    ffi::{OsStr, OsString, c_void},
    io::{self, ErrorKind},
    mem,
    os::windows::ffi::OsStrExt,
    path::{Component, Path, PathBuf, Prefix},
    ptr,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

#[cfg(test)]
use std::os::windows::ffi::OsStringExt;

use crate::tmon::Size;
use crate::tmon::pty_limits::{
    MAX_PROTOCOL_REPLY_BACKLOG_BYTES as MAX_PENDING_PROTOCOL_REPLY_BYTES,
    MAX_PROTOCOL_REPLY_BACKLOG_ENTRIES as MAX_PENDING_PROTOCOL_REPLY_ENTRIES,
    MAX_WRITE_BACKLOG_BYTES as MAX_PENDING_WRITE_BYTES,
    MAX_WRITE_BACKLOG_ENTRIES as MAX_PENDING_WRITE_ENTRIES,
    MAX_WRITE_CHUNK_BYTES as MAX_WRITER_WRITE_CHUNK,
};

type Bool = i32;
type Dword = u32;
type Handle = *mut c_void;
type HModule = *mut c_void;
type HResult = i32;
type HpCon = *mut c_void;

const FALSE: Bool = 0;
const TRUE: Bool = 1;
const CSTR_LESS_THAN: i32 = 1;
const CSTR_EQUAL: i32 = 2;
const CSTR_GREATER_THAN: i32 = 3;
const WAIT_OBJECT_0: Dword = 0;
const WAIT_TIMEOUT: Dword = 258;
const WAIT_FAILED: Dword = Dword::MAX;
const INFINITE: Dword = Dword::MAX;
const ERROR_BROKEN_PIPE: i32 = 109;
const ERROR_NO_DATA: i32 = 232;
const ERROR_OPERATION_ABORTED: i32 = 995;
const INVALID_HANDLE_VALUE: Handle = -1_isize as Handle;
const EXTENDED_STARTUPINFO_PRESENT: Dword = 0x0008_0000;
const CREATE_UNICODE_ENVIRONMENT: Dword = 0x0000_0400;
const STARTF_USESTDHANDLES: Dword = 0x0000_0100;
const PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE: usize = 0x0002_0016;
const CONTROL_POLL_INTERVAL: Duration = Duration::from_millis(10);
const MAX_COMMAND_LINE_UNITS: usize = 32_767;
const WRITER_CHANNEL_CAPACITY: usize = MAX_PENDING_WRITE_ENTRIES + 1;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Coord {
    x: i16,
    y: i16,
}

impl TryFrom<Size> for Coord {
    type Error = io::Error;

    fn try_from(size: Size) -> io::Result<Self> {
        let x = i16::try_from(size.cols).map_err(|_| invalid_console_size(size))?;
        let y = i16::try_from(size.rows).map_err(|_| invalid_console_size(size))?;
        if x == 0 || y == 0 {
            return Err(invalid_console_size(size));
        }
        Ok(Self { x, y })
    }
}

fn invalid_console_size(size: Size) -> io::Error {
    io::Error::new(
        ErrorKind::InvalidInput,
        format!(
            "ConPTY dimensions must be between 1 and {} cells (got {}x{})",
            i16::MAX,
            size.cols,
            size.rows
        ),
    )
}

#[repr(C)]
struct StartupInfoW {
    cb: Dword,
    reserved: *mut u16,
    desktop: *mut u16,
    title: *mut u16,
    x: Dword,
    y: Dword,
    x_size: Dword,
    y_size: Dword,
    x_count_chars: Dword,
    y_count_chars: Dword,
    fill_attribute: Dword,
    flags: Dword,
    show_window: u16,
    reserved_2_length: u16,
    reserved_2: *mut u8,
    standard_input: Handle,
    standard_output: Handle,
    standard_error: Handle,
}

impl StartupInfoW {
    fn zeroed() -> Self {
        Self {
            cb: 0,
            reserved: ptr::null_mut(),
            desktop: ptr::null_mut(),
            title: ptr::null_mut(),
            x: 0,
            y: 0,
            x_size: 0,
            y_size: 0,
            x_count_chars: 0,
            y_count_chars: 0,
            fill_attribute: 0,
            flags: 0,
            show_window: 0,
            reserved_2_length: 0,
            reserved_2: ptr::null_mut(),
            standard_input: ptr::null_mut(),
            standard_output: ptr::null_mut(),
            standard_error: ptr::null_mut(),
        }
    }
}

#[repr(C)]
struct StartupInfoExW {
    startup_info: StartupInfoW,
    attribute_list: *mut c_void,
}

#[repr(C)]
struct ProcessInformation {
    process: Handle,
    thread: Handle,
    process_id: Dword,
    thread_id: Dword,
}

impl ProcessInformation {
    fn zeroed() -> Self {
        Self {
            process: ptr::null_mut(),
            thread: ptr::null_mut(),
            process_id: 0,
            thread_id: 0,
        }
    }
}

type CreatePseudoConsoleFn =
    unsafe extern "system" fn(Coord, Handle, Handle, Dword, *mut HpCon) -> HResult;
type ResizePseudoConsoleFn = unsafe extern "system" fn(HpCon, Coord) -> HResult;
type ClosePseudoConsoleFn = unsafe extern "system" fn(HpCon);

#[derive(Clone, Copy)]
struct ConPtyApi {
    create: CreatePseudoConsoleFn,
    resize: ResizePseudoConsoleFn,
    close: ClosePseudoConsoleFn,
}

impl ConPtyApi {
    fn load_uncached() -> Option<Self> {
        const KERNEL32: [u16; 13] = [
            b'k' as u16,
            b'e' as u16,
            b'r' as u16,
            b'n' as u16,
            b'e' as u16,
            b'l' as u16,
            b'3' as u16,
            b'2' as u16,
            b'.' as u16,
            b'd' as u16,
            b'l' as u16,
            b'l' as u16,
            0,
        ];
        // SAFETY: `KERNEL32` is NUL-terminated and kernel32.dll is already
        // loaded in every Win32 process. GetModuleHandleW does not load code.
        let module = unsafe { GetModuleHandleW(KERNEL32.as_ptr()) };
        if module.is_null() {
            return None;
        }

        Some(Self {
            create: load_symbol(module, b"CreatePseudoConsole\0")?,
            resize: load_symbol(module, b"ResizePseudoConsole\0")?,
            close: load_symbol(module, b"ClosePseudoConsole\0")?,
        })
    }
}

static CONPTY_API: OnceLock<Option<ConPtyApi>> = OnceLock::new();

fn conpty_api() -> Option<ConPtyApi> {
    *CONPTY_API.get_or_init(ConPtyApi::load_uncached)
}

fn load_symbol<T: Copy>(module: HModule, name: &'static [u8]) -> Option<T> {
    // SAFETY: `module` came from GetModuleHandleW and `name` is a static,
    // NUL-terminated ASCII export name. The caller supplies the documented ABI.
    let symbol = unsafe { GetProcAddress(module, name.as_ptr()) };
    if symbol.is_null() || mem::size_of::<T>() != mem::size_of::<*mut c_void>() {
        return None;
    }
    // SAFETY: all three callers instantiate `T` with the exact documented
    // `extern "system"` signature of the named kernel32 ConPTY export. Windows
    // function pointers have the same representation as GetProcAddress output.
    Some(unsafe { mem::transmute_copy::<*mut c_void, T>(&symbol) })
}

pub(crate) fn available() -> bool {
    conpty_api().is_some()
}

#[derive(Clone, Debug)]
pub(crate) struct SpawnConfig {
    pub(crate) program: String,
    pub(crate) args: Vec<String>,
    pub(crate) working_directory: Option<PathBuf>,
    pub(crate) environment: Vec<(String, String)>,
}

struct LaunchCommand {
    application: PathBuf,
    command_line: Vec<u16>,
}

pub(crate) struct Pty {
    writer: WriterHandle,
    child_pid: u32,
}

#[derive(Clone)]
struct WriterHandle {
    sender: mpsc::SyncSender<WriterCommand>,
    control: ControlHandle,
    budget: Arc<WriteBudget>,
    protocol_replies: Arc<ProtocolReplyQueue>,
    wake_pending: Arc<AtomicBool>,
}

enum WriterCommand {
    Write(ReservedWrite),
    Wake(WakeReservation),
}

struct WriteBudget {
    bytes: AtomicUsize,
    entries: AtomicUsize,
    max_bytes: usize,
    max_entries: usize,
}

struct WriteReservation {
    budget: Arc<WriteBudget>,
    bytes: usize,
}

struct ReservedWrite {
    bytes: Vec<u8>,
    offset: usize,
    _reservation: WriteReservation,
}

struct ProtocolReplyQueue {
    pending: Mutex<VecDeque<ReservedWrite>>,
    budget: Arc<WriteBudget>,
}

struct WakeReservation {
    pending: Arc<AtomicBool>,
}

#[derive(Clone)]
struct ControlHandle {
    sender: mpsc::SyncSender<ControlCommand>,
    state: Arc<ControlState>,
}

enum ControlCommand {
    Wake,
}

#[derive(Default)]
struct ControlState {
    close_requested: AtomicBool,
    pending_resize: Mutex<Option<Coord>>,
}

impl ControlState {
    fn is_closed(&self) -> bool {
        self.close_requested.load(Ordering::Acquire)
    }

    fn request_resize(&self, size: Coord) -> bool {
        if self.is_closed() {
            return false;
        }
        let mut pending = self
            .pending_resize
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.is_closed() {
            return false;
        }
        *pending = Some(size);
        true
    }

    fn take_resize(&self) -> Option<Coord> {
        self.pending_resize
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    fn request_close(&self) {
        self.close_requested.store(true, Ordering::Release);
        self.pending_resize
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
    }
}

impl ControlHandle {
    fn request_resize(&self, size: Coord) -> io::Result<()> {
        if !self.state.request_resize(size) {
            return Err(pty_closed_error());
        }
        match self.sender.try_send(ControlCommand::Wake) {
            Ok(()) | Err(mpsc::TrySendError::Full(_)) => Ok(()),
            Err(mpsc::TrySendError::Disconnected(_)) => {
                self.state.request_close();
                Err(pty_closed_error())
            }
        }
    }

    fn close(&self) {
        self.state.request_close();
        // A full one-slot channel already contains the only notification the
        // watcher needs. Close itself is sticky in shared state.
        let _ = self.sender.try_send(ControlCommand::Wake);
    }
}

impl WriterHandle {
    fn channel(control: ControlHandle) -> (Self, mpsc::Receiver<WriterCommand>) {
        Self::channel_with_limits(
            control,
            MAX_PENDING_WRITE_BYTES,
            MAX_PENDING_WRITE_ENTRIES,
            MAX_PENDING_PROTOCOL_REPLY_BYTES,
            MAX_PENDING_PROTOCOL_REPLY_ENTRIES,
        )
    }

    fn channel_with_limits(
        control: ControlHandle,
        write_bytes: usize,
        write_entries: usize,
        protocol_reply_bytes: usize,
        protocol_reply_entries: usize,
    ) -> (Self, mpsc::Receiver<WriterCommand>) {
        // Reservations cap writes at 4096; one extra slot guarantees room for
        // the single coalesced close wake without reducing write capacity.
        let (sender, receiver) = mpsc::sync_channel(WRITER_CHANNEL_CAPACITY);
        let wake_pending = Arc::new(AtomicBool::new(false));
        (
            Self {
                sender,
                control,
                budget: Arc::new(WriteBudget::with_limits(write_bytes, write_entries)),
                protocol_replies: Arc::new(ProtocolReplyQueue {
                    pending: Mutex::new(VecDeque::new()),
                    budget: Arc::new(WriteBudget::with_limits(
                        protocol_reply_bytes,
                        protocol_reply_entries,
                    )),
                }),
                wake_pending,
            },
            receiver,
        )
    }

    fn write_owned(&self, bytes: Vec<u8>) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let Some(reservation) = self.reserve_write(bytes.capacity())? else {
            return Ok(());
        };
        self.enqueue_reserved(bytes, reservation)
    }

    fn write(&self, bytes: &[u8]) -> io::Result<()> {
        let Some(reservation) = self.reserve_write(bytes.len())? else {
            return Ok(());
        };
        // Admission happens before this copy, so an oversized or saturated
        // borrowed write cannot allocate an otherwise doomed queue payload.
        let owned = bytes.to_vec();
        let mut reservation = reservation;
        if !reservation.grow_to(owned.capacity()) {
            return Err(writer_full_error());
        }
        self.enqueue_reserved(owned, reservation)
    }

    fn write_protocol_reply_owned(&self, bytes: Vec<u8>) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        if self.control.state.is_closed() {
            return Err(pty_closed_error());
        }
        let Some(reservation) = self.protocol_replies.budget.reserve(bytes.capacity()) else {
            return Err(protocol_reply_full_error());
        };
        if self.control.state.is_closed() {
            drop(reservation);
            return Err(pty_closed_error());
        }
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
                pty_closed_error()
            })?;
        self.wake()
    }

    fn reserve_write(&self, bytes: usize) -> io::Result<Option<WriteReservation>> {
        if bytes == 0 {
            return Ok(None);
        }
        if self.control.state.is_closed() {
            return Err(pty_closed_error());
        }
        let Some(reservation) = self.budget.reserve(bytes) else {
            return Err(writer_full_error());
        };
        if self.control.state.is_closed() {
            drop(reservation);
            return Err(pty_closed_error());
        }
        Ok(Some(reservation))
    }

    fn enqueue_reserved(&self, bytes: Vec<u8>, reservation: WriteReservation) -> io::Result<()> {
        let command = WriterCommand::Write(ReservedWrite {
            bytes,
            offset: 0,
            _reservation: reservation,
        });
        match self.sender.try_send(command) {
            Ok(()) => Ok(()),
            Err(mpsc::TrySendError::Full(command)) => {
                drop(command);
                Err(writer_full_error())
            }
            Err(mpsc::TrySendError::Disconnected(command)) => {
                drop(command);
                self.protocol_replies.close(&self.control);
                Err(pty_closed_error())
            }
        }
    }

    fn close(&self) {
        self.protocol_replies.close(&self.control);
        let _ = self.wake();
    }

    fn wake(&self) -> io::Result<()> {
        if self
            .wake_pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Ok(());
        }
        let reservation = WakeReservation {
            pending: self.wake_pending.clone(),
        };
        match self.sender.try_send(WriterCommand::Wake(reservation)) {
            Ok(()) => Ok(()),
            Err(mpsc::TrySendError::Full(command)) => {
                // Dropping the rejected command releases the pending-wake bit.
                // A full queue already contains writes that wake the receiver.
                drop(command);
                Ok(())
            }
            Err(mpsc::TrySendError::Disconnected(command)) => {
                drop(command);
                self.protocol_replies.close(&self.control);
                Err(pty_closed_error())
            }
        }
    }
}

impl Default for WriteBudget {
    fn default() -> Self {
        Self::with_limits(MAX_PENDING_WRITE_BYTES, MAX_PENDING_WRITE_ENTRIES)
    }
}

impl WriteBudget {
    fn with_limits(max_bytes: usize, max_entries: usize) -> Self {
        Self {
            bytes: AtomicUsize::new(0),
            entries: AtomicUsize::new(0),
            max_bytes,
            max_entries,
        }
    }

    fn reserve(self: &Arc<Self>, bytes: usize) -> Option<WriteReservation> {
        if bytes == 0 || bytes > self.max_bytes {
            return None;
        }
        self.entries
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |entries| {
                (entries < self.max_entries).then_some(entries + 1)
            })
            .ok()?;
        if self
            .bytes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |reserved| {
                (reserved <= self.max_bytes - bytes).then_some(reserved + bytes)
            })
            .is_err()
        {
            self.entries.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        Some(WriteReservation {
            budget: self.clone(),
            bytes,
        })
    }

    fn grow(&self, additional: usize) -> bool {
        additional == 0
            || self
                .bytes
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |reserved| {
                    reserved
                        .checked_add(additional)
                        .filter(|total| *total <= self.max_bytes)
                })
                .is_ok()
    }

    #[cfg(test)]
    fn usage(&self) -> (usize, usize) {
        (
            self.bytes.load(Ordering::Acquire),
            self.entries.load(Ordering::Acquire),
        )
    }
}

impl WriteReservation {
    fn grow_to(&mut self, bytes: usize) -> bool {
        let additional = bytes.saturating_sub(self.bytes);
        if !self.budget.grow(additional) {
            return false;
        }
        self.bytes += additional;
        true
    }
}

impl ProtocolReplyQueue {
    fn push_if_open(
        &self,
        control: &ControlHandle,
        write: ReservedWrite,
    ) -> Result<(), ReservedWrite> {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if control.state.is_closed() {
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

    fn close(&self, control: &ControlHandle) {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        control.close();
        pending.clear();
    }
}

impl Drop for WriteReservation {
    fn drop(&mut self) {
        let bytes = self.budget.bytes.fetch_sub(self.bytes, Ordering::AcqRel);
        let entries = self.budget.entries.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(bytes >= self.bytes, "write-byte reservation underflow");
        debug_assert!(entries >= 1, "write-entry reservation underflow");
    }
}

impl Drop for WakeReservation {
    fn drop(&mut self) {
        self.pending.store(false, Ordering::Release);
    }
}

fn pty_closed_error() -> io::Error {
    io::Error::new(ErrorKind::BrokenPipe, "tmon PTY is closed")
}

fn writer_full_error() -> io::Error {
    io::Error::new(
        ErrorKind::WouldBlock,
        "tmon PTY write queue reached its 8 MiB or 4096-entry limit",
    )
}

fn protocol_reply_full_error() -> io::Error {
    io::Error::new(
        ErrorKind::WouldBlock,
        "tmon PTY protocol-reply queue reached its 2 MiB or 256-entry limit",
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StartState {
    Pending,
    Running,
    Cancelled,
}

struct StartGate {
    state: Mutex<StartState>,
    changed: Condvar,
}

impl StartGate {
    fn new() -> Self {
        Self {
            state: Mutex::new(StartState::Pending),
            changed: Condvar::new(),
        }
    }

    fn release(&self, state: StartState) {
        let mut current = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *current == StartState::Pending {
            *current = state;
            self.changed.notify_all();
        }
    }

    fn wait(&self) -> StartState {
        let mut current = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while *current == StartState::Pending {
            current = self
                .changed
                .wait(current)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        *current
    }
}

impl Pty {
    pub(crate) fn spawn(
        config: SpawnConfig,
        size: Size,
        mut on_output: impl FnMut(&[u8]) -> Vec<u8> + Send + 'static,
        on_exit: impl FnOnce() + Send + 'static,
    ) -> io::Result<Self> {
        let api = conpty_api().ok_or_else(|| {
            io::Error::new(
                ErrorKind::Unsupported,
                "Windows ConPTY exports are unavailable on this system",
            )
        })?;
        let coord = Coord::try_from(size)?;
        let working_directory = normalize_working_directory(config.working_directory.as_deref())?;
        let lookup_directory = working_directory
            .clone()
            .map_or_else(normalized_current_directory, Ok)?;
        let launch = resolve_launch_command(&config, &lookup_directory)?;
        let application = wide_nul(launch.application.as_os_str(), "terminal program")?;
        let mut command_line = launch.command_line;
        let mut environment = child_environment(&config.environment, working_directory.as_deref())?;
        let current_directory = working_directory
            .as_deref()
            .map(|path| wide_nul(path.as_os_str(), "working directory"))
            .transpose()?;

        let (pseudo_input, input_writer) = create_pipe()?;
        let (output_reader, pseudo_output) = create_pipe()?;
        let pseudo_console = PseudoConsole::create(api, coord, &pseudo_input, &pseudo_output)?;

        let mut attributes = AttributeList::new(1)?;
        attributes.set_pseudo_console(pseudo_console.raw())?;
        let mut startup = StartupInfoExW {
            startup_info: StartupInfoW::zeroed(),
            attribute_list: attributes.as_mut_ptr(),
        };
        startup.startup_info.cb = mem::size_of::<StartupInfoExW>() as Dword;
        // Prevent redirected parent handles from bypassing the pseudoconsole.
        startup.startup_info.flags = STARTF_USESTDHANDLES;
        startup.startup_info.standard_input = INVALID_HANDLE_VALUE;
        startup.startup_info.standard_output = INVALID_HANDLE_VALUE;
        startup.startup_info.standard_error = INVALID_HANDLE_VALUE;
        let mut process_info = ProcessInformation::zeroed();
        let current_directory_ptr = current_directory
            .as_ref()
            .map_or(ptr::null(), |directory| directory.as_ptr());

        // SAFETY: all pointers reference writable or immutable buffers that
        // outlive CreateProcessW. `command_line` is mutable and NUL-terminated,
        // `environment` is a double-NUL-terminated UTF-16 environment block,
        // and startup carries one initialized pseudoconsole attribute.
        let created = unsafe {
            CreateProcessW(
                application.as_ptr(),
                command_line.as_mut_ptr(),
                ptr::null_mut(),
                ptr::null_mut(),
                FALSE,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                environment.as_mut_ptr().cast::<c_void>(),
                current_directory_ptr,
                &mut startup.startup_info,
                &mut process_info,
            )
        };
        if created == FALSE {
            let error = io::Error::last_os_error();
            // Closing the client endpoints first prevents legacy
            // ClosePseudoConsole implementations from waiting for undrained IO.
            drop(output_reader);
            drop(input_writer);
            drop(pseudo_input);
            drop(pseudo_output);
            drop(pseudo_console);
            return Err(error);
        }

        let Some(process) = OwnedHandle::new(process_info.process) else {
            close_raw_handle(process_info.thread);
            return Err(io::Error::other(
                "CreateProcessW returned an invalid process handle",
            ));
        };
        let process_thread = OwnedHandle::new(process_info.thread);
        drop(process_thread);
        drop(pseudo_input);
        drop(pseudo_output);
        drop(attributes);

        let state = Arc::new(ControlState::default());
        let (control_sender, control_receiver) = mpsc::sync_channel(1);
        let control = ControlHandle {
            sender: control_sender,
            state: state.clone(),
        };
        let (writer, writer_receiver) = WriterHandle::channel(control.clone());
        let gate = Arc::new(StartGate::new());
        let (control_finished_sender, control_finished_receiver) = mpsc::channel();
        let resources = Arc::new(Mutex::new(Some(ChildResources::new(
            process,
            pseudo_console,
        ))));

        let reader_gate = gate.clone();
        let reader_writer = writer.clone();
        let reader_thread = thread::Builder::new()
            .name("tmon-conpty-reader".to_string())
            .spawn(move || match reader_gate.wait() {
                StartState::Running => run_reader(
                    output_reader,
                    &mut on_output,
                    on_exit,
                    reader_writer,
                    control_finished_receiver,
                ),
                StartState::Cancelled => drain_discard(output_reader),
                StartState::Pending => unreachable!("start gate cannot release as pending"),
            });
        let reader_thread = match reader_thread {
            Ok(thread) => thread,
            Err(error) => {
                writer.close();
                cleanup_unstarted_resources(&resources);
                return Err(error);
            }
        };

        let writer_gate = gate.clone();
        let writer_control = control;
        let writer_protocol_replies = writer.protocol_replies.clone();
        let writer_thread = thread::Builder::new()
            .name("tmon-conpty-writer".to_string())
            .spawn(move || {
                if writer_gate.wait() == StartState::Running {
                    run_writer(
                        input_writer,
                        writer_receiver,
                        writer_control,
                        writer_protocol_replies,
                    );
                }
            });
        let writer_thread = match writer_thread {
            Ok(thread) => thread,
            Err(error) => {
                gate.release(StartState::Cancelled);
                writer.close();
                cleanup_unstarted_resources(&resources);
                let _ = reader_thread.join();
                return Err(error);
            }
        };

        let control_gate = gate.clone();
        let control_state = state;
        let control_resources = resources.clone();
        let control_thread = thread::Builder::new()
            .name("tmon-conpty-control".to_string())
            .spawn(move || {
                if control_gate.wait() != StartState::Running {
                    return;
                }
                let resources = control_resources
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take();
                if let Some(resources) = resources {
                    run_control(resources, control_receiver, control_state);
                }
                let _ = control_finished_sender.send(());
            });
        if let Err(error) = control_thread {
            gate.release(StartState::Cancelled);
            writer.close();
            cleanup_unstarted_resources(&resources);
            let _ = writer_thread.join();
            let _ = reader_thread.join();
            return Err(error);
        }

        gate.release(StartState::Running);
        Ok(Self {
            writer,
            child_pid: process_info.process_id,
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
                Err(error)
            }
        }
    }

    pub(crate) fn resize(&self, size: Size) -> io::Result<()> {
        self.writer.control.request_resize(Coord::try_from(size)?)
    }

    pub(crate) fn child_pid(&self) -> u32 {
        self.child_pid
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        self.writer.close();
    }
}

fn cleanup_unstarted_resources(resources: &Mutex<Option<ChildResources>>) {
    if let Some(resources) = resources
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
    {
        drop(resources);
    }
}

fn run_reader(
    reader: OwnedHandle,
    on_output: &mut impl FnMut(&[u8]) -> Vec<u8>,
    on_exit: impl FnOnce(),
    writer: WriterHandle,
    control_finished: mpsc::Receiver<()>,
) {
    let mut buffer = [0u8; 32 * 1024];
    loop {
        match read_handle(&reader, &mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                let reply = on_output(&buffer[..read]);
                if !queue_reader_protocol_reply(&writer, reply) {
                    break;
                }
            }
            Err(error) if is_pipe_shutdown(&error) => break,
            Err(_) => break,
        }
    }
    drop(reader);
    writer.close();
    // Normal EOF follows ClosePseudoConsole. For an unexpected pipe failure,
    // this also waits until the watcher has terminated the process and closed
    // its HPCON, so Exit never races ahead of session teardown.
    let _ = control_finished.recv();
    on_exit();
}

fn queue_reader_protocol_reply(writer: &WriterHandle, reply: Vec<u8>) -> bool {
    if reply.is_empty() {
        return true;
    }
    if writer.write_protocol_reply_owned(reply).is_ok() {
        true
    } else {
        // The control thread terminates the child and closes the pseudoconsole;
        // continuing without the reply could leave the child waiting forever.
        writer.close();
        false
    }
}

fn drain_discard(reader: OwnedHandle) {
    let mut buffer = [0u8; 32 * 1024];
    while read_handle(&reader, &mut buffer).is_ok_and(|read| read != 0) {}
}

fn run_writer(
    writer: OwnedHandle,
    receiver: mpsc::Receiver<WriterCommand>,
    control: ControlHandle,
    protocol_replies: Arc<ProtocolReplyQueue>,
) {
    let mut current_write: Option<ReservedWrite> = None;
    let mut current_protocol_reply: Option<ReservedWrite> = None;
    while !control.state.is_closed() {
        if current_protocol_reply.is_none() {
            current_protocol_reply = protocol_replies.pop();
        }
        if current_protocol_reply.is_none() && current_write.is_none() {
            let Ok(command) = receiver.recv() else {
                control.close();
                break;
            };
            match command {
                WriterCommand::Write(write) => current_write = Some(write),
                WriterCommand::Wake(_reservation) => {}
            }
        }

        // The wake that released recv may correspond to a priority reply.
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
        let Ok(written) = write_handle(&writer, &write.bytes[write.offset..end]) else {
            control.close();
            break;
        };
        write.offset += written;
        if write.offset == write.bytes.len() {
            if writing_protocol_reply {
                current_protocol_reply = None;
            } else {
                current_write = None;
            }
        }
    }
    protocol_replies.close(&control);
}

fn run_control(
    mut resources: ChildResources,
    receiver: mpsc::Receiver<ControlCommand>,
    state: Arc<ControlState>,
) {
    loop {
        match resources.process_signaled() {
            Ok(true) => {
                resources.process_exited = true;
                break;
            }
            Ok(false) => {}
            Err(_) => {
                state.request_close();
                break;
            }
        }

        if state.is_closed() {
            break;
        }
        if let Some(size) = state.take_resize()
            && resources.pseudo_console().resize(size).is_err()
        {
            state.request_close();
            break;
        }

        match receiver.recv_timeout(CONTROL_POLL_INTERVAL) {
            Ok(ControlCommand::Wake) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                state.request_close();
                break;
            }
        }
    }

    if !resources.process_exited {
        let _ = resources.terminate_and_wait();
    }
    state.request_close();
    // ClosePseudoConsole intentionally runs on this control thread while the
    // dedicated reader keeps draining. Older Windows releases can block here
    // until all final pseudoconsole output has been consumed.
    resources.close_pseudo_console();
}

fn read_handle(handle: &OwnedHandle, buffer: &mut [u8]) -> io::Result<usize> {
    let requested = buffer.len().min(Dword::MAX as usize) as Dword;
    let mut read = 0;
    // SAFETY: `handle` owns a synchronous pipe handle and `buffer` is writable
    // for `requested` bytes. A null OVERLAPPED pointer selects synchronous IO.
    let succeeded = unsafe {
        ReadFile(
            handle.raw(),
            buffer.as_mut_ptr().cast::<c_void>(),
            requested,
            &mut read,
            ptr::null_mut(),
        )
    };
    if succeeded == FALSE {
        return Err(io::Error::last_os_error());
    }
    Ok(read as usize)
}

fn write_handle(handle: &OwnedHandle, bytes: &[u8]) -> io::Result<usize> {
    let requested = bytes.len().min(Dword::MAX as usize) as Dword;
    let mut written = 0;
    // SAFETY: `handle` owns a synchronous pipe handle and `bytes` is readable
    // for `requested` bytes. The written count is valid for the duration.
    let succeeded = unsafe {
        WriteFile(
            handle.raw(),
            bytes.as_ptr().cast::<c_void>(),
            requested,
            &mut written,
            ptr::null_mut(),
        )
    };
    if succeeded == FALSE {
        return Err(io::Error::last_os_error());
    }
    if written == 0 {
        return Err(io::Error::new(
            ErrorKind::WriteZero,
            "ConPTY input pipe accepted zero bytes",
        ));
    }
    Ok(written as usize)
}

fn is_pipe_shutdown(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(ERROR_BROKEN_PIPE | ERROR_NO_DATA | ERROR_OPERATION_ABORTED)
    )
}

struct OwnedHandle(Handle);

// SAFETY: Windows kernel handles are process-wide opaque values. Ownership is
// transferred to exactly one worker thread and CloseHandle remains valid there.
unsafe impl Send for OwnedHandle {}

impl OwnedHandle {
    fn new(handle: Handle) -> Option<Self> {
        (!handle.is_null() && handle as isize != -1).then_some(Self(handle))
    }

    fn raw(&self) -> Handle {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        close_raw_handle(self.0);
    }
}

fn close_raw_handle(handle: Handle) {
    if !handle.is_null() && handle as isize != -1 {
        // SAFETY: callers pass an owned Win32 HANDLE exactly once. Return value
        // cannot be acted on during Drop and does not change ownership.
        unsafe {
            let _ = CloseHandle(handle);
        }
    }
}

fn create_pipe() -> io::Result<(OwnedHandle, OwnedHandle)> {
    let mut read = ptr::null_mut();
    let mut write = ptr::null_mut();
    // SAFETY: both output pointers are valid. Null security attributes produce
    // non-inheritable synchronous anonymous-pipe handles as required here.
    if unsafe { CreatePipe(&mut read, &mut write, ptr::null_mut(), 0) } == FALSE {
        return Err(io::Error::last_os_error());
    }
    let Some(read) = OwnedHandle::new(read) else {
        close_raw_handle(write);
        return Err(io::Error::other(
            "CreatePipe returned an invalid read handle",
        ));
    };
    let Some(write) = OwnedHandle::new(write) else {
        return Err(io::Error::other(
            "CreatePipe returned an invalid write handle",
        ));
    };
    Ok((read, write))
}

struct PseudoConsole {
    handle: HpCon,
    api: ConPtyApi,
}

// SAFETY: HPCON is an opaque process-wide kernel object. This wrapper transfers
// its sole ClosePseudoConsole responsibility to the control thread.
unsafe impl Send for PseudoConsole {}

impl PseudoConsole {
    fn create(
        api: ConPtyApi,
        size: Coord,
        input: &OwnedHandle,
        output: &OwnedHandle,
    ) -> io::Result<Self> {
        let mut handle = ptr::null_mut();
        // SAFETY: both pipe handles remain live through this call and `handle`
        // is valid output storage. The dynamically loaded function has the
        // documented CreatePseudoConsole system ABI.
        let result = unsafe { (api.create)(size, input.raw(), output.raw(), 0, &mut handle) };
        if result < 0 {
            return Err(hresult_error("CreatePseudoConsole", result));
        }
        if handle.is_null() {
            return Err(io::Error::other(
                "CreatePseudoConsole returned a null HPCON",
            ));
        }
        Ok(Self { handle, api })
    }

    fn raw(&self) -> HpCon {
        self.handle
    }

    fn resize(&self, size: Coord) -> io::Result<()> {
        // SAFETY: this wrapper owns a live HPCON, the control thread serializes
        // resize with close, and the function pointer has the documented ABI.
        let result = unsafe { (self.api.resize)(self.handle, size) };
        if result < 0 {
            return Err(hresult_error("ResizePseudoConsole", result));
        }
        Ok(())
    }
}

impl Drop for PseudoConsole {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            // SAFETY: this is the unique owner of a live HPCON. Close is called
            // exactly once and the handle is cleared before Drop can finish.
            unsafe { (self.api.close)(self.handle) };
            self.handle = ptr::null_mut();
        }
    }
}

fn hresult_error(operation: &str, result: HResult) -> io::Error {
    let facility = ((result as u32) >> 16) & 0x1fff;
    if facility == 7 {
        let code = (result as u32 & 0xffff) as i32;
        return io::Error::new(
            io::Error::from_raw_os_error(code).kind(),
            format!("{operation} failed: {}", io::Error::from_raw_os_error(code)),
        );
    }
    io::Error::other(format!(
        "{operation} failed with HRESULT 0x{:08X}",
        result as u32
    ))
}

struct ChildResources {
    process: Option<OwnedHandle>,
    pseudo_console: Option<PseudoConsole>,
    process_exited: bool,
}

impl ChildResources {
    fn new(process: OwnedHandle, pseudo_console: PseudoConsole) -> Self {
        Self {
            process: Some(process),
            pseudo_console: Some(pseudo_console),
            process_exited: false,
        }
    }

    fn process(&self) -> &OwnedHandle {
        self.process
            .as_ref()
            .expect("child process handle remains owned until cleanup")
    }

    fn pseudo_console(&self) -> &PseudoConsole {
        self.pseudo_console
            .as_ref()
            .expect("pseudoconsole remains owned until control cleanup")
    }

    fn process_signaled(&self) -> io::Result<bool> {
        // SAFETY: the control thread owns this live process handle. A zero
        // timeout only observes signal state and does not consume the handle.
        match unsafe { WaitForSingleObject(self.process().raw(), 0) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            WAIT_FAILED => Err(io::Error::last_os_error()),
            value => Err(io::Error::other(format!(
                "WaitForSingleObject returned unexpected status 0x{value:08X}"
            ))),
        }
    }

    fn terminate_and_wait(&mut self) -> io::Result<()> {
        if self.process_exited {
            return Ok(());
        }
        // SAFETY: this wrapper owns the exact process created for the terminal.
        // TerminateProcess is the Windows shutdown fallback when its HPCON
        // session owner is dropped before the client exits naturally.
        let terminated = unsafe { TerminateProcess(self.process().raw(), 1) };
        if terminated == FALSE && !self.process_signaled().unwrap_or(false) {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the exact child process handle remains live. After successful
        // termination, waiting without a timeout completes process teardown.
        let waited = unsafe { WaitForSingleObject(self.process().raw(), INFINITE) };
        if waited != WAIT_OBJECT_0 {
            return Err(if waited == WAIT_FAILED {
                io::Error::last_os_error()
            } else {
                io::Error::other(format!(
                    "WaitForSingleObject returned unexpected status 0x{waited:08X}"
                ))
            });
        }
        self.process_exited = true;
        Ok(())
    }

    fn close_pseudo_console(&mut self) {
        drop(self.pseudo_console.take());
    }
}

impl Drop for ChildResources {
    fn drop(&mut self) {
        if !self.process_exited {
            let _ = self.terminate_and_wait();
        }
        self.close_pseudo_console();
        drop(self.process.take());
    }
}

#[repr(C, align(16))]
#[derive(Clone, Copy, Default)]
struct AttributeStorageBlock([u8; 16]);

struct AttributeList {
    storage: Vec<AttributeStorageBlock>,
    initialized: bool,
}

impl AttributeList {
    fn new(attribute_count: Dword) -> io::Result<Self> {
        let mut bytes = 0usize;
        // SAFETY: the first documented probe uses a null list and valid size
        // output to obtain the allocation requirement for `attribute_count`.
        unsafe {
            let _ =
                InitializeProcThreadAttributeList(ptr::null_mut(), attribute_count, 0, &mut bytes);
        }
        if bytes == 0 {
            return Err(io::Error::last_os_error());
        }
        let blocks = bytes.div_ceil(mem::size_of::<AttributeStorageBlock>());
        let mut list = Self {
            // HeapAlloc, used by Microsoft's sample, is 16-byte aligned on
            // Win64. Preserve that stronger alignment instead of relying on a
            // Vec<u8> or pointer-sized allocation for this opaque SDK object.
            storage: vec![AttributeStorageBlock::default(); blocks],
            initialized: false,
        };
        // SAFETY: `storage` is pointer-aligned and contains at least `bytes`
        // writable bytes. It is not moved or resized until the list is deleted.
        if unsafe {
            InitializeProcThreadAttributeList(list.as_mut_ptr(), attribute_count, 0, &mut bytes)
        } == FALSE
        {
            return Err(io::Error::last_os_error());
        }
        list.initialized = true;
        Ok(list)
    }

    fn as_mut_ptr(&mut self) -> *mut c_void {
        self.storage.as_mut_ptr().cast::<c_void>()
    }

    fn set_pseudo_console(&mut self, console: HpCon) -> io::Result<()> {
        // SAFETY: the attribute list is initialized for one entry. Unlike
        // scalar process attributes, Microsoft's pseudoconsole contract passes
        // the opaque HPCON value itself as `lpValue` (not the address of an
        // HPCON variable), while `cbSize` is still `sizeof(HPCON)`.
        if unsafe {
            UpdateProcThreadAttribute(
                self.as_mut_ptr(),
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
                console.cast::<c_void>(),
                mem::size_of::<HpCon>(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        } == FALSE
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        if self.initialized {
            // SAFETY: this list was successfully initialized, has not been
            // deleted, and its backing storage is still live and unmoved.
            unsafe { DeleteProcThreadAttributeList(self.as_mut_ptr()) };
        }
    }
}

include!("pty_windows/launch.rs");

#[cfg(test)]
#[path = "pty_windows_tests.rs"]
mod tests;
