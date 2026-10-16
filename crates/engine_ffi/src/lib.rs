//! `libtermy_engine`: a C ABI over Termy's headless terminal engine.
//!
//! The library holds a parser, screens, scrollback and protocol state, and
//! nothing else: no PTY, config files, threads, timers, fonts or network. A
//! host feeds it program output and reads cells, state and encoded input
//! bytes back. `include/termy_engine.h` is the matching C header.
//!
//! Every function takes the handle it acts on; handles are independent and
//! not internally synchronized. Panics are caught at the boundary and
//! reported as [`TermyEngineStatus::Panicked`].

#![allow(clippy::missing_safety_doc)]

pub mod frame;
pub mod graphics;
pub mod input;
pub mod text;

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    ptr, slice,
};

use termy_core::TerminalColor;
use termy_core::TerminalQueryColors;
use termy_core::terminal_engine::{
    CursorShape, DEFAULT_SCROLLBACK_BYTES, Engine, Event, MAX_CLIPBOARD_PAYLOAD_BYTES,
    MAX_STRING_PAYLOAD_BYTES, MIN_SNAPSHOT_VERSION, MouseEncoding, MouseTracking, Options,
    SNAPSHOT_VERSION, Size, SnapshotError, snapshot_version,
};

pub use frame::{
    TermyEngineCell, TermyEngineCursor, TermyEngineFrame, TermyEngineHyperlink, TermyEngineSpan,
};
pub use graphics::TermyEngineImagePlacement;
pub use input::{TermyEngineKeyEvent, TermyEngineMouseEvent};
pub use text::TermyEngineTextRange;

/// Bumped on every incompatible change to `termy_engine.h`.
pub const TERMY_ENGINE_ABI_VERSION: u32 = 2;

/// `scrollback_rows` value that leaves history bounded by bytes alone.
pub const TERMY_ENGINE_SCROLLBACK_UNLIMITED: u32 = u32::MAX;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TermyEngineStatus {
    Ok = 0,
    Null = 1,
    InvalidArgument = 2,
    Panicked = 3,
    NotASnapshot = 4,
    UnsupportedSnapshotVersion = 5,
    TruncatedSnapshot = 6,
    CorruptSnapshot = 7,
}

impl From<SnapshotError> for TermyEngineStatus {
    fn from(error: SnapshotError) -> Self {
        match error {
            SnapshotError::NotASnapshot => Self::NotASnapshot,
            SnapshotError::UnsupportedVersion(_) => Self::UnsupportedSnapshotVersion,
            SnapshotError::Truncated => Self::TruncatedSnapshot,
            SnapshotError::Corrupt(_) => Self::CorruptSnapshot,
        }
    }
}

/// Owned bytes. Release with [`termy_engine_bytes_free`]; an empty buffer
/// has a null pointer and owns nothing.
#[repr(C)]
#[derive(Debug)]
pub struct TermyEngineBytes {
    pub ptr: *mut u8,
    pub len: usize,
    pub capacity: usize,
}

impl TermyEngineBytes {
    const EMPTY: Self = Self {
        ptr: ptr::null_mut(),
        len: 0,
        capacity: 0,
    };

    fn from_vec(bytes: Vec<u8>) -> Self {
        if bytes.is_empty() {
            return Self::EMPTY;
        }
        let mut bytes = std::mem::ManuallyDrop::new(bytes);
        Self {
            ptr: bytes.as_mut_ptr(),
            len: bytes.len(),
            capacity: bytes.capacity(),
        }
    }
}

/// Replies to queries (DA, DSR, DECRQM, color and status reports).
pub const TERMY_ENGINE_REPLIES_RETURN: u32 = 0;
/// Replicas parse queries but never answer them.
pub const TERMY_ENGINE_REPLIES_DISCARD: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TermyEngineOptions {
    pub cols: u16,
    pub rows: u16,
    pub scrollback_rows: u32,
    pub reply_mode: u32,
    /// Recognize `?1016` (SGR pixel mouse reports). Servers and replicas
    /// must agree; snapshots carry the setting.
    /// A C `bool`, read as a byte; any nonzero value is true.
    pub pixel_mouse_reports: u8,
    pub cell_width_px: u16,
    pub cell_height_px: u16,
    /// History byte limit; 0 selects the 16 MiB default.
    pub scrollback_bytes: u64,
    /// OSC/DCS and OSC 52 payload limits; 0 selects the defaults (64 KiB
    /// and 8 MiB, also the maxima).
    pub max_string_bytes: u32,
    pub max_clipboard_bytes: u32,
}

