#![allow(
    clippy::borrow_as_ptr,
    clippy::wildcard_imports,
    reason = "this test intentionally exercises the C pointer API from Rust"
)]

use std::{ffi::CStr, ptr, slice};

use tmon_ffi::*;

fn byte_view(bytes: &[u8]) -> TmonByteSlice {
    TmonByteSlice {
        data: bytes.as_ptr(),
        length: bytes.len(),
    }
}

unsafe fn borrowed_bytes(view: TmonByteSlice) -> &'static [u8] {
    if view.length == 0 {
        &[]
    } else {
        // SAFETY: Test callers consume ABI views inside their documented validity window.
        unsafe { slice::from_raw_parts(view.data, view.length) }
    }
}

unsafe fn terminal(columns: usize, rows: usize) -> *mut TmonTerminal {
    let config = TmonTerminalConfig {
        columns,
        rows,
        scrollback_limit: 100,
    };
    let mut terminal = ptr::null_mut();
    // SAFETY: The config and out pointer are valid for this call.
    assert_eq!(
        unsafe { tmon_terminal_new(&config, &mut terminal) },
        TMON_OK
    );
    assert!(!terminal.is_null());
    terminal
}

#[test]
fn frame_updates_are_packed_and_events_share_one_backing_buffer() {
    // SAFETY: This test owns and serializes the handle until the matching free.
    unsafe {
        let terminal = terminal(8, 2);
        let mut initial = TmonFrameView::default();
        assert_eq!(
            tmon_terminal_frame_update(terminal, 1, &mut initial),
            TMON_OK
        );
        assert_eq!((initial.columns, initial.rows), (8, 2));
        assert_eq!(initial.full, 1);
        assert_eq!(initial.row_update_count, 2);
        assert_eq!(initial.cell_count, 16);

        let bytes = b"\x1b]2;embedded\x07\x1b[38;2;12;34;56mA";
        assert_eq!(
            tmon_terminal_feed(terminal, bytes.as_ptr(), bytes.len()),
            TMON_OK
        );

        let mut frame = TmonFrameView::default();
        assert_eq!(tmon_terminal_frame_update(terminal, 0, &mut frame), TMON_OK);
        assert_eq!(frame.full, 0);
        assert_eq!(frame.row_update_count, 1);
        assert_eq!(frame.cell_count, 1);
        let cells = slice::from_raw_parts(frame.cells, frame.cell_count);
        let text = borrowed_bytes(frame.text);
        let cell = cells[0];
        assert_eq!(
            &text[cell.text.offset..cell.text.offset + cell.text.length],
            b"A"
        );
        assert_eq!(cell.foreground.kind, TMON_COLOR_RGB);
        assert_eq!(
            (
                cell.foreground.red,
                cell.foreground.green,
                cell.foreground.blue
            ),
            (12, 34, 56)
        );

        let mut events = TmonEventBatchView::default();
        assert_eq!(tmon_terminal_drain_events(terminal, &mut events), TMON_OK);
        assert_eq!(events.event_count, 1);
        let event = *events.events;
        assert_eq!(event.kind, TMON_EVENT_TITLE);
        let data = borrowed_bytes(events.data);
        assert_eq!(
            &data[event.primary.offset..event.primary.offset + event.primary.length],
            b"embedded"
        );

        assert_eq!(tmon_terminal_free(terminal), TMON_OK);
    }
}

#[test]
fn abi_v2_packs_scroll_operations_before_exposed_row_updates() {
    // SAFETY: This test owns and serializes the handle until the matching free.
    unsafe {
        assert_eq!(tmon_abi_version(), 0x0002_0000);
        let terminal = terminal(8, 3);
        let mut initial = TmonFrameView::default();
        assert_eq!(
            tmon_terminal_frame_update(terminal, 1, &mut initial),
            TMON_OK
        );

        let bytes = b"\x1b[3;1H\r\n";
        assert_eq!(
            tmon_terminal_feed(terminal, bytes.as_ptr(), bytes.len()),
            TMON_OK
        );
        let mut frame = TmonFrameView::default();
        assert_eq!(tmon_terminal_frame_update(terminal, 0, &mut frame), TMON_OK);

        assert_eq!(frame.full, 0);
        assert_eq!(frame.row_move_count, 1);
        let movement = *frame.row_moves;
        assert_eq!(movement.start_row, 0);
        assert_eq!(movement.end_row, 3);
        assert_eq!(movement.direction, TMON_ROW_MOVE_UP);
        assert_eq!(movement.count, 1);
        assert_eq!(frame.row_update_count, 1);
        assert_eq!((*frame.row_updates).row, 2);

        assert_eq!(tmon_terminal_free(terminal), TMON_OK);
    }
}

