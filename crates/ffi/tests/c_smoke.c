#include "tmon.h"

#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static TmonByteSlice bytes(const char *text) {
  TmonByteSlice view = {(const uint8_t *)text, strlen(text)};
  return view;
}

static void require_ok(TmonStatus status) {
  if (status != TMON_OK) {
    fprintf(stderr, "Tmon FFI error %u: %s\n", status,
            tmon_last_error_message());
    exit(1);
  }
}

static const uint8_t *range_start(TmonByteSlice storage,
                                  TmonRange range) {
  assert(range.offset <= storage.length);
  assert(range.length <= storage.length - range.offset);
  return storage.data + range.offset;
}

int main(void) {
  _Static_assert(TMON_ABI_VERSION == 0x00020000u,
                 "unexpected Tmon ABI version");
  assert(tmon_abi_version() == TMON_ABI_VERSION);
  TmonByteSlice version = tmon_library_version();
  assert(version.data != NULL && version.length > 0);

  TmonTerminalConfig config = tmon_terminal_config_default();
  config.columns = 8;
  config.rows = 2;
  config.scrollback_limit = 100;

  TmonTerminal *terminal = NULL;
  require_ok(tmon_terminal_new(&config, &terminal));
  assert(terminal != NULL);

  TmonFrameView initial = {0};
  require_ok(tmon_terminal_frame_update(terminal, 1, &initial));
  assert(initial.full == 1);
  assert(initial.columns == 8 && initial.rows == 2);
  assert(initial.row_update_count == 2 && initial.cell_count == 16);

  const char output[] = "\033]2;C host\007\033[38;2;1;2;3mA";
  require_ok(tmon_terminal_feed(terminal, (const uint8_t *)output,
                                     sizeof(output) - 1));

  TmonFrameView frame = {0};
  require_ok(tmon_terminal_frame_update(terminal, 0, &frame));
  assert(frame.full == 0 && frame.row_update_count == 1);
  assert(frame.cell_count == 1);
  const TmonCell *cell = &frame.cells[0];
  assert(cell->text.length == 1);
  assert(*range_start(frame.text, cell->text) == 'A');
  assert(cell->foreground.kind == TMON_COLOR_RGB);
  assert(cell->foreground.red == 1 && cell->foreground.green == 2 &&
         cell->foreground.blue == 3);

  TmonEventBatchView events = {0};
  require_ok(tmon_terminal_drain_events(terminal, &events));
  assert(events.event_count == 1);
  assert(events.events[0].kind == TMON_EVENT_TITLE);
  assert(events.events[0].primary.length == strlen("C host"));
  assert(memcmp(range_start(events.data, events.events[0].primary), "C host",
                strlen("C host")) == 0);

  TmonKeyEvent at = {0};
  at.key_kind = TMON_KEY_CHARACTER;
  at.key_value = '2';
  at.modifiers = TMON_MOD_SHIFT;
  at.event_kind = TMON_KEY_PRESS;
  at.text = bytes("@");
  at.has_text = 1;
  TmonByteSlice encoded = {0};
  require_ok(tmon_terminal_encode_key(terminal, &at, &encoded));
  assert(encoded.length == 1 && encoded.data[0] == '@');

  TmonKeyEvent backtab = {0};
  backtab.key_kind = TMON_KEY_TAB;
  backtab.modifiers = TMON_MOD_SHIFT;
  backtab.event_kind = TMON_KEY_PRESS;
  require_ok(tmon_terminal_encode_key(terminal, &backtab, &encoded));
  assert(encoded.length == 3 && memcmp(encoded.data, "\033[Z", 3) == 0);

  TmonTerminalMetrics metrics = {0};
  require_ok(tmon_terminal_metrics(terminal, &metrics));
  assert(metrics.feed_calls == 1 && metrics.frame_requests == 2);

  require_ok(tmon_terminal_free(terminal));
  puts("tmon C ABI smoke test passed");
  return 0;
}
