#![allow(clippy::missing_safety_doc)]

use std::{
    collections::{BTreeSet, HashMap},
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    ptr, slice, str,
    time::Duration,
};

#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};

use flume::{Receiver, Sender, bounded};
use termy_core::{
    ConfigDiagnostic, ConfigDiagnosticKind, KittyGraphicsRenderPlacement, LoadedTermyConfig,
    ProgressState, Terminal, TerminalClipboardTarget, TerminalDamageSnapshot, TerminalDirtySpan,
    TerminalEvent, TerminalKeyEventKind, TerminalMouseButton, TerminalMouseEventKind,
    TerminalMouseModifiers, TerminalMousePosition, TerminalOptions, TerminalQueryColors,
    TerminalReplyHost, TerminalRuntimeConfig, TerminalSize, TermyCell, TermyColor,
    TermyFrameUpdate, TermyKeystroke, TermyModifiers, TermySearchOptions, TermySharedSearchMatch,
    encode_mouse_report, keystroke_to_input_with_options, load_config_from_contents,
    load_config_from_default_path, load_config_from_path,
};

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TermyFfiStatus {
    Ok = 0,
    Null = 1,
    InvalidUtf8 = 2,
    SpawnFailed = 3,
    ConfigLoadFailed = 4,
    UnknownKey = 5,
    WriteFailed = 6,
    SerializeFailed = 7,
    Panicked = 8,
}

#[cfg(test)]
static PANIC_NEXT_FEED_OUTPUT: AtomicBool = AtomicBool::new(false);
#[cfg(test)]
static PANIC_NEXT_RESIZE: AtomicBool = AtomicBool::new(false);

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TermyFfiSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: f32,
    pub cell_height: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyFfiColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

