//! A small experimental terminal engine for Termy.
//!
//! Tmon deliberately uses only the Rust standard library. It owns its VT parser,
//! bounded grid/scrollback, damage tracking, and native PTY lifecycle. The
//! desktop app keeps it behind `TERMY_EXPERIMENTAL_TMON_ENGINE=1`; the regular
//! native and tmux engines do not depend on this runtime path.

mod graphics;
mod graphics_geometry;
mod grid;
mod inflate;
#[doc(hidden)]
pub mod kitty_graphics_unicode;
pub use graphics_geometry::{GraphicsRowSpan, graphics_display_size};
mod graphics_animation;
pub use graphics_animation::{
    GraphicsAnimation, GraphicsAnimationControl, GraphicsComposition, GraphicsFrameUpdate,
};
mod graphics_shared_memory;
pub use graphics_shared_memory::read_graphics_shared_memory;
mod parser;
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
mod pty;
#[cfg(target_os = "windows")]
#[path = "pty_windows.rs"]
mod pty;
mod pty_limits;
mod terminal_read;
mod unicode_width;

#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "windows"
))]
use std::sync::Weak;
use std::{
    collections::VecDeque,
    fmt, io,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

pub use graphics::{GraphicsImage, GraphicsRenderPlacement};
pub use grid::{
    Attributes, Cell, Color, Combining, CursorState, CursorStyle, DamageSnapshot, DirtySpan,
    Hyperlink, KeyboardMode, LinkMatch, MouseMode, Palette, Rgb, ScrollDamage, ScrollDirection,
    Snapshot, UnderlineStyle, ViewportLink, ViewportMetadata,
};
pub use parser::{Progress, QueryColors};

use grid::{Grid, LinkCandidate, MAX_GRID_DIMENSION};
use parser::{ParsedEvent, Parser};
use pty_limits::MAX_PROTOCOL_REPLY_BACKLOG_BYTES as MAX_BUFFERED_PROTOCOL_REPLY_BYTES;

const MAX_DRAIN_EVENTS: usize = 2048;
const MAX_QUEUED_EVENTS: usize = 8192;
const SYNC_WATCHDOG_DELAY: Duration = Duration::from_millis(175);

type ProtocolReplySink = Arc<dyn Fn(Vec<u8>) + Send + Sync>;

#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "windows"
))]
struct PtyProtocolReplyTarget {
    pty: Option<Weak<pty::Pty>>,
    pending: VecDeque<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: f32,
    pub cell_height: f32,
}

impl Default for Size {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            cell_width: 9.0,
            cell_height: 18.0,
        }
    }
}

impl Size {
    fn clamped(self) -> Self {
        Self {
            cols: self.cols.clamp(1, MAX_GRID_DIMENSION),
            rows: self.rows.clamp(1, MAX_GRID_DIMENSION),
            cell_width: finite_cell_metric(self.cell_width),
            cell_height: finite_cell_metric(self.cell_height),
        }
    }
}

/// Stable bounds captured while visiting terminal lines under one engine lock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalLineRange {
    pub first_line: i32,
    pub last_line: i32,
    pub columns: usize,
}

/// Read-only terminal state captured under one engine lock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalStateSnapshot {
    pub size: Size,
    pub display_offset: usize,
    pub history_size: usize,
    pub cursor_state: Option<CursorState>,
    pub bracketed_paste_mode: bool,
    pub kitty_clipboard_paste_events_mode: bool,
    pub alternate_screen_mode: bool,
    pub mouse_mode: MouseMode,
    pub keyboard_mode: KeyboardMode,
    pub child_pid: Option<u32>,
}

/// Whether this host can start a native Tmon PTY session.
///
/// Native PTYs are available on Linux, Android, and macOS. Windows support
/// is detected at runtime so loading Termy on a pre-ConPTY Windows build does
/// not make the regular Alacritty engine unavailable.
pub fn native_pty_available() -> bool {
    #[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
    {
        true
    }
    #[cfg(target_os = "windows")]
    {
        pty::available()
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "windows"
    )))]
    {
        false
    }
}

