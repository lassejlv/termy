#ifndef TMON_H
#define TMON_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define TMON_ABI_VERSION 0x00020000u

typedef uint32_t TmonStatus;
enum {
  TMON_OK = 0,
  TMON_NULL_POINTER = 1,
  TMON_INVALID_ARGUMENT = 2,
  TMON_INVALID_UTF8 = 3,
  TMON_ENGINE_ERROR = 4,
  TMON_PANICKED = 5,
};

typedef struct TmonTerminal TmonTerminal;
typedef struct TmonPty TmonPty;

typedef struct TmonByteSlice {
  const uint8_t *data;
  size_t length;
} TmonByteSlice;

typedef struct TmonRange {
  size_t offset;
  size_t length;
} TmonRange;

typedef uint32_t TmonColorKind;
enum {
  TMON_COLOR_DEFAULT = 0,
  TMON_COLOR_INDEXED = 1,
  TMON_COLOR_RGB = 2,
};

typedef struct TmonColor {
  TmonColorKind kind;
  uint8_t red;
  uint8_t green;
  uint8_t blue;
  uint8_t index;
} TmonColor;

typedef uint32_t TmonCursorShape;
enum {
  TMON_CURSOR_BLOCK = 0,
  TMON_CURSOR_UNDERLINE = 1,
  TMON_CURSOR_BAR = 2,
};

typedef uint16_t TmonCellFlags;
enum {
  TMON_CELL_BOLD = 1u << 0,
  TMON_CELL_DIM = 1u << 1,
  TMON_CELL_ITALIC = 1u << 2,
  TMON_CELL_UNDERLINE = 1u << 3,
  TMON_CELL_BLINK = 1u << 4,
  TMON_CELL_INVERSE = 1u << 5,
  TMON_CELL_HIDDEN = 1u << 6,
  TMON_CELL_STRIKEOUT = 1u << 7,
  TMON_CELL_DOUBLE_UNDERLINE = 1u << 8,
  TMON_CELL_WIDE = 1u << 9,
  TMON_CELL_WIDE_SPACER = 1u << 10,
};

typedef struct TmonTerminalConfig {
  size_t columns;
  size_t rows;
  size_t scrollback_limit;
} TmonTerminalConfig;

typedef struct TmonCursor {
  size_t row;
  size_t column;
  TmonCursorShape shape;
  uint8_t visible;
  uint8_t blinking;
} TmonCursor;

typedef struct TmonSelectionPoint {
  size_t column;
  size_t row;
} TmonSelectionPoint;

typedef struct TmonSelectionRange {
  TmonSelectionPoint start;
  TmonSelectionPoint end;
} TmonSelectionRange;

typedef uint32_t TmonSelectionMode;
enum {
  TMON_SELECTION_CHARACTER = 0,
  TMON_SELECTION_WORD = 1,
  TMON_SELECTION_LINE = 2,
};

typedef uint32_t TmonSearchDirection;
enum {
  TMON_SEARCH_FORWARD = 0,
  TMON_SEARCH_BACKWARD = 1,
};

typedef struct TmonSearchOptions {
  TmonSearchDirection direction;
  uint8_t case_sensitive;
  uint8_t wrap;
} TmonSearchOptions;

typedef struct TmonSearchMatch {
  TmonSelectionRange selection;
  size_t display_offset;
} TmonSearchMatch;

/* `text` and `hyperlink` are ranges in TmonFrameView.text. */
typedef struct TmonCell {
  TmonRange text;
  TmonRange hyperlink;
  TmonColor foreground;
  TmonColor background;
  TmonCellFlags flags;
  uint8_t has_hyperlink;
} TmonCell;

typedef struct TmonRowUpdate {
  size_t row;
  size_t start_column;
  size_t cell_offset;
  size_t cell_count;
} TmonRowUpdate;

typedef uint32_t TmonRowMoveDirection;
enum {
  TMON_ROW_MOVE_UP = 0,
  TMON_ROW_MOVE_DOWN = 1,
};