#[test]
fn layout_text_backtab_mouse_and_selection_use_engine_protocols() {
    // SAFETY: This test owns and serializes the handle until the matching free.
    unsafe {
        let terminal = terminal(12, 2);

        let at = b"@";
        let event = TmonKeyEvent {
            key_kind: TMON_KEY_CHARACTER,
            key_value: u32::from('2'),
            modifiers: TMON_MOD_SHIFT,
            event_kind: TMON_KEY_PRESS,
            text: byte_view(at),
            has_text: 1,
            ..TmonKeyEvent::default()
        };
        let mut encoded = TmonByteSlice::empty();
        assert_eq!(
            tmon_terminal_encode_key(terminal, &event, &mut encoded),
            TMON_OK
        );
        assert_eq!(borrowed_bytes(encoded), b"@");

        let backtab = TmonKeyEvent {
            key_kind: TMON_KEY_TAB,
            modifiers: TMON_MOD_SHIFT,
            event_kind: TMON_KEY_PRESS,
            ..TmonKeyEvent::default()
        };
        assert_eq!(
            tmon_terminal_encode_key(terminal, &backtab, &mut encoded),
            TMON_OK
        );
        assert_eq!(borrowed_bytes(encoded), b"\x1b[Z");

        let mouse_mode = b"\x1b[?1000h\x1b[?1006h";
        assert_eq!(
            tmon_terminal_feed(terminal, mouse_mode.as_ptr(), mouse_mode.len()),
            TMON_OK
        );
        let mouse = TmonMouseEvent {
            button: TMON_MOUSE_BUTTON_LEFT,
            kind: TMON_MOUSE_PRESS,
            column: 0,
            row: 0,
            pixel_x: 0,
            pixel_y: 0,
            modifiers: 0,
        };
        let mut has_mouse = 0;
        assert_eq!(
            tmon_terminal_encode_mouse(terminal, &mouse, &mut encoded, &mut has_mouse,),
            TMON_OK
        );
        assert_eq!(has_mouse, 1);
        assert_eq!(borrowed_bytes(encoded), b"\x1b[<0;1;1M");

        let text = b"copy me";
        assert_eq!(
            tmon_terminal_feed(terminal, text.as_ptr(), text.len()),
            TMON_OK
        );
        let mut changed = 0;
        assert_eq!(
            tmon_terminal_begin_selection(
                terminal,
                TmonSelectionPoint { column: 0, row: 0 },
                &mut changed,
            ),
            TMON_OK
        );
        assert_eq!(
            tmon_terminal_update_selection(
                terminal,
                TmonSelectionPoint { column: 6, row: 0 },
                &mut changed,
            ),
            TMON_OK
        );
        let mut selected = TmonByteSlice::empty();
        let mut has_selection = 0;
        assert_eq!(
            tmon_terminal_selected_text(terminal, &mut selected, &mut has_selection,),
            TMON_OK
        );
        assert_eq!(has_selection, 1);
        assert_eq!(borrowed_bytes(selected), b"copy me");

        assert_eq!(tmon_terminal_free(terminal), TMON_OK);
    }
}

#[test]
fn invalid_inputs_return_status_and_thread_local_diagnostic() {
    // SAFETY: All pointers passed are either deliberately null or valid for the call.
    unsafe {
        let config = tmon_terminal_config_default();
        assert_eq!(
            tmon_terminal_new(&config, ptr::null_mut()),
            TMON_NULL_POINTER
        );
        let message = CStr::from_ptr(tmon_last_error_message())
            .to_str()
            .expect("diagnostic is UTF-8");
        assert!(message.contains("out_terminal"));

        let terminal = terminal(8, 2);
        let invalid = [0xff];
        let mut encoded = TmonByteSlice::empty();
        assert_eq!(
            tmon_terminal_encode_text(terminal, byte_view(&invalid), &mut encoded,),
            TMON_INVALID_UTF8
        );
        assert_eq!(tmon_terminal_free(terminal), TMON_OK);
        assert_eq!(tmon_terminal_free(ptr::null_mut()), TMON_OK);
    }
}

