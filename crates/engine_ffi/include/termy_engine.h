/*
 * libtermy_engine: Termy's terminal engine as a C library.
 *
 * The library is a VT parser, screens, scrollback and protocol state, and
 * nothing else: no PTY, config files, threads, timers, fonts or network.
 * The host feeds program output in and reads cells, state, protocol replies
 * and encoded input bytes out. Link libtermy_engine.a (static) or the shared
 * library built from crates/engine_ffi.
 *
 * CONVENTIONS
 *   - Functions return TermyEngineStatus; outputs go through pointers.
 *   - (ptr, len) inputs may be (NULL, 0).
 *   - TermyEngineBytes outputs are owned by the caller and released with
 *     termy_engine_bytes_free(). An empty result has ptr == NULL.
 *   - A handle is not internally synchronized: serialize all calls on one
 *     handle. Distinct handles are independent.
 *   - Rust panics are caught and reported as TERMY_ENGINE_PANICKED.
 *   - Check termy_engine_abi_version() == TERMY_ENGINE_ABI_VERSION at
 *     startup; structs and enums below change only with that version.
 */
#ifndef TERMY_ENGINE_H
#define TERMY_ENGINE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define TERMY_ENGINE_ABI_VERSION 2

typedef enum {
  TERMY_ENGINE_OK = 0,
  TERMY_ENGINE_NULL = 1,
  TERMY_ENGINE_INVALID_ARGUMENT = 2,
  TERMY_ENGINE_PANICKED = 3,
  TERMY_ENGINE_NOT_A_SNAPSHOT = 4,
  TERMY_ENGINE_UNSUPPORTED_SNAPSHOT_VERSION = 5,
  TERMY_ENGINE_TRUNCATED_SNAPSHOT = 6,
  TERMY_ENGINE_CORRUPT_SNAPSHOT = 7,
} TermyEngineStatus;

typedef struct TermyEngine TermyEngine;

typedef struct {
  uint8_t *ptr;
  size_t len;
  size_t capacity;
} TermyEngineBytes;

uint32_t termy_engine_abi_version(void);
/* Releases the buffer and resets it to empty; NULL and empty are no-ops. */
void termy_engine_bytes_free(TermyEngineBytes *bytes);

/* ------------------------------------------------------------------------
 * Lifecycle and output
 * ------------------------------------------------------------------------ */

/* The authoritative terminal answers queries (DA, DSR, DECRQM, OSC color
 * and status reports, XTVERSION-style DCS). Replies are produced
 * synchronously while parsing, in byte order: for "CSI 5n CSI c" the status
 * report precedes the DA reply. */
#define TERMY_ENGINE_REPLIES_RETURN 0
/* Replica mode: queries are parsed and change no state, and every reply is
 * dropped. Use it for terminals mirroring another terminal's output. */
#define TERMY_ENGINE_REPLIES_DISCARD 1

/* scrollback_rows value with no row limit: history is bounded by
 * scrollback_bytes alone. */
#define TERMY_ENGINE_SCROLLBACK_UNLIMITED UINT32_MAX

typedef struct {
  uint16_t cols;
  uint16_t rows;
  /* Scrollback rows, or TERMY_ENGINE_SCROLLBACK_UNLIMITED; see "Scrollback
   * limit" below. */
  uint32_t scrollback_rows;
  uint32_t reply_mode;
  /* Recognize ?1016 (SGR pixel mouse reports). Only for hosts that report
   * pointer pixels; a server and its replicas must agree. */
  bool pixel_mouse_reports;
  /* Cell size in pixels, for XTWINOPS size reports. */
  uint16_t cell_width_px;
  uint16_t cell_height_px;
  /* Scrollback byte limit; 0 selects the default, 16 MiB. */
  uint64_t scrollback_bytes;
  /* OSC and DCS payloads longer than max_string_bytes, and OSC 52 payloads
   * longer than max_clipboard_bytes, are dropped whole. 0 selects the
   * default; the defaults (64 KiB and 8 MiB) are also the maxima. */
  uint32_t max_string_bytes;
  uint32_t max_clipboard_bytes;
} TermyEngineOptions;

/* 80x24, 10000 rows and 16 MiB of scrollback, replies returned, no pixel
 * reports, default payload limits. */
