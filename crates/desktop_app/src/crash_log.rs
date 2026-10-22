use std::{
    backtrace::Backtrace,
    env,
    ffi::OsString,
    fs,
    io::{self, Write},
    panic::{self, PanicHookInfo},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const CRASH_LOG_ENV: &str = "TERMY_CRASH_LOG_PATH";
/// The log file is rotated to `<name>.1` once it grows past this size.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const MAX_LOG_FILE_BYTES: u64 = 5 * 1024 * 1024;

/// Installs the crash-log panic hook. Only the GUI process installs it, so
/// the Windows crash dialog never appears for CLI delegation.
pub(crate) fn install_panic_hook() {
    let previous_hook = panic::take_hook();
    #[cfg(target_os = "windows")]
    let main_thread = std::thread::current().id();

    panic::set_hook(Box::new(move |info| {
        let path = crash_log_path();
        let written = match append_panic_report(&path, info) {
            Ok(()) => true,
            Err(error) => {
                eprintln!("Failed to write Termy crash log: {error}");
                false
            }
        };
        log::error!(
            "Termy panicked: {} (crash log: {})",
            panic_message(info),
            path.display()
        );
        previous_hook(info);
        // Main-thread panics end the process; a background thread's panic
        // is recorded without interrupting the user.
        #[cfg(target_os = "windows")]
        if std::thread::current().id() == main_thread {
            show_crash_dialog(&path, written);
        }
        #[cfg(not(target_os = "windows"))]
        let _ = written;
    }));
}

/// Routes `log` output for the GUI process. RUST_LOG keeps its usual
/// meaning. Windows GUI builds have no stderr when started from Explorer or
/// a shortcut, so they also append to [`log_file_path`].
pub(crate) fn init_logging() {
    #[cfg(target_os = "windows")]
    {
        let writer = TeeLogWriter {
            file: RotatingLogFile::open(log_file_path(), MAX_LOG_FILE_BYTES).ok(),
        };
        // Warnings are worth keeping when nobody watches stderr.
        let env = env_logger::Env::default().default_filter_or("warn");
        let _ = env_logger::Builder::from_env(env)
            .target(env_logger::Target::Pipe(Box::new(writer)))
            .try_init();
    }
    #[cfg(not(target_os = "windows"))]
    env_logger::init();
}

pub(crate) fn crash_log_path() -> PathBuf {
    crash_log_path_with(
        |name| env::var(name).ok(),
        dirs::data_local_dir(),
        env::current_dir().ok(),
    )
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn log_file_path() -> PathBuf {
    log_file_path_with(dirs::data_local_dir())
}

fn log_file_path_with(data_local_dir: Option<PathBuf>) -> PathBuf {
    data_local_dir
        .unwrap_or_else(env::temp_dir)
        .join("termy")
        .join("logs")
        .join("termy.log")
}

fn crash_log_path_with(
    get_var: impl Fn(&str) -> Option<String>,
    data_local_dir: Option<PathBuf>,
    current_dir: Option<PathBuf>,
) -> PathBuf {
    if let Some(path) = get_var(CRASH_LOG_ENV).filter(|path| !path.trim().is_empty()) {
        return PathBuf::from(path);
    }

    if let Some(data_local_dir) = data_local_dir {
        return data_local_dir.join("termy").join("crash.log");
    }

    current_dir
        .unwrap_or_else(env::temp_dir)
        .join("termy-crash.log")
}

fn append_panic_report(path: &Path, info: &PanicHookInfo<'_>) -> io::Result<()> {
    let message = panic_message(info);
    let location = info.location().map_or_else(
        || "unknown".to_string(),
        |location| {
            format!(
                "{}:{}:{}",
                location.file(),
                location.line(),
                location.column()
            )
        },
    );
    let backtrace = Backtrace::force_capture().to_string();

    append_crash_report(path, &message, &location, &backtrace)
}

fn append_crash_report(
    path: &Path,
    message: &str,
    location: &str,
    backtrace: &str,
) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "===== Termy panic =====")?;
    writeln!(file, "time_unix_secs={}", unix_timestamp_secs())?;
    writeln!(file, "version={}", crate::APP_VERSION)?;
    writeln!(file, "thread={:?}", std::thread::current().name())?;
    writeln!(file, "location={location}")?;
    writeln!(file, "message={message}")?;
    writeln!(file, "backtrace:\n{backtrace}")?;
    writeln!(file)?;
    Ok(())
}