fn scrollback_rows(rows: u32) -> usize {
    if rows == TERMY_ENGINE_SCROLLBACK_UNLIMITED {
        usize::MAX
    } else {
        rows as usize
    }
}

fn or_default(value: u64, default: usize) -> usize {
    if value == 0 {
        default
    } else {
        usize::try_from(value).unwrap_or(usize::MAX)
    }
}

fn engine_options(options: &TermyEngineOptions) -> Options {
    Options {
        scrollback_history: scrollback_rows(options.scrollback_rows),
        scrollback_bytes: or_default(options.scrollback_bytes, DEFAULT_SCROLLBACK_BYTES),
        max_string_bytes: or_default(options.max_string_bytes.into(), MAX_STRING_PAYLOAD_BYTES),
        max_clipboard_bytes: or_default(
            options.max_clipboard_bytes.into(),
            MAX_CLIPBOARD_PAYLOAD_BYTES,
        ),
    }
}

/// Engine state for hosts; see `TermyEngineState` in the header.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TermyEngineState {
    pub cols: u16,
    pub rows: u16,
    pub cursor_col: u16,
    pub cursor_row: u16,
    pub cursor_visible: bool,
    pub cursor_blinking: bool,
    pub cursor_shape: u8,
    pub alternate_screen: bool,
    pub mouse_tracking: u8,
    pub mouse_encoding: u8,
    pub focus_reporting: bool,
    pub alternate_scroll: bool,
    pub bracketed_paste: bool,
    pub application_cursor: bool,
    pub application_keypad: bool,
    pub synchronized_output: bool,
    pub reverse_video: bool,
    pub kitty_keyboard_flags: u8,
    pub history_size: u32,
    pub display_offset: u32,
    pub scrollback_limit: u32,
    pub generation: u64,
    pub palette_revision: u64,
    pub scrolled_into_history: u64,
    pub scrollback_bytes: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyEngineScrollbar {
    pub total: u64,
    pub offset: u64,
    pub len: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TermyEngineColors {
    pub foreground: u32,
    pub background: u32,
    pub cursor: u32,
    pub cursor_set: bool,
    pub reverse_video: bool,
    pub palette: [u32; 256],
}

#[repr(C)]
#[derive(Debug)]
pub struct TermyEngineEvent {
    pub kind: u32,
    pub value: i32,
    pub payload: TermyEngineBytes,
    pub data: TermyEngineBytes,
}

pub const TERMY_ENGINE_EVENT_BELL: u32 = 1;
pub const TERMY_ENGINE_EVENT_TITLE: u32 = 2;
pub const TERMY_ENGINE_EVENT_RESET_TITLE: u32 = 3;
pub const TERMY_ENGINE_EVENT_WORKING_DIRECTORY: u32 = 4;
pub const TERMY_ENGINE_EVENT_SHELL_INTEGRATION: u32 = 5;
pub const TERMY_ENGINE_EVENT_PROGRESS: u32 = 6;
pub const TERMY_ENGINE_EVENT_CLIPBOARD: u32 = 7;
pub const TERMY_ENGINE_EVENT_PROGRAM_STATUS: u32 = 8;

/// The host's default colors, kept beside the engine: palette entries
/// 16..=255 are not part of the engine's query colors.
#[derive(Clone)]
struct HostColors {
    query: TerminalQueryColors,
    palette: [TerminalColor; 256],
}

impl Default for HostColors {
    fn default() -> Self {
        let query = TerminalQueryColors::default();
        Self {
            palette: std::array::from_fn(|index| query.indexed_color(index as u8)),
            query,
        }
    }
}

pub struct TermyEngine {
    engine: Engine,
    reply_mode: u32,
    replies: Vec<u8>,
    colors: HostColors,
    frame: frame::FrameBuffers,
    graphics: graphics::GraphicsBuffers,
}

impl TermyEngine {
    fn new(engine: Engine, reply_mode: u32) -> Self {
        Self {
            engine,
            reply_mode,
            replies: Vec::new(),
            colors: HostColors::default(),
            frame: frame::FrameBuffers::default(),
            graphics: graphics::GraphicsBuffers::default(),
        }
    }

    /// Hand queued replies to `out`, or drop them for replicas.
    fn collect_replies(&mut self, out: Option<&mut TermyEngineBytes>) {
        if self.reply_mode == TERMY_ENGINE_REPLIES_DISCARD {
            self.engine.discard_replies();
            if let Some(out) = out {
                *out = TermyEngineBytes::EMPTY;
            }
            return;
        }
        let Some(out) = out else {
            // Kept, in order, for the next caller that asks.
            return;
        };
        self.engine.drain_replies(&mut self.replies);
        *out = TermyEngineBytes::from_vec(std::mem::take(&mut self.replies));
    }
}

fn guard(body: impl FnOnce() -> TermyEngineStatus) -> TermyEngineStatus {
    catch_unwind(AssertUnwindSafe(body)).unwrap_or(TermyEngineStatus::Panicked)
}

/// # Safety
/// `pointer` is null or a live handle from this library.
unsafe fn handle<'a>(pointer: *mut TermyEngine) -> Option<&'a mut TermyEngine> {
    // SAFETY: per the contract above.
    unsafe { pointer.as_mut() }
}

/// # Safety
/// `ptr` is null with `len == 0`, or valid for `len` bytes.
unsafe fn input<'a>(ptr: *const u8, len: usize) -> Option<&'a [u8]> {
    if len == 0 {
        return Some(&[]);
    }
    if ptr.is_null() {
        return None;
    }
    // SAFETY: per the contract above.
    Some(unsafe { slice::from_raw_parts(ptr, len) })
}

fn size(cols: u16, rows: u16) -> Option<Size> {
    (cols != 0 && rows != 0).then_some(Size {
        cols: usize::from(cols),
        rows: usize::from(rows),
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn termy_engine_abi_version() -> u32 {
    TERMY_ENGINE_ABI_VERSION
}

#[unsafe(no_mangle)]
pub extern "C" fn termy_engine_snapshot_version() -> u32 {
    SNAPSHOT_VERSION
}

#[unsafe(no_mangle)]
pub extern "C" fn termy_engine_snapshot_min_version() -> u32 {
    MIN_SNAPSHOT_VERSION
}

#[unsafe(no_mangle)]
pub extern "C" fn termy_engine_options_default() -> TermyEngineOptions {
    TermyEngineOptions {
        cols: 80,
        rows: 24,
        scrollback_rows: 10_000,
        reply_mode: TERMY_ENGINE_REPLIES_RETURN,
        pixel_mouse_reports: 0,
        cell_width_px: 9,
        cell_height_px: 18,
        scrollback_bytes: DEFAULT_SCROLLBACK_BYTES as u64,
        max_string_bytes: MAX_STRING_PAYLOAD_BYTES as u32,
        max_clipboard_bytes: MAX_CLIPBOARD_PAYLOAD_BYTES as u32,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_new(
    options: *const TermyEngineOptions,
    out_engine: *mut *mut TermyEngine,
) -> TermyEngineStatus {
    guard(|| {
        if out_engine.is_null() {
            return TermyEngineStatus::Null;
        }
        // SAFETY: the caller passes a writable out pointer.
        unsafe { *out_engine = ptr::null_mut() };
        // SAFETY: null or a valid options struct.
        let options = unsafe { options.as_ref() }
            .copied()
            .unwrap_or_else(|| termy_engine_options_default());
        let Some(size) = size(options.cols, options.rows) else {
            return TermyEngineStatus::InvalidArgument;
        };
        if options.reply_mode > TERMY_ENGINE_REPLIES_DISCARD {
            return TermyEngineStatus::InvalidArgument;
        }
        let mut engine = Engine::new(size, engine_options(&options));
        engine.set_pixel_mouse_reports(options.pixel_mouse_reports != 0);
        engine.set_cell_pixels(
            f32::from(options.cell_width_px),
            f32::from(options.cell_height_px),
        );
        let handle = Box::new(TermyEngine::new(engine, options.reply_mode));
        // SAFETY: checked non-null above.
        unsafe { *out_engine = Box::into_raw(handle) };
        TermyEngineStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_free(engine: *mut TermyEngine) {
    if engine.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: a handle from `termy_engine_new`/`_snapshot_decode`, freed once.
        drop(unsafe { Box::from_raw(engine) });
    }));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_bytes_free(bytes: *mut TermyEngineBytes) {
    // SAFETY: null or a buffer this library returned.
    let Some(bytes) = (unsafe { bytes.as_mut() }) else {
        return;
    };
    let owned = std::mem::replace(bytes, TermyEngineBytes::EMPTY);
    if owned.ptr.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: built by `from_vec` from a `Vec<u8>` with these parts.
        drop(unsafe { Vec::from_raw_parts(owned.ptr, owned.len, owned.capacity) });
    }));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_set_reply_mode(
    engine: *mut TermyEngine,
    reply_mode: u32,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a live handle.
        let Some(engine) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        if reply_mode > TERMY_ENGINE_REPLIES_DISCARD {
            return TermyEngineStatus::InvalidArgument;
        }
        engine.reply_mode = reply_mode;
        if reply_mode == TERMY_ENGINE_REPLIES_DISCARD {
            engine.replies.clear();
            engine.engine.discard_replies();
        }
        TermyEngineStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_feed(
    engine: *mut TermyEngine,
    bytes_ptr: *const u8,
    bytes_len: usize,
    out_replies: *mut TermyEngineBytes,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a live handle.
        let Some(engine) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        // SAFETY: valid for `bytes_len` bytes per the header contract.
        let Some(bytes) = (unsafe { input(bytes_ptr, bytes_len) }) else {
            return TermyEngineStatus::Null;
        };
        engine.engine.feed(bytes);
        // SAFETY: null or a writable buffer struct.
        engine.collect_replies(unsafe { out_replies.as_mut() });
        TermyEngineStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_take_replies(
    engine: *mut TermyEngine,
    out_replies: *mut TermyEngineBytes,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a live handle.
        let Some(engine) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        // SAFETY: null or a writable buffer struct.
        let Some(out) = (unsafe { out_replies.as_mut() }) else {
            return TermyEngineStatus::Null;
        };
        engine.collect_replies(Some(out));
        TermyEngineStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_synchronized_deadline(
    engine: *mut TermyEngine,
    out_pending: *mut bool,
    out_remaining_ms: *mut u64,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a live handle.
        let Some(engine) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        if out_pending.is_null() || out_remaining_ms.is_null() {
            return TermyEngineStatus::Null;
        }
        let deadline = engine.engine.synchronized_update_deadline();
        let remaining = deadline.map_or(0, |deadline| {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            u64::try_from(left.as_millis()).unwrap_or(u64::MAX)
        });
        // SAFETY: checked non-null above.
        unsafe {
            *out_pending = deadline.is_some();
            *out_remaining_ms = remaining;
        }
        TermyEngineStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_tick(
    engine: *mut TermyEngine,
    force: bool,
    out_committed: *mut bool,
    out_replies: *mut TermyEngineBytes,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a live handle.
        let Some(engine) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        let due = engine
            .engine
            .synchronized_update_deadline()
            .is_some_and(|deadline| force || deadline <= std::time::Instant::now());
        let committed = due && engine.engine.stop_synchronized_update();
        // SAFETY: null or a writable flag.
        if let Some(out) = unsafe { out_committed.as_mut() } {
            *out = committed;
        }
        // SAFETY: null or a writable buffer struct.
        engine.collect_replies(unsafe { out_replies.as_mut() });
        TermyEngineStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_resize(
    engine: *mut TermyEngine,
    cols: u16,
    rows: u16,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a live handle.
        let Some(engine) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        let Some(size) = size(cols, rows) else {
            return TermyEngineStatus::InvalidArgument;
        };
        engine.engine.resize(size);
        TermyEngineStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_set_cell_pixels(
    engine: *mut TermyEngine,
    width: u16,
    height: u16,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a live handle.
        let Some(engine) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        engine
            .engine
            .set_cell_pixels(f32::from(width), f32::from(height));
        TermyEngineStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_set_scrollback(
    engine: *mut TermyEngine,
    rows: u32,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a live handle.
        let Some(engine) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        let current = engine.engine.options();
        engine.engine.set_options(Options {
            scrollback_history: scrollback_rows(rows),
            ..current
        });
        TermyEngineStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_set_scrollback_bytes(
    engine: *mut TermyEngine,
    bytes: u64,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a live handle.
        let Some(engine) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        let current = engine.engine.options();
        engine.engine.set_options(Options {
            scrollback_bytes: or_default(bytes, DEFAULT_SCROLLBACK_BYTES),
            ..current
        });
        TermyEngineStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_set_payload_limits(
    engine: *mut TermyEngine,
    max_string_bytes: u32,
    max_clipboard_bytes: u32,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a live handle.
        let Some(engine) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        let current = engine.engine.options();
        engine.engine.set_options(Options {
            max_string_bytes: or_default(max_string_bytes.into(), MAX_STRING_PAYLOAD_BYTES),
            max_clipboard_bytes: or_default(
                max_clipboard_bytes.into(),
                MAX_CLIPBOARD_PAYLOAD_BYTES,
            ),
            ..current
        });
        TermyEngineStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_clear_scrollback(
    engine: *mut TermyEngine,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a live handle.
        let Some(engine) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        engine.engine.clear_scrollback();
        TermyEngineStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_compact_history(
    engine: *mut TermyEngine,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a live handle.
        let Some(engine) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        engine.engine.compact_history();
        TermyEngineStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_snapshot_encode(
    engine: *mut TermyEngine,
    out_snapshot: *mut TermyEngineBytes,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a live handle.
        let Some(engine) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        // SAFETY: null or a writable buffer struct.
        let Some(out) = (unsafe { out_snapshot.as_mut() }) else {
            return TermyEngineStatus::Null;
        };
        *out = TermyEngineBytes::from_vec(engine.engine.encode_snapshot());
        TermyEngineStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_snapshot_encode_version(
    engine: *mut TermyEngine,
    version: u32,
    out_snapshot: *mut TermyEngineBytes,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a live handle.
        let Some(engine) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        // SAFETY: null or a writable buffer struct.
        let Some(out) = (unsafe { out_snapshot.as_mut() }) else {
            return TermyEngineStatus::Null;
        };
        *out = TermyEngineBytes::from_vec(Vec::new());
        match engine.engine.encode_snapshot_version(version) {
            Ok(bytes) => {
                *out = TermyEngineBytes::from_vec(bytes);
                TermyEngineStatus::Ok
            }
            Err(error) => error.into(),
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_snapshot_peek_version(
    snapshot_ptr: *const u8,
    snapshot_len: usize,
    out_version: *mut u32,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: valid for `snapshot_len` bytes per the header contract.
        let Some(bytes) = (unsafe { input(snapshot_ptr, snapshot_len) }) else {
            return TermyEngineStatus::Null;
        };
        if out_version.is_null() {
            return TermyEngineStatus::Null;
        }
        match snapshot_version(bytes) {
            Ok(version) => {
                // SAFETY: checked non-null above.
                unsafe { *out_version = version };
                TermyEngineStatus::Ok
            }
            Err(error) => error.into(),
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_snapshot_decode(
    snapshot_ptr: *const u8,
    snapshot_len: usize,
    reply_mode: u32,
    out_engine: *mut *mut TermyEngine,
) -> TermyEngineStatus {
    guard(|| {
        if out_engine.is_null() {
            return TermyEngineStatus::Null;
        }
        // SAFETY: checked non-null above.
        unsafe { *out_engine = ptr::null_mut() };
        // SAFETY: valid for `snapshot_len` bytes per the header contract.
        let Some(bytes) = (unsafe { input(snapshot_ptr, snapshot_len) }) else {
            return TermyEngineStatus::Null;
        };
        if reply_mode > TERMY_ENGINE_REPLIES_DISCARD {
            return TermyEngineStatus::InvalidArgument;
        }
        match Engine::decode_snapshot(bytes) {
            Ok(engine) => {
                let handle = Box::new(TermyEngine::new(engine, reply_mode));
                // SAFETY: checked non-null above.
                unsafe { *out_engine = Box::into_raw(handle) };
                TermyEngineStatus::Ok
            }
            Err(error) => error.into(),
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_mode(
    engine: *mut TermyEngine,
    mode: u16,
    ansi: bool,
    out_report: *mut u8,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a live handle.
        let Some(engine) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        if out_report.is_null() {
            return TermyEngineStatus::Null;
        }
        // DECRQM values: 0 not recognized, 1 set, 2 reset, 3 permanently set.
        let report = match engine.engine.mode(!ansi, mode) {
            None => 0,
            Some(true) if !ansi && mode == 2027 => 3,
            Some(true) => 1,
            Some(false) => 2,
        };
        // SAFETY: checked non-null above.
        unsafe { *out_report = report };
        TermyEngineStatus::Ok
    })
}

fn cursor_shape_code(shape: CursorShape) -> u8 {
    match shape {
        CursorShape::Block => 0,
        CursorShape::Beam => 1,
        CursorShape::Underline => 2,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_get_state(
    engine: *mut TermyEngine,
    out_state: *mut TermyEngineState,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a live handle.
        let Some(engine) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        // SAFETY: null or a writable state struct.
        let Some(out) = (unsafe { out_state.as_mut() }) else {
            return TermyEngineStatus::Null;
        };
        let terminal = &engine.engine;
        let size = terminal.size();
        let cursor = terminal.cursor();
        let modes = terminal.modes();
        let clamp = |value: usize| u32::try_from(value).unwrap_or(u32::MAX);
        *out = TermyEngineState {
            cols: size.cols as u16,
            rows: size.rows as u16,
            cursor_col: cursor.col as u16,
            cursor_row: cursor.row as u16,
            cursor_visible: cursor.visible,
            cursor_blinking: cursor.blinking,
            cursor_shape: cursor_shape_code(cursor.shape),
            alternate_screen: terminal.alternate_screen(),
            mouse_tracking: match modes.mouse_tracking {
                MouseTracking::None => 0,
                MouseTracking::Press => 1,
                MouseTracking::Click => 2,
                MouseTracking::Drag => 3,
                MouseTracking::Motion => 4,
            },
            mouse_encoding: match modes.mouse_encoding {
                MouseEncoding::Default => 0,
                MouseEncoding::Utf8 => 1,
                MouseEncoding::Sgr => 2,
                MouseEncoding::SgrPixels => 3,
            },
            focus_reporting: modes.focus_events,
            alternate_scroll: modes.alternate_scroll,
            bracketed_paste: modes.bracketed_paste,
            application_cursor: modes.application_cursor,
            application_keypad: modes.application_keypad,
            synchronized_output: modes.synchronized_update,
            reverse_video: modes.reverse_video,
            kitty_keyboard_flags: modes.kitty_keyboard,
            history_size: clamp(terminal.history_size()),
            display_offset: clamp(terminal.display_offset()),
            scrollback_limit: clamp(terminal.scrollback_limit()),
            scrollback_bytes: u64::try_from(terminal.scrollback_byte_limit()).unwrap_or(u64::MAX),
            generation: terminal.generation(),
            palette_revision: terminal.palette_revision(),
            scrolled_into_history: terminal.scrolled_into_history(),
        };
        TermyEngineStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_title(
    engine: *mut TermyEngine,
    out_title: *mut TermyEngineBytes,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a live handle.
        let Some(engine) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        // SAFETY: null or a writable buffer struct.
        let Some(out) = (unsafe { out_title.as_mut() }) else {
            return TermyEngineStatus::Null;
        };
        *out = TermyEngineBytes::from_vec(engine.engine.title().as_bytes().to_vec());
        TermyEngineStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_working_directory(
    engine: *mut TermyEngine,
    out_found: *mut bool,
    out_path: *mut TermyEngineBytes,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a live handle.
        let Some(engine) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        // SAFETY: null or a writable buffer struct.
        let Some(out) = (unsafe { out_path.as_mut() }) else {
            return TermyEngineStatus::Null;
        };
        if out_found.is_null() {
            return TermyEngineStatus::Null;
        }
        let path = engine.engine.working_directory();
        // SAFETY: checked non-null above.
        unsafe { *out_found = path.is_some() };
        *out = TermyEngineBytes::from_vec(path.unwrap_or_default().as_bytes().to_vec());
        TermyEngineStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_scrollbar(
    engine: *mut TermyEngine,
    out_scrollbar: *mut TermyEngineScrollbar,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a live handle.
        let Some(engine) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        // SAFETY: null or a writable struct.
        let Some(out) = (unsafe { out_scrollbar.as_mut() }) else {
            return TermyEngineStatus::Null;
        };
        let history = engine.engine.history_size() as u64;
        let rows = engine.engine.size().rows as u64;
        *out = TermyEngineScrollbar {
            total: history + rows,
            offset: history - engine.engine.display_offset() as u64,
            len: rows,
        };
        TermyEngineStatus::Ok
    })
}

/// Scroll so the viewport's top row is `offset` rows below the oldest
/// history row (0 shows the top of the scrollback).
fn scroll_to_offset(engine: &mut Engine, offset: u64) -> bool {
    let history = engine.history_size() as u64;
    let target = history - offset.min(history);
    let current = engine.display_offset() as u64;
    let delta = target as i64 - current as i64;
    delta != 0
        && engine.scroll_display(delta.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32)
}

pub const TERMY_ENGINE_SCROLL_DELTA: u32 = 0;
pub const TERMY_ENGINE_SCROLL_TOP: u32 = 1;
pub const TERMY_ENGINE_SCROLL_BOTTOM: u32 = 2;
pub const TERMY_ENGINE_SCROLL_OFFSET: u32 = 3;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_scroll_viewport(
    engine: *mut TermyEngine,
    kind: u32,
    value: i64,
    out_changed: *mut bool,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a live handle.
        let Some(engine) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        let terminal = &mut engine.engine;
        let changed = match kind {
            TERMY_ENGINE_SCROLL_DELTA => {
                let delta = value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
                delta != 0 && terminal.scroll_display(delta)
            }
            TERMY_ENGINE_SCROLL_TOP => scroll_to_offset(terminal, 0),
            TERMY_ENGINE_SCROLL_BOTTOM => {
                let offset = terminal.display_offset() as i32;
                offset != 0 && terminal.scroll_display(-offset)
            }
            TERMY_ENGINE_SCROLL_OFFSET if value >= 0 => scroll_to_offset(terminal, value as u64),
            _ => return TermyEngineStatus::InvalidArgument,
        };
        // SAFETY: null or a writable flag.
        if let Some(out) = unsafe { out_changed.as_mut() } {
            *out = changed;
        }
        TermyEngineStatus::Ok
    })
}

fn terminal_color(rgb: u32) -> TerminalColor {
    TerminalColor {
        r: (rgb >> 16) as u8,
        g: (rgb >> 8) as u8,
        b: rgb as u8,
    }
}

fn rgb(color: TerminalColor) -> u32 {
    u32::from(color.r) << 16 | u32::from(color.g) << 8 | u32::from(color.b)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_set_default_colors(
    engine: *mut TermyEngine,
    foreground: u32,
    background: u32,
    cursor: *const u32,
    palette: *const u32,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a live handle.
        let Some(engine) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        let mut colors = HostColors::default();
        if !palette.is_null() {
            // SAFETY: the header requires 256 entries when non-null.
            let palette = unsafe { slice::from_raw_parts(palette, 256) };
            for (slot, value) in colors.palette.iter_mut().zip(palette) {
                *slot = terminal_color(*value);
            }
            colors.query.ansi.copy_from_slice(&colors.palette[..16]);
        }
        colors.query.foreground = terminal_color(foreground);
        colors.query.background = terminal_color(background);
        // SAFETY: null or one color.
        colors.query.cursor = unsafe { cursor.as_ref() }.map(|value| terminal_color(*value));
        engine.engine.set_query_colors(colors.query);
        engine.colors = colors;
        TermyEngineStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_effective_colors(
    engine: *mut TermyEngine,
    out_colors: *mut TermyEngineColors,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a live handle.
        let Some(engine) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        // SAFETY: null or a writable struct.
        let Some(out) = (unsafe { out_colors.as_mut() }) else {
            return TermyEngineStatus::Null;
        };
        let terminal = &engine.engine;
        let host = &engine.colors;
        let resolve = |color: termy_core::terminal_engine::Color| {
            color
                .as_rgb()
                .map(|(r, g, b)| u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b))
        };
        let mut palette = [0; 256];
        for (index, slot) in palette.iter_mut().enumerate() {
            *slot = terminal.palette()[index]
                .and_then(resolve)
                .unwrap_or_else(|| rgb(host.palette[index]));
        }
        let mut foreground = terminal
            .foreground()
            .and_then(resolve)
            .unwrap_or_else(|| rgb(host.query.foreground));
        let mut background = terminal
            .background()
            .and_then(resolve)
            .unwrap_or_else(|| rgb(host.query.background));
        let cursor = terminal
            .cursor_color()
            .and_then(resolve)
            .or_else(|| host.query.cursor.map(rgb));
        let reverse_video = terminal.modes().reverse_video;
        if reverse_video {
            std::mem::swap(&mut foreground, &mut background);
        }
        *out = TermyEngineColors {
            foreground,
            background,
            cursor: cursor.unwrap_or(foreground),
            cursor_set: cursor.is_some(),
            reverse_video,
            palette,
        };
        TermyEngineStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_next_event(
    engine: *mut TermyEngine,
    out_found: *mut bool,
    out_event: *mut TermyEngineEvent,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a live handle.
        let Some(engine) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        // SAFETY: null or writable outputs.
        let (Some(found), Some(out)) =
            (unsafe { out_found.as_mut() }, unsafe { out_event.as_mut() })
        else {
            return TermyEngineStatus::Null;
        };
        *found = false;
        *out = TermyEngineEvent {
            kind: 0,
            value: 0,
            payload: TermyEngineBytes::EMPTY,
            data: TermyEngineBytes::EMPTY,
        };
        let text = |text: String| TermyEngineBytes::from_vec(text.into_bytes());
        // Program status is coalesced apart from the bounded event queue.
        if let Some(records) = engine.engine.take_program_status() {
            out.kind = TERMY_ENGINE_EVENT_PROGRAM_STATUS;
            out.payload = text(serde_json::to_string(&records).unwrap_or_else(|_| "[]".into()));
            *found = true;
            return TermyEngineStatus::Ok;
        }
        while let Some(event) = engine.engine.pop_event() {
            match event {
                Event::Bell => out.kind = TERMY_ENGINE_EVENT_BELL,
                Event::Title(title) => {
                    out.kind = TERMY_ENGINE_EVENT_TITLE;
                    out.payload = text(title);
                }
                Event::ResetTitle => out.kind = TERMY_ENGINE_EVENT_RESET_TITLE,
                Event::WorkingDirectory(path) => {
                    out.kind = TERMY_ENGINE_EVENT_WORKING_DIRECTORY;
                    out.payload = text(path);
                }
                Event::ShellIntegration(payload) => {
                    out.kind = TERMY_ENGINE_EVENT_SHELL_INTEGRATION;
                    out.payload = text(payload);
                }
                Event::Progress(progress) => {
                    use termy_core::ProgressState;
                    let (state, value) = match progress {
                        ProgressState::Clear => (0, 0),
                        ProgressState::InProgress(value) => (1, value),
                        ProgressState::Error(value) => (2, value),
                        ProgressState::Indeterminate => (3, 0),
                        ProgressState::Warning(value) => (4, value),
                    };
                    out.kind = TERMY_ENGINE_EVENT_PROGRESS;
                    out.value = state << 8 | i32::from(value);
                }
                Event::Clipboard { selection, data } => {
                    out.kind = TERMY_ENGINE_EVENT_CLIPBOARD;
                    out.payload = text(selection);
                    out.data = text(data);
                }
                // Kitty OSC 5522 needs a permission-aware host; not exposed.
                Event::KittyClipboard(_) | Event::KittyClipboardControl(_) => continue,
            }
            *found = true;
            break;
        }
        TermyEngineStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_event_free(event: *mut TermyEngineEvent) {
    // SAFETY: null or an event this library filled.
    let Some(event) = (unsafe { event.as_mut() }) else {
        return;
    };
    // SAFETY: both buffers came from this library.
    unsafe {
        termy_engine_bytes_free(&raw mut event.payload);
        termy_engine_bytes_free(&raw mut event.data);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_process_exited(
    engine: *mut TermyEngine,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a live handle.
        let Some(engine) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        engine.engine.process_exited();
        TermyEngineStatus::Ok
    })
}

#[cfg(test)]
mod tests;