TermyEngineOptions termy_engine_options_default(void);
/* options may be NULL for the defaults. */
TermyEngineStatus termy_engine_new(
    const TermyEngineOptions *options,
    TermyEngine **out_engine);
void termy_engine_free(TermyEngine *engine);
TermyEngineStatus termy_engine_set_reply_mode(TermyEngine *engine, uint32_t reply_mode);

/* Parse program output. In TERMY_ENGINE_REPLIES_RETURN mode, out_replies
 * receives every reply the bytes produced (and any still queued), in order;
 * pass NULL to leave them queued for termy_engine_take_replies(). In
 * TERMY_ENGINE_REPLIES_DISCARD mode replies are dropped and out_replies is
 * set empty. No allocation happens for output that produces no reply.
 *
 * Synchronized output (?2026) buffers bytes until the update ends; queries
 * inside it are answered when it commits, still in order. */
TermyEngineStatus termy_engine_feed(
    TermyEngine *engine,
    const uint8_t *bytes_ptr,
    size_t bytes_len,
    TermyEngineBytes *out_replies);
TermyEngineStatus termy_engine_take_replies(
    TermyEngine *engine,
    TermyEngineBytes *out_replies);

/* A pending synchronized update must commit within its deadline even if
 * the program never ends it. The library has no timer: hosts poll the
 * deadline and call termy_engine_tick() once it passes (force commits now).
 * out_replies may be NULL. */
TermyEngineStatus termy_engine_synchronized_deadline(
    TermyEngine *engine,
    bool *out_pending,
    uint64_t *out_remaining_ms);
TermyEngineStatus termy_engine_tick(
    TermyEngine *engine,
    bool force,
    bool *out_committed,
    TermyEngineBytes *out_replies);

TermyEngineStatus termy_engine_resize(TermyEngine *engine, uint16_t cols, uint16_t rows);
TermyEngineStatus termy_engine_set_cell_pixels(TermyEngine *engine, uint16_t width, uint16_t height);
TermyEngineStatus termy_engine_set_scrollback(TermyEngine *engine, uint32_t rows);
/* 0 selects the default (16 MiB). */
TermyEngineStatus termy_engine_set_scrollback_bytes(TermyEngine *engine, uint64_t bytes);
/* See TermyEngineOptions; 0 selects a default. A server and its replicas
 * should use the same limits (snapshots carry them). */
TermyEngineStatus termy_engine_set_payload_limits(
    TermyEngine *engine,
    uint32_t max_string_bytes,
    uint32_t max_clipboard_bytes);
TermyEngineStatus termy_engine_clear_scrollback(TermyEngine *engine);
/* Pack cold scrollback after output goes quiet. Optional; it changes no
 * content, but it changes how later scrolled rows are stored, which counts
 * toward the history byte limit. A server and its replicas should not
 * call it (the engine packs history itself when the budget requires), so
 * that both evict the same rows. */
TermyEngineStatus termy_engine_compact_history(TermyEngine *engine);
/* Tell the engine the program exited (ends in-flight program status). */
TermyEngineStatus termy_engine_process_exited(TermyEngine *engine);