fn panic_message(info: &PanicHookInfo<'_>) -> String {
    if let Some(message) = info.payload().downcast_ref::<&str>() {
        return (*message).to_string();
    }
    if let Some(message) = info.payload().downcast_ref::<String>() {
        return message.clone();
    }
    "<non-string panic payload>".to_string()
}

fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn crash_dialog_message(path: &Path, written: bool) -> String {
    if written {
        format!(
            "Termy crashed unexpectedly.\n\nA crash report was saved to:\n{}\n\nPlease attach it when reporting the problem at https://github.com/lassejlv/termy/issues.",
            path.display()
        )
    } else {
        format!(
            "Termy crashed unexpectedly, and the crash report could not be saved to:\n{}",
            path.display()
        )
    }
}

#[cfg(target_os = "windows")]
fn show_crash_dialog(path: &Path, written: bool) {
    use std::sync::atomic::{AtomicBool, Ordering};
    use windows::Win32::UI::WindowsAndMessaging::{
        MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MB_TOPMOST, MessageBoxW,
    };
    use windows::core::PCWSTR;

    static SHOWN: AtomicBool = AtomicBool::new(false);
    if SHOWN.swap(true, Ordering::SeqCst) {
        return;
    }
    let message = crash_dialog_message(path, written);
    // The dialog's modal loop runs on its own thread so it cannot dispatch
    // messages into the GPUI windows of the thread that is panicking.
    let dialog = std::thread::Builder::new()
        .name("termy-crash-dialog".to_string())
        .spawn(move || {
            let text = wide_null(&message);
            let caption = wide_null("Termy crashed");
            // SAFETY: both buffers are null-terminated and outlive the call.
            unsafe {
                MessageBoxW(
                    None,
                    PCWSTR(text.as_ptr()),
                    PCWSTR(caption.as_ptr()),
                    MB_OK | MB_ICONERROR | MB_SETFOREGROUND | MB_TOPMOST,
                );
            }
        });
    if let Ok(dialog) = dialog {
        let _ = dialog.join();
    }
}

#[cfg(target_os = "windows")]
fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Writes each log record to stderr (useful under `cargo run`) and, when it
/// could be opened, to the log file. Logging never fails the caller.
#[cfg(target_os = "windows")]
struct TeeLogWriter {
    file: Option<RotatingLogFile>,
}

#[cfg(target_os = "windows")]
impl Write for TeeLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let _ = io::stderr().write_all(buf);
        if let Some(file) = self.file.as_mut() {
            let _ = file.write_all(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let _ = io::stderr().flush();
        if let Some(file) = self.file.as_mut() {
            let _ = file.flush();
        }
        Ok(())
    }
}

/// An append-only log file that moves itself to `<name>.1` (replacing any
/// older one) once it exceeds `max_len` bytes.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
struct RotatingLogFile {
    path: PathBuf,
    file: fs::File,
    len: u64,
    max_len: u64,
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
impl RotatingLogFile {
    fn open(path: PathBuf, max_len: u64) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = open_append(&path)?;
        let len = file.metadata().map_or(0, |metadata| metadata.len());
        let mut log = Self {
            path,
            file,
            len,
            max_len,
        };
        if log.len >= log.max_len {
            log.rotate();
        }
        Ok(log)
    }

    fn rotate(&mut self) {
        let rotated = rotated_log_path(&self.path);
        // On failure keep appending to the current file and retry after
        // another `max_len` bytes rather than on every record.
        if let Ok(file) = fs::rename(&self.path, &rotated).and_then(|()| open_append(&self.path)) {
            self.file = file;
        }
        self.len = 0;
    }
}

