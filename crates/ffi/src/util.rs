//! Small checked pointer and conversion helpers used by ABI entry points.

use std::{ptr::NonNull, slice};

use crate::{error::FfiError, types::TmonByteSlice};

pub(crate) unsafe fn required_ref<'a, T>(pointer: *const T, name: &str) -> Result<&'a T, FfiError> {
    // SAFETY: The caller of this helper owns the FFI contract. `as_ref` only creates the
    // reference after the null check; validity and alignment are documented requirements.
    unsafe { pointer.as_ref() }.ok_or_else(|| FfiError::null(name))
}

pub(crate) unsafe fn required_mut<'a, T>(
    pointer: *mut T,
    name: &str,
) -> Result<&'a mut T, FfiError> {
    // SAFETY: See `required_ref`; the exported function also requires exclusive access.
    unsafe { pointer.as_mut() }.ok_or_else(|| FfiError::null(name))
}

pub(crate) unsafe fn write_out<T>(pointer: *mut T, value: T, name: &str) -> Result<(), FfiError> {
    let pointer = NonNull::new(pointer).ok_or_else(|| FfiError::null(name))?;
    // SAFETY: The exported ABI requires `pointer` to identify writable, properly aligned storage
    // for one `T`. `ptr::write` also supports uninitialized out parameters.
    unsafe { pointer.as_ptr().write(value) };
    Ok(())
}

pub(crate) unsafe fn bytes_from_raw<'a>(
    pointer: *const u8,
    length: usize,
    name: &str,
) -> Result<&'a [u8], FfiError> {
    if length == 0 {
        return Ok(&[]);
    }
    if pointer.is_null() {
        return Err(FfiError::null(name));
    }
    // SAFETY: The exported ABI requires `length` readable bytes at `pointer`. The non-empty null
    // case was rejected above.
    Ok(unsafe { slice::from_raw_parts(pointer, length) })
}

pub(crate) unsafe fn bytes_from_view<'a>(
    view: TmonByteSlice,
    name: &str,
) -> Result<&'a [u8], FfiError> {
    // SAFETY: Forwarded from the owning exported function's byte-slice contract.
    unsafe { bytes_from_raw(view.data, view.length, name) }
}

pub(crate) unsafe fn utf8_from_view<'a>(
    view: TmonByteSlice,
    name: &str,
) -> Result<&'a str, FfiError> {
    // SAFETY: Forwarded from the owning exported function's byte-slice contract.
    let bytes = unsafe { bytes_from_view(view, name)? };
    std::str::from_utf8(bytes).map_err(|_| FfiError::utf8(name))
}

pub(crate) unsafe fn views_from_raw<'a, T>(
    pointer: *const T,
    length: usize,
    name: &str,
) -> Result<&'a [T], FfiError> {
    if length == 0 {
        return Ok(&[]);
    }
    if pointer.is_null() {
        return Err(FfiError::null(name));
    }
    // SAFETY: The exported ABI requires `length` readable, aligned values at `pointer`.
    Ok(unsafe { slice::from_raw_parts(pointer, length) })
}

pub(crate) fn slice_view(bytes: &[u8]) -> TmonByteSlice {
    TmonByteSlice {
        data: if bytes.is_empty() {
            std::ptr::null()
        } else {
            bytes.as_ptr()
        },
        length: bytes.len(),
    }
}

pub(crate) fn slice_pointer<T>(values: &[T]) -> *const T {
    if values.is_empty() {
        std::ptr::null()
    } else {
        values.as_ptr()
    }
}

pub(crate) fn to_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}
