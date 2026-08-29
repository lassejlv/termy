use std::{ffi::c_void, slice, str};

use termy_ffi::{
    TermyFfiByteSlice, TermyFfiClipboardContent, TermyFfiClipboardReadReplyCallback,
    TermyFfiClipboardReadRequest, TermyFfiClipboardReadResponse, TermyFfiClipboardWriteRequest,
    TermyFfiClipboardWriteResponse, TermyFfiEventBatch, TermyFfiStatus, TermyFfiTerminal,
    termy_display_terminal_new, termy_event_batch_free, termy_size_default,
    termy_terminal_drain_events_with_clipboard, termy_terminal_feed_output, termy_terminal_free,
    termy_terminal_kitty_clipboard_paste_events_enabled,
    termy_terminal_send_kitty_clipboard_paste_event,
};

const CLIPBOARD_LOCATION: u32 = 1;
const RESULT_SUCCESS: u32 = 0;
const RESULT_IO_ERROR: u32 = 5;

#[derive(Default)]
struct ClipboardCallbackState {
    callbacks_valid: bool,
    read_location: u32,
    read_mime_types: Vec<String>,
    write_location: u32,
    write_contents: Vec<(String, Vec<u8>)>,
    protocol_replies: Vec<u8>,
}

fn borrowed(bytes: &[u8]) -> TermyFfiByteSlice {
    TermyFfiByteSlice {
        ptr: bytes.as_ptr(),
        len: bytes.len(),
    }
}

unsafe fn read_bytes(bytes: TermyFfiByteSlice) -> Option<Vec<u8>> {
    if bytes.len == 0 {
        return Some(Vec::new());
    }
    if bytes.ptr.is_null() {
        return None;
    }
    Some(unsafe { slice::from_raw_parts(bytes.ptr, bytes.len) }.to_vec())
}

unsafe fn read_string(bytes: TermyFfiByteSlice) -> Option<String> {
    let bytes = unsafe { read_bytes(bytes) }?;
    str::from_utf8(&bytes).ok().map(ToOwned::to_owned)
}

unsafe extern "C" fn read_clipboard(
    user_data: *mut c_void,
    request: *const TermyFfiClipboardReadRequest,
    reply_user_data: *mut c_void,
    reply_callback: TermyFfiClipboardReadReplyCallback,
) {
    if user_data.is_null() || request.is_null() {
        let response = TermyFfiClipboardReadResponse {
            status: RESULT_IO_ERROR,
            ..TermyFfiClipboardReadResponse::default()
        };
        unsafe { reply_callback(reply_user_data, &response) };
        return;
    }
    let state = unsafe { &mut *user_data.cast::<ClipboardCallbackState>() };
    let request = unsafe { &*request };
    if request.mime_types_len > 0 && request.mime_types_ptr.is_null() {
        let response = TermyFfiClipboardReadResponse {
            status: RESULT_IO_ERROR,
            ..TermyFfiClipboardReadResponse::default()
        };
        unsafe { reply_callback(reply_user_data, &response) };
        return;
    }
    let mime_types = if request.mime_types_len == 0 {
        &[][..]
    } else {
        unsafe { slice::from_raw_parts(request.mime_types_ptr, request.mime_types_len) }
    };
    let Some(mime_types) = mime_types
        .iter()
        .map(|mime_type| unsafe { read_string(*mime_type) })
        .collect::<Option<Vec<_>>>()
    else {
        let response = TermyFfiClipboardReadResponse {
            status: RESULT_IO_ERROR,
            ..TermyFfiClipboardReadResponse::default()
        };
        unsafe { reply_callback(reply_user_data, &response) };
        return;
    };

    state.callbacks_valid = !request.list_available
        && !request.has_name
        && !request.permission_granted
        && !request.can_remember_permission;
    state.read_location = request.location;
    state.read_mime_types = mime_types;
    let available_formats = [borrowed(b"text/plain")];
    let read_contents = [TermyFfiClipboardContent {
        mime_type: borrowed(b"text/plain"),
        data: borrowed(b"from-host"),
    }];
    let response = TermyFfiClipboardReadResponse {
        status: RESULT_SUCCESS,
        available_formats_ptr: available_formats.as_ptr(),
        available_formats_len: available_formats.len(),
        contents_ptr: read_contents.as_ptr(),
        contents_len: read_contents.len(),
        remember_permission: false,
    };
    unsafe { reply_callback(reply_user_data, &response) };
}

