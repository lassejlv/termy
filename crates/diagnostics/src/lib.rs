//! Bounded, privacy-safe diagnostics shared by the native app and detached daemon.
//!
//! Events contain fixed identifiers only. Callers cannot attach terminal text, command arguments,
//! environment values, clipboard data, or filesystem paths by accident.

#![cfg(unix)]

use std::{
    fs::{self, File, OpenOptions},
    io::{Seek, SeekFrom, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, SystemTime},
};

use nix::unistd::Uid;

const MAX_LOG_BYTES: u64 = 512 * 1024;
const EVENT_RATE_LIMIT: Duration = Duration::from_secs(30);
const EVENT_COUNT: usize = 12;
const LOGGER_TAG: &str = "com.tmon.app";

/// A fixed diagnostic event. No variant accepts user or terminal data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    StartupFailed,
    RendererFailed,
    MultiplexerDaemonFailed,
    MultiplexerPeerRejected,
    MultiplexerFrameRejected,
    MultiplexerWriteFailed,
    MultiplexerDrainFailed,
    MultiplexerResizeFailed,
    MultiplexerTabFailed,
    ClipboardPasteRejected,
    ConfigOpenFailed,
    HyperlinkOpenFailed,
}

impl Event {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::StartupFailed => "app.startup_failed",
            Self::RendererFailed => "renderer.fatal_error",
            Self::MultiplexerDaemonFailed => "mux.daemon_failed",
            Self::MultiplexerPeerRejected => "mux.peer_rejected",
            Self::MultiplexerFrameRejected => "mux.frame_rejected",
            Self::MultiplexerWriteFailed => "mux.write_failed",
            Self::MultiplexerDrainFailed => "mux.drain_failed",
            Self::MultiplexerResizeFailed => "mux.resize_failed",
            Self::MultiplexerTabFailed => "mux.tab_failed",
            Self::ClipboardPasteRejected => "clipboard.paste_rejected",
            Self::ConfigOpenFailed => "config.open_failed",
            Self::HyperlinkOpenFailed => "hyperlink.open_failed",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        match code {
            "app.startup_failed" => Some(Self::StartupFailed),
            "renderer.fatal_error" => Some(Self::RendererFailed),
            "mux.daemon_failed" => Some(Self::MultiplexerDaemonFailed),
            "mux.peer_rejected" => Some(Self::MultiplexerPeerRejected),
            "mux.frame_rejected" => Some(Self::MultiplexerFrameRejected),
            "mux.write_failed" => Some(Self::MultiplexerWriteFailed),
            "mux.drain_failed" => Some(Self::MultiplexerDrainFailed),
            "mux.resize_failed" => Some(Self::MultiplexerResizeFailed),
            "mux.tab_failed" => Some(Self::MultiplexerTabFailed),
            "clipboard.paste_rejected" => Some(Self::ClipboardPasteRejected),
            "config.open_failed" => Some(Self::ConfigOpenFailed),
            "hyperlink.open_failed" => Some(Self::HyperlinkOpenFailed),
            _ => None,
        }
    }

    const fn index(self) -> usize {
        self as usize
    }

    const fn priority(self) -> &'static str {
        match self {
            Self::ClipboardPasteRejected
            | Self::MultiplexerPeerRejected
            | Self::MultiplexerFrameRejected => "user.warning",
            _ => "user.err",
        }
    }
}

/// Initializes the local diagnostic sink for the app or daemon process.
pub fn initialize() {
    let _ = LOGGER.get_or_init(Logger::default_for_current_user);
}

/// Records one fixed event when the process initialized diagnostics.
pub fn record(event: Event) {
    if let Some(logger) = LOGGER.get() {
        logger.record(event);
    }
}

/// Returns the conventional log path for support tooling without creating it.
#[must_use]
pub fn default_log_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| {
        PathBuf::from(home)
            .join("Library/Logs/Tmon")
            .join("events.log")
    })
}

/// One validated fixed-code event from the private local log.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticRecord {
    pub unix_seconds: u64,
    pub event: Event,
}

/// Reads at most the newest `limit` validated local event records.
///
/// The reader refuses symlinks, files owned by another user, non-private permissions, and files
/// larger than the sink's hard limit. Malformed or unknown lines are ignored so arbitrary log text
/// can never enter a support bundle.
///
/// # Errors
///
/// Returns an error when an existing log path violates the private-log contract or cannot be read.
pub fn recent_events(limit: usize) -> std::io::Result<Vec<DiagnosticRecord>> {
    let Some(path) = default_log_path() else {
        return Ok(Vec::new());
    };
    read_recent_events_at(&path, limit)
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

#[derive(Debug)]
struct Logger {
    state: Mutex<State>,
}

impl Logger {
    fn default_for_current_user() -> Self {
        let file = default_log_path().and_then(|path| open_private_log(&path).ok());
        let bytes_written = file
            .as_ref()
            .and_then(|file| file.metadata().ok())
            .map_or(0, |metadata| metadata.len().min(MAX_LOG_BYTES));
        Self {
            state: Mutex::new(State {
                file,
                bytes_written,
                max_bytes: MAX_LOG_BYTES,
                last_events: [None; EVENT_COUNT],
            }),
        }
    }

    fn record(&self, event: Event) {
        let now = Instant::now();
        let should_emit = self.state.lock().is_ok_and(|mut state| {
            let last = &mut state.last_events[event.index()];
            if last.is_some_and(|last| now.saturating_duration_since(last) < EVENT_RATE_LIMIT) {
                return false;
            }
            *last = Some(now);
            state.write(event);
            true
        });
        if should_emit {
            write_system_log(event);
        }
    }
}

#[derive(Debug)]
struct State {
    file: Option<File>,
    bytes_written: u64,
    max_bytes: u64,
    last_events: [Option<Instant>; EVENT_COUNT],
}

impl State {
    fn write(&mut self, event: Event) {
        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs());
        let line = format!("{timestamp} {}\n", event.code());
        let Ok(line_bytes) = u64::try_from(line.len()) else {
            return;
        };
        let Some(file) = &mut self.file else {
            return;
        };
        if self.bytes_written.saturating_add(line_bytes) > self.max_bytes {
            if file.set_len(0).is_err() || file.seek(SeekFrom::Start(0)).is_err() {
                self.file = None;
                return;
            }
            self.bytes_written = 0;
        }
        if file.write_all(line.as_bytes()).is_ok() && file.flush().is_ok() {
            self.bytes_written = self.bytes_written.saturating_add(line_bytes);
        } else {
            self.file = None;
        }
    }
}

