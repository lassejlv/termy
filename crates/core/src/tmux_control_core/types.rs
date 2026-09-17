#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxRuntimeConfig {
    pub binary: String,
    /// Argv prefix used to reach the tmux binary, e.g. `["wsl.exe", "-e"]` or
    /// `["ssh", "myhost"]`. When non-empty, the control client and out-of-band
    /// tmux commands run through this prefix with piped stdio instead of a
    /// locally spawned tmux on a pty, which also works on non-unix hosts.
    pub command_prefix: Vec<String>,
    pub launch: TmuxLaunchTarget,
    pub show_active_pane_border: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TmuxSocketTarget {
    DedicatedTermy,
    Default,
    Named(String),
}

pub const TERMY_TMUX_SOCKET_NAME: &str = "termy";

impl TmuxSocketTarget {
    pub fn socket_name(&self) -> Option<&str> {
        match self {
            Self::DedicatedTermy => Some(TERMY_TMUX_SOCKET_NAME),
            Self::Default => None,
            Self::Named(name) => Some(name.as_str()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TmuxLaunchTarget {
    Managed {
        persistence: bool,
    },
    Session {
        name: String,
        socket: TmuxSocketTarget,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TmuxShutdownMode {
    DetachOnly,
    DetachAndTeardownSession,
}

impl Default for TmuxRuntimeConfig {
    fn default() -> Self {
        Self {
            binary: "tmux".to_string(),
            command_prefix: Vec::new(),
            launch: TmuxLaunchTarget::Managed { persistence: false },
            show_active_pane_border: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TmuxPaneMouseMode {
    pub standard: bool,
    pub button: bool,
    pub any: bool,
    pub sgr: bool,
    pub utf8: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxPaneState {
    pub id: String,
    pub window_id: String,
    pub session_id: String,
    pub is_active: bool,
    pub left: u16,
    pub top: u16,
    pub width: u16,
    pub height: u16,
    pub cursor_x: u16,
    pub cursor_y: u16,
    pub mouse_mode: TmuxPaneMouseMode,
    pub current_path: String,
    pub current_command: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxWindowState {
    pub id: String,
    pub index: i32,
    pub name: String,
    pub layout: String,
    pub is_active: bool,
    pub automatic_rename: bool,
    pub active_pane_id: Option<String>,
    pub panes: Vec<TmuxPaneState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxSnapshot {
    pub session_name: String,
    pub session_id: Option<String>,
    pub windows: Vec<TmuxWindowState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxSessionSummary {
    pub name: String,
    pub id: String,
    pub window_count: u16,
    pub attached_clients: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TmuxNotification {
    Output {
        pane_id: String,
        bytes: Vec<u8>,
    },
    NeedsRefresh,
    /// A `%subscription-changed` notification: a format registered via
    /// `TmuxClient::subscribe` changed value. `pane` is `-` for
    /// window- or session-scoped subscriptions.
    SubscriptionChanged {
        name: String,
        session: String,
        window: String,
        pane: String,
        value: String,
    },
    Warning(String),
    Exit(Option<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TmuxControlErrorKind {
    Channel,
    Protocol,
    Parse,
    Runtime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxControlError {
    pub kind: TmuxControlErrorKind,
    pub message: String,
}

impl TmuxControlError {
    pub fn channel(message: impl Into<String>) -> Self {
        Self {
            kind: TmuxControlErrorKind::Channel,
            message: message.into(),
        }
    }

    pub fn protocol(message: impl Into<String>) -> Self {
        Self {
            kind: TmuxControlErrorKind::Protocol,
            message: message.into(),
        }
    }

    pub fn parse(message: impl Into<String>) -> Self {
        Self {
            kind: TmuxControlErrorKind::Parse,
            message: message.into(),
        }
    }

    pub fn runtime(message: impl Into<String>) -> Self {
        Self {
            kind: TmuxControlErrorKind::Runtime,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for TmuxControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self.kind {
            TmuxControlErrorKind::Channel => "channel",
            TmuxControlErrorKind::Protocol => "protocol",
            TmuxControlErrorKind::Parse => "parse",
            TmuxControlErrorKind::Runtime => "runtime",
        };
        write!(f, "tmux control {} error: {}", kind, self.message)
    }
}

impl std::error::Error for TmuxControlError {}