#[test]
fn selection_modes_are_available_through_the_c_abi() {
    // SAFETY: The test owns the handle and all pointers remain valid for every call.
    unsafe {
        let terminal = terminal(24, 1);
        let text = b"alpha /tmp/foo.rs tail";
        assert_eq!(
            tmon_terminal_feed(terminal, text.as_ptr(), text.len()),
            TMON_OK
        );

        let mut changed = 0;
        assert_eq!(
            tmon_terminal_begin_selection_with_mode(
                terminal,
                TmonSelectionPoint { column: 9, row: 0 },
                TMON_SELECTION_WORD,
                &mut changed,
            ),
            TMON_OK
        );
        assert_eq!(changed, 1);
        let mut selected = TmonByteSlice::empty();
        let mut has_selection = 0;
        assert_eq!(
            tmon_terminal_selected_text(terminal, &mut selected, &mut has_selection),
            TMON_OK
        );
        assert_eq!(borrowed_bytes(selected), b"/tmp/foo.rs");

        assert_eq!(
            tmon_terminal_begin_selection_with_mode(
                terminal,
                TmonSelectionPoint { column: 0, row: 0 },
                u32::MAX,
                &mut changed,
            ),
            TMON_INVALID_ARGUMENT
        );
        assert_eq!(tmon_terminal_free(terminal), TMON_OK);
    }
}

#[test]
fn full_buffer_search_is_available_through_the_c_abi() {
    // SAFETY: The test owns the handle and all pointers remain valid for every call.
    unsafe {
        let terminal = terminal(12, 2);
        let text = b"first\r\nFind me\r\nlast";
        assert_eq!(
            tmon_terminal_feed(terminal, text.as_ptr(), text.len()),
            TMON_OK
        );
        let options = tmon_search_options_default();
        assert_eq!(options.direction, TMON_SEARCH_BACKWARD);
        assert_eq!(options.wrap, 1);

        let mut found = TmonSearchMatch::default();
        let mut has_value = 0;
        assert_eq!(
            tmon_terminal_search(
                terminal,
                byte_view(b"find"),
                options,
                &mut found,
                &mut has_value,
            ),
            TMON_OK
        );
        assert_eq!(has_value, 1);
        assert_eq!(found.selection.start.column, 0);
        assert_eq!(found.selection.end.column, 3);

        let mut changed = 0;
        assert_eq!(tmon_terminal_reset_search(terminal, &mut changed), TMON_OK);
        assert_eq!(changed, 1);
        assert_eq!(
            tmon_terminal_search(
                terminal,
                byte_view(b"find"),
                TmonSearchOptions {
                    direction: u32::MAX,
                    ..options
                },
                &mut found,
                &mut has_value,
            ),
            TMON_INVALID_ARGUMENT
        );
        assert_eq!(tmon_terminal_free(terminal), TMON_OK);
    }
}

#[test]
fn metrics_and_memory_stats_are_available_without_snapshots() {
    // SAFETY: This test owns and serializes the handle until the matching free.
    unsafe {
        let terminal = terminal(8, 2);
        let bytes = b"metrics";
        assert_eq!(
            tmon_terminal_feed(terminal, bytes.as_ptr(), bytes.len()),
            TMON_OK
        );
        let mut frame = TmonFrameView::default();
        assert_eq!(tmon_terminal_frame_update(terminal, 0, &mut frame), TMON_OK);
        let mut metrics = TmonTerminalMetrics::default();
        assert_eq!(tmon_terminal_metrics(terminal, &mut metrics), TMON_OK);
        assert_eq!(metrics.feed_calls, 1);
        assert_eq!(metrics.bytes_fed, bytes.len() as u64);
        assert_eq!(metrics.frame_requests, 1);

        let mut memory = TmonMemoryStats::default();
        assert_eq!(tmon_terminal_memory_stats(terminal, &mut memory), TMON_OK);
        assert!(memory.total_cell_capacity >= 16);
        assert!(memory.cell_capacity_bytes > 0);
        assert_eq!(tmon_terminal_free(terminal), TMON_OK);
    }
}
