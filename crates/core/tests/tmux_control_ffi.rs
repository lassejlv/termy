//! Exercises the tmux control-mode FFI surface against real tmux.
//! Ignored by default (needs tmux >= 3.3): run with `--ignored`.
#![cfg(unix)]

use std::process::Command;
use std::time::Duration;

use termy_core::ffi::{
    TermyFfiBytes, TermyFfiStatus, TermyFfiTmuxNotificationBatch, termy_buffer_free,
    termy_tmux_control_close, termy_tmux_control_notifications_free, termy_tmux_control_open,
    termy_tmux_control_poll, termy_tmux_control_send,
};
use termy_core::tmux_control_core::session::ControlSession;

#[test]
#[ignore = "requires tmux >= 3.3"]
fn ffi_control_open_poll_send_close() {
    let binary = b"tmux";
    let socket = format!("termy-ffi-tmux-{}", std::process::id());
    let session_name = b"termyffi";

    let mut handle: *mut ControlSession = std::ptr::null_mut();
    let status = unsafe {
        termy_tmux_control_open(
            binary.as_ptr(),
            binary.len(),
            socket.as_ptr(),
            socket.len(),
            session_name.as_ptr(),
            session_name.len(),
            &mut handle,
        )
    };
    assert_eq!(status, TermyFfiStatus::Ok);
    assert!(!handle.is_null());

    let mut received = false;
    for _ in 0..40 {
        let mut batch = TermyFfiTmuxNotificationBatch {
            notifications_ptr: std::ptr::null_mut(),
            notifications_len: 0,
            notifications_capacity: 0,
        };
        assert_eq!(
            unsafe { termy_tmux_control_poll(handle, &mut batch) },
            TermyFfiStatus::Ok
        );
        if batch.notifications_len > 0 {
            received = true;
        }
        unsafe { termy_tmux_control_notifications_free(&mut batch) };
        if received {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(received, "expected control notifications via FFI");

    let command = b"display-message -p termy-ffi-ok";
    let mut output = TermyFfiBytes {
        ptr: std::ptr::null_mut(),
        len: 0,
        capacity: 0,
    };
    assert_eq!(
        unsafe { termy_tmux_control_send(handle, command.as_ptr(), command.len(), &mut output) },
        TermyFfiStatus::Ok
    );
    unsafe { termy_buffer_free(output) };

    unsafe { termy_tmux_control_close(handle) };
    let _ = Command::new("tmux")
        .args(["-L", &socket, "kill-server"])
        .output();
}
