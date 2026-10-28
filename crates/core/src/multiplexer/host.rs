//! Process set-up shared by every session host entry point. The host's logger
//! and panic hook write to `host.log` in its private directory, rotating it
//! to `host.log.1` past [`HOST_LOG_LIMIT`]. `connect_or_start` also appends
//! the host's stderr to `host.log`, which catches anything written before the
//! logger exists, such as an entry point's start-up error.
use crate::multiplexer::{discovery::HOST_LOG_LIMIT, protocol::build_id, serve};
use std::{
    collections::HashMap,
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Each distinct message is written at most this many times per
/// [`REPEAT_INTERVAL`]; further repeats are counted and reported once.
const REPEAT_BURST: u32 = 5;
const REPEAT_INTERVAL: Duration = Duration::from_secs(60);
/// Distinct messages tracked at once; beyond this every count is reported.
const MAX_TRACKED: usize = 256;
/// A panic message, with its backtrace, is cut to this size.
const MAX_PANIC_BYTES: usize = 16 * 1024;

/// Runs a session host in `root` until it is shut down. Entry points call
/// this rather than [`serve`], and report its error on stderr before exiting.
pub fn run_host(root: &Path) -> anyhow::Result<()> {
    install_logger(root);
    std::panic::set_hook(Box::new(|info| {
        let thread = std::thread::current();
        // The key leaves out the backtrace, so a panic that repeats in every
        // client thread is rate-limited like any other message.
        let key = format!(
            "PANIC session host panicked in thread {}: {info}",
            thread.name().unwrap_or("unnamed")
        );
        let mut text = format!("{key}\n{}", std::backtrace::Backtrace::force_capture());
        truncate(&mut text, MAX_PANIC_BYTES);
        match LOGGER.get() {
            Some(logger) => logger.record(&key, &text),
            None => {
                let _ = writeln!(std::io::stderr(), "{} {text}", timestamp());
            }
        }
    }));
    log::info!(
        "session host started: {}, pid {}",
        build_id(),
        std::process::id()
    );
    let result = serve(root);
    if result.is_ok() {
        log::info!("session host stopped");
    }
    result
}

static LOGGER: OnceLock<HostLogger> = OnceLock::new();

fn install_logger(root: &Path) {
    let level = std::env::var("RUST_LOG")
        .ok()
        .and_then(|level| level.parse().ok())
        .unwrap_or(log::LevelFilter::Info);
    let logger = LOGGER.get_or_init(|| HostLogger {
        sink: Mutex::new(Sink::open(Some(root.join("host.log")), HOST_LOG_LIMIT)),
    });
    if log::set_logger(logger).is_ok() {
        log::set_max_level(level);
    }
}

/// Opens `host.log` again after the session directory was recreated: the
/// file the logger has open went away with the old directory.
pub(crate) fn reopen_log() {
    let Some(logger) = LOGGER.get() else {
        return;
    };
    let mut sink = logger
        .sink
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some(file) = sink.path.as_deref().and_then(open_log) {
        sink.file = Some(file);
        sink.written = 0;
    }
}

fn timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("[{}.{:03}]", now.as_secs(), now.subsec_millis())
}

fn truncate(text: &mut String, limit: usize) {
    if text.len() > limit {
        let mut end = limit;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("\n[truncated]");
    }
}

struct HostLogger {
    sink: Mutex<Sink>,
}

impl HostLogger {
    fn record(&self, key: &str, text: &str) {
        // A panic while the lock is held must not silence the log.
        let mut sink = self.sink.lock().unwrap_or_else(|error| error.into_inner());
        sink.record(Instant::now(), key, text);
    }
}

impl log::Log for HostLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let key = format!("{} {}: {}", record.level(), record.target(), record.args());
        self.record(&key, &key);
    }

    fn flush(&self) {}
}

struct Repeat {
    since: Instant,
    count: u32,
    suppressed: u64,
}

/// Where log lines go: `host.log`, rotated once it would pass `limit`, or
/// stderr when the file cannot be opened.
struct Sink {
    path: Option<PathBuf>,
    file: Option<File>,
    written: u64,
    limit: u64,
    repeats: HashMap<String, Repeat>,
    last_sweep: Option<Instant>,
}

impl Sink {
    fn open(path: Option<PathBuf>, limit: u64) -> Self {
        let file = path.as_deref().and_then(open_log);
        let written = file
            .as_ref()
            .and_then(|file| file.metadata().ok())
            .map_or(0, |metadata| metadata.len());
        Self {
            path,
            file,
            written,
            limit,
            repeats: HashMap::new(),
            last_sweep: None,
        }
    }

