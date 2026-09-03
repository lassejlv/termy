//! Audited unsafe boundary for macOS PTY and process syscalls.

use std::{
    env,
    ffi::{CString, OsStr, OsString},
    fs::{self, File},
    io::{self, Read},
    os::{
        fd::{AsRawFd, FromRawFd, RawFd},
        unix::{ffi::OsStrExt, fs::PermissionsExt, net::UnixStream},
    },
    path::{Path, PathBuf},
    ptr,
};

use crate::{Command, ExitStatus, PtySize};

pub(super) struct Spawned {
    pub master: File,
    pub pid: libc::pid_t,
}

struct PreparedCommand {
    executable: CString,
    arguments: Vec<CString>,
    argument_pointers: Vec<*const libc::c_char>,
    environment: Vec<CString>,
    environment_pointers: Vec<*const libc::c_char>,
    working_directory: Option<CString>,
}

impl PreparedCommand {
    fn new(command: &Command) -> io::Result<Self> {
        let environment = merged_environment(command.environment());
        let executable = resolve_program(
            command.program(),
            command.working_directory(),
            environment_value(&environment, OsStr::new("PATH")),
        )?;
        let executable = os_string_to_cstring(executable.as_os_str(), "program")?;

        let mut arguments = Vec::with_capacity(command.arguments().len() + 1);
        arguments.push(os_string_to_cstring(command.program(), "program")?);
        for argument in command.arguments() {
            arguments.push(os_string_to_cstring(argument, "argument")?);
        }
        let mut argument_pointers = arguments
            .iter()
            .map(|argument| argument.as_ptr())
            .collect::<Vec<_>>();
        argument_pointers.push(ptr::null());

        let environment = environment
            .iter()
            .map(|(name, value)| environment_entry(name, value))
            .collect::<io::Result<Vec<_>>>()?;
        let mut environment_pointers = environment
            .iter()
            .map(|entry| entry.as_ptr())
            .collect::<Vec<_>>();
        environment_pointers.push(ptr::null());

        let working_directory = command
            .working_directory()
            .map(|directory| os_string_to_cstring(directory.as_os_str(), "working directory"))
            .transpose()?;

        Ok(Self {
            executable,
            arguments,
            argument_pointers,
            environment,
            environment_pointers,
            working_directory,
        })
    }

    fn keep_alive(&self) {
        debug_assert_eq!(self.argument_pointers.len(), self.arguments.len() + 1);
        debug_assert_eq!(self.environment_pointers.len(), self.environment.len() + 1);
    }
}

pub(super) fn spawn(command: &Command, size: PtySize) -> io::Result<Spawned> {
    let prepared = PreparedCommand::new(command)?;
    let (mut error_reader, error_writer) = UnixStream::pair()?;
    let error_writer_fd = error_writer.as_raw_fd();
    let descriptors_to_close = inherited_descriptors(error_writer_fd);
    let mut window_size = libc::winsize {
        ws_row: size.rows,
        ws_col: size.cols,
        ws_xpixel: size.pixel_width,
        ws_ypixel: size.pixel_height,
    };
    let mut master_fd = -1;

    // SAFETY: Every pointer is valid for the call. `window_size` is initialized, and null termios
    // requests the platform's standard PTY settings. The child branch immediately performs only
    // async-signal-safe syscalls using memory fully prepared before the fork.
    let pid = unsafe {
        libc::forkpty(
            &raw mut master_fd,
            ptr::null_mut(),
            ptr::null_mut(),
            &raw mut window_size,
        )
    };
    if pid == -1 {
        return Err(io::Error::last_os_error());
    }
    if pid == 0 {
        child_exec(
            &prepared,
            error_writer_fd,
            error_reader.as_raw_fd(),
            &descriptors_to_close,
        );
    }

    prepared.keep_alive();
    drop(error_writer);

    // SAFETY: `forkpty` returned a unique, open master descriptor to the parent.
    let master = unsafe { File::from_raw_fd(master_fd) };
    if let Err(error) = set_close_on_exec(master.as_raw_fd()) {
        let _ = terminate_process_group(pid);
        let _ = wait(pid);
        return Err(error);
    }

    match read_exec_error(&mut error_reader) {
        Ok(None) => Ok(Spawned { master, pid }),
        Ok(Some(error)) => {
            let _ = wait(pid);
            Err(error)
        }
        Err(error) => {
            let _ = terminate_process_group(pid);
            let _ = wait(pid);
            Err(error)
        }
    }
}