fn finite_cell_metric(value: f32) -> f32 {
    if value.is_finite() {
        value.max(1.0)
    } else {
        1.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Launch {
    ShellCommand(String),
    Program { program: String, args: Vec<String> },
}

#[derive(Clone, Debug)]
pub struct Config {
    pub shell: Option<String>,
    pub working_directory: Option<PathBuf>,
    pub environment: Vec<(String, String)>,
    pub launch: Option<Launch>,
    pub scrollback_history: usize,
    pub default_cursor_style: CursorStyle,
    pub query_colors: QueryColors,
    pub osc52: Osc52,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            shell: None,
            working_directory: None,
            environment: Vec::new(),
            launch: None,
            scrollback_history: 1000,
            default_cursor_style: CursorStyle::Block,
            query_colors: QueryColors::default(),
            osc52: Osc52::OnlyCopy,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Osc52 {
    Disabled,
    #[default]
    OnlyCopy,
    OnlyPaste,
    CopyPaste,
}

impl Osc52 {
    fn allows_copy(self) -> bool {
        matches!(self, Self::OnlyCopy | Self::CopyPaste)
    }

    fn allows_paste(self) -> bool {
        matches!(self, Self::OnlyPaste | Self::CopyPaste)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardTarget {
    Clipboard,
    Selection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipboardRequest {
    target: ClipboardTarget,
    selector: u8,
    bell_terminated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KittyClipboardPacket {
    body: Vec<u8>,
    bell_terminated: bool,
}

impl KittyClipboardPacket {
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    pub fn bell_terminated(&self) -> bool {
        self.bell_terminated
    }
}

impl ClipboardRequest {
    pub fn target(self) -> ClipboardTarget {
        self.target
    }

    pub fn format_reply(self, text: &str) -> Vec<u8> {
        let mut reply = format!(
            "\x1b]52;{};{}",
            char::from(self.selector),
            encode_base64(text.as_bytes())
        )
        .into_bytes();
        if self.bell_terminated {
            reply.push(0x07);
        } else {
            reply.extend_from_slice(b"\x1b\\");
        }
        reply
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalOptions {
    pub scrollback_history: usize,
    pub default_cursor_style: CursorStyle,
}

impl Default for TerminalOptions {
    fn default() -> Self {
        Self {
            scrollback_history: 1000,
            default_cursor_style: CursorStyle::Block,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Wakeup,
    Title(String),
    ResetTitle,
    Bell,
    Exit,
    ClipboardStore(String),
    ClipboardLoad(ClipboardRequest),
    KittyClipboard(KittyClipboardPacket),
    KittyClipboardMode(bool),
    KittyClipboardReset,
    ShellPromptStart,
    ShellCommandStart,
    ShellCommandExecuting,
    ShellCommandFinished(Option<i32>),
    Progress(Progress),
    WorkingDirectory(String),
}

#[derive(Clone)]
pub struct WakeupNotifier {
    notify: Arc<dyn Fn() + Send + Sync>,
}

impl WakeupNotifier {
    pub fn new(notify: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            notify: Arc::new(notify),
        }
    }

    fn notify(&self) {
        (self.notify)();
    }
}

#[derive(Debug)]
pub struct Error {
    inner: io::Error,
}

impl Error {
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "windows"
    )))]
    fn unsupported(message: &'static str) -> Self {
        Self {
            inner: io::Error::new(io::ErrorKind::Unsupported, message),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(formatter)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.inner)
    }
}

impl From<io::Error> for Error {
    fn from(inner: io::Error) -> Self {
        Self { inner }
    }
}

#[derive(Debug)]
struct Engine {
    parser: Parser,
    grid: Grid,
    size: Size,
    render_generation: u64,
}

/// Damage plus ordered viewport row permutations for incremental render caches.
///
/// The existing [`Terminal::take_damage_snapshot`] API intentionally reports a
/// conservative full frame for scrolls. Renderers that understand row movement
/// can use this additive API and validate `generation` while visiting dirty cells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderDamageSnapshot {
    /// Full damage, or dirty spans expressed in final viewport coordinates.
    pub damage: DamageSnapshot,
    /// Chronological row permutations to replay before patching partial spans.
    /// This is empty whenever `damage` is full.
    pub scrolls: Vec<ScrollDamage>,
    /// Monotonic engine generation used by
    /// [`Terminal::for_each_viewport_range_at_generation`].
    pub generation: u64,
}

/// Coherent renderer state captured while visiting one frame update.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameUpdate {
    pub render: RenderDamageSnapshot,
    pub size: Size,
    pub viewport: ViewportMetadata,
    pub palette: Palette,
    pub graphics_revision: u64,
    pub graphics_placements: Vec<GraphicsRenderPlacement>,
}

struct EngineOutput {
    events: Vec<Event>,
    replies: Vec<u8>,
    synchronized_update_active: bool,
    synchronized_update_deadline: Option<Instant>,
    synchronized_update_refreshed: bool,
    synchronized_update_committed: bool,
    unsynchronized_activity: bool,
}

impl Engine {
    fn new(size: Size, config: &Config) -> Self {
        let mut parser = Parser::default();
        parser.set_size(size);
        parser.set_query_colors(config.query_colors);
        parser.set_osc52(config.osc52);
        let mut engine = Self {
            parser,
            grid: Grid::new(
                size.cols,
                size.rows,
                config.scrollback_history,
                config.default_cursor_style,
            ),
            size,
            render_generation: 0,
        };
        engine
            .grid
            .set_cell_metrics(size.cell_width, size.cell_height);
        engine
    }

    fn feed_detailed(&mut self, bytes: &[u8]) -> EngineOutput {
        let resize_anchor_was_suppressed = self.grid.resize_anchor_suppressed_after_clear();
        let parsed = self.parser.advance(&mut self.grid, bytes);
        self.grid
            .observe_live_output_for_resize_anchor(resize_anchor_was_suppressed);
        self.bump_render_generation();
        let events = parsed.events.into_iter().map(Event::from).collect();
        EngineOutput {
            events,
            replies: parsed.replies,
            synchronized_update_active: parsed.synchronized_update_active,
            synchronized_update_deadline: parsed.synchronized_update_deadline,
            synchronized_update_refreshed: parsed.synchronized_update_refreshed,
            synchronized_update_committed: parsed.synchronized_update_committed,
            unsynchronized_activity: parsed.unsynchronized_activity,
        }
    }

    fn feed(&mut self, bytes: &[u8]) -> (Vec<Event>, Vec<u8>, bool) {
        let output = self.feed_detailed(bytes);
        (
            output.events,
            output.replies,
            output.synchronized_update_active,
        )
    }

    fn stop_synchronized_update(&mut self) -> EngineOutput {
        let parsed = self.parser.stop_synchronized_update(&mut self.grid);
        self.bump_render_generation();
        EngineOutput {
            events: parsed.events.into_iter().map(Event::from).collect(),
            replies: parsed.replies,
            synchronized_update_active: parsed.synchronized_update_active,
            synchronized_update_deadline: parsed.synchronized_update_deadline,
            synchronized_update_refreshed: parsed.synchronized_update_refreshed,
            synchronized_update_committed: parsed.synchronized_update_committed,
            unsynchronized_activity: parsed.unsynchronized_activity,
        }
    }

    fn bump_render_generation(&mut self) {
        self.render_generation = self.render_generation.wrapping_add(1);
    }
}

impl From<ParsedEvent> for Event {
    fn from(event: ParsedEvent) -> Self {
        match event {
            ParsedEvent::Title(title) => Self::Title(title),
            ParsedEvent::ResetTitle => Self::ResetTitle,
            ParsedEvent::Bell => Self::Bell,
            ParsedEvent::ClipboardStore(text) => Self::ClipboardStore(text),
            ParsedEvent::ClipboardLoad(request) => Self::ClipboardLoad(request),
            ParsedEvent::KittyClipboard(packet) => Self::KittyClipboard(packet),
            ParsedEvent::KittyClipboardMode(enabled) => Self::KittyClipboardMode(enabled),
            ParsedEvent::KittyClipboardReset => Self::KittyClipboardReset,
            ParsedEvent::ShellPromptStart => Self::ShellPromptStart,
            ParsedEvent::ShellCommandStart => Self::ShellCommandStart,
            ParsedEvent::ShellCommandExecuting => Self::ShellCommandExecuting,
            ParsedEvent::ShellCommandFinished(exit_code) => Self::ShellCommandFinished(exit_code),
            ParsedEvent::Progress(progress) => Self::Progress(progress),
            ParsedEvent::WorkingDirectory(path) => Self::WorkingDirectory(path),
        }
    }
}

fn encode_base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        output.push(char::from(TABLE[((value >> 18) & 63) as usize]));
        output.push(char::from(TABLE[((value >> 12) & 63) as usize]));
        output.push(if chunk.len() > 1 {
            char::from(TABLE[((value >> 6) & 63) as usize])
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            char::from(TABLE[(value & 63) as usize])
        } else {
            '='
        });
    }
    output
}

fn decode_base64(input: &[u8]) -> Option<Vec<u8>> {
    let mut output = Vec::new();
    output.try_reserve(input.len().saturating_mul(3) / 4).ok()?;
    let mut quartet = [0u8; 4];
    let mut quartet_len = 0usize;
    let mut padding = 0usize;

    for &byte in input {
        if matches!(byte, b'\r' | b'\n' | b'\t' | b' ') {
            continue;
        }
        if byte == b'=' {
            padding = padding.saturating_add(1);
            if padding > 2 {
                return None;
            }
            continue;
        }
        if padding != 0 {
            return None;
        }
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        quartet[quartet_len] = value;
        quartet_len += 1;
        if quartet_len == 4 {
            output.extend_from_slice(&[
                (quartet[0] << 2) | (quartet[1] >> 4),
                (quartet[1] << 4) | (quartet[2] >> 2),
                (quartet[2] << 6) | quartet[3],
            ]);
            quartet_len = 0;
        }
    }

    match (quartet_len, padding) {
        (0, 0) => {}
        (2, 0 | 2) if quartet[1] & 0x0f == 0 => {
            output.push((quartet[0] << 2) | (quartet[1] >> 4));
        }
        (3, 0 | 1) if quartet[2] & 0x03 == 0 => {
            output.extend_from_slice(&[
                (quartet[0] << 2) | (quartet[1] >> 4),
                (quartet[1] << 4) | (quartet[2] >> 2),
            ]);
        }
        _ => return None,
    }
    Some(output)
}

/// Independent Tmon terminal state and optional PTY session.
pub struct Terminal {
    engine: Arc<Mutex<Engine>>,
    events: Arc<Mutex<VecDeque<Event>>>,
    protocol_replies: Arc<Mutex<VecDeque<u8>>>,
    protocol_reply_sink: ProtocolReplySink,
    wakeup_queued: Arc<AtomicBool>,
    wakeup_enabled: Arc<AtomicBool>,
    sync_batch_active: Arc<AtomicBool>,
    sync_generation: Arc<AtomicU64>,
    wakeup_notifier: Option<WakeupNotifier>,
    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "windows"
    ))]
    pty: Option<Arc<pty::Pty>>,
}

