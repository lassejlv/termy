//! Stable C-compatible values shared by the exported ABI.

use std::ffi::c_void;

pub const TMON_ABI_VERSION: u32 = 0x0002_0000;

pub const TMON_OK: u32 = 0;
pub const TMON_NULL_POINTER: u32 = 1;
pub const TMON_INVALID_ARGUMENT: u32 = 2;
pub const TMON_INVALID_UTF8: u32 = 3;
pub const TMON_ENGINE_ERROR: u32 = 4;
pub const TMON_PANICKED: u32 = 5;

pub const TMON_COLOR_DEFAULT: u32 = 0;
pub const TMON_COLOR_INDEXED: u32 = 1;
pub const TMON_COLOR_RGB: u32 = 2;

pub const TMON_CURSOR_BLOCK: u32 = 0;
pub const TMON_CURSOR_UNDERLINE: u32 = 1;
pub const TMON_CURSOR_BAR: u32 = 2;

pub const TMON_ROW_MOVE_UP: u32 = 0;
pub const TMON_ROW_MOVE_DOWN: u32 = 1;

pub const TMON_CELL_BOLD: u16 = 1 << 0;
pub const TMON_CELL_DIM: u16 = 1 << 1;
pub const TMON_CELL_ITALIC: u16 = 1 << 2;
pub const TMON_CELL_UNDERLINE: u16 = 1 << 3;
pub const TMON_CELL_BLINK: u16 = 1 << 4;
pub const TMON_CELL_INVERSE: u16 = 1 << 5;
pub const TMON_CELL_HIDDEN: u16 = 1 << 6;
pub const TMON_CELL_STRIKEOUT: u16 = 1 << 7;
pub const TMON_CELL_DOUBLE_UNDERLINE: u16 = 1 << 8;
pub const TMON_CELL_WIDE: u16 = 1 << 9;
pub const TMON_CELL_WIDE_SPACER: u16 = 1 << 10;

pub const TMON_KEY_CHARACTER: u32 = 0;
pub const TMON_KEY_ESCAPE: u32 = 1;
pub const TMON_KEY_ENTER: u32 = 2;
pub const TMON_KEY_TAB: u32 = 3;
pub const TMON_KEY_BACKTAB: u32 = 4;
pub const TMON_KEY_BACKSPACE: u32 = 5;
pub const TMON_KEY_INSERT: u32 = 6;
pub const TMON_KEY_DELETE: u32 = 7;
pub const TMON_KEY_UP: u32 = 8;
pub const TMON_KEY_DOWN: u32 = 9;
pub const TMON_KEY_LEFT: u32 = 10;
pub const TMON_KEY_RIGHT: u32 = 11;
pub const TMON_KEY_PAGE_UP: u32 = 12;
pub const TMON_KEY_PAGE_DOWN: u32 = 13;
pub const TMON_KEY_HOME: u32 = 14;
pub const TMON_KEY_END: u32 = 15;
pub const TMON_KEY_FUNCTION: u32 = 16;
pub const TMON_KEY_KEYPAD: u32 = 17;
pub const TMON_KEY_CAPS_LOCK: u32 = 18;
pub const TMON_KEY_SCROLL_LOCK: u32 = 19;
pub const TMON_KEY_NUM_LOCK: u32 = 20;
pub const TMON_KEY_PRINT_SCREEN: u32 = 21;
pub const TMON_KEY_PAUSE: u32 = 22;
pub const TMON_KEY_MENU: u32 = 23;
pub const TMON_KEY_MEDIA: u32 = 24;
pub const TMON_KEY_MODIFIER: u32 = 25;

pub const TMON_KEYPAD_DIGIT_0: u32 = 0;
pub const TMON_KEYPAD_DIGIT_9: u32 = 9;
pub const TMON_KEYPAD_DECIMAL: u32 = 10;
pub const TMON_KEYPAD_DIVIDE: u32 = 11;
pub const TMON_KEYPAD_MULTIPLY: u32 = 12;
pub const TMON_KEYPAD_SUBTRACT: u32 = 13;
pub const TMON_KEYPAD_ADD: u32 = 14;
pub const TMON_KEYPAD_ENTER: u32 = 15;
pub const TMON_KEYPAD_EQUAL: u32 = 16;
pub const TMON_KEYPAD_SEPARATOR: u32 = 17;
pub const TMON_KEYPAD_LEFT: u32 = 18;
pub const TMON_KEYPAD_RIGHT: u32 = 19;
pub const TMON_KEYPAD_UP: u32 = 20;
pub const TMON_KEYPAD_DOWN: u32 = 21;
pub const TMON_KEYPAD_PAGE_UP: u32 = 22;
pub const TMON_KEYPAD_PAGE_DOWN: u32 = 23;
pub const TMON_KEYPAD_HOME: u32 = 24;
pub const TMON_KEYPAD_END: u32 = 25;
pub const TMON_KEYPAD_INSERT: u32 = 26;
pub const TMON_KEYPAD_DELETE: u32 = 27;
pub const TMON_KEYPAD_BEGIN: u32 = 28;

