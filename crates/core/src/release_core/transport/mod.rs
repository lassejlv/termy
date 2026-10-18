use std::time::Duration;

pub mod github;

pub const USER_AGENT: &str = "Termy-Updater/1.0";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Per read/write, not per request: a large installer download on a slow
/// link keeps going as long as bytes keep arriving.
const IO_TIMEOUT: Duration = Duration::from_secs(30);

/// HTTP client for release lookups and update downloads. A bare `ureq::get`
/// has no timeouts, so a stalled connection would block the updater forever.
pub fn http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout_read(IO_TIMEOUT)
        .timeout_write(IO_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
}