impl Terminal {
    /// Start a terminal backed by an independent PTY process.
    pub fn new(
        size: Size,
        config: Config,
        wakeup_notifier: Option<WakeupNotifier>,
    ) -> Result<Self, Error> {
        let size = size.clamped();
        let engine = Arc::new(Mutex::new(Engine::new(size, &config)));
        let events: Arc<Mutex<VecDeque<Event>>> = Arc::new(Mutex::new(VecDeque::with_capacity(64)));
        let protocol_replies: Arc<Mutex<VecDeque<u8>>> = Arc::new(Mutex::new(VecDeque::new()));
        let wakeup_queued = Arc::new(AtomicBool::new(false));
        let wakeup_enabled = Arc::new(AtomicBool::new(true));
        let sync_batch_active = Arc::new(AtomicBool::new(false));
        let sync_generation = Arc::new(AtomicU64::new(0));

        #[cfg(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "windows"
        ))]
        let (pty, protocol_reply_sink) = {
            let reply_target = Arc::new(Mutex::new(PtyProtocolReplyTarget {
                pty: None,
                pending: VecDeque::new(),
            }));
            let sink_target = reply_target.clone();
            let protocol_reply_sink: ProtocolReplySink = Arc::new(move |reply| {
                let mut target = sink_target
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let pty = target.pty.as_ref().and_then(Weak::upgrade);
                if let Some(pty) = pty {
                    drop(target);
                    // The priority method closes the session if its bounded
                    // reply lane cannot accept a response.
                    drop(pty.write_protocol_reply_owned(reply));
                } else if target.pty.is_none() {
                    buffer_protocol_reply_queue(&mut target.pending, reply);
                }
            });
            let output_engine = engine.clone();
            let output_events = events.clone();
            let output_wakeup_queued = wakeup_queued.clone();
            let output_enabled = wakeup_enabled.clone();
            let output_notifier = wakeup_notifier.clone();
            let output_sync_active = sync_batch_active.clone();
            let output_sync_generation = sync_generation.clone();
            let output_protocol_reply_sink = protocol_reply_sink.clone();

            let exit_events = events.clone();
            let exit_wakeup_queued = wakeup_queued.clone();
            let exit_enabled = wakeup_enabled.clone();
            let exit_notifier = wakeup_notifier.clone();
            let spawn = resolve_spawn_config(config)?;
            let pty = Arc::new(pty::Pty::spawn(
                spawn,
                size,
                move |bytes| {
                    process_output(
                        &output_engine,
                        &output_events,
                        &output_wakeup_queued,
                        &output_enabled,
                        output_notifier.as_ref(),
                        &output_sync_active,
                        &output_sync_generation,
                        &output_protocol_reply_sink,
                        bytes,
                        true,
                    )
                },
                move || {
                    queue_events(
                        &exit_events,
                        &exit_wakeup_queued,
                        &exit_enabled,
                        exit_notifier.as_ref(),
                        [Event::Exit],
                        false,
                        true,
                    );
                },
            )?);
            let pending = {
                let mut target = reply_target
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                target.pty = Some(Arc::downgrade(&pty));
                target.pending.drain(..).collect::<Vec<_>>()
            };
            if !pending.is_empty() {
                pty.write_protocol_reply_owned(pending)?;
            }
            (Some(pty), protocol_reply_sink)
        };

        #[cfg(not(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "windows"
        )))]
        {
            let _ = config;
            let _ = engine;
            let _ = events;
            let _ = protocol_replies;
            let _ = wakeup_queued;
            let _ = wakeup_enabled;
            let _ = sync_batch_active;
            let _ = sync_generation;
            let _ = wakeup_notifier;
            return Err(Error::unsupported(
                "the experimental tmon PTY currently supports Linux, Android, macOS, and Windows hosts only",
            ));
        }

        #[cfg(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "windows"
        ))]
        Ok(Self {
            engine,
            events,
            protocol_replies,
            protocol_reply_sink,
            wakeup_queued,
            wakeup_enabled,
            sync_batch_active,
            sync_generation,
            wakeup_notifier,
            pty,
        })
    }

    /// Construct a parser/grid surface with no child process. Useful for replay and tests.
    pub fn new_display(size: Size, config: Config) -> Self {
        Self::new_display_inner(size, config, None)
    }

    /// Construct a display-only surface with a host wakeup notifier.
    pub fn new_display_with_wakeup_notifier(
        size: Size,
        config: Config,
        wakeup_notifier: WakeupNotifier,
    ) -> Self {
        Self::new_display_inner(size, config, Some(wakeup_notifier))
    }

    fn new_display_inner(
        size: Size,
        config: Config,
        wakeup_notifier: Option<WakeupNotifier>,
    ) -> Self {
        let size = size.clamped();
        let protocol_replies = Arc::new(Mutex::new(VecDeque::new()));
        let sink_replies = protocol_replies.clone();
        let protocol_reply_sink: ProtocolReplySink = Arc::new(move |reply| {
            buffer_protocol_reply(&sink_replies, reply);
        });
        Self {
            engine: Arc::new(Mutex::new(Engine::new(size, &config))),
            events: Arc::new(Mutex::new(VecDeque::with_capacity(16))),
            protocol_replies,
            protocol_reply_sink,
            wakeup_queued: Arc::new(AtomicBool::new(false)),
            wakeup_enabled: Arc::new(AtomicBool::new(true)),
            sync_batch_active: Arc::new(AtomicBool::new(false)),
            sync_generation: Arc::new(AtomicU64::new(0)),
            wakeup_notifier,
            #[cfg(any(
                target_os = "linux",
                target_os = "android",
                target_os = "macos",
                target_os = "windows"
            ))]
            pty: None,
        }
    }

    pub fn child_pid(&self) -> Option<u32> {
        #[cfg(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "windows"
        ))]
        {
            self.pty.as_ref().map(|pty| pty.child_pid())
        }
        #[cfg(not(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "windows"
        )))]
        {
            None
        }
    }

    pub fn set_wakeup_enabled(&self, enabled: bool) {
        let was_enabled = self.wakeup_enabled.swap(enabled, Ordering::AcqRel);
        if enabled
            && !was_enabled
            && !self
                .events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
            && let Some(notifier) = &self.wakeup_notifier
        {
            notifier.notify();
        }
    }

    pub fn write(&self, input: &[u8]) {
        #[cfg(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "windows"
        ))]
        if let Some(pty) = &self.pty {
            let _ = pty.write(input);
        }
        #[cfg(not(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "windows"
        )))]
        let _ = input;
    }

    pub fn write_owned(&self, input: Vec<u8>) {
        #[cfg(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "windows"
        ))]
        if let Some(pty) = &self.pty {
            let _ = pty.write_owned(input);
            return;
        }
        let _ = input;
    }

    /// Send a terminal-protocol response to the child PTY.
    ///
    /// Live terminals use the bounded priority reply lane so required responses
    /// are not trapped behind ordinary input. Display-only terminals retain the
    /// bytes for [`Self::drain_protocol_replies`].
    pub fn write_protocol_reply_owned(&self, reply: Vec<u8>) {
        if !reply.is_empty() {
            (self.protocol_reply_sink)(reply);
        }
    }

    /// Parse bytes into the grid without writing them to the child PTY.
    pub fn hydrate_output(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let mut engine = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = engine.feed(bytes);
        engine.parser.reset_transient_state_after_hydration();
        self.sync_batch_active.store(false, Ordering::Release);
        self.sync_generation.fetch_add(1, Ordering::AcqRel);
    }

    pub fn feed_output(&self, bytes: &[u8]) {
        let replies = process_output(
            &self.engine,
            &self.events,
            &self.wakeup_queued,
            &self.wakeup_enabled,
            self.wakeup_notifier.as_ref(),
            &self.sync_batch_active,
            &self.sync_generation,
            &self.protocol_reply_sink,
            bytes,
            true,
        );
        if !replies.is_empty() {
            (self.protocol_reply_sink)(replies);
        }
    }

    /// Drain protocol replies produced by a display-only terminal.
    ///
    /// PTY-backed terminals write these directly to their child. Display-only
    /// hosts can forward the returned bytes to their own transport.
    pub fn drain_protocol_replies(&self) -> Vec<u8> {
        self.protocol_replies
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain(..)
            .collect()
    }

    /// Drain at most `limit` pending display-only protocol reply bytes.
    pub fn drain_protocol_replies_bounded(&self, limit: usize) -> (Vec<u8>, bool) {
        let mut replies = self
            .protocol_replies
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let count = replies.len().min(limit);
        let drained = replies.drain(..count).collect();
        (drained, !replies.is_empty())
    }

    /// Discard at most `limit` pending display-only protocol reply bytes.
    pub fn discard_protocol_replies(&self, limit: usize) -> (usize, bool) {
        let mut replies = self
            .protocol_replies
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let count = replies.len().min(limit);
        replies.drain(..count);
        (count, !replies.is_empty())
    }

    /// Returns whether events are currently queued for the host.
    pub fn has_pending_events(&self) -> bool {
        !self
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    }

    pub fn drain_events(&self) -> (Vec<Event>, bool) {
        let mut queue = self
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let count = queue.len().min(MAX_DRAIN_EVENTS);
        let events: Vec<Event> = queue.drain(..count).collect();
        if events.iter().any(|event| matches!(event, Event::Wakeup)) {
            self.wakeup_queued.store(false, Ordering::Release);
        }
        let has_more = !queue.is_empty();
        (events, has_more)
    }

    pub fn resize(&self, size: Size) {
        let size = size.clamped();
        {
            let mut engine = self
                .engine
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if size == engine.size {
                return;
            }
            let Engine {
                parser,
                grid,
                size: engine_size,
                render_generation,
            } = &mut *engine;
            let graphics_can_change = parser.has_graphics_placements();
            grid.resize(size.cols, size.rows);
            *engine_size = size;
            parser.set_size(size);
            grid.set_cell_metrics(size.cell_width, size.cell_height);
            let graphics_changed = parser.sync_grid_effects(grid);
            if graphics_can_change && !graphics_changed {
                parser.bump_graphics_revision();
            }
            *render_generation = render_generation.wrapping_add(1);
        }
        #[cfg(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "windows"
        ))]
        if let Some(pty) = &self.pty {
            let _ = pty.resize(size);
        }
    }

    pub fn nudge_resize(&self) {
        #[cfg(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "windows"
        ))]
        if let Some(pty) = &self.pty {
            let _ = pty.resize(self.size());
        }
    }

    pub fn size(&self) -> Size {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .size
    }

    /// Capture inspector-style metadata without repeatedly locking the engine.
    pub fn state_snapshot(&self) -> TerminalStateSnapshot {
        let child_pid = self.child_pid();
        let engine = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        TerminalStateSnapshot {
            size: engine.size,
            display_offset: engine.grid.display_offset(),
            history_size: engine.grid.history_size(),
            cursor_state: engine.grid.cursor_state(),
            bracketed_paste_mode: engine.grid.bracketed_paste_mode(),
            kitty_clipboard_paste_events_mode: engine.parser.kitty_clipboard_paste_events_mode(),
            alternate_screen_mode: engine.grid.alternate_screen_mode(),
            mouse_mode: engine.grid.mouse_mode(),
            keyboard_mode: engine.grid.keyboard_mode(),
            child_pid,
        }
    }

    pub fn set_options(&self, options: TerminalOptions) {
        let mut engine = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Engine {
            parser,
            grid,
            render_generation,
            ..
        } = &mut *engine;
        grid.set_history_limit(options.scrollback_history);
        grid.set_default_cursor_style(options.default_cursor_style);
        parser.sync_grid_effects(grid);
        *render_generation = render_generation.wrapping_add(1);
    }

    pub fn set_query_colors(&self, colors: QueryColors) {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .parser
            .set_query_colors(colors);
    }

    pub fn scroll_display(&self, delta_lines: i32) -> bool {
        let mut engine = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let graphics_can_change = engine.parser.has_primary_graphics_placements();
        let changed = engine.grid.scroll_display(delta_lines);
        if changed {
            engine.bump_render_generation();
            if graphics_can_change {
                engine.parser.bump_graphics_revision();
            }
        }
        changed
    }

    pub fn scroll_to_bottom(&self) -> bool {
        let mut engine = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let graphics_can_change = engine.parser.has_primary_graphics_placements();
        let changed = engine.grid.scroll_to_bottom();
        if changed {
            engine.bump_render_generation();
            if graphics_can_change {
                engine.parser.bump_graphics_revision();
            }
        }
        changed
    }

    pub fn clear_scrollback(&self) -> bool {
        let mut engine = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Engine {
            parser,
            grid,
            render_generation,
            ..
        } = &mut *engine;
        let changed = grid.clear_history();
        parser.sync_grid_effects(grid);
        if changed {
            *render_generation = render_generation.wrapping_add(1);
        }
        changed
    }

    pub fn scroll_state(&self) -> (usize, usize) {
        let engine = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (engine.grid.display_offset(), engine.grid.history_size())
    }

    pub fn cursor_state(&self) -> Option<CursorState> {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grid
            .cursor_state()
    }

    pub fn cursor_position(&self) -> (usize, usize) {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grid
            .render_cursor_position()
    }

    pub fn bracketed_paste_mode(&self) -> bool {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grid
            .bracketed_paste_mode()
    }

    pub fn kitty_clipboard_paste_events_mode(&self) -> bool {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .parser
            .kitty_clipboard_paste_events_mode()
    }

    pub fn alternate_screen_mode(&self) -> bool {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grid
            .alternate_screen_mode()
    }

    pub fn mouse_mode(&self) -> MouseMode {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grid
            .mouse_mode()
    }

    pub fn keyboard_mode(&self) -> KeyboardMode {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grid
            .keyboard_mode()
    }

    pub fn take_damage_snapshot(&self) -> DamageSnapshot {
        self.take_damage_snapshot_with_viewport_state().0
    }

    /// Take renderer-aware damage without losing ordered row movement.
    pub fn take_render_damage_snapshot(&self) -> RenderDamageSnapshot {
        self.take_render_damage_snapshot_with_palette_revision().0
    }

    /// Visit one coherent renderer update and consume only the represented damage.
    ///
    /// The callback runs under the engine lock and must not call back into this terminal.
    pub fn visit_frame_update(
        &self,
        mut visitor: impl FnMut(usize, usize, i32, usize, &Cell, Option<Combining<'_>>),
    ) -> FrameUpdate {
        self.visit_frame_update_with_options(false, &mut visitor)
    }

    /// Visit one coherent renderer update, optionally forcing a full rebuild.
    pub fn visit_frame_update_with_options(
        &self,
        force_full: bool,
        mut visitor: impl FnMut(usize, usize, i32, usize, &Cell, Option<Combining<'_>>),
    ) -> FrameUpdate {
        let mut engine = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (damage, scrolls) = if force_full {
            (DamageSnapshot::Full, Vec::new())
        } else {
            engine.grid.render_damage_snapshot()
        };
        let ranges = match &damage {
            DamageSnapshot::Full => (0..usize::from(engine.size.rows))
                .map(|row| (row, 0, usize::from(engine.size.cols).saturating_sub(1)))
                .collect::<Vec<_>>(),
            DamageSnapshot::Partial(spans) => spans
                .iter()
                .map(|span| (span.row, span.left_col, span.right_col))
                .collect(),
        };
        let display_offset = engine.grid.for_each_viewport_range(ranges, &mut visitor);
        let viewport = ViewportMetadata {
            cols: engine.size.cols,
            rows: engine.size.rows,
            cursor: engine.grid.cursor_state(),
            display_offset,
            history_size: engine.grid.history_size(),
        };
        engine.grid.clear_damage();
        FrameUpdate {
            render: RenderDamageSnapshot {
                damage,
                scrolls,
                generation: engine.render_generation,
            },
            size: engine.size,
            viewport,
            palette: engine.grid.palette(),
            graphics_revision: engine.parser.graphics_revision(),
            graphics_placements: engine.parser.graphics_placements(&engine.grid),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.snapshot_with_palette().0
    }

    pub fn palette(&self) -> Palette {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grid
            .palette()
    }

    /// Monotonic revision for rendered Kitty placement state.
    pub fn kitty_graphics_revision(&self) -> u64 {
        let mut engine = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        engine.parser.advance_graphics();
        engine.parser.graphics_revision()
    }

    pub fn kitty_graphics_placements(&self) -> Vec<GraphicsRenderPlacement> {
        self.kitty_graphics_snapshot().1
    }

    /// Capture the Kitty revision and visible placements under one engine lock.
    pub fn kitty_graphics_snapshot(&self) -> (u64, Vec<GraphicsRenderPlacement>) {
        let mut engine = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        engine.parser.advance_graphics();
        (
            engine.parser.graphics_revision(),
            engine.parser.graphics_placements(&engine.grid),
        )
    }

    #[inline]
    pub fn for_each_viewport_cell(
        &self,
        visitor: impl FnMut(usize, i32, usize, &Cell, Option<Combining<'_>>),
    ) -> usize {
        self.visit_viewport_cells(visitor).display_offset
    }

    /// Visit every visible cell and return metadata from the same engine lock.
    #[inline]
    pub fn visit_viewport_cells(
        &self,
        visitor: impl FnMut(usize, i32, usize, &Cell, Option<Combining<'_>>),
    ) -> ViewportMetadata {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grid
            .visit_viewport_cells(visitor)
    }

    /// Visit a coherent full viewport and consume all damage already represented by it.
    ///
    /// Incremental renderers use this for fallback rebuilds. Clearing pending
    /// row permutations under the same lock prevents a freshly rebuilt cache
    /// from replaying those permutations a second time on its next update.
    #[inline]
    pub fn visit_viewport_cells_and_clear_damage(
        &self,
        visitor: impl FnMut(usize, i32, usize, &Cell, Option<Combining<'_>>),
    ) -> ViewportMetadata {
        let mut engine = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let viewport = engine.grid.visit_viewport_cells(visitor);
        engine.grid.clear_damage();
        viewport
    }

    pub fn with_viewport_cell<R>(
        &self,
        row: usize,
        col: usize,
        f: impl FnOnce(usize, i32, &Cell) -> R,
    ) -> Option<R> {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grid
            .with_viewport_cell(row, col, f)
    }

    pub fn hyperlink_at(&self, row: usize, col: usize) -> Option<Hyperlink> {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grid
            .hyperlink_at(row, col)
    }

    /// Resolve the OSC 8 or classified text link under a viewport cell.
    ///
    /// Grid traversal happens under one engine lock. Text classification runs
    /// after releasing that lock, so host classifiers may safely perform file
    /// lookups or call back into this terminal.
    pub fn link_at(
        &self,
        row: usize,
        col: usize,
        classifier: impl FnOnce(&[char], usize) -> Option<LinkMatch>,
    ) -> Option<ViewportLink> {
        let candidate = {
            self.engine
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .grid
                .link_candidate(row, col)?
        };
        match candidate {
            LinkCandidate::Resolved(link) => Some(link),
            LinkCandidate::Text(candidate) => {
                let (characters, hovered_index) = candidate.classifier_input();
                let link_match = classifier(characters, hovered_index)?;
                candidate.resolve(link_match)
            }
        }
    }

    pub fn for_each_viewport_range(
        &self,
        ranges: impl IntoIterator<Item = (usize, usize, usize)>,
        visitor: impl FnMut(usize, usize, i32, usize, &Cell, Option<Combining<'_>>),
    ) -> usize {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grid
            .for_each_viewport_range(ranges, visitor)
    }

    /// Visit viewport ranges only when they still match a previously captured render generation.
    pub fn for_each_viewport_range_at_generation(
        &self,
        generation: u64,
        ranges: impl IntoIterator<Item = (usize, usize, usize)>,
        visitor: impl FnMut(usize, usize, i32, usize, &Cell, Option<Combining<'_>>),
    ) -> Option<usize> {
        let engine = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if engine.render_generation != generation {
            return None;
        }
        Some(engine.grid.for_each_viewport_range(ranges, visitor))
    }

    pub fn line_bounds(&self) -> (i32, i32) {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grid
            .line_bounds()
    }

    pub fn with_line_cells<R>(&self, line: i32, f: impl FnOnce(&[Cell]) -> R) -> Option<R> {
        let engine = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        engine.grid.line(line).map(|line| line.with_dense(f))
    }

    pub fn for_each_line_cell(
        &self,
        line: i32,
        visitor: impl FnMut(usize, &Cell, Option<Combining<'_>>),
    ) -> bool {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grid
            .for_each_line_cell(line, visitor)
    }

    /// Visit a requested inclusive line range from one coherent engine state.
    ///
    /// The returned bounds and column count are captured by the same lock as all
    /// cell visits. The callback must not call back into this terminal.
    pub fn for_each_line_cell_range(
        &self,
        requested_first: i32,
        requested_last: i32,
        mut visitor: impl FnMut(TerminalLineRange, i32, usize, &Cell, Option<Combining<'_>>),
    ) -> TerminalLineRange {
        let engine = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (first_line, last_line) = engine.grid.line_bounds();
        let range = TerminalLineRange {
            first_line,
            last_line,
            columns: usize::from(engine.size.cols),
        };
        engine.grid.for_each_line_cell_range(
            requested_first,
            requested_last,
            |line, col, cell, combining| visitor(range, line, col, cell, combining),
        );
        range
    }
}

include!("runtime_support.rs");

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;
