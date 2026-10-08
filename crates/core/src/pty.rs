//! Native pseudo-terminal processes, without a terminal engine.
//!
//! [`Pty`] runs a program on a Unix PTY (`forkpty`) or a Windows ConPTY and
//! moves raw bytes in and out. It is the process layer the Termy app uses for
//! its own sessions, exposed for hosts that bring their own terminal, such as
//! the `@termysh/pty` Node addon. Output is delivered unparsed and nothing
//! answers terminal queries on the program's behalf.

use std::{io, path::PathBuf};

use crate::terminal_engine::transport::{ChildExit, PtySize, SpawnConfig, Transport};

/// A program to run on a PTY.
#[derive(Clone, Debug, Default)]
pub struct PtyCommand {
    /// Program to run. Names without a path separator are looked up on
    /// `PATH`, from `environment` when it sets one, otherwise the parent's.
    pub program: String,
    pub args: Vec<String>,
    pub working_directory: Option<PathBuf>,
    /// Variables for the child. With `inherit_environment` they override the
    /// parent's; without it they are the child's whole environment.
    pub environment: Vec<(String, String)>,
    pub inherit_environment: bool,
}

/// PTY dimensions. Pixel sizes are optional; zero reports them as unknown.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PtyDimensions {
    pub cols: u16,
    pub rows: u16,
    pub pixel_width: u32,
    pub pixel_height: u32,
}

impl PtyDimensions {
    fn transport_size(self) -> io::Result<PtySize> {
        if self.cols == 0 || self.rows == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "PTY columns and rows must be at least 1",
            ));
        }
        let cell = |pixels: u32, cells: u16| {
            if pixels == 0 {
                0.0
            } else {
                pixels as f32 / f32::from(cells)
            }
        };
        Ok(PtySize {
            cols: self.cols,
            rows: self.rows,
            cell_width: cell(self.pixel_width, self.cols),
            cell_height: cell(self.pixel_height, self.rows),
        })
    }
}

/// How the program ended. On Unix exactly one field is set; on Windows only
/// `code` is, as an unsigned `DWORD`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PtyExit {
    pub code: Option<i64>,
    pub signal: Option<i32>,
}

impl From<ChildExit> for PtyExit {
    fn from(exit: ChildExit) -> Self {
        #[cfg(windows)]
        let code = exit.code.map(|code| i64::from(code as u32));
        #[cfg(not(windows))]
        let code = exit.code.map(i64::from);
        Self {
            code,
            signal: exit.signal,
        }
    }
}

/// A running program on a pseudo-terminal.
///
/// Dropping a `Pty` hangs up the session: Unix children receive `SIGHUP`, then
/// `SIGKILL` if they are still running 250 ms later; Windows children are
/// terminated.
pub struct Pty {
    transport: Transport,
}

impl Pty {
    /// Start `command`. `on_output` runs on a reader thread with each chunk of
    /// output; blocking in it stops reading, which applies backpressure to the
    /// program. `on_exit` runs once, on the same thread, after the final output.
    pub fn spawn(
        command: PtyCommand,
        dimensions: PtyDimensions,
        mut on_output: impl FnMut(&[u8]) + Send + 'static,
        on_exit: impl FnOnce(PtyExit) + Send + 'static,
    ) -> io::Result<Self> {
        let transport = Transport::spawn_with_exit(
            SpawnConfig {
                program: command.program,
                args: command.args,
                working_directory: command.working_directory,
                environment: command.environment,
            },
            dimensions.transport_size()?,
            command.inherit_environment,
            move |bytes| {
                on_output(bytes);
                Vec::new()
            },
            move |exit| on_exit(exit.into()),
        )?;
        Ok(Self { transport })
    }

    /// Queue input for the program. Fails when the program has exited or more
    /// than 8 MiB of input is still waiting to be written.
    pub fn write(&self, input: &[u8]) -> io::Result<()> {
        self.transport.write(input)
    }

    /// Resize the PTY. Rapid resizes coalesce to the latest size.
    pub fn resize(&self, dimensions: PtyDimensions) -> io::Result<()> {
        self.transport.resize(dimensions.transport_size()?)
    }

    pub fn pid(&self) -> u32 {
        self.transport.child_pid()
    }

    /// Send `signal` to the program (Unix). Does nothing once it has exited.
    #[cfg(not(windows))]
    pub fn signal(&self, signal: i32) -> io::Result<()> {
        self.transport.signal(signal)
    }

    /// End the program: `SIGHUP` on Unix, process termination on Windows.
    pub fn kill(&self) -> io::Result<()> {
        #[cfg(not(windows))]
        {
            self.transport.signal(1)
        }
        #[cfg(windows)]
        {
            self.transport.terminate();
            Ok(())
        }
    }

    /// Name of the program in the foreground of the PTY, such as `vim` while
    /// it runs in a shell. `None` when unknown, and always on Windows.
    pub fn foreground_process_name(&self) -> Option<String> {
        #[cfg(not(windows))]
        {
            self.transport.foreground_process_name()
        }
        #[cfg(windows)]
        {
            None
        }
    }
}

/// Whether this platform can create PTYs (Windows needs ConPTY, 1809+).
pub fn available() -> bool {
    crate::terminal_engine::transport::available()
}