/* ------------------------------------------------------------------------
 * Snapshots
 *
 * A snapshot holds the full terminal state: both screens and all scrollback
 * (cells, grapheme clusters, styles, hyperlinks, wrap flags), cursor (with
 * style, visibility and pending wrap), saved cursors (DECSC), scroll region,
 * origin mode, margins, tab stops, charsets, all DEC/ANSI modes including
 * mouse tracking/encoding, bracketed paste, focus, alternate scroll, DECCKM,
 * DECKPAM and the kitty keyboard flag stack of each screen, OSC 4/10/11/12
 * overrides, the title and title stack, the OSC 7 directory, OSC 7501
 * program status, the parser's in-progress sequence (mid-escape, mid-string,
 * mid-UTF-8) and a pending synchronized update.
 *
 * decode(encode(t)) fed bytes B ends in the same state as t fed B,
 * including scrollback eviction. Not included: queued replies and events
 * (drain them first) and render damage (a decoded engine starts with a full
 * frame). Kitty graphics (images, animations, placements) are included from
 * version 2.
 *
 * Format: "TRMYSNAP", u32 version, u32 flags, u64 table offset, all
 * little-endian, so snapshots move between Linux x86_64/aarch64 and macOS
 * arm64 unchanged. Encoding 16 MiB of scrollback takes tens of
 * milliseconds in release builds.
 *
 * Versions: decode reads every version from termy_engine_snapshot_min_version()
 * through termy_engine_snapshot_version(); others fail with
 * TERMY_ENGINE_UNSUPPORTED_SNAPSHOT_VERSION. When the two ends of an attach
 * run different releases, the newer side adapts: a newer client decodes an
 * older server's snapshot as is, and a newer server encodes for an older
 * client with termy_engine_snapshot_encode_version(), after learning the
 * client's termy_engine_snapshot_version() in the attach handshake.
 *
 *   1  first format.
 *   2  adds the scrollback byte limit (rows past 20000), payload limits and
 *      kitty graphics.
 *
 * Writing version 1 keeps the newest 20000 history rows and drops kitty
 * graphics, and the client gets the default byte and payload limits. Engines of different releases
 * may charge history rows differently, so eviction is only guaranteed to
 * match between a server and replicas on the same release.
 * ------------------------------------------------------------------------ */

uint32_t termy_engine_snapshot_version(void);
uint32_t termy_engine_snapshot_min_version(void);
TermyEngineStatus termy_engine_snapshot_encode(
    TermyEngine *engine,
    TermyEngineBytes *out_snapshot);
/* Encode in an older format; out_snapshot is empty on error. */
TermyEngineStatus termy_engine_snapshot_encode_version(
    TermyEngine *engine,
    uint32_t version,
    TermyEngineBytes *out_snapshot);
TermyEngineStatus termy_engine_snapshot_peek_version(
    const uint8_t *snapshot_ptr,
    size_t snapshot_len,
    uint32_t *out_version);
/* Create a new handle from a snapshot. Host-side settings (reply mode and
 * the colors from termy_engine_set_default_colors) are not part of a
 * snapshot; the OSC color query answers are. */
TermyEngineStatus termy_engine_snapshot_decode(
    const uint8_t *snapshot_ptr,
    size_t snapshot_len,
    uint32_t reply_mode,
    TermyEngine **out_engine);

/* ------------------------------------------------------------------------
 * State
 * ------------------------------------------------------------------------ */

/* DECRQM for one mode: ansi selects ANSI modes (IRM 4, LNM 20) rather than
 * DEC private modes. *out_report is 0 not recognized, 1 set, 2 reset,
 * 3 permanently set. */
TermyEngineStatus termy_engine_mode(
    TermyEngine *engine,
    uint16_t mode,
    bool ansi,
    uint8_t *out_report);

#define TERMY_ENGINE_CURSOR_BLOCK 0
#define TERMY_ENGINE_CURSOR_BAR 1
#define TERMY_ENGINE_CURSOR_UNDERLINE 2

#define TERMY_ENGINE_MOUSE_TRACKING_OFF 0
#define TERMY_ENGINE_MOUSE_TRACKING_X10 1    /* ?9 */
#define TERMY_ENGINE_MOUSE_TRACKING_NORMAL 2 /* ?1000 */
#define TERMY_ENGINE_MOUSE_TRACKING_BUTTON 3 /* ?1002 */
#define TERMY_ENGINE_MOUSE_TRACKING_ANY 4    /* ?1003 */

#define TERMY_ENGINE_MOUSE_ENCODING_X10 0
#define TERMY_ENGINE_MOUSE_ENCODING_UTF8 1        /* ?1005 */
#define TERMY_ENGINE_MOUSE_ENCODING_SGR 2         /* ?1006 */
#define TERMY_ENGINE_MOUSE_ENCODING_SGR_PIXELS 3  /* ?1016 */

