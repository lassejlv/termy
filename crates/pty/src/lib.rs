//! Direct macOS pseudo-terminal process management for Tmon.
//!
//! This crate deliberately exposes a small synchronous file-descriptor API. Higher layers choose
//! their own reader thread or event loop, while PTY creation, child setup, resize, signalling, and
//! waiting stay behind safe Rust types.

#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(not(target_os = "macos"))]
compile_error!("tmon-pty currently supports macOS only");

mod native;

use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io,
    path::{Path, PathBuf},
};

/// Character and pixel dimensions applied to a pseudo-terminal.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PtySize {
    pub rows: u16,
    pub cols: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

/// A child command and the environment overrides applied before execution.
#[derive(Clone, Debug)]
pub struct Command {
    program: OsString,
    arguments: Vec<OsString>,
    working_directory: Option<PathBuf>,
    environment: Vec<(OsString, OsString)>,
}

impl Command {
    /// Creates a command with inherited environment and no arguments.
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            arguments: Vec::new(),
            working_directory: None,
            environment: Vec::new(),
        }
    }

    /// Appends one argument.
    #[must_use]
    pub fn arg(mut self, argument: impl Into<OsString>) -> Self {
        self.arguments.push(argument.into());
        self
    }

    /// Appends multiple arguments.
    #[must_use]
    pub fn args<I, S>(mut self, arguments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.arguments.extend(
            arguments
                .into_iter()
                .map(|argument| argument.as_ref().to_owned()),
        );
        self
    }

    /// Sets the child's working directory.
    #[must_use]
    pub fn current_dir(mut self, directory: impl Into<PathBuf>) -> Self {
        self.working_directory = Some(directory.into());
        self
    }

    /// Sets or replaces one child environment variable.
    #[must_use]
    pub fn env(mut self, name: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        let name = name.into();
        let value = value.into();
        if let Some((_, existing)) = self
            .environment
            .iter_mut()
            .find(|(existing, _)| *existing == name)
        {
            *existing = value;
        } else {
            self.environment.push((name, value));
        }
        self
    }

    fn program(&self) -> &OsStr {
        &self.program
    }

    fn arguments(&self) -> &[OsString] {
        &self.arguments
    }

    fn working_directory(&self) -> Option<&Path> {
        self.working_directory.as_deref()
    }

    fn environment(&self) -> &[(OsString, OsString)] {
        &self.environment
    }
}

/// Owned PTY master descriptor.
#[derive(Debug)]
pub struct PtyMaster {
    file: File,
}

impl PtyMaster {
    /// Duplicates the master descriptor for independent reading or writing.
    ///
    /// The duplicate is close-on-exec and points at the same PTY master.
    ///
    /// # Errors
    ///
    /// Returns an OS error if the descriptor cannot be duplicated.
    pub fn try_clone(&self) -> io::Result<File> {
        self.file.try_clone()
    }

    /// Applies terminal character and pixel dimensions with `TIOCSWINSZ`.
    ///
    /// # Errors
    ///
    /// Returns an OS error if the kernel rejects the resize.
    pub fn resize(&self, size: PtySize) -> io::Result<()> {
        native::resize(&self.file, size)
    }
}

/// Copyable handle for signalling the PTY child's process group.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessGroup {
    leader: libc::pid_t,
}

impl ProcessGroup {
    /// Sends `SIGKILL` to the complete PTY process group.
    ///
    /// A process that has already exited is treated as successfully terminated.
    ///
    /// # Errors
    ///
    /// Returns an OS error if signalling fails for any other reason.
    pub fn terminate(self) -> io::Result<()> {
        native::terminate_process_group(self.leader)
    }
}

/// Reaped child status.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExitStatus {
    code: u32,
    signal: Option<String>,
}

impl ExitStatus {
    #[must_use]
    pub const fn exit_code(&self) -> u32 {
        self.code
    }

    #[must_use]
    pub fn signal(&self) -> Option<&str> {
        self.signal.as_deref()
    }
}

/// PTY child process. Dropping an unwaited child terminates its process group and reaps it.
#[derive(Debug)]
pub struct Child {
    pid: libc::pid_t,
    reaped: bool,
}

impl Child {
    #[must_use]
    pub fn process_id(&self) -> u32 {
        debug_assert!(self.pid > 0);
        self.pid.unsigned_abs()
    }

    #[must_use]
    pub const fn process_group(&self) -> ProcessGroup {
        ProcessGroup { leader: self.pid }
    }

    /// Terminates the complete PTY process group.
    ///
    /// # Errors
    ///
    /// Returns an OS error if signalling fails.
    pub fn kill(&self) -> io::Result<()> {
        self.process_group().terminate()
    }

    /// Waits for and reaps the child process.
    ///
    /// # Errors
    ///
    /// Returns an OS error if `waitpid` fails.
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        let status = native::wait(self.pid)?;
        self.reaped = true;
        Ok(status)
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = self.kill();
            let _ = native::wait(self.pid);
            self.reaped = true;
        }
    }
}

/// A newly spawned PTY master and its child process.
#[derive(Debug)]
pub struct SpawnedPty {
    pub master: PtyMaster,
    pub child: Child,
}

/// Spawns a command as a session and process-group leader attached to a new controlling PTY.
///
/// The child receives inherited environment plus [`Command::env`] overrides. Commands without a
/// slash are resolved against the resulting `PATH` before `forkpty`, so the post-fork child path
/// performs only async-signal-safe operations.
///
/// # Errors
///
/// Returns an error for invalid command data, path resolution failure, PTY/fork failure, child
/// setup failure, or `execve` failure.
pub fn spawn(command: &Command, size: PtySize) -> io::Result<SpawnedPty> {
    let native::Spawned { master, pid } = native::spawn(command, size)?;
    Ok(SpawnedPty {
        master: PtyMaster { file: master },
        child: Child { pid, reaped: false },
    })
}