fn child_exec(
    command: &PreparedCommand,
    error_fd: RawFd,
    error_reader_fd: RawFd,
    descriptors_to_close: &[RawFd],
) -> ! {
    // SAFETY: This runs only in the post-fork child. All calls are async-signal-safe, pointers refer
    // to immutable storage created before `forkpty`, and failures terminate with `_exit` rather
    // than unwinding through copied parent state.
    unsafe {
        libc::close(error_reader_fd);
        for descriptor in descriptors_to_close {
            if *descriptor != error_fd && *descriptor > libc::STDERR_FILENO {
                libc::close(*descriptor);
            }
        }

        reset_child_signals();

        if let Some(directory) = &command.working_directory
            && libc::chdir(directory.as_ptr()) == -1
        {
            report_child_error_and_exit(error_fd);
        }

        libc::execve(
            command.executable.as_ptr(),
            command.argument_pointers.as_ptr(),
            command.environment_pointers.as_ptr(),
        );
        report_child_error_and_exit(error_fd);
    }
}

unsafe fn reset_child_signals() {
    for signal in [
        libc::SIGHUP,
        libc::SIGINT,
        libc::SIGQUIT,
        libc::SIGPIPE,
        libc::SIGALRM,
        libc::SIGTERM,
        libc::SIGCHLD,
        libc::SIGTSTP,
        libc::SIGTTIN,
        libc::SIGTTOU,
        libc::SIGWINCH,
    ] {
        // SAFETY: Restoring a valid signal number to `SIG_DFL` is async-signal-safe.
        unsafe {
            libc::signal(signal, libc::SIG_DFL);
        }
    }

    // SAFETY: The signal set is initialized before it is passed to `sigprocmask`.
    unsafe {
        let mut empty_set = std::mem::zeroed();
        libc::sigemptyset(&raw mut empty_set);
        libc::sigprocmask(libc::SIG_SETMASK, &raw const empty_set, ptr::null_mut());
    }
}

unsafe fn report_child_error_and_exit(error_fd: RawFd) -> ! {
    // SAFETY: macOS exposes the calling thread's errno through `__error`. The fixed-size stack
    // buffer remains valid for `write`, and `_exit` does not run copied parent destructors.
    unsafe {
        let error = *libc::__error();
        let bytes = error.to_ne_bytes();
        let _ = libc::write(error_fd, bytes.as_ptr().cast(), bytes.len());
        libc::_exit(127);
    }
}

fn read_exec_error(reader: &mut UnixStream) -> io::Result<Option<io::Error>> {
    let mut bytes = [0_u8; size_of::<libc::c_int>()];
    let mut read = 0;
    loop {
        match reader.read(&mut bytes[read..]) {
            Ok(0) if read == 0 => return Ok(None),
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "PTY child returned a truncated exec error",
                ));
            }
            Ok(length) => {
                read += length;
                if read == bytes.len() {
                    return Ok(Some(io::Error::from_raw_os_error(
                        libc::c_int::from_ne_bytes(bytes),
                    )));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

pub(super) fn resize(master: &File, size: PtySize) -> io::Result<()> {
    let window_size = libc::winsize {
        ws_row: size.rows,
        ws_col: size.cols,
        ws_xpixel: size.pixel_width,
        ws_ypixel: size.pixel_height,
    };
    // SAFETY: `master` owns a valid descriptor and `window_size` points to initialized storage for
    // the duration of the ioctl.
    let result =
        unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ, &raw const window_size) };
    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub(super) fn terminate_process_group(leader: libc::pid_t) -> io::Result<()> {
    if leader <= 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to signal an invalid PTY process group",
        ));
    }
    // SAFETY: A negative positive PID addresses only the child process group created by `forkpty`.
    let result = unsafe { libc::kill(-leader, libc::SIGKILL) };
    if result == -1 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(error)
        }
    } else {
        Ok(())
    }
}