typedef struct {
  uint16_t cols;
  uint16_t rows;
  /* In the active screen, independent of scrollback. */
  uint16_t cursor_col;
  uint16_t cursor_row;
  bool cursor_visible;
  bool cursor_blinking;
  uint8_t cursor_shape;
  bool alternate_screen;
  uint8_t mouse_tracking;
  uint8_t mouse_encoding;
  bool focus_reporting;       /* ?1004 */
  bool alternate_scroll;      /* ?1007 */
  bool bracketed_paste;       /* ?2004 */
  bool application_cursor;    /* DECCKM */
  bool application_keypad;    /* DECKPAM */
  bool synchronized_output;   /* ?2026 */
  bool reverse_video;         /* DECSCNM */
  uint8_t kitty_keyboard_flags;
  uint32_t history_size;
  uint32_t display_offset;
  uint32_t scrollback_limit;
  /* Changes whenever output or a viewport change may alter a read. */
  uint64_t generation;
  uint64_t palette_revision;
  /* Rows output has scrolled into history. History row i (0 the oldest) has
   * the stable number scrolled_into_history - history_size + i and live row
   * r has scrolled_into_history + r: anchor selections with it while output
   * scrolls and old history is evicted. Resize does not update it. */
  uint64_t scrolled_into_history;
  /* TERMY_ENGINE_SCROLLBACK_UNLIMITED in scrollback_limit means no row
   * limit; this is the byte limit. */
  uint64_t scrollback_bytes;
} TermyEngineState;

TermyEngineStatus termy_engine_get_state(TermyEngine *engine, TermyEngineState *out_state);
/* OSC 0/2 title; empty when none was set. Restored replicas get it from
 * the snapshot, without a title event. */
TermyEngineStatus termy_engine_title(TermyEngine *engine, TermyEngineBytes *out_title);
/* The last OSC 7 (or OSC 9;9) directory, percent-decoded. */
TermyEngineStatus termy_engine_working_directory(
    TermyEngine *engine,
    bool *out_found,
    TermyEngineBytes *out_path);

/* Scrollback limit: history keeps at most scrollback_rows rows and at most
 * scrollback_bytes of row storage, whichever binds first; with
 * TERMY_ENGINE_SCROLLBACK_UNLIMITED only the byte limit applies. Each row
 * is charged its slot, its cells and the grapheme, link and underline-color
 * metadata it references, measured by value. Cells are dense (16 bytes) or
 * packed; output scrolling into a history that would hold more dense rows
 * than the byte limit or 16 MiB packs each line with room for a full one
 * (about 5 bytes per column, 650 bytes per row at 120 columns). Without
 * termy_engine_compact_history() calls the charge follows from the bytes
 * fed alone, and snapshots restore it exactly, so a server and its
 * replicas on the same release evict the same rows. */

typedef struct {
  /* history_size + rows. */
  uint64_t total;
  /* Rows above the viewport's top: history_size - display_offset. */
  uint64_t offset;
  /* Visible rows. */
  uint64_t len;
} TermyEngineScrollbar;

TermyEngineStatus termy_engine_scrollbar(TermyEngine *engine, TermyEngineScrollbar *out_scrollbar);

#define TERMY_ENGINE_SCROLL_DELTA 0  /* value rows; positive scrolls back */
#define TERMY_ENGINE_SCROLL_TOP 1
#define TERMY_ENGINE_SCROLL_BOTTOM 2
#define TERMY_ENGINE_SCROLL_OFFSET 3 /* value = scrollbar offset to show */
TermyEngineStatus termy_engine_scroll_viewport(
    TermyEngine *engine,
    uint32_t kind,
    int64_t value,
    bool *out_changed);

/* Default colors as 0xRRGGBB, answered to OSC 4/10/11/12 queries and used
 * by termy_engine_effective_colors(). cursor and palette (256 entries) may
 * be NULL: no cursor color (queries report the foreground) and the xterm
 * palette. */
TermyEngineStatus termy_engine_set_default_colors(
    TermyEngine *engine,
    uint32_t foreground,
    uint32_t background,
    const uint32_t *cursor,
    const uint32_t *palette);

typedef struct {
  /* 0xRRGGBB after OSC 4/10/11/12 overrides. Under DECSCNM foreground and
   * background are already swapped. */
  uint32_t foreground;
  uint32_t background;
  uint32_t cursor;
  /* False when no cursor color is set and cursor repeats the foreground. */
  bool cursor_set;
  bool reverse_video;
  uint32_t palette[256];
} TermyEngineColors;

TermyEngineStatus termy_engine_effective_colors(TermyEngine *engine, TermyEngineColors *out_colors);

/* ------------------------------------------------------------------------
 * Events
 * ------------------------------------------------------------------------ */