pub const TMON_MEDIA_PLAY: u32 = 0;
pub const TMON_MEDIA_PAUSE: u32 = 1;
pub const TMON_MEDIA_PLAY_PAUSE: u32 = 2;
pub const TMON_MEDIA_REVERSE: u32 = 3;
pub const TMON_MEDIA_STOP: u32 = 4;
pub const TMON_MEDIA_FAST_FORWARD: u32 = 5;
pub const TMON_MEDIA_REWIND: u32 = 6;
pub const TMON_MEDIA_TRACK_NEXT: u32 = 7;
pub const TMON_MEDIA_TRACK_PREVIOUS: u32 = 8;
pub const TMON_MEDIA_RECORD: u32 = 9;
pub const TMON_MEDIA_LOWER_VOLUME: u32 = 10;
pub const TMON_MEDIA_RAISE_VOLUME: u32 = 11;
pub const TMON_MEDIA_MUTE: u32 = 12;

pub const TMON_MODIFIER_KEY_LEFT_SHIFT: u32 = 0;
pub const TMON_MODIFIER_KEY_LEFT_CONTROL: u32 = 1;
pub const TMON_MODIFIER_KEY_LEFT_ALT: u32 = 2;
pub const TMON_MODIFIER_KEY_LEFT_SUPER: u32 = 3;
pub const TMON_MODIFIER_KEY_LEFT_HYPER: u32 = 4;
pub const TMON_MODIFIER_KEY_LEFT_META: u32 = 5;
pub const TMON_MODIFIER_KEY_RIGHT_SHIFT: u32 = 6;
pub const TMON_MODIFIER_KEY_RIGHT_CONTROL: u32 = 7;
pub const TMON_MODIFIER_KEY_RIGHT_ALT: u32 = 8;
pub const TMON_MODIFIER_KEY_RIGHT_SUPER: u32 = 9;
pub const TMON_MODIFIER_KEY_RIGHT_HYPER: u32 = 10;
pub const TMON_MODIFIER_KEY_RIGHT_META: u32 = 11;
pub const TMON_MODIFIER_KEY_ISO_LEVEL3_SHIFT: u32 = 12;
pub const TMON_MODIFIER_KEY_ISO_LEVEL5_SHIFT: u32 = 13;

pub const TMON_KEY_PRESS: u32 = 0;
pub const TMON_KEY_REPEAT: u32 = 1;
pub const TMON_KEY_RELEASE: u32 = 2;

pub const TMON_MOD_SHIFT: u32 = 1 << 0;
pub const TMON_MOD_ALT: u32 = 1 << 1;
pub const TMON_MOD_CONTROL: u32 = 1 << 2;
pub const TMON_MOD_SUPER: u32 = 1 << 3;
pub const TMON_MOD_HYPER: u32 = 1 << 4;
pub const TMON_MOD_META: u32 = 1 << 5;
pub const TMON_MOD_CAPS_LOCK: u32 = 1 << 6;
pub const TMON_MOD_NUM_LOCK: u32 = 1 << 7;

pub const TMON_MOUSE_BUTTON_NONE: u32 = 0;
pub const TMON_MOUSE_BUTTON_LEFT: u32 = 1;
pub const TMON_MOUSE_BUTTON_MIDDLE: u32 = 2;
pub const TMON_MOUSE_BUTTON_RIGHT: u32 = 3;
pub const TMON_MOUSE_BUTTON_WHEEL_UP: u32 = 4;
pub const TMON_MOUSE_BUTTON_WHEEL_DOWN: u32 = 5;

pub const TMON_MOUSE_PRESS: u32 = 0;
pub const TMON_MOUSE_RELEASE: u32 = 1;
pub const TMON_MOUSE_MOTION: u32 = 2;

pub const TMON_MOUSE_TRACKING_DISABLED: u32 = 0;
pub const TMON_MOUSE_TRACKING_PRESS: u32 = 1;
pub const TMON_MOUSE_TRACKING_BUTTON_MOTION: u32 = 2;
pub const TMON_MOUSE_TRACKING_ANY_MOTION: u32 = 3;

pub const TMON_SELECTION_CHARACTER: u32 = 0;
pub const TMON_SELECTION_WORD: u32 = 1;
pub const TMON_SELECTION_LINE: u32 = 2;

pub const TMON_SEARCH_FORWARD: u32 = 0;
pub const TMON_SEARCH_BACKWARD: u32 = 1;

