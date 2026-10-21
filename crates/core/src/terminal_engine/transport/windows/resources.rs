// Ownership of the ConPTY child process and pseudoconsole handles.
// Included into `windows.rs`, so it shares that module's imports and items.

/// How long a closing shell may take to exit before it is terminated.
const CLOSE_GRACE_MS: Dword = 250;

/// Exit code of a child Termy terminated: `STATUS_CONTROL_C_EXIT`, what
/// Windows reports for a console program ended by closing its console, so a
/// forced close is distinguishable from a program's own exit code 1.
const FORCED_EXIT_CODE: u32 = 0xC000_013A;

const JOB_OBJECT_LIMIT_BREAKAWAY_OK: Dword = 0x0000_0800;
const JOB_OBJECT_BASIC_LIMIT_INFORMATION_CLASS: i32 = 2;
const JOB_OBJECT_BASIC_PROCESS_ID_LIST_CLASS: i32 = 3;
const ERROR_MORE_DATA: i32 = 234;
const PROCESS_TERMINATE: Dword = 0x0000_0001;
const IMAGE_SUBSYSTEM_WINDOWS_CUI: u16 = 3;
/// Image paths whose PE subsystem was read; cleared when it grows past this.
const MAX_CACHED_SUBSYSTEMS: usize = 256;

#[repr(C)]
#[derive(Default)]
struct JobObjectBasicLimitInformation {
    per_process_user_time_limit: i64,
    per_job_user_time_limit: i64,
    limit_flags: Dword,
    minimum_working_set_size: usize,
    maximum_working_set_size: usize,
    active_process_limit: Dword,
    affinity: usize,
    priority_class: Dword,
    scheduling_class: Dword,
}

/// A job holding the terminal's child and, by inheritance, everything it
/// starts. It only records membership: nothing dies when it closes, so GUI
/// programs launched from the shell (`code .`, `notepad`, `start ...`)
/// outlive their tab and Termy itself, as in other Windows terminals.
/// Closing a tab ends the job's console programs, which belong to the tab.
/// Programs that ask for `CREATE_BREAKAWAY_FROM_JOB` may leave it.
struct JobObject(OwnedHandle);