#define TERMY_ENGINE_EVENT_BELL 1
#define TERMY_ENGINE_EVENT_TITLE 2              /* payload: title */
#define TERMY_ENGINE_EVENT_RESET_TITLE 3
#define TERMY_ENGINE_EVENT_WORKING_DIRECTORY 4  /* payload: path */
#define TERMY_ENGINE_EVENT_SHELL_INTEGRATION 5  /* payload: OSC 133 body */
#define TERMY_ENGINE_EVENT_PROGRESS 6           /* value: state << 8 | percent */
#define TERMY_ENGINE_EVENT_CLIPBOARD 7          /* payload: targets, data: base64 */
#define TERMY_ENGINE_EVENT_PROGRAM_STATUS 8     /* payload: JSON records */

typedef struct {
  uint32_t kind;
  int32_t value;
  TermyEngineBytes payload;
  TermyEngineBytes data;
} TermyEngineEvent;

/* Pop the next event. The queue is bounded (1024); replicas that don't
 * need events may simply never read them. Release with event_free. */
TermyEngineStatus termy_engine_next_event(
    TermyEngine *engine,
    bool *out_found,
    TermyEngineEvent *out_event);
void termy_engine_event_free(TermyEngineEvent *event);

/* ------------------------------------------------------------------------
 * Frames
 *
 * Colors in cells are raw: 0 the terminal default, 0x01000000 | n palette
 * entry n, 0x02RRGGBB a direct color. Resolve them with
 * termy_engine_effective_colors().
 * ------------------------------------------------------------------------ */

#define TERMY_ENGINE_COLOR_TAG(color) ((color) >> 24)
#define TERMY_ENGINE_COLOR_DEFAULT 0
#define TERMY_ENGINE_COLOR_INDEXED 1
#define TERMY_ENGINE_COLOR_RGB 2

#define TERMY_ENGINE_CELL_NARROW 0
#define TERMY_ENGINE_CELL_WIDE 1        /* first column of a wide glyph */
#define TERMY_ENGINE_CELL_SPACER_TAIL 2 /* second column of a wide glyph */
#define TERMY_ENGINE_CELL_SPACER_HEAD 3 /* last column, wide glyph wrapped */

#define TERMY_ENGINE_UNDERLINE_NONE 0
#define TERMY_ENGINE_UNDERLINE_SINGLE 1
#define TERMY_ENGINE_UNDERLINE_DOUBLE 2
#define TERMY_ENGINE_UNDERLINE_CURLY 3
#define TERMY_ENGINE_UNDERLINE_DOTTED 4
#define TERMY_ENGINE_UNDERLINE_DASHED 5

#define TERMY_ENGINE_CELL_BOLD (1u << 0)
#define TERMY_ENGINE_CELL_FAINT (1u << 1)
#define TERMY_ENGINE_CELL_ITALIC (1u << 2)
#define TERMY_ENGINE_CELL_INVERSE (1u << 3)
#define TERMY_ENGINE_CELL_INVISIBLE (1u << 4)
#define TERMY_ENGINE_CELL_STRIKETHROUGH (1u << 5)
#define TERMY_ENGINE_CELL_BLINK (1u << 6)
#define TERMY_ENGINE_CELL_PROTECTED (1u << 7)
/* Any non-default style, background, underline color or hyperlink. */
#define TERMY_ENGINE_CELL_STYLED (1u << 8)
/* A plain space with no styling or metadata: renderers may skip it. */
#define TERMY_ENGINE_CELL_BLANK (1u << 9)
/* Last cell of a row that soft-wraps into the next. */
#define TERMY_ENGINE_CELL_WRAPPED (1u << 10)

typedef struct {
  /* The first scalar of the grapheme cluster (' ' for blanks and
   * spacers). */
  uint32_t codepoint;
  /* Further scalars of the cluster (combining marks, ZWJ sequences,
   * variation selectors): frame.graphemes_ptr[grapheme_start ..
   * grapheme_start + grapheme_len]. */
  uint32_t grapheme_start;
  uint16_t grapheme_len;
  uint8_t width;
  uint8_t underline;
  uint32_t foreground;
  uint32_t background;
  /* 0 means the foreground. */
  uint32_t underline_color;
  uint16_t flags;
  uint16_t reserved;
  /* 0 none, else frame.hyperlinks_ptr[hyperlink - 1]. */
  uint32_t hyperlink;
} TermyEngineCell;