pub const TMON_EVENT_BELL: u32 = 0;
pub const TMON_EVENT_TITLE: u32 = 1;
pub const TMON_EVENT_RESET_TITLE: u32 = 2;
pub const TMON_EVENT_CURRENT_DIRECTORY: u32 = 3;
pub const TMON_EVENT_CLIPBOARD_STORE: u32 = 4;
pub const TMON_EVENT_SET_DYNAMIC_COLOR: u32 = 5;
pub const TMON_EVENT_RESET_DYNAMIC_COLOR: u32 = 6;
pub const TMON_EVENT_MOUSE_POINTER_SHAPE: u32 = 7;
pub const TMON_EVENT_REPLY: u32 = 8;

pub const TMON_DYNAMIC_COLOR_FOREGROUND: u32 = 0;
pub const TMON_DYNAMIC_COLOR_BACKGROUND: u32 = 1;
pub const TMON_DYNAMIC_COLOR_CURSOR: u32 = 2;

pub const TMON_POINTER_DEFAULT: u32 = 0;
pub const TMON_POINTER_POINTER: u32 = 1;
pub const TMON_POINTER_TEXT: u32 = 2;
pub const TMON_POINTER_CROSSHAIR: u32 = 3;
pub const TMON_POINTER_MOVE: u32 = 4;
pub const TMON_POINTER_NOT_ALLOWED: u32 = 5;
pub const TMON_POINTER_HELP: u32 = 6;
pub const TMON_POINTER_PROGRESS: u32 = 7;
pub const TMON_POINTER_WAIT: u32 = 8;
pub const TMON_POINTER_CELL: u32 = 9;
pub const TMON_POINTER_VERTICAL_TEXT: u32 = 10;
pub const TMON_POINTER_ALIAS: u32 = 11;
pub const TMON_POINTER_COPY: u32 = 12;
pub const TMON_POINTER_NO_DROP: u32 = 13;
pub const TMON_POINTER_GRAB: u32 = 14;
pub const TMON_POINTER_GRABBING: u32 = 15;
pub const TMON_POINTER_E_RESIZE: u32 = 16;
pub const TMON_POINTER_N_RESIZE: u32 = 17;
pub const TMON_POINTER_NE_RESIZE: u32 = 18;
pub const TMON_POINTER_NW_RESIZE: u32 = 19;
pub const TMON_POINTER_S_RESIZE: u32 = 20;
pub const TMON_POINTER_SE_RESIZE: u32 = 21;
pub const TMON_POINTER_SW_RESIZE: u32 = 22;
pub const TMON_POINTER_W_RESIZE: u32 = 23;
pub const TMON_POINTER_EW_RESIZE: u32 = 24;
pub const TMON_POINTER_NS_RESIZE: u32 = 25;
pub const TMON_POINTER_NESW_RESIZE: u32 = 26;
pub const TMON_POINTER_NWSE_RESIZE: u32 = 27;
pub const TMON_POINTER_ZOOM_IN: u32 = 28;
pub const TMON_POINTER_ZOOM_OUT: u32 = 29;

pub const TMON_PTY_WAKE: u32 = 0;
pub const TMON_PTY_EXIT: u32 = 1;
pub const TMON_PTY_READ_ERROR: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TmonByteSlice {
    pub data: *const u8,
    pub length: usize,
}

impl TmonByteSlice {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            data: std::ptr::null(),
            length: 0,
        }
    }
}