unsafe extern "C" fn read_clipboard_with_unrequested_content(
    _user_data: *mut c_void,
    _request: *const TermyFfiClipboardReadRequest,
    reply_user_data: *mut c_void,
    reply_callback: TermyFfiClipboardReadReplyCallback,
) {
    let contents = [TermyFfiClipboardContent {
        mime_type: borrowed(b"text/plain"),
        data: borrowed(b"secret"),
    }];
    let response = TermyFfiClipboardReadResponse {
        status: RESULT_SUCCESS,
        contents_ptr: contents.as_ptr(),
        contents_len: contents.len(),
        ..TermyFfiClipboardReadResponse::default()
    };
    unsafe { reply_callback(reply_user_data, &response) };
}

unsafe extern "C" fn write_clipboard(
    user_data: *mut c_void,
    request: *const TermyFfiClipboardWriteRequest,
) -> TermyFfiClipboardWriteResponse {
    if user_data.is_null() || request.is_null() {
        return TermyFfiClipboardWriteResponse {
            status: RESULT_IO_ERROR,
            remember_permission: false,
        };
    }
    let state = unsafe { &mut *user_data.cast::<ClipboardCallbackState>() };
    let request = unsafe { &*request };
    if request.contents_len > 0 && request.contents_ptr.is_null() {
        return TermyFfiClipboardWriteResponse {
            status: RESULT_IO_ERROR,
            remember_permission: false,
        };
    }
    let contents = if request.contents_len == 0 {
        &[][..]
    } else {
        unsafe { slice::from_raw_parts(request.contents_ptr, request.contents_len) }
    };
    let Some(contents) = contents
        .iter()
        .map(|content| {
            Some((unsafe { read_string(content.mime_type) }?, unsafe {
                read_bytes(content.data)
            }?))
        })
        .collect::<Option<Vec<_>>>()
    else {
        return TermyFfiClipboardWriteResponse {
            status: RESULT_IO_ERROR,
            remember_permission: false,
        };
    };

    state.callbacks_valid &= !request.has_name && !request.permission_granted;
    state.write_location = request.location;
    state.write_contents = contents;
    TermyFfiClipboardWriteResponse {
        status: RESULT_SUCCESS,
        remember_permission: false,
    }
}

unsafe extern "C" fn protocol_reply(user_data: *mut c_void, reply: TermyFfiByteSlice) {
    if user_data.is_null() {
        return;
    }
    let Some(reply) = (unsafe { read_bytes(reply) }) else {
        return;
    };
    let state = unsafe { &mut *user_data.cast::<ClipboardCallbackState>() };
    state.protocol_replies.extend(reply);
}