typedef struct {
  const uint8_t *id_ptr;
  size_t id_len;
  const uint8_t *uri_ptr;
  size_t uri_len;
} TermyEngineHyperlink;

/* Viewport row and half-open column range. */
typedef struct {
  uint16_t row;
  uint16_t start_col;
  uint16_t end_col;
} TermyEngineSpan;

typedef struct {
  /* Viewport position (the cursor moves down while scrolled back). */
  uint16_t row;
  uint16_t col;
  uint8_t shape;
  /* DECTCEM and inside the viewport. Hosts draw a hollow block when
   * unfocused; the engine has no hollow shape. */
  bool visible;
  bool blinking;
  bool in_viewport;
  /* The cursor sits on the right half of a wide glyph. */
  bool on_wide_tail;
} TermyEngineCursor;

#define TERMY_ENGINE_DAMAGE_PARTIAL 0
#define TERMY_ENGINE_DAMAGE_FULL 1

typedef struct {
  uint16_t cols;
  uint16_t rows;
  uint32_t damage;
  /* PARTIAL: changed spans in order; cells hold exactly their cells. */
  const TermyEngineSpan *spans_ptr;
  size_t spans_len;
  /* FULL: rows * cols cells, row-major. */
  const TermyEngineCell *cells_ptr;
  size_t cells_len;
  const uint32_t *graphemes_ptr;
  size_t graphemes_len;
  const TermyEngineHyperlink *hyperlinks_ptr;
  size_t hyperlinks_len;
  TermyEngineCursor cursor;
  uint64_t display_offset;
  uint64_t history_size;
  uint64_t generation;
  uint64_t palette_revision;
} TermyEngineFrame;

/* Changes since the last frame. A frame is FULL when force_full is set, on
 * the first frame, and whenever the size, the active screen, the viewport
 * scroll position, the palette or reverse video changed, or the engine
 * could not express the change as spans (screen-wide scrolls and clears).
 * Otherwise spans cover every changed cell, including rows that output
 * scrolled. Cursor fields are always current.
 *
 * Every pointer borrows memory owned by the handle and stays valid until
 * the next termy_engine_frame / termy_engine_read_lines call, or until the
 * handle is freed. Nothing needs to be released. */
TermyEngineStatus termy_engine_frame(
    TermyEngine *engine,
    bool force_full,
    TermyEngineFrame *out_frame);

/* count buffer lines from first_line (negative lines are scrollback, -1
 * the newest; 0 is the first live row), clamped to the buffer, as a FULL
 * frame of `rows` rows starting at max(first_line, -history_size). One call
 * reads at most 65535 rows; read the rest from first_line + rows. Reading
 * makes the next termy_engine_frame() FULL. */
TermyEngineStatus termy_engine_read_lines(
    TermyEngine *engine,
    int64_t first_line,
    uint32_t count,
    TermyEngineFrame *out_frame);

/* ------------------------------------------------------------------------
 * Kitty graphics. Placements carry pixel rectangles with cropping, c/r
 * fitting, letterboxing, offsets, scroll clipping and Unicode placeholder
 * tiles applied, so a host draws one textured rectangle per entry: the
 * image stretched over image_*, shown only inside clip_*.
 * ------------------------------------------------------------------------ */

#define TERMY_ENGINE_IMAGE_RGBA 0
#define TERMY_ENGINE_IMAGE_PNG 1

typedef struct {
  /* With image_generation, names the pixels: the same pair always has the
   * same pixels, so textures can be cached by it. */
  uint32_t image_id;
  /* Below 0 draws under text; below -1073741824 under cell backgrounds. */
  int32_t z_index;
  uint64_t image_generation;
  uint32_t image_width;
  uint32_t image_height;
  /* Pixels from the grid's top-left, in the cell size passed in. */
  float clip_x;
  float clip_y;
  float clip_width;
  float clip_height;
  float image_x;
  float image_y;
  float image_draw_width;
  float image_draw_height;
} TermyEngineImagePlacement;