/* Moves retained rows inside [start_row, end_row) before applying row updates. */
typedef struct TmonRowMove {
  size_t start_row;
  size_t end_row;
  TmonRowMoveDirection direction;
  size_t count;
} TmonRowMove;

/*
 * A full update contains every row. A partial update patches each row span into the
 * host's retained grid. Pointers are borrowed from the terminal and remain valid until
 * the next tmon_terminal_frame_update call for that handle or handle destruction.
 */
typedef struct TmonFrameView {
  size_t columns;
  size_t rows;
  const TmonRowUpdate *row_updates;
  size_t row_update_count;
  const TmonCell *cells;
  size_t cell_count;
  TmonByteSlice text;
  TmonCursor cursor;
  TmonSelectionRange selection;
  size_t display_offset;
  uint64_t revision;
  uint8_t full;
  uint8_t metadata_changed;
  uint8_t has_selection;
  const TmonRowMove *row_moves;
  size_t row_move_count;
} TmonFrameView;

typedef uint32_t TmonEventKind;
enum {
  TMON_EVENT_BELL = 0,
  TMON_EVENT_TITLE = 1,
  TMON_EVENT_RESET_TITLE = 2,
  TMON_EVENT_CURRENT_DIRECTORY = 3,
  TMON_EVENT_CLIPBOARD_STORE = 4,
  TMON_EVENT_SET_DYNAMIC_COLOR = 5,
  TMON_EVENT_RESET_DYNAMIC_COLOR = 6,
  TMON_EVENT_MOUSE_POINTER_SHAPE = 7,
  TMON_EVENT_REPLY = 8,
};

typedef uint32_t TmonDynamicColor;
enum {
  TMON_DYNAMIC_COLOR_FOREGROUND = 0,
  TMON_DYNAMIC_COLOR_BACKGROUND = 1,
  TMON_DYNAMIC_COLOR_CURSOR = 2,
};

typedef uint32_t TmonPointerShape;
enum {
  TMON_POINTER_DEFAULT = 0,
  TMON_POINTER_POINTER = 1,
  TMON_POINTER_TEXT = 2,
  TMON_POINTER_CROSSHAIR = 3,
  TMON_POINTER_MOVE = 4,
  TMON_POINTER_NOT_ALLOWED = 5,
  TMON_POINTER_HELP = 6,
  TMON_POINTER_PROGRESS = 7,
  TMON_POINTER_WAIT = 8,
  TMON_POINTER_CELL = 9,
  TMON_POINTER_VERTICAL_TEXT = 10,
  TMON_POINTER_ALIAS = 11,
  TMON_POINTER_COPY = 12,
  TMON_POINTER_NO_DROP = 13,
  TMON_POINTER_GRAB = 14,
  TMON_POINTER_GRABBING = 15,
  TMON_POINTER_E_RESIZE = 16,
  TMON_POINTER_N_RESIZE = 17,
  TMON_POINTER_NE_RESIZE = 18,
  TMON_POINTER_NW_RESIZE = 19,
  TMON_POINTER_S_RESIZE = 20,
  TMON_POINTER_SE_RESIZE = 21,
  TMON_POINTER_SW_RESIZE = 22,
  TMON_POINTER_W_RESIZE = 23,
  TMON_POINTER_EW_RESIZE = 24,
  TMON_POINTER_NS_RESIZE = 25,
  TMON_POINTER_NESW_RESIZE = 26,
  TMON_POINTER_NWSE_RESIZE = 27,
  TMON_POINTER_ZOOM_IN = 28,
  TMON_POINTER_ZOOM_OUT = 29,
};

/*
 * `value` is a TmonDynamicColor or TmonPointerShape where applicable.
 * `primary` and `secondary` are ranges in TmonEventBatchView.data.
 */
typedef struct TmonEvent {
  TmonEventKind kind;
  uint32_t value;
  TmonColor color;
  TmonRange primary;
  TmonRange secondary;
} TmonEvent;

/* Borrowed until the next event drain for this terminal or handle destruction. */
typedef struct TmonEventBatchView {
  const TmonEvent *events;
  size_t event_count;
  TmonByteSlice data;
} TmonEventBatchView;