#[test]
fn ffi_routes_kitty_clipboard_requests_and_paste_events() {
    let mut terminal: *mut TermyFfiTerminal = std::ptr::null_mut();
    assert_eq!(
        unsafe { termy_display_terminal_new(termy_size_default(), &mut terminal) },
        TermyFfiStatus::Ok
    );

    let output = b"\x1b[?5522h\
        \x1b]5522;type=read:id=read;dGV4dC9wbGFpbg==\x1b\\\
        \x1b]5522;type=write:id=write\x1b\\\
        \x1b]5522;type=wdata:mime=dGV4dC9wbGFpbg==;aGVsbG8=\x1b\\\
        \x1b]5522;type=wdata;\x1b\\";
    assert_eq!(
        unsafe { termy_terminal_feed_output(terminal, output.as_ptr(), output.len()) },
        TermyFfiStatus::Ok
    );

    let mut state = ClipboardCallbackState::default();
    let mut batch = TermyFfiEventBatch::default();
    assert_eq!(
        unsafe {
            termy_terminal_drain_events_with_clipboard(
                terminal,
                std::ptr::from_mut(&mut state).cast(),
                Some(read_clipboard),
                Some(write_clipboard),
                Some(protocol_reply),
                &mut batch,
            )
        },
        TermyFfiStatus::Ok
    );

    assert!(state.callbacks_valid);
    assert_eq!(state.read_location, CLIPBOARD_LOCATION);
    assert_eq!(state.read_mime_types, ["text/plain"]);
    assert_eq!(state.write_location, CLIPBOARD_LOCATION);
    assert_eq!(
        state.write_contents,
        [("text/plain".to_string(), b"hello".to_vec())]
    );
    let replies = String::from_utf8_lossy(&state.protocol_replies);
    assert!(replies.contains("type=read:status=OK:id=read"));
    assert!(replies.contains("type=read:status=DONE:id=read"));
    assert!(replies.contains("type=write:status=DONE:id=write"));

    let mut paste_events_enabled = false;
    assert_eq!(
        unsafe {
            termy_terminal_kitty_clipboard_paste_events_enabled(terminal, &mut paste_events_enabled)
        },
        TermyFfiStatus::Ok
    );
    assert!(paste_events_enabled);

    let formats = [borrowed(b"text/plain"), borrowed(b"image/png")];
    let mut sent = false;
    assert_eq!(
        unsafe {
            termy_terminal_send_kitty_clipboard_paste_event(
                terminal,
                CLIPBOARD_LOCATION,
                formats.as_ptr(),
                formats.len(),
                &mut sent,
            )
        },
        TermyFfiStatus::Ok
    );
    assert!(sent);

    assert_eq!(
        unsafe { termy_event_batch_free(&mut batch) },
        TermyFfiStatus::Ok
    );
    assert_eq!(
        unsafe {
            termy_terminal_drain_events_with_clipboard(
                terminal,
                std::ptr::from_mut(&mut state).cast(),
                Some(read_clipboard),
                Some(write_clipboard),
                Some(protocol_reply),
                &mut batch,
            )
        },
        TermyFfiStatus::Ok
    );
    let replies = String::from_utf8_lossy(&state.protocol_replies);
    assert!(replies.matches("type=read:status=OK").count() >= 2);
    assert_eq!(
        unsafe { termy_event_batch_free(&mut batch) },
        TermyFfiStatus::Ok
    );
    assert_eq!(unsafe { termy_terminal_free(terminal) }, TermyFfiStatus::Ok);
}

#[test]
fn ffi_rejects_unrequested_content_from_list_only_reads() {
    let mut terminal: *mut TermyFfiTerminal = std::ptr::null_mut();
    assert_eq!(
        unsafe { termy_display_terminal_new(termy_size_default(), &mut terminal) },
        TermyFfiStatus::Ok
    );
    let output = b"\x1b]5522;type=read:id=list;Lg==\x1b\\";
    assert_eq!(
        unsafe { termy_terminal_feed_output(terminal, output.as_ptr(), output.len()) },
        TermyFfiStatus::Ok
    );

    let mut state = ClipboardCallbackState::default();
    let mut batch = TermyFfiEventBatch::default();
    assert_eq!(
        unsafe {
            termy_terminal_drain_events_with_clipboard(
                terminal,
                std::ptr::from_mut(&mut state).cast(),
                Some(read_clipboard_with_unrequested_content),
                None,
                Some(protocol_reply),
                &mut batch,
            )
        },
        TermyFfiStatus::Ok
    );

    let replies = String::from_utf8_lossy(&state.protocol_replies);
    assert!(replies.contains("type=read:status=EBUSY:id=list"));
    assert!(!replies.contains("type=read:status=DATA"));
    assert!(!replies.contains("secret"));
    assert_eq!(
        unsafe { termy_event_batch_free(&mut batch) },
        TermyFfiStatus::Ok
    );
    assert_eq!(unsafe { termy_terminal_free(terminal) }, TermyFfiStatus::Ok);
}
