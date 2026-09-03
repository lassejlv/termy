//! Versioned C ABI for embedding the headless `Tmon` engine.
//!
//! The C header in `include/tmon.h` is the foreign-language source of truth. Rust hosts
//! should depend on `engine` directly.

#![deny(unsafe_op_in_unsafe_fn)]
#![allow(
    clippy::missing_safety_doc,
    reason = "the complete pointer and lifetime contract is maintained in the public C header"
)]

mod error;
mod pty;
mod terminal;
mod types;
mod util;

use std::ffi::c_char;

pub use pty::*;
pub use terminal::*;
pub use types::*;

use crate::{error::ffi_value, util::slice_view};

#[unsafe(no_mangle)]
pub const extern "C" fn tmon_abi_version() -> u32 {
    TMON_ABI_VERSION
}

#[unsafe(no_mangle)]
pub extern "C" fn tmon_library_version() -> TmonByteSlice {
    ffi_value(TmonByteSlice::empty(), || {
        slice_view(env!("CARGO_PKG_VERSION").as_bytes())
    })
}

/// Returns a NUL-terminated diagnostic owned by thread-local storage.
///
/// The pointer remains valid until another `Tmon` FFI call on the same thread fails or succeeds.
#[unsafe(no_mangle)]
pub extern "C" fn tmon_last_error_message() -> *const c_char {
    error::last_error_pointer()
}