impl Default for TmonByteSlice {
    fn default() -> Self {
        Self::empty()
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TmonRange {
    pub offset: usize,
    pub length: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TmonColor {
    pub kind: u32,
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub index: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TmonTerminalConfig {
    pub columns: usize,
    pub rows: usize,
    pub scrollback_limit: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TmonCursor {
    pub row: usize,
    pub column: usize,
    pub shape: u32,
    pub visible: u8,
    pub blinking: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TmonSelectionPoint {
    pub column: usize,
    pub row: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TmonSelectionRange {
    pub start: TmonSelectionPoint,
    pub end: TmonSelectionPoint,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TmonSearchOptions {
    pub direction: u32,
    pub case_sensitive: u8,
    pub wrap: u8,
}

impl Default for TmonSearchOptions {
    fn default() -> Self {
        Self {
            direction: TMON_SEARCH_BACKWARD,
            case_sensitive: 0,
            wrap: 1,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TmonSearchMatch {
    pub selection: TmonSelectionRange,
    pub display_offset: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TmonCell {
    pub text: TmonRange,
    pub hyperlink: TmonRange,
    pub foreground: TmonColor,
    pub background: TmonColor,
    pub flags: u16,
    pub has_hyperlink: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TmonRowUpdate {
    pub row: usize,
    pub start_column: usize,
    pub cell_offset: usize,
    pub cell_count: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TmonRowMove {
    pub start_row: usize,
    pub end_row: usize,
    pub direction: u32,
    pub count: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TmonFrameView {
    pub columns: usize,
    pub rows: usize,
    pub row_updates: *const TmonRowUpdate,
    pub row_update_count: usize,
    pub cells: *const TmonCell,
    pub cell_count: usize,
    pub text: TmonByteSlice,
    pub cursor: TmonCursor,
    pub selection: TmonSelectionRange,
    pub display_offset: usize,
    pub revision: u64,
    pub full: u8,
    pub metadata_changed: u8,
    pub has_selection: u8,
    pub row_moves: *const TmonRowMove,
    pub row_move_count: usize,
}

impl Default for TmonFrameView {
    fn default() -> Self {
        Self {
            columns: 0,
            rows: 0,
            row_updates: std::ptr::null(),
            row_update_count: 0,
            cells: std::ptr::null(),
            cell_count: 0,
            text: TmonByteSlice::empty(),
            cursor: TmonCursor::default(),
            selection: TmonSelectionRange::default(),
            display_offset: 0,
            revision: 0,
            full: 0,
            metadata_changed: 0,
            has_selection: 0,
            row_moves: std::ptr::null(),
            row_move_count: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TmonEvent {
    pub kind: u32,
    pub value: u32,
    pub color: TmonColor,
    pub primary: TmonRange,
    pub secondary: TmonRange,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TmonEventBatchView {
    pub events: *const TmonEvent,
    pub event_count: usize,
    pub data: TmonByteSlice,
}

impl Default for TmonEventBatchView {
    fn default() -> Self {
        Self {
            events: std::ptr::null(),
            event_count: 0,
            data: TmonByteSlice::empty(),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TmonKeyEvent {
    pub key_kind: u32,
    pub key_value: u32,
    pub modifiers: u32,
    pub event_kind: u32,
    pub text: TmonByteSlice,
    pub shifted_key: u32,
    pub base_layout_key: u32,
    pub has_text: u8,
    pub has_shifted_key: u8,
    pub has_base_layout_key: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TmonMouseEvent {
    pub button: u32,
    pub kind: u32,
    pub column: usize,
    pub row: usize,
    pub pixel_x: usize,
    pub pixel_y: usize,
    pub modifiers: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TmonTerminalMetrics {
    pub feed_calls: u64,
    pub bytes_fed: u64,
    pub frame_requests: u64,
    pub damaged_frames: u64,
    pub full_frames: u64,
    pub row_moves: u64,
    pub rows_moved: u64,
    pub row_updates: u64,
    pub cells_copied: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TmonMemoryStats {
    pub live_rows: u64,
    pub scrollback_rows: u64,
    pub spare_rows: u64,
    pub live_cell_capacity: u64,
    pub scrollback_cell_capacity: u64,
    pub spare_cell_capacity: u64,
    pub live_row_capacity: u64,
    pub scrollback_row_capacity: u64,
    pub damage_row_capacity: u64,
    pub damage_snapshot_capacity: u64,
    pub total_cell_capacity: u64,
    pub cell_capacity_bytes: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TmonPtyConfig {
    pub program: TmonByteSlice,
    pub arguments: *const TmonByteSlice,
    pub argument_count: usize,
    pub working_directory: TmonByteSlice,
    pub columns: usize,
    pub rows: usize,
    pub cell_width: f32,
    pub cell_height: f32,
    pub has_working_directory: u8,
}

impl Default for TmonPtyConfig {
    fn default() -> Self {
        Self {
            program: TmonByteSlice::empty(),
            arguments: std::ptr::null(),
            argument_count: 0,
            working_directory: TmonByteSlice::empty(),
            columns: 80,
            rows: 24,
            cell_width: 8.0,
            cell_height: 16.0,
            has_working_directory: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TmonPtyEvent {
    pub kind: u32,
    pub exit_code: u32,
    pub data: TmonByteSlice,
    pub has_data: u8,
}

pub type TmonPtyEventCallback =
    unsafe extern "C" fn(user_data: *mut c_void, event: *const TmonPtyEvent);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TmonPtyBufferMetrics {
    pub pending_bytes: u64,
    pub pending_capacity_bytes: u64,
    pub high_water_bytes: u64,
    pub bytes_buffered: u64,
    pub bytes_drained: u64,
    pub drain_calls: u64,
    pub producer_waits: u64,
    pub buffer_growths: u64,
    pub wake_events: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TmonPtyIoMetrics {
    pub resize_requests: u64,
    pub resize_ioctls: u64,
    pub resize_suppressed: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TmonOptionalU32 {
    pub value: u32,
    pub has_value: u8,
}
