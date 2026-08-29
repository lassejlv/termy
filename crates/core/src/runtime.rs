use crate::backend;
use crate::frame::{
    TerminalRenderDamageSnapshot, TerminalRenderRead, TerminalViewportMetadata, TermyFrame,
    TermyFrameUpdate, snapshot_from_term, snapshot_update_from_term,
};
use crate::keyboard::TerminalKeyboardMode;
use crate::kitty_graphics::{
    KittyGraphicsInterceptor, KittyGraphicsItem, KittyGraphicsRenderPlacement, KittyGraphicsScreen,
    KittyGraphicsState,
};
#[cfg(unix)]
use crate::locale::{Utf8LocaleOverridePlan, preferred_utf8_locale, utf8_locale_override_plan};
use crate::mouse_protocol::TerminalMouseMode;
use crate::osc_intercept::{OscEvent, OscInterceptor};
use crate::path_env::normalized_path_env;
use crate::protocol::{
    KittyClipboardControl, KittyClipboardHostState, KittyClipboardInput, KittyClipboardInterceptor,
    KittyClipboardOsc, KittyClipboardOscTerminator, TerminalClipboardLocation, TerminalQueryColors,
    TerminalReplyHost, reply_bytes_for_event,
};
use crate::render_metrics::increment_runtime_wakeup_count;
use crate::search::{
    TermySearchMatch, TermySearchOptions, TermySharedSearchMatch, search_lines_shared,
};
use crate::shell_integration::ProgressState;
use alacritty_terminal::{
    event::{Event as AlacEvent, EventListener, OnResize, WindowSize},
    grid::Dimensions,
    index::{Column, Line},
    sync::FairMutex,
    term::{Term, TermMode, cell::Flags},
    thread,
    tty::{self, EventedPty, EventedReadWrite, Options as PtyOptions, Shell},
    vte::ansi::{self, Handler, NamedPrivateMode, PrivateMode},
};
use flume::{Receiver, Sender, unbounded};
use polling::{Event as PollingEvent, Events, PollMode, Poller};
#[cfg(not(target_os = "windows"))]
use std::path::Path;
use std::{
    borrow::Cow,
    collections::HashMap,
    env,
    io::{self, ErrorKind, Read, Write},
    num::NonZeroUsize,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver as StdReceiver, Sender as StdSender, TryRecvError},
    },
    time::Instant,
};
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone)]
pub struct TabTitleShellIntegration {
    pub enabled: bool,
    pub explicit_prefix: String,
}

const DEFAULT_TERM: &str = "xterm-256color";
const DEFAULT_COLORTERM: &str = "truecolor";
const TERMY_TERM_PROGRAM: &str = "termy";
const GHOSTTY_COMPAT_TERM_PROGRAM: &str = "ghostty";
const GHOSTTY_COMPAT_TERM_PROGRAM_VERSION: &str = "1.2.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkingDirFallback {
    Home,
    Process,
}

#[allow(clippy::derivable_impls)]
impl Default for WorkingDirFallback {
    fn default() -> Self {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            Self::Home
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            Self::Process
        }
    }
}

const DEFAULT_SCROLLBACK_HISTORY: usize = 1000;

/// Upper clamp on scrollback lines, enforced at the point the value is applied
/// to the live grid. The config-file parser (`config_core`) already bounds this
/// at parse time, but the runtime/FFI setters (`with_scrollback_history`,
/// `set_scrollback_history`) and directly-constructed `TerminalRuntimeConfig`s
/// bypass that parser, so the core must self-defend: each pane eagerly grows its
/// scrollback toward this cap, so an unbounded value plus hostile output is an
/// unbounded memory leak. Kept in parity with `config_core`'s constant of the
/// same name.
pub const MAX_TERMINAL_SCROLLBACK_HISTORY: usize = 20_000;

