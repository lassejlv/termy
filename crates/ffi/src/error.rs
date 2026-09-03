//! Panic containment, status mapping, and thread-local diagnostics.

use std::{
    any::Any,
    cell::RefCell,
    ffi::{CString, c_char},
    panic::{AssertUnwindSafe, catch_unwind},
};

use crate::types::{
    TMON_ENGINE_ERROR, TMON_INVALID_ARGUMENT, TMON_INVALID_UTF8, TMON_NULL_POINTER, TMON_OK,
    TMON_PANICKED,
};

thread_local! {
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::default());
}

#[derive(Debug)]
pub(crate) struct FfiError {
    status: u32,
    message: String,
}

impl FfiError {
    pub(crate) fn null(name: &str) -> Self {
        Self {
            status: TMON_NULL_POINTER,
            message: format!("{name} must not be null"),
        }
    }

    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self {
            status: TMON_INVALID_ARGUMENT,
            message: message.into(),
        }
    }

    pub(crate) fn utf8(name: &str) -> Self {
        Self {
            status: TMON_INVALID_UTF8,
            message: format!("{name} must contain valid UTF-8"),
        }
    }

    pub(crate) fn engine(error: impl std::fmt::Display) -> Self {
        Self {
            status: TMON_ENGINE_ERROR,
            message: error.to_string(),
        }
    }
}

pub(crate) fn ffi_status(operation: impl FnOnce() -> Result<(), FfiError>) -> u32 {
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(Ok(())) => {
            set_last_error("");
            TMON_OK
        }
        Ok(Err(error)) => {
            set_last_error(&error.message);
            error.status
        }
        Err(payload) => {
            set_last_error(&format!(
                "Tmon panicked across an FFI boundary: {}",
                panic_message(payload.as_ref())
            ));
            TMON_PANICKED
        }
    }
}

pub(crate) fn ffi_value<T>(fallback: T, operation: impl FnOnce() -> T) -> T {
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(value) => value,
        Err(payload) => {
            set_last_error(&format!(
                "Tmon panicked across an FFI boundary: {}",
                panic_message(payload.as_ref())
            ));
            fallback
        }
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("unknown panic")
}

fn set_last_error(message: &str) {
    let sanitized = message.replace('\0', "\\0");
    let value = CString::new(sanitized).unwrap_or_default();
    LAST_ERROR.with(|slot| *slot.borrow_mut() = value);
}

pub(crate) fn last_error_pointer() -> *const c_char {
    LAST_ERROR.with(|slot| slot.borrow().as_ptr())
}
