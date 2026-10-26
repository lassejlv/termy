//! Built-in, display-independent terminal session host.
mod client;
mod discovery;
mod escape;
mod host;
mod layout;
mod layout_store;
mod pane;
mod protocol;
mod server;
#[cfg(windows)]
mod windows_acl;
#[cfg(test)]
mod wire_golden;

pub use client::{LayoutUpdate, LayoutWatch, SessionClient, is_connection_error};
pub use discovery::{
    VersionMismatch, connect_or_start, default_root, host_is_running, is_host_unavailable,
    is_version_mismatch, kill_host, shutdown_host,
};
pub use host::run_host;
pub use protocol::{InputCommand, LaunchConfig, PaneInfo, PaneLaunch, ShellState};
pub use server::{is_exited_pane, is_missing_pane, serve};