impl JobObject {
    fn new() -> io::Result<Self> {
        // SAFETY: no security attributes and no name create a private job.
        let handle = unsafe { CreateJobObjectW(ptr::null_mut(), ptr::null()) };
        let job = OwnedHandle::new(handle)
            .map(Self)
            .ok_or_else(io::Error::last_os_error)?;
        let limits = JobObjectBasicLimitInformation {
            limit_flags: JOB_OBJECT_LIMIT_BREAKAWAY_OK,
            ..Default::default()
        };
        // SAFETY: `limits` is a fully initialized JOBOBJECT_BASIC_LIMIT_
        // INFORMATION whose exact size is passed alongside it.
        let set = unsafe {
            SetInformationJobObject(
                job.0.raw(),
                JOB_OBJECT_BASIC_LIMIT_INFORMATION_CLASS,
                (&raw const limits).cast::<c_void>(),
                mem::size_of::<JobObjectBasicLimitInformation>() as Dword,
            )
        };
        if set == FALSE {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    fn assign(&self, process: &OwnedHandle) -> io::Result<()> {
        // SAFETY: both handles are owned and live; assignment does not
        // transfer ownership of either.
        if unsafe { AssignProcessToJobObject(self.0.raw(), process.raw()) } == FALSE {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Ids of the processes currently in the job.
    fn process_ids(&self) -> io::Result<Vec<Dword>> {
        // JOBOBJECT_BASIC_PROCESS_ID_LIST: two DWORD counts, then ULONG_PTR
        // ids at offset 8 on both 32- and 64-bit Windows.
        let header_words = 8 / mem::size_of::<usize>();
        let mut capacity = 64usize;
        loop {
            let mut buffer = vec![0usize; header_words + capacity];
            let length = Dword::try_from(mem::size_of_val(buffer.as_slice()))
                .map_err(|_| io::Error::other("job process list is too large"))?;
            // SAFETY: `buffer` is writable, pointer-aligned and `length`
            // bytes long; the job handle is live.
            let queried = unsafe {
                QueryInformationJobObject(
                    self.0.raw(),
                    JOB_OBJECT_BASIC_PROCESS_ID_LIST_CLASS,
                    buffer.as_mut_ptr().cast::<c_void>(),
                    length,
                    ptr::null_mut(),
                )
            };
            // SAFETY: the buffer starts with the two initialized DWORD counts.
            let (assigned, listed) = unsafe {
                let counts = buffer.as_ptr().cast::<Dword>();
                (*counts as usize, *counts.add(1) as usize)
            };
            if queried == FALSE {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(ERROR_MORE_DATA) || assigned <= capacity {
                    return Err(error);
                }
                // Processes may join between calls; leave headroom.
                capacity = assigned + 16;
                continue;
            }
            return Ok(buffer[header_words..header_words + listed.min(capacity)]
                .iter()
                .map(|pid| *pid as Dword)
                .collect());
        }
    }

    /// Terminate the job's console programs except `skip_pid` (the shell,
    /// which its owner terminates by handle). GUI programs, and processes
    /// that cannot be opened or inspected, are left running.
    fn terminate_console_processes(&self, skip_pid: Dword) {
        let Ok(pids) = self.process_ids() else {
            return;
        };
        for pid in pids.into_iter().filter(|pid| *pid != skip_pid) {
            // SAFETY: opening by id takes no pointers; failure returns null.
            let Some(process) = OwnedHandle::new(unsafe {
                OpenProcess(
                    PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
                    FALSE,
                    pid,
                )
            }) else {
                continue;
            };
            // The id may have been recycled after the listed process exited.
            let mut in_job = FALSE;
            // SAFETY: both handles are live and `in_job` is writable.
            if unsafe { IsProcessInJob(process.raw(), self.0.raw(), &mut in_job) } == FALSE
                || in_job == FALSE
            {
                continue;
            }
            if process_image_path(&process)
                .and_then(|path| cached_image_subsystem(&path))
                .is_some_and(|subsystem| subsystem == IMAGE_SUBSYSTEM_WINDOWS_CUI)
            {
                // SAFETY: the handle was opened with PROCESS_TERMINATE.
                unsafe {
                    let _ = TerminateProcess(process.raw(), FORCED_EXIT_CODE);
                }
            }
        }
    }
}

fn process_image_path(process: &OwnedHandle) -> Option<PathBuf> {
    let mut buffer = vec![0u16; MAX_COMMAND_LINE_UNITS];
    let mut length = buffer.len() as Dword;
    // SAFETY: `buffer` holds `length` writable UTF-16 units and the handle
    // has PROCESS_QUERY_LIMITED_INFORMATION access.
    let read =
        unsafe { QueryFullProcessImageNameW(process.raw(), 0, buffer.as_mut_ptr(), &mut length) };
    if read == FALSE {
        return None;
    }
    let length = (length as usize).min(buffer.len());
    Some(PathBuf::from(OsString::from_wide(&buffer[..length])))
}

fn cached_image_subsystem(path: &Path) -> Option<u16> {
    static CACHE: OnceLock<Mutex<std::collections::HashMap<String, Option<u16>>>> =
        OnceLock::new();
    let key = path.to_string_lossy().to_lowercase();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(subsystem) = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&key)
    {
        return *subsystem;
    }
    let subsystem = image_subsystem(path);
    let mut cache = cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if cache.len() >= MAX_CACHED_SUBSYSTEMS {
        cache.clear();
    }
    cache.insert(key, subsystem);
    subsystem
}

/// The `Subsystem` field of a PE image's optional header (3 for console
/// programs, 2 for GUI programs), or `None` when the file is not a PE image.
fn image_subsystem(path: &Path) -> Option<u16> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(path).ok()?;
    let mut dos_header = [0u8; 64];
    file.read_exact(&mut dos_header).ok()?;
    if &dos_header[..2] != b"MZ" {
        return None;
    }
    let pe_offset = u32::from_le_bytes(dos_header[0x3C..0x40].try_into().ok()?);
    file.seek(SeekFrom::Start(u64::from(pe_offset))).ok()?;
    // Signature (4) and COFF file header (20), then the optional header,
    // whose `Subsystem` sits at offset 68 in both PE32 and PE32+.
    let mut headers = [0u8; 24 + 70];
    file.read_exact(&mut headers).ok()?;
    if &headers[..4] != b"PE\0\0" {
        return None;
    }
    let optional = &headers[24..];
    let magic = u16::from_le_bytes([optional[0], optional[1]]);
    if magic != 0x10B && magic != 0x20B {
        return None;
    }
    Some(u16::from_le_bytes([optional[68], optional[69]]))
}

struct ChildResources {
    process: Option<OwnedHandle>,
    process_id: Dword,
    pseudo_console: Option<PseudoConsole>,
    job: Option<JobObject>,
    process_exited: bool,
}

impl ChildResources {
    fn new(
        process: OwnedHandle,
        process_id: Dword,
        pseudo_console: PseudoConsole,
        job: Option<JobObject>,
    ) -> Self {
        Self {
            process: Some(process),
            process_id,
            pseudo_console: Some(pseudo_console),
            job,
            process_exited: false,
        }
    }

    fn process(&self) -> &OwnedHandle {
        self.process
            .as_ref()
            .expect("child process handle remains owned until cleanup")
    }

    fn pseudo_console(&self) -> &PseudoConsole {
        self.pseudo_console
            .as_ref()
            .expect("pseudoconsole remains owned until control cleanup")
    }

    fn process_signaled(&self) -> io::Result<bool> {
        // SAFETY: the control thread owns this live process handle. A zero
        // timeout only observes signal state and does not consume the handle.
        match unsafe { WaitForSingleObject(self.process().raw(), 0) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            WAIT_FAILED => Err(io::Error::last_os_error()),
            value => Err(io::Error::other(format!(
                "WaitForSingleObject returned unexpected status 0x{value:08X}"
            ))),
        }
    }

    /// End the console programs the shell started, which would otherwise
    /// keep running detached from their closed tab. Runs even when the shell
    /// itself already exited during the close grace period.
    fn terminate_console_descendants(&self) {
        if let Some(job) = &self.job {
            job.terminate_console_processes(self.process_id);
        }
    }

    fn terminate_and_wait(&mut self) -> io::Result<()> {
        if self.process_exited {
            return Ok(());
        }
        // SAFETY: this wrapper owns the exact process created for the terminal.
        // TerminateProcess is the Windows shutdown fallback when its HPCON
        // session owner is dropped before the client exits naturally.
        let terminated = unsafe { TerminateProcess(self.process().raw(), FORCED_EXIT_CODE) };
        if terminated == FALSE {
            let error = io::Error::last_os_error();
            if !self.process_signaled().unwrap_or(false) {
                return Err(error);
            }
        }
        // SAFETY: the exact child process handle remains live. After successful
        // termination, waiting without a timeout completes process teardown.
        let waited = unsafe { WaitForSingleObject(self.process().raw(), INFINITE) };
        if waited != WAIT_OBJECT_0 {
            return Err(if waited == WAIT_FAILED {
                io::Error::last_os_error()
            } else {
                io::Error::other(format!(
                    "WaitForSingleObject returned unexpected status 0x{waited:08X}"
                ))
            });
        }
        self.process_exited = true;
        Ok(())
    }

    /// Wait up to `timeout_ms` for the child to exit on its own.
    fn wait_for_exit(&mut self, timeout_ms: Dword) {
        // SAFETY: the control thread owns this live process handle; the
        // bounded wait only observes its signal state.
        if unsafe { WaitForSingleObject(self.process().raw(), timeout_ms) } == WAIT_OBJECT_0 {
            self.process_exited = true;
        }
    }

    fn close_pseudo_console(&mut self) {
        drop(self.pseudo_console.take());
    }

    /// The child's exit code; [`FORCED_EXIT_CODE`] when Termy terminated it.
    fn exit_code(&self) -> Option<Dword> {
        if !self.process_exited {
            return None;
        }
        let mut code: Dword = 0;
        // SAFETY: the exited child's handle remains owned; `code` is writable.
        let read = unsafe { GetExitCodeProcess(self.process().raw(), &mut code) };
        (read != FALSE).then_some(code)
    }
}

impl Drop for ChildResources {
    fn drop(&mut self) {
        if !self.process_exited {
            self.terminate_console_descendants();
            let _ = self.terminate_and_wait();
        }
        self.close_pseudo_console();
        drop(self.process.take());
        drop(self.job.take());
    }
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateJobObjectW(attributes: *mut c_void, name: *const u16) -> Handle;
    fn SetInformationJobObject(
        job: Handle,
        information_class: i32,
        information: *const c_void,
        information_length: Dword,
    ) -> Bool;
    fn QueryInformationJobObject(
        job: Handle,
        information_class: i32,
        information: *mut c_void,
        information_length: Dword,
        return_length: *mut Dword,
    ) -> Bool;
    fn AssignProcessToJobObject(job: Handle, process: Handle) -> Bool;
    fn IsProcessInJob(process: Handle, job: Handle, result: *mut Bool) -> Bool;
    fn QueryFullProcessImageNameW(
        process: Handle,
        flags: Dword,
        exe_name: *mut u16,
        size: *mut Dword,
    ) -> Bool;
    fn ResumeThread(thread: Handle) -> Dword;
}