pub(super) fn wait(pid: libc::pid_t) -> io::Result<ExitStatus> {
    loop {
        let mut status = 0;
        // SAFETY: `status` is valid writable storage and `pid` is the exact child returned by
        // `forkpty`.
        let result = unsafe { libc::waitpid(pid, &raw mut status, 0) };
        if result == -1 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if libc::WIFEXITED(status) {
            return Ok(ExitStatus {
                code: u32::try_from(libc::WEXITSTATUS(status)).unwrap_or(1),
                signal: None,
            });
        }
        if libc::WIFSIGNALED(status) {
            let signal = libc::WTERMSIG(status);
            return Ok(ExitStatus {
                code: 1,
                signal: Some(signal_description(signal)),
            });
        }
    }
}

fn signal_description(signal: libc::c_int) -> String {
    match signal {
        libc::SIGHUP => "Hangup: 1".to_owned(),
        libc::SIGINT => "Interrupt: 2".to_owned(),
        libc::SIGQUIT => "Quit: 3".to_owned(),
        libc::SIGKILL => "Killed: 9".to_owned(),
        libc::SIGPIPE => "Broken pipe: 13".to_owned(),
        libc::SIGALRM => "Alarm clock: 14".to_owned(),
        libc::SIGTERM => "Terminated: 15".to_owned(),
        _ => format!("Signal {signal}"),
    }
}

fn set_close_on_exec(descriptor: RawFd) -> io::Result<()> {
    // SAFETY: `descriptor` is the live master descriptor returned by `forkpty`.
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFD) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: The descriptor remains live and the bitwise-updated flags are valid for `F_SETFD`.
    if unsafe { libc::fcntl(descriptor, libc::F_SETFD, flags | libc::FD_CLOEXEC) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn inherited_descriptors(error_writer: RawFd) -> Vec<RawFd> {
    let mut descriptors = fs::read_dir("/dev/fd")
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse::<RawFd>().ok())
        .filter(|descriptor| *descriptor > libc::STDERR_FILENO)
        .collect::<Vec<_>>();
    if !descriptors.contains(&error_writer) {
        descriptors.push(error_writer);
    }
    descriptors
}

fn merged_environment(overrides: &[(OsString, OsString)]) -> Vec<(OsString, OsString)> {
    let mut environment = env::vars_os().collect::<Vec<_>>();
    for (name, value) in overrides {
        if let Some((_, existing)) = environment
            .iter_mut()
            .find(|(existing, _)| existing == name)
        {
            existing.clone_from(value);
        } else {
            environment.push((name.clone(), value.clone()));
        }
    }
    environment
}

fn environment_value<'a>(
    environment: &'a [(OsString, OsString)],
    name: &OsStr,
) -> Option<&'a OsStr> {
    environment
        .iter()
        .find_map(|(candidate, value)| (candidate == name).then_some(value.as_os_str()))
}

fn resolve_program(
    program: &OsStr,
    working_directory: Option<&Path>,
    path: Option<&OsStr>,
) -> io::Result<PathBuf> {
    if program.as_bytes().contains(&b'/') {
        return Ok(PathBuf::from(program));
    }

    let path = path.unwrap_or_else(|| OsStr::new("/usr/bin:/bin"));
    for directory in env::split_paths(path) {
        let base = if directory.as_os_str().is_empty() {
            working_directory.map_or_else(|| PathBuf::from("."), Path::to_path_buf)
        } else if directory.is_relative() {
            working_directory
                .map(|working_directory| working_directory.join(&directory))
                .unwrap_or(directory)
        } else {
            directory
        };
        let candidate = base.join(program);
        if is_executable(&candidate) {
            return candidate.canonicalize();
        }
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("command {} was not found in PATH", program.display()),
    ))
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

fn environment_entry(name: &OsStr, value: &OsStr) -> io::Result<CString> {
    if name.as_bytes().contains(&b'=') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "environment variable name contains '='",
        ));
    }
    let mut entry = Vec::with_capacity(name.as_bytes().len() + value.as_bytes().len() + 1);
    entry.extend_from_slice(name.as_bytes());
    entry.push(b'=');
    entry.extend_from_slice(value.as_bytes());
    CString::new(entry).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "environment variable contains a NUL byte",
        )
    })
}

fn os_string_to_cstring(value: &OsStr, label: &str) -> io::Result<CString> {
    CString::new(value.as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{label} contains a NUL byte"),
        )
    })
}
