//! Shared tmux control-mode (`tmux -CC`) protocol core: command construction,
//! payload (un)escaping, the control-stream parser/state machine, notification
//! coalescing, and worker channel plumbing shared by the GPUI app and the
//! FFI/native host.

pub mod command;
pub mod control;
pub mod payload;
pub mod session;
pub mod types;