typedef uint32_t TmonKeyKind;
enum {
  TMON_KEY_CHARACTER = 0,
  TMON_KEY_ESCAPE = 1,
  TMON_KEY_ENTER = 2,
  TMON_KEY_TAB = 3,
  TMON_KEY_BACKTAB = 4,
  TMON_KEY_BACKSPACE = 5,
  TMON_KEY_INSERT = 6,
  TMON_KEY_DELETE = 7,
  TMON_KEY_UP = 8,
  TMON_KEY_DOWN = 9,
  TMON_KEY_LEFT = 10,
  TMON_KEY_RIGHT = 11,
  TMON_KEY_PAGE_UP = 12,
  TMON_KEY_PAGE_DOWN = 13,
  TMON_KEY_HOME = 14,
  TMON_KEY_END = 15,
  TMON_KEY_FUNCTION = 16,
  TMON_KEY_KEYPAD = 17,
  TMON_KEY_CAPS_LOCK = 18,
  TMON_KEY_SCROLL_LOCK = 19,
  TMON_KEY_NUM_LOCK = 20,
  TMON_KEY_PRINT_SCREEN = 21,
  TMON_KEY_PAUSE = 22,
  TMON_KEY_MENU = 23,
  TMON_KEY_MEDIA = 24,
  TMON_KEY_MODIFIER = 25,
};

typedef uint32_t TmonKeypadKey;
enum {
  TMON_KEYPAD_DIGIT_0 = 0,
  TMON_KEYPAD_DIGIT_1 = 1,
  TMON_KEYPAD_DIGIT_2 = 2,
  TMON_KEYPAD_DIGIT_3 = 3,
  TMON_KEYPAD_DIGIT_4 = 4,
  TMON_KEYPAD_DIGIT_5 = 5,
  TMON_KEYPAD_DIGIT_6 = 6,
  TMON_KEYPAD_DIGIT_7 = 7,
  TMON_KEYPAD_DIGIT_8 = 8,
  TMON_KEYPAD_DIGIT_9 = 9,
  TMON_KEYPAD_DECIMAL = 10,
  TMON_KEYPAD_DIVIDE = 11,
  TMON_KEYPAD_MULTIPLY = 12,
  TMON_KEYPAD_SUBTRACT = 13,
  TMON_KEYPAD_ADD = 14,
  TMON_KEYPAD_ENTER = 15,
  TMON_KEYPAD_EQUAL = 16,
  TMON_KEYPAD_SEPARATOR = 17,
  TMON_KEYPAD_LEFT = 18,
  TMON_KEYPAD_RIGHT = 19,
  TMON_KEYPAD_UP = 20,
  TMON_KEYPAD_DOWN = 21,
  TMON_KEYPAD_PAGE_UP = 22,
  TMON_KEYPAD_PAGE_DOWN = 23,
  TMON_KEYPAD_HOME = 24,
  TMON_KEYPAD_END = 25,
  TMON_KEYPAD_INSERT = 26,
  TMON_KEYPAD_DELETE = 27,
  TMON_KEYPAD_BEGIN = 28,
};

typedef uint32_t TmonMediaKey;
enum {
  TMON_MEDIA_PLAY = 0,
  TMON_MEDIA_PAUSE = 1,
  TMON_MEDIA_PLAY_PAUSE = 2,
  TMON_MEDIA_REVERSE = 3,
  TMON_MEDIA_STOP = 4,
  TMON_MEDIA_FAST_FORWARD = 5,
  TMON_MEDIA_REWIND = 6,
  TMON_MEDIA_TRACK_NEXT = 7,
  TMON_MEDIA_TRACK_PREVIOUS = 8,
  TMON_MEDIA_RECORD = 9,
  TMON_MEDIA_LOWER_VOLUME = 10,
  TMON_MEDIA_RAISE_VOLUME = 11,
  TMON_MEDIA_MUTE = 12,
};