    /// Writes `text` unless the message `key` was already written
    /// [`REPEAT_BURST`] times in its current [`REPEAT_INTERVAL`].
    fn record(&mut self, now: Instant, key: &str, text: &str) {
        self.sweep(now);
        let repeat = self.repeats.entry(key.to_owned()).or_insert(Repeat {
            since: now,
            count: 0,
            suppressed: 0,
        });
        let admitted = repeat.count < REPEAT_BURST;
        if admitted {
            repeat.count += 1;
        } else {
            repeat.suppressed += 1;
        }
        if admitted {
            self.write(&format!("{} {text}\n", timestamp()));
        }
    }

    /// Ends the intervals that are over, reporting what each suppressed.
    /// Runs once per interval, or sooner when too many messages are tracked.
    fn sweep(&mut self, now: Instant) {
        let full = self.repeats.len() >= MAX_TRACKED;
        if !full
            && self
                .last_sweep
                .is_some_and(|last| now.duration_since(last) < REPEAT_INTERVAL)
        {
            return;
        }
        self.last_sweep = Some(now);
        let expired: Vec<String> = self
            .repeats
            .iter()
            .filter(|(_, repeat)| full || now.duration_since(repeat.since) >= REPEAT_INTERVAL)
            .map(|(key, _)| key.clone())
            .collect();
        for key in expired {
            let Some(repeat) = self.repeats.remove(&key) else {
                continue;
            };
            if repeat.suppressed > 0 {
                let mut key = key;
                truncate(&mut key, 1024);
                self.write(&format!(
                    "{} suppressed {} more of: {key}\n",
                    timestamp(),
                    repeat.suppressed
                ));
            }
        }
    }

    fn write(&mut self, line: &str) {
        let length = line.len() as u64;
        if self.written > 0 && self.written + length > self.limit {
            self.rotate();
        }
        self.written += length;
        match &mut self.file {
            Some(file) => {
                let _ = file.write_all(line.as_bytes());
            }
            None => {
                let _ = std::io::stderr().write_all(line.as_bytes());
            }
        }
    }

    /// Moves `host.log` to `host.log.1` and starts a new one. If it cannot be
    /// moved (Windows refuses while another process has it open without
    /// sharing), it is emptied instead, so it never grows past the limit.
    fn rotate(&mut self) {
        let (Some(path), Some(file)) = (&self.path, self.file.take()) else {
            return;
        };
        self.written = 0;
        let mut rotated = path.clone().into_os_string();
        rotated.push(".1");
        if std::fs::rename(path, &rotated).is_ok()
            && let Some(new) = open_log(path)
        {
            self.file = Some(new);
            return;
        }
        let _ = file.set_len(0);
        self.file = Some(file);
    }
}

fn open_log(path: &Path) -> Option<File> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_default()
    }

    #[test]
    fn repeated_messages_are_counted_instead_of_written() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("host.log");
        let mut sink = Sink::open(Some(path.clone()), HOST_LOG_LIMIT);
        let start = Instant::now();
        for _ in 0..100 {
            sink.record(start, "WARN accept failed", "WARN accept failed");
        }
        sink.record(start, "INFO other", "INFO other");
        let text = read(&path);
        assert_eq!(text.matches("accept failed").count(), REPEAT_BURST as usize);
        assert!(text.contains("INFO other"));

        // The next interval reports the count and writes the message again.
        sink.record(
            start + REPEAT_INTERVAL,
            "WARN accept failed",
            "WARN accept failed",
        );
        let text = read(&path);
        assert!(
            text.contains(&format!(
                "suppressed {} more of: WARN accept failed",
                100 - REPEAT_BURST
            )),
            "{text}"
        );
        assert_eq!(
            text.lines()
                .filter(|line| line.ends_with("] WARN accept failed"))
                .count(),
            REPEAT_BURST as usize + 1
        );
    }

    #[test]
    fn log_rotates_instead_of_stopping() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("host.log");
        let mut sink = Sink::open(Some(path.clone()), 1000);
        let start = Instant::now();
        for index in 0..60 {
            let message = format!("message {index:02} {}", "x".repeat(40));
            sink.record(start, &message, &message);
        }
        let current = read(&path);
        let previous = read(&temp.path().join("host.log.1"));
        assert!(current.len() <= 1000, "{}", current.len());
        assert!(previous.len() <= 1000, "{}", previous.len());
        // The newest message is in the current file; nothing stopped logging.
        assert!(current.contains("message 59"), "{current}");
        assert!(!previous.is_empty());
    }

    #[test]
    fn panic_messages_are_capped() {
        let mut text = "é".repeat(MAX_PANIC_BYTES);
        truncate(&mut text, MAX_PANIC_BYTES);
        assert!(text.len() <= MAX_PANIC_BYTES + "\n[truncated]".len());
        assert!(text.ends_with("[truncated]"));
    }
}
