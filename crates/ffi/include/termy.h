#ifndef TERMY_H
#define TERMY_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef enum {
  TERMY_FFI_OK = 0,
  TERMY_FFI_NULL = 1,
  TERMY_FFI_INVALID_UTF8 = 2,
  TERMY_FFI_SPAWN_FAILED = 3,
  TERMY_FFI_CONFIG_LOAD_FAILED = 4,
  TERMY_FFI_UNKNOWN_KEY = 5,
  TERMY_FFI_WRITE_FAILED = 6,
  TERMY_FFI_SERIALIZE_FAILED = 7,
  /* A Rust panic was caught before it could unwind across the C ABI. */
  TERMY_FFI_PANICKED = 8,
} TermyFfiStatus;

typedef enum {
  TERMY_FFI_EVENT_WAKEUP = 1,
  TERMY_FFI_EVENT_TITLE = 2,
  TERMY_FFI_EVENT_RESET_TITLE = 3,
  TERMY_FFI_EVENT_BELL = 4,
  TERMY_FFI_EVENT_EXIT = 5,
  TERMY_FFI_EVENT_CLIPBOARD_STORE = 6,
  TERMY_FFI_EVENT_SHELL_PROMPT_START = 7,
  TERMY_FFI_EVENT_SHELL_COMMAND_START = 8,
  TERMY_FFI_EVENT_SHELL_COMMAND_EXECUTING = 9,
  TERMY_FFI_EVENT_SHELL_COMMAND_FINISHED = 10,
  TERMY_FFI_EVENT_PROGRESS = 11,
  TERMY_FFI_EVENT_WORKING_DIRECTORY = 12,
} TermyFfiEventKind;

typedef enum {
  TERMY_FFI_PROGRESS_CLEAR = 0,
  TERMY_FFI_PROGRESS_IN_PROGRESS = 1,
  TERMY_FFI_PROGRESS_ERROR = 2,
  TERMY_FFI_PROGRESS_INDETERMINATE = 3,
  TERMY_FFI_PROGRESS_WARNING = 4,
} TermyFfiProgressState;

/*
 * Opaque terminal handle. Created by one of the *_new constructors below and
 * destroyed with the free function. Not copyable: free each handle exactly once
 * and never use it afterward.
 *
 * THREAD SAFETY. A handle is not internally synchronized. With one exception,
 * no two functions taking the same handle may run concurrently — the caller
 * must serialize all access to a given handle (confine it to one thread, or
 * hold an external lock). Distinct handles are independent and may be used from
 * different threads simultaneously.
 *
 * The exception is the wake channel: wait_for_wakeup and notify_wakeup touch
 * only an internal wake channel, never the terminal state, and are the only
 * functions safe to call concurrently with the serialized calls above. The
 * intended pattern is a single dedicated thread blocked in wait_for_wakeup
 * while another thread drives the terminal; notify_wakeup may be called from
 * any thread to wake that waiter.
 *
 * LIFETIME. The free function invalidates the handle and releases everything it
 * owns, including the wake channel. The caller MUST guarantee that no other
 * function — including a thread blocked in wait_for_wakeup — is executing on
 * the handle when it is freed, and that none is called afterward. To tear down
 * a handle that has a wakeup thread: stop issuing terminal calls, call
 * notify_wakeup to release the blocked wait, JOIN that thread, then free.
 * Freeing while a thread is inside wait_for_wakeup is a use-after-free.
 */
typedef struct TermyFfiTerminal TermyFfiTerminal;
typedef struct TermyFfiConfig TermyFfiConfig;

typedef struct {
  uint16_t cols;
  uint16_t rows;
  float cell_width;
  float cell_height;
} TermyFfiSize;

typedef struct {
  uint8_t r;
  uint8_t g;
  uint8_t b;
  uint8_t a;
} TermyFfiColor;

/* One viewport cell. Carries no position: full frames are row-major
 * (index = row * cols + col) and frame-update cells follow the dirty spans
 * in order, so the host derives position from context. */
typedef struct {
  uint32_t codepoint;
  TermyFfiColor fg;
  TermyFfiColor bg;
  bool uses_terminal_default_bg;
  bool bold;
  bool render_text;
  bool wide_character_spacer;
  bool line_wrapped;
  bool italic;
  bool underline;
  bool strikethrough;
} TermyFfiCell;

typedef struct {
  bool visible;
  size_t col;
  size_t row;
  uint32_t style;
} TermyFfiCursor;