typedef uint32_t TmonModifierKey;
enum {
  TMON_MODIFIER_KEY_LEFT_SHIFT = 0,
  TMON_MODIFIER_KEY_LEFT_CONTROL = 1,
  TMON_MODIFIER_KEY_LEFT_ALT = 2,
  TMON_MODIFIER_KEY_LEFT_SUPER = 3,
  TMON_MODIFIER_KEY_LEFT_HYPER = 4,
  TMON_MODIFIER_KEY_LEFT_META = 5,
  TMON_MODIFIER_KEY_RIGHT_SHIFT = 6,
  TMON_MODIFIER_KEY_RIGHT_CONTROL = 7,
  TMON_MODIFIER_KEY_RIGHT_ALT = 8,
  TMON_MODIFIER_KEY_RIGHT_SUPER = 9,
  TMON_MODIFIER_KEY_RIGHT_HYPER = 10,
  TMON_MODIFIER_KEY_RIGHT_META = 11,
  TMON_MODIFIER_KEY_ISO_LEVEL3_SHIFT = 12,
  TMON_MODIFIER_KEY_ISO_LEVEL5_SHIFT = 13,
};

typedef uint32_t TmonKeyEventKind;
enum {
  TMON_KEY_PRESS = 0,
  TMON_KEY_REPEAT = 1,
  TMON_KEY_RELEASE = 2,
};

typedef uint32_t TmonModifiers;
enum {
  TMON_MOD_SHIFT = 1u << 0,
  TMON_MOD_ALT = 1u << 1,
  TMON_MOD_CONTROL = 1u << 2,
  TMON_MOD_SUPER = 1u << 3,
  TMON_MOD_HYPER = 1u << 4,
  TMON_MOD_META = 1u << 5,
  TMON_MOD_CAPS_LOCK = 1u << 6,
  TMON_MOD_NUM_LOCK = 1u << 7,
};

/*
 * key_value is a Unicode scalar, function number, TmonKeypadKey,
 * TmonMediaKey, or TmonModifierKey according to key_kind.
 * `text` is copied during the call when has_text is nonzero.
 */
typedef struct TmonKeyEvent {
  TmonKeyKind key_kind;
  uint32_t key_value;
  TmonModifiers modifiers;
  TmonKeyEventKind event_kind;
  TmonByteSlice text;
  uint32_t shifted_key;
  uint32_t base_layout_key;
  uint8_t has_text;
  uint8_t has_shifted_key;
  uint8_t has_base_layout_key;
} TmonKeyEvent;

typedef uint32_t TmonMouseButton;
enum {
  TMON_MOUSE_BUTTON_NONE = 0,
  TMON_MOUSE_BUTTON_LEFT = 1,
  TMON_MOUSE_BUTTON_MIDDLE = 2,
  TMON_MOUSE_BUTTON_RIGHT = 3,
  TMON_MOUSE_BUTTON_WHEEL_UP = 4,
  TMON_MOUSE_BUTTON_WHEEL_DOWN = 5,
};

typedef uint32_t TmonMouseEventKind;
enum {
  TMON_MOUSE_PRESS = 0,
  TMON_MOUSE_RELEASE = 1,
  TMON_MOUSE_MOTION = 2,
};

typedef struct TmonMouseEvent {
  TmonMouseButton button;
  TmonMouseEventKind kind;
  size_t column;
  size_t row;
  size_t pixel_x;
  size_t pixel_y;
  TmonModifiers modifiers;
} TmonMouseEvent;

typedef uint32_t TmonMouseTrackingMode;
enum {
  TMON_MOUSE_TRACKING_DISABLED = 0,
  TMON_MOUSE_TRACKING_PRESS = 1,
  TMON_MOUSE_TRACKING_BUTTON_MOTION = 2,
  TMON_MOUSE_TRACKING_ANY_MOTION = 3,
};

typedef struct TmonTerminalMetrics {
  uint64_t feed_calls;
  uint64_t bytes_fed;
  uint64_t frame_requests;
  uint64_t damaged_frames;
  uint64_t full_frames;
  uint64_t row_moves;
  uint64_t rows_moved;
  uint64_t row_updates;
  uint64_t cells_copied;
} TmonTerminalMetrics;