/* Placements on the viewport for cells of cell_width x cell_height pixels,
 * lowest z first. The array borrows memory owned by the handle and stays
 * valid until the next termy_engine_image_placements call or until the
 * handle is freed. INVALID_ARGUMENT for a cell size that isn't positive. */
TermyEngineStatus termy_engine_image_placements(
    TermyEngine *engine,
    float cell_width,
    float cell_height,
    const TermyEngineImagePlacement **out_placements,
    size_t *out_len);

/* A copy of an image's pixels, for an image in the last placements call:
 * TERMY_ENGINE_IMAGE_RGBA rows of image_width * 4 bytes, or
 * TERMY_ENGINE_IMAGE_PNG. INVALID_ARGUMENT when the pair isn't there.
 * Release with termy_engine_bytes_free(). */
TermyEngineStatus termy_engine_image_data(
    TermyEngine *engine,
    uint32_t image_id,
    uint64_t image_generation,
    uint32_t *out_format,
    TermyEngineBytes *out_bytes);

/* ------------------------------------------------------------------------
 * Input encoders. They return the bytes to send to the program and never
 * write anywhere. An empty result means nothing should be sent.
 * ------------------------------------------------------------------------ */

#define TERMY_ENGINE_MOD_SHIFT (1u << 0)
#define TERMY_ENGINE_MOD_ALT (1u << 1)
#define TERMY_ENGINE_MOD_CTRL (1u << 2)
#define TERMY_ENGINE_MOD_SUPER (1u << 3)
/* Which Alt/Option key is held, when the host knows. */
#define TERMY_ENGINE_MOD_LEFT_ALT (1u << 8)
#define TERMY_ENGINE_MOD_RIGHT_ALT (1u << 9)

#define TERMY_ENGINE_OPTION_AS_ALT_NONE 0
#define TERMY_ENGINE_OPTION_AS_ALT_LEFT 1
#define TERMY_ENGINE_OPTION_AS_ALT_RIGHT 2
#define TERMY_ENGINE_OPTION_AS_ALT_BOTH 3

#define TERMY_ENGINE_KEY_PRESS 0
#define TERMY_ENGINE_KEY_REPEAT 1
#define TERMY_ENGINE_KEY_RELEASE 2

typedef struct {
  /* Termy key name, lowercase: a single character for text keys ("a",
   * "1", "/"), or enter, tab, escape, space, backspace, delete, insert,
   * home, end, pageup, pagedown, up, down, left, right, f1..f35,
   * printscreen, scrolllock, pause, menu, shift, control, alt, super,
   * capslock, numlock. May be empty when unshifted_codepoint is set. */
  const uint8_t *key_ptr;
  size_t key_len;
  /* UTF-8 text the key produced with the current layout, if any. */
  const uint8_t *text_ptr;
  size_t text_len;
  /* The key's character without modifiers (kitty base/alternate keys). */
  uint32_t unshifted_codepoint;
  uint32_t mods;
  /* Modifiers the layout used to produce text (e.g. Option for "å"). */
  uint32_t consumed_mods;
  uint8_t action;
  /* An IME composition is in progress: nothing is sent. */
  bool composing;
} TermyEngineKeyEvent;

/* Legacy, DECCKM/DECKPAM and every kitty keyboard flag, as the program
 * negotiated. An Alt reported with a side bit is Alt only when
 * option_as_alt includes that side; otherwise it composed the text.
 * Without side bits Alt is always Alt. */
TermyEngineStatus termy_engine_encode_key(
    TermyEngine *engine,
    const TermyEngineKeyEvent *event,
    uint32_t option_as_alt,
    TermyEngineBytes *out_bytes);

#define TERMY_ENGINE_MOUSE_PRESS 0
#define TERMY_ENGINE_MOUSE_RELEASE 1
#define TERMY_ENGINE_MOUSE_MOTION 2

