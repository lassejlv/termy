//! Built-in, display-independent terminal session host.
mod client;
mod discovery;
mod layout;
mod pane;
mod protocol;
mod server;

pub use client::SessionClient;
pub use discovery::connect_or_start;
pub use protocol::{PaneInfo, PaneLaunch};
pub use server::serve;