typedef struct TmonMemoryStats {
  uint64_t live_rows;
  uint64_t scrollback_rows;
  uint64_t spare_rows;
  uint64_t live_cell_capacity;
  uint64_t scrollback_cell_capacity;
  uint64_t spare_cell_capacity;
  uint64_t live_row_capacity;
  uint64_t scrollback_row_capacity;
  uint64_t damage_row_capacity;
  uint64_t damage_snapshot_capacity;
  uint64_t total_cell_capacity;
  uint64_t cell_capacity_bytes;
} TmonMemoryStats;

typedef struct TmonPtyConfig {
  TmonByteSlice program;
  const TmonByteSlice *arguments;
  size_t argument_count;
  TmonByteSlice working_directory;
  size_t columns;
  size_t rows;
  float cell_width;
  float cell_height;
  uint8_t has_working_directory;
} TmonPtyConfig;

typedef uint32_t TmonPtyEventKind;
enum {
  TMON_PTY_WAKE = 0,
  TMON_PTY_EXIT = 1,
  TMON_PTY_READ_ERROR = 2,
};

/* `data` is borrowed only for the duration of the callback. */
typedef struct TmonPtyEvent {
  TmonPtyEventKind kind;
  uint32_t exit_code;
  TmonByteSlice data;
  uint8_t has_data;
} TmonPtyEvent;

typedef void (*TmonPtyEventCallback)(void *user_data,
                                         const TmonPtyEvent *event);

typedef struct TmonPtyBufferMetrics {
  uint64_t pending_bytes;
  uint64_t pending_capacity_bytes;
  uint64_t high_water_bytes;
  uint64_t bytes_buffered;
  uint64_t bytes_drained;
  uint64_t drain_calls;
  uint64_t producer_waits;
  uint64_t buffer_growths;
  uint64_t wake_events;
} TmonPtyBufferMetrics;

typedef struct TmonPtyIoMetrics {
  uint64_t resize_requests;
  uint64_t resize_ioctls;
  uint64_t resize_suppressed;
} TmonPtyIoMetrics;

typedef struct TmonOptionalU32 {
  uint32_t value;
  uint8_t has_value;
} TmonOptionalU32;

uint32_t tmon_abi_version(void);
TmonByteSlice tmon_library_version(void);
/* Borrowed thread-local NUL-terminated text. */
const char *tmon_last_error_message(void);

TmonTerminalConfig tmon_terminal_config_default(void);
TmonStatus tmon_terminal_new(const TmonTerminalConfig *config,
                                       TmonTerminal **out_terminal);
TmonStatus tmon_terminal_free(TmonTerminal *terminal);
TmonStatus tmon_terminal_feed(TmonTerminal *terminal,
                                        const uint8_t *bytes, size_t length);
TmonStatus tmon_terminal_resize(TmonTerminal *terminal,
                                          size_t columns, size_t rows);
TmonStatus tmon_terminal_set_pixel_size(TmonTerminal *terminal,
                                                  uint32_t width,
                                                  uint32_t height);
TmonStatus
tmon_terminal_set_scrollback_limit(TmonTerminal *terminal,
                                        size_t limit);
TmonStatus tmon_terminal_frame_update(TmonTerminal *terminal,
                                                uint8_t force_full,
                                                TmonFrameView *out_frame);
TmonStatus
tmon_terminal_drain_events(TmonTerminal *terminal,
                                TmonEventBatchView *out_events);

/* Encoded output is borrowed until the next encode/focus call or handle free. */
TmonStatus tmon_terminal_encode_key(TmonTerminal *terminal,
                                              const TmonKeyEvent *event,
                                              TmonByteSlice *out_bytes);
TmonStatus tmon_terminal_encode_text(TmonTerminal *terminal,
                                               TmonByteSlice text,
                                               TmonByteSlice *out_bytes);
TmonStatus tmon_terminal_encode_paste(TmonTerminal *terminal,
                                                TmonByteSlice text,
                                                TmonByteSlice *out_bytes);