/// One viewport cell. Carries no position: full frames are row-major
/// (`index = row * cols + col`) and frame-update cells follow the dirty spans
/// in order, so the host derives position from context.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyFfiCell {
    pub codepoint: u32,
    pub fg: TermyFfiColor,
    pub bg: TermyFfiColor,
    pub uses_terminal_default_bg: bool,
    pub bold: bool,
    pub render_text: bool,
    pub wide_character_spacer: bool,
    pub line_wrapped: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyFfiCursor {
    pub visible: bool,
    pub col: usize,
    pub row: usize,
    pub style: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyFfiFrame {
    pub cols: u16,
    pub rows: u16,
    pub cells_ptr: *mut TermyFfiCell,
    pub cells_len: usize,
    pub cells_capacity: usize,
    pub cursor: TermyFfiCursor,
    pub display_offset: usize,
    pub history_size: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyFfiFrameUpdate {
    pub cols: u16,
    pub rows: u16,
    pub cells_ptr: *mut TermyFfiCell,
    pub cells_len: usize,
    pub cells_capacity: usize,
    pub cursor: TermyFfiCursor,
    pub display_offset: usize,
    pub history_size: usize,
    pub damage_kind: u32,
    pub spans_ptr: *mut TermyFfiDirtySpan,
    pub spans_len: usize,
    pub spans_capacity: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyFfiBytes {
    pub ptr: *mut u8,
    pub len: usize,
    pub capacity: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyFfiKittyGraphicsPlacement {
    pub placement_serial: u64,
    pub image_id: u32,
    pub placement_id: u32,
    pub png: TermyFfiBytes,
    pub image_width: u32,
    pub image_height: u32,
    pub image_generation: u64,
    pub viewport_row: i32,
    pub col: usize,
    pub source_x: u32,
    pub source_y: u32,
    pub source_width: u32,
    pub source_height: u32,
    pub has_display_cols: bool,
    pub display_cols: u32,
    pub has_display_rows: bool,
    pub display_rows: u32,
    pub occupied_cols: u32,
    pub occupied_rows: u32,
    pub x_offset: u32,
    pub y_offset: u32,
    pub z_index: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyFfiKittyGraphicsBatch {
    pub revision: u64,
    pub placements_ptr: *mut TermyFfiKittyGraphicsPlacement,
    pub placements_len: usize,
    pub placements_capacity: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyFfiEvent {
    pub kind: u32,
    pub exit_code: i32,
    pub progress_state: u8,
    pub progress_value: u8,
    pub payload: TermyFfiBytes,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyFfiEventBatch {
    pub events_ptr: *mut TermyFfiEvent,
    pub events_len: usize,
    pub events_capacity: usize,
    pub has_more: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyFfiDirtySpan {
    pub row: usize,
    pub left_col: usize,
    pub right_col: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyFfiDamage {
    pub kind: u32,
    pub spans_ptr: *mut TermyFfiDirtySpan,
    pub spans_len: usize,
    pub spans_capacity: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyFfiHyperlink {
    pub start_col: usize,
    pub end_col: usize,
    pub uri: TermyFfiBytes,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyFfiSearchMatch {
    pub row: usize,
    pub start_col: usize,
    pub end_col: usize,
    pub line: TermyFfiBytes,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyFfiSearchBatch {
    pub matches_ptr: *mut TermyFfiSearchMatch,
    pub matches_len: usize,
    pub matches_capacity: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyFfiSearchOptions {
    pub case_sensitive: bool,
    pub regex: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyFfiConfigDiagnostic {
    pub line_number: usize,
    pub kind: u32,
    pub message: TermyFfiBytes,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyFfiConfigDiagnosticBatch {
    pub diagnostics_ptr: *mut TermyFfiConfigDiagnostic,
    pub diagnostics_len: usize,
    pub diagnostics_capacity: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TermyFfiRenderConfig {
    pub font_family: TermyFfiBytes,
    pub active_theme: TermyFfiBytes,
    pub foreground: TermyFfiColor,
    pub background: TermyFfiColor,
    pub cursor: TermyFfiColor,
    pub font_size: f32,
    pub line_height: f32,
    pub padding_x: f32,
    pub padding_y: f32,
    pub background_opacity: f32,
    pub background_opacity_cells: bool,
    pub cursor_blink: bool,
    pub cursor_style: u32,
    pub cell_width: f32,
    pub cell_height: f32,
    pub background_blur: bool,
    pub mouse_scroll_multiplier: f32,
    pub scrollbar_visibility: u32,
    pub scrollbar_style: u32,
    pub copy_on_select: bool,
    pub copy_on_select_toast: bool,
    pub pane_focus_effect: u32,
    pub pane_focus_strength: f32,
    pub chrome_contrast: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyFfiSafetyConfig {
    pub warn_on_quit: bool,
    pub warn_on_quit_with_running_process: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TermyFfiNativeConfig {
    pub auto_update: bool,
    pub tmux_enabled: bool,
    pub tmux_persistence: bool,
    pub tmux_exclusive: bool,
    pub tmux_show_active_pane_border: bool,
    pub simple_mode: bool,
    pub native_tab_persistence: bool,
    pub native_layout_autosave: bool,
    pub native_buffer_persistence: bool,
    pub show_debug_overlay: bool,
    pub onboarding_complete: bool,
    pub tab_close_visibility: u32,
    pub tab_width_mode: u32,
    pub tab_bar_position: u32,
    pub native_tab_placement: u32,
    pub tab_switch_modifier_hints: bool,
    pub chrome_contrast: bool,
    pub command_palette_show_keybinds: bool,
    pub app_icon: u32,
    pub shell_integration_enabled: bool,
    pub progress_indicator_enabled: bool,
    pub auto_hide_tabbar: bool,
    pub show_termy_in_titlebar: bool,
    pub macos_option_as_alt: bool,
}

/// Opaque terminal handle passed across the C ABI as `*mut TermyFfiTerminal`.
///
/// # Thread safety
///
/// The handle is **not** internally synchronized. With a single exception, no
/// two functions taking the same handle may execute concurrently — the caller
/// must serialize all access (confine the handle to one thread, or guard it
/// with an external lock). Distinct handles are independent.
///
/// The exception is the wake channel: `termy_terminal_wait_for_wakeup` and
/// `termy_terminal_notify_wakeup` touch only the internal wake channel, never
/// the terminal state, so they are the only functions safe to call concurrently
/// with the serialized calls above. The intended pattern is one dedicated
/// thread blocked in `termy_terminal_wait_for_wakeup` while another thread
/// drives the terminal.
///
/// # Lifetime
///
/// `termy_terminal_free` consumes the handle and drops everything it owns,
/// including the wake channel. The caller must ensure no other function —
/// including a thread blocked in `termy_terminal_wait_for_wakeup` — is executing
/// on the handle when it is freed, and that none is called afterward. Teardown
/// for a handle that has a wakeup thread: stop terminal calls, call
/// `termy_terminal_notify_wakeup` to release the blocked wait, join that thread,
/// then call `termy_terminal_free`. Freeing while a thread is parked in
/// `termy_terminal_wait_for_wakeup` is a use-after-free.
pub struct TermyFfiTerminal {
    terminal: Terminal,
    wakeups_tx: Option<Sender<()>>,
    wakeups_rx: Option<Receiver<()>>,
}

pub struct TermyFfiConfig {
    loaded: LoadedTermyConfig,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyFfiEnvVar {
    pub key_ptr: *const u8,
    pub key_len: usize,
    pub value_ptr: *const u8,
    pub value_len: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyFfiTerminalOptions {
    pub config: *const TermyFfiConfig,
    pub working_directory_ptr: *const u8,
    pub working_directory_len: usize,
    pub startup_command_ptr: *const u8,
    pub startup_command_len: usize,
    pub env_vars_ptr: *const TermyFfiEnvVar,
    pub env_vars_len: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyFfiKeystroke {
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    pub platform: bool,
    pub function: bool,
    pub key_ptr: *const u8,
    pub key_len: usize,
    pub key_char_ptr: *const u8,
    pub key_char_len: usize,
    pub event_kind: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyFfiMouseInput {
    pub kind: u32,
    pub button: u32,
    pub col: usize,
    pub row: usize,
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
}

struct EmptyReplyHost;

impl TerminalReplyHost for EmptyReplyHost {
    fn load_clipboard(&mut self, _target: TerminalClipboardTarget) -> Option<String> {
        None
    }
}

impl From<TermyFfiSize> for TerminalSize {
    fn from(size: TermyFfiSize) -> Self {
        Self {
            cols: size.cols,
            rows: size.rows,
            cell_width: size.cell_width,
            cell_height: size.cell_height,
        }
    }
}

impl From<TermyColor> for TermyFfiColor {
    fn from(color: TermyColor) -> Self {
        Self {
            r: color.r,
            g: color.g,
            b: color.b,
            a: color.a,
        }
    }
}

impl From<TermyCell> for TermyFfiCell {
    fn from(cell: TermyCell) -> Self {
        Self {
            codepoint: cell.char as u32,
            fg: cell.fg.into(),
            bg: cell.bg.into(),
            uses_terminal_default_bg: cell.uses_terminal_default_bg,
            bold: cell.bold,
            render_text: cell.render_text,
            wide_character_spacer: cell.wide_character_spacer,
            line_wrapped: cell.line_wrapped,
            italic: cell.italic,
            underline: cell.underline,
            strikethrough: cell.strikethrough,
        }
    }
}

fn ffi_cursor_from_cursor(cursor: Option<termy_core::TerminalCursorState>) -> TermyFfiCursor {
    cursor.map_or_else(TermyFfiCursor::default, |cursor| TermyFfiCursor {
        visible: true,
        col: cursor.col,
        row: cursor.row,
        style: match cursor.style {
            termy_core::TerminalCursorStyle::Line => 1,
            termy_core::TerminalCursorStyle::Block => 2,
        },
    })
}

fn ffi_frame_update_from_update(update: TermyFrameUpdate) -> TermyFfiFrameUpdate {
    let cells = update
        .cells
        .into_iter()
        .map(TermyFfiCell::from)
        .collect::<Vec<_>>();
    let (cells_ptr, cells_len, cells_capacity) = leak_vec(cells);

    let (damage_kind, spans) = match update.damage {
        TerminalDamageSnapshot::Full => (1, Vec::new()),
        TerminalDamageSnapshot::Partial(spans) if spans.is_empty() => (0, Vec::new()),
        TerminalDamageSnapshot::Partial(spans) => {
            let spans = spans
                .into_iter()
                .map(
                    |TerminalDirtySpan {
                         row,
                         left_col,
                         right_col,
                     }| TermyFfiDirtySpan {
                        row,
                        left_col,
                        right_col,
                    },
                )
                .collect::<Vec<_>>();
            (2, spans)
        }
    };
    let (spans_ptr, spans_len, spans_capacity) = leak_vec(spans);

    TermyFfiFrameUpdate {
        cols: update.cols,
        rows: update.rows,
        cells_ptr,
        cells_len,
        cells_capacity,
        cursor: ffi_cursor_from_cursor(update.cursor),
        display_offset: update.display_offset,
        history_size: update.history_size,
        damage_kind,
        spans_ptr,
        spans_len,
        spans_capacity,
    }
}

fn ffi_bytes_from_vec(mut bytes: Vec<u8>) -> TermyFfiBytes {
    let result = TermyFfiBytes {
        ptr: bytes.as_mut_ptr(),
        len: bytes.len(),
        capacity: bytes.capacity(),
    };
    std::mem::forget(bytes);
    result
}

fn ffi_bytes_from_string(value: String) -> TermyFfiBytes {
    ffi_bytes_from_vec(value.into_bytes())
}

fn ffi_kitty_graphics_placement_from_placement(
    placement: KittyGraphicsRenderPlacement,
) -> TermyFfiKittyGraphicsPlacement {
    TermyFfiKittyGraphicsPlacement {
        placement_serial: placement.placement_serial,
        image_id: placement.image_id,
        placement_id: placement.placement_id,
        png: ffi_bytes_from_vec(placement.png.as_ref().to_vec()),
        image_width: placement.image_width,
        image_height: placement.image_height,
        image_generation: placement.image_generation,
        viewport_row: placement.viewport_row,
        col: placement.col,
        source_x: placement.source_x,
        source_y: placement.source_y,
        source_width: placement.source_width,
        source_height: placement.source_height,
        has_display_cols: placement.display_cols.is_some(),
        display_cols: placement.display_cols.unwrap_or_default(),
        has_display_rows: placement.display_rows.is_some(),
        display_rows: placement.display_rows.unwrap_or_default(),
        occupied_cols: placement.occupied_cols,
        occupied_rows: placement.occupied_rows,
        x_offset: placement.x_offset,
        y_offset: placement.y_offset,
        z_index: placement.z_index,
    }
}

fn progress_parts(progress: ProgressState) -> (u8, u8) {
    match progress {
        ProgressState::Clear => (0, 0),
        ProgressState::InProgress(value) => (1, value),
        ProgressState::Error(value) => (2, value),
        ProgressState::Indeterminate => (3, 0),
        ProgressState::Warning(value) => (4, value),
    }
}

fn ffi_event_from_event(event: TerminalEvent) -> TermyFfiEvent {
    match event {
        TerminalEvent::Wakeup => TermyFfiEvent {
            kind: 1,
            ..TermyFfiEvent::default()
        },
        TerminalEvent::Title(title) => TermyFfiEvent {
            kind: 2,
            payload: ffi_bytes_from_string(title),
            ..TermyFfiEvent::default()
        },
        TerminalEvent::ResetTitle => TermyFfiEvent {
            kind: 3,
            ..TermyFfiEvent::default()
        },
        TerminalEvent::Bell => TermyFfiEvent {
            kind: 4,
            ..TermyFfiEvent::default()
        },
        TerminalEvent::Exit => TermyFfiEvent {
            kind: 5,
            ..TermyFfiEvent::default()
        },
        TerminalEvent::ClipboardStore(text) => TermyFfiEvent {
            kind: 6,
            payload: ffi_bytes_from_string(text),
            ..TermyFfiEvent::default()
        },
        TerminalEvent::ShellPromptStart => TermyFfiEvent {
            kind: 7,
            ..TermyFfiEvent::default()
        },
        TerminalEvent::ShellCommandStart => TermyFfiEvent {
            kind: 8,
            ..TermyFfiEvent::default()
        },
        TerminalEvent::ShellCommandExecuting => TermyFfiEvent {
            kind: 9,
            ..TermyFfiEvent::default()
        },
        TerminalEvent::ShellCommandFinished(code) => TermyFfiEvent {
            kind: 10,
            exit_code: code.unwrap_or(-1),
            ..TermyFfiEvent::default()
        },
        TerminalEvent::Progress(progress) => {
            let (progress_state, progress_value) = progress_parts(progress);
            TermyFfiEvent {
                kind: 11,
                progress_state,
                progress_value,
                ..TermyFfiEvent::default()
            }
        }
        TerminalEvent::WorkingDirectory(path) => TermyFfiEvent {
            kind: 12,
            payload: ffi_bytes_from_string(path),
            ..TermyFfiEvent::default()
        },
    }
}

fn ffi_search_match_from_match(search_match: TermySharedSearchMatch) -> TermyFfiSearchMatch {
    TermyFfiSearchMatch {
        row: search_match.row,
        start_col: search_match.start_col,
        end_col: search_match.end_col,
        // The core shares one line across matches. The final match can move the
        // original allocation into the FFI batch; earlier matches clone only
        // because the C ABI gives every result independent ownership.
        line: ffi_bytes_from_string(std::sync::Arc::unwrap_or_clone(search_match.line)),
    }
}

fn leak_vec<T>(mut vec: Vec<T>) -> (*mut T, usize, usize) {
    let ptr = vec.as_mut_ptr();
    let len = vec.len();
    let capacity = vec.capacity();
    std::mem::forget(vec);
    (ptr, len, capacity)
}

fn ffi_guard<T>(default: T, f: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(default)
}

fn ffi_status_guard(f: impl FnOnce() -> TermyFfiStatus) -> TermyFfiStatus {
    ffi_guard(TermyFfiStatus::Panicked, f)
}

fn ffi_default_size_fallback() -> TermyFfiSize {
    TermyFfiSize {
        cols: 80,
        rows: 24,
        cell_width: 9.0,
        cell_height: 18.0,
    }
}

fn ffi_default_size() -> TermyFfiSize {
    let size = TerminalSize::default();
    TermyFfiSize {
        cols: size.cols,
        rows: size.rows,
        cell_width: size.cell_width,
        cell_height: size.cell_height,
    }
}

unsafe fn optional_utf8<'a>(ptr: *const u8, len: usize) -> Result<Option<&'a str>, TermyFfiStatus> {
    if ptr.is_null() || len == 0 {
        return Ok(None);
    }

    let bytes = unsafe { slice::from_raw_parts(ptr, len) };
    str::from_utf8(bytes)
        .map(Some)
        .map_err(|_| TermyFfiStatus::InvalidUtf8)
}

unsafe fn optional_utf8_owned(
    ptr: *const u8,
    len: usize,
) -> Result<Option<String>, TermyFfiStatus> {
    unsafe { optional_utf8(ptr, len) }.map(|value| value.map(ToOwned::to_owned))
}

unsafe fn required_utf8<'a>(ptr: *const u8, len: usize) -> Result<&'a str, TermyFfiStatus> {
    if ptr.is_null() {
        return Err(TermyFfiStatus::Null);
    }

    let bytes = unsafe { slice::from_raw_parts(ptr, len) };
    str::from_utf8(bytes).map_err(|_| TermyFfiStatus::InvalidUtf8)
}

unsafe fn contents_utf8<'a>(ptr: *const u8, len: usize) -> Result<&'a str, TermyFfiStatus> {
    if ptr.is_null() {
        if len == 0 {
            return Ok("");
        }
        return Err(TermyFfiStatus::Null);
    }

    unsafe { required_utf8(ptr, len) }
}

unsafe fn env_vars_from_ffi(
    ptr: *const TermyFfiEnvVar,
    len: usize,
) -> Result<HashMap<String, String>, TermyFfiStatus> {
    if len == 0 {
        return Ok(HashMap::new());
    }
    if ptr.is_null() {
        return Err(TermyFfiStatus::Null);
    }

    let env_vars = unsafe { slice::from_raw_parts(ptr, len) };
    let mut result = HashMap::with_capacity(env_vars.len());
    for env_var in env_vars {
        let key = unsafe { optional_utf8_owned(env_var.key_ptr, env_var.key_len) }?;
        let Some(key) = key.map(|value| value.trim().to_string()) else {
            continue;
        };
        if key.is_empty() {
            continue;
        }
        let value = unsafe { optional_utf8_owned(env_var.value_ptr, env_var.value_len) }?
            .unwrap_or_default();
        result.insert(key, value);
    }
    Ok(result)
}

fn config_diagnostic_kind(kind: ConfigDiagnosticKind) -> u32 {
    match kind {
        ConfigDiagnosticKind::UnknownSection => 1,
        ConfigDiagnosticKind::UnknownRootKey => 2,
        ConfigDiagnosticKind::UnknownColorKey => 3,
        ConfigDiagnosticKind::InvalidSyntax => 4,
        ConfigDiagnosticKind::InvalidValue => 5,
        ConfigDiagnosticKind::DuplicateRootKey => 6,
    }
}

fn mouse_button(button: u32) -> Option<TerminalMouseButton> {
    match button {
        1 => Some(TerminalMouseButton::Left),
        2 => Some(TerminalMouseButton::Middle),
        3 => Some(TerminalMouseButton::Right),
        _ => None,
    }
}

fn mouse_event(input: TermyFfiMouseInput) -> Option<TerminalMouseEventKind> {
    match input.kind {
        1 => Some(TerminalMouseEventKind::Press(mouse_button(input.button)?)),
        2 => Some(TerminalMouseEventKind::Release(mouse_button(input.button)?)),
        3 => Some(TerminalMouseEventKind::Drag(mouse_button(input.button)?)),
        4 => Some(TerminalMouseEventKind::Move),
        5 => Some(TerminalMouseEventKind::WheelUp),
        6 => Some(TerminalMouseEventKind::WheelDown),
        7 => Some(TerminalMouseEventKind::WheelLeft),
        8 => Some(TerminalMouseEventKind::WheelRight),
        _ => None,
    }
}

fn ffi_config_diagnostic_from_diagnostic(diagnostic: ConfigDiagnostic) -> TermyFfiConfigDiagnostic {
    TermyFfiConfigDiagnostic {
        line_number: diagnostic.line_number,
        kind: config_diagnostic_kind(diagnostic.kind),
        message: ffi_bytes_from_string(diagnostic.message),
    }
}

fn leak_loaded_config(
    loaded: Result<LoadedTermyConfig, termy_core::TermyConfigError>,
    out_config: *mut *mut TermyFfiConfig,
) -> TermyFfiStatus {
    if out_config.is_null() {
        return TermyFfiStatus::Null;
    }

    let Ok(loaded) = loaded else {
        return TermyFfiStatus::ConfigLoadFailed;
    };

    unsafe {
        *out_config = Box::into_raw(Box::new(TermyFfiConfig { loaded }));
    }
    TermyFfiStatus::Ok
}

fn tab_title_shell_integration_from_config(
    app_config: &termy_core::AppConfig,
) -> termy_core::TabTitleShellIntegration {
    termy_core::TabTitleShellIntegration {
        enabled: app_config.shell_integration_enabled && app_config.tab_title.shell_integration,
        explicit_prefix: app_config.tab_title.explicit_prefix.clone(),
    }
}

unsafe fn terminal_new_with_runtime_config(
    size: TermyFfiSize,
    runtime_config: &TerminalRuntimeConfig,
    tab_title_shell_integration: Option<&termy_core::TabTitleShellIntegration>,
    configured_working_dir: Option<&str>,
    startup_command_ptr: *const u8,
    startup_command_len: usize,
    out_terminal: *mut *mut TermyFfiTerminal,
) -> TermyFfiStatus {
    if out_terminal.is_null() {
        return TermyFfiStatus::Null;
    }

    let startup_command = match unsafe { optional_utf8(startup_command_ptr, startup_command_len) } {
        Ok(value) => value,
        Err(status) => return status,
    };

    let (wakeups_tx, wakeups_rx) = bounded(1);
    let Ok(terminal) = Terminal::new(
        size.into(),
        configured_working_dir,
        Some(wakeups_tx.clone()),
        tab_title_shell_integration,
        Some(runtime_config),
        startup_command,
    ) else {
        return TermyFfiStatus::SpawnFailed;
    };

    unsafe {
        *out_terminal = Box::into_raw(Box::new(TermyFfiTerminal {
            terminal,
            wakeups_tx: Some(wakeups_tx),
            wakeups_rx: Some(wakeups_rx),
        }));
    }
    TermyFfiStatus::Ok
}

#[unsafe(no_mangle)]
pub extern "C" fn termy_size_default() -> TermyFfiSize {
    ffi_guard(ffi_default_size_fallback(), ffi_default_size)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_new(
    size: TermyFfiSize,
    startup_command_ptr: *const u8,
    startup_command_len: usize,
    out_terminal: *mut *mut TermyFfiTerminal,
) -> TermyFfiStatus {
    ffi_status_guard(|| unsafe {
        terminal_new_with_runtime_config(
            size,
            &TerminalRuntimeConfig::default(),
            None,
            None,
            startup_command_ptr,
            startup_command_len,
            out_terminal,
        )
    })
}

/// Create a display-only terminal: a grid with no PTY/shell, fed via
/// `termy_terminal_feed_output`. Used for tmux control-mode panes. All other
/// terminal functions (render, resize, snapshot, free) work on the result.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_display_terminal_new(
    size: TermyFfiSize,
    out_terminal: *mut *mut TermyFfiTerminal,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if out_terminal.is_null() {
            return TermyFfiStatus::Null;
        }
        let terminal = Terminal::new_display(size.into(), None);
        unsafe {
            *out_terminal = Box::into_raw(Box::new(TermyFfiTerminal {
                terminal,
                wakeups_tx: None,
                wakeups_rx: None,
            }));
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_new_with_config(
    size: TermyFfiSize,
    config: *const TermyFfiConfig,
    startup_command_ptr: *const u8,
    startup_command_len: usize,
    out_terminal: *mut *mut TermyFfiTerminal,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if config.is_null() {
            return TermyFfiStatus::Null;
        }

        unsafe {
            let tab_title_shell_integration =
                tab_title_shell_integration_from_config(&(*config).loaded.app_config);
            terminal_new_with_runtime_config(
                size,
                &(*config).loaded.runtime_config,
                Some(&tab_title_shell_integration),
                None,
                startup_command_ptr,
                startup_command_len,
                out_terminal,
            )
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_new_with_options(
    size: TermyFfiSize,
    options: *const TermyFfiTerminalOptions,
    out_terminal: *mut *mut TermyFfiTerminal,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if options.is_null() || out_terminal.is_null() {
            return TermyFfiStatus::Null;
        }

        let options = unsafe { *options };
        let (mut runtime_config, tab_title_shell_integration) = if options.config.is_null() {
            (TerminalRuntimeConfig::default(), None)
        } else {
            unsafe {
                (
                    (*options.config).loaded.runtime_config.clone(),
                    Some(tab_title_shell_integration_from_config(
                        &(*options.config).loaded.app_config,
                    )),
                )
            }
        };
        let working_directory = match unsafe {
            optional_utf8(options.working_directory_ptr, options.working_directory_len)
        } {
            Ok(value) => value,
            Err(status) => return status,
        };
        let environment =
            match unsafe { env_vars_from_ffi(options.env_vars_ptr, options.env_vars_len) } {
                Ok(value) => value,
                Err(status) => return status,
            };
        runtime_config.environment.extend(environment);

        unsafe {
            terminal_new_with_runtime_config(
                size,
                &runtime_config,
                tab_title_shell_integration.as_ref(),
                working_directory,
                options.startup_command_ptr,
                options.startup_command_len,
                out_terminal,
            )
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_config_load_default(
    out_config: *mut *mut TermyFfiConfig,
) -> TermyFfiStatus {
    ffi_status_guard(|| leak_loaded_config(load_config_from_default_path(), out_config))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_config_load_path(
    path_ptr: *const u8,
    path_len: usize,
    out_config: *mut *mut TermyFfiConfig,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        let path = match unsafe { required_utf8(path_ptr, path_len) } {
            Ok(path) => path,
            Err(status) => return status,
        };
        leak_loaded_config(load_config_from_path(Path::new(path)), out_config)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_config_from_contents(
    contents_ptr: *const u8,
    contents_len: usize,
    out_config: *mut *mut TermyFfiConfig,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if out_config.is_null() {
            return TermyFfiStatus::Null;
        }

        let contents = match unsafe { contents_utf8(contents_ptr, contents_len) } {
            Ok(contents) => contents,
            Err(status) => return status,
        };

        let loaded = load_config_from_contents(contents);
        unsafe {
            *out_config = Box::into_raw(Box::new(TermyFfiConfig { loaded }));
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_config_free(config: *mut TermyFfiConfig) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if config.is_null() {
            return TermyFfiStatus::Null;
        }

        unsafe {
            drop(Box::from_raw(config));
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_config_loaded_from_disk(config: *const TermyFfiConfig) -> bool {
    ffi_guard(false, || {
        if config.is_null() {
            return false;
        }

        unsafe { (*config).loaded.loaded_from_disk }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_config_runtime_scrollback_history(
    config: *const TermyFfiConfig,
) -> usize {
    ffi_guard(0, || {
        if config.is_null() {
            return 0;
        }

        unsafe { (*config).loaded.runtime_config.scrollback_history }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_config_runtime_inactive_tab_scrollback(
    config: *const TermyFfiConfig,
    out_enabled: *mut bool,
    out_value: *mut usize,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if config.is_null() || out_enabled.is_null() || out_value.is_null() {
            return TermyFfiStatus::Null;
        }

        let inactive_tab_scrollback =
            unsafe { (*config).loaded.app_config.inactive_tab_scrollback };
        unsafe {
            *out_enabled = inactive_tab_scrollback.is_some();
            *out_value = inactive_tab_scrollback.unwrap_or_default();
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_config_diagnostic_count(config: *const TermyFfiConfig) -> usize {
    ffi_guard(0, || {
        if config.is_null() {
            return 0;
        }

        unsafe { (*config).loaded.diagnostics.len() }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_config_window_size(
    config: *const TermyFfiConfig,
    out_width: *mut f32,
    out_height: *mut f32,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if config.is_null() || out_width.is_null() || out_height.is_null() {
            return TermyFfiStatus::Null;
        }

        let app_config = unsafe { &(*config).loaded.app_config };
        unsafe {
            *out_width = app_config.window_width;
            *out_height = app_config.window_height;
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_config_working_directory(
    config: *const TermyFfiConfig,
    out_working_directory: *mut TermyFfiBytes,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if config.is_null() || out_working_directory.is_null() {
            return TermyFfiStatus::Null;
        }

        let working_directory = unsafe { (*config).loaded.app_config.working_dir.as_ref() };
        let bytes = working_directory.map_or_else(
            || termy_null_buffer(),
            |working_directory| ffi_bytes_from_string(working_directory.clone()),
        );
        unsafe {
            *out_working_directory = bytes;
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_config_safety(
    config: *const TermyFfiConfig,
    out_safety: *mut TermyFfiSafetyConfig,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if config.is_null() || out_safety.is_null() {
            return TermyFfiStatus::Null;
        }

        let app_config = unsafe { &(*config).loaded.app_config };
        unsafe {
            *out_safety = TermyFfiSafetyConfig {
                warn_on_quit: app_config.warn_on_quit,
                warn_on_quit_with_running_process: app_config.warn_on_quit_with_running_process,
            };
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_config_native(
    config: *const TermyFfiConfig,
    out_native: *mut TermyFfiNativeConfig,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if config.is_null() || out_native.is_null() {
            return TermyFfiStatus::Null;
        }

        let app_config = unsafe { &(*config).loaded.app_config };
        unsafe {
            *out_native = TermyFfiNativeConfig {
                auto_update: app_config.auto_update,
                tmux_enabled: app_config.tmux_enabled,
                tmux_persistence: app_config.tmux_persistence,
                tmux_exclusive: app_config.tmux_exclusive,
                tmux_show_active_pane_border: app_config.tmux_show_active_pane_border,
                simple_mode: app_config.simple_mode,
                native_tab_persistence: app_config.native_tab_persistence,
                native_layout_autosave: app_config.native_layout_autosave,
                native_buffer_persistence: app_config.native_buffer_persistence,
                show_debug_overlay: app_config.show_debug_overlay,
                onboarding_complete: app_config.onboarding_complete,
                tab_close_visibility: match app_config.tab_close_visibility {
                    cfg::TabCloseVisibility::ActiveHover => 0,
                    cfg::TabCloseVisibility::Hover => 1,
                    cfg::TabCloseVisibility::Always => 2,
                },
                tab_width_mode: match app_config.tab_width_mode {
                    cfg::TabWidthMode::Stable => 0,
                    cfg::TabWidthMode::ActiveGrow => 1,
                    cfg::TabWidthMode::ActiveGrowSticky => 2,
                    cfg::TabWidthMode::Uniform => 3,
                },
                tab_bar_position: match app_config.tab_bar_position {
                    cfg::TabBarPosition::Top => 0,
                    cfg::TabBarPosition::Right => 1,
                },
                native_tab_placement: match app_config.native_tab_placement {
                    cfg::NativeTabPlacement::NativeTabbar => 0,
                    cfg::NativeTabPlacement::Sidebar => 1,
                },
                tab_switch_modifier_hints: app_config.tab_switch_modifier_hints,
                chrome_contrast: app_config.chrome_contrast,
                command_palette_show_keybinds: app_config.command_palette_show_keybinds,
                app_icon: match app_config.app_icon {
                    cfg::AppIcon::TermyDefault => 0,
                    cfg::AppIcon::TermyOld => 1,
                },
                shell_integration_enabled: app_config.shell_integration_enabled,
                progress_indicator_enabled: app_config.progress_indicator_enabled,
                auto_hide_tabbar: app_config.auto_hide_tabbar,
                show_termy_in_titlebar: app_config.show_termy_in_titlebar,
                macos_option_as_alt: app_config.macos_option_as_alt,
            };
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_config_tmux_binary(
    config: *const TermyFfiConfig,
    out_binary: *mut TermyFfiBytes,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if config.is_null() || out_binary.is_null() {
            return TermyFfiStatus::Null;
        }

        let binary = unsafe { (*config).loaded.app_config.tmux_binary.clone() };
        unsafe {
            *out_binary = ffi_bytes_from_string(binary);
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_config_ui_font_family(
    config: *const TermyFfiConfig,
    out_font_family: *mut TermyFfiBytes,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if config.is_null() || out_font_family.is_null() {
            return TermyFfiStatus::Null;
        }

        let font_family = unsafe { (*config).loaded.app_config.ui_font_family.clone() };
        unsafe {
            *out_font_family = ffi_bytes_from_string(font_family);
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_config_path(
    config: *const TermyFfiConfig,
    out_path: *mut TermyFfiBytes,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if config.is_null() || out_path.is_null() {
            return TermyFfiStatus::Null;
        }

        let path = unsafe { (*config).loaded.path.as_ref() };
        let bytes = path.map_or_else(
            || termy_null_buffer(),
            |path| ffi_bytes_from_string(path.to_string_lossy().into_owned()),
        );
        unsafe {
            *out_path = bytes;
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_config_tasks_json(
    config: *const TermyFfiConfig,
    out_json: *mut TermyFfiBytes,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if config.is_null() || out_json.is_null() {
            return TermyFfiStatus::Null;
        }

        let tasks = unsafe { &(*config).loaded.app_config.tasks };
        let Ok(json) = serde_json::to_string(
            &tasks
                .iter()
                .map(|task| {
                    serde_json::json!({
                        "name": task.name,
                        "command": task.command,
                        "layout": task.layout,
                        "working_dir": task.working_dir,
                    })
                })
                .collect::<Vec<_>>(),
        ) else {
            return TermyFfiStatus::SerializeFailed;
        };

        unsafe {
            *out_json = ffi_bytes_from_string(json);
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_config_keybinds_json(
    config: *const TermyFfiConfig,
    out_json: *mut TermyFfiBytes,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if config.is_null() || out_json.is_null() {
            return TermyFfiStatus::Null;
        }

        let keybind_lines = unsafe { &(*config).loaded.app_config.keybind_lines };
        let line_refs = keybind_lines
            .iter()
            .map(|line| termy_command_core::KeybindLineRef {
                line_number: line.line_number,
                value: line.value.as_str(),
            });
        let (directives, _warnings) =
            termy_command_core::parse_keybind_directives_from_iter(line_refs);
        let resolved = termy_command_core::resolve_keybinds(
            termy_command_core::default_resolved_keybinds_for_platform(
                termy_command_core::KeybindPlatform::MacOs,
            ),
            &directives,
        );
        let Ok(json) = serde_json::to_string(
            &resolved
                .iter()
                .map(|keybind| {
                    serde_json::json!({
                        "trigger": keybind.trigger,
                        "action": keybind.action.config_name(),
                    })
                })
                .collect::<Vec<_>>(),
        ) else {
            return TermyFfiStatus::SerializeFailed;
        };

        unsafe {
            *out_json = ffi_bytes_from_string(json);
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_config_diagnostics(
    config: *const TermyFfiConfig,
    out_batch: *mut TermyFfiConfigDiagnosticBatch,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if config.is_null() || out_batch.is_null() {
            return TermyFfiStatus::Null;
        }

        let diagnostics = unsafe {
            (*config)
                .loaded
                .diagnostics
                .clone()
                .into_iter()
                .map(ffi_config_diagnostic_from_diagnostic)
                .collect::<Vec<_>>()
        };
        let (diagnostics_ptr, diagnostics_len, diagnostics_capacity) = leak_vec(diagnostics);

        unsafe {
            *out_batch = TermyFfiConfigDiagnosticBatch {
                diagnostics_ptr,
                diagnostics_len,
                diagnostics_capacity,
            };
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_config_diagnostics_free(
    batch: *mut TermyFfiConfigDiagnosticBatch,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if batch.is_null() {
            return TermyFfiStatus::Null;
        }

        let batch = unsafe { &mut *batch };
        if !batch.diagnostics_ptr.is_null() {
            let diagnostics = unsafe {
                Vec::from_raw_parts(
                    batch.diagnostics_ptr,
                    batch.diagnostics_len,
                    batch.diagnostics_capacity,
                )
            };
            for diagnostic in diagnostics {
                free_bytes(diagnostic.message);
            }
        }
        *batch = TermyFfiConfigDiagnosticBatch::default();
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_config_render_config(
    config: *const TermyFfiConfig,
    out_render_config: *mut TermyFfiRenderConfig,
) -> TermyFfiStatus {
    ffi_status_guard(|| unsafe {
        termy_config_render_config_for_appearance(config, 1, out_render_config)
    })
}

fn system_appearance_from_raw(system_appearance: u32) -> termy_core::SystemAppearance {
    match system_appearance {
        0 => termy_core::SystemAppearance::Light,
        _ => termy_core::SystemAppearance::Dark,
    }
}

fn terminal_query_colors_for_appearance(
    loaded: &LoadedTermyConfig,
    system_appearance: u32,
) -> TerminalQueryColors {
    termy_core::terminal_query_colors_from_resolved_theme(
        &termy_core::resolve_theme_colors_from_app_config(
            &loaded.app_config,
            loaded.path.as_deref(),
            system_appearance_from_raw(system_appearance),
        ),
    )
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_config_render_config_for_appearance(
    config: *const TermyFfiConfig,
    system_appearance: u32,
    out_render_config: *mut TermyFfiRenderConfig,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if config.is_null() || out_render_config.is_null() {
            return TermyFfiStatus::Null;
        }

        let loaded = unsafe { &(*config).loaded };
        let app_config = &loaded.app_config;
        let cell_metrics = termy_core::measure_cell_from_config(app_config);
        let theme_colors = termy_core::resolve_theme_colors_from_app_config(
            app_config,
            loaded.path.as_deref(),
            system_appearance_from_raw(system_appearance),
        );
        unsafe {
            *out_render_config = TermyFfiRenderConfig {
                font_family: ffi_bytes_from_string(app_config.font_family.clone()),
                active_theme: ffi_bytes_from_string(theme_colors.active_theme),
                foreground: theme_colors.foreground.into(),
                background: theme_colors.background.into(),
                cursor: theme_colors.cursor.into(),
                font_size: app_config.font_size,
                line_height: app_config.line_height,
                padding_x: app_config.padding_x,
                padding_y: app_config.padding_y,
                background_opacity: app_config.background_opacity,
                background_opacity_cells: app_config.background_opacity_cells,
                cursor_blink: app_config.cursor_blink,
                cursor_style: match app_config.cursor_style {
                    termy_core::AppConfigCursorStyle::Line => 1,
                    termy_core::AppConfigCursorStyle::Block => 2,
                },
                cell_width: cell_metrics.cell_width,
                cell_height: cell_metrics.cell_height,
                background_blur: app_config.background_blur,
                mouse_scroll_multiplier: app_config.mouse_scroll_multiplier,
                scrollbar_visibility: match app_config.terminal_scrollbar_visibility {
                    cfg::TerminalScrollbarVisibility::Off => 0,
                    cfg::TerminalScrollbarVisibility::Always => 1,
                    cfg::TerminalScrollbarVisibility::OnScroll => 2,
                },
                scrollbar_style: match app_config.terminal_scrollbar_style {
                    cfg::TerminalScrollbarStyle::Neutral => 0,
                    cfg::TerminalScrollbarStyle::MutedTheme => 1,
                    cfg::TerminalScrollbarStyle::Theme => 2,
                },
                copy_on_select: app_config.copy_on_select,
                copy_on_select_toast: app_config.copy_on_select_toast,
                pane_focus_effect: match app_config.pane_focus_effect {
                    cfg::PaneFocusEffect::Off => 0,
                    cfg::PaneFocusEffect::SoftSpotlight => 1,
                    cfg::PaneFocusEffect::Cinematic => 2,
                    cfg::PaneFocusEffect::Minimal => 3,
                },
                pane_focus_strength: app_config.pane_focus_strength,
                chrome_contrast: app_config.chrome_contrast,
            };
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_render_config_free(
    render_config: *mut TermyFfiRenderConfig,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if render_config.is_null() {
            return TermyFfiStatus::Null;
        }

        let render_config = unsafe { &mut *render_config };
        free_bytes(render_config.font_family);
        free_bytes(render_config.active_theme);
        *render_config = TermyFfiRenderConfig::default();
        TermyFfiStatus::Ok
    })
}

// ---------------------------------------------------------------------------
// Settings (native settings window <-> config.txt bridge)
// ---------------------------------------------------------------------------

use termy_config_core as cfg;

fn settings_read_contents() -> String {
    match cfg::config_path().and_then(|path| std::fs::read_to_string(path).ok()) {
        Some(contents) if !contents.trim().is_empty() => contents,
        _ => cfg::DEFAULT_CONFIG_TEMPLATE.to_string(),
    }
}

fn settings_write_contents(contents: &str) -> Result<(), TermyFfiStatus> {
    let path = cfg::config_path().ok_or(TermyFfiStatus::WriteFailed)?;
    settings_write_contents_to(&path, contents)
}

fn settings_write_contents_to(
    path: &std::path::Path,
    contents: &str,
) -> Result<(), TermyFfiStatus> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|_| TermyFfiStatus::WriteFailed)?;
    }
    std::fs::write(path, contents).map_err(|_| TermyFfiStatus::WriteFailed)
}

fn settings_color_hex(app: &cfg::AppConfig, id: cfg::ColorSettingId) -> Option<String> {
    use cfg::ColorSettingId::*;
    let rgb = match id {
        Foreground => app.colors.foreground,
        Background => app.colors.background,
        Cursor => app.colors.cursor,
        Black => app.colors.ansi[0],
        Red => app.colors.ansi[1],
        Green => app.colors.ansi[2],
        Yellow => app.colors.ansi[3],
        Blue => app.colors.ansi[4],
        Magenta => app.colors.ansi[5],
        Cyan => app.colors.ansi[6],
        White => app.colors.ansi[7],
        BrightBlack => app.colors.ansi[8],
        BrightRed => app.colors.ansi[9],
        BrightGreen => app.colors.ansi[10],
        BrightYellow => app.colors.ansi[11],
        BrightBlue => app.colors.ansi[12],
        BrightMagenta => app.colors.ansi[13],
        BrightCyan => app.colors.ansi[14],
        BrightWhite => app.colors.ansi[15],
    };
    rgb.map(|c| format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b))
}

const SETTINGS_THEME_REGISTRY_URL: &str =
    "https://raw.githubusercontent.com/termy-org/themes/main/index.json";
const SETTINGS_THEME_REGISTRY_CACHE_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct SettingsThemeStoreTheme {
    name: String,
    slug: String,
    description: String,
    latest_version: Option<String>,
    file_url: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SettingsThemeRegistryCache {
    version: u32,
    fetched_at: u64,
    registry_url: String,
    etag: Option<String>,
    themes: Vec<SettingsThemeStoreTheme>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettingsLegacyThemeStoreTheme {
    name: String,
    slug: String,
    #[serde(default)]
    description: String,
    latest_version: Option<String>,
    file_url: Option<String>,
}

impl SettingsLegacyThemeStoreTheme {
    fn into_theme(self) -> Option<SettingsThemeStoreTheme> {
        let name = self.name.trim().to_string();
        let slug = termy_themes::normalize_theme_id(&self.slug);
        if name.is_empty() || slug.is_empty() {
            return None;
        }

        Some(SettingsThemeStoreTheme {
            name,
            slug,
            description: self.description,
            latest_version: self.latest_version,
            file_url: self.file_url,
        })
    }
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum SettingsThemeStorePayload {
    Legacy(Vec<SettingsLegacyThemeStoreTheme>),
    Registry(termy_themes::ThemeRegistryIndex),
}

fn settings_installed_theme_ids(config_path: Option<&Path>) -> Vec<String> {
    let Some(config_path) = config_path else {
        return Vec::new();
    };
    let Some(config_dir) = config_path.parent() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(config_dir.join("themes")) else {
        return Vec::new();
    };

    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let is_json = path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("json"));
            if !is_json {
                return None;
            }

            let stem = path.file_stem()?.to_str()?;
            let normalized = termy_themes::normalize_theme_id(stem);
            (!normalized.is_empty()).then_some(normalized)
        })
        .collect()
}

fn settings_theme_registry_url() -> String {
    std::env::var("TERMY_THEME_REGISTRY_URL")
        .or_else(|_| std::env::var("THEME_STORE_REGISTRY_URL"))
        .unwrap_or_else(|_| SETTINGS_THEME_REGISTRY_URL.to_string())
}

fn settings_theme_registry_cache_path(config_path: Option<&Path>) -> Option<PathBuf> {
    let owned_config_path;
    let config_path = if let Some(path) = config_path {
        path
    } else {
        owned_config_path = cfg::config_path()?;
        owned_config_path.as_path()
    };
    Some(config_path.parent()?.join("theme_registry.cache"))
}

fn settings_current_unix_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn settings_theme_registry_fetch_url(registry_url: &str) -> String {
    if !registry_url.contains("raw.githubusercontent.com") {
        return registry_url.to_string();
    }

    let separator = if registry_url.contains('?') { '&' } else { '?' };
    format!(
        "{registry_url}{separator}termy_cache_bust={}",
        settings_current_unix_timestamp()
    )
}

fn settings_load_theme_registry_cache(
    config_path: Option<&Path>,
) -> Option<SettingsThemeRegistryCache> {
    let path = settings_theme_registry_cache_path(config_path)?;
    let bytes = std::fs::read(path).ok()?;
    let cache: SettingsThemeRegistryCache = bincode::deserialize(&bytes).ok()?;
    (cache.version == SETTINGS_THEME_REGISTRY_CACHE_VERSION).then_some(cache)
}

fn settings_save_theme_registry_cache(
    config_path: Option<&Path>,
    themes: &[SettingsThemeStoreTheme],
    registry_url: &str,
    etag: Option<String>,
) {
    let Some(path) = settings_theme_registry_cache_path(config_path) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let cache = SettingsThemeRegistryCache {
        version: SETTINGS_THEME_REGISTRY_CACHE_VERSION,
        fetched_at: settings_current_unix_timestamp(),
        registry_url: registry_url.to_string(),
        etag,
        themes: themes.to_vec(),
    };

    if let Ok(bytes) = bincode::serialize(&cache) {
        let _ = std::fs::write(path, bytes);
    }
}

fn settings_parse_theme_store_payload(
    raw_json: &str,
    registry_url: &str,
) -> Result<Vec<SettingsThemeStoreTheme>, String> {
    let payload: SettingsThemeStorePayload = serde_json::from_str(raw_json)
        .map_err(|error| format!("Invalid theme registry response: {error}"))?;

    let mut parsed: Vec<SettingsThemeStoreTheme> = match payload {
        SettingsThemeStorePayload::Legacy(themes) => themes
            .into_iter()
            .filter_map(SettingsLegacyThemeStoreTheme::into_theme)
            .collect(),
        SettingsThemeStorePayload::Registry(index) => index
            .themes
            .into_iter()
            .filter_map(|theme| {
                let slug = termy_themes::normalize_theme_id(&theme.slug);
                (!slug.is_empty() && !theme.name.trim().is_empty()).then(|| {
                    SettingsThemeStoreTheme {
                        name: theme.name.trim().to_string(),
                        slug,
                        description: theme.description,
                        latest_version: Some(theme.latest_version),
                        file_url: Some(termy_themes::registry_file_url(registry_url, &theme.file)),
                    }
                })
            })
            .collect(),
    };

    parsed.sort_unstable_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
    });
    Ok(parsed)
}

fn settings_fetch_theme_registry_themes(
    config_path: Option<&Path>,
) -> Result<Vec<SettingsThemeStoreTheme>, String> {
    let registry_url = settings_theme_registry_url();
    let cached = settings_load_theme_registry_cache(config_path);
    let cached_etag = cached.as_ref().and_then(|cache| {
        (cache.registry_url == registry_url)
            .then(|| cache.etag.clone())
            .flatten()
    });

    // Bound the request: ureq otherwise blocks indefinitely on connect/read,
    // and this runs synchronously on the main thread when Settings opens, so a
    // slow or hostile registry host (the URL is overridable via
    // TERMY_THEME_REGISTRY_URL) could hang the app.
    let mut request = ureq::get(&settings_theme_registry_fetch_url(&registry_url))
        .timeout(std::time::Duration::from_secs(10))
        .set("Accept", "application/json")
        .set("Cache-Control", "no-cache")
        .set("Pragma", "no-cache");

    if let Some(ref etag) = cached_etag {
        request = request.set("If-None-Match", etag);
    }

    match request.call() {
        Ok(response) if response.status() == 304 => cached
            .filter(|cache| cache.registry_url == registry_url)
            .map(|cache| cache.themes)
            .ok_or_else(|| {
                "Server returned 304 Not Modified but no matching local cache exists".to_string()
            }),
        Ok(response) => {
            let etag = response.header("etag").map(str::to_string);
            let raw = response
                .into_string()
                .map_err(|error| format!("Invalid theme registry response: {error}"))?;
            let themes = settings_parse_theme_store_payload(&raw, &registry_url)?;
            settings_save_theme_registry_cache(config_path, &themes, &registry_url, etag);
            Ok(themes)
        }
        Err(error) => cached
            .filter(|cache| cache.registry_url == registry_url)
            .map(|cache| cache.themes)
            .ok_or_else(|| format!("Failed to fetch store themes: {error}")),
    }
}

fn settings_theme_store_ids(config_path: Option<&Path>) -> Vec<String> {
    if config_path.is_none() {
        return Vec::new();
    }

    let registry_url = settings_theme_registry_url();
    if let Some(cache) = settings_load_theme_registry_cache(config_path)
        && cache.registry_url == registry_url
        && !cache.themes.is_empty()
    {
        return cache.themes.into_iter().map(|theme| theme.slug).collect();
    }

    settings_fetch_theme_registry_themes(config_path)
        .map(|themes| themes.into_iter().map(|theme| theme.slug).collect())
        .unwrap_or_default()
}

fn settings_installed_theme_versions_path(config_path: &Path) -> Option<PathBuf> {
    Some(config_path.parent()?.join("theme_store_installed.json"))
}

fn settings_installed_theme_file_path(config_path: &Path, slug: &str) -> Option<PathBuf> {
    let normalized = termy_themes::normalize_theme_id(slug);
    if normalized.is_empty() {
        return None;
    }
    Some(
        config_path
            .parent()?
            .join("themes")
            .join(format!("{normalized}.json")),
    )
}

fn settings_load_installed_theme_versions(config_path: &Path) -> HashMap<String, String> {
    let Some(path) = settings_installed_theme_versions_path(config_path) else {
        return HashMap::new();
    };
    let Ok(contents) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    serde_json::from_str::<HashMap<String, String>>(&contents)
        .unwrap_or_default()
        .into_iter()
        .map(|(slug, version)| {
            (
                termy_themes::normalize_theme_id(&slug),
                version.trim().to_string(),
            )
        })
        .filter(|(slug, _)| !slug.is_empty())
        .collect()
}

fn settings_persist_installed_theme_versions(
    config_path: &Path,
    versions: &HashMap<String, String>,
) -> Result<(), TermyFfiStatus> {
    let path =
        settings_installed_theme_versions_path(config_path).ok_or(TermyFfiStatus::WriteFailed)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|_| TermyFfiStatus::WriteFailed)?;
    }
    let contents =
        serde_json::to_string_pretty(versions).map_err(|_| TermyFfiStatus::SerializeFailed)?;
    std::fs::write(path, contents).map_err(|_| TermyFfiStatus::WriteFailed)
}

fn settings_install_theme_from_registry(slug: &str) -> Result<(), TermyFfiStatus> {
    let normalized = termy_themes::normalize_theme_id(slug);
    if normalized.is_empty() {
        return Err(TermyFfiStatus::UnknownKey);
    }

    let config_path = cfg::config_path().ok_or(TermyFfiStatus::ConfigLoadFailed)?;
    let themes = settings_fetch_theme_registry_themes(Some(config_path.as_path()))
        .map_err(|_| TermyFfiStatus::ConfigLoadFailed)?;
    let theme = themes
        .into_iter()
        .find(|theme| theme.slug.eq_ignore_ascii_case(&normalized))
        .ok_or(TermyFfiStatus::UnknownKey)?;
    let file_url = theme.file_url.ok_or(TermyFfiStatus::ConfigLoadFailed)?;

    let response = ureq::get(&file_url)
        .timeout(std::time::Duration::from_secs(10))
        .set("Accept", "application/json")
        .call()
        .map_err(|_| TermyFfiStatus::ConfigLoadFailed)?;
    let contents = response
        .into_string()
        .map_err(|_| TermyFfiStatus::ConfigLoadFailed)?;
    termy_themes::parse_theme_colors_json(&contents)
        .map_err(|_| TermyFfiStatus::SerializeFailed)?;

    let path = settings_installed_theme_file_path(&config_path, &normalized)
        .ok_or(TermyFfiStatus::WriteFailed)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|_| TermyFfiStatus::WriteFailed)?;
    }
    std::fs::write(path, contents).map_err(|_| TermyFfiStatus::WriteFailed)?;

    let mut versions = settings_load_installed_theme_versions(&config_path);
    versions.insert(normalized, theme.latest_version.unwrap_or_default());
    settings_persist_installed_theme_versions(&config_path, &versions)
}

fn settings_theme_label(theme_id: &str) -> String {
    if theme_id == cfg::SHELL_DECIDE_THEME_ID {
        return "Shell Decide".to_string();
    }

    theme_id
        .split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn settings_installed_theme_colors(
    theme_id: &str,
    config_path: Option<&Path>,
) -> Option<termy_themes::ThemeColors> {
    let normalized = termy_themes::normalize_theme_id(theme_id);
    if normalized.is_empty() || normalized == cfg::SHELL_DECIDE_THEME_ID {
        return None;
    }

    let config_dir = config_path.and_then(Path::parent)?;
    let path = config_dir.join("themes").join(format!("{normalized}.json"));
    if let Ok(contents) = std::fs::read_to_string(path)
        && let Ok(colors) = termy_themes::parse_theme_colors_json(&contents)
    {
        return Some(colors);
    }
    None
}

fn settings_theme_swatches(theme_id: &str, config_path: Option<&Path>) -> Vec<String> {
    let Some(colors) = settings_installed_theme_colors(theme_id, config_path) else {
        return Vec::new();
    };
    [
        colors.background,
        colors.foreground,
        colors.ansi[1],
        colors.ansi[2],
        colors.ansi[4],
        colors.ansi[5],
    ]
    .into_iter()
    .map(|color| format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b))
    .collect()
}

fn settings_theme_choices(
    loaded: &LoadedTermyConfig,
    current_value: Option<&str>,
) -> Vec<serde_json::Value> {
    use serde_json::json;

    let mut ids = BTreeSet::new();
    ids.insert(cfg::SHELL_DECIDE_THEME_ID.to_string());
    let installed_ids: BTreeSet<String> = settings_installed_theme_ids(loaded.path.as_deref())
        .into_iter()
        .collect();
    ids.extend(settings_theme_store_ids(loaded.path.as_deref()));
    ids.extend(installed_ids.iter().cloned());
    if let Some(current_value) = current_value
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        ids.insert(current_value.to_string());
    }

    ids.into_iter()
        .map(|theme_id| {
            json!({
                "value": theme_id,
                "label": settings_theme_label(&theme_id),
                "installed": theme_id == cfg::SHELL_DECIDE_THEME_ID || installed_ids.contains(&theme_id),
                "swatches": settings_theme_swatches(&theme_id, loaded.path.as_deref()),
            })
        })
        .collect()
}

fn settings_schema_json(loaded: &LoadedTermyConfig) -> String {
    use serde_json::{Value, json};

    let app = &loaded.app_config;
    let sections_meta = [
        (
            cfg::SettingsSection::Appearance,
            "appearance",
            "Appearance",
            "paintbrush",
        ),
        (
            cfg::SettingsSection::Terminal,
            "terminal",
            "Terminal",
            "terminal",
        ),
        (
            cfg::SettingsSection::Tabs,
            "tabs",
            "Tabs",
            "square.on.square",
        ),
        (
            cfg::SettingsSection::Advanced,
            "advanced",
            "Advanced",
            "gearshape.2",
        ),
        (
            cfg::SettingsSection::Colors,
            "colors",
            "Colors",
            "paintpalette",
        ),
        (
            cfg::SettingsSection::Keybindings,
            "keybindings",
            "Keybindings",
            "keyboard",
        ),
    ];

    let mut sections = Vec::new();
    for (section, id, label, icon) in sections_meta {
        let mut obj = json!({ "id": id, "label": label, "systemImage": icon });

        match section {
            cfg::SettingsSection::Colors => {
                let colors: Vec<Value> = cfg::COLOR_SETTING_SPECS
                    .iter()
                    .map(|spec| {
                        json!({
                            "key": spec.key,
                            "title": spec.title,
                            "description": spec.description,
                            "hex": settings_color_hex(app, spec.id),
                        })
                    })
                    .collect();
                obj["colors"] = json!(colors);
            }
            cfg::SettingsSection::Keybindings => {
                let lines: Vec<&str> = app
                    .keybind_lines
                    .iter()
                    .map(|line| line.value.as_str())
                    .collect();
                obj["keybinds"] = json!(lines);
            }
            _ => {
                let mut groups: Vec<(&str, Vec<Value>)> = Vec::new();
                for spec in cfg::ROOT_SETTING_SPECS {
                    if spec.section != section || matches!(spec.id, cfg::RootSettingId::Keybind) {
                        continue;
                    }
                    let kind = match spec.value_kind {
                        cfg::RootSettingValueKind::Text => "text",
                        cfg::RootSettingValueKind::Numeric => "numeric",
                        cfg::RootSettingValueKind::Boolean => "boolean",
                        cfg::RootSettingValueKind::Enum => "enum",
                        cfg::RootSettingValueKind::Special => "special",
                    };

                    let mut setting = json!({
                        "key": spec.key,
                        "title": spec.title,
                        "description": spec.description,
                        "kind": kind,
                        "value": cfg::root_setting_default_value(app, spec.id),
                    });

                    if let Some(choices) = cfg::root_setting_enum_choices(spec.id) {
                        let choices: Vec<Value> = choices
                            .iter()
                            .map(|choice| json!({ "value": choice.value, "label": choice.label }))
                            .collect();
                        setting["choices"] = json!(choices);
                    }
                    if matches!(
                        spec.id,
                        cfg::RootSettingId::Theme
                            | cfg::RootSettingId::ThemeLight
                            | cfg::RootSettingId::ThemeDark
                    ) {
                        setting["choices"] =
                            json!(settings_theme_choices(loaded, setting["value"].as_str()));
                    }

                    match groups.iter_mut().find(|(group, _)| *group == spec.group) {
                        Some((_, settings)) => settings.push(setting),
                        None => groups.push((spec.group, vec![setting])),
                    }
                }

                let groups: Vec<Value> = groups
                    .into_iter()
                    .map(|(group, settings)| json!({ "label": group, "settings": settings }))
                    .collect();
                obj["groups"] = json!(groups);
            }
        }

        sections.push(obj);
    }

    json!({
        "configPath": loaded.path.as_ref().map(|path| path.to_string_lossy().into_owned()),
        "loadedFromDisk": loaded.loaded_from_disk,
        "sections": sections,
    })
    .to_string()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_settings_schema_json(
    config: *const TermyFfiConfig,
    out_bytes: *mut TermyFfiBytes,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if config.is_null() || out_bytes.is_null() {
            return TermyFfiStatus::Null;
        }

        let loaded = unsafe { &(*config).loaded };
        let json = settings_schema_json(loaded);
        unsafe {
            *out_bytes = ffi_bytes_from_string(json);
        }
        TermyFfiStatus::Ok
    })
}

unsafe fn settings_set_root_inner(
    key_ptr: *const u8,
    key_len: usize,
    value_ptr: *const u8,
    value_len: usize,
) -> Result<(), TermyFfiStatus> {
    let key = unsafe { required_utf8(key_ptr, key_len) }?;
    let value = unsafe { optional_utf8(value_ptr, value_len) }?.unwrap_or("");
    let id = cfg::root_setting_from_key(key).ok_or(TermyFfiStatus::UnknownKey)?;
    let updated = cfg::upsert_root_setting(&settings_read_contents(), id, value.trim());
    settings_write_contents(&updated)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_settings_set_root(
    key_ptr: *const u8,
    key_len: usize,
    value_ptr: *const u8,
    value_len: usize,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        match unsafe { settings_set_root_inner(key_ptr, key_len, value_ptr, value_len) } {
            Ok(()) => TermyFfiStatus::Ok,
            Err(status) => status,
        }
    })
}

unsafe fn settings_reset_root_inner(
    key_ptr: *const u8,
    key_len: usize,
) -> Result<(), TermyFfiStatus> {
    let key = unsafe { required_utf8(key_ptr, key_len) }?;
    let id = cfg::root_setting_from_key(key).ok_or(TermyFfiStatus::UnknownKey)?;
    let updated = cfg::remove_root_setting(&settings_read_contents(), id);
    settings_write_contents(&updated)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_settings_reset_root(
    key_ptr: *const u8,
    key_len: usize,
) -> TermyFfiStatus {
    ffi_status_guard(
        || match unsafe { settings_reset_root_inner(key_ptr, key_len) } {
            Ok(()) => TermyFfiStatus::Ok,
            Err(status) => status,
        },
    )
}

unsafe fn settings_set_color_inner(
    key_ptr: *const u8,
    key_len: usize,
    hex_ptr: *const u8,
    hex_len: usize,
) -> Result<(), TermyFfiStatus> {
    let key = unsafe { required_utf8(key_ptr, key_len) }?;
    let id = cfg::color_setting_from_key(key).ok_or(TermyFfiStatus::UnknownKey)?;
    let value = unsafe { optional_utf8(hex_ptr, hex_len) }?.map(|hex| hex.trim().to_string());
    let updated = cfg::apply_color_updates(
        &settings_read_contents(),
        &[cfg::ColorSettingUpdate { id, value }],
    );
    settings_write_contents(&updated)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_settings_set_color(
    key_ptr: *const u8,
    key_len: usize,
    hex_ptr: *const u8,
    hex_len: usize,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        match unsafe { settings_set_color_inner(key_ptr, key_len, hex_ptr, hex_len) } {
            Ok(()) => TermyFfiStatus::Ok,
            Err(status) => status,
        }
    })
}

unsafe fn settings_set_keybinds_inner(
    text_ptr: *const u8,
    text_len: usize,
) -> Result<(), TermyFfiStatus> {
    let text = unsafe { optional_utf8(text_ptr, text_len) }?.unwrap_or("");
    let lines: Vec<String> = text
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect();
    let updated = cfg::replace_keybind_lines(&settings_read_contents(), &lines);
    settings_write_contents(&updated)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_settings_set_keybinds(
    text_ptr: *const u8,
    text_len: usize,
) -> TermyFfiStatus {
    ffi_status_guard(
        || match unsafe { settings_set_keybinds_inner(text_ptr, text_len) } {
            Ok(()) => TermyFfiStatus::Ok,
            Err(status) => status,
        },
    )
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_settings_install_theme(
    slug_ptr: *const u8,
    slug_len: usize,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        let slug = match unsafe { required_utf8(slug_ptr, slug_len) } {
            Ok(slug) => slug,
            Err(status) => return status,
        };
        match settings_install_theme_from_registry(slug) {
            Ok(()) => TermyFfiStatus::Ok,
            Err(status) => status,
        }
    })
}

/// Installs the bundled `termy-cli` shim (symlink into a PATH dir + shell PATH
/// setup), reusing `termy_cli_install_core`. `shell` may be null to use $SHELL.
/// A human-readable summary (or error) is written to `out_message`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_cli_install(
    shell_ptr: *const u8,
    shell_len: usize,
    out_message: *mut TermyFfiBytes,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        let shell = if shell_ptr.is_null() || shell_len == 0 {
            None
        } else {
            match unsafe { required_utf8(shell_ptr, shell_len) } {
                Ok(value) => Some(value),
                Err(status) => return status,
            }
        };
        match termy_cli_install_core::install_cli(shell) {
            Ok(result) => {
                if !out_message.is_null() {
                    let message = format!("Installed CLI to {}", result.install_path.display());
                    unsafe { *out_message = ffi_bytes_from_string(message) };
                }
                TermyFfiStatus::Ok
            }
            Err(error) => {
                if !out_message.is_null() {
                    unsafe { *out_message = ffi_bytes_from_string(error) };
                }
                TermyFfiStatus::WriteFailed
            }
        }
    })
}

// MARK: - tmux control mode (unix only)

#[cfg(unix)]
#[repr(C)]
pub struct TermyFfiTmuxNotification {
    /// 0 = output, 1 = needs-refresh, 2 = warning, 3 = exit.
    pub kind: u32,
    pub pane_id: TermyFfiBytes,
    pub data: TermyFfiBytes,
}

#[cfg(unix)]
#[repr(C)]
pub struct TermyFfiTmuxNotificationBatch {
    pub notifications_ptr: *mut TermyFfiTmuxNotification,
    pub notifications_len: usize,
    pub notifications_capacity: usize,
}

#[cfg(unix)]
fn ffi_tmux_notification(
    notification: tmux_control_core::types::TmuxNotification,
) -> Option<TermyFfiTmuxNotification> {
    use tmux_control_core::types::TmuxNotification;
    Some(match notification {
        TmuxNotification::Output { pane_id, bytes } => TermyFfiTmuxNotification {
            kind: 0,
            pane_id: ffi_bytes_from_string(pane_id),
            data: ffi_bytes_from_vec(bytes),
        },
        TmuxNotification::NeedsRefresh => TermyFfiTmuxNotification {
            kind: 1,
            pane_id: ffi_bytes_from_vec(Vec::new()),
            data: ffi_bytes_from_vec(Vec::new()),
        },
        TmuxNotification::Warning(message) => TermyFfiTmuxNotification {
            kind: 2,
            pane_id: ffi_bytes_from_vec(Vec::new()),
            data: ffi_bytes_from_string(message),
        },
        TmuxNotification::Exit(reason) => TermyFfiTmuxNotification {
            kind: 3,
            pane_id: ffi_bytes_from_vec(Vec::new()),
            data: ffi_bytes_from_string(reason.unwrap_or_default()),
        },
        TmuxNotification::SubscriptionChanged { .. } => {
            // The Swift/FFI client does not consume format subscriptions yet.
            // Keep this surface unchanged until it grows fields for the full
            // subscription payload.
            return None;
        }
    })
}

/// Opens a `tmux -CC` control session. On success writes a session handle to
/// `out_session`; free it with `termy_tmux_control_close`.
#[cfg(unix)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_tmux_control_open(
    binary_ptr: *const u8,
    binary_len: usize,
    socket_ptr: *const u8,
    socket_len: usize,
    session_ptr: *const u8,
    session_len: usize,
    out_session: *mut *mut tmux_control_core::session::ControlSession,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if out_session.is_null() {
            return TermyFfiStatus::Null;
        }
        let binary = match unsafe { required_utf8(binary_ptr, binary_len) } {
            Ok(value) => value,
            Err(status) => return status,
        };
        let socket = match unsafe { required_utf8(socket_ptr, socket_len) } {
            Ok(value) => value,
            Err(status) => return status,
        };
        let session_name = match unsafe { required_utf8(session_ptr, session_len) } {
            Ok(value) => value,
            Err(status) => return status,
        };
        match tmux_control_core::session::ControlSession::launch(binary, socket, session_name) {
            Ok(session) => {
                unsafe { *out_session = Box::into_raw(Box::new(session)) };
                TermyFfiStatus::Ok
            }
            Err(_) => TermyFfiStatus::SpawnFailed,
        }
    })
}

/// Drains pending control notifications into `out_batch`; free with
/// `termy_tmux_control_notifications_free`.
#[cfg(unix)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_tmux_control_poll(
    session: *mut tmux_control_core::session::ControlSession,
    out_batch: *mut TermyFfiTmuxNotificationBatch,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if session.is_null() || out_batch.is_null() {
            return TermyFfiStatus::Null;
        }
        let session = unsafe { &*session };
        let notifications: Vec<TermyFfiTmuxNotification> = session
            .poll()
            .into_iter()
            .filter_map(ffi_tmux_notification)
            .collect();
        let (ptr, len, capacity) = leak_vec(notifications);
        unsafe {
            *out_batch = TermyFfiTmuxNotificationBatch {
                notifications_ptr: ptr,
                notifications_len: len,
                notifications_capacity: capacity,
            };
        }
        TermyFfiStatus::Ok
    })
}

#[cfg(unix)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_tmux_control_notifications_free(
    batch: *mut TermyFfiTmuxNotificationBatch,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if batch.is_null() {
            return TermyFfiStatus::Null;
        }
        let batch = unsafe { &mut *batch };
        if !batch.notifications_ptr.is_null() {
            let notifications = unsafe {
                Vec::from_raw_parts(
                    batch.notifications_ptr,
                    batch.notifications_len,
                    batch.notifications_capacity,
                )
            };
            for notification in notifications {
                free_bytes(notification.pane_id);
                free_bytes(notification.data);
            }
            batch.notifications_ptr = std::ptr::null_mut();
            batch.notifications_len = 0;
            batch.notifications_capacity = 0;
        }
        TermyFfiStatus::Ok
    })
}

/// Runs a tmux command over the control channel; writes its output to
/// `out_output` (free with `termy_buffer_free`).
#[cfg(unix)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_tmux_control_send(
    session: *mut tmux_control_core::session::ControlSession,
    command_ptr: *const u8,
    command_len: usize,
    out_output: *mut TermyFfiBytes,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if session.is_null() {
            return TermyFfiStatus::Null;
        }
        let command = match unsafe { required_utf8(command_ptr, command_len) } {
            Ok(value) => value,
            Err(status) => return status,
        };
        let session = unsafe { &*session };
        match session.send_command(command) {
            Ok(output) => {
                if !out_output.is_null() {
                    unsafe { *out_output = ffi_bytes_from_string(output) };
                }
                TermyFfiStatus::Ok
            }
            Err(_) => TermyFfiStatus::WriteFailed,
        }
    })
}

/// Closes and frees a control session handle from `termy_tmux_control_open`.
#[cfg(unix)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_tmux_control_close(
    session: *mut tmux_control_core::session::ControlSession,
) {
    ffi_guard((), || {
        if !session.is_null() {
            drop(unsafe { Box::from_raw(session) });
        }
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_settings_prettify_config() -> TermyFfiStatus {
    ffi_status_guard(|| {
        let contents = settings_read_contents();
        let prettified = cfg::prettify_config_contents(&contents);
        match settings_write_contents(&prettified) {
            Ok(()) => TermyFfiStatus::Ok,
            Err(status) => status,
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_reload_default_config_colors(
    terminal: *mut TermyFfiTerminal,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if terminal.is_null() {
            return TermyFfiStatus::Null;
        }

        let Ok(loaded) = load_config_from_default_path() else {
            return TermyFfiStatus::ConfigLoadFailed;
        };
        let query_colors = terminal_query_colors_for_appearance(&loaded, 1);
        unsafe {
            (*terminal).terminal.set_query_colors(query_colors);
        }
        TermyFfiStatus::Ok
    })
}

/// Apply the palette resolved from `config` for the host's current appearance
/// to an existing terminal. This updates default and ANSI colors used by frame
/// snapshots and terminal color-query replies without restarting the PTY.
///
/// `system_appearance` uses the same ABI as
/// [`termy_config_render_config_for_appearance`]: `0` is light and every other
/// value is dark.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_apply_config_colors_for_appearance(
    terminal: *mut TermyFfiTerminal,
    config: *const TermyFfiConfig,
    system_appearance: u32,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if terminal.is_null() || config.is_null() {
            return TermyFfiStatus::Null;
        }

        let query_colors =
            unsafe { terminal_query_colors_for_appearance(&(*config).loaded, system_appearance) };
        unsafe {
            (*terminal).terminal.set_query_colors(query_colors);
        }
        TermyFfiStatus::Ok
    })
}

/// Destroy a terminal handle created by one of the `*_new` constructors.
///
/// See [`TermyFfiTerminal`] for the full lifetime contract: the caller must
/// guarantee no other function is running on `terminal` — in particular that
/// any wakeup thread has been woken (via `termy_terminal_notify_wakeup`) and
/// **joined** — before calling this. Freeing a handle while a thread is parked
/// in `termy_terminal_wait_for_wakeup` is a use-after-free. Passing null is a
/// no-op that returns `Null`; every non-null handle must be freed exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_free(terminal: *mut TermyFfiTerminal) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if terminal.is_null() {
            return TermyFfiStatus::Null;
        }

        unsafe {
            drop(Box::from_raw(terminal));
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_write(
    terminal: *mut TermyFfiTerminal,
    bytes_ptr: *const u8,
    bytes_len: usize,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if terminal.is_null() {
            return TermyFfiStatus::Null;
        }
        // An empty payload is a no-op, not an error. A zero-length Swift array
        // yields a null `baseAddress`, so treat (null, 0) as empty rather than
        // rejecting it (matches the other byte-buffer entry points).
        if bytes_len == 0 {
            return TermyFfiStatus::Ok;
        }
        if bytes_ptr.is_null() {
            return TermyFfiStatus::Null;
        }

        let bytes = unsafe { slice::from_raw_parts(bytes_ptr, bytes_len) };
        unsafe {
            (*terminal).terminal.write(bytes);
        }
        TermyFfiStatus::Ok
    })
}

/// Advance a display-only terminal's grid with output bytes (e.g. tmux
/// `%output`), without sending input to a PTY.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_feed_output(
    terminal: *mut TermyFfiTerminal,
    bytes_ptr: *const u8,
    bytes_len: usize,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if terminal.is_null() {
            return TermyFfiStatus::Null;
        }
        // Empty `%output` payloads are valid no-ops; don't fail on (null, 0).
        if bytes_len == 0 {
            return TermyFfiStatus::Ok;
        }
        if bytes_ptr.is_null() {
            return TermyFfiStatus::Null;
        }

        let bytes = unsafe { slice::from_raw_parts(bytes_ptr, bytes_len) };
        #[cfg(test)]
        if PANIC_NEXT_FEED_OUTPUT.swap(false, Ordering::SeqCst) {
            panic!("test-only feed_output panic");
        }
        unsafe {
            (*terminal).terminal.feed_output(bytes);
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_encode_key(
    terminal: *mut TermyFfiTerminal,
    keystroke: *const TermyFfiKeystroke,
    out_bytes: *mut TermyFfiBytes,
) -> TermyFfiStatus {
    ffi_status_guard(|| unsafe {
        termy_terminal_encode_key_impl(terminal, keystroke, false, out_bytes)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_encode_key_with_options(
    terminal: *mut TermyFfiTerminal,
    keystroke: *const TermyFfiKeystroke,
    macos_option_as_alt: bool,
    out_bytes: *mut TermyFfiBytes,
) -> TermyFfiStatus {
    ffi_status_guard(|| unsafe {
        termy_terminal_encode_key_impl(terminal, keystroke, macos_option_as_alt, out_bytes)
    })
}

unsafe fn termy_terminal_encode_key_impl(
    terminal: *mut TermyFfiTerminal,
    keystroke: *const TermyFfiKeystroke,
    macos_option_as_alt: bool,
    out_bytes: *mut TermyFfiBytes,
) -> TermyFfiStatus {
    if terminal.is_null() || keystroke.is_null() || out_bytes.is_null() {
        return TermyFfiStatus::Null;
    }

    let keystroke = unsafe { *keystroke };
    let key = match unsafe { required_utf8(keystroke.key_ptr, keystroke.key_len) } {
        Ok(key) => key.to_owned(),
        Err(status) => return status,
    };
    let key_char = match unsafe { optional_utf8(keystroke.key_char_ptr, keystroke.key_char_len) } {
        Ok(key_char) => key_char.map(ToOwned::to_owned),
        Err(status) => return status,
    };
    let event_kind = match keystroke.event_kind {
        2 => TerminalKeyEventKind::Repeat,
        3 => TerminalKeyEventKind::Release,
        _ => TerminalKeyEventKind::Press,
    };
    let input = unsafe {
        let terminal = &(*terminal).terminal;
        keystroke_to_input_with_options(
            &TermyKeystroke {
                modifiers: TermyModifiers {
                    control: keystroke.control,
                    alt: keystroke.alt,
                    shift: keystroke.shift,
                    platform: keystroke.platform,
                    function: keystroke.function,
                },
                key,
                key_char,
            },
            event_kind,
            terminal.keyboard_mode(),
            true,
            macos_option_as_alt,
        )
    };

    unsafe {
        *out_bytes = input.map_or_else(|| termy_null_buffer(), ffi_bytes_from_vec);
    }
    TermyFfiStatus::Ok
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_encode_mouse(
    terminal: *mut TermyFfiTerminal,
    input: *const TermyFfiMouseInput,
    out_bytes: *mut TermyFfiBytes,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if terminal.is_null() || input.is_null() || out_bytes.is_null() {
            return TermyFfiStatus::Null;
        }

        let input = unsafe { *input };
        let Some(event) = mouse_event(input) else {
            return TermyFfiStatus::UnknownKey;
        };

        let encoded = unsafe {
            let terminal = &(*terminal).terminal;
            encode_mouse_report(
                terminal.mouse_mode(),
                event,
                TerminalMousePosition {
                    col: input.col,
                    row: input.row,
                },
                TerminalMouseModifiers {
                    shift: input.shift,
                    alt: input.alt,
                    control: input.control,
                },
            )
        };

        unsafe {
            *out_bytes = encoded.map_or_else(|| termy_null_buffer(), ffi_bytes_from_vec);
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_resize(
    terminal: *mut TermyFfiTerminal,
    size: TermyFfiSize,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if terminal.is_null() {
            return TermyFfiStatus::Null;
        }

        #[cfg(test)]
        if PANIC_NEXT_RESIZE.swap(false, Ordering::SeqCst) {
            panic!("test-only resize panic");
        }

        unsafe {
            let terminal = &mut (*terminal).terminal;
            let next_size = size.into();
            if terminal.size() == next_size {
                // Preserve the legacy C/Swift contract: embedders may resend
                // the current dimensions solely to deliver SIGWINCH to a TUI.
                terminal.nudge_resize();
            } else {
                terminal.resize(next_size);
            }
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_set_wakeup_enabled(
    terminal: *mut TermyFfiTerminal,
    enabled: bool,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if terminal.is_null() {
            return TermyFfiStatus::Null;
        }

        unsafe {
            (*terminal).terminal.set_wakeup_enabled(enabled);
        }
        TermyFfiStatus::Ok
    })
}

/// Block up to `timeout_ms` (0 = poll without blocking) for a terminal wake
/// signal, coalescing all pending signals into a single `*out_woke = true`.
///
/// This is the one function designed to run on a **dedicated** thread
/// concurrently with the serialized terminal calls — it reads only the internal
/// wake channel, never terminal state. It must not still be executing when the
/// handle is freed; see [`TermyFfiTerminal`] for the teardown ordering.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_wait_for_wakeup(
    terminal: *mut TermyFfiTerminal,
    timeout_ms: u64,
    out_woke: *mut bool,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if terminal.is_null() || out_woke.is_null() {
            return TermyFfiStatus::Null;
        }

        let receiver = unsafe { (*terminal).wakeups_rx.as_ref() };
        let woke = match receiver {
            Some(receiver) => {
                let received = if timeout_ms == 0 {
                    receiver.try_recv().is_ok()
                } else {
                    receiver
                        .recv_timeout(Duration::from_millis(timeout_ms))
                        .is_ok()
                };
                if received {
                    while receiver.try_recv().is_ok() {}
                    true
                } else {
                    false
                }
            }
            None => false,
        };

        unsafe {
            *out_woke = woke;
        }
        TermyFfiStatus::Ok
    })
}

/// Wake a thread blocked in `termy_terminal_wait_for_wakeup`. Safe to call from
/// any thread; used both to nudge the host to drain events and, during teardown,
/// to release the wakeup thread so it can be joined before `termy_terminal_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_notify_wakeup(
    terminal: *mut TermyFfiTerminal,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if terminal.is_null() {
            return TermyFfiStatus::Null;
        }

        if let Some(sender) = unsafe { (*terminal).wakeups_tx.as_ref() } {
            let _ = sender.try_send(());
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_scroll_display(
    terminal: *mut TermyFfiTerminal,
    delta_lines: i32,
    out_changed: *mut bool,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if terminal.is_null() || out_changed.is_null() {
            return TermyFfiStatus::Null;
        }

        unsafe {
            *out_changed = (*terminal).terminal.scroll_display(delta_lines);
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_scroll_to_bottom(
    terminal: *mut TermyFfiTerminal,
    out_changed: *mut bool,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if terminal.is_null() || out_changed.is_null() {
            return TermyFfiStatus::Null;
        }

        unsafe {
            *out_changed = (*terminal).terminal.scroll_to_bottom();
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_clear_scrollback(
    terminal: *mut TermyFfiTerminal,
    out_changed: *mut bool,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if terminal.is_null() || out_changed.is_null() {
            return TermyFfiStatus::Null;
        }

        unsafe {
            *out_changed = (*terminal).terminal.clear_scrollback();
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_set_scrollback_history(
    terminal: *mut TermyFfiTerminal,
    scrollback_history: usize,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if terminal.is_null() {
            return TermyFfiStatus::Null;
        }

        unsafe {
            (*terminal)
                .terminal
                .set_scrollback_history(scrollback_history);
        }
        TermyFfiStatus::Ok
    })
}

/// Writes whether the terminal currently has bracketed-paste mode enabled into
/// `out_enabled`. Hosts use this to wrap pasted text in the bracketed-paste
/// markers (and to strip an embedded terminator) so a multi-line paste cannot
/// be executed line-by-line by the foreground program.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_bracketed_paste_mode(
    terminal: *mut TermyFfiTerminal,
    out_enabled: *mut bool,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if terminal.is_null() || out_enabled.is_null() {
            return TermyFfiStatus::Null;
        }

        unsafe {
            *out_enabled = (*terminal).terminal.bracketed_paste_mode();
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_snapshot(
    terminal: *mut TermyFfiTerminal,
    out_frame: *mut TermyFfiFrame,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if terminal.is_null() || out_frame.is_null() {
            return TermyFfiStatus::Null;
        }

        let frame = unsafe { (*terminal).terminal.snapshot() };
        let cells = frame
            .cells
            .into_iter()
            .map(TermyFfiCell::from)
            .collect::<Vec<_>>();
        let (cells_ptr, cells_len, cells_capacity) = leak_vec(cells);
        let cursor = ffi_cursor_from_cursor(frame.cursor);

        unsafe {
            *out_frame = TermyFfiFrame {
                cols: frame.cols,
                rows: frame.rows,
                cells_ptr,
                cells_len,
                cells_capacity,
                cursor,
                display_offset: frame.display_offset,
                history_size: frame.history_size,
            };
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_frame_free(frame: *mut TermyFfiFrame) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if frame.is_null() {
            return TermyFfiStatus::Null;
        }

        let frame = unsafe { &mut *frame };
        if !frame.cells_ptr.is_null() {
            unsafe {
                drop(Vec::from_raw_parts(
                    frame.cells_ptr,
                    frame.cells_len,
                    frame.cells_capacity,
                ));
            }
        }
        *frame = TermyFfiFrame::default();
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_take_frame_update(
    terminal: *mut TermyFfiTerminal,
    force_full: bool,
    out_update: *mut TermyFfiFrameUpdate,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if terminal.is_null() || out_update.is_null() {
            return TermyFfiStatus::Null;
        }

        let update = unsafe { (*terminal).terminal.frame_update(force_full) };
        unsafe {
            *out_update = ffi_frame_update_from_update(update);
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_frame_update_free(
    update: *mut TermyFfiFrameUpdate,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if update.is_null() {
            return TermyFfiStatus::Null;
        }

        let update = unsafe { &mut *update };
        if !update.cells_ptr.is_null() {
            unsafe {
                drop(Vec::from_raw_parts(
                    update.cells_ptr,
                    update.cells_len,
                    update.cells_capacity,
                ));
            }
        }
        if !update.spans_ptr.is_null() {
            unsafe {
                drop(Vec::from_raw_parts(
                    update.spans_ptr,
                    update.spans_len,
                    update.spans_capacity,
                ));
            }
        }
        *update = TermyFfiFrameUpdate::default();
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_kitty_graphics_revision(
    terminal: *mut TermyFfiTerminal,
    out_revision: *mut u64,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if terminal.is_null() || out_revision.is_null() {
            return TermyFfiStatus::Null;
        }

        unsafe {
            *out_revision = (*terminal).terminal.kitty_graphics_revision();
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_kitty_graphics_placements(
    terminal: *mut TermyFfiTerminal,
    out_batch: *mut TermyFfiKittyGraphicsBatch,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if terminal.is_null() || out_batch.is_null() {
            return TermyFfiStatus::Null;
        }

        let (revision, placements) = unsafe { (*terminal).terminal.kitty_graphics_snapshot() };
        let placements = placements
            .into_iter()
            .map(ffi_kitty_graphics_placement_from_placement)
            .collect::<Vec<_>>();
        let (placements_ptr, placements_len, placements_capacity) = leak_vec(placements);
        unsafe {
            *out_batch = TermyFfiKittyGraphicsBatch {
                revision,
                placements_ptr,
                placements_len,
                placements_capacity,
            };
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_kitty_graphics_batch_free(
    batch: *mut TermyFfiKittyGraphicsBatch,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if batch.is_null() {
            return TermyFfiStatus::Null;
        }

        let batch = unsafe { &mut *batch };
        if !batch.placements_ptr.is_null() {
            let placements = unsafe {
                Vec::from_raw_parts(
                    batch.placements_ptr,
                    batch.placements_len,
                    batch.placements_capacity,
                )
            };
            for placement in placements {
                free_bytes(placement.png);
            }
        }
        *batch = TermyFfiKittyGraphicsBatch::default();
        TermyFfiStatus::Ok
    })
}

/// Look up the OSC 8 hyperlink under a viewport cell. Sets `out_found` and,
/// when found, fills `out_link` with the contiguous link run on that row.
/// A found link's `uri` must be released with `termy_hyperlink_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_hyperlink_at(
    terminal: *mut TermyFfiTerminal,
    row: usize,
    col: usize,
    out_found: *mut bool,
    out_link: *mut TermyFfiHyperlink,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if terminal.is_null() || out_found.is_null() || out_link.is_null() {
            return TermyFfiStatus::Null;
        }

        let link = unsafe { (*terminal).terminal.hyperlink_at(row, col) };
        unsafe {
            if let Some(link) = link {
                *out_found = true;
                *out_link = TermyFfiHyperlink {
                    start_col: link.start_col,
                    end_col: link.end_col,
                    uri: ffi_bytes_from_string(link.target),
                };
            } else {
                *out_found = false;
                *out_link = TermyFfiHyperlink::default();
            }
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_hyperlink_free(link: *mut TermyFfiHyperlink) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if link.is_null() {
            return TermyFfiStatus::Null;
        }

        let link = unsafe { &mut *link };
        if !link.uri.ptr.is_null() {
            free_bytes(link.uri);
        }
        *link = TermyFfiHyperlink::default();
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_take_damage(
    terminal: *mut TermyFfiTerminal,
    out_damage: *mut TermyFfiDamage,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if terminal.is_null() || out_damage.is_null() {
            return TermyFfiStatus::Null;
        }

        let damage = unsafe { (*terminal).terminal.take_damage_snapshot() };
        let result = match damage {
            TerminalDamageSnapshot::Full => TermyFfiDamage {
                kind: 1,
                ..TermyFfiDamage::default()
            },
            TerminalDamageSnapshot::Partial(spans) if spans.is_empty() => TermyFfiDamage::default(),
            TerminalDamageSnapshot::Partial(spans) => {
                let spans = spans
                    .into_iter()
                    .map(
                        |TerminalDirtySpan {
                             row,
                             left_col,
                             right_col,
                         }| TermyFfiDirtySpan {
                            row,
                            left_col,
                            right_col,
                        },
                    )
                    .collect::<Vec<_>>();
                let (spans_ptr, spans_len, spans_capacity) = leak_vec(spans);
                TermyFfiDamage {
                    kind: 2,
                    spans_ptr,
                    spans_len,
                    spans_capacity,
                }
            }
        };

        unsafe {
            *out_damage = result;
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_damage_free(damage: *mut TermyFfiDamage) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if damage.is_null() {
            return TermyFfiStatus::Null;
        }

        let damage = unsafe { &mut *damage };
        if !damage.spans_ptr.is_null() {
            unsafe {
                drop(Vec::from_raw_parts(
                    damage.spans_ptr,
                    damage.spans_len,
                    damage.spans_capacity,
                ));
            }
        }
        *damage = TermyFfiDamage::default();
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_drain_events(
    terminal: *mut TermyFfiTerminal,
    out_batch: *mut TermyFfiEventBatch,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if terminal.is_null() || out_batch.is_null() {
            return TermyFfiStatus::Null;
        }

        let (events, has_more) = unsafe { (*terminal).terminal.drain_events(&mut EmptyReplyHost) };
        let events = events
            .into_iter()
            .map(ffi_event_from_event)
            .collect::<Vec<_>>();
        let (events_ptr, events_len, events_capacity) = leak_vec(events);

        unsafe {
            *out_batch = TermyFfiEventBatch {
                events_ptr,
                events_len,
                events_capacity,
                has_more,
            };
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_event_batch_free(batch: *mut TermyFfiEventBatch) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if batch.is_null() {
            return TermyFfiStatus::Null;
        }

        let batch = unsafe { &mut *batch };
        if !batch.events_ptr.is_null() {
            let events = unsafe {
                Vec::from_raw_parts(batch.events_ptr, batch.events_len, batch.events_capacity)
            };
            for event in events {
                free_bytes(event.payload);
            }
        }
        *batch = TermyFfiEventBatch::default();
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_search(
    terminal: *mut TermyFfiTerminal,
    query_ptr: *const u8,
    query_len: usize,
    out_batch: *mut TermyFfiSearchBatch,
) -> TermyFfiStatus {
    ffi_status_guard(|| unsafe {
        termy_terminal_search_with_options(
            terminal,
            query_ptr,
            query_len,
            TermyFfiSearchOptions::default(),
            out_batch,
        )
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_search_with_options(
    terminal: *mut TermyFfiTerminal,
    query_ptr: *const u8,
    query_len: usize,
    options: TermyFfiSearchOptions,
    out_batch: *mut TermyFfiSearchBatch,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if terminal.is_null() || out_batch.is_null() {
            return TermyFfiStatus::Null;
        }

        let query = match unsafe { contents_utf8(query_ptr, query_len) } {
            Ok(query) => query,
            Err(status) => return status,
        };

        let matches = unsafe {
            (*terminal).terminal.search_shared_with_options(
                query,
                TermySearchOptions {
                    case_sensitive: options.case_sensitive,
                    regex: options.regex,
                },
            )
        }
        .into_iter()
        .map(ffi_search_match_from_match)
        .collect::<Vec<_>>();
        let (matches_ptr, matches_len, matches_capacity) = leak_vec(matches);

        unsafe {
            *out_batch = TermyFfiSearchBatch {
                matches_ptr,
                matches_len,
                matches_capacity,
            };
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_search_batch_free(
    batch: *mut TermyFfiSearchBatch,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if batch.is_null() {
            return TermyFfiStatus::Null;
        }

        let batch = unsafe { &mut *batch };
        if !batch.matches_ptr.is_null() {
            let matches = unsafe {
                Vec::from_raw_parts(batch.matches_ptr, batch.matches_len, batch.matches_capacity)
            };
            for search_match in matches {
                free_bytes(search_match.line);
            }
        }
        *batch = TermyFfiSearchBatch::default();
        TermyFfiStatus::Ok
    })
}

fn free_bytes(bytes: TermyFfiBytes) {
    if bytes.ptr.is_null() {
        return;
    }

    unsafe {
        drop(Vec::from_raw_parts(bytes.ptr, bytes.len, bytes.capacity));
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_buffer_free(bytes: TermyFfiBytes) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if bytes.ptr.is_null() {
            return TermyFfiStatus::Null;
        }

        free_bytes(bytes);
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn termy_null_buffer() -> TermyFfiBytes {
    ffi_guard(TermyFfiBytes::default(), || TermyFfiBytes {
        ptr: ptr::null_mut(),
        len: 0,
        capacity: 0,
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn termy_runtime_config_default_scrollback() -> usize {
    ffi_guard(0, || TerminalRuntimeConfig::default().scrollback_history)
}

#[unsafe(no_mangle)]
pub extern "C" fn termy_terminal_options_default_scrollback() -> usize {
    ffi_guard(0, || TerminalOptions::default().scrollback_history)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_query_color_default_foreground(
    out_color: *mut TermyFfiColor,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if out_color.is_null() {
            return TermyFfiStatus::Null;
        }

        let color = TerminalQueryColors::default().foreground;
        unsafe {
            *out_color = TermyFfiColor {
                r: color.r,
                g: color.g,
                b: color.b,
                a: 255,
            };
        }
        TermyFfiStatus::Ok
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_size_is_nonzero() {
        let size = termy_size_default();
        assert!(size.cols > 0);
        assert!(size.rows > 0);
        assert!(size.cell_width > 0.0);
        assert!(size.cell_height > 0.0);
    }

    #[cfg(unix)]
    #[test]
    fn settings_write_reports_read_only_config_directory() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().expect("temporary settings root");
        let config_dir = root.path().join("termy");
        std::fs::create_dir(&config_dir).expect("create config directory");
        std::fs::set_permissions(&config_dir, std::fs::Permissions::from_mode(0o500))
            .expect("make config directory read-only");

        let result = settings_write_contents_to(&config_dir.join("config.txt"), "font_size = 14\n");

        std::fs::set_permissions(&config_dir, std::fs::Permissions::from_mode(0o700))
            .expect("restore config directory permissions");
        assert_eq!(result, Err(TermyFfiStatus::WriteFailed));
    }

    #[test]
    fn settings_schema_json_covers_sections_and_values() {
        let contents = b"font_size = 18\ncursor_style = line\ntmux_enabled = true\n[colors]\nforeground = #abcdef\n";
        let mut config = ptr::null_mut();
        assert_eq!(
            unsafe { termy_config_from_contents(contents.as_ptr(), contents.len(), &mut config) },
            TermyFfiStatus::Ok
        );

        let mut bytes = TermyFfiBytes::default();
        assert_eq!(
            unsafe { termy_settings_schema_json(config, &mut bytes) },
            TermyFfiStatus::Ok
        );
        let json = unsafe {
            str::from_utf8(slice::from_raw_parts(bytes.ptr, bytes.len)).expect("schema utf8")
        };
        let value: serde_json::Value = serde_json::from_str(json).expect("valid json");

        let sections = value["sections"].as_array().expect("sections array");
        let ids: Vec<&str> = sections
            .iter()
            .map(|section| section["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            vec![
                "appearance",
                "terminal",
                "tabs",
                "advanced",
                "colors",
                "keybindings"
            ]
        );

        // Appearance carries the edited font size value.
        let appearance = &sections[0];
        let font_size = appearance["groups"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|group| group["settings"].as_array().unwrap())
            .find(|setting| setting["key"] == "font_size")
            .expect("font_size setting");
        assert_eq!(font_size["value"], "18");
        assert_eq!(font_size["kind"], "numeric");
        let theme = appearance["groups"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|group| group["settings"].as_array().unwrap())
            .find(|setting| setting["key"] == "theme")
            .expect("theme setting");
        assert_eq!(theme["kind"], "special");
        let theme_choices = theme["choices"].as_array().expect("theme choices");
        assert!(
            theme_choices
                .iter()
                .any(|choice| choice["value"] == cfg::SHELL_DECIDE_THEME_ID)
        );
        assert!(
            theme_choices
                .iter()
                .any(|choice| choice["value"] == "termy")
        );
        assert!(
            !theme_choices
                .iter()
                .any(|choice| choice["value"] == "tokyo-night"),
            "schema loaded from in-memory contents must not synthesize legacy built-in themes"
        );
        assert!(
            sections
                .iter()
                .flat_map(|section| section["groups"].as_array().into_iter().flatten())
                .flat_map(|group| group["settings"].as_array().into_iter().flatten())
                .any(|setting| setting["key"] == "tmux_enabled")
        );

        // Colors section reflects the override hex.
        let colors = &sections[4]["colors"].as_array().unwrap();
        let foreground = colors
            .iter()
            .find(|color| color["key"] == "foreground")
            .expect("foreground color");
        assert_eq!(foreground["hex"], "#abcdef");

        assert_eq!(unsafe { termy_buffer_free(bytes) }, TermyFfiStatus::Ok);
        assert_eq!(unsafe { termy_config_free(config) }, TermyFfiStatus::Ok);
    }

    #[test]
    fn settings_schema_json_exposes_all_shared_root_settings() {
        let mut config = ptr::null_mut();
        assert_eq!(
            unsafe { termy_config_from_contents(b"".as_ptr(), 0, &mut config) },
            TermyFfiStatus::Ok
        );

        let mut bytes = TermyFfiBytes::default();
        assert_eq!(
            unsafe { termy_settings_schema_json(config, &mut bytes) },
            TermyFfiStatus::Ok
        );
        let json = unsafe {
            str::from_utf8(slice::from_raw_parts(bytes.ptr, bytes.len)).expect("schema json utf8")
        };
        let value: serde_json::Value = serde_json::from_str(json).expect("valid schema json");
        let exposed_keys = value["sections"]
            .as_array()
            .expect("sections array")
            .iter()
            .flat_map(|section| section["groups"].as_array().into_iter().flatten())
            .flat_map(|group| group["settings"].as_array().into_iter().flatten())
            .filter_map(|setting| setting["key"].as_str())
            .collect::<std::collections::HashSet<_>>();

        for spec in cfg::ROOT_SETTING_SPECS {
            if matches!(spec.id, cfg::RootSettingId::Keybind) {
                continue;
            }
            assert!(
                exposed_keys.contains(spec.key),
                "settings schema omitted shared config key {}",
                spec.key
            );
        }

        assert_eq!(unsafe { termy_buffer_free(bytes) }, TermyFfiStatus::Ok);
        assert_eq!(unsafe { termy_config_free(config) }, TermyFfiStatus::Ok);
    }

    #[test]
    fn config_from_contents_exposes_safety_config() {
        let contents = b"warn_on_quit = true\nwarn_on_quit_with_running_process = false\nauto_update = false\ntmux_enabled = true\ntmux_persistence = false\ntmux_exclusive = true\ntmux_binary = /opt/homebrew/bin/tmux\ntmux_show_active_pane_border = false\nsimple_mode = true\nnative_tab_persistence = true\nnative_layout_autosave = true\nnative_buffer_persistence = true\nshow_debug_overlay = true\nonboarding_complete = false\ntab_close_visibility = always\ntab_width_mode = active_grow_sticky\ntab_bar_position = right\nnative_tab_placement = sidebar\ntab_switch_modifier_hints = false\nui_font_family = Avenir Next\nchrome_contrast = true\ncommand_palette_show_keybinds = false\napp_icon = old\nshell_integration_enabled = false\nprogress_indicator_enabled = false\nauto_hide_tabbar = false\nshow_termy_in_titlebar = false\n";
        let mut config = ptr::null_mut();
        assert_eq!(
            unsafe { termy_config_from_contents(contents.as_ptr(), contents.len(), &mut config) },
            TermyFfiStatus::Ok
        );

        let mut safety = TermyFfiSafetyConfig::default();
        assert_eq!(
            unsafe { termy_config_safety(config, &mut safety) },
            TermyFfiStatus::Ok
        );
        assert!(safety.warn_on_quit);
        assert!(!safety.warn_on_quit_with_running_process);

        let mut native = TermyFfiNativeConfig::default();
        assert_eq!(
            unsafe { termy_config_native(config, &mut native) },
            TermyFfiStatus::Ok
        );
        assert!(!native.auto_update);
        assert!(native.tmux_enabled);
        assert!(!native.tmux_persistence);
        assert!(native.tmux_exclusive);
        assert!(!native.tmux_show_active_pane_border);
        assert!(native.simple_mode);
        assert!(native.native_tab_persistence);
        assert!(native.native_layout_autosave);
        assert!(native.native_buffer_persistence);
        assert!(native.show_debug_overlay);
        assert!(!native.onboarding_complete);
        assert_eq!(native.tab_close_visibility, 2);
        assert_eq!(native.tab_width_mode, 2);
        assert_eq!(native.tab_bar_position, 1);
        assert_eq!(native.native_tab_placement, 1);
        assert!(!native.tab_switch_modifier_hints);
        assert!(native.chrome_contrast);
        assert!(!native.command_palette_show_keybinds);
        assert_eq!(native.app_icon, 1);
        assert!(!native.shell_integration_enabled);
        assert!(!native.progress_indicator_enabled);
        assert!(!native.auto_hide_tabbar);
        assert!(!native.show_termy_in_titlebar);

        let mut tmux_binary = TermyFfiBytes::default();
        assert_eq!(
            unsafe { termy_config_tmux_binary(config, &mut tmux_binary) },
            TermyFfiStatus::Ok
        );
        let tmux_binary_value = unsafe {
            str::from_utf8(slice::from_raw_parts(tmux_binary.ptr, tmux_binary.len))
                .expect("tmux binary utf8")
        };
        assert_eq!(tmux_binary_value, "/opt/homebrew/bin/tmux");
        assert_eq!(
            unsafe { termy_buffer_free(tmux_binary) },
            TermyFfiStatus::Ok
        );

        let mut ui_font_family = TermyFfiBytes::default();
        assert_eq!(
            unsafe { termy_config_ui_font_family(config, &mut ui_font_family) },
            TermyFfiStatus::Ok
        );
        let ui_font_family_value = unsafe {
            str::from_utf8(slice::from_raw_parts(
                ui_font_family.ptr,
                ui_font_family.len,
            ))
            .expect("ui font utf8")
        };
        assert_eq!(ui_font_family_value, "Avenir Next");
        assert_eq!(
            unsafe { termy_buffer_free(ui_font_family) },
            TermyFfiStatus::Ok
        );

        assert_eq!(unsafe { termy_config_free(config) }, TermyFfiStatus::Ok);
    }

    #[test]
    fn config_from_contents_exposes_tasks_json() {
        let contents = b"task.build.command = cargo build\ntask.build.working_dir = crates/cli\ntask.dev.layout = dashboard\ntask.dev.command = cargo run\n";
        let mut config = ptr::null_mut();
        assert_eq!(
            unsafe { termy_config_from_contents(contents.as_ptr(), contents.len(), &mut config) },
            TermyFfiStatus::Ok
        );

        let mut bytes = TermyFfiBytes::default();
        assert_eq!(
            unsafe { termy_config_tasks_json(config, &mut bytes) },
            TermyFfiStatus::Ok
        );
        let json = unsafe {
            str::from_utf8(slice::from_raw_parts(bytes.ptr, bytes.len)).expect("tasks json utf8")
        };
        let tasks: serde_json::Value = serde_json::from_str(json).expect("valid tasks json");
        assert_eq!(tasks[0]["name"], "build");
        assert_eq!(tasks[0]["command"], "cargo build");
        assert_eq!(tasks[0]["working_dir"], "crates/cli");
        assert_eq!(tasks[0]["layout"], serde_json::Value::Null);
        assert_eq!(tasks[1]["name"], "dev");
        assert_eq!(tasks[1]["layout"], "dashboard");

        assert_eq!(unsafe { termy_buffer_free(bytes) }, TermyFfiStatus::Ok);
        assert_eq!(unsafe { termy_config_free(config) }, TermyFfiStatus::Ok);
    }

    #[test]
    fn config_from_contents_exposes_resolved_keybinds_json() {
        let contents = b"keybind = clear\nkeybind = cmd-p=toggle_command_palette\nkeybind = cmd-d=split_pane_vertical\n";
        let mut config = ptr::null_mut();
        assert_eq!(
            unsafe { termy_config_from_contents(contents.as_ptr(), contents.len(), &mut config) },
            TermyFfiStatus::Ok
        );

        let mut bytes = TermyFfiBytes::default();
        assert_eq!(
            unsafe { termy_config_keybinds_json(config, &mut bytes) },
            TermyFfiStatus::Ok
        );
        let json = unsafe {
            str::from_utf8(slice::from_raw_parts(bytes.ptr, bytes.len)).expect("keybind json utf8")
        };
        let keybinds: serde_json::Value = serde_json::from_str(json).expect("valid keybind json");
        assert_eq!(keybinds.as_array().map(Vec::len), Some(2));
        assert_eq!(keybinds[0]["trigger"], "cmd-p");
        assert_eq!(keybinds[0]["action"], "toggle_command_palette");
        assert_eq!(keybinds[1]["trigger"], "cmd-d");
        assert_eq!(keybinds[1]["action"], "split_pane_vertical");

        assert_eq!(unsafe { termy_buffer_free(bytes) }, TermyFfiStatus::Ok);
        assert_eq!(unsafe { termy_config_free(config) }, TermyFfiStatus::Ok);
    }

    #[test]
    fn settings_prettify_config_function_is_exported() {
        let _function: unsafe extern "C" fn() -> TermyFfiStatus = termy_settings_prettify_config;
    }

    #[test]
    fn config_from_contents_exposes_runtime_fields_and_diagnostics() {
        let contents = b"scrollback = 77\nwindow_width = 1440\nwindow_height = 900\nworking_dir = /tmp\nunknown_key = true\n";
        let mut config = ptr::null_mut();

        let status =
            unsafe { termy_config_from_contents(contents.as_ptr(), contents.len(), &mut config) };
        assert_eq!(status, TermyFfiStatus::Ok);
        assert!(!config.is_null());

        assert_eq!(
            unsafe { termy_config_runtime_scrollback_history(config) },
            77
        );
        assert_eq!(unsafe { termy_config_diagnostic_count(config) }, 1);

        let mut width = 0.0;
        let mut height = 0.0;
        assert_eq!(
            unsafe { termy_config_window_size(config, &mut width, &mut height) },
            TermyFfiStatus::Ok
        );
        assert_eq!(width, 1440.0);
        assert_eq!(height, 900.0);

        let mut working_directory = TermyFfiBytes::default();
        assert_eq!(
            unsafe { termy_config_working_directory(config, &mut working_directory) },
            TermyFfiStatus::Ok
        );
        let working_directory_text = unsafe {
            str::from_utf8(slice::from_raw_parts(
                working_directory.ptr,
                working_directory.len,
            ))
            .expect("working directory utf8")
        };
        assert_eq!(working_directory_text, "/tmp");
        assert_eq!(
            unsafe { termy_buffer_free(working_directory) },
            TermyFfiStatus::Ok
        );

        let mut diagnostics = TermyFfiConfigDiagnosticBatch::default();
        assert_eq!(
            unsafe { termy_config_diagnostics(config, &mut diagnostics) },
            TermyFfiStatus::Ok
        );
        assert_eq!(diagnostics.diagnostics_len, 1);
        let first = unsafe { *diagnostics.diagnostics_ptr };
        assert_eq!(first.line_number, 5);
        assert_eq!(first.kind, 2);
        assert!(!first.message.ptr.is_null());

        assert_eq!(
            unsafe { termy_config_diagnostics_free(&mut diagnostics) },
            TermyFfiStatus::Ok
        );
        assert_eq!(unsafe { termy_config_free(config) }, TermyFfiStatus::Ok);
    }

    #[test]
    fn config_from_contents_exposes_inactive_tab_scrollback() {
        let contents = b"inactive_tab_scrollback = 123\n";
        let mut config = ptr::null_mut();

        let status =
            unsafe { termy_config_from_contents(contents.as_ptr(), contents.len(), &mut config) };
        assert_eq!(status, TermyFfiStatus::Ok);
        assert!(!config.is_null());

        let mut enabled = false;
        let mut value = 0;
        assert_eq!(
            unsafe {
                termy_config_runtime_inactive_tab_scrollback(config, &mut enabled, &mut value)
            },
            TermyFfiStatus::Ok
        );
        assert!(enabled);
        assert_eq!(value, 123);

        assert_eq!(unsafe { termy_config_free(config) }, TermyFfiStatus::Ok);
    }

    #[test]
    fn config_from_contents_exposes_disabled_inactive_tab_scrollback() {
        let contents = b"inactive_tab_scrollback = none\n";
        let mut config = ptr::null_mut();

        let status =
            unsafe { termy_config_from_contents(contents.as_ptr(), contents.len(), &mut config) };
        assert_eq!(status, TermyFfiStatus::Ok);
        assert!(!config.is_null());

        let mut enabled = true;
        let mut value = 999;
        assert_eq!(
            unsafe {
                termy_config_runtime_inactive_tab_scrollback(config, &mut enabled, &mut value)
            },
            TermyFfiStatus::Ok
        );
        assert!(!enabled);
        assert_eq!(value, 0);

        assert_eq!(unsafe { termy_config_free(config) }, TermyFfiStatus::Ok);
    }

    #[test]
    fn config_from_contents_exposes_render_config() {
        let contents = b"theme = nord\nfont_family = Example Mono\nfont_size = 18\nline_height = 1.25\npadding_x = 3\npadding_y = 5\nbackground_opacity = 0.5\nbackground_opacity_cells = true\nbackground_blur = true\nmouse_scroll_multiplier = 4.5\nscrollbar_visibility = always\nscrollbar_style = theme\ncopy_on_select = true\ncopy_on_select_toast = false\npane_focus_effect = cinematic\npane_focus_strength = 1.25\nchrome_contrast = true\ncursor_blink = false\ncursor_style = line\n[colors]\nbackground = #010203\ncursor = #040506\n";
        let mut config = ptr::null_mut();

        let status =
            unsafe { termy_config_from_contents(contents.as_ptr(), contents.len(), &mut config) };
        assert_eq!(status, TermyFfiStatus::Ok);
        assert!(!config.is_null());

        let mut render_config = TermyFfiRenderConfig::default();
        assert_eq!(
            unsafe { termy_config_render_config(config, &mut render_config) },
            TermyFfiStatus::Ok
        );
        let font_family = unsafe {
            str::from_utf8(slice::from_raw_parts(
                render_config.font_family.ptr,
                render_config.font_family.len,
            ))
            .expect("font family utf8")
        };
        let active_theme = unsafe {
            str::from_utf8(slice::from_raw_parts(
                render_config.active_theme.ptr,
                render_config.active_theme.len,
            ))
            .expect("active theme utf8")
        };

        assert_eq!(font_family, "Example Mono");
        assert_eq!(active_theme, "nord");
        assert_eq!(
            render_config.background,
            TermyFfiColor {
                r: 1,
                g: 2,
                b: 3,
                a: 255,
            }
        );
        assert_eq!(
            render_config.cursor,
            TermyFfiColor {
                r: 4,
                g: 5,
                b: 6,
                a: 255,
            }
        );
        assert_eq!(render_config.font_size, 18.0);
        assert_eq!(render_config.line_height, 1.25);
        assert_eq!(render_config.padding_x, 3.0);
        assert_eq!(render_config.padding_y, 5.0);
        assert_eq!(render_config.background_opacity, 0.5);
        assert!(render_config.background_opacity_cells);
        assert!(!render_config.cursor_blink);
        assert_eq!(render_config.cursor_style, 1);
        assert!(render_config.cell_width >= 1.0);
        assert_eq!(render_config.cell_height, 22.5);
        assert!(render_config.background_blur);
        assert_eq!(render_config.mouse_scroll_multiplier, 4.5);
        assert_eq!(render_config.scrollbar_visibility, 1);
        assert_eq!(render_config.scrollbar_style, 2);
        assert!(render_config.copy_on_select);
        assert!(!render_config.copy_on_select_toast);
        assert_eq!(render_config.pane_focus_effect, 2);
        assert_eq!(render_config.pane_focus_strength, 1.25);
        assert!(render_config.chrome_contrast);

        assert_eq!(
            unsafe { termy_render_config_free(&mut render_config) },
            TermyFfiStatus::Ok
        );
        assert_eq!(unsafe { termy_config_free(config) }, TermyFfiStatus::Ok);
    }

    #[test]
    fn render_config_respects_requested_system_appearance() {
        let contents = b"theme_mode = system\ntheme_light = nord\ntheme_dark = termy\n";
        let mut config = ptr::null_mut();

        let status =
            unsafe { termy_config_from_contents(contents.as_ptr(), contents.len(), &mut config) };
        assert_eq!(status, TermyFfiStatus::Ok);
        assert!(!config.is_null());

        let mut light = TermyFfiRenderConfig::default();
        let mut dark = TermyFfiRenderConfig::default();
        assert_eq!(
            unsafe { termy_config_render_config_for_appearance(config, 0, &mut light) },
            TermyFfiStatus::Ok
        );
        assert_eq!(
            unsafe { termy_config_render_config_for_appearance(config, 1, &mut dark) },
            TermyFfiStatus::Ok
        );

        let light_theme = unsafe {
            str::from_utf8(slice::from_raw_parts(
                light.active_theme.ptr,
                light.active_theme.len,
            ))
            .expect("light theme utf8")
        };
        let dark_theme = unsafe {
            str::from_utf8(slice::from_raw_parts(
                dark.active_theme.ptr,
                dark.active_theme.len,
            ))
            .expect("dark theme utf8")
        };

        assert_eq!(light_theme, "nord");
        assert_eq!(dark_theme, "termy");

        assert_eq!(
            unsafe { termy_render_config_free(&mut light) },
            TermyFfiStatus::Ok
        );
        assert_eq!(
            unsafe { termy_render_config_free(&mut dark) },
            TermyFfiStatus::Ok
        );
        assert_eq!(unsafe { termy_config_free(config) }, TermyFfiStatus::Ok);
    }

    #[test]
    fn terminal_search_returns_visible_matches() {
        let size = TermyFfiSize {
            cols: 16,
            rows: 4,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        #[cfg(target_os = "windows")]
        let command: &[u8] = b"echo alpha beta && echo beta gamma";
        #[cfg(not(target_os = "windows"))]
        let command: &[u8] = b"printf 'alpha beta\nbeta gamma'";
        let mut terminal = ptr::null_mut();

        assert_eq!(
            unsafe { termy_terminal_new(size, command.as_ptr(), command.len(), &mut terminal,) },
            TermyFfiStatus::Ok
        );
        std::thread::sleep(std::time::Duration::from_millis(100));

        let query = b"beta";
        let mut batch = TermyFfiSearchBatch::default();
        assert_eq!(
            unsafe { termy_terminal_search(terminal, query.as_ptr(), query.len(), &mut batch) },
            TermyFfiStatus::Ok
        );
        assert!(batch.matches_len >= 1);

        let matches = unsafe { slice::from_raw_parts(batch.matches_ptr, batch.matches_len) };
        assert!(
            matches
                .iter()
                .any(|search_match| search_match.start_col == 6)
        );

        assert_eq!(
            unsafe { termy_search_batch_free(&mut batch) },
            TermyFfiStatus::Ok
        );
        assert_eq!(unsafe { termy_terminal_free(terminal) }, TermyFfiStatus::Ok);
    }

    #[test]
    fn terminal_wakeup_wait_is_coalesced_and_notifiable() {
        let size = TermyFfiSize {
            cols: 16,
            rows: 4,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        #[cfg(target_os = "windows")]
        let command: &[u8] = b"";
        #[cfg(not(target_os = "windows"))]
        let command: &[u8] = b"sleep 1";
        let mut terminal = ptr::null_mut();

        assert_eq!(
            unsafe { termy_terminal_new(size, command.as_ptr(), command.len(), &mut terminal) },
            TermyFfiStatus::Ok
        );

        let mut woke = false;
        loop {
            assert_eq!(
                unsafe { termy_terminal_wait_for_wakeup(terminal, 0, &mut woke) },
                TermyFfiStatus::Ok
            );
            if !woke {
                break;
            }
        }

        assert_eq!(
            unsafe { termy_terminal_notify_wakeup(terminal) },
            TermyFfiStatus::Ok
        );
        assert_eq!(
            unsafe { termy_terminal_notify_wakeup(terminal) },
            TermyFfiStatus::Ok
        );
        assert_eq!(
            unsafe { termy_terminal_wait_for_wakeup(terminal, 50, &mut woke) },
            TermyFfiStatus::Ok
        );
        assert!(woke);
        assert_eq!(
            unsafe { termy_terminal_wait_for_wakeup(terminal, 0, &mut woke) },
            TermyFfiStatus::Ok
        );
        assert!(!woke);

        assert_eq!(unsafe { termy_terminal_free(terminal) }, TermyFfiStatus::Ok);
    }

    /// Wraps the raw handle so it can cross a thread boundary. The FFI contract
    /// permits `termy_terminal_wait_for_wakeup` on a dedicated thread
    /// concurrently with serialized terminal calls elsewhere.
    struct SendTerminal(*mut TermyFfiTerminal);
    // SAFETY: the wait thread only calls `wait_for_wakeup`, which touches the
    // internal wake channel and never the terminal state mutated by the main
    // thread. The handle is freed only after this thread is joined.
    unsafe impl Send for SendTerminal {}

    #[test]
    fn wakeup_thread_runs_concurrently_with_terminal_calls_then_tears_down() {
        let size = TermyFfiSize {
            cols: 24,
            rows: 6,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        #[cfg(target_os = "windows")]
        let command: &[u8] = b"";
        #[cfg(not(target_os = "windows"))]
        let command: &[u8] = b"sleep 1";
        let mut terminal = ptr::null_mut();
        assert_eq!(
            unsafe { termy_terminal_new(size, command.as_ptr(), command.len(), &mut terminal) },
            TermyFfiStatus::Ok
        );

        // A dedicated thread parks on the wake channel — the contract's wakeup
        // thread — until told to stop.
        let shutdown = std::sync::Arc::new(AtomicBool::new(false));
        let waiter_shutdown = std::sync::Arc::clone(&shutdown);
        let handle = SendTerminal(terminal);
        let waiter = std::thread::spawn(move || {
            // Rebind the whole wrapper so the closure captures `SendTerminal`
            // (which is `Send`) rather than disjointly capturing the raw pointer
            // field, which is not.
            let handle = handle;
            let mut woke = false;
            while !waiter_shutdown.load(Ordering::Acquire) {
                assert_eq!(
                    unsafe { termy_terminal_wait_for_wakeup(handle.0, 25, &mut woke) },
                    TermyFfiStatus::Ok
                );
            }
        });

        // Meanwhile drive the terminal from this thread only (serialized access
        // to terminal state), concurrent with the parked wait thread above.
        let input = b"echo hi\n";
        for _ in 0..100 {
            assert_eq!(
                unsafe { termy_terminal_write(terminal, input.as_ptr(), input.len()) },
                TermyFfiStatus::Ok
            );
            assert_eq!(
                unsafe { termy_terminal_resize(terminal, size) },
                TermyFfiStatus::Ok
            );
        }

        // Teardown ordering from the contract: signal, wake the waiter, join it,
        // then free — so nothing is inside `wait_for_wakeup` when the handle dies.
        shutdown.store(true, Ordering::Release);
        assert_eq!(
            unsafe { termy_terminal_notify_wakeup(terminal) },
            TermyFfiStatus::Ok
        );
        waiter.join().expect("wakeup thread joins cleanly");
        assert_eq!(unsafe { termy_terminal_free(terminal) }, TermyFfiStatus::Ok);
    }

    #[test]
    fn terminal_feed_output_panic_returns_status() {
        let size = TermyFfiSize {
            cols: 8,
            rows: 2,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        let mut terminal = ptr::null_mut();
        assert_eq!(
            unsafe { termy_display_terminal_new(size, &mut terminal) },
            TermyFfiStatus::Ok
        );

        PANIC_NEXT_FEED_OUTPUT.store(true, Ordering::SeqCst);
        let bytes = b"x";
        assert_eq!(
            unsafe { termy_terminal_feed_output(terminal, bytes.as_ptr(), bytes.len()) },
            TermyFfiStatus::Panicked
        );
        assert!(!terminal.is_null());
        assert_eq!(unsafe { termy_terminal_free(terminal) }, TermyFfiStatus::Ok);
    }

    #[test]
    fn terminal_resize_panic_returns_status() {
        let size = TermyFfiSize {
            cols: 8,
            rows: 2,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        let mut terminal = ptr::null_mut();
        assert_eq!(
            unsafe { termy_display_terminal_new(size, &mut terminal) },
            TermyFfiStatus::Ok
        );

        PANIC_NEXT_RESIZE.store(true, Ordering::SeqCst);
        assert_eq!(
            unsafe { termy_terminal_resize(terminal, size) },
            TermyFfiStatus::Panicked
        );
        assert!(!terminal.is_null());
        assert_eq!(unsafe { termy_terminal_free(terminal) }, TermyFfiStatus::Ok);
    }

    #[test]
    fn terminal_search_with_options_supports_case_sensitive_matching() {
        let size = TermyFfiSize {
            cols: 16,
            rows: 4,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        #[cfg(target_os = "windows")]
        let command: &[u8] = b"echo alpha Beta && echo beta gamma";
        #[cfg(not(target_os = "windows"))]
        let command: &[u8] = b"printf 'alpha Beta\nbeta gamma'";
        let mut terminal = ptr::null_mut();

        assert_eq!(
            unsafe { termy_terminal_new(size, command.as_ptr(), command.len(), &mut terminal,) },
            TermyFfiStatus::Ok
        );
        std::thread::sleep(std::time::Duration::from_millis(100));

        let query = b"beta";
        let mut batch = TermyFfiSearchBatch::default();
        assert_eq!(
            unsafe {
                termy_terminal_search_with_options(
                    terminal,
                    query.as_ptr(),
                    query.len(),
                    TermyFfiSearchOptions {
                        case_sensitive: true,
                        regex: false,
                    },
                    &mut batch,
                )
            },
            TermyFfiStatus::Ok
        );

        let matches = unsafe { slice::from_raw_parts(batch.matches_ptr, batch.matches_len) };
        assert!(!matches.is_empty());
        assert!(
            matches
                .iter()
                .all(|search_match| search_match.start_col == 0)
        );

        assert_eq!(
            unsafe { termy_search_batch_free(&mut batch) },
            TermyFfiStatus::Ok
        );
        assert_eq!(unsafe { termy_terminal_free(terminal) }, TermyFfiStatus::Ok);
    }

    #[test]
    fn terminal_take_frame_update_supports_full_path() {
        let size = TermyFfiSize {
            cols: 8,
            rows: 2,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        #[cfg(target_os = "windows")]
        let command: &[u8] = b"echo abc";
        #[cfg(not(target_os = "windows"))]
        let command: &[u8] = b"printf abc";
        let mut terminal = ptr::null_mut();

        assert_eq!(
            unsafe { termy_terminal_new(size, command.as_ptr(), command.len(), &mut terminal) },
            TermyFfiStatus::Ok
        );
        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut update = TermyFfiFrameUpdate::default();
        assert_eq!(
            unsafe { termy_terminal_take_frame_update(terminal, true, &mut update) },
            TermyFfiStatus::Ok
        );
        assert_eq!(update.damage_kind, 1);
        assert_eq!(
            update.cells_len,
            usize::from(size.cols) * usize::from(size.rows)
        );
        let cells = unsafe { slice::from_raw_parts(update.cells_ptr, update.cells_len) };
        assert!(cells.iter().any(|cell| cell.codepoint == u32::from(b'a')));
        assert_eq!(
            unsafe { termy_frame_update_free(&mut update) },
            TermyFfiStatus::Ok
        );

        assert_eq!(unsafe { termy_terminal_free(terminal) }, TermyFfiStatus::Ok);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn terminal_hyperlink_at_reports_osc8_links() {
        let size = TermyFfiSize {
            cols: 24,
            rows: 2,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        // printf expands \033 to ESC; emits "docs" wrapped in an OSC 8 link.
        let command: &[u8] =
            b"printf '\\033]8;;https://example.com\\033\\\\docs\\033]8;;\\033\\\\'";
        let mut terminal = ptr::null_mut();

        assert_eq!(
            unsafe { termy_terminal_new(size, command.as_ptr(), command.len(), &mut terminal) },
            TermyFfiStatus::Ok
        );
        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut found = false;
        let mut link = TermyFfiHyperlink::default();
        assert_eq!(
            unsafe { termy_terminal_hyperlink_at(terminal, 0, 1, &mut found, &mut link) },
            TermyFfiStatus::Ok
        );
        assert!(found);
        assert_eq!(link.start_col, 0);
        assert_eq!(link.end_col, 3);
        let uri = unsafe { slice::from_raw_parts(link.uri.ptr, link.uri.len) };
        assert_eq!(uri, b"https://example.com");
        assert_eq!(
            unsafe { termy_hyperlink_free(&mut link) },
            TermyFfiStatus::Ok
        );

        assert_eq!(
            unsafe { termy_terminal_hyperlink_at(terminal, 0, 10, &mut found, &mut link) },
            TermyFfiStatus::Ok
        );
        assert!(!found);
        assert!(link.uri.ptr.is_null());

        assert_eq!(unsafe { termy_terminal_free(terminal) }, TermyFfiStatus::Ok);
    }

    #[test]
    fn terminal_encode_key_uses_core_keyboard_mapping() {
        let size = TermyFfiSize {
            cols: 16,
            rows: 4,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        let mut terminal = ptr::null_mut();

        assert_eq!(
            unsafe { termy_terminal_new(size, ptr::null(), 0, &mut terminal) },
            TermyFfiStatus::Ok
        );

        let key = b"tab";
        let keystroke = TermyFfiKeystroke {
            shift: true,
            key_ptr: key.as_ptr(),
            key_len: key.len(),
            event_kind: 1,
            ..TermyFfiKeystroke::default()
        };
        let mut bytes = TermyFfiBytes::default();
        assert_eq!(
            unsafe { termy_terminal_encode_key(terminal, &keystroke, &mut bytes) },
            TermyFfiStatus::Ok
        );
        let encoded = unsafe { slice::from_raw_parts(bytes.ptr, bytes.len) };
        assert_eq!(encoded, b"\x1b[Z");

        assert_eq!(unsafe { termy_buffer_free(bytes) }, TermyFfiStatus::Ok);
        assert_eq!(unsafe { termy_terminal_free(terminal) }, TermyFfiStatus::Ok);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn terminal_encode_key_with_options_applies_macos_option_as_alt() {
        let size = TermyFfiSize {
            cols: 16,
            rows: 4,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        let mut terminal = ptr::null_mut();

        assert_eq!(
            unsafe { termy_terminal_new(size, ptr::null(), 0, &mut terminal) },
            TermyFfiStatus::Ok
        );

        let key = b"space";
        let key_char = "\u{a0}".as_bytes();
        let keystroke = TermyFfiKeystroke {
            alt: true,
            key_ptr: key.as_ptr(),
            key_len: key.len(),
            key_char_ptr: key_char.as_ptr(),
            key_char_len: key_char.len(),
            event_kind: 1,
            ..TermyFfiKeystroke::default()
        };
        let mut bytes = TermyFfiBytes::default();
        assert_eq!(
            unsafe {
                termy_terminal_encode_key_with_options(terminal, &keystroke, true, &mut bytes)
            },
            TermyFfiStatus::Ok
        );
        let encoded = unsafe { slice::from_raw_parts(bytes.ptr, bytes.len) };
        assert_eq!(encoded, b"\x1b ");

        assert_eq!(unsafe { termy_buffer_free(bytes) }, TermyFfiStatus::Ok);
        assert_eq!(unsafe { termy_terminal_free(terminal) }, TermyFfiStatus::Ok);
    }

    #[test]
    fn terminal_encode_mouse_uses_live_mouse_mode() {
        let size = TermyFfiSize {
            cols: 16,
            rows: 4,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        let mut terminal = ptr::null_mut();

        assert_eq!(
            unsafe { termy_terminal_new(size, ptr::null(), 0, &mut terminal) },
            TermyFfiStatus::Ok
        );

        let input = TermyFfiMouseInput {
            kind: 1,
            button: 1,
            col: 4,
            row: 2,
            ..TermyFfiMouseInput::default()
        };
        let mut bytes = TermyFfiBytes::default();
        assert_eq!(
            unsafe { termy_terminal_encode_mouse(terminal, &input, &mut bytes) },
            TermyFfiStatus::Ok
        );
        assert!(bytes.ptr.is_null());

        unsafe {
            (*terminal)
                .terminal
                .hydrate_output(b"\x1b[?1000h\x1b[?1006h");
        }
        assert_eq!(
            unsafe { termy_terminal_encode_mouse(terminal, &input, &mut bytes) },
            TermyFfiStatus::Ok
        );
        let encoded = unsafe { slice::from_raw_parts(bytes.ptr, bytes.len) };
        assert_eq!(encoded, b"\x1b[<0;5;3M");

        assert_eq!(unsafe { termy_buffer_free(bytes) }, TermyFfiStatus::Ok);
        assert_eq!(unsafe { termy_terminal_free(terminal) }, TermyFfiStatus::Ok);
    }
}