typedef struct {
  uint16_t cols;
  uint16_t rows;
  TermyFfiCell *cells_ptr;
  size_t cells_len;
  size_t cells_capacity;
  TermyFfiCursor cursor;
  size_t display_offset;
  size_t history_size;
} TermyFfiFrame;

typedef struct {
  uint8_t *ptr;
  size_t len;
  size_t capacity;
} TermyFfiBytes;

typedef struct {
  uint64_t placement_serial;
  uint32_t image_id;
  uint32_t placement_id;
  TermyFfiBytes png;
  uint32_t image_width;
  uint32_t image_height;
  uint64_t image_generation;
  int32_t viewport_row;
  size_t col;
  uint32_t source_x;
  uint32_t source_y;
  uint32_t source_width;
  uint32_t source_height;
  bool has_display_cols;
  uint32_t display_cols;
  bool has_display_rows;
  uint32_t display_rows;
  uint32_t occupied_cols;
  uint32_t occupied_rows;
  uint32_t x_offset;
  uint32_t y_offset;
  int32_t z_index;
} TermyFfiKittyGraphicsPlacement;

typedef struct {
  uint64_t revision;
  TermyFfiKittyGraphicsPlacement *placements_ptr;
  size_t placements_len;
  size_t placements_capacity;
} TermyFfiKittyGraphicsBatch;

typedef struct {
  uint32_t kind;
  int32_t exit_code;
  uint8_t progress_state;
  uint8_t progress_value;
  TermyFfiBytes payload;
} TermyFfiEvent;

typedef struct {
  TermyFfiEvent *events_ptr;
  size_t events_len;
  size_t events_capacity;
  bool has_more;
} TermyFfiEventBatch;

typedef struct {
  size_t row;
  size_t left_col;
  size_t right_col;
} TermyFfiDirtySpan;

typedef struct {
  uint32_t kind;
  TermyFfiDirtySpan *spans_ptr;
  size_t spans_len;
  size_t spans_capacity;
} TermyFfiDamage;

typedef struct {
  uint16_t cols;
  uint16_t rows;
  TermyFfiCell *cells_ptr;
  size_t cells_len;
  size_t cells_capacity;
  TermyFfiCursor cursor;
  size_t display_offset;
  size_t history_size;
  uint32_t damage_kind;
  TermyFfiDirtySpan *spans_ptr;
  size_t spans_len;
  size_t spans_capacity;
} TermyFfiFrameUpdate;

typedef struct {
  size_t start_col;
  size_t end_col;
  TermyFfiBytes uri;
} TermyFfiHyperlink;

typedef struct {
  size_t row;
  size_t start_col;
  size_t end_col;
  TermyFfiBytes line;
} TermyFfiSearchMatch;

typedef struct {
  TermyFfiSearchMatch *matches_ptr;
  size_t matches_len;
  size_t matches_capacity;
} TermyFfiSearchBatch;

typedef struct {
  bool case_sensitive;
  bool regex;
} TermyFfiSearchOptions;

typedef struct {
  size_t line_number;
  uint32_t kind;
  TermyFfiBytes message;
} TermyFfiConfigDiagnostic;

typedef struct {
  TermyFfiConfigDiagnostic *diagnostics_ptr;
  size_t diagnostics_len;
  size_t diagnostics_capacity;
} TermyFfiConfigDiagnosticBatch;

typedef struct {
  TermyFfiBytes font_family;
  TermyFfiBytes active_theme;
  TermyFfiColor foreground;
  TermyFfiColor background;
  TermyFfiColor cursor;
  float font_size;
  float line_height;
  float padding_x;
  float padding_y;
  float background_opacity;
  bool background_opacity_cells;
  bool cursor_blink;
  uint32_t cursor_style;
  float cell_width;
  float cell_height;
  bool background_blur;
  float mouse_scroll_multiplier;
  uint32_t scrollbar_visibility;
  uint32_t scrollbar_style;
  bool copy_on_select;
  bool copy_on_select_toast;
  uint32_t pane_focus_effect;
  float pane_focus_strength;
  bool chrome_contrast;
} TermyFfiRenderConfig;

typedef struct {
  bool warn_on_quit;
  bool warn_on_quit_with_running_process;
} TermyFfiSafetyConfig;

