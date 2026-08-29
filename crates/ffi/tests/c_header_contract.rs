#![cfg(unix)]

use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};

const C_CONTRACT_SOURCE: &str = r#"
#include <stddef.h>
#include <stdint.h>

#include "termy.h"

_Static_assert(TERMY_FFI_OK == 0, "status enum starts at OK");
_Static_assert(TERMY_FFI_PANICKED == 8, "panic status is stable");
_Static_assert(TERMY_FFI_INVALID_ARGUMENT == 9, "invalid argument status is stable");
_Static_assert(TERMY_FFI_CLIPBOARD_LOCATION_CLIPBOARD == 1, "clipboard location is stable");
_Static_assert(TERMY_FFI_CLIPBOARD_LOCATION_PRIMARY == 2, "primary location is stable");
_Static_assert(TERMY_FFI_CLIPBOARD_RESULT_SUCCESS == 0, "clipboard success is stable");
_Static_assert(TERMY_FFI_CLIPBOARD_RESULT_IO_ERROR == 5, "clipboard I/O error is stable");
_Static_assert(sizeof(TermyFfiCell) == 20, "cell ABI size is stable");
_Static_assert(sizeof(TermyFfiGlyphPoint) == 8, "glyph point ABI size is stable");
_Static_assert(sizeof(TermyFfiGlyphRect) == 24, "glyph rect ABI size is stable");
_Static_assert(sizeof(TermyFfiGlyphStroke) == 60, "glyph stroke ABI size is stable");
_Static_assert(offsetof(TermyFfiCell, italic) > offsetof(TermyFfiCell, line_wrapped), "text attributes use trailing cell padding");
_Static_assert(offsetof(TermyFfiFrame, cells_ptr) < offsetof(TermyFfiFrame, cursor), "frame cell storage precedes cursor");
_Static_assert(offsetof(TermyFfiFrameUpdate, damage_kind) < offsetof(TermyFfiFrameUpdate, spans_ptr), "frame update damage metadata precedes spans");
_Static_assert(offsetof(TermyFfiEventBatch, has_more) > offsetof(TermyFfiEventBatch, events_capacity), "event batch has_more follows vector storage");
_Static_assert(offsetof(TermyFfiClipboardReadRequest, name) > offsetof(TermyFfiClipboardReadRequest, mime_types_len), "clipboard read name follows MIME types");
_Static_assert(offsetof(TermyFfiClipboardWriteRequest, name) > offsetof(TermyFfiClipboardWriteRequest, contents_len), "clipboard write name follows contents");

static void read_clipboard(
    TermyFfiUserData user_data,
    const TermyFfiClipboardReadRequest *request,
    TermyFfiUserData reply_user_data,
    TermyFfiClipboardReadReplyCallback reply_callback) {
  (void)user_data;
  (void)request;
  TermyFfiClipboardReadResponse response = {0};
  response.status = TERMY_FFI_CLIPBOARD_RESULT_UNSUPPORTED;
  reply_callback(reply_user_data, &response);
}

static TermyFfiClipboardWriteResponse write_clipboard(
    TermyFfiUserData user_data,
    const TermyFfiClipboardWriteRequest *request) {
  (void)user_data;
  (void)request;
  TermyFfiClipboardWriteResponse response = {0};
  response.status = TERMY_FFI_CLIPBOARD_RESULT_UNSUPPORTED;
  return response;
}

static void protocol_reply(
    TermyFfiUserData user_data,
    TermyFfiByteSlice reply) {
  (void)user_data;
  (void)reply;
}

void termy_header_contract(void) {
  TermyFfiSize size = termy_size_default();
  TermyFfiTerminal *terminal = 0;
  TermyFfiFrame frame = {0};
  TermyFfiGlyphMetrics glyph_metrics = {size.cell_width, size.cell_height, 14.0f};
  TermyFfiGlyphRenderPlan glyph_plan = {0};
  TermyFfiKittyGraphicsBatch graphics = {0};
  uint64_t graphics_revision = 0;
  bool clipboard_enabled = false;
  bool paste_event_sent = false;
  TermyFfiEventBatch events = {0};
  const uint8_t bytes[] = {'o', 'k'};
  TermyFfiByteSlice formats[] = {{bytes, sizeof(bytes)}};

  TermyFfiStatus status = termy_display_terminal_new(size, &terminal);
  (void)status;
  (void)termy_terminal_feed_output(terminal, bytes, sizeof(bytes));
  (void)termy_terminal_snapshot(terminal, &frame);
  (void)termy_cells_build_glyph_render_plan(
      frame.cells_ptr,
      frame.cells_len,
      frame.cols,
      frame.rows,
      0,
      0,
      glyph_metrics,
      &glyph_plan);
  (void)termy_glyph_render_plan_free(&glyph_plan);
  (void)termy_terminal_kitty_graphics_revision(terminal, &graphics_revision);
  (void)termy_terminal_kitty_graphics_placements(terminal, &graphics);
  (void)termy_kitty_graphics_batch_free(&graphics);
  (void)termy_terminal_drain_events_with_clipboard(
      terminal, 0, read_clipboard, write_clipboard, protocol_reply, &events);
  (void)termy_event_batch_free(&events);
  (void)termy_terminal_kitty_clipboard_paste_events_enabled(
      terminal, &clipboard_enabled);
  (void)termy_terminal_send_kitty_clipboard_paste_event(
      terminal,
      TERMY_FFI_CLIPBOARD_LOCATION_CLIPBOARD,
      formats,
      1,
      &paste_event_sent);
  (void)termy_frame_free(&frame);
  (void)termy_terminal_free(terminal);
}
"#;

fn compiler_exists(compiler: &str) -> bool {
    Command::new(compiler)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn c_compiler() -> Option<String> {
    std::env::var("CC")
        .ok()
        .filter(|compiler| compiler_exists(compiler))
        .or_else(|| {
            ["cc", "clang", "gcc"]
                .into_iter()
                .find(|compiler| compiler_exists(compiler))
                .map(str::to_string)
        })
}

#[test]
fn c_header_compiles_minimal_display_terminal_contract() {
    let compiler = c_compiler().expect("expected CC, cc, clang, or gcc to compile termy.h");
    let temp = tempfile::tempdir().expect("tempdir");
    let source_path = temp.path().join("termy_header_contract.c");
    let object_path = temp.path().join("termy_header_contract.o");
    fs::write(&source_path, C_CONTRACT_SOURCE).expect("write C contract source");

    let header_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("include");
    let output = Command::new(&compiler)
        .arg("-std=c11")
        .arg("-Wall")
        .arg("-Wextra")
        .arg("-Werror")
        .arg("-I")
        .arg(&header_dir)
        .arg("-c")
        .arg(&source_path)
        .arg("-o")
        .arg(&object_path)
        .output()
        .unwrap_or_else(|error| panic!("failed to run C compiler {compiler}: {error}"));

    assert!(
        output.status.success(),
        "failed to compile C contract against termy.h with {compiler}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