fn open_private_log(path: &Path) -> std::io::Result<File> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "diagnostic path has no parent",
        )
    })?;
    match fs::symlink_metadata(parent) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true).mode(0o700).create(parent)?;
        }
        Err(error) => return Err(error),
    }
    let directory = fs::symlink_metadata(parent)?;
    if directory.file_type().is_symlink()
        || !directory.is_dir()
        || directory.uid() != current_user_id()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "diagnostic directory is not private and current-user-owned",
        ));
    }
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;

    if let Ok(metadata) = fs::symlink_metadata(path)
        && (metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.uid() != current_user_id())
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "diagnostic log is not a current-user-owned regular file",
        ));
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .mode(0o600)
        .open(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    if file.metadata()?.len() > MAX_LOG_BYTES {
        file.set_len(0)?;
    }
    Ok(file)
}

fn read_recent_events_at(path: &Path, limit: usize) -> std::io::Result<Vec<DiagnosticRecord>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "diagnostic path has no parent",
        )
    })?;
    let directory = fs::symlink_metadata(parent)?;
    if directory.file_type().is_symlink()
        || !directory.is_dir()
        || directory.uid() != current_user_id()
        || directory.permissions().mode() & 0o077 != 0
        || metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.uid() != current_user_id()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.len() > MAX_LOG_BYTES
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "diagnostic log does not satisfy the private-log contract",
        ));
    }
    if limit == 0 {
        return Ok(Vec::new());
    }
    let source = fs::read_to_string(path)?;
    let mut records = source
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_ascii_whitespace();
            let unix_seconds = fields.next()?.parse().ok()?;
            let event = Event::from_code(fields.next()?)?;
            fields.next().is_none().then_some(DiagnosticRecord {
                unix_seconds,
                event,
            })
        })
        .collect::<Vec<_>>();
    if records.len() > limit {
        records.drain(..records.len() - limit);
    }
    Ok(records)
}

fn current_user_id() -> u32 {
    Uid::effective().as_raw()
}

#[cfg(target_os = "macos")]
fn write_system_log(event: Event) {
    let _ = Command::new("/usr/bin/logger")
        .args(["-p", event.priority(), "-t", LOGGER_TAG, event.code()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

#[cfg(not(target_os = "macos"))]
fn write_system_log(_event: Event) {}

#[cfg(test)]
mod tests {
    use std::{fs, io::Write as _, os::unix::fs::PermissionsExt, time::SystemTime};

    use super::{Event, State, open_private_log, read_recent_events_at};

    #[test]
    fn private_log_is_bounded_and_contains_fixed_event_codes_only() {
        let root = std::env::temp_dir().join(format!(
            "tmon-diagnostics-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("clock follows Unix epoch")
                .as_nanos()
        ));
        let path = root.join("events.log");
        let file = open_private_log(&path).expect("open private test log");
        let mut state = State {
            file: Some(file),
            bytes_written: 0,
            max_bytes: 96,
            last_events: [None; super::EVENT_COUNT],
        };
        for _ in 0..20 {
            state.write(Event::RendererFailed);
        }
        drop(state);

        let contents = fs::read_to_string(&path).expect("read bounded log");
        assert!(contents.len() <= 96);
        assert!(contents.lines().all(|line| {
            let mut fields = line.split_ascii_whitespace();
            fields
                .next()
                .is_some_and(|value| value.parse::<u64>().is_ok())
                && fields.next() == Some("renderer.fatal_error")
                && fields.next().is_none()
        }));
        assert_eq!(
            fs::metadata(&root)
                .expect("diagnostic directory metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path)
                .expect("diagnostic file metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let records = read_recent_events_at(&path, 2).expect("read private diagnostic records");
        assert_eq!(records.len(), 2);
        assert!(
            records
                .iter()
                .all(|record| record.event == Event::RendererFailed)
        );
        fs::remove_dir_all(root).expect("remove diagnostic test root");
    }

    #[test]
    fn support_reader_ignores_unknown_or_malformed_content() {
        let root = std::env::temp_dir().join(format!(
            "tmon-diagnostics-reader-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("clock follows Unix epoch")
                .as_nanos()
        ));
        let path = root.join("events.log");
        let mut file = open_private_log(&path).expect("open private test log");
        writeln!(file, "10 app.startup_failed").expect("write valid event");
        writeln!(file, "terminal text must never escape").expect("write malformed event");
        writeln!(file, "20 unknown.event").expect("write unknown event");
        writeln!(file, "30 mux.daemon_failed extra").expect("write overlong event");
        drop(file);

        assert_eq!(
            read_recent_events_at(&path, 10).expect("read sanitized events"),
            vec![super::DiagnosticRecord {
                unix_seconds: 10,
                event: Event::StartupFailed,
            }]
        );
        fs::remove_dir_all(root).expect("remove diagnostic test root");
    }
}
