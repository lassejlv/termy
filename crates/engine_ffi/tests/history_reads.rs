//! Whole-buffer reads must not leave memory behind. The engine keeps a dense
//! copy of every history row read through `Engine::line` until the next
//! output, so a capture of an idle terminal used to hold all of its
//! scrollback expanded indefinitely; and a large `termy_engine_read_lines`
//! page used to stay allocated in the handle's frame buffers for good.
//!
//! A separate test binary, so the counting allocator sees only this file.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    ptr,
};

use termy_engine::{
    TermyEngine, TermyEngineBytes, TermyEngineFrame, TermyEngineStatus,
    frame::{termy_engine_frame, termy_engine_read_lines},
    termy_engine_bytes_free, termy_engine_compact_history, termy_engine_feed, termy_engine_free,
    termy_engine_new, termy_engine_options_default,
    text::{FORMAT_PLAIN, termy_engine_read_text},
};

struct Counting;

thread_local! {
    /// Bytes allocated and not yet freed by this thread.
    static LIVE: Cell<isize> = const { Cell::new(0) };
}

fn count(delta: isize) {
    let _ = LIVE.try_with(|live| live.set(live.get() + delta));
}

// SAFETY: forwards to the system allocator; only counts.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: same contract as the caller's.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            count(layout.size() as isize);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: same contract as the caller's.
        unsafe { System.dealloc(pointer, layout) };
        count(-(layout.size() as isize));
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn live() -> isize {
    LIVE.with(Cell::get)
}

/// 120 columns and 3977 rows of packed scrollback: about 11 MiB if expanded.
fn engine_with_packed_history() -> *mut TermyEngine {
    let mut options = termy_engine_options_default();
    options.cols = 120;
    options.rows = 24;
    let mut engine = ptr::null_mut();
    // SAFETY: valid options and out pointer.
    assert_eq!(
        unsafe { termy_engine_new(&options, &mut engine) },
        TermyEngineStatus::Ok
    );
    let mut output = Vec::new();
    for line in 0..4000 {
        output.extend_from_slice(format!("line {line:05} {}\r\n", "x".repeat(100)).as_bytes());
    }
    // SAFETY: live handle, valid input, replies left queued.
    unsafe {
        assert_eq!(
            termy_engine_feed(engine, output.as_ptr(), output.len(), ptr::null_mut()),
            TermyEngineStatus::Ok
        );
        assert_eq!(termy_engine_compact_history(engine), TermyEngineStatus::Ok);
    }
    engine
}

const RETAINED_LIMIT: isize = 2 * 1024 * 1024;

#[test]
fn capturing_the_buffer_does_not_keep_scrollback_expanded() {
    let engine = engine_with_packed_history();
    let before = live();
    let mut text = TermyEngineBytes {
        ptr: ptr::null_mut(),
        len: 0,
        capacity: 0,
    };
    // SAFETY: live handle; a null range reads the whole buffer.
    let status = unsafe { termy_engine_read_text(engine, ptr::null(), FORMAT_PLAIN, 0, &mut text) };
    assert_eq!(status, TermyEngineStatus::Ok);
    // SAFETY: the buffer is valid for its length until freed.
    let captured = unsafe { std::slice::from_raw_parts(text.ptr, text.len) };
    assert!(captured.starts_with(b"line 00000 xxx"));
    assert!(captured.len() > 4000 * 100);
    // SAFETY: a buffer from the library, freed once.
    unsafe { termy_engine_bytes_free(&mut text) };
    let retained = live() - before;
    assert!(
        retained < RETAINED_LIMIT,
        "a capture left {retained} bytes allocated"
    );
    // SAFETY: live handle, freed once.
    unsafe { termy_engine_free(engine) };
}

#[test]
fn paging_through_scrollback_does_not_keep_it_expanded() {
    let engine = engine_with_packed_history();
    let before = live();
    // 3977 history rows (4000 lines, 23 still on screen).
    let mut first = -3900_i64;
    while first < 0 {
        let mut frame = TermyEngineFrame::default();
        // SAFETY: live handle and out struct.
        let status = unsafe { termy_engine_read_lines(engine, first, 100, &mut frame) };
        assert_eq!(status, TermyEngineStatus::Ok);
        assert_eq!(frame.rows, 100);
        first += 100;
    }
    // The handle keeps the last page's cells (100 rows); nothing more.
    let retained = live() - before;
    assert!(
        retained < RETAINED_LIMIT,
        "paging left {retained} bytes allocated"
    );
    // SAFETY: live handle, freed once.
    unsafe { termy_engine_free(engine) };
}

#[test]
fn a_large_page_is_released_by_the_next_frame() {
    let engine = engine_with_packed_history();
    let before = live();
    let mut frame = TermyEngineFrame::default();
    // SAFETY: live handle and out struct.
    let status = unsafe { termy_engine_read_lines(engine, -3977, u32::MAX, &mut frame) };
    assert_eq!(status, TermyEngineStatus::Ok);
    assert_eq!(frame.rows, 3977 + 24);
    // 4001 rows of 120 cells, about 15 MiB, borrowed until the next call.
    assert!(live() - before > 4 * RETAINED_LIMIT);
    // SAFETY: live handle and out struct.
    let status = unsafe { termy_engine_frame(engine, false, &mut frame) };
    assert_eq!(status, TermyEngineStatus::Ok);
    let retained = live() - before;
    assert!(
        retained < RETAINED_LIMIT,
        "a frame after a large page kept {retained} bytes allocated"
    );
    // SAFETY: live handle, freed once.
    unsafe { termy_engine_free(engine) };
}
