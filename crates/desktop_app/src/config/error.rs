use std::{fmt, io, path::PathBuf, process::ExitStatus};

#[derive(Debug)]
pub enum ConfigIoError {
    ConfigPathUnavailable,
    InvalidConfigPath(PathBuf),
    CreateDir {
        path: PathBuf,
        source: io::Error,
    },
    ReadConfig {
        path: PathBuf,
        source: io::Error,
    },
    WriteConfig {
        path: PathBuf,
        source: io::Error,
    },
    CreateTempFile {
        path: PathBuf,
        source: io::Error,
    },
    PersistTempFile {
        path: PathBuf,
        source: io::Error,
    },
    LaunchOpenCommand {
        command: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    // Windows opens files through ShellExecuteW, which has no exit status.
    #[cfg_attr(target_os = "windows", allow(dead_code))]
    OpenCommandFailed {
        command: &'static str,
        path: PathBuf,
        status: ExitStatus,
    },
}

impl fmt::Display for ConfigIoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConfigPathUnavailable => write!(f, "Unable to determine config file path"),
            Self::InvalidConfigPath(path) => {
                write!(f, "Invalid config file path: {}", path.display())
            }
            Self::CreateDir { path, source } => {
                write!(
                    f,
                    "Failed to create config directory '{}': {}",
                    path.display(),
                    source
                )
            }
            Self::ReadConfig { path, source } => {
                write!(
                    f,
                    "Failed to read config file '{}': {}",
                    path.display(),
                    source
                )
            }
            Self::WriteConfig { path, source } => {
                write!(
                    f,
                    "Failed to write config file '{}': {}",
                    path.display(),
                    source
                )
            }
            Self::CreateTempFile { path, source } => {
                write!(
                    f,
                    "Failed to create temp config file near '{}': {}",
                    path.display(),
                    source
                )
            }
            Self::PersistTempFile { path, source } => {
                write!(
                    f,
                    "Failed to persist config file '{}': {}",
                    path.display(),
                    source
                )
            }
            Self::LaunchOpenCommand {
                command,
                path,
                source,
            } => {
                write!(
                    f,
                    "Failed to launch '{}' for '{}': {}",
                    command,
                    path.display(),
                    source
                )
            }
            Self::OpenCommandFailed {
                command,
                path,
                status,
            } => {
                write!(
                    f,
                    "'{}' failed for '{}' with status {}",
                    command,
                    path.display(),
                    status
                )
            }
        }
    }
}

impl std::error::Error for ConfigIoError {}
