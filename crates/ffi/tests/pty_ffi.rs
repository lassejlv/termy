#![cfg(unix)]
#![allow(
    clippy::borrow_as_ptr,
    clippy::wildcard_imports,
    reason = "this test intentionally exercises the C pointer API from Rust"
)]

use std::{
    ffi::c_void,
    ptr, slice,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

use tmon_ffi::*;

#[derive(Default)]
struct CallbackState {
    wake_count: AtomicU64,
    exited: AtomicBool,
}

unsafe extern "C" fn on_event(user_data: *mut c_void, event: *const TmonPtyEvent) {
    // SAFETY: The test keeps `CallbackState` alive until `tmon_pty_free` returns, and the ABI
    // guarantees that the event record lives for this callback.
    let state = unsafe { &*(user_data.cast::<CallbackState>()) };
    // SAFETY: See above.
    let event = unsafe { &*event };
    match event.kind {
        TMON_PTY_WAKE => {
            state.wake_count.fetch_add(1, Ordering::Relaxed);
        }
        TMON_PTY_EXIT => state.exited.store(true, Ordering::Release),
        TMON_PTY_READ_ERROR => panic!("PTY reader reported an error"),
        _ => panic!("unknown PTY callback event"),
    }
}

fn view(bytes: &[u8]) -> TmonByteSlice {
    TmonByteSlice {
        data: bytes.as_ptr(),
        length: bytes.len(),
    }
}

#[test]
fn native_pty_can_be_spawned_drained_measured_and_destroyed() {
    let program = b"/bin/sh";
    let arguments = [view(b"-c"), view(b"printf ffi-pty")];
    let config = TmonPtyConfig {
        program: view(program),
        arguments: arguments.as_ptr(),
        argument_count: arguments.len(),
        ..tmon_pty_config_default()
    };
    let state = CallbackState::default();
    let mut pty = ptr::null_mut();

    // SAFETY: Configuration views and callback state remain valid for the required lifetimes.
    unsafe {
        assert_eq!(
            tmon_pty_spawn(
                &config,
                Some(on_event),
                (&raw const state).cast_mut().cast::<c_void>(),
                &mut pty,
            ),
            TMON_OK
        );
        assert!(!pty.is_null());

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut collected = Vec::new();
        while Instant::now() < deadline && collected != b"ffi-pty" {
            let mut output = TmonByteSlice::empty();
            assert_eq!(tmon_pty_drain_output(pty, &mut output), TMON_OK);
            if output.length > 0 {
                collected.extend_from_slice(slice::from_raw_parts(output.data, output.length));
            }
            if collected == b"ffi-pty" {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(collected, b"ffi-pty");

        let mut pid = TmonOptionalU32::default();
        assert_eq!(tmon_pty_child_pid(pty, &mut pid), TMON_OK);
        assert_eq!(pid.has_value, 1);
        assert!(pid.value > 0);

        let mut metrics = TmonPtyBufferMetrics::default();
        assert_eq!(tmon_pty_buffer_metrics(pty, &mut metrics), TMON_OK);
        assert!(metrics.bytes_buffered >= 7);
        assert!(metrics.bytes_drained >= 7);
        assert!(state.wake_count.load(Ordering::Relaxed) >= 1);

        assert_eq!(tmon_pty_free(pty), TMON_OK);
    }
}