typedef struct {
  bool auto_update;
  bool tmux_enabled;
  bool tmux_persistence;
  bool tmux_exclusive;
  bool tmux_show_active_pane_border;
  bool simple_mode;
  bool native_tab_persistence;
  bool native_layout_autosave;
  bool native_buffer_persistence;
  bool show_debug_overlay;
  bool onboarding_complete;
  uint32_t tab_close_visibility;
  uint32_t tab_width_mode;
  uint32_t tab_bar_position;
  uint32_t native_tab_placement;
  bool tab_switch_modifier_hints;
  bool chrome_contrast;
  bool command_palette_show_keybinds;
  uint32_t app_icon;
  bool shell_integration_enabled;
  bool progress_indicator_enabled;
  bool auto_hide_tabbar;
  bool show_termy_in_titlebar;
  bool macos_option_as_alt;
} TermyFfiNativeConfig;

typedef struct {
  const uint8_t *key_ptr;
  size_t key_len;
  const uint8_t *value_ptr;
  size_t value_len;
} TermyFfiEnvVar;

typedef struct {
  const TermyFfiConfig *config;
  const uint8_t *working_directory_ptr;
  size_t working_directory_len;
  const uint8_t *startup_command_ptr;
  size_t startup_command_len;
  const TermyFfiEnvVar *env_vars_ptr;
  size_t env_vars_len;
} TermyFfiTerminalOptions;

typedef struct {
  bool control;
  bool alt;
  bool shift;
  bool platform;
  bool function;
  const uint8_t *key_ptr;
  size_t key_len;
  const uint8_t *key_char_ptr;
  size_t key_char_len;
  uint32_t event_kind;
} TermyFfiKeystroke;

typedef struct {
  uint32_t kind;
  uint32_t button;
  size_t col;
  size_t row;
  bool control;
  bool alt;
  bool shift;
} TermyFfiMouseInput;

TermyFfiSize termy_size_default(void);
TermyFfiStatus termy_terminal_new(
    TermyFfiSize size,
    const uint8_t *startup_command_ptr,
    size_t startup_command_len,
    TermyFfiTerminal **out_terminal);
TermyFfiStatus termy_terminal_new_with_config(
    TermyFfiSize size,
    const TermyFfiConfig *config,
    const uint8_t *startup_command_ptr,
    size_t startup_command_len,
    TermyFfiTerminal **out_terminal);
TermyFfiStatus termy_terminal_new_with_options(
    TermyFfiSize size,
    const TermyFfiTerminalOptions *options,
    TermyFfiTerminal **out_terminal);
TermyFfiStatus termy_display_terminal_new(
    TermyFfiSize size,
    TermyFfiTerminal **out_terminal);
TermyFfiStatus termy_config_load_default(TermyFfiConfig **out_config);
TermyFfiStatus termy_config_load_path(
    const uint8_t *path_ptr,
    size_t path_len,
    TermyFfiConfig **out_config);
TermyFfiStatus termy_config_from_contents(
    const uint8_t *contents_ptr,
    size_t contents_len,
    TermyFfiConfig **out_config);
TermyFfiStatus termy_config_free(TermyFfiConfig *config);
bool termy_config_loaded_from_disk(const TermyFfiConfig *config);
size_t termy_config_runtime_scrollback_history(const TermyFfiConfig *config);
TermyFfiStatus termy_config_runtime_inactive_tab_scrollback(
    const TermyFfiConfig *config,
    bool *out_enabled,
    size_t *out_value);
size_t termy_config_diagnostic_count(const TermyFfiConfig *config);
TermyFfiStatus termy_config_window_size(
    const TermyFfiConfig *config,
    float *out_width,
    float *out_height);
TermyFfiStatus termy_config_working_directory(
    const TermyFfiConfig *config,
    TermyFfiBytes *out_working_directory);
TermyFfiStatus termy_config_safety(
    const TermyFfiConfig *config,
    TermyFfiSafetyConfig *out_safety);
TermyFfiStatus termy_config_native(
    const TermyFfiConfig *config,
    TermyFfiNativeConfig *out_native);
TermyFfiStatus termy_config_tmux_binary(
    const TermyFfiConfig *config,
    TermyFfiBytes *out_binary);
TermyFfiStatus termy_config_ui_font_family(
    const TermyFfiConfig *config,
    TermyFfiBytes *out_font_family);
TermyFfiStatus termy_config_path(
    const TermyFfiConfig *config,
    TermyFfiBytes *out_path);
