//! Terminal defaults and launch choices shared by configuration and runtime.

pub(crate) const DEFAULT_TERM: &str = "xterm-256color";
pub(crate) const DEFAULT_COLORTERM: &str = "truecolor";
pub(crate) const DEFAULT_SCROLLBACK_HISTORY: usize = 1000;

/// Upper clamp for native terminals' scrollback rows. Both config parsing and
/// runtime/FFI setters enforce this limit, including callers that bypass config.
/// The engine additionally bounds history by its byte budget.
pub const MAX_TERMINAL_SCROLLBACK_HISTORY: usize = 20_000;

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkingDirFallback {
    Home,
    Process,
}

impl WorkingDirFallback {
    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "home" | "user" => Some(Self::Home),
            "process" | "cwd" => Some(Self::Process),
            _ => None,
        }
    }
}

#[allow(clippy::derivable_impls)]
impl Default for WorkingDirFallback {
    fn default() -> Self {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            Self::Home
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            Self::Process
        }
    }
}

/// Value advertised to child processes through `TERM_PROGRAM`.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TermProgram {
    Termy,
    /// Claude Code and similar CLIs gate terminal progress escape sequences on
    /// known terminal identities. Termy supports Ghostty's OSC progress
    /// protocol, so this advertises that compatibility while keeping TERM
    /// conservative for terminfo.
    #[default]
    Ghostty,
}

impl std::str::FromStr for TermProgram {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "termy" => Ok(Self::Termy),
            "ghostty" => Ok(Self::Ghostty),
            _ => Err(()),
        }
    }
}

impl TermProgram {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Termy => "termy",
            Self::Ghostty => "ghostty",
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowsShell {
    Cmd,
    PowerShell,
    PowerShellCore,
    GitBash,
    /// PowerShell 7 (`pwsh.exe`) when it is installed, else Windows
    /// PowerShell, else the `COMSPEC` command processor. Declared last so
    /// serialized indices of the explicit presets stay stable.
    #[default]
    Auto,
}

impl WindowsShell {
    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" | "automatic" => Some(Self::Auto),
            "cmd" | "command_prompt" | "commandprompt" => Some(Self::Cmd),
            "powershell" | "power_shell" | "windows_powershell" | "ps" => Some(Self::PowerShell),
            "pwsh" | "powershell_core" | "powershellcore" | "powershell_7" | "powershell-7"
            | "powershell7" | "power_shell_7" | "power_shell_core" => Some(Self::PowerShellCore),
            "git_bash" | "gitbash" | "git-bash" | "bash" => Some(Self::GitBash),
            _ => None,
        }
    }
}