/// Upper clamp on terminal dimensions. Real displays never approach this (an 8K
/// display at a 4px font is ~1900 columns); it exists only to stop a buggy or
/// hostile embedder from requesting a multi-gigabyte grid — `u16::MAX` on both
/// axes is ~4.3 billion cells. Clamping each axis bounds the worst-case grid
/// (and the frame snapshot allocated from it) to `MAX_TERMINAL_COLS` ×
/// `MAX_TERMINAL_ROWS` cells.
const MAX_TERMINAL_COLS: u16 = 4096;
const MAX_TERMINAL_ROWS: u16 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowsShell {
    #[default]
    Cmd,
    PowerShell,
    PowerShellCore,
    GitBash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalCursorStyle {
    Line,
    Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalCursorState {
    pub col: usize,
    pub row: usize,
    pub style: TerminalCursorStyle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalOptions {
    pub scrollback_history: usize,
    pub default_cursor_style: TerminalCursorStyle,
}

impl Default for TerminalOptions {
    fn default() -> Self {
        Self {
            scrollback_history: DEFAULT_SCROLLBACK_HISTORY,
            default_cursor_style: TerminalCursorStyle::Block,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TerminalRuntimeConfig {
    pub shell: Option<String>,
    pub windows_shell: WindowsShell,
    pub term: String,
    pub colorterm: Option<String>,
    pub environment: HashMap<String, String>,
    pub query_colors: TerminalQueryColors,
    pub working_dir_fallback: WorkingDirFallback,
    pub scrollback_history: usize,
    pub default_cursor_style: TerminalCursorStyle,
}

/// Selects what owns a newly-created PTY.
///
/// `ShellCommand` preserves the existing shell-evaluated startup-command API.
/// Structured tools such as OpenSSH must use `Program`, which sends each
/// argument directly to the child without routing through a shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalLaunch {
    ShellCommand(String),
    Program { program: String, args: Vec<String> },
}

impl Default for TerminalRuntimeConfig {
    fn default() -> Self {
        Self {
            shell: None,
            windows_shell: WindowsShell::default(),
            term: DEFAULT_TERM.to_string(),
            colorterm: Some(DEFAULT_COLORTERM.to_string()),
            environment: HashMap::new(),
            query_colors: TerminalQueryColors::default(),
            working_dir_fallback: WorkingDirFallback::default(),
            scrollback_history: DEFAULT_SCROLLBACK_HISTORY,
            default_cursor_style: TerminalCursorStyle::Block,
        }
    }
}

impl TerminalRuntimeConfig {
    pub fn resolved_shell_program(&self) -> String {
        default_shell_launch(self).program
    }
}

impl TerminalOptions {
    pub fn with_scrollback_history(self, scrollback_history: usize) -> Self {
        Self {
            scrollback_history,
            ..self
        }
    }
}

impl TerminalRuntimeConfig {
    pub fn term_options(&self) -> TerminalOptions {
        TerminalOptions {
            scrollback_history: self.scrollback_history,
            default_cursor_style: self.default_cursor_style,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct KittyGraphicsScrollRegion {
    top: usize,
    bottom: Option<usize>,
}

impl Default for KittyGraphicsScrollRegion {
    fn default() -> Self {
        Self {
            top: 1,
            bottom: None,
        }
    }
}

impl KittyGraphicsScrollRegion {
    fn bounds(self, screen_lines: usize) -> (usize, usize) {
        let top = self.top.saturating_sub(1).min(screen_lines);
        let bottom = self.bottom.unwrap_or(screen_lines).min(screen_lines);
        (top, bottom)
    }

    fn covers_full_screen(self, screen_lines: usize) -> bool {
        self.bounds(screen_lines) == (0, screen_lines)
    }

    fn set(&mut self, top: usize, bottom: Option<usize>, screen_lines: usize) {
        // Match Alacritty: resolve an omitted bottom before validating, then
        // clamp the accepted region to the screen during use.
        if top >= bottom.unwrap_or(screen_lines) {
            return;
        }
        self.top = top;
        self.bottom = bottom;
    }

    fn reset(&mut self) {
        self.top = 1;
        self.bottom = None;
    }
}

/// Tracks DECSTBM alongside the real terminal parser so Kitty commands can
/// choose a scroll-safe cursor policy without reaching into Alacritty internals.
#[derive(Default)]
pub struct KittyGraphicsCursorTracker {
    region: KittyGraphicsScrollRegion,
}

impl KittyGraphicsCursorTracker {
    pub fn region_covers_full_screen(&self, screen_lines: usize) -> bool {
        self.region.covers_full_screen(screen_lines)
    }

    pub fn reset_scroll_region(&mut self) {
        self.region.reset();
    }

    pub fn set_scroll_region(&mut self, top: usize, bottom: Option<usize>, screen_lines: usize) {
        self.region.set(top, bottom, screen_lines);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KittyGraphicsTextEffect {
    EnteredAlternateScreen,
    TerminalReset,
    PreservePrimaryAcrossPartialHistoryGrowth(usize),
    ScrollUpWithoutHistory {
        screen: KittyGraphicsScreen,
        lines: usize,
    },
    ClearViewport {
        screen: KittyGraphicsScreen,
        history_size: usize,
        rows: usize,
        cols: usize,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KittyGraphicsTextEffects {
    effects: Vec<KittyGraphicsTextEffect>,
}

impl KittyGraphicsTextEffects {
    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }

    pub fn apply_to(self, graphics: &mut KittyGraphicsState) -> bool {
        let mut changed = false;
        for effect in self.effects {
            changed |= match effect {
                KittyGraphicsTextEffect::EnteredAlternateScreen => {
                    graphics.clear_visible_on_screen(KittyGraphicsScreen::Alternate)
                }
                KittyGraphicsTextEffect::TerminalReset => graphics.reset(),
                KittyGraphicsTextEffect::PreservePrimaryAcrossPartialHistoryGrowth(lines) => {
                    graphics.preserve_primary_placements_across_partial_history_growth(lines)
                }
                KittyGraphicsTextEffect::ScrollUpWithoutHistory { screen, lines } => {
                    graphics.scroll_up_without_history_on_screen(lines, screen)
                }
                KittyGraphicsTextEffect::ClearViewport {
                    screen,
                    history_size,
                    rows,
                    cols,
                } => graphics.clear_viewport_on_screen(screen, history_size, rows, cols),
            };
        }
        changed
    }

    fn push(&mut self, effect: KittyGraphicsTextEffect) {
        self.effects.push(effect);
    }
}

#[derive(Clone, Copy, Debug)]
struct KittyGraphicsScrollObservation {
    screen: KittyGraphicsScreen,
    full_screen_region: bool,
    physical_lines: usize,
    history_before: usize,
}

struct KittyGraphicsTrackingHandler<'a, T> {
    term: &'a mut Term<T>,
    tracker: &'a mut KittyGraphicsCursorTracker,
    effects: &'a mut KittyGraphicsTextEffects,
    resize_anchor_state: Option<&'a crate::resize_anchor::ResizeAnchorState>,
    track_scrolls: bool,
}

impl<T: EventListener> KittyGraphicsTrackingHandler<'_, T> {
    fn linefeed_scroll_lines(&self) -> usize {
        let screen_lines = self.term.grid().screen_lines();
        let (_, bottom) = self.tracker.region.bounds(screen_lines);
        let cursor_line = self.term.grid().cursor.point.line.0.max(0) as usize;
        usize::from(screen_lines > 0 && cursor_line.saturating_add(1) == bottom)
    }

    fn input_scroll_lines(&self, c: char) -> usize {
        let Some(width) = c.width() else {
            return 0;
        };
        if width == 0 || !self.term.mode().contains(TermMode::LINE_WRAP) {
            return 0;
        }

        let cursor = &self.term.grid().cursor;
        let needs_wrap = cursor.input_needs_wrap
            || (width == 2
                && cursor.point.column.0.saturating_add(1) >= self.term.grid().columns());
        if needs_wrap {
            self.linefeed_scroll_lines()
        } else {
            0
        }
    }

    fn explicit_scroll_up_lines(&self, lines: usize) -> usize {
        let screen_lines = self.term.grid().screen_lines();
        let (top, bottom) = self.tracker.region.bounds(screen_lines);
        lines.min(bottom.saturating_sub(top))
    }

    fn observe_scroll(&self, physical_lines: usize) -> Option<KittyGraphicsScrollObservation> {
        if !self.track_scrolls || physical_lines == 0 {
            return None;
        }
        let screen_lines = self.term.grid().screen_lines();
        Some(KittyGraphicsScrollObservation {
            screen: KittyGraphicsScreen::from_alternate_screen(
                self.term.mode().contains(TermMode::ALT_SCREEN),
            ),
            full_screen_region: self.tracker.region.covers_full_screen(screen_lines),
            physical_lines,
            history_before: self.term.grid().history_size(),
        })
    }

    fn finish_scroll(&mut self, observation: Option<KittyGraphicsScrollObservation>) {
        let Some(observation) = observation else {
            return;
        };
        let history_growth = self
            .term
            .grid()
            .history_size()
            .saturating_sub(observation.history_before);
        match (observation.screen, observation.full_screen_region) {
            (KittyGraphicsScreen::Primary, true) => {
                let lines = observation.physical_lines.saturating_sub(history_growth);
                if lines > 0 {
                    self.effects
                        .push(KittyGraphicsTextEffect::ScrollUpWithoutHistory {
                            screen: KittyGraphicsScreen::Primary,
                            lines,
                        });
                }
            }
            (KittyGraphicsScreen::Primary, false) => {
                if history_growth > 0 {
                    self.effects.push(
                        KittyGraphicsTextEffect::PreservePrimaryAcrossPartialHistoryGrowth(
                            history_growth,
                        ),
                    );
                }
            }
            (KittyGraphicsScreen::Alternate, true) => {
                self.effects
                    .push(KittyGraphicsTextEffect::ScrollUpWithoutHistory {
                        screen: KittyGraphicsScreen::Alternate,
                        lines: observation.physical_lines,
                    });
            }
            (KittyGraphicsScreen::Alternate, false) => (),
        }
    }

    fn observe_live_output(&self) {
        if let Some(state) = self.resize_anchor_state {
            state.observe_live_output(self.term);
        }
    }
}

macro_rules! forward_kitty_graphics_handler_methods {
    ($(fn $name:ident($($arg:ident: $ty:ty),*);)*) => {
        $(
            fn $name(&mut self $(, $arg: $ty)*) {
                Handler::$name(&mut *self.term $(, $arg)*);
            }
        )*
    };
}

impl<T: EventListener> Handler for KittyGraphicsTrackingHandler<'_, T> {
    fn input(&mut self, c: char) {
        let physical_lines = if self.track_scrolls {
            self.input_scroll_lines(c)
        } else {
            0
        };
        let observation = self.observe_scroll(physical_lines);
        Handler::input(&mut *self.term, c);
        self.finish_scroll(observation);
        self.observe_live_output();
    }

    fn put_tab(&mut self, count: u16) {
        let physical_lines = if self.track_scrolls
            && self.term.grid().cursor.input_needs_wrap
            && self.term.mode().contains(TermMode::LINE_WRAP)
        {
            self.linefeed_scroll_lines()
        } else {
            0
        };
        let observation = self.observe_scroll(physical_lines);
        Handler::put_tab(&mut *self.term, count);
        self.finish_scroll(observation);
        self.observe_live_output();
    }

    fn linefeed(&mut self) {
        let physical_lines = if self.track_scrolls {
            self.linefeed_scroll_lines()
        } else {
            0
        };
        let observation = self.observe_scroll(physical_lines);
        Handler::linefeed(&mut *self.term);
        self.finish_scroll(observation);
        self.observe_live_output();
    }

    fn newline(&mut self) {
        let physical_lines = if self.track_scrolls {
            self.linefeed_scroll_lines()
        } else {
            0
        };
        let observation = self.observe_scroll(physical_lines);
        Handler::newline(&mut *self.term);
        self.finish_scroll(observation);
        self.observe_live_output();
    }

    fn scroll_up(&mut self, lines: usize) {
        let physical_lines = if self.track_scrolls {
            self.explicit_scroll_up_lines(lines)
        } else {
            0
        };
        let observation = self.observe_scroll(physical_lines);
        Handler::scroll_up(&mut *self.term, lines);
        self.finish_scroll(observation);
    }

    fn reset_state(&mut self) {
        self.tracker.reset_scroll_region();
        if let Some(state) = self.resize_anchor_state {
            state.reset();
        }
        self.effects.push(KittyGraphicsTextEffect::TerminalReset);
        Handler::reset_state(&mut *self.term);
    }

    fn set_private_mode(&mut self, mode: PrivateMode) {
        if mode == NamedPrivateMode::ColumnMode.into() {
            self.tracker.reset_scroll_region();
        }
        if mode == NamedPrivateMode::SwapScreenAndSetRestoreCursor.into()
            && !self.term.mode().contains(TermMode::ALT_SCREEN)
        {
            self.effects
                .push(KittyGraphicsTextEffect::EnteredAlternateScreen);
        }
        Handler::set_private_mode(&mut *self.term, mode);
    }

    fn unset_private_mode(&mut self, mode: PrivateMode) {
        if mode == NamedPrivateMode::ColumnMode.into() {
            self.tracker.reset_scroll_region();
        }
        Handler::unset_private_mode(&mut *self.term, mode);
    }

    fn set_scrolling_region(&mut self, top: usize, bottom: Option<usize>) {
        self.tracker
            .region
            .set(top, bottom, self.term.grid().screen_lines());
        Handler::set_scrolling_region(&mut *self.term, top, bottom);
    }

    fn clear_screen(&mut self, mode: ansi::ClearMode) {
        if let Some(state) = self.resize_anchor_state {
            state.clear_screen(self.term, &mode);
        }
        let clear_viewport = if self.track_scrolls && matches!(mode, ansi::ClearMode::All) {
            Some(KittyGraphicsTextEffect::ClearViewport {
                screen: KittyGraphicsScreen::from_alternate_screen(
                    self.term.mode().contains(TermMode::ALT_SCREEN),
                ),
                history_size: self.term.grid().history_size(),
                rows: self.term.grid().screen_lines(),
                cols: self.term.grid().columns(),
            })
        } else {
            None
        };

        Handler::clear_screen(&mut *self.term, mode);
        if let Some(clear_viewport) = clear_viewport {
            self.effects.push(clear_viewport);
        }
    }

    forward_kitty_graphics_handler_methods! {
        fn set_title(title: Option<String>);
        fn set_cursor_style(style: Option<ansi::CursorStyle>);
        fn set_cursor_shape(shape: ansi::CursorShape);
        fn goto(line: i32, col: usize);
        fn goto_line(line: i32);
        fn goto_col(col: usize);
        fn insert_blank(count: usize);
        fn move_up(rows: usize);
        fn move_down(rows: usize);
        fn identify_terminal(intermediate: Option<char>);
        fn device_status(status: usize);
        fn move_forward(cols: usize);
        fn move_backward(cols: usize);
        fn move_down_and_cr(rows: usize);
        fn move_up_and_cr(rows: usize);
        fn backspace();
        fn carriage_return();
        fn bell();
        fn substitute();
        fn set_horizontal_tabstop();
        fn scroll_down(rows: usize);
        fn insert_blank_lines(lines: usize);
        fn delete_lines(lines: usize);
        fn erase_chars(count: usize);
        fn delete_chars(count: usize);
        fn move_backward_tabs(count: u16);
        fn move_forward_tabs(count: u16);
        fn save_cursor_position();
        fn restore_cursor_position();
        fn clear_line(mode: ansi::LineClearMode);
        fn clear_tabs(mode: ansi::TabulationClearMode);
        fn set_tabs(interval: u16);
        fn reverse_index();
        fn terminal_attribute(attr: ansi::Attr);
        fn set_mode(mode: ansi::Mode);
        fn unset_mode(mode: ansi::Mode);
        fn report_mode(mode: ansi::Mode);
        fn report_private_mode(mode: ansi::PrivateMode);
        fn set_keypad_application_mode();
        fn unset_keypad_application_mode();
        fn set_active_charset(index: ansi::CharsetIndex);
        fn configure_charset(index: ansi::CharsetIndex, charset: ansi::StandardCharset);
        fn set_color(index: usize, color: ansi::Rgb);
        fn dynamic_color_sequence(prefix: String, index: usize, terminator: &str);
        fn reset_color(index: usize);
        fn clipboard_store(clipboard: u8, data: &[u8]);
        fn clipboard_load(clipboard: u8, terminator: &str);
        fn decaln();
        fn push_title();
        fn pop_title();
        fn text_area_size_pixels();
        fn text_area_size_chars();
        fn set_hyperlink(hyperlink: Option<ansi::Hyperlink>);
        fn set_mouse_cursor_icon(icon: ansi::cursor_icon::CursorIcon);
        fn report_keyboard_mode();
        fn push_keyboard_mode(mode: ansi::KeyboardModes);
        fn pop_keyboard_modes(to_pop: u16);
        fn set_keyboard_mode(mode: ansi::KeyboardModes, behavior: ansi::KeyboardModesApplyBehavior);
        fn set_modify_other_keys(mode: ansi::ModifyOtherKeys);
        fn report_modify_other_keys();
        fn set_scp(char_path: ansi::ScpCharPath, update_mode: ansi::ScpUpdateMode);
    }
}

fn advance_terminal_text<T: EventListener>(
    tracker: &mut KittyGraphicsCursorTracker,
    parser: &mut ansi::Processor,
    term: &mut Term<T>,
    bytes: &[u8],
    track_scrolls: bool,
    resize_anchor_state: Option<&crate::resize_anchor::ResizeAnchorState>,
) -> KittyGraphicsTextEffects {
    let mut effects = KittyGraphicsTextEffects::default();
    {
        let mut handler = KittyGraphicsTrackingHandler {
            term,
            tracker,
            effects: &mut effects,
            resize_anchor_state,
            track_scrolls,
        };
        parser.advance(&mut handler, bytes);
    }
    effects
}

/// Apply Kitty's default post-placement cursor movement.
///
/// Moving down through the terminal handler keeps the placement attached to
/// scrollback when it reaches the bottom row. A regular CSI cursor-down would
/// clamp at the bottom instead, leaving most of a tall image off-screen.
///
/// Returns the number of scrolled lines that were not represented by history
/// growth, so graphics anchors can follow alternate-screen, zero-history, and
/// full-history row rotation.
/// On Windows, `CreateProcessW` splits `lpCommandLine` on spaces to find the
/// executable name when `lpApplicationName` is `NULL`.  A shell path that contains
/// spaces (e.g. `C:\Program Files\PowerShell\7\pwsh.exe`) must therefore be
/// wrapped in double-quotes so the entire path is treated as a single token.
///
/// This function is a no-op on non-Windows platforms.
#[cfg(target_os = "windows")]
fn quote_shell_program_if_needed(shell_path: &str) -> String {
    // Already fully quoted (starts and ends with a double-quote): leave unchanged.
    if shell_path.starts_with('"') && shell_path.ends_with('"') && shell_path.len() >= 2 {
        return shell_path.to_string();
    }
    // No quoting required when the path contains no spaces.
    if !shell_path.contains(' ') {
        return shell_path.to_string();
    }
    // Escape any embedded double-quotes inside the path, then wrap in outer quotes.
    // (Windows file names cannot legally contain '"', but we handle it defensively.)
    let escaped = shell_path.replace('"', "\\\"");
    format!("\"{}\"", escaped)
}

fn login_shell_args(shell_path: &str) -> Vec<String> {
    #[cfg(target_os = "windows")]
    {
        let _ = shell_path;
        Vec::new()
    }

    // On macOS, terminals conventionally launch login shells so that the user's
    // PATH and environment (set up in ~/.bash_profile, ~/.zprofile, etc.) are
    // available.  Pass both -i (interactive) and -l (login).
    #[cfg(target_os = "macos")]
    match Path::new(shell_path)
        .file_name()
        .and_then(|name| name.to_str())
    {
        Some("bash" | "zsh" | "fish") => vec!["-i".to_string(), "-l".to_string()],
        _ => Vec::new(),
    }

    // On Linux (and other non-macOS Unix), the user is already in a login
    // session, so sourcing all login scripts on every terminal open adds
    // unnecessary startup latency.  Launch an interactive non-login shell
    // instead, which is the convention used by alacritty and other Linux
    // terminal emulators.
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    match Path::new(shell_path)
        .file_name()
        .and_then(|name| name.to_str())
    {
        Some("bash" | "zsh" | "fish") => vec!["-i".to_string()],
        _ => Vec::new(),
    }
}

/// The executable and argument vector selected for a terminal PTY.
///
/// All terminal engines must use [`resolve_terminal_launch`] instead of
/// independently interpreting [`TerminalRuntimeConfig`] or [`TerminalLaunch`].
/// This keeps platform shell selection, login-shell arguments, and startup
/// command handling identical across engines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTerminalLaunch {
    pub program: String,
    pub args: Vec<String>,
}

#[cfg(target_os = "windows")]
fn windows_cmd_path() -> String {
    if let Ok(comspec) = env::var("COMSPEC")
        && !comspec.trim().is_empty()
    {
        return comspec;
    }
    "C:\\Windows\\System32\\cmd.exe".to_string()
}

#[cfg(target_os = "windows")]
fn windows_git_bash_path() -> String {
    let mut candidates = Vec::new();
    if let Ok(program_files) = env::var("ProgramFiles")
        && !program_files.trim().is_empty()
    {
        candidates.push(PathBuf::from(program_files).join("Git\\bin\\bash.exe"));
    }
    if let Ok(program_files_x86) = env::var("ProgramFiles(x86)")
        && !program_files_x86.trim().is_empty()
    {
        candidates.push(PathBuf::from(program_files_x86).join("Git\\bin\\bash.exe"));
    }
    if let Ok(local_app_data) = env::var("LOCALAPPDATA")
        && !local_app_data.trim().is_empty()
    {
        candidates.push(PathBuf::from(local_app_data).join("Programs\\Git\\bin\\bash.exe"));
    }

    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .map_or_else(|| "bash.exe".to_string(), |path| path.display().to_string())
}

#[cfg(any(not(target_os = "windows"), test))]
fn resolve_shell_path(configured_shell: Option<&str>) -> String {
    if let Some(shell) = configured_shell
        .map(str::trim)
        .filter(|shell| !shell.is_empty())
    {
        return shell.to_string();
    }

    if let Ok(shell) = env::var("SHELL")
        && !shell.trim().is_empty()
    {
        return shell;
    }

    #[cfg(target_os = "windows")]
    {
        windows_cmd_path()
    }

    #[cfg(target_os = "macos")]
    {
        "/bin/zsh".to_string()
    }

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        "/bin/bash".to_string()
    }
}

#[cfg(target_os = "windows")]
fn windows_shell_launch(windows_shell: WindowsShell) -> ResolvedTerminalLaunch {
    match windows_shell {
        WindowsShell::Cmd => ResolvedTerminalLaunch {
            program: windows_cmd_path(),
            args: Vec::new(),
        },
        WindowsShell::PowerShell => ResolvedTerminalLaunch {
            program: "powershell.exe".to_string(),
            args: vec!["-NoLogo".to_string()],
        },
        WindowsShell::PowerShellCore => ResolvedTerminalLaunch {
            program: "pwsh.exe".to_string(),
            args: vec!["-NoLogo".to_string()],
        },
        WindowsShell::GitBash => ResolvedTerminalLaunch {
            program: windows_git_bash_path(),
            args: vec!["--login".to_string(), "-i".to_string()],
        },
    }
}

#[cfg(target_os = "windows")]
fn windows_startup_command_shell(
    windows_shell: WindowsShell,
    command: &str,
) -> ResolvedTerminalLaunch {
    match windows_shell {
        WindowsShell::Cmd => ResolvedTerminalLaunch {
            program: windows_cmd_path(),
            args: vec!["/C".to_string(), command.to_string()],
        },
        WindowsShell::PowerShell => ResolvedTerminalLaunch {
            program: "powershell.exe".to_string(),
            args: vec![
                "-NoLogo".to_string(),
                "-NoProfile".to_string(),
                "-ExecutionPolicy".to_string(),
                "Bypass".to_string(),
                "-Command".to_string(),
                command.to_string(),
            ],
        },
        WindowsShell::PowerShellCore => ResolvedTerminalLaunch {
            program: "pwsh.exe".to_string(),
            args: vec![
                "-NoLogo".to_string(),
                "-NoProfile".to_string(),
                "-ExecutionPolicy".to_string(),
                "Bypass".to_string(),
                "-Command".to_string(),
                command.to_string(),
            ],
        },
        WindowsShell::GitBash => ResolvedTerminalLaunch {
            program: windows_git_bash_path(),
            args: vec!["-lc".to_string(), command.to_string()],
        },
    }
}

fn configured_shell_launch(configured_shell: Option<&str>) -> Option<ResolvedTerminalLaunch> {
    let shell_path = configured_shell
        .map(str::trim)
        .filter(|shell| !shell.is_empty())?;
    Some(ResolvedTerminalLaunch {
        program: shell_path.to_string(),
        args: login_shell_args(shell_path),
    })
}

fn default_shell_launch(runtime_config: &TerminalRuntimeConfig) -> ResolvedTerminalLaunch {
    if let Some(launch) = configured_shell_launch(runtime_config.shell.as_deref()) {
        return launch;
    }

    #[cfg(target_os = "windows")]
    {
        windows_shell_launch(runtime_config.windows_shell)
    }

    #[cfg(not(target_os = "windows"))]
    {
        let shell_path = resolve_shell_path(None);
        ResolvedTerminalLaunch {
            program: shell_path.clone(),
            args: login_shell_args(&shell_path),
        }
    }
}

fn launch_to_shell(launch: ResolvedTerminalLaunch) -> Shell {
    #[cfg(target_os = "windows")]
    let program = quote_shell_program_if_needed(&launch.program);
    #[cfg(not(target_os = "windows"))]
    let program = launch.program;

    Shell::new(program, launch.args)
}

pub fn resolve_terminal_launch(
    runtime_config: &TerminalRuntimeConfig,
    launch: Option<&TerminalLaunch>,
) -> anyhow::Result<ResolvedTerminalLaunch> {
    if let Some(TerminalLaunch::Program { program, args }) = launch {
        anyhow::ensure!(
            !program.trim().is_empty(),
            "terminal program cannot be empty"
        );
        anyhow::ensure!(
            !program.contains('\0') && !args.iter().any(|arg| arg.contains('\0')),
            "terminal program and arguments cannot contain NUL bytes"
        );
        return Ok(ResolvedTerminalLaunch {
            program: program.clone(),
            args: args.clone(),
        });
    }

    if let Some(command) = launch.and_then(|launch| match launch {
        TerminalLaunch::ShellCommand(command) => {
            Some(command.trim()).filter(|command| !command.is_empty())
        }
        TerminalLaunch::Program { .. } => None,
    }) {
        #[cfg(unix)]
        {
            return Ok(ResolvedTerminalLaunch {
                program: "/bin/sh".to_string(),
                args: vec!["-c".to_string(), command.to_string()],
            });
        }

        #[cfg(target_os = "windows")]
        {
            if runtime_config
                .shell
                .as_deref()
                .map(str::trim)
                .is_some_and(|shell| !shell.is_empty())
            {
                return Ok(ResolvedTerminalLaunch {
                    program: "cmd.exe".to_string(),
                    args: vec!["/C".to_string(), command.to_string()],
                });
            }

            return Ok(windows_startup_command_shell(
                runtime_config.windows_shell,
                command,
            ));
        }
    }

    Ok(default_shell_launch(runtime_config))
}

fn user_home_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        if let Ok(user_profile) = env::var("USERPROFILE")
            && !user_profile.trim().is_empty()
        {
            return Some(PathBuf::from(user_profile));
        }

        if let (Ok(home_drive), Ok(home_path)) = (env::var("HOMEDRIVE"), env::var("HOMEPATH"))
            && !home_drive.trim().is_empty()
            && !home_path.trim().is_empty()
        {
            return Some(PathBuf::from(format!("{home_drive}{home_path}")));
        }
    }

    if let Ok(home) = env::var("HOME")
        && !home.trim().is_empty()
    {
        return Some(PathBuf::from(home));
    }

    None
}

/// Build the child-process environment shared by native terminal engines.
///
/// Keeping this at the engine boundary prevents experimental backends from
/// drifting on terminal identity, PATH normalization, shell integration, or
/// Unix UTF-8 locale repair.
pub fn terminal_environment_overrides(
    shell_integration: Option<&TabTitleShellIntegration>,
    runtime_config: &TerminalRuntimeConfig,
) -> HashMap<String, String> {
    let mut env_overrides = HashMap::new();

    if let Some(path) = normalized_path_env(
        env::var_os("PATH")
            .or_else(|| env::var_os("Path"))
            .as_deref(),
    ) {
        env_overrides.insert("PATH".to_string(), path);
    }

    let term = runtime_config.term.trim();
    let term = if term.is_empty() { DEFAULT_TERM } else { term };
    env_overrides.insert("TERM".to_string(), term.to_string());

    if let Some(colorterm) = runtime_config
        .colorterm
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        env_overrides.insert("COLORTERM".to_string(), colorterm.to_string());
    }

    // Claude Code and similar CLIs gate terminal progress escape sequences on
    // known terminal identities. Termy supports Ghostty's OSC progress
    // protocol, so advertise that compatibility to child processes while
    // keeping TERM conservative for terminfo.
    env_overrides.insert(
        "TERM_PROGRAM".to_string(),
        GHOSTTY_COMPAT_TERM_PROGRAM.to_string(),
    );
    env_overrides.insert(
        "TERM_PROGRAM_VERSION".to_string(),
        GHOSTTY_COMPAT_TERM_PROGRAM_VERSION.to_string(),
    );
    env_overrides.insert(
        "TERMY_TERM_PROGRAM".to_string(),
        TERMY_TERM_PROGRAM.to_string(),
    );

    for (key, value) in &runtime_config.environment {
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        env_overrides.insert(key.to_string(), value.clone());
    }

    // Locale overrides are intentionally Unix-only. POSIX shells use libc locale
    // (`LC_*`/`LANG`) for wcwidth/prompt width, while native Windows shells
    // (`cmd.exe`/PowerShell) do not use this locale contract.
    #[cfg(unix)]
    {
        apply_utf8_locale_overrides(&mut env_overrides);
    }

    let shell_integration_enabled = shell_integration.is_some_and(|cfg| cfg.enabled);
    env_overrides.insert(
        "TERMY_SHELL_INTEGRATION".to_string(),
        if shell_integration_enabled { "1" } else { "0" }.to_string(),
    );

    if shell_integration_enabled {
        let prefix = shell_integration
            .and_then(|cfg| {
                let trimmed = cfg.explicit_prefix.trim();
                (!trimmed.is_empty()).then_some(trimmed)
            })
            .unwrap_or("termy:tab:");
        env_overrides.insert("TERMY_TAB_TITLE_PREFIX".to_string(), prefix.to_string());
    }

    env_overrides
}

#[cfg(unix)]
fn apply_utf8_locale_overrides(env_overrides: &mut HashMap<String, String>) {
    let lc_all = env::var("LC_ALL").ok();
    let lc_ctype = env::var("LC_CTYPE").ok();
    let lang = env::var("LANG").ok();
    let target_utf8_locale =
        preferred_utf8_locale(lc_all.as_deref(), lc_ctype.as_deref(), lang.as_deref());

    // zsh prompt width calculations rely on libc wcwidth + locale. If the shell
    // starts in C/POSIX/non-UTF-8 locale, multibyte prompt glyphs (e.g. U+276F)
    // can be counted by byte-length, drifting completion rendering.
    match utf8_locale_override_plan(lc_all.as_deref(), lc_ctype.as_deref(), lang.as_deref()) {
        Utf8LocaleOverridePlan::None => {}
        Utf8LocaleOverridePlan::LcCtypeOnly => {
            env_overrides.insert("LC_CTYPE".to_string(), target_utf8_locale);
        }
        Utf8LocaleOverridePlan::LcAllAndLcCtype => {
            env_overrides.insert("LC_ALL".to_string(), target_utf8_locale.clone());
            env_overrides.insert("LC_CTYPE".to_string(), target_utf8_locale);
        }
    }
}

pub fn resolve_working_directory_path(configured: Option<&str>) -> Option<std::path::PathBuf> {
    let configured = configured?.trim();
    if configured.is_empty() {
        return None;
    }

    let path = if configured == "~" {
        user_home_dir()?
    } else if let Some(relative) = configured
        .strip_prefix("~/")
        .or_else(|| configured.strip_prefix("~\\"))
    {
        user_home_dir()?.join(relative)
    } else {
        PathBuf::from(configured)
    };

    if path.is_dir() { Some(path) } else { None }
}

pub fn resolve_launch_working_directory(
    configured: Option<&str>,
    fallback: WorkingDirFallback,
) -> Option<PathBuf> {
    resolve_working_directory_path(configured)
        .or_else(|| default_working_directory_with_fallback(fallback))
}

pub fn normalize_working_directory_candidate(candidate: Option<&str>) -> Option<String> {
    let candidate = candidate?.trim();
    if candidate.is_empty() || candidate.bytes().any(|byte| byte.is_ascii_control()) {
        return None;
    }

    Some(resolve_working_directory_path(Some(candidate)).map_or_else(
        || candidate.to_string(),
        |path| path.to_string_lossy().into_owned(),
    ))
}

fn default_working_directory_with_fallback(fallback: WorkingDirFallback) -> Option<PathBuf> {
    if fallback == WorkingDirFallback::Home
        && let Some(home) = user_home_dir()
        && home.is_dir()
    {
        return Some(home);
    }

    env::current_dir().ok()
}

#[cfg(target_os = "windows")]
fn pty_child_pid(pty: &tty::Pty) -> Option<u32> {
    pty.child_watcher().pid().map(|pid| pid.get())
}

#[cfg(not(any(unix, target_os = "windows")))]
fn pty_child_pid(_pty: &tty::Pty) -> Option<u32> {
    None
}

/// Events sent from the terminal to the view
#[derive(Debug, Clone)]
pub enum TerminalEvent {
    /// Terminal content has changed, needs redraw
    Wakeup,
    /// Terminal title changed
    #[allow(dead_code)]
    Title(String),
    /// Terminal title reset
    ResetTitle,
    /// Bell character received
    Bell,
    /// Terminal exited
    Exit,
    /// OSC 52 clipboard store request
    ClipboardStore(String),

    // Shell integration events (OSC 133)
    /// OSC 133;A - Shell prompt start
    ShellPromptStart,
    /// OSC 133;B - Command input start
    ShellCommandStart,
    /// OSC 133;C - Command executing
    ShellCommandExecuting,
    /// OSC 133;D - Command finished with optional exit code
    ShellCommandFinished(Option<i32>),

    // Progress indicator (OSC 9;4)
    /// Progress state change from OSC 9;4
    Progress(ProgressState),

    // Working directory (OSC 7)
    /// Working directory changed
    WorkingDirectory(String),
}

/// Host-provided callback used to schedule terminal event draining.
///
/// The callback is intentionally payload-free: hosts that multiplex several
/// terminals can capture their own stable terminal identifier, while the FFI
/// host can keep using its existing one-terminal wake channel.
#[derive(Clone)]
pub struct TerminalWakeupNotifier {
    notify: Arc<dyn Fn() + Send + Sync>,
}

impl TerminalWakeupNotifier {
    pub fn new(notify: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            notify: Arc::new(notify),
        }
    }

    fn notify(&self) {
        (self.notify)();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalDirtySpan {
    pub row: usize,
    pub left_col: usize,
    pub right_col: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalDamageSnapshot {
    Full,
    Partial(Vec<TerminalDirtySpan>),
}

fn search_term_buffer<T: EventListener>(
    term: &Term<T>,
    query: &str,
    options: TermySearchOptions,
) -> Vec<TermySearchMatch> {
    search_term_buffer_shared(term, query, options)
        .into_iter()
        .map(Into::into)
        .collect()
}

fn search_term_buffer_shared<T: EventListener>(
    term: &Term<T>,
    query: &str,
    options: TermySearchOptions,
) -> Vec<TermySharedSearchMatch> {
    let grid = term.grid();
    let cols = grid.columns();
    let history_size = grid.history_size();
    let total_lines = grid.total_lines();
    if cols == 0 || total_lines == 0 {
        return Vec::new();
    }

    let lines = (0..total_lines).map(|absolute_row| {
        let line = Line(absolute_row as i32 - history_size as i32);
        (absolute_row, searchable_grid_line(term, line, cols))
    });
    search_lines_shared(lines, query, options)
}

fn searchable_grid_line<T: EventListener>(term: &Term<T>, line: Line, cols: usize) -> String {
    let grid = term.grid();
    let mut text = String::with_capacity(cols);
    for col in 0..cols {
        let cell = &grid[line][Column(col)];
        let render_text = !cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER | Flags::HIDDEN)
            && cell.c != '\0'
            && !cell.c.is_control();
        text.push(if render_text { cell.c } else { ' ' });
    }
    let trimmed_len = text.trim_end().len();
    text.truncate(trimmed_len);
    text
}

/// Event listener that forwards alacritty events to our channel
#[derive(Clone)]
pub struct JsonEventListener {
    events_tx: Sender<RuntimeEvent>,
    wakeup_notifier: Option<TerminalWakeupNotifier>,
    replay_suppressed: Arc<AtomicBool>,
    wakeup_queued: Arc<AtomicBool>,
    wakeup_signal_pending: Arc<AtomicBool>,
    wakeup_enabled: Arc<AtomicBool>,
}

impl JsonEventListener {
    #[cfg(test)]
    fn new(events_tx: Sender<RuntimeEvent>, wake_tx: Option<Sender<()>>) -> Self {
        let wakeup_notifier = wake_tx.map(|wake_tx| {
            TerminalWakeupNotifier::new(move || {
                let _ = wake_tx.try_send(());
            })
        });
        Self::new_with_wakeup_notifier(events_tx, wakeup_notifier)
    }

    fn new_with_wakeup_notifier(
        events_tx: Sender<RuntimeEvent>,
        wakeup_notifier: Option<TerminalWakeupNotifier>,
    ) -> Self {
        Self {
            events_tx,
            wakeup_notifier,
            replay_suppressed: Arc::new(AtomicBool::new(false)),
            wakeup_queued: Arc::new(AtomicBool::new(false)),
            wakeup_signal_pending: Arc::new(AtomicBool::new(false)),
            wakeup_enabled: Arc::new(AtomicBool::new(true)),
        }
    }

    fn set_replay_suppressed(&self, suppressed: bool) {
        self.replay_suppressed.store(suppressed, Ordering::Release);
    }

    fn set_wakeup_enabled(&self, enabled: bool) {
        let was_enabled = self.wakeup_enabled.swap(enabled, Ordering::AcqRel);
        if !enabled && was_enabled {
            // A wake signal issued just before this terminal became hidden may
            // be consumed without its queued event being drained. Remember that
            // edge so reactivation nudges the host once more.
            if self.wakeup_queued.load(Ordering::Acquire) {
                self.wakeup_signal_pending.store(true, Ordering::Release);
            }
        } else if enabled && !was_enabled {
            self.signal_pending_wakeup_if_enabled();
        }
    }

    fn reset_wakeup_queued(&self) -> bool {
        // Clear pending-signal state first. Any producer that observes the
        // subsequent queued=false transition will publish fresh pending state,
        // so this ordering cannot erase a newly queued wakeup.
        self.wakeup_signal_pending.store(false, Ordering::Release);
        self.wakeup_queued.swap(false, Ordering::AcqRel)
    }

    fn signal_pending_wakeup_if_enabled(&self) {
        if self.wakeup_enabled.load(Ordering::Acquire)
            && self.wakeup_signal_pending.swap(false, Ordering::AcqRel)
        {
            self.send_wake_signal();
        }
    }

    fn send_wake_signal(&self) {
        if let Some(wakeup_notifier) = &self.wakeup_notifier {
            wakeup_notifier.notify();
        }
    }

    fn send_terminal_event(&self, event: TerminalEvent) {
        if self.replay_suppressed.load(Ordering::Acquire) {
            return;
        }
        self.send_runtime_event(RuntimeEvent::Terminal(event));
        self.send_wake_signal();
    }

    fn send_kitty_clipboard_event(&self, event: KittyClipboardOsc) {
        if self.replay_suppressed.load(Ordering::Acquire) {
            return;
        }
        self.send_runtime_event(RuntimeEvent::KittyClipboard(event));
        self.send_wake_signal();
    }

    fn send_kitty_clipboard_control(&self, control: KittyClipboardControl) {
        if self.replay_suppressed.load(Ordering::Acquire) {
            return;
        }
        self.send_runtime_event(RuntimeEvent::KittyClipboardControl(control));
        self.send_wake_signal();
    }

    fn send_kitty_clipboard_input(&self, input: KittyClipboardInput) {
        match input {
            KittyClipboardInput::Packet(packet) => self.send_kitty_clipboard_event(packet),
            KittyClipboardInput::Control(control) => self.send_kitty_clipboard_control(control),
        }
    }

    /// Queues an event unless the host has stopped draining and the event is
    /// safe to shed. The channel is unbounded because the producer may hold
    /// the terminal lock while sending and the drain path takes the same lock,
    /// so blocking backpressure could deadlock; this soft cap is what keeps a
    /// non-draining host (hidden pane, stalled UI) from growing the queue
    /// without bound instead.
    fn send_runtime_event(&self, event: RuntimeEvent) {
        if should_drop_event(self.events_tx.len(), &event) {
            return;
        }
        let _ = self.events_tx.send(event);
    }
}

/// Soft cap on pending, undrained runtime events. Well above what a draining
/// host ever accumulates (the per-frame drain batch is 2048), so shedding only
/// starts once the host has clearly stopped consuming.
const EVENT_QUEUE_SOFT_CAP: usize = 8192;

/// Absolute ceiling on pending, undrained runtime events. Beyond the soft cap,
/// reply-bearing events (query responses the child may block on) are normally
/// never shed — but a host that has genuinely stopped draining (a hidden or
/// stalled pane) running a child that spams device/color queries would grow the
/// queue without bound, since those replies are exempt from the soft cap. This
/// hard cap sheds *everything* once the backlog is this deep, trading a lost
/// reply on an already-broken pane for a bounded memory footprint. A draining
/// pane never approaches it (the per-frame drain batch is 2048, ~32x below
/// this), so foreground correctness is unaffected.
const EVENT_QUEUE_HARD_CAP: usize = 65_536;

/// Decide whether a runtime event should be dropped rather than queued, given
/// the current queue depth. Split out as a pure function so the shedding policy
/// is unit-testable without a live event loop.
fn should_drop_event(queue_len: usize, event: &RuntimeEvent) -> bool {
    // Absolute ceiling: once the queue is this deep the host has clearly stopped
    // draining, so shed even non-droppable (reply-bearing) events to keep memory
    // bounded against a hostile child.
    if queue_len >= EVENT_QUEUE_HARD_CAP {
        return true;
    }
    // Soft cap: shed only latest-wins / cosmetic events; reply-bearing events
    // stay queued so a responsive-but-busy host never loses a child's reply.
    queue_len >= EVENT_QUEUE_SOFT_CAP && droppable_when_backlogged(event)
}

/// Whether an event can be shed once the queue is backlogged. State-refresh
/// events are safe: they either carry latest-wins state that the next
/// occurrence re-synchronizes (title, working directory, progress) or are
/// cosmetic (bell, cursor state, shell integration marks). Everything else —
/// exit, clipboard, and the query events whose drain-time replies the child
/// process may block on — is always queued.
fn droppable_when_backlogged(event: &RuntimeEvent) -> bool {
    match event {
        RuntimeEvent::Alacritty(event) => matches!(
            event,
            AlacEvent::Title(_)
                | AlacEvent::ResetTitle
                | AlacEvent::Bell
                | AlacEvent::CursorBlinkingChange
                | AlacEvent::MouseCursorDirty
        ),
        RuntimeEvent::Terminal(_) => true,
        RuntimeEvent::KittyClipboard(_) | RuntimeEvent::KittyClipboardControl(_) => false,
    }
}

impl EventListener for JsonEventListener {
    fn send_event(&self, event: AlacEvent) {
        if self.replay_suppressed.load(Ordering::Acquire) {
            return;
        }
        if matches!(event, AlacEvent::Wakeup) {
            increment_runtime_wakeup_count();
            if self.wakeup_queued.swap(true, Ordering::AcqRel) {
                return;
            }
            let _ = self.events_tx.send(RuntimeEvent::Alacritty(event));
            // Publish pending state after queueing, then let either this producer
            // or a concurrent hidden-to-visible transition claim the one signal.
            self.wakeup_signal_pending.store(true, Ordering::Release);
            self.signal_pending_wakeup_if_enabled();
            return;
        }
        self.send_runtime_event(RuntimeEvent::Alacritty(event));
        // This channel only nudges the UI thread to drain terminal events promptly.
        self.send_wake_signal();
    }
}

/// Sized to hold one `NATIVE_EVENT_LOOP_MAX_LOCKED_READ` parse budget: reads
/// accumulate here while the UI holds the terminal lock, and once a full
/// budget is buffered the reader blocks on the lock anyway (the kernel PTY
/// buffer absorbs the rest), so larger sizes only dirty more per-pane thread
/// stack for the lifetime of the session.
const NATIVE_EVENT_LOOP_READ_BUFFER_SIZE: usize = 0x1_0000;
const NATIVE_EVENT_LOOP_MAX_LOCKED_READ: usize = u16::MAX as usize;
const NATIVE_EVENT_LOOP_POLL_EVENT_CAPACITY: usize = 8;
#[cfg(not(target_os = "windows"))]
const NATIVE_EVENT_LOOP_READ_WRITE_TOKEN: usize = 0;
#[cfg(not(target_os = "windows"))]
const NATIVE_EVENT_LOOP_CHILD_EVENT_TOKEN: usize = 1;
#[cfg(target_os = "windows")]
const NATIVE_EVENT_LOOP_READ_WRITE_TOKEN: usize = 2;
#[cfg(target_os = "windows")]
const NATIVE_EVENT_LOOP_CHILD_EVENT_TOKEN: usize = 1;

#[derive(Debug, Clone)]
enum RuntimeEvent {
    Alacritty(AlacEvent),
    Terminal(TerminalEvent),
    KittyClipboard(KittyClipboardOsc),
    KittyClipboardControl(KittyClipboardControl),
}

#[derive(Debug)]
enum EventLoopMsg {
    Input(Cow<'static, [u8]>),
    Shutdown,
    Resize(WindowSize),
    NudgeResize(WindowSize),
}

#[derive(Clone)]
struct EventLoopSender {
    sender: StdSender<EventLoopMsg>,
    poller: Arc<Poller>,
}

impl EventLoopSender {
    fn send(&self, msg: EventLoopMsg) -> io::Result<()> {
        self.sender
            .send(msg)
            .map_err(|error| io::Error::new(ErrorKind::BrokenPipe, error.to_string()))?;
        self.poller.notify()
    }
}

struct Writing {
    source: Cow<'static, [u8]>,
    written: usize,
}

impl Writing {
    fn new(source: Cow<'static, [u8]>) -> Self {
        Self { source, written: 0 }
    }

    fn advance(&mut self, count: usize) {
        self.written += count;
    }

    fn remaining_bytes(&self) -> &[u8] {
        &self.source[self.written..]
    }

    fn finished(&self) -> bool {
        self.written >= self.source.len()
    }
}

struct PeekableReceiver<T> {
    rx: StdReceiver<T>,
    peeked: Option<T>,
}

impl<T> PeekableReceiver<T> {
    fn new(rx: StdReceiver<T>) -> Self {
        Self { rx, peeked: None }
    }

    fn peek(&mut self) -> Option<&T> {
        if self.peeked.is_none() {
            self.peeked = self.rx.try_recv().ok();
        }

        self.peeked.as_ref()
    }

    fn recv(&mut self) -> Option<T> {
        if self.peeked.is_some() {
            self.peeked.take()
        } else {
            match self.rx.try_recv() {
                Err(TryRecvError::Disconnected) => panic!("event loop channel closed"),
                res => res.ok(),
            }
        }
    }
}

#[derive(Default)]
struct NativeEventLoopState {
    write_list: std::collections::VecDeque<Cow<'static, [u8]>>,
    writing: Option<Writing>,
    parser: ansi::Processor,
    osc_interceptor: OscInterceptor,
    kitty_clipboard_interceptor: KittyClipboardInterceptor,
    kitty_graphics_interceptor: KittyGraphicsInterceptor,
    kitty_graphics_cursor_tracker: KittyGraphicsCursorTracker,
}

impl NativeEventLoopState {
    fn ensure_next(&mut self) {
        if self.writing.is_none() {
            self.goto_next();
        }
    }

    fn goto_next(&mut self) {
        self.writing = self.write_list.pop_front().map(Writing::new);
    }

    fn take_current(&mut self) -> Option<Writing> {
        self.writing.take()
    }

    fn needs_write(&self) -> bool {
        self.writing.is_some() || !self.write_list.is_empty()
    }

    fn set_current(&mut self, current: Option<Writing>) {
        self.writing = current;
    }
}

struct NativeRenderState {
    terminal: Arc<FairMutex<Term<JsonEventListener>>>,
    generation: Arc<AtomicU64>,
    palette_revision: Arc<AtomicU64>,
    palette_snapshot: Arc<FairMutex<crate::TerminalPalette>>,
}

impl NativeRenderState {
    fn record_mutation<T: EventListener>(&self, term: &Term<T>) {
        record_render_mutation(
            term,
            &self.generation,
            &self.palette_revision,
            &self.palette_snapshot,
        );
    }
}

fn refresh_palette_revision<T: EventListener>(
    term: &Term<T>,
    palette_revision: &AtomicU64,
    palette_snapshot: &FairMutex<crate::TerminalPalette>,
) {
    let mut snapshot = palette_snapshot.lock();
    if backend::palette_changed(term, &snapshot) {
        let revision = palette_revision
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        *snapshot = backend::palette(term, revision);
    }
}

fn record_render_mutation<T: EventListener>(
    term: &Term<T>,
    generation: &AtomicU64,
    palette_revision: &AtomicU64,
    palette_snapshot: &FairMutex<crate::TerminalPalette>,
) {
    generation.fetch_add(1, Ordering::Relaxed);
    refresh_palette_revision(term, palette_revision, palette_snapshot);
}

fn stop_synchronized_update<T: EventListener>(
    parser: &mut ansi::Processor,
    term: &mut Term<T>,
    generation: &AtomicU64,
    palette_revision: &AtomicU64,
    palette_snapshot: &FairMutex<crate::TerminalPalette>,
) {
    parser.stop_sync(term);
    record_render_mutation(term, generation, palette_revision, palette_snapshot);
}

fn capture_viewport_ranges_at_generation<T: EventListener>(
    term: &Term<T>,
    current_generation: &AtomicU64,
    requested_generation: u64,
    spans: &[TerminalDirtySpan],
) -> Option<Vec<backend::ViewportRangeCell>> {
    (current_generation.load(Ordering::Relaxed) == requested_generation)
        .then(|| backend::viewport_ranges(term, spans))
}

#[allow(clippy::too_many_arguments)]
fn move_graphics_cursor_and_record<T: EventListener>(
    term: &mut Term<T>,
    cols: u32,
    rows: u32,
    full_screen_scroll_region: bool,
    generation: &AtomicU64,
    palette_revision: &AtomicU64,
    palette_snapshot: &FairMutex<crate::TerminalPalette>,
) -> usize {
    let untracked_scroll =
        backend::move_graphics_cursor(term, cols, rows, full_screen_scroll_region);
    record_render_mutation(term, generation, palette_revision, palette_snapshot);
    untracked_scroll
}

struct NativeEventLoop {
    poll: Arc<Poller>,
    pty: tty::Pty,
    rx: PeekableReceiver<EventLoopMsg>,
    tx: StdSender<EventLoopMsg>,
    render_state: NativeRenderState,
    kitty_graphics: Arc<FairMutex<KittyGraphicsState>>,
    kitty_graphics_revision: Arc<AtomicU64>,
    resize_anchor_state: Arc<crate::resize_anchor::ResizeAnchorState>,
    terminal_size: Arc<FairMutex<TerminalSize>>,
    event_proxy: JsonEventListener,
    drain_on_exit: bool,
}

impl NativeEventLoop {
    fn new(
        render_state: NativeRenderState,
        kitty_graphics: Arc<FairMutex<KittyGraphicsState>>,
        kitty_graphics_revision: Arc<AtomicU64>,
        resize_anchor_state: Arc<crate::resize_anchor::ResizeAnchorState>,
        terminal_size: Arc<FairMutex<TerminalSize>>,
        event_proxy: JsonEventListener,
        pty: tty::Pty,
        drain_on_exit: bool,
    ) -> io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        Ok(Self {
            poll: Poller::new()?.into(),
            pty,
            rx: PeekableReceiver::new(rx),
            tx,
            render_state,
            kitty_graphics,
            kitty_graphics_revision,
            resize_anchor_state,
            terminal_size,
            event_proxy,
            drain_on_exit,
        })
    }

    fn channel(&self) -> EventLoopSender {
        EventLoopSender {
            sender: self.tx.clone(),
            poller: self.poll.clone(),
        }
    }

    fn drain_recv_channel(&mut self, state: &mut NativeEventLoopState) -> bool {
        while let Some(msg) = self.rx.recv() {
            match msg {
                EventLoopMsg::Input(input) => state.write_list.push_back(input),
                EventLoopMsg::Resize(window_size) => {
                    state.kitty_graphics_cursor_tracker.reset_scroll_region();
                    self.pty.on_resize(window_size);
                }
                EventLoopMsg::NudgeResize(window_size) => self.pty.on_resize(window_size),
                EventLoopMsg::Shutdown => return false,
            }
        }

        true
    }

    fn handle_osc_events(&self, osc_events: Vec<OscEvent>) {
        for osc_event in osc_events {
            self.event_proxy
                .send_terminal_event(terminal_event_from_osc(osc_event));
        }
    }

    fn pty_read(&mut self, state: &mut NativeEventLoopState, buf: &mut [u8]) -> io::Result<()> {
        let mut parsed = 0usize;
        let mut processed = 0usize;
        let mut unprocessed = 0usize;
        let mut graphics_changed = false;
        let mut term_mutated = false;

        let _terminal_lease = Some(self.render_state.terminal.lease());
        let mut terminal = None;

        loop {
            // Whether this iteration pulled new bytes off the PTY. When the
            // PTY is dry and the terminal lock is contended, looping back to
            // read() would hot-spin a core; block on the lock instead.
            let mut read_progressed = false;
            match self.pty.reader().read(&mut buf[unprocessed..]) {
                Ok(0) if unprocessed == 0 => break,
                Ok(got) => {
                    read_progressed = got > 0;
                    unprocessed += got;
                }
                Err(err) => match err.kind() {
                    ErrorKind::Interrupted | ErrorKind::WouldBlock => {
                        if unprocessed == 0 {
                            break;
                        }
                    }
                    _ => return Err(err),
                },
            }

            let terminal = match &mut terminal {
                Some(terminal) => terminal,
                None => terminal.insert(match self.render_state.terminal.try_lock_unfair() {
                    None if !read_progressed
                        || unprocessed >= NATIVE_EVENT_LOOP_READ_BUFFER_SIZE =>
                    {
                        self.render_state.terminal.lock_unfair()
                    }
                    None => continue,
                    Some(terminal) => terminal,
                }),
            };

            let (filtered, kitty_clipboard_inputs) = state
                .kitty_clipboard_interceptor
                .process(&buf[..unprocessed]);
            for input in kitty_clipboard_inputs {
                self.event_proxy.send_kitty_clipboard_input(input);
            }
            let (filtered, osc_events) = state.osc_interceptor.process(&filtered);
            processed = processed.saturating_add(unprocessed);
            self.handle_osc_events(osc_events);

            for item in state.kitty_graphics_interceptor.process(&filtered) {
                match item {
                    KittyGraphicsItem::Text(text) => {
                        parsed = parsed.saturating_add(text.len());
                        term_mutated = true;
                        let track_scrolls = self.kitty_graphics.lock().has_placements();
                        let effects = advance_terminal_text(
                            &mut state.kitty_graphics_cursor_tracker,
                            &mut state.parser,
                            &mut **terminal,
                            &text,
                            track_scrolls,
                            Some(&self.resize_anchor_state),
                        );
                        if !effects.is_empty() {
                            graphics_changed |= effects.apply_to(&mut self.kitty_graphics.lock());
                        }
                    }
                    KittyGraphicsItem::Command(command) => {
                        let cursor = terminal.grid().cursor.point;
                        let history_size = terminal.grid().history_size();
                        let screen_lines = terminal.grid().screen_lines();
                        let full_screen_scroll_region = state
                            .kitty_graphics_cursor_tracker
                            .region_covers_full_screen(screen_lines);
                        let screen = KittyGraphicsScreen::from_alternate_screen(
                            terminal.mode().contains(TermMode::ALT_SCREEN),
                        );
                        let size = *self.terminal_size.lock();
                        let result = self.kitty_graphics.lock().apply_on_screen(
                            command,
                            cursor.column.0,
                            cursor.line.0.max(0) as usize,
                            history_size,
                            size,
                            screen,
                        );
                        graphics_changed |= result.changed;
                        if let Some(response) = result.response {
                            state.write_list.push_back(Cow::Owned(response));
                        }
                        if result.cursor_advance_screen == Some(screen)
                            && let Some((cols, rows)) = result.cursor_advance
                        {
                            let untracked_scroll = move_graphics_cursor_and_record(
                                &mut **terminal,
                                cols,
                                rows,
                                full_screen_scroll_region,
                                &self.render_state.generation,
                                &self.render_state.palette_revision,
                                &self.render_state.palette_snapshot,
                            );
                            if untracked_scroll > 0 {
                                graphics_changed |= self
                                    .kitty_graphics
                                    .lock()
                                    .scroll_up_without_history_on_screen(untracked_scroll, screen);
                            }
                        }
                    }
                }
            }

            unprocessed = 0;
            if processed >= NATIVE_EVENT_LOOP_MAX_LOCKED_READ {
                break;
            }
        }

        if graphics_changed {
            self.kitty_graphics_revision.fetch_add(1, Ordering::Relaxed);
        }
        if term_mutated && let Some(terminal) = terminal.as_deref() {
            self.render_state.record_mutation(terminal);
        }
        if graphics_changed || (state.parser.sync_bytes_count() < parsed && parsed > 0) {
            self.event_proxy.send_event(AlacEvent::Wakeup);
        }

        Ok(())
    }

    fn pty_write(&mut self, state: &mut NativeEventLoopState) -> io::Result<()> {
        state.ensure_next();

        'write_many: while let Some(mut current) = state.take_current() {
            'write_one: loop {
                match self.pty.writer().write(current.remaining_bytes()) {
                    Ok(0) => {
                        state.set_current(Some(current));
                        break 'write_many;
                    }
                    Ok(count) => {
                        current.advance(count);
                        if current.finished() {
                            state.goto_next();
                            break 'write_one;
                        }
                    }
                    Err(err) => {
                        state.set_current(Some(current));
                        match err.kind() {
                            ErrorKind::Interrupted | ErrorKind::WouldBlock => break 'write_many,
                            _ => return Err(err),
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn spawn(mut self) {
        let _ = thread::spawn_named("PTY reader", move || {
            let mut state = NativeEventLoopState::default();
            let mut buf = [0u8; NATIVE_EVENT_LOOP_READ_BUFFER_SIZE];
            let poll_mode = PollMode::Level;
            let mut interest = PollingEvent::readable(NATIVE_EVENT_LOOP_READ_WRITE_TOKEN);

            if unsafe { self.pty.register(&self.poll, interest, poll_mode) }.is_err() {
                return;
            }

            // Only the PTY read/write source and child-exit source are
            // registered. Reserving 1024 event slots dirtied tens of KiB per
            // pane for a poll result that normally contains one or two items.
            let mut events = Events::with_capacity(
                NonZeroUsize::new(NATIVE_EVENT_LOOP_POLL_EVENT_CAPACITY).expect("non-zero"),
            );

            'event_loop: loop {
                let timeout = self
                    .rx
                    .peek()
                    .is_none()
                    .then(|| state.parser.sync_timeout().sync_timeout())
                    .flatten()
                    .map(|deadline| deadline.saturating_duration_since(Instant::now()));

                events.clear();
                if self.poll.wait(&mut events, timeout).is_err() {
                    break 'event_loop;
                }

                if events.is_empty() && self.rx.peek().is_none() {
                    let mut term = self.render_state.terminal.lock();
                    stop_synchronized_update(
                        &mut state.parser,
                        &mut term,
                        &self.render_state.generation,
                        &self.render_state.palette_revision,
                        &self.render_state.palette_snapshot,
                    );
                    self.event_proxy.send_event(AlacEvent::Wakeup);
                    continue;
                }

                if !self.drain_recv_channel(&mut state) {
                    break 'event_loop;
                }

                for event in events.iter() {
                    match event.key {
                        NATIVE_EVENT_LOOP_CHILD_EVENT_TOKEN => {
                            if let Some(tty::ChildEvent::Exited(_)) = self.pty.next_child_event() {
                                if self.drain_on_exit {
                                    let _ = self.pty_read(&mut state, &mut buf);
                                }
                                self.render_state.terminal.lock().exit();
                                self.event_proxy.send_event(AlacEvent::Wakeup);
                                break 'event_loop;
                            }
                        }
                        NATIVE_EVENT_LOOP_READ_WRITE_TOKEN => {
                            if event.is_interrupt() {
                                continue;
                            }

                            if event.readable && self.pty_read(&mut state, &mut buf).is_err() {
                                break 'event_loop;
                            }

                            if event.writable && self.pty_write(&mut state).is_err() {
                                break 'event_loop;
                            }
                        }
                        _ => {}
                    }
                }

                let needs_write = state.needs_write();
                if needs_write != interest.writable {
                    interest.writable = needs_write;
                    if self
                        .pty
                        .reregister(&self.poll, interest, poll_mode)
                        .is_err()
                    {
                        break 'event_loop;
                    }
                }
            }

            let _ = self.pty.deregister(&self.poll);
        });
    }
}

/// Terminal dimensions in cells and pixels
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerminalSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: f32,
    pub cell_height: f32,
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            cell_width: 9.0,
            cell_height: 18.0,
        }
    }
}

impl TerminalSize {
    /// Clamp the cell dimensions into the supported range. Applied at every
    /// entry point that sizes the grid (`Terminal::new`, `new_display`,
    /// `resize`) so a buggy or hostile embedder cannot request a grid large
    /// enough to exhaust memory. The pixel cell metrics are left untouched.
    /// Columns/rows are floored at 1 so downstream grid math never sees a zero
    /// dimension.
    fn clamped(self) -> Self {
        Self {
            cols: self.cols.clamp(1, MAX_TERMINAL_COLS),
            rows: self.rows.clamp(1, MAX_TERMINAL_ROWS),
            ..self
        }
    }
}

impl From<TerminalSize> for WindowSize {
    fn from(size: TerminalSize) -> Self {
        // Kitty clients (and Grok's fit_image_to_cells) read TIOCGWINSZ
        // ws_xpixel/ws_ypixel via these cell metrics. A zero here makes
        // `cols * cell_width == 0`, which often causes clients to fall back
        // to treating image pixels as cell counts and request full-screen
        // placements.
        WindowSize {
            num_cols: size.cols,
            num_lines: size.rows,
            cell_width: size.cell_width.round().clamp(1.0, u16::MAX as f32) as u16,
            cell_height: size.cell_height.round().clamp(1.0, u16::MAX as f32) as u16,
        }
    }
}

impl Dimensions for TerminalSize {
    fn total_lines(&self) -> usize {
        self.rows as usize
    }

    fn screen_lines(&self) -> usize {
        self.rows as usize
    }

    fn columns(&self) -> usize {
        self.cols as usize
    }

    fn last_column(&self) -> alacritty_terminal::index::Column {
        alacritty_terminal::index::Column(self.cols.saturating_sub(1) as usize)
    }

    fn bottommost_line(&self) -> alacritty_terminal::index::Line {
        alacritty_terminal::index::Line(i32::from(self.rows.saturating_sub(1)))
    }

    fn topmost_line(&self) -> alacritty_terminal::index::Line {
        alacritty_terminal::index::Line(0)
    }
}

/// The terminal state wrapper
pub struct Terminal {
    backend: engine_backend::Backend,
}

mod engine_backend;
mod tmon_backend;

mod alacritty_backend;

impl Terminal {
    /// The active engine name for diagnostics. Do not branch application
    /// behavior on this value; construction policy remains owned by core.
    pub fn engine_label(&self) -> &'static str {
        self.backend.engine_label()
    }

    /// Create a new terminal with the given size.
    pub fn new(
        size: TerminalSize,
        configured_working_dir: Option<&str>,
        event_wakeup_tx: Option<Sender<()>>,
        tab_title_shell_integration: Option<&TabTitleShellIntegration>,
        runtime_config: Option<&TerminalRuntimeConfig>,
        startup_command: Option<&str>,
    ) -> anyhow::Result<Self> {
        engine_backend::Backend::new(
            size,
            configured_working_dir,
            event_wakeup_tx,
            tab_title_shell_integration,
            runtime_config,
            startup_command,
        )
        .map(|backend| Self { backend })
    }

    /// Create a terminal whose wakeups are routed through a host callback.
    pub fn new_with_wakeup_notifier(
        size: TerminalSize,
        configured_working_dir: Option<&str>,
        wakeup_notifier: Option<TerminalWakeupNotifier>,
        tab_title_shell_integration: Option<&TabTitleShellIntegration>,
        runtime_config: Option<&TerminalRuntimeConfig>,
        startup_command: Option<&str>,
    ) -> anyhow::Result<Self> {
        engine_backend::Backend::new_with_wakeup_notifier(
            size,
            configured_working_dir,
            wakeup_notifier,
            tab_title_shell_integration,
            runtime_config,
            startup_command,
        )
        .map(|backend| Self { backend })
    }

    /// Create a terminal whose child is selected with a typed launch contract.
    pub fn new_with_launch_and_wakeup_notifier(
        size: TerminalSize,
        configured_working_dir: Option<&str>,
        wakeup_notifier: Option<TerminalWakeupNotifier>,
        tab_title_shell_integration: Option<&TabTitleShellIntegration>,
        runtime_config: Option<&TerminalRuntimeConfig>,
        launch: Option<&TerminalLaunch>,
    ) -> anyhow::Result<Self> {
        engine_backend::Backend::new_with_launch_and_wakeup_notifier(
            size,
            configured_working_dir,
            wakeup_notifier,
            tab_title_shell_integration,
            runtime_config,
            launch,
        )
        .map(|backend| Self { backend })
    }

    /// Create a display-only terminal with no PTY or child process.
    pub fn new_display(size: TerminalSize, runtime_config: Option<&TerminalRuntimeConfig>) -> Self {
        Self {
            backend: engine_backend::Backend::new_display(size, runtime_config),
        }
    }

    /// Create a display-only terminal whose committed output wakes the host.
    pub fn new_display_with_wakeup_notifier(
        size: TerminalSize,
        runtime_config: Option<&TerminalRuntimeConfig>,
        wakeup_notifier: Option<TerminalWakeupNotifier>,
    ) -> Self {
        Self {
            backend: engine_backend::Backend::new_display_with_wakeup_notifier(
                size,
                runtime_config,
                wakeup_notifier,
            ),
        }
    }

    #[cfg(test)]
    fn new_alacritty_display_for_test(
        size: TerminalSize,
        runtime_config: Option<&TerminalRuntimeConfig>,
    ) -> Self {
        Self {
            backend: engine_backend::Backend::new_alacritty_display_for_test(size, runtime_config),
        }
    }

    pub fn feed_output(&self, bytes: &[u8]) {
        self.backend.feed_output(bytes);
    }

    pub fn child_pid(&self) -> Option<u32> {
        self.backend.child_pid()
    }

    pub fn set_wakeup_enabled(&self, enabled: bool) {
        self.backend.set_wakeup_enabled(enabled);
    }

    pub fn write(&self, input: &[u8]) {
        self.backend.write(input);
    }

    pub fn write_owned(&self, input: Vec<u8>) {
        self.backend.write_owned(input);
    }

    pub fn hydrate_output(&self, bytes: &[u8]) {
        self.backend.hydrate_output(bytes);
    }

    #[allow(dead_code)]
    pub fn write_str(&self, input: &str) {
        self.backend.write_str(input);
    }

    pub fn resize(&mut self, new_size: TerminalSize) {
        self.backend.resize(new_size);
    }

    pub fn nudge_resize(&self) {
        self.backend.nudge_resize();
    }

    pub fn size(&self) -> TerminalSize {
        self.backend.size()
    }

    pub fn kitty_graphics_placements(&self) -> Vec<KittyGraphicsRenderPlacement> {
        self.backend.kitty_graphics_placements()
    }

    pub fn kitty_graphics_revision(&self) -> u64 {
        self.backend.kitty_graphics_revision()
    }

    pub fn kitty_graphics_snapshot(&self) -> (u64, Vec<KittyGraphicsRenderPlacement>) {
        self.backend.kitty_graphics_snapshot()
    }

    pub fn kitty_clipboard_paste_events_enabled(&self) -> bool {
        self.backend.kitty_clipboard_paste_events_enabled()
    }

    pub fn send_kitty_clipboard_paste_event(
        &self,
        location: TerminalClipboardLocation,
        available_formats: &[String],
    ) -> bool {
        self.backend
            .send_kitty_clipboard_paste_event(location, available_formats)
    }

    pub fn drain_events(&self, host: &mut impl TerminalReplyHost) -> (Vec<TerminalEvent>, bool) {
        self.backend.drain_events(host)
    }

    pub fn set_query_colors(&mut self, query_colors: TerminalQueryColors) {
        self.backend.set_query_colors(query_colors);
    }

    pub fn palette(&self) -> crate::TerminalPalette {
        self.backend.palette()
    }

    pub fn snapshot(&self) -> TermyFrame {
        self.backend.snapshot()
    }

    pub fn frame_update(&self, force_full: bool) -> TermyFrameUpdate {
        self.backend.frame_update(force_full)
    }

    pub fn take_render_damage_snapshot(&self) -> TerminalRenderDamageSnapshot {
        self.backend.take_render_damage_snapshot()
    }

    pub fn render_read(&self, force_full: bool) -> TerminalRenderRead {
        self.backend.render_read(force_full)
    }

    pub fn visit_viewport_cells(
        &self,
        visitor: impl FnMut(usize, i32, usize, &crate::TerminalRenderCell),
    ) -> TerminalViewportMetadata {
        self.backend.visit_viewport_cells(visitor)
    }

    pub fn visit_viewport_ranges_at_generation(
        &self,
        generation: u64,
        spans: &[TerminalDirtySpan],
        visitor: impl FnMut(usize, usize, i32, usize, &crate::TerminalRenderCell),
    ) -> bool {
        self.backend
            .visit_viewport_ranges_at_generation(generation, spans, visitor)
    }

    pub fn line_bounds(&self) -> (i32, i32) {
        self.backend.line_bounds()
    }

    /// Visit a requested inclusive buffer-line range from one coherent backend state.
    ///
    /// The callback runs under the backend lock and must not call back into this terminal.
    pub fn visit_line_cells(
        &self,
        requested_first: i32,
        requested_last: i32,
        visitor: impl FnMut((i32, i32, usize), i32, usize, &crate::TerminalRenderCell),
    ) -> (i32, i32, usize) {
        self.backend
            .visit_line_cells(requested_first, requested_last, visitor)
    }

    pub fn search(&self, query: &str) -> Vec<TermySearchMatch> {
        self.backend.search(query)
    }

    pub fn search_with_options(
        &self,
        query: &str,
        options: TermySearchOptions,
    ) -> Vec<TermySearchMatch> {
        self.backend.search_with_options(query, options)
    }

    pub fn search_shared(&self, query: &str) -> Vec<TermySharedSearchMatch> {
        self.backend.search_shared(query)
    }

    pub fn search_shared_with_options(
        &self,
        query: &str,
        options: TermySearchOptions,
    ) -> Vec<TermySharedSearchMatch> {
        self.backend.search_shared_with_options(query, options)
    }

    pub fn hyperlink_at(&self, row: usize, col: usize) -> Option<crate::links::DetectedLink> {
        self.backend.hyperlink_at(row, col)
    }

    pub fn link_at(&self, row: usize, col: usize) -> Option<crate::links::DetectedViewportLink> {
        self.backend.link_at(row, col)
    }

    pub fn take_damage_snapshot(&self) -> TerminalDamageSnapshot {
        self.backend.take_damage_snapshot()
    }

    pub fn scroll_display(&self, delta_lines: i32) -> bool {
        self.backend.scroll_display(delta_lines)
    }

    pub fn scroll_to_bottom(&self) -> bool {
        self.backend.scroll_to_bottom()
    }

    pub fn clear_scrollback(&self) -> bool {
        self.backend.clear_scrollback()
    }

    pub fn scroll_state(&self) -> (usize, usize) {
        self.backend.scroll_state()
    }

    pub fn cursor_state(&self) -> Option<TerminalCursorState> {
        self.backend.cursor_state()
    }

    pub fn cursor_position(&self) -> (usize, usize) {
        self.backend.cursor_position()
    }

    #[allow(dead_code)]
    pub fn has_pending_events(&self) -> bool {
        self.backend.has_pending_events()
    }

    pub fn set_term_options(&self, options: TerminalOptions) {
        self.backend.set_term_options(options);
    }

    pub fn set_scrollback_history(&self, scrollback_history: usize) {
        self.backend.set_scrollback_history(scrollback_history);
    }

    pub fn bracketed_paste_mode(&self) -> bool {
        self.backend.bracketed_paste_mode()
    }

    pub fn mouse_mode(&self) -> TerminalMouseMode {
        self.backend.mouse_mode()
    }

    pub fn keyboard_mode(&self) -> TerminalKeyboardMode {
        self.backend.keyboard_mode()
    }

    pub fn alternate_screen_mode(&self) -> bool {
        self.backend.alternate_screen_mode()
    }
}

/// Maximum number of alacritty events to drain in a single frame. Prevents
/// massive output (e.g. `cat huge_file`) from blocking the render thread.
const EVENT_DRAIN_BATCH_LIMIT: usize = 2048;

/// Drain pending events, returning the collected terminal events and whether
/// the batch limit was hit (indicating more events remain).
fn drain_runtime_events<T: EventListener>(
    first_event: RuntimeEvent,
    events_rx: &Receiver<RuntimeEvent>,
    size: TerminalSize,
    term: &FairMutex<Term<T>>,
    query_colors: TerminalQueryColors,
    kitty_clipboard: &FairMutex<KittyClipboardHostState>,
    host: &mut impl TerminalReplyHost,
    mut write_reply: impl FnMut(&[u8]) -> bool,
) -> (Vec<TerminalEvent>, bool) {
    let fallback_live_colors = alacritty_terminal::term::color::Colors::default();
    let mut events = Vec::with_capacity(16);
    let mut drained = 0usize;
    let mut wakeup_pending = false;

    let mut next_event = Some(first_event);
    while let Some(runtime_event) = next_event.take().or_else(|| events_rx.try_recv().ok()) {
        match runtime_event {
            RuntimeEvent::Alacritty(event) => {
                let response = match &event {
                    AlacEvent::ColorRequest(_, _) => {
                        let term = term.lock();
                        reply_bytes_for_event(&event, size, term.colors(), query_colors, host)
                    }
                    _ => reply_bytes_for_event(
                        &event,
                        size,
                        &fallback_live_colors,
                        query_colors,
                        host,
                    ),
                };

                if let Some(response) = response {
                    dispatch_protocol_reply(host, &mut write_reply, &response);
                }

                if let Some(event) = terminal_event_from_alacritty(event) {
                    push_drained_terminal_event(&mut events, &mut wakeup_pending, event);
                }
            }
            RuntimeEvent::Terminal(event) => {
                push_drained_terminal_event(&mut events, &mut wakeup_pending, event);
            }
            RuntimeEvent::KittyClipboard(packet) => {
                for response in kitty_clipboard.lock().handle_osc(packet, host) {
                    dispatch_protocol_reply(host, &mut write_reply, &response);
                }
            }
            RuntimeEvent::KittyClipboardControl(control) => match control {
                KittyClipboardControl::Set(enabled) => {
                    kitty_clipboard.lock().set_paste_events_enabled(enabled);
                }
                KittyClipboardControl::Reset => kitty_clipboard.lock().reset(),
                KittyClipboardControl::Query => {
                    let status = if kitty_clipboard.lock().paste_events_enabled() {
                        1
                    } else {
                        2
                    };
                    dispatch_protocol_reply(
                        host,
                        &mut write_reply,
                        format!("\x1b[?5522;{status}$y").as_bytes(),
                    );
                }
            },
        }

        drained += 1;
        if drained >= EVENT_DRAIN_BATCH_LIMIT {
            flush_pending_wakeup(&mut events, &mut wakeup_pending);
            return (events, true);
        }
    }

    flush_pending_wakeup(&mut events, &mut wakeup_pending);
    (events, false)
}

fn dispatch_protocol_reply(
    host: &mut impl TerminalReplyHost,
    write_reply: &mut impl FnMut(&[u8]) -> bool,
    response: &[u8],
) {
    if !write_reply(response) {
        host.protocol_reply(response);
    }
}

fn push_drained_terminal_event(
    events: &mut Vec<TerminalEvent>,
    wakeup_pending: &mut bool,
    event: TerminalEvent,
) {
    if matches!(event, TerminalEvent::Wakeup) {
        *wakeup_pending = true;
        return;
    }

    flush_pending_wakeup(events, wakeup_pending);
    events.push(event);
}

fn flush_pending_wakeup(events: &mut Vec<TerminalEvent>, wakeup_pending: &mut bool) {
    if *wakeup_pending {
        events.push(TerminalEvent::Wakeup);
        *wakeup_pending = false;
    }
}

fn terminal_event_from_alacritty(event: AlacEvent) -> Option<TerminalEvent> {
    match event {
        AlacEvent::Wakeup => Some(TerminalEvent::Wakeup),
        AlacEvent::Title(title) => Some(TerminalEvent::Title(title)),
        AlacEvent::ResetTitle => Some(TerminalEvent::ResetTitle),
        AlacEvent::Bell => Some(TerminalEvent::Bell),
        AlacEvent::Exit => Some(TerminalEvent::Exit),
        AlacEvent::ClipboardStore(_, text) => Some(TerminalEvent::ClipboardStore(text)),
        _ => None,
    }
}

fn terminal_event_from_osc(event: OscEvent) -> TerminalEvent {
    match event {
        OscEvent::WorkingDirectory(path) => TerminalEvent::WorkingDirectory(path),
        OscEvent::Progress(state) => TerminalEvent::Progress(state),
        OscEvent::ShellPromptStart => TerminalEvent::ShellPromptStart,
        OscEvent::ShellCommandStart => TerminalEvent::ShellCommandStart,
        OscEvent::ShellCommandExecuting => TerminalEvent::ShellCommandExecuting,
        OscEvent::ShellCommandFinished(code) => TerminalEvent::ShellCommandFinished(code),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_TERM, EVENT_QUEUE_HARD_CAP, EVENT_QUEUE_SOFT_CAP, GHOSTTY_COMPAT_TERM_PROGRAM,
        GHOSTTY_COMPAT_TERM_PROGRAM_VERSION, JsonEventListener, KittyGraphicsCursorTracker,
        MAX_TERMINAL_COLS, MAX_TERMINAL_ROWS, MAX_TERMINAL_SCROLLBACK_HISTORY, RuntimeEvent,
        TERMY_TERM_PROGRAM, Terminal, TerminalCursorState, TerminalCursorStyle,
        TerminalDamageSnapshot, TerminalEvent, TerminalLaunch, TerminalOptions,
        TerminalRuntimeConfig, TerminalSize, TerminalWakeupNotifier, WindowsShell,
        WorkingDirFallback, advance_terminal_text, capture_viewport_ranges_at_generation,
        drain_runtime_events, normalize_working_directory_candidate,
        resolve_launch_working_directory, resolve_shell_path, resolve_terminal_launch,
        search_term_buffer, should_drop_event, stop_synchronized_update,
        terminal_environment_overrides, terminal_event_from_osc, user_home_dir,
    };
    #[cfg(target_os = "windows")]
    use super::{
        default_shell_launch, quote_shell_program_if_needed, windows_cmd_path,
        windows_git_bash_path,
    };
    use crate::backend;
    use crate::keyboard::{
        Keystroke, Modifiers, TerminalKeyEventKind, TerminalKeyboardMode, keystroke_to_input,
    };
    use crate::protocol::{
        TerminalClipboardReadRequest, TerminalClipboardReadResult, TerminalClipboardTarget,
        TerminalClipboardWriteRequest, TerminalClipboardWriteResult, TerminalQueryColors,
        TerminalReplyHost,
    };
    use crate::resize_anchor::ResizeAnchorState;
    use crate::search::TermySearchOptions;
    use alacritty_terminal::{
        event::{Event as AlacEvent, EventListener, VoidListener, WindowSize},
        grid::Dimensions,
        index::{Column, Line},
        sync::FairMutex,
        term::{ClipboardType, Config as TermConfig, Term, TermMode},
        vte::ansi::{self, CursorShape, NamedColor},
    };
    use flume::unbounded;
    use std::collections::HashMap;
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };

    #[test]
    fn terminal_size_clamps_absurd_dimensions() {
        let huge = TerminalSize {
            cols: u16::MAX,
            rows: u16::MAX,
            cell_width: 8.0,
            cell_height: 16.0,
        }
        .clamped();
        assert_eq!(huge.cols, MAX_TERMINAL_COLS);
        assert_eq!(huge.rows, MAX_TERMINAL_ROWS);
    }

    #[test]
    fn display_terminal_intercepts_and_places_kitty_graphics() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        let initial_revision = terminal.kitty_graphics_revision();
        terminal.feed_output(b"\x1b_Ga=T,f=32,s=1,v=1,i=77,c=2,r=3;AQID/w==\x1b\\");

        let (revision, placements) = terminal.kitty_graphics_snapshot();
        assert!(revision > initial_revision);
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].image_id, 77);
        assert_eq!(placements[0].display_cols, Some(2));
        assert_eq!(placements[0].display_rows, Some(3));
        assert!(placements[0].png.starts_with(b"\x89PNG"));

        let cursor = terminal.cursor_position();
        assert_eq!(cursor, (2, 3));
    }

    #[test]
    fn display_terminal_notifier_coalesces_until_events_are_drained() {
        let notifications = Arc::new(AtomicU64::new(0));
        let notification_count = notifications.clone();
        let terminal = Terminal::new_display_with_wakeup_notifier(
            test_terminal_size(),
            None,
            Some(TerminalWakeupNotifier::new(move || {
                notification_count.fetch_add(1, Ordering::Relaxed);
            })),
        );

        terminal.feed_output(b"a");
        terminal.feed_output(b"b");
        assert_eq!(notifications.load(Ordering::Relaxed), 1);

        let mut reply_host = RecordingReplyHost::default();
        let _ = terminal.drain_events(&mut reply_host);
        terminal.feed_output(b"c");
        assert_eq!(notifications.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn kitty_command_only_cursor_movement_rejects_old_render_generation() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        let update = terminal.take_render_damage_snapshot();

        terminal.feed_output(b"\x1b_Ga=T,f=32,s=1,v=1,i=177,c=2,r=3;AQID/w==\x1b\\");

        let mut visited = 0;
        assert!(!terminal.visit_viewport_ranges_at_generation(
            update.generation,
            &[crate::TerminalDirtySpan {
                row: 0,
                left_col: 0,
                right_col: 0,
            }],
            |_, _, _, _, _| visited += 1,
        ));
        assert_eq!(visited, 0);
    }

    #[test]
    fn synchronized_update_watchdog_replay_rejects_old_render_generation() {
        let size = test_terminal_size();
        let listener = VoidListener;
        let mut term = Term::new(TermConfig::default(), &size, listener);
        let palette_snapshot = FairMutex::new(backend::palette(&term, 0));
        let generation = AtomicU64::new(0);
        let palette_revision = AtomicU64::new(0);
        let mut parser = ansi::Processor::new();

        parser.advance(&mut term, b"\x1b[?2026h\x1b]4;1;#123456\x07X");
        let old_generation = generation.load(Ordering::Relaxed);
        assert_eq!(term.grid()[Line(0)][Column(0)].c, ' ');

        stop_synchronized_update(
            &mut parser,
            &mut term,
            &generation,
            &palette_revision,
            &palette_snapshot,
        );

        assert_eq!(term.grid()[Line(0)][Column(0)].c, 'X');
        assert!(generation.load(Ordering::Relaxed) > old_generation);
        assert!(
            capture_viewport_ranges_at_generation(
                &term,
                &generation,
                old_generation,
                &[crate::TerminalDirtySpan {
                    row: 0,
                    left_col: 0,
                    right_col: 0,
                }],
            )
            .is_none()
        );
        assert!(palette_revision.load(Ordering::Relaxed) > 0);
        assert_eq!(
            palette_snapshot.lock().indexed[1],
            Some(crate::TerminalColor {
                r: 0x12,
                g: 0x34,
                b: 0x56,
            })
        );
    }

    #[test]
    fn display_terminal_scrolls_for_kitty_cursor_advance_at_bottom() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.feed_output(b"\x1b[4;1H\x1b_Ga=T,f=32,s=1,v=1,i=78,c=2,r=3;AQID/w==\x1b\\");

        assert_eq!(terminal.scroll_state(), (0, 3));
        assert_eq!(terminal.cursor_position(), (2, 3));
        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].viewport_row, 0);
    }

    #[test]
    fn kitty_cursor_advance_tracks_zero_history_screen_scroll() {
        let runtime_config = TerminalRuntimeConfig {
            scrollback_history: 0,
            ..TerminalRuntimeConfig::default()
        };
        let terminal = Terminal::new_display(test_terminal_size(), Some(&runtime_config));
        terminal.feed_output(b"\x1b[4;1H\x1b_Ga=T,f=32,s=1,v=1,i=79,c=2,r=3;AQID/w==\x1b\\");

        assert_eq!(terminal.scroll_state(), (0, 0));
        assert_eq!(terminal.cursor_position(), (2, 3));
        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].viewport_row, 0);
    }

    #[test]
    fn kitty_cursor_advance_tracks_full_history_screen_scroll() {
        let runtime_config = TerminalRuntimeConfig {
            scrollback_history: 2,
            ..TerminalRuntimeConfig::default()
        };
        let terminal = Terminal::new_display(test_terminal_size(), Some(&runtime_config));
        terminal.feed_output(b"\x1b[4;1H\n\n");
        assert_eq!(terminal.scroll_state(), (0, 2));

        terminal.feed_output(b"\x1b_Ga=T,f=32,s=1,v=1,i=81,c=2,r=3;AQID/w==\x1b\\");

        assert_eq!(terminal.scroll_state(), (0, 2));
        assert_eq!(terminal.cursor_position(), (2, 3));
        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].viewport_row, 0);
    }

    #[test]
    fn kitty_cursor_advance_tracks_alternate_screen_scroll() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal
            .feed_output(b"\x1b[?1049h\x1b[4;1H\x1b_Ga=T,f=32,s=1,v=1,i=80,c=2,r=3;AQID/w==\x1b\\");

        assert!(terminal.alternate_screen_mode());
        assert_eq!(terminal.scroll_state(), (0, 0));
        assert_eq!(terminal.cursor_position(), (2, 3));
        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].viewport_row, 0);
    }

    #[test]
    fn ordinary_newlines_shift_and_remove_alternate_screen_kitty_placement() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.feed_output(
            b"\x1b[?1049h\x1b[2;1H\x1b_Ga=T,f=32,s=1,v=1,i=86,c=2,r=1,C=1;AQID/w==\x1b\\",
        );
        assert_eq!(terminal.kitty_graphics_placements()[0].viewport_row, 1);

        terminal.feed_output(b"\x1b[4;1H\n");
        assert_eq!(terminal.kitty_graphics_placements()[0].viewport_row, 0);

        terminal.feed_output(b"\n");
        assert!(terminal.kitty_graphics_placements().is_empty());
    }

    #[test]
    fn synchronized_newlines_shift_alternate_screen_kitty_placement() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.feed_output(
            b"\x1b[?1049h\x1b[2;1H\x1b_Ga=T,f=32,s=1,v=1,i=96,c=2,r=1,C=1;AQID/w==\x1b\\",
        );

        terminal.feed_output(b"\x1b[?2026h\x1b[3;1H\n\n\x1b[?2026l");

        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].viewport_row, 0);
    }

    #[test]
    fn ordinary_newlines_shift_and_remove_zero_history_kitty_placement() {
        let runtime_config = TerminalRuntimeConfig {
            scrollback_history: 0,
            ..TerminalRuntimeConfig::default()
        };
        let terminal = Terminal::new_display(test_terminal_size(), Some(&runtime_config));
        terminal.feed_output(b"\x1b[2;1H\x1b_Ga=T,f=32,s=1,v=1,i=87,c=2,r=1,C=1;AQID/w==\x1b\\");
        assert_eq!(terminal.kitty_graphics_placements()[0].viewport_row, 1);

        terminal.feed_output(b"\x1b[4;1H\n");
        assert_eq!(terminal.scroll_state(), (0, 0));
        assert_eq!(terminal.kitty_graphics_placements()[0].viewport_row, 0);

        terminal.feed_output(b"\n");
        assert!(terminal.kitty_graphics_placements().is_empty());
    }

    #[test]
    fn ordinary_wrapped_text_shifts_and_removes_zero_history_kitty_placement() {
        let runtime_config = TerminalRuntimeConfig {
            scrollback_history: 0,
            ..TerminalRuntimeConfig::default()
        };
        let terminal = Terminal::new_display(test_terminal_size(), Some(&runtime_config));
        terminal.feed_output(b"\x1b[2;1H\x1b_Ga=T,f=32,s=1,v=1,i=97,c=2,r=1,C=1;AQID/w==\x1b\\");

        terminal.feed_output(b"\x1b[4;32Hab");
        assert_eq!(terminal.kitty_graphics_placements()[0].viewport_row, 0);

        terminal.feed_output("\x1b[4;32H界".as_bytes());
        assert!(terminal.kitty_graphics_placements().is_empty());
    }

    #[test]
    fn ordinary_newlines_shift_and_remove_full_history_kitty_placement() {
        let runtime_config = TerminalRuntimeConfig {
            scrollback_history: 1,
            ..TerminalRuntimeConfig::default()
        };
        let terminal = Terminal::new_display(test_terminal_size(), Some(&runtime_config));
        terminal.feed_output(b"\x1b[4;1H\n");
        assert_eq!(terminal.scroll_state(), (0, 1));
        terminal.feed_output(b"\x1b[2;1H\x1b_Ga=T,f=32,s=1,v=1,i=88,c=2,r=1,C=1;AQID/w==\x1b\\");
        assert_eq!(terminal.kitty_graphics_placements()[0].viewport_row, 1);

        terminal.feed_output(b"\x1b[4;1H\n");
        assert_eq!(terminal.scroll_state(), (0, 1));
        assert_eq!(terminal.kitty_graphics_placements()[0].viewport_row, 0);

        terminal.feed_output(b"\n");
        assert!(terminal.kitty_graphics_placements().is_empty());
    }

    #[test]
    fn ordinary_partial_region_scroll_does_not_shift_kitty_placement() {
        let runtime_config = TerminalRuntimeConfig {
            scrollback_history: 0,
            ..TerminalRuntimeConfig::default()
        };
        let terminal = Terminal::new_display(test_terminal_size(), Some(&runtime_config));
        terminal.feed_output(b"\x1b[1;1H\x1b_Ga=T,f=32,s=1,v=1,i=89,c=2,r=1,C=1;AQID/w==\x1b\\");

        terminal.feed_output(b"\x1b[2;3r\x1b[3;1H\n");

        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].viewport_row, 0);
    }

    #[test]
    fn top_anchored_partial_region_scroll_keeps_footer_kitty_placement_fixed() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.feed_output(b"\x1b[4;1H\x1b_Ga=T,f=32,s=1,v=1,i=90,c=2,r=1,C=1;AQID/w==\x1b\\");
        assert_eq!(terminal.kitty_graphics_placements()[0].viewport_row, 3);

        terminal.feed_output(b"\x1b[1;3r\x1b[3;1H\n");

        assert_eq!(terminal.scroll_state(), (0, 1));
        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].viewport_row, 3);
    }

    #[test]
    fn deccolm_resets_tracked_scroll_region() {
        let size = test_terminal_size();
        let mut term: Term<VoidListener> = Term::new(TermConfig::default(), &size, VoidListener);
        let mut parser = ansi::Processor::new();
        let mut tracker = KittyGraphicsCursorTracker::default();
        advance_terminal_text(
            &mut tracker,
            &mut parser,
            &mut term,
            b"\x1b[2;3r",
            false,
            None,
        );
        assert!(!tracker.region_covers_full_screen(4));

        advance_terminal_text(
            &mut tracker,
            &mut parser,
            &mut term,
            b"\x1b[?3h",
            false,
            None,
        );
        assert!(tracker.region_covers_full_screen(4));

        advance_terminal_text(
            &mut tracker,
            &mut parser,
            &mut term,
            b"\x1b[2;3r\x1b[?3l",
            false,
            None,
        );
        assert!(tracker.region_covers_full_screen(4));
    }

    #[test]
    fn primary_ed2_suppresses_resize_anchor_until_live_output_reaches_bottom() {
        let size = test_terminal_size();
        let mut term: Term<VoidListener> = Term::new(TermConfig::default(), &size, VoidListener);
        let mut parser = ansi::Processor::new();
        let mut tracker = KittyGraphicsCursorTracker::default();
        let state = ResizeAnchorState::default();

        advance_terminal_text(
            &mut tracker,
            &mut parser,
            &mut term,
            b"\x1b[2Jx",
            false,
            Some(&state),
        );
        assert!(!state.allows_restore());

        advance_terminal_text(
            &mut tracker,
            &mut parser,
            &mut term,
            b"\r\n1\r\n2\r\n3",
            false,
            Some(&state),
        );
        assert!(state.allows_restore());
    }

    #[test]
    fn alternate_ed2_does_not_suppress_primary_resize_anchor() {
        let size = test_terminal_size();
        let mut term: Term<VoidListener> = Term::new(TermConfig::default(), &size, VoidListener);
        let mut parser = ansi::Processor::new();
        let mut tracker = KittyGraphicsCursorTracker::default();
        let state = ResizeAnchorState::default();

        advance_terminal_text(
            &mut tracker,
            &mut parser,
            &mut term,
            b"\x1b[?1049h\x1b[2J",
            false,
            Some(&state),
        );

        assert!(state.allows_restore());
    }

    #[test]
    fn history_clear_and_terminal_reset_release_resize_anchor_suppression() {
        let size = test_terminal_size();
        let mut term: Term<VoidListener> = Term::new(TermConfig::default(), &size, VoidListener);
        let mut parser = ansi::Processor::new();
        let mut tracker = KittyGraphicsCursorTracker::default();
        let state = ResizeAnchorState::default();

        advance_terminal_text(
            &mut tracker,
            &mut parser,
            &mut term,
            b"\x1b[2J",
            false,
            Some(&state),
        );
        assert!(!state.allows_restore());
        advance_terminal_text(
            &mut tracker,
            &mut parser,
            &mut term,
            b"\x1b[3J",
            false,
            Some(&state),
        );
        assert!(state.allows_restore());

        advance_terminal_text(
            &mut tracker,
            &mut parser,
            &mut term,
            b"\x1b[2J",
            false,
            Some(&state),
        );
        assert!(!state.allows_restore());
        advance_terminal_text(
            &mut tracker,
            &mut parser,
            &mut term,
            b"\x1bc",
            false,
            Some(&state),
        );
        assert!(state.allows_restore());
    }

    #[test]
    fn invalid_omitted_bottom_decstbm_preserves_tracked_region() {
        let size = test_terminal_size();
        let mut term: Term<VoidListener> = Term::new(TermConfig::default(), &size, VoidListener);
        let mut parser = ansi::Processor::new();
        let mut tracker = KittyGraphicsCursorTracker::default();
        advance_terminal_text(
            &mut tracker,
            &mut parser,
            &mut term,
            b"\x1b[2;3r",
            false,
            None,
        );
        assert!(!tracker.region_covers_full_screen(4));

        advance_terminal_text(
            &mut tracker,
            &mut parser,
            &mut term,
            b"\x1b[99r",
            false,
            None,
        );

        assert!(!tracker.region_covers_full_screen(4));
    }

    #[test]
    fn kitty_cursor_advance_does_not_scroll_partial_decstbm_region() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.feed_output(
            b"\x1b[2;3r\x1b[3;1H\x1b_Ga=T,f=32,s=1,v=1,i=82,c=2,r=3;AQID/w==\x1b\\\x1b[r",
        );

        assert_eq!(terminal.scroll_state(), (0, 0));
        assert_eq!(terminal.cursor_position(), (0, 0));
        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].image_id, 82);
        assert_eq!(placements[0].viewport_row, 2);
    }

    #[test]
    fn primary_kitty_placement_survives_alternate_screen_scroll() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.feed_output(b"\x1b[2;1H\x1b_Ga=T,f=32,s=1,v=1,i=83,c=2,r=2,C=1;AQID/w==\x1b\\");
        let primary = terminal.kitty_graphics_placements();
        assert_eq!(primary.len(), 1);
        assert_eq!(primary[0].image_id, 83);
        assert_eq!(primary[0].viewport_row, 1);

        terminal
            .feed_output(b"\x1b[?1049h\x1b[4;1H\x1b_Ga=T,f=32,s=1,v=1,i=84,c=2,r=3;AQID/w==\x1b\\");
        let alternate = terminal.kitty_graphics_placements();
        assert_eq!(alternate.len(), 1);
        assert_eq!(alternate[0].image_id, 84);
        assert_eq!(alternate[0].viewport_row, 0);

        terminal.feed_output(b"\x1b[?1049l");
        let restored_primary = terminal.kitty_graphics_placements();
        assert_eq!(restored_primary.len(), 1);
        assert_eq!(restored_primary[0].image_id, 83);
        assert_eq!(restored_primary[0].viewport_row, 1);

        terminal.feed_output(b"\x1b[?1049h");
        assert!(terminal.kitty_graphics_placements().is_empty());
    }

    #[test]
    fn terminal_reset_clears_kitty_graphics() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.feed_output(b"\x1b_Ga=T,f=32,s=1,v=1,i=85,c=2,r=2,C=1;AQID/w==\x1b\\");
        assert_eq!(terminal.kitty_graphics_placements().len(), 1);

        terminal.feed_output(b"\x1bc");

        assert!(!terminal.alternate_screen_mode());
        assert!(terminal.kitty_graphics_placements().is_empty());
    }

    #[test]
    fn clear_screen_erases_only_the_active_viewport_graphics() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.feed_output(b"\x1b[2;1H\x1b_Ga=T,f=32,s=1,v=1,i=98,c=2,r=1,C=1;AQID/w==\x1b\\");

        terminal.feed_output(b"\x1b[J\x1b[1J\x1b[3J");
        assert_eq!(
            terminal.kitty_graphics_placements().len(),
            1,
            "non-ED2 erase commands must not affect graphics"
        );

        terminal.feed_output(
            b"\x1b[?1049h\x1b[2;1H\x1b_Ga=T,f=32,s=1,v=1,i=99,c=2,r=1,C=1;AQID/w==\x1b\\",
        );
        assert_eq!(terminal.kitty_graphics_placements().len(), 1);
        terminal.feed_output(b"\x1b[2J");
        assert!(terminal.kitty_graphics_placements().is_empty());

        terminal.feed_output(b"\x1b[?1049l");
        assert_eq!(
            terminal.kitty_graphics_placements().len(),
            1,
            "clearing the alternate viewport must preserve primary graphics"
        );

        terminal.feed_output(b"\x1b[H\x1b[2J");
        assert!(terminal.kitty_graphics_placements().is_empty());
    }

    #[test]
    fn kitty_cursor_advance_caps_linefeeds_to_screen_height() {
        let size = test_terminal_size();
        let mut term: Term<VoidListener> = Term::new(TermConfig::default(), &size, VoidListener);
        let mut parser: ansi::Processor = ansi::Processor::new();
        parser.advance(&mut term, b"\x1b[4;1H");

        let untracked_scroll = backend::move_graphics_cursor(&mut term, u32::MAX, u32::MAX, true);

        assert_eq!(untracked_scroll, 0);
        assert_eq!(term.grid().history_size(), size.rows as usize);
        assert_eq!(term.grid().cursor.point.column.0, size.cols as usize - 1);
        assert_eq!(term.grid().cursor.point.line.0, i32::from(size.rows) - 1);
    }

    #[test]
    fn terminal_size_clamp_leaves_realistic_dimensions_untouched() {
        let clamped = TerminalSize {
            cols: 200,
            rows: 60,
            cell_width: 9.0,
            cell_height: 18.0,
        }
        .clamped();
        assert_eq!(clamped.cols, 200);
        assert_eq!(clamped.rows, 60);
    }

    #[test]
    fn window_size_cell_metrics_round_and_never_report_zero() {
        let rounded = WindowSize::from(TerminalSize {
            cols: 80,
            rows: 24,
            cell_width: 9.6,
            cell_height: 18.4,
        });
        assert_eq!(rounded.cell_width, 10);
        assert_eq!(rounded.cell_height, 18);

        let tiny = WindowSize::from(TerminalSize {
            cols: 80,
            rows: 24,
            cell_width: 0.2,
            cell_height: 0.0,
        });
        assert_eq!(tiny.cell_width, 1);
        assert_eq!(tiny.cell_height, 1);
    }

    #[test]
    fn identical_terminal_resize_does_not_redamage_the_grid() {
        let size = TerminalSize {
            cols: 80,
            rows: 24,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        let mut terminal = Terminal::new_display(size, None);
        assert_eq!(
            terminal.take_damage_snapshot(),
            TerminalDamageSnapshot::Full
        );
        let stable_damage = terminal.take_damage_snapshot();
        assert_eq!(terminal.take_damage_snapshot(), stable_damage);

        terminal.resize(size);

        assert_eq!(terminal.size(), size);
        assert_eq!(terminal.take_damage_snapshot(), stable_damage);
    }

    #[test]
    fn terminal_size_clamp_floors_zero_dimensions_at_one() {
        let empty = TerminalSize {
            cols: 0,
            rows: 0,
            cell_width: 9.0,
            cell_height: 18.0,
        }
        .clamped();
        assert_eq!(empty.cols, 1);
        assert_eq!(empty.rows, 1);
    }

    #[test]
    fn terminal_options_clamp_scrollback_history() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.set_term_options(TerminalOptions {
            scrollback_history: 10_000_000,
            default_cursor_style: TerminalCursorStyle::Block,
        });
        terminal.feed_output(
            &(0..MAX_TERMINAL_SCROLLBACK_HISTORY.saturating_add(100))
                .map(|_| "x\r\n")
                .collect::<String>()
                .into_bytes(),
        );
        assert!(terminal.scroll_state().1 <= MAX_TERMINAL_SCROLLBACK_HISTORY);
    }

    #[test]
    fn terminal_options_preserve_in_range_scrollback_history() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.set_term_options(TerminalOptions {
            scrollback_history: 5,
            default_cursor_style: TerminalCursorStyle::Block,
        });
        terminal.feed_output(&(0..20).map(|_| "x\r\n").collect::<String>().into_bytes());
        assert_eq!(terminal.scroll_state().1, 5);
    }

    #[test]
    fn hard_cap_sheds_reply_bearing_events_when_backlogged() {
        // A device-attributes reply the child may block on: exempt from the soft
        // cap, but the hard cap must shed it to keep memory bounded against a
        // hostile child on a non-draining pane.
        let reply = RuntimeEvent::Alacritty(AlacEvent::PtyWrite("\x1b[?6c".to_string()));
        assert!(should_drop_event(EVENT_QUEUE_HARD_CAP, &reply));
        assert!(should_drop_event(EVENT_QUEUE_HARD_CAP + 1, &reply));
    }

    #[test]
    fn reply_bearing_events_survive_below_hard_cap() {
        let reply = RuntimeEvent::Alacritty(AlacEvent::PtyWrite("\x1b[?6c".to_string()));
        // Even past the soft cap, a reply stays queued for a responsive host
        // until the absolute ceiling is reached.
        assert!(!should_drop_event(0, &reply));
        assert!(!should_drop_event(EVENT_QUEUE_SOFT_CAP, &reply));
        assert!(!should_drop_event(EVENT_QUEUE_HARD_CAP - 1, &reply));
    }

    #[test]
    fn cosmetic_events_shed_at_soft_cap() {
        // Bell is latest-wins/cosmetic: shed once the soft cap is hit, kept below.
        let bell = RuntimeEvent::Alacritty(AlacEvent::Bell);
        assert!(!should_drop_event(EVENT_QUEUE_SOFT_CAP - 1, &bell));
        assert!(should_drop_event(EVENT_QUEUE_SOFT_CAP, &bell));
    }

    fn test_terminal_size() -> TerminalSize {
        TerminalSize {
            cols: 32,
            rows: 4,
            cell_width: 9.0,
            cell_height: 18.0,
        }
    }

    fn cursor_after_bytes(input: &[u8]) -> (usize, i32) {
        let size = test_terminal_size();
        let mut term: Term<VoidListener> = Term::new(TermConfig::default(), &size, VoidListener);
        let mut parser: ansi::Processor = ansi::Processor::new();
        parser.advance(&mut term, input);
        let point = term.grid().cursor.point;
        (point.column.0, point.line.0)
    }

    fn term_after_bytes(input: &[u8]) -> Term<VoidListener> {
        let size = test_terminal_size();
        let mut term: Term<VoidListener> = Term::new(TermConfig::default(), &size, VoidListener);
        let mut parser: ansi::Processor = ansi::Processor::new();
        parser.advance(&mut term, input);
        term
    }

    #[test]
    fn search_term_buffer_includes_scrollback_rows() {
        let size = TerminalSize {
            cols: 16,
            rows: 2,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        let config = TermConfig {
            scrolling_history: 8,
            ..TermConfig::default()
        };
        let mut term: Term<VoidListener> = Term::new(config, &size, VoidListener);
        let mut parser: ansi::Processor = ansi::Processor::new();
        parser.advance(&mut term, b"alpha\r\nbeta\r\ngamma");

        let matches = search_term_buffer(&term, "alpha", TermySearchOptions::default());

        assert_eq!(term.grid().history_size(), 1);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].row, 0);
        assert_eq!(matches[0].start_col, 0);
        assert_eq!(matches[0].end_col, 4);
    }

    fn cursor_state_after_bytes(
        input: &[u8],
        runtime_config: TerminalRuntimeConfig,
    ) -> Option<TerminalCursorState> {
        let terminal = Terminal::new_display(test_terminal_size(), Some(&runtime_config));
        terminal.feed_output(input);
        terminal.cursor_state()
    }

    fn cursor_position_after_bytes(input: &[u8]) -> (usize, usize) {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.feed_output(input);
        terminal.cursor_position()
    }

    fn mouse_mode_after_bytes(input: &[u8]) -> crate::mouse_protocol::TerminalMouseMode {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.feed_output(input);
        terminal.mouse_mode()
    }

    fn keystroke(key: &str, modifiers: Modifiers) -> Keystroke {
        Keystroke {
            modifiers,
            key: key.to_string(),
            key_char: None,
        }
    }

    fn press_mode() -> TerminalKeyboardMode {
        TerminalKeyboardMode::default()
    }

    fn keyboard_mode(flags: TermMode) -> TerminalKeyboardMode {
        backend::keyboard_mode(flags)
    }

    #[derive(Default)]
    struct RecordingReplyHost {
        clipboard_text: Option<String>,
        requested_targets: Vec<TerminalClipboardTarget>,
        kitty_reads: Vec<TerminalClipboardReadRequest>,
        kitty_writes: Vec<TerminalClipboardWriteRequest>,
        protocol_replies: Vec<u8>,
    }

    impl TerminalReplyHost for RecordingReplyHost {
        fn load_clipboard(&mut self, target: TerminalClipboardTarget) -> Option<String> {
            self.requested_targets.push(target);
            self.clipboard_text.clone()
        }

        fn protocol_reply(&mut self, bytes: &[u8]) {
            self.protocol_replies.extend_from_slice(bytes);
        }

        fn read_clipboard(
            &mut self,
            request: TerminalClipboardReadRequest,
        ) -> TerminalClipboardReadResult {
            self.kitty_reads.push(request);
            TerminalClipboardReadResult::Success {
                available_formats: vec!["text/plain".to_string(), "image/png".to_string()],
                contents: Vec::new(),
                remember_permission: false,
            }
        }

        fn write_clipboard(
            &mut self,
            request: TerminalClipboardWriteRequest,
        ) -> TerminalClipboardWriteResult {
            self.kitty_writes.push(request);
            TerminalClipboardWriteResult::Success {
                remember_permission: false,
            }
        }
    }

    fn assert_kitty_clipboard_runtime(terminal: &Terminal) {
        terminal.feed_output(
            b"\x1b[?5522h\
              \x1b]5522;type=read:id=list;Lg==\x1b\\\
              \x1b]5522;type=write:id=write\x1b\\\
              \x1b]5522;type=walias:mime=dGV4dC9wbGFpbg==;dGV4dC91dGY4\x1b\\\
              \x1b]5522;type=wdata:mime=dGV4dC9wbGFpbg==;aGVsbG8=\x1b\\\
              \x1b]5522;type=wdata;\x1b\\",
        );

        let mut host = RecordingReplyHost::default();
        let (_, has_more) = terminal.drain_events(&mut host);

        assert!(!has_more);
        assert!(terminal.kitty_clipboard_paste_events_enabled());
        assert_eq!(host.kitty_reads.len(), 1);
        assert!(host.kitty_reads[0].list_available);
        assert_eq!(host.kitty_writes.len(), 1);
        assert_eq!(host.kitty_writes[0].contents.len(), 2);
        assert_eq!(host.kitty_writes[0].contents[0].mime_type, "text/utf8");
        assert_eq!(host.kitty_writes[0].contents[1].data, b"hello");
        let replies = String::from_utf8_lossy(&host.protocol_replies);
        assert!(replies.contains("type=read:status=OK:id=list"));
        assert!(replies.contains("type=read:status=DONE:id=list"));
        assert!(replies.contains("type=write:status=DONE:id=write"));

        host.protocol_replies.clear();
        assert!(terminal.send_kitty_clipboard_paste_event(
            crate::TerminalClipboardLocation::Clipboard,
            &["text/plain".to_string()],
        ));
        let (_, has_more) = terminal.drain_events(&mut host);
        assert!(!has_more);
        let replies = String::from_utf8_lossy(&host.protocol_replies);
        assert!(replies.contains("type=read:status=OK"));
        assert!(replies.contains("type=read:status=DONE"));
    }

    #[test]
    fn kitty_clipboard_routes_through_both_core_backends() {
        assert_kitty_clipboard_runtime(&Terminal::new_display(test_terminal_size(), None));
        assert_kitty_clipboard_runtime(&Terminal::new_alacritty_display_for_test(
            test_terminal_size(),
            None,
        ));
    }

    #[test]
    fn terminal_size_dimensions_saturate_bottommost_line_for_zero_rows() {
        let size = TerminalSize {
            cols: 0,
            rows: 0,
            cell_width: 9.0,
            cell_height: 18.0,
        };

        assert_eq!(size.last_column().0, 0);
        assert_eq!(size.bottommost_line().0, 0);
    }

    #[test]
    fn normalize_working_directory_candidate_preserves_relative_paths() {
        assert_eq!(
            normalize_working_directory_candidate(Some(" crates/cli ")).as_deref(),
            Some("crates/cli")
        );
    }

    #[test]
    fn normalize_working_directory_candidate_rejects_control_characters() {
        assert_eq!(
            normalize_working_directory_candidate(Some("/tmp/project\nrun-shell")),
            None
        );
    }

    #[test]
    fn resolve_launch_working_directory_falls_back_when_configured_path_is_invalid() {
        let fallback = std::env::current_dir().expect("current dir");
        let resolved = resolve_launch_working_directory(
            Some("/definitely/not/a/real/termy/path"),
            WorkingDirFallback::Process,
        )
        .expect("fallback path");
        assert_eq!(resolved, fallback);
    }

    #[test]
    fn normalize_working_directory_candidate_expands_home_directory() {
        let expected = user_home_dir()
            .expect("home dir")
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            normalize_working_directory_candidate(Some("~")).as_deref(),
            Some(expected.as_str())
        );
    }

    #[test]
    fn drain_runtime_events_replays_replies_and_collects_runtime_events() {
        let (events_tx, events_rx) = unbounded();
        events_tx
            .send(RuntimeEvent::Alacritty(
                alacritty_terminal::event::Event::PtyWrite("\x1b[?6c".to_string()),
            ))
            .unwrap();
        events_tx
            .send(RuntimeEvent::Alacritty(
                alacritty_terminal::event::Event::TextAreaSizeRequest(Arc::new(|window_size| {
                    format!("size:{}x{}", window_size.num_cols, window_size.num_lines)
                })),
            ))
            .unwrap();
        events_tx
            .send(RuntimeEvent::Alacritty(
                alacritty_terminal::event::Event::ClipboardLoad(
                    ClipboardType::Selection,
                    Arc::new(|text| format!("clip:{text}")),
                ),
            ))
            .unwrap();
        events_tx
            .send(RuntimeEvent::Alacritty(
                alacritty_terminal::event::Event::ColorRequest(
                    NamedColor::Foreground as usize,
                    Arc::new(|color| format!("fg:{:02x}{:02x}{:02x}", color.r, color.g, color.b)),
                ),
            ))
            .unwrap();
        events_tx
            .send(RuntimeEvent::Alacritty(
                alacritty_terminal::event::Event::Wakeup,
            ))
            .unwrap();
        events_tx
            .send(RuntimeEvent::Alacritty(
                alacritty_terminal::event::Event::Title("shell title".to_string()),
            ))
            .unwrap();
        events_tx
            .send(RuntimeEvent::Alacritty(
                alacritty_terminal::event::Event::ClipboardStore(
                    ClipboardType::Clipboard,
                    "stored text".to_string(),
                ),
            ))
            .unwrap();
        events_tx
            .send(RuntimeEvent::Alacritty(
                alacritty_terminal::event::Event::Exit,
            ))
            .unwrap();
        drop(events_tx);

        let term = FairMutex::new(term_after_bytes(b"\x1b]10;#123456\x07"));
        let mut reply_host = RecordingReplyHost {
            clipboard_text: Some("payload".to_string()),
            requested_targets: Vec::new(),
            ..RecordingReplyHost::default()
        };
        let mut replies = Vec::new();
        let kitty_clipboard = FairMutex::new(crate::KittyClipboardHostState::new());

        let first_event = events_rx.try_recv().expect("queued runtime event");
        let (events, _has_more) = drain_runtime_events(
            first_event,
            &events_rx,
            test_terminal_size(),
            &term,
            TerminalQueryColors::default(),
            &kitty_clipboard,
            &mut reply_host,
            |response| {
                replies.push(String::from_utf8(response.to_vec()).unwrap());
                true
            },
        );

        assert_eq!(
            replies,
            vec![
                "\x1b[?6c".to_string(),
                "size:32x4".to_string(),
                "clip:payload".to_string(),
                "fg:123456".to_string(),
            ]
        );
        assert_eq!(
            reply_host.requested_targets,
            vec![TerminalClipboardTarget::Selection]
        );
        assert!(matches!(
            events.as_slice(),
            [
                TerminalEvent::Wakeup,
                TerminalEvent::Title(title),
                TerminalEvent::ClipboardStore(text),
                TerminalEvent::Exit,
            ] if title == "shell title" && text == "stored text"
        ));
    }

    #[test]
    fn drain_runtime_events_includes_custom_osc_progress_events() {
        let (events_tx, events_rx) = unbounded();
        events_tx
            .send(RuntimeEvent::Terminal(terminal_event_from_osc(
                crate::osc_intercept::OscEvent::Progress(
                    crate::shell_integration::ProgressState::Indeterminate,
                ),
            )))
            .unwrap();
        drop(events_tx);

        let term = FairMutex::new(term_after_bytes(b""));
        let mut reply_host = RecordingReplyHost::default();
        let kitty_clipboard = FairMutex::new(crate::KittyClipboardHostState::new());

        let first_event = events_rx.try_recv().expect("queued runtime event");
        let (events, has_more) = drain_runtime_events(
            first_event,
            &events_rx,
            test_terminal_size(),
            &term,
            TerminalQueryColors::default(),
            &kitty_clipboard,
            &mut reply_host,
            |_| true,
        );

        assert!(!has_more);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            TerminalEvent::Progress(crate::shell_integration::ProgressState::Indeterminate)
        ));
    }

    #[test]
    fn drain_runtime_events_coalesces_consecutive_wakeups() {
        let (events_tx, events_rx) = unbounded();
        for _ in 0..128 {
            events_tx
                .send(RuntimeEvent::Alacritty(
                    alacritty_terminal::event::Event::Wakeup,
                ))
                .unwrap();
        }
        events_tx
            .send(RuntimeEvent::Alacritty(
                alacritty_terminal::event::Event::Title("ready".to_string()),
            ))
            .unwrap();
        drop(events_tx);

        let term = FairMutex::new(term_after_bytes(b""));
        let mut reply_host = RecordingReplyHost::default();
        let kitty_clipboard = FairMutex::new(crate::KittyClipboardHostState::new());

        let first_event = events_rx.try_recv().expect("queued runtime event");
        let (events, has_more) = drain_runtime_events(
            first_event,
            &events_rx,
            test_terminal_size(),
            &term,
            TerminalQueryColors::default(),
            &kitty_clipboard,
            &mut reply_host,
            |_| true,
        );

        assert!(!has_more);
        assert!(matches!(
            events.as_slice(),
            [TerminalEvent::Wakeup, TerminalEvent::Title(title)] if title == "ready"
        ));
    }

    #[test]
    fn json_event_listener_coalesces_queued_wakeups_until_reset() {
        let (events_tx, events_rx) = unbounded();
        let (wake_tx, wake_rx) = unbounded();
        let listener = JsonEventListener::new(events_tx, Some(wake_tx));

        listener.send_event(alacritty_terminal::event::Event::Wakeup);
        listener.send_event(alacritty_terminal::event::Event::Wakeup);
        listener.send_event(alacritty_terminal::event::Event::Wakeup);

        let events: Vec<_> = events_rx.try_iter().collect();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            RuntimeEvent::Alacritty(alacritty_terminal::event::Event::Wakeup)
        ));
        assert_eq!(wake_rx.try_iter().count(), 1);

        listener.reset_wakeup_queued();
        listener.send_event(alacritty_terminal::event::Event::Wakeup);

        let events: Vec<_> = events_rx.try_iter().collect();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            RuntimeEvent::Alacritty(alacritty_terminal::event::Event::Wakeup)
        ));
        assert_eq!(wake_rx.try_iter().count(), 1);
    }

    #[test]
    fn json_event_listener_routes_coalesced_wakeups_through_notifier() {
        let (events_tx, _events_rx) = unbounded();
        let notifications = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let notification_count = notifications.clone();
        let listener = JsonEventListener::new_with_wakeup_notifier(
            events_tx,
            Some(TerminalWakeupNotifier::new(move || {
                notification_count.fetch_add(1, Ordering::Relaxed);
            })),
        );

        listener.send_event(alacritty_terminal::event::Event::Wakeup);
        listener.send_event(alacritty_terminal::event::Event::Wakeup);
        assert_eq!(notifications.load(Ordering::Relaxed), 1);

        listener.reset_wakeup_queued();
        listener.send_event(alacritty_terminal::event::Event::Wakeup);
        assert_eq!(notifications.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn empty_terminal_drain_preserves_a_coalesced_wakeup() {
        let terminal = Terminal::new_alacritty_display_for_test(test_terminal_size(), None);
        terminal.backend.send_wakeup_for_test();
        assert!(matches!(
            terminal.backend.try_recv_event_for_test(),
            Some(RuntimeEvent::Alacritty(
                alacritty_terminal::event::Event::Wakeup
            ))
        ));

        // The queued event was already consumed, but another terminal update
        // can still be coalesced into the listener flag before the host drains.
        terminal.backend.send_wakeup_for_test();
        assert!(terminal.backend.event_queue_is_empty_for_test());

        let mut reply_host = RecordingReplyHost::default();
        let (events, has_more) = terminal.drain_events(&mut reply_host);
        assert!(matches!(events.as_slice(), [TerminalEvent::Wakeup]));
        assert!(!has_more);

        terminal.backend.send_wakeup_for_test();
        assert!(matches!(
            terminal.backend.try_recv_event_for_test(),
            Some(RuntimeEvent::Alacritty(
                alacritty_terminal::event::Event::Wakeup
            ))
        ));
    }

    #[test]
    fn json_event_listener_can_suppress_plain_wakeup_signals() {
        let (events_tx, events_rx) = unbounded();
        let (wake_tx, wake_rx) = unbounded();
        let listener = JsonEventListener::new(events_tx, Some(wake_tx));
        listener.set_wakeup_enabled(false);

        listener.send_event(alacritty_terminal::event::Event::Wakeup);
        listener.send_event(alacritty_terminal::event::Event::Wakeup);

        let events: Vec<_> = events_rx.try_iter().collect();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            RuntimeEvent::Alacritty(alacritty_terminal::event::Event::Wakeup)
        ));
        assert_eq!(wake_rx.try_iter().count(), 0);
    }

    #[test]
    fn json_event_listener_resignals_pending_wakeup_when_reenabled() {
        let (events_tx, events_rx) = unbounded();
        let (wake_tx, wake_rx) = unbounded();
        let listener = JsonEventListener::new(events_tx, Some(wake_tx));
        listener.set_wakeup_enabled(false);

        listener.send_event(alacritty_terminal::event::Event::Wakeup);
        listener.send_event(alacritty_terminal::event::Event::Wakeup);

        assert_eq!(events_rx.len(), 1);
        assert_eq!(wake_rx.try_iter().count(), 0);

        listener.set_wakeup_enabled(true);
        listener.set_wakeup_enabled(true);

        assert_eq!(wake_rx.try_iter().count(), 1);
    }

    #[test]
    fn json_event_listener_resignals_undrained_wakeup_after_visibility_cycle() {
        let (events_tx, events_rx) = unbounded();
        let (wake_tx, wake_rx) = unbounded();
        let listener = JsonEventListener::new(events_tx, Some(wake_tx));

        listener.send_event(alacritty_terminal::event::Event::Wakeup);
        assert_eq!(events_rx.len(), 1);
        assert_eq!(wake_rx.try_iter().count(), 1);

        listener.set_wakeup_enabled(false);
        listener.set_wakeup_enabled(true);
        listener.set_wakeup_enabled(true);

        assert_eq!(events_rx.len(), 1);
        assert_eq!(wake_rx.try_iter().count(), 1);
    }

    #[test]
    fn json_event_listener_wakes_when_output_arrives_after_reenable() {
        let (events_tx, events_rx) = unbounded();
        let (wake_tx, wake_rx) = unbounded();
        let listener = JsonEventListener::new(events_tx, Some(wake_tx));
        listener.set_wakeup_enabled(false);
        listener.set_wakeup_enabled(true);

        assert_eq!(wake_rx.try_iter().count(), 0);

        listener.send_event(alacritty_terminal::event::Event::Wakeup);

        assert_eq!(events_rx.len(), 1);
        assert_eq!(wake_rx.try_iter().count(), 1);
    }

    #[test]
    fn json_event_listener_still_wakes_for_metadata_when_wakeups_are_suppressed() {
        let (events_tx, _events_rx) = unbounded();
        let (wake_tx, wake_rx) = unbounded();
        let listener = JsonEventListener::new(events_tx, Some(wake_tx));
        listener.set_wakeup_enabled(false);

        listener.send_event(alacritty_terminal::event::Event::Title("ready".to_string()));
        listener.send_terminal_event(TerminalEvent::Progress(
            crate::shell_integration::ProgressState::Indeterminate,
        ));

        assert_eq!(wake_rx.try_iter().count(), 2);
    }

    #[test]
    fn backlogged_event_queue_sheds_state_refresh_events_only() {
        let (events_tx, events_rx) = unbounded();
        let listener = JsonEventListener::new(events_tx, None);

        for _ in 0..(EVENT_QUEUE_SOFT_CAP + 100) {
            listener.send_event(alacritty_terminal::event::Event::Title("t".to_string()));
        }
        assert_eq!(events_rx.len(), EVENT_QUEUE_SOFT_CAP);

        // OSC-derived terminal events shed under backlog too.
        listener.send_terminal_event(TerminalEvent::Bell);
        assert_eq!(events_rx.len(), EVENT_QUEUE_SOFT_CAP);

        // Protocol-critical events are queued even while backlogged.
        listener.send_event(alacritty_terminal::event::Event::Exit);
        listener.send_event(alacritty_terminal::event::Event::ClipboardStore(
            ClipboardType::Clipboard,
            "x".to_string(),
        ));
        assert_eq!(events_rx.len(), EVENT_QUEUE_SOFT_CAP + 2);
    }

    #[test]
    fn mouse_mode_detects_click_reporting() {
        let mode = mouse_mode_after_bytes(b"\x1b[?1000h");
        assert!(mode.enabled);
        assert!(mode.report_click);
        assert!(!mode.report_drag);
        assert!(!mode.report_motion);
    }

    #[test]
    fn mouse_mode_detects_drag_reporting() {
        let mode = mouse_mode_after_bytes(b"\x1b[?1002h");
        assert!(mode.enabled);
        assert!(mode.report_drag);
        assert!(!mode.report_motion);
    }

    #[test]
    fn mouse_mode_detects_motion_reporting() {
        let mode = mouse_mode_after_bytes(b"\x1b[?1003h");
        assert!(mode.enabled);
        assert!(mode.report_motion);
    }

    #[test]
    fn mouse_mode_detects_sgr_encoding() {
        let mode = mouse_mode_after_bytes(b"\x1b[?1006h");
        assert!(mode.sgr_encoding);
    }

    #[test]
    fn mouse_mode_detects_utf8_reporting() {
        let mode = mouse_mode_after_bytes(b"\x1b[?1005h");
        assert!(mode.utf8_encoding);
    }

    #[test]
    fn terminal_damage_snapshot_is_full_for_new_terminal() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        assert!(matches!(
            terminal.take_damage_snapshot(),
            TerminalDamageSnapshot::Full
        ));
    }

    #[test]
    fn terminal_damage_snapshot_resets_damage_after_read() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        let _ = terminal.take_damage_snapshot();
        let second = terminal.take_damage_snapshot();
        let third = terminal.take_damage_snapshot();
        assert!(matches!(second, TerminalDamageSnapshot::Partial(_)));
        assert_eq!(second, third);
    }

    #[test]
    fn terminal_damage_snapshot_returns_partial_spans_for_output() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        let _ = terminal.take_damage_snapshot();
        terminal.feed_output(b"abc");
        assert!(matches!(
            terminal.take_damage_snapshot(),
            TerminalDamageSnapshot::Partial(spans) if !spans.is_empty()
        ));
    }

    #[test]
    fn terminal_damage_snapshot_while_scrolled_returns_empty_partial_without_damage() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        let _ = terminal.take_damage_snapshot();
        terminal.feed_output(b"1\n2\n3\n4\n5\n6\n");
        let _ = terminal.take_damage_snapshot();

        assert!(terminal.scroll_display(1));
        assert!(terminal.scroll_state().0 > 0);

        assert!(matches!(
            terminal.take_damage_snapshot(),
            TerminalDamageSnapshot::Full
        ));
        assert_eq!(
            terminal.take_damage_snapshot(),
            TerminalDamageSnapshot::Partial(Vec::new())
        );
    }

    #[test]
    fn terminal_damage_snapshot_while_scrolled_maps_damage_to_viewport_rows() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        let _ = terminal.take_damage_snapshot();
        terminal.feed_output(b"1\n2\n3\n4\n5\n6\n");
        let _ = terminal.take_damage_snapshot();

        assert!(terminal.scroll_display(1));
        let _ = terminal.take_damage_snapshot();
        let _ = terminal.take_damage_snapshot();

        terminal.feed_output(b"\x1b[1;1H");
        match terminal.take_damage_snapshot() {
            TerminalDamageSnapshot::Partial(spans) => {
                assert!(spans.iter().any(|span| span.row == 1), "spans: {spans:?}");
                assert!(spans.iter().all(|span| span.row < 4), "spans: {spans:?}");
            }
            TerminalDamageSnapshot::Full => {
                panic!("visible damage while scrolled should stay partial")
            }
        }
    }

    #[test]
    fn terminal_damage_snapshot_while_scrolled_drops_damage_below_viewport() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        let _ = terminal.take_damage_snapshot();
        terminal.feed_output(b"1\n2\n3\n4\n5\n6\n");
        let _ = terminal.take_damage_snapshot();

        assert!(terminal.scroll_display(3));
        let _ = terminal.take_damage_snapshot();
        let _ = terminal.take_damage_snapshot();

        terminal.feed_output(b"x");
        match terminal.take_damage_snapshot() {
            TerminalDamageSnapshot::Partial(spans) => {
                assert!(spans.iter().all(|span| span.row < 4), "spans: {spans:?}");
            }
            TerminalDamageSnapshot::Full => {
                panic!("invisible damage while scrolled should stay partial")
            }
        }
    }

    #[test]
    fn rich_render_read_preserves_text_colors_attributes_and_metadata() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.feed_output(
            "\x1b[38;2;1;2;3;48;5;4;4:3;58:2::7:8:9;1;2;3;7;8;9me\u{301}\x1b[0m 界".as_bytes(),
        );

        let read = terminal.render_read(true);
        assert_eq!((read.metadata.cols, read.metadata.rows), (32, 4));
        assert_eq!(read.metadata.generation, read.update.generation);
        assert_eq!(read.metadata.palette_revision, read.update.palette_revision);
        assert_eq!(read.metadata.palette_revision, read.palette.revision);
        assert!(matches!(read.update.damage, TerminalDamageSnapshot::Full));
        assert!(read.update.scrolls.is_empty());

        let cell = &read.cells[0];
        assert_eq!(cell.text, "e\u{301}");
        assert_eq!(
            cell.foreground,
            crate::TerminalRenderColor::Rgb(crate::TerminalColor { r: 1, g: 2, b: 3 })
        );
        assert_eq!(cell.background, crate::TerminalRenderColor::Indexed(4));
        assert_eq!(
            cell.underline_color,
            Some(crate::TerminalRenderColor::Rgb(crate::TerminalColor {
                r: 7,
                g: 8,
                b: 9,
            }))
        );
        assert_eq!(cell.underline_style, crate::TerminalUnderlineStyle::Curly);
        assert!(cell.bold);
        assert!(cell.dim);
        assert!(cell.italic);
        assert!(cell.inverse);
        assert!(cell.hidden);
        assert!(cell.strikethrough);
        assert!(read.cells.iter().any(|cell| cell.wide_character_spacer));
    }

    #[test]
    fn viewport_visitors_allow_reentrant_terminal_reads() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.feed_output(b"abc");

        let mut viewport_reentered = false;
        terminal.visit_viewport_cells(|_, _, _, _| {
            if !viewport_reentered {
                viewport_reentered = true;
                assert_eq!(terminal.line_bounds().1, 3);
            }
        });
        assert!(viewport_reentered);

        let update = terminal.take_render_damage_snapshot();
        let spans = match &update.damage {
            TerminalDamageSnapshot::Partial(spans) if !spans.is_empty() => spans.clone(),
            TerminalDamageSnapshot::Full | TerminalDamageSnapshot::Partial(_) => {
                vec![crate::TerminalDirtySpan {
                    row: 0,
                    left_col: 0,
                    right_col: 0,
                }]
            }
        };
        let mut range_reentered = false;
        assert!(terminal.visit_viewport_ranges_at_generation(
            update.generation,
            &spans,
            |_, _, _, _, _| {
                if !range_reentered {
                    range_reentered = true;
                    let _ = terminal.palette();
                }
            },
        ));
        assert!(range_reentered);
    }

    #[test]
    fn rich_line_visitor_streams_cells_in_buffer_order() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.feed_output(b"abc\r\ndef");

        let mut visited = Vec::new();
        let bounds = terminal.visit_line_cells(0, 1, |range, line, col, cell| {
            if col < 3 {
                visited.push((range, line, col, cell.text.to_string()));
            }
        });

        assert_eq!(bounds, (0, 3, 32));
        assert_eq!(
            visited,
            vec![
                ((0, 3, 32), 0, 0, "a".to_string()),
                ((0, 3, 32), 0, 1, "b".to_string()),
                ((0, 3, 32), 0, 2, "c".to_string()),
                ((0, 3, 32), 1, 0, "d".to_string()),
                ((0, 3, 32), 1, 1, "e".to_string()),
                ((0, 3, 32), 1, 2, "f".to_string()),
            ]
        );
    }

    #[test]
    fn rich_render_cell_keeps_common_text_inline() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.feed_output(b"plain");

        let read = terminal.render_read(true);
        assert!(read.cells.iter().all(|cell| !cell.text.is_heap_allocated()));

        terminal.feed_output("\rcombined e\u{301}".as_bytes());
        let read = terminal.render_read(true);
        assert!(
            read.cells
                .iter()
                .filter(|cell| !cell.text.is_empty())
                .all(|cell| !cell.text.is_heap_allocated())
        );
    }

    #[test]
    fn rich_render_read_reports_wrap_generation_palette_and_partial_cells() {
        let terminal = Terminal::new_display(
            TerminalSize {
                cols: 4,
                rows: 2,
                ..test_terminal_size()
            },
            None,
        );
        let initial = terminal.render_read(true);
        let _ = terminal.take_render_damage_snapshot();
        terminal.feed_output(b"abcde");

        let update = terminal.take_render_damage_snapshot();
        assert!(update.generation > initial.metadata.generation);
        assert!(matches!(
            update.damage,
            TerminalDamageSnapshot::Partial(ref spans) if !spans.is_empty()
        ));
        let spans = match &update.damage {
            TerminalDamageSnapshot::Partial(spans) => spans,
            TerminalDamageSnapshot::Full => unreachable!(),
        };
        let mut visited = Vec::new();
        assert!(terminal.visit_viewport_ranges_at_generation(
            update.generation,
            spans,
            |row, display_offset, line, col, cell| {
                visited.push((row, display_offset, line, col, cell.text.clone()));
            },
        ));
        assert!(!visited.is_empty());

        terminal.feed_output(b"\x1b]4;1;#123456\x07");
        let read = terminal.render_read(true);
        assert!(read.cells[3].line_wrapped);
        assert_eq!(
            read.palette.indexed[1],
            Some(crate::TerminalColor {
                r: 0x12,
                g: 0x34,
                b: 0x56,
            })
        );
        assert!(read.metadata.palette_revision > initial.metadata.palette_revision);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_secondary_shortcuts_map_to_line_editing_sequences() {
        let secondary = Modifiers {
            platform: true,
            ..Default::default()
        };

        assert_eq!(
            keystroke_to_input(
                &keystroke("left", secondary),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x01".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("home", secondary),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x01".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("right", secondary),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x05".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("end", secondary),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x05".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("backspace", secondary),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x15".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("delete", secondary),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x0b".to_vec())
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_alt_shortcuts_map_to_word_editing_sequences() {
        let alt = Modifiers {
            alt: true,
            ..Default::default()
        };

        assert_eq!(
            keystroke_to_input(
                &keystroke("left", alt),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x1bb".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("right", alt),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x1bf".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("backspace", alt),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x1b\x7f".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("delete", alt),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x1bd".to_vec())
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn non_macos_secondary_shortcuts_map_to_native_word_sequences() {
        let secondary = Modifiers {
            control: true,
            ..Default::default()
        };

        assert_eq!(
            keystroke_to_input(
                &keystroke("left", secondary),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x1bb".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("right", secondary),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x1bf".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("backspace", secondary),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x17".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("delete", secondary),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x1bd".to_vec())
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn non_macos_secondary_shortcuts_do_not_remap_in_alternate_screen() {
        let secondary = Modifiers {
            control: true,
            ..Default::default()
        };

        assert_eq!(
            keystroke_to_input(
                &keystroke("left", secondary),
                TerminalKeyEventKind::Press,
                press_mode(),
                false,
            ),
            Some(b"\x1b[D".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("right", secondary),
                TerminalKeyEventKind::Press,
                press_mode(),
                false,
            ),
            Some(b"\x1b[C".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("backspace", secondary),
                TerminalKeyEventKind::Press,
                press_mode(),
                false,
            ),
            Some(vec![0x7f])
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("delete", secondary),
                TerminalKeyEventKind::Press,
                press_mode(),
                false,
            ),
            Some(b"\x1b[3~".to_vec())
        );
    }

    #[test]
    fn plain_special_key_sequences_remain_unchanged() {
        let none = Modifiers::default();

        assert_eq!(
            keystroke_to_input(
                &keystroke("backspace", none),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(vec![0x7f])
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("delete", none),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x1b[3~".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("left", none),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x1b[D".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("right", none),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x1b[C".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("home", none),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x1b[H".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("end", none),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x1b[F".to_vec())
        );
    }

    #[test]
    fn control_letter_mappings_remain_unchanged() {
        let control = Modifiers {
            control: true,
            ..Default::default()
        };

        assert_eq!(
            keystroke_to_input(
                &keystroke("a", control),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(vec![0x01])
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("c", control),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(vec![0x03])
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("z", control),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(vec![0x1a])
        );
    }

    #[test]
    fn keyboard_mode_detects_report_all_and_event_types() {
        let mode = keyboard_mode(TermMode::REPORT_ALL_KEYS_AS_ESC | TermMode::REPORT_EVENT_TYPES);
        assert!(mode.report_all_keys_as_esc());
        assert!(mode.report_event_types());
        assert!(mode.enhanced_reporting_active());
    }

    #[test]
    fn keyboard_mode_augment_only_flags_do_not_activate_enhanced_reporting() {
        let mode =
            keyboard_mode(TermMode::REPORT_ALTERNATE_KEYS | TermMode::REPORT_ASSOCIATED_TEXT);
        assert!(mode.report_alternate_keys());
        assert!(mode.report_associated_text());
        assert!(!mode.enhanced_reporting_active());
    }

    #[test]
    fn env_overrides_set_term_by_default() {
        let env = terminal_environment_overrides(None, &TerminalRuntimeConfig::default());
        assert_eq!(env.get("TERM").map(String::as_str), Some(DEFAULT_TERM));
    }

    #[test]
    fn env_overrides_advertise_ghostty_progress_capability() {
        let env = terminal_environment_overrides(None, &TerminalRuntimeConfig::default());
        assert_eq!(
            env.get("TERM_PROGRAM").map(String::as_str),
            Some(GHOSTTY_COMPAT_TERM_PROGRAM)
        );
        assert_eq!(
            env.get("TERM_PROGRAM_VERSION").map(String::as_str),
            Some(GHOSTTY_COMPAT_TERM_PROGRAM_VERSION)
        );
        assert_eq!(
            env.get("TERMY_TERM_PROGRAM").map(String::as_str),
            Some(TERMY_TERM_PROGRAM)
        );
    }

    #[test]
    fn env_overrides_allow_disabling_colorterm() {
        let config = TerminalRuntimeConfig {
            colorterm: None,
            ..TerminalRuntimeConfig::default()
        };
        let env = terminal_environment_overrides(None, &config);
        assert!(!env.contains_key("COLORTERM"));
    }

    #[test]
    fn env_overrides_merge_host_environment_last() {
        let config = TerminalRuntimeConfig {
            environment: HashMap::from([
                ("CMUX_SOCKET_PATH".to_string(), "/tmp/cmux.sock".to_string()),
                ("TERM_PROGRAM".to_string(), "cmux".to_string()),
            ]),
            ..TerminalRuntimeConfig::default()
        };
        let env = terminal_environment_overrides(None, &config);
        assert_eq!(
            env.get("CMUX_SOCKET_PATH").map(String::as_str),
            Some("/tmp/cmux.sock")
        );
        assert_eq!(env.get("TERM_PROGRAM").map(String::as_str), Some("cmux"));
    }

    #[test]
    fn explicit_shell_path_wins() {
        assert_eq!(resolve_shell_path(Some("/bin/custom")), "/bin/custom");
        let config = TerminalRuntimeConfig {
            shell: Some("/bin/custom".to_string()),
            windows_shell: WindowsShell::PowerShell,
            ..TerminalRuntimeConfig::default()
        };
        let launch = resolve_terminal_launch(&config, None).expect("configured shell launch");
        assert_eq!(launch.program, "/bin/custom");
        assert_eq!(config.resolved_shell_program(), "/bin/custom");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn configured_unix_shell_keeps_macos_interactive_login_arguments() {
        let resolved = resolve_terminal_launch(
            &TerminalRuntimeConfig {
                shell: Some("/opt/custom/bin/zsh".to_string()),
                ..TerminalRuntimeConfig::default()
            },
            None,
        )
        .expect("configured shell launch");

        assert_eq!(resolved.program, "/opt/custom/bin/zsh");
        assert_eq!(resolved.args, ["-i", "-l"]);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn configured_unix_shell_keeps_non_macos_interactive_arguments() {
        let resolved = resolve_terminal_launch(
            &TerminalRuntimeConfig {
                shell: Some("/opt/custom/bin/zsh".to_string()),
                ..TerminalRuntimeConfig::default()
            },
            None,
        )
        .expect("configured shell launch");

        assert_eq!(resolved.program, "/opt/custom/bin/zsh");
        assert_eq!(resolved.args, ["-i"]);
    }

    #[test]
    fn typed_program_launch_keeps_arguments_out_of_the_shell() {
        let launch = TerminalLaunch::Program {
            program: "ssh".to_string(),
            args: vec![
                "-i".to_string(),
                "/tmp/key; touch /tmp/should-not-exist".to_string(),
                "--".to_string(),
                "example.com".to_string(),
            ],
        };
        let resolved = resolve_terminal_launch(&TerminalRuntimeConfig::default(), Some(&launch))
            .expect("typed launch");
        assert_eq!(resolved.program, "ssh");
        assert_eq!(
            resolved.args,
            vec![
                "-i".to_string(),
                "/tmp/key; touch /tmp/should-not-exist".to_string(),
                "--".to_string(),
                "example.com".to_string(),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn existing_startup_commands_still_use_the_unix_shell() {
        let launch = TerminalLaunch::ShellCommand("printf existing-behavior".to_string());
        let resolved = resolve_terminal_launch(&TerminalRuntimeConfig::default(), Some(&launch))
            .expect("shell command launch");
        assert_eq!(resolved.program, "/bin/sh");
        assert_eq!(
            resolved.args,
            vec!["-c".to_string(), "printf existing-behavior".to_string()]
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_shell_setting_selects_powershell() {
        let launch = default_shell_launch(&TerminalRuntimeConfig {
            windows_shell: WindowsShell::PowerShell,
            ..TerminalRuntimeConfig::default()
        });

        assert_eq!(launch.program, "powershell.exe");
        assert_eq!(launch.args, vec!["-NoLogo".to_string()]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_shell_setting_selects_powershell_core() {
        let launch = default_shell_launch(&TerminalRuntimeConfig {
            windows_shell: WindowsShell::PowerShellCore,
            ..TerminalRuntimeConfig::default()
        });

        assert_eq!(launch.program, "pwsh.exe");
        assert_eq!(launch.args, vec!["-NoLogo".to_string()]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn public_resolver_preserves_every_windows_shell_launch() {
        let cases = [
            (WindowsShell::Cmd, windows_cmd_path(), Vec::new()),
            (
                WindowsShell::PowerShell,
                "powershell.exe".to_string(),
                vec!["-NoLogo".to_string()],
            ),
            (
                WindowsShell::PowerShellCore,
                "pwsh.exe".to_string(),
                vec!["-NoLogo".to_string()],
            ),
            (
                WindowsShell::GitBash,
                windows_git_bash_path(),
                vec!["--login".to_string(), "-i".to_string()],
            ),
        ];

        for (windows_shell, program, args) in cases {
            let resolved = resolve_terminal_launch(
                &TerminalRuntimeConfig {
                    windows_shell,
                    ..TerminalRuntimeConfig::default()
                },
                None,
            )
            .expect("Windows shell launch");
            assert_eq!(resolved.program, program);
            assert_eq!(resolved.args, args);
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn public_resolver_preserves_every_windows_startup_command_launch() {
        let command = "echo startup";
        let cases = [
            (
                WindowsShell::Cmd,
                windows_cmd_path(),
                vec!["/C".to_string(), command.to_string()],
            ),
            (
                WindowsShell::PowerShell,
                "powershell.exe".to_string(),
                vec![
                    "-NoLogo".to_string(),
                    "-NoProfile".to_string(),
                    "-ExecutionPolicy".to_string(),
                    "Bypass".to_string(),
                    "-Command".to_string(),
                    command.to_string(),
                ],
            ),
            (
                WindowsShell::PowerShellCore,
                "pwsh.exe".to_string(),
                vec![
                    "-NoLogo".to_string(),
                    "-NoProfile".to_string(),
                    "-ExecutionPolicy".to_string(),
                    "Bypass".to_string(),
                    "-Command".to_string(),
                    command.to_string(),
                ],
            ),
            (
                WindowsShell::GitBash,
                windows_git_bash_path(),
                vec!["-lc".to_string(), command.to_string()],
            ),
        ];
        let launch = TerminalLaunch::ShellCommand(command.to_string());

        for (windows_shell, program, args) in cases {
            let resolved = resolve_terminal_launch(
                &TerminalRuntimeConfig {
                    windows_shell,
                    ..TerminalRuntimeConfig::default()
                },
                Some(&launch),
            )
            .expect("Windows startup command launch");
            assert_eq!(resolved.program, program);
            assert_eq!(resolved.args, args);
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn public_resolver_preserves_custom_windows_shell_startup_behavior() {
        let runtime_config = TerminalRuntimeConfig {
            shell: Some(r"C:\Tools\custom-shell.exe".to_string()),
            windows_shell: WindowsShell::PowerShellCore,
            ..TerminalRuntimeConfig::default()
        };

        let interactive =
            resolve_terminal_launch(&runtime_config, None).expect("custom Windows shell launch");
        assert_eq!(interactive.program, r"C:\Tools\custom-shell.exe");
        assert!(interactive.args.is_empty());

        let startup = TerminalLaunch::ShellCommand("echo startup".to_string());
        let command = resolve_terminal_launch(&runtime_config, Some(&startup))
            .expect("custom Windows startup command launch");
        assert_eq!(command.program, "cmd.exe");
        assert_eq!(command.args, ["/C", "echo startup"]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_startup_commands_use_selected_shell() {
        let launch = super::windows_startup_command_shell(WindowsShell::GitBash, "echo hi");

        assert_eq!(launch.args, vec!["-lc".to_string(), "echo hi".to_string()]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn shell_program_with_spaces_is_quoted() {
        let path = r"C:\Program Files\PowerShell\7\pwsh.exe";
        let quoted = quote_shell_program_if_needed(path);
        assert_eq!(quoted, r#""C:\Program Files\PowerShell\7\pwsh.exe""#);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn shell_program_without_spaces_is_unchanged() {
        let path = r"C:\Windows\System32\cmd.exe";
        assert_eq!(quote_shell_program_if_needed(path), path);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn already_quoted_shell_program_is_not_double_quoted() {
        let path = r#""C:\Program Files\PowerShell\7\pwsh.exe""#;
        assert_eq!(quote_shell_program_if_needed(path), path);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn shell_program_with_embedded_quotes_is_escaped() {
        // Defensively handle a path that (illegally on Windows) contains a
        // double-quote character alongside spaces.
        let path = "C:\\weird \\path\"\\pwsh.exe";
        let quoted = quote_shell_program_if_needed(path);
        assert_eq!(quoted, r#""C:\weird \path\"\pwsh.exe""#);
    }

    #[test]
    fn core_cursor_advance_matches_for_ascii_and_starship_glyph() {
        let ascii = cursor_after_bytes(b"> ");
        let starship = cursor_after_bytes("❯ ".as_bytes());
        assert_eq!(ascii, starship);
    }

    #[test]
    fn core_cursor_advance_ignores_ansi_sequences_for_ascii_and_starship_glyph() {
        let ascii = cursor_after_bytes(b"\x1b[1;32m>\x1b[0m ");
        let starship = cursor_after_bytes("\x1b[1;32m❯\x1b[0m ".as_bytes());
        assert_eq!(ascii, starship);
    }

    #[test]
    fn core_cursor_advance_matches_after_osc_title_with_bel_terminator() {
        let ascii = cursor_after_bytes(b"\x1b]2;termy:tab:prompt:/tmp\x07> ");
        let starship = cursor_after_bytes("\x1b]2;termy:tab:prompt:/tmp\x07❯ ".as_bytes());
        assert_eq!(ascii, starship);
    }

    #[test]
    fn core_cursor_advance_matches_after_osc_title_with_st_terminator() {
        let ascii = cursor_after_bytes(b"\x1b]2;termy:tab:prompt:/tmp\x1b\\> ");
        let starship = cursor_after_bytes("\x1b]2;termy:tab:prompt:/tmp\x1b\\❯ ".as_bytes());
        assert_eq!(ascii, starship);
    }

    #[test]
    fn cursor_state_hides_and_restores_with_terminal_visibility_sequences() {
        let hidden = cursor_state_after_bytes(b"prompt\x1b[?25l", TerminalRuntimeConfig::default());
        assert_eq!(hidden, None);

        let restored = cursor_state_after_bytes(
            b"prompt\x1b[?25l\x1b[?25h",
            TerminalRuntimeConfig::default(),
        );
        assert_eq!(
            restored,
            Some(TerminalCursorState {
                col: 6,
                row: 0,
                style: TerminalCursorStyle::Block,
            })
        );
    }

    #[test]
    fn cursor_position_remains_available_when_terminal_hides_cursor() {
        assert_eq!(cursor_position_after_bytes(b"prompt\x1b[?25l"), (6, 0));
    }

    #[test]
    fn cursor_state_maps_terminal_requested_shapes_to_supported_renderer_styles() {
        let block = cursor_state_after_bytes(
            b"\x1b[2 q",
            TerminalRuntimeConfig {
                default_cursor_style: TerminalCursorStyle::Line,
                ..TerminalRuntimeConfig::default()
            },
        );
        assert_eq!(
            block,
            Some(TerminalCursorState {
                col: 0,
                row: 0,
                style: TerminalCursorStyle::Block,
            })
        );

        let underline = cursor_state_after_bytes(b"\x1b[4 q", TerminalRuntimeConfig::default());
        assert_eq!(
            underline,
            Some(TerminalCursorState {
                col: 0,
                row: 0,
                style: TerminalCursorStyle::Line,
            })
        );

        let beam = cursor_state_after_bytes(b"\x1b[6 q", TerminalRuntimeConfig::default());
        assert_eq!(
            beam,
            Some(TerminalCursorState {
                col: 0,
                row: 0,
                style: TerminalCursorStyle::Line,
            })
        );
    }

    #[test]
    fn applying_runtime_options_preserves_default_cursor_style_when_scrollback_changes() {
        let size = test_terminal_size();
        let initial = TerminalRuntimeConfig {
            scrollback_history: 256,
            default_cursor_style: TerminalCursorStyle::Line,
            ..TerminalRuntimeConfig::default()
        };
        let mut term: Term<VoidListener> = Term::new(
            backend::term_config(initial.term_options()),
            &size,
            VoidListener,
        );

        let updated = TerminalRuntimeConfig {
            scrollback_history: 8,
            ..initial
        };
        term.set_options(backend::term_config(updated.term_options()));
        let mut parser: ansi::Processor = ansi::Processor::new();
        let output = (0..80)
            .map(|index| format!("line-{index}\r\n"))
            .collect::<String>();
        parser.advance(&mut term, output.as_bytes());

        assert_eq!(term.grid().history_size(), 8);
        assert_eq!(term.cursor_style().shape, CursorShape::Beam);
    }

    #[test]
    fn shrinking_scrollback_trims_history_and_keeps_terminal_usable() {
        let size = test_terminal_size();
        let initial = TerminalRuntimeConfig {
            scrollback_history: 256,
            ..TerminalRuntimeConfig::default()
        };
        let mut term: Term<VoidListener> = Term::new(
            backend::term_config(initial.term_options()),
            &size,
            VoidListener,
        );

        let mut parser: ansi::Processor = ansi::Processor::new();
        let output = (0..300)
            .map(|index| format!("line-{index}\r\n"))
            .collect::<String>();
        parser.advance(&mut term, output.as_bytes());
        assert_eq!(term.grid().history_size(), 256);

        // Shrink (the inactive-tab path), which must also trim the raw buffer.
        let inactive = TerminalRuntimeConfig {
            scrollback_history: 16,
            ..initial.clone()
        };
        backend::apply_term_options(&mut term, inactive.term_options());
        assert_eq!(term.grid().history_size(), 16);

        // Grow back (tab reactivated) and keep scrolling: storage must regrow.
        backend::apply_term_options(&mut term, initial.term_options());
        parser.advance(&mut term, output.as_bytes());
        assert_eq!(term.grid().history_size(), 256);
    }

    #[test]
    fn applying_runtime_options_preserves_scrollback_when_cursor_style_changes() {
        let size = test_terminal_size();
        let initial = TerminalRuntimeConfig {
            scrollback_history: 8,
            ..TerminalRuntimeConfig::default()
        };
        let mut term: Term<VoidListener> = Term::new(
            backend::term_config(initial.term_options()),
            &size,
            VoidListener,
        );

        let updated = TerminalRuntimeConfig {
            default_cursor_style: TerminalCursorStyle::Line,
            ..initial
        };
        term.set_options(backend::term_config(updated.term_options()));
        let mut parser: ansi::Processor = ansi::Processor::new();
        let output = (0..80)
            .map(|index| format!("line-{index}\r\n"))
            .collect::<String>();
        parser.advance(&mut term, output.as_bytes());

        assert_eq!(term.grid().history_size(), 8);
        assert_eq!(term.cursor_style().shape, CursorShape::Beam);
    }

    #[cfg(unix)]
    #[test]
    fn locale_override_plan_forces_lc_ctype_when_no_utf8_and_no_lc_all() {
        assert_eq!(
            super::utf8_locale_override_plan(None, Some("C"), Some("")),
            super::Utf8LocaleOverridePlan::LcCtypeOnly
        );
    }

    #[cfg(unix)]
    #[test]
    fn locale_override_plan_forces_lc_all_when_lc_all_is_non_utf8() {
        assert_eq!(
            super::utf8_locale_override_plan(Some("C"), Some("C"), Some("")),
            super::Utf8LocaleOverridePlan::LcAllAndLcCtype
        );
    }

    #[cfg(unix)]
    #[test]
    fn locale_override_plan_skips_when_utf8_present() {
        assert_eq!(
            super::utf8_locale_override_plan(Some("en_US.UTF-8"), Some("C"), Some("")),
            super::Utf8LocaleOverridePlan::None
        );
    }

    #[cfg(unix)]
    #[test]
    fn locale_override_plan_prefers_lc_all_over_lang() {
        assert_eq!(
            super::utf8_locale_override_plan(
                Some("fr_FR.ISO8859-1"),
                Some("C"),
                Some("en_US.UTF-8")
            ),
            super::Utf8LocaleOverridePlan::LcAllAndLcCtype
        );
    }

    #[cfg(unix)]
    #[test]
    fn locale_override_plan_does_not_skip_for_utf8_substring_false_positive() {
        assert_eq!(
            super::utf8_locale_override_plan(Some("en_US.fakeutf8"), Some("C"), Some("")),
            super::Utf8LocaleOverridePlan::LcAllAndLcCtype
        );
    }

    #[cfg(unix)]
    #[test]
    fn locale_override_plan_skips_for_utf8_with_modifier() {
        assert_eq!(
            super::utf8_locale_override_plan(Some("en_US.UTF-8@variant"), Some("C"), Some("")),
            super::Utf8LocaleOverridePlan::None
        );
    }

    #[cfg(unix)]
    #[test]
    fn preferred_utf8_locale_preserves_lang_region_from_lc_all() {
        assert_eq!(
            super::preferred_utf8_locale(
                Some("fr_FR.ISO8859-1"),
                Some("C"),
                Some("en_US.ISO8859-1")
            ),
            "fr_FR.UTF-8"
        );
    }

    #[cfg(unix)]
    #[test]
    fn preferred_utf8_locale_preserves_locale_modifier() {
        assert_eq!(
            super::preferred_utf8_locale(None, Some("sr_RS@latin"), Some("")),
            "sr_RS.UTF-8@latin"
        );
    }

    #[cfg(unix)]
    #[test]
    fn preferred_utf8_locale_falls_back_for_c_or_posix() {
        assert_eq!(
            super::preferred_utf8_locale(Some("C"), Some("POSIX"), Some("")),
            crate::locale::DEFAULT_UTF8_LOCALE
        );
    }
}