TmonStatus tmon_terminal_encode_mouse(
    TmonTerminal *terminal, const TmonMouseEvent *event,
    TmonByteSlice *out_bytes, uint8_t *out_has_value);
TmonStatus tmon_terminal_focus_changed(
    TmonTerminal *terminal, uint8_t focused, TmonByteSlice *out_bytes,
    uint8_t *out_has_value);

TmonStatus tmon_terminal_scroll_display(TmonTerminal *terminal,
                                                  int64_t lines,
                                                  uint8_t *out_changed);
TmonStatus
tmon_terminal_scroll_to_bottom(TmonTerminal *terminal);
TmonStatus tmon_terminal_begin_selection(
    TmonTerminal *terminal, TmonSelectionPoint point,
    uint8_t *out_changed);
TmonStatus tmon_terminal_begin_selection_with_mode(
    TmonTerminal *terminal, TmonSelectionPoint point,
    TmonSelectionMode mode, uint8_t *out_changed);
TmonStatus tmon_terminal_update_selection(
    TmonTerminal *terminal, TmonSelectionPoint point,
    uint8_t *out_changed);
TmonStatus tmon_terminal_clear_selection(TmonTerminal *terminal,
                                                   uint8_t *out_changed);
TmonSearchOptions tmon_search_options_default(void);
TmonStatus tmon_terminal_search(
    TmonTerminal *terminal, TmonByteSlice query,
    TmonSearchOptions options, TmonSearchMatch *out_match,
    uint8_t *out_has_value);
TmonStatus tmon_terminal_reset_search(TmonTerminal *terminal,
                                                uint8_t *out_changed);
/* Selected text is borrowed until the next selected-text call or handle free. */
TmonStatus tmon_terminal_selected_text(
    TmonTerminal *terminal, TmonByteSlice *out_text,
    uint8_t *out_has_value);
TmonStatus tmon_terminal_mouse_tracking_mode(
    const TmonTerminal *terminal, TmonMouseTrackingMode *out_mode);
TmonStatus tmon_terminal_metrics(
    const TmonTerminal *terminal, TmonTerminalMetrics *out_metrics);
TmonStatus
tmon_terminal_reset_metrics(TmonTerminal *terminal);
TmonStatus tmon_terminal_memory_stats(
    const TmonTerminal *terminal, TmonMemoryStats *out_stats);

TmonPtyConfig tmon_pty_config_default(void);
/*
 * The callback may run on the PTY reader thread. It must return quickly and normally
 * and only schedule host event-loop work. It must not call or free the same PTY handle.
 * Events produced during spawn are delivered after out_pty has been populated. Keep
 * user_data alive until tmon_pty_free returns.
 */
TmonStatus tmon_pty_spawn(const TmonPtyConfig *config,
                                    TmonPtyEventCallback callback,
                                    void *user_data, TmonPty **out_pty);
TmonStatus tmon_pty_free(TmonPty *pty);
/* Borrowed until the next drain for this PTY or handle destruction. */
TmonStatus tmon_pty_drain_output(TmonPty *pty,
                                           TmonByteSlice *out_bytes);
TmonStatus tmon_pty_write(const TmonPty *pty,
                                    const uint8_t *bytes, size_t length);
TmonStatus tmon_pty_resize(const TmonPty *pty, size_t columns,
                                     size_t rows, float cell_width,
                                     float cell_height);
TmonStatus tmon_pty_child_pid(const TmonPty *pty,
                                        TmonOptionalU32 *out_pid);
TmonStatus tmon_pty_buffer_metrics(
    const TmonPty *pty, TmonPtyBufferMetrics *out_metrics);
TmonStatus tmon_pty_io_metrics(const TmonPty *pty,
                                         TmonPtyIoMetrics *out_metrics);

/*
 * Serialize calls made against one handle and never free it while a call is active.
 * Input slices are borrowed only for the duration of a call. Copy any borrowed output
 * that must outlive the validity window documented above.
 */

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* TMON_H */