TermyFfiStatus termy_config_tasks_json(
    const TermyFfiConfig *config,
    TermyFfiBytes *out_json);
TermyFfiStatus termy_config_keybinds_json(
    const TermyFfiConfig *config,
    TermyFfiBytes *out_json);
TermyFfiStatus termy_config_diagnostics(
    const TermyFfiConfig *config,
    TermyFfiConfigDiagnosticBatch *out_batch);
TermyFfiStatus termy_config_diagnostics_free(TermyFfiConfigDiagnosticBatch *batch);
TermyFfiStatus termy_config_render_config(
    const TermyFfiConfig *config,
    TermyFfiRenderConfig *out_render_config);
TermyFfiStatus termy_config_render_config_for_appearance(
    const TermyFfiConfig *config,
    uint32_t system_appearance,
    TermyFfiRenderConfig *out_render_config);
TermyFfiStatus termy_render_config_free(TermyFfiRenderConfig *render_config);
TermyFfiStatus termy_settings_schema_json(
    const TermyFfiConfig *config,
    TermyFfiBytes *out_bytes);
TermyFfiStatus termy_settings_set_root(
    const uint8_t *key_ptr,
    size_t key_len,
    const uint8_t *value_ptr,
    size_t value_len);
TermyFfiStatus termy_settings_reset_root(
    const uint8_t *key_ptr,
    size_t key_len);
TermyFfiStatus termy_settings_set_color(
    const uint8_t *key_ptr,
    size_t key_len,
    const uint8_t *hex_ptr,
    size_t hex_len);
TermyFfiStatus termy_settings_set_keybinds(
    const uint8_t *text_ptr,
    size_t text_len);
TermyFfiStatus termy_settings_install_theme(
    const uint8_t *slug_ptr,
    size_t slug_len);
TermyFfiStatus termy_cli_install(
    const uint8_t *shell_ptr,
    size_t shell_len,
    TermyFfiBytes *out_message);

typedef struct {
  uint32_t kind;
  TermyFfiBytes pane_id;
  TermyFfiBytes data;
} TermyFfiTmuxNotification;

typedef struct {
  TermyFfiTmuxNotification *notifications_ptr;
  size_t notifications_len;
  size_t notifications_capacity;
} TermyFfiTmuxNotificationBatch;

typedef struct TermyFfiTmuxControl TermyFfiTmuxControl;

TermyFfiStatus termy_tmux_control_open(
    const uint8_t *binary_ptr,
    size_t binary_len,
    const uint8_t *socket_ptr,
    size_t socket_len,
    const uint8_t *session_ptr,
    size_t session_len,
    TermyFfiTmuxControl **out_session);
TermyFfiStatus termy_tmux_control_poll(
    TermyFfiTmuxControl *session,
    TermyFfiTmuxNotificationBatch *out_batch);
TermyFfiStatus termy_tmux_control_notifications_free(
    TermyFfiTmuxNotificationBatch *batch);
TermyFfiStatus termy_tmux_control_send(
    TermyFfiTmuxControl *session,
    const uint8_t *command_ptr,
    size_t command_len,
    TermyFfiBytes *out_output);
void termy_tmux_control_close(TermyFfiTmuxControl *session);

TermyFfiStatus termy_settings_prettify_config(void);
TermyFfiStatus termy_terminal_reload_default_config_colors(TermyFfiTerminal *terminal);
TermyFfiStatus termy_terminal_apply_config_colors_for_appearance(
    TermyFfiTerminal *terminal,
    const TermyFfiConfig *config,
    uint32_t system_appearance);
/* Destroy a terminal handle. See the handle contract above: any wakeup thread
 * must be woken (notify) and joined before calling this; freeing while a thread
 * is inside wait_for_wakeup is a use-after-free. Passing null is a no-op. */
TermyFfiStatus termy_terminal_free(TermyFfiTerminal *terminal);
TermyFfiStatus termy_terminal_write(
    TermyFfiTerminal *terminal,
    const uint8_t *bytes_ptr,
    size_t bytes_len);
TermyFfiStatus termy_terminal_feed_output(
    TermyFfiTerminal *terminal,
    const uint8_t *bytes_ptr,
    size_t bytes_len);
TermyFfiStatus termy_terminal_encode_key(
    TermyFfiTerminal *terminal,
    const TermyFfiKeystroke *keystroke,
    TermyFfiBytes *out_bytes);