impl Write for RotatingLogFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.len > 0 && self.len.saturating_add(buf.len() as u64) > self.max_len {
            self.rotate();
        }
        let written = self.file.write(buf)?;
        self.len = self.len.saturating_add(written as u64);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

fn open_append(path: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new().create(true).append(true).open(path)
}

fn rotated_log_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map_or_else(|| OsString::from("termy.log"), OsString::from);
    name.push(".1");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crash_log_path_prefers_env_override() {
        let path = crash_log_path_with(
            |name| (name == CRASH_LOG_ENV).then(|| "/tmp/custom-crash.log".to_string()),
            Some(PathBuf::from("/tmp/data")),
            Some(PathBuf::from("/tmp/cwd")),
        );

        assert_eq!(path, PathBuf::from("/tmp/custom-crash.log"));
    }

    #[test]
    fn crash_log_path_uses_local_data_directory() {
        let path = crash_log_path_with(
            |_| None,
            Some(PathBuf::from("/tmp/data")),
            Some(PathBuf::from("/tmp/cwd")),
        );

        assert_eq!(path, PathBuf::from("/tmp/data/termy/crash.log"));
    }

    #[test]
    fn crash_log_path_falls_back_to_current_directory() {
        let path = crash_log_path_with(|_| None, None, Some(PathBuf::from("/tmp/cwd")));

        assert_eq!(path, PathBuf::from("/tmp/cwd/termy-crash.log"));
    }

    #[test]
    fn log_file_lives_in_the_local_data_logs_directory() {
        assert_eq!(
            log_file_path_with(Some(PathBuf::from("/tmp/data"))),
            PathBuf::from("/tmp/data/termy/logs/termy.log")
        );
    }

    #[test]
    fn log_file_rotates_once_it_exceeds_the_size_cap() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("logs").join("termy.log");
        let mut log = RotatingLogFile::open(path.clone(), 16).expect("open log");

        log.write_all(b"0123456789\n").expect("first record");
        log.write_all(b"abcdefghij\n").expect("second record");
        log.flush().expect("flush");

        assert_eq!(
            fs::read_to_string(rotated_log_path(&path)).expect("rotated log"),
            "0123456789\n"
        );
        assert_eq!(fs::read_to_string(&path).expect("log"), "abcdefghij\n");
    }

    #[test]
    fn oversized_log_file_is_rotated_when_opened() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("termy.log");
        fs::write(&path, "x".repeat(32)).expect("old log");

        let mut log = RotatingLogFile::open(path.clone(), 16).expect("open log");
        log.write_all(b"fresh\n").expect("record");

        assert_eq!(rotated_log_path(&path), temp.path().join("termy.log.1"));
        assert_eq!(
            fs::read_to_string(rotated_log_path(&path)).expect("rotated log"),
            "x".repeat(32)
        );
        assert_eq!(fs::read_to_string(&path).expect("log"), "fresh\n");
    }

    #[test]
    fn crash_dialog_points_at_the_crash_log() {
        let path = PathBuf::from("/tmp/data/termy/crash.log");
        let message = crash_dialog_message(&path, true);
        assert!(message.contains(&path.display().to_string()));
        assert!(message.contains("crash report was saved"));
        assert!(crash_dialog_message(&path, false).contains("could not be saved"));
    }

    #[test]
    fn append_crash_report_creates_parent_and_appends() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("nested").join("crash.log");

        append_crash_report(&path, "first panic", "first.rs:1:2", "first trace")
            .expect("write first crash report");
        append_crash_report(&path, "second panic", "second.rs:3:4", "second trace")
            .expect("write second crash report");

        let contents = fs::read_to_string(path).expect("read crash log");
        assert!(contents.contains("version="));
        assert!(contents.contains("first panic"));
        assert!(contents.contains("first.rs:1:2"));
        assert!(contents.contains("first trace"));
        assert!(contents.contains("second panic"));
        assert!(contents.contains("second.rs:3:4"));
        assert!(contents.contains("second trace"));
        assert_eq!(contents.matches("===== Termy panic =====").count(), 2);
    }
}
