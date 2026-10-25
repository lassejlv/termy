//! Built-in, display-independent terminal session host.
mod client;
mod discovery;
mod escape;
mod host;
mod layout;
mod pane;
mod protocol;
mod server;
#[cfg(test)]
mod wire_golden;

pub use client::SessionClient;
pub use discovery::{
    VersionMismatch, connect_or_start, default_root, host_is_running, is_version_mismatch,
    kill_host, shutdown_host,
};
pub use host::run_host;
pub use protocol::{PaneInfo, PaneLaunch};
pub use server::serve;