TermyFfiStatus termy_terminal_encode_key_with_options(
    TermyFfiTerminal *terminal,
    const TermyFfiKeystroke *keystroke,
    bool macos_option_as_alt,
    TermyFfiBytes *out_bytes);
TermyFfiStatus termy_terminal_encode_mouse(
    TermyFfiTerminal *terminal,
    const TermyFfiMouseInput *input,
    TermyFfiBytes *out_bytes);
TermyFfiStatus termy_terminal_resize(TermyFfiTerminal *terminal, TermyFfiSize size);
TermyFfiStatus termy_terminal_set_wakeup_enabled(
    TermyFfiTerminal *terminal,
    bool enabled);
/* Block up to timeout_ms (0 = poll) for a wake signal, coalescing pending
 * signals into one *out_woke. The one function meant to run on a dedicated
 * thread concurrently with serialized terminal calls; must not be running when
 * the handle is freed. */
TermyFfiStatus termy_terminal_wait_for_wakeup(
    TermyFfiTerminal *terminal,
    uint64_t timeout_ms,
    bool *out_woke);
TermyFfiStatus termy_terminal_notify_wakeup(TermyFfiTerminal *terminal);
TermyFfiStatus termy_terminal_scroll_display(
    TermyFfiTerminal *terminal,
    int32_t delta_lines,
    bool *out_changed);
TermyFfiStatus termy_terminal_scroll_to_bottom(
    TermyFfiTerminal *terminal,
    bool *out_changed);
TermyFfiStatus termy_terminal_clear_scrollback(
    TermyFfiTerminal *terminal,
    bool *out_changed);
TermyFfiStatus termy_terminal_set_scrollback_history(
    TermyFfiTerminal *terminal,
    size_t scrollback_history);
TermyFfiStatus termy_terminal_bracketed_paste_mode(
    TermyFfiTerminal *terminal,
    bool *out_enabled);
TermyFfiStatus termy_terminal_snapshot(
    TermyFfiTerminal *terminal,
    TermyFfiFrame *out_frame);
TermyFfiStatus termy_frame_free(TermyFfiFrame *frame);
TermyFfiStatus termy_terminal_take_frame_update(
    TermyFfiTerminal *terminal,
    bool force_full,
    TermyFfiFrameUpdate *out_update);
TermyFfiStatus termy_frame_update_free(TermyFfiFrameUpdate *update);
TermyFfiStatus termy_terminal_kitty_graphics_revision(
    TermyFfiTerminal *terminal,
    uint64_t *out_revision);
TermyFfiStatus termy_terminal_kitty_graphics_placements(
    TermyFfiTerminal *terminal,
    TermyFfiKittyGraphicsBatch *out_batch);
TermyFfiStatus termy_kitty_graphics_batch_free(
    TermyFfiKittyGraphicsBatch *batch);
TermyFfiStatus termy_terminal_hyperlink_at(
    TermyFfiTerminal *terminal,
    size_t row,
    size_t col,
    bool *out_found,
    TermyFfiHyperlink *out_link);
TermyFfiStatus termy_hyperlink_free(TermyFfiHyperlink *link);
TermyFfiStatus termy_terminal_take_damage(
    TermyFfiTerminal *terminal,
    TermyFfiDamage *out_damage);
TermyFfiStatus termy_damage_free(TermyFfiDamage *damage);
TermyFfiStatus termy_terminal_drain_events(
    TermyFfiTerminal *terminal,
    TermyFfiEventBatch *out_batch);
TermyFfiStatus termy_event_batch_free(TermyFfiEventBatch *batch);
TermyFfiStatus termy_terminal_search(
    TermyFfiTerminal *terminal,
    const uint8_t *query_ptr,
    size_t query_len,
    TermyFfiSearchBatch *out_batch);
TermyFfiStatus termy_terminal_search_with_options(
    TermyFfiTerminal *terminal,
    const uint8_t *query_ptr,
    size_t query_len,
    TermyFfiSearchOptions options,
    TermyFfiSearchBatch *out_batch);
TermyFfiStatus termy_search_batch_free(TermyFfiSearchBatch *batch);
TermyFfiStatus termy_buffer_free(TermyFfiBytes bytes);
TermyFfiBytes termy_null_buffer(void);
size_t termy_runtime_config_default_scrollback(void);
size_t termy_terminal_options_default_scrollback(void);
TermyFfiStatus termy_query_color_default_foreground(TermyFfiColor *out_color);

#ifdef __cplusplus
}
#endif

#endif
