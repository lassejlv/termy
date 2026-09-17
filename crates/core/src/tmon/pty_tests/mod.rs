use super::*;
use std::os::fd::IntoRawFd;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::process::Command;
use std::{sync::atomic::AtomicUsize, time::Duration};

#[cfg(any(target_os = "linux", target_os = "macos"))]
const SIGNAL_IGNORE: usize = 1;
#[cfg(any(target_os = "linux", target_os = "macos"))]
const SIGNAL_ERROR: usize = usize::MAX;

static TEST_DIRECTORY_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

struct KillProcessOnDrop(c_int);

impl Drop for KillProcessOnDrop {
    fn drop(&mut self) {
        // SAFETY: tests construct this guard from the exact descendant PID
        // reported by the shell that they just spawned.
        unsafe {
            let _ = kill(self.0, SIGKILL);
        }
    }
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(label: &str) -> Self {
        let sequence = TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "tmon-pty-{label}-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&path).expect("test directory should be created");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_test_file(path: &Path, contents: &[u8], mode: u32) {
    std::fs::write(path, contents).expect("test file should be written");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .expect("test file permissions should be set");
}

fn assert_spawn_error(config: SpawnConfig, expected_kind: ErrorKind) -> io::Error {
    let output_count = Arc::new(AtomicUsize::new(0));
    let callback_output_count = output_count.clone();
    let exit_count = Arc::new(AtomicUsize::new(0));
    let callback_exit_count = exit_count.clone();
    let result = Pty::spawn(
        config,
        test_size(),
        move |_| {
            callback_output_count.fetch_add(1, Ordering::AcqRel);
            Vec::new()
        },
        move || {
            callback_exit_count.fetch_add(1, Ordering::AcqRel);
        },
    );
    let error = match result {
        Ok(terminal) => {
            drop(terminal);
            panic!("invalid PTY configuration unexpectedly started a session");
        }
        Err(error) => error,
    };
    assert_eq!(error.kind(), expected_kind);
    assert_eq!(output_count.load(Ordering::Acquire), 0);
    assert_eq!(exit_count.load(Ordering::Acquire), 0);
    error
}

fn test_size() -> Size {
    Size {
        cols: 80,
        rows: 24,
        cell_width: 8.0,
        cell_height: 16.0,
    }
}

#[derive(Default)]
struct PartialWriter {
    bytes: Vec<u8>,
    max_write: usize,
    write_calls: usize,
}

impl Write for PartialWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.write_calls += 1;
        let written = bytes.len().min(self.max_write);
        self.bytes.extend_from_slice(&bytes[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Default)]
struct WouldBlockOnceWriter {
    blocked: bool,
    bytes: Vec<u8>,
}

impl Write for WouldBlockOnceWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if !self.blocked {
            self.blocked = true;
            return Err(ErrorKind::WouldBlock.into());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Default)]
struct PartialThenWouldBlockWriter {
    write_calls: usize,
    bytes: Vec<u8>,
}

impl Write for PartialThenWouldBlockWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.write_calls += 1;
        if self.write_calls == 2 {
            return Err(ErrorKind::WouldBlock.into());
        }
        let written = if self.write_calls == 1 {
            bytes.len().min(2)
        } else {
            bytes.len()
        };
        self.bytes.extend_from_slice(&bytes[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

mod process;
mod writer;