#define TERMY_ENGINE_MOUSE_BUTTON_NONE 0
#define TERMY_ENGINE_MOUSE_BUTTON_LEFT 1
#define TERMY_ENGINE_MOUSE_BUTTON_MIDDLE 2
#define TERMY_ENGINE_MOUSE_BUTTON_RIGHT 3
#define TERMY_ENGINE_MOUSE_WHEEL_UP 4
#define TERMY_ENGINE_MOUSE_WHEEL_DOWN 5
#define TERMY_ENGINE_MOUSE_WHEEL_LEFT 6
#define TERMY_ENGINE_MOUSE_WHEEL_RIGHT 7
/* Buttons 8-11 (back, forward, ...). */
#define TERMY_ENGINE_MOUSE_BUTTON_8 8
#define TERMY_ENGINE_MOUSE_BUTTON_9 9
#define TERMY_ENGINE_MOUSE_BUTTON_10 10
#define TERMY_ENGINE_MOUSE_BUTTON_11 11

typedef struct {
  uint8_t action;
  /* For motion: the held button, or NONE. */
  uint8_t button;
  /* For motion: whether any button is held (?1002 reports only then). */
  bool any_button_pressed;
  /* SHIFT, ALT and CTRL are reported. */
  uint32_t mods;
  /* Viewport cell, zero-based. */
  uint32_t col;
  uint32_t row;
  /* Pointer position in pixels from the top-left of the cell grid, used
   * with ?1016. */
  uint32_t x_px;
  uint32_t y_px;
} TermyEngineMouseEvent;

/* Every tracking mode (?9, ?1000, ?1002, ?1003) and encoding (X10, ?1005,
 * ?1006, ?1016). Empty when the program did not ask for this event, or a
 * position cannot be encoded. Wheel events with no tracking and ?1007 are
 * the host's decision: send cursor keys from termy_engine_encode_key. */
TermyEngineStatus termy_engine_encode_mouse(
    TermyEngine *engine,
    const TermyEngineMouseEvent *event,
    TermyEngineBytes *out_bytes);

/* Line endings become CR; other C0 controls, DEL and C1 controls are
 * removed so pasted text cannot run commands or end the bracket early.
 * Pass bracketed = TermyEngineState.bracketed_paste. */
TermyEngineStatus termy_engine_encode_paste(
    const uint8_t *text_ptr,
    size_t text_len,
    bool bracketed,
    TermyEngineBytes *out_bytes);

/* CSI I or CSI O. Send it only when TermyEngineState.focus_reporting. */
TermyEngineStatus termy_engine_encode_focus(bool gained, TermyEngineBytes *out_bytes);

/* ------------------------------------------------------------------------
 * Text and selection
 * ------------------------------------------------------------------------ */

#define TERMY_ENGINE_FORMAT_PLAIN 0
#define TERMY_ENGINE_FORMAT_VT 1   /* SGR and OSC 8 preserved */
#define TERMY_ENGINE_FORMAT_HTML 2 /* <pre> with inline styles; OSC 8 links
                                   * only for http(s), mailto, file, ftp(s),
                                   * sftp and ssh URIs */

/* Join soft-wrapped rows without a line break. */
#define TERMY_ENGINE_TEXT_UNWRAP (1u << 0)
/* Drop trailing blanks at hard line ends. */
#define TERMY_ENGINE_TEXT_TRIM (1u << 1)

/* Buffer lines as in termy_engine_read_lines; columns inclusive. A
 * rectangular range takes the same columns from every line. */
typedef struct {
  int64_t start_line;
  uint32_t start_col;
  int64_t end_line;
  uint32_t end_col;
  bool rectangular;
} TermyEngineTextRange;

/* range NULL reads the whole buffer (scrollback and screen), e.g. for a
 * capture. Wide glyph spacers are skipped; grapheme clusters are kept. */
TermyEngineStatus termy_engine_read_text(
    TermyEngine *engine,
    const TermyEngineTextRange *range,
    uint32_t format,
    uint32_t options,
    TermyEngineBytes *out_text);

/* Double-click: the word (or run of one symbol) at a cell, across soft
 * wraps. Triple-click: the logical line, across soft wraps. Hosts keep the
 * selection state machine (timing, drag extension) and anchor ranges with
 * TermyEngineState.scrolled_into_history. */
TermyEngineStatus termy_engine_select_word(
    TermyEngine *engine,
    int64_t line,
    uint32_t col,
    TermyEngineTextRange *out_range);
TermyEngineStatus termy_engine_select_line(
    TermyEngine *engine,
    int64_t line,
    TermyEngineTextRange *out_range);

#ifdef __cplusplus
}
#endif

#endif
