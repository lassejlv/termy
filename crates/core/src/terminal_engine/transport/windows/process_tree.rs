// Foreground-program lookup for a ConPTY session.
// Included into `windows.rs`, so it shares that module's imports and items.

const TH32CS_SNAPPROCESS: Dword = 0x0000_0002;
const PROCESS_QUERY_LIMITED_INFORMATION: Dword = 0x0000_1000;
const MAX_PATH: usize = 260;
/// Bounds the descendant walk; parent ids recycled into a cycle stop here.
const MAX_PROCESS_TREE_DEPTH: usize = 64;

#[repr(C)]
struct ProcessEntry32W {
    size: Dword,
    usage: Dword,
    process_id: Dword,
    default_heap_id: usize,
    module_id: Dword,
    threads: Dword,
    parent_process_id: Dword,
    priority_class_base: i32,
    flags: Dword,
    exe_file: [u16; MAX_PATH],
}

impl ProcessEntry32W {
    fn new() -> Self {
        Self {
            size: mem::size_of::<Self>() as Dword,
            usage: 0,
            process_id: 0,
            default_heap_id: 0,
            module_id: 0,
            threads: 0,
            parent_process_id: 0,
            priority_class_base: 0,
            flags: 0,
            exe_file: [0; MAX_PATH],
        }
    }

    fn exe_name(&self) -> String {
        let length = self
            .exe_file
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(MAX_PATH);
        String::from_utf16_lossy(&self.exe_file[..length])
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FileTime {
    low: Dword,
    high: Dword,
}

struct ProcessRecord {
    pid: Dword,
    parent: Dword,
    name: String,
}

fn process_snapshot() -> Option<Vec<ProcessRecord>> {
    // SAFETY: a process snapshot takes no pointers; failure returns
    // INVALID_HANDLE_VALUE, which OwnedHandle rejects.
    let snapshot = OwnedHandle::new(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) })?;
    let mut entry = ProcessEntry32W::new();
    let mut processes = Vec::new();
    // SAFETY: `entry` is writable and its `size` field is initialized as the
    // API requires; the snapshot handle stays open for the whole walk.
    let mut more = unsafe { Process32FirstW(snapshot.raw(), &mut entry) } != FALSE;
    while more {
        processes.push(ProcessRecord {
            pid: entry.process_id,
            parent: entry.parent_process_id,
            name: entry.exe_name(),
        });
        // SAFETY: as above; the API keeps `size` intact between calls.
        more = unsafe { Process32NextW(snapshot.raw(), &mut entry) } != FALSE;
    }
    Some(processes)
}

/// Creation time as a FILETIME tick count, when the process can be opened.
fn process_creation_time(pid: Dword) -> Option<u64> {
    // SAFETY: limited query access opens no more than the creation time needs.
    let process = OwnedHandle::new(unsafe {
        OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid)
    })?;
    let (mut creation, mut exit, mut kernel, mut user) = (
        FileTime::default(),
        FileTime::default(),
        FileTime::default(),
        FileTime::default(),
    );
    // SAFETY: all four outputs are writable FILETIMEs; the handle is live.
    let read = unsafe {
        GetProcessTimes(
            process.raw(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    };
    (read != FALSE).then(|| (u64::from(creation.high) << 32) | u64::from(creation.low))
}

fn is_console_host(name: &str) -> bool {
    name.eq_ignore_ascii_case("conhost.exe") || name.eq_ignore_ascii_case("OpenConsole.exe")
}

/// Follow the newest child of `root_pid` down to the deepest descendant and
/// return its executable name. Console hosts are skipped, and a child that
/// predates its parent is a recycled parent id, not a descendant.
fn foreground_descendant_name(root_pid: Dword) -> Option<String> {
    let processes = process_snapshot()?;
    let mut current = processes.iter().find(|process| process.pid == root_pid)?;
    let mut current_created = process_creation_time(root_pid);
    for _ in 0..MAX_PROCESS_TREE_DEPTH {
        let newest_child = processes
            .iter()
            .filter(|process| {
                process.parent == current.pid
                    && process.pid != current.pid
                    && process.pid != root_pid
                    && !is_console_host(&process.name)
            })
            .filter_map(|process| {
                let created = process_creation_time(process.pid);
                match (created, current_created) {
                    (Some(child), Some(parent)) if child < parent => None,
                    _ => Some((process, created)),
                }
            })
            .max_by_key(|(_, created)| created.unwrap_or(0));
        let Some((child, created)) = newest_child else {
            break;
        };
        current = child;
        current_created = created;
    }
    (!current.name.is_empty()).then(|| current.name.clone())
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateToolhelp32Snapshot(flags: Dword, process_id: Dword) -> Handle;
    fn Process32FirstW(snapshot: Handle, entry: *mut ProcessEntry32W) -> Bool;
    fn Process32NextW(snapshot: Handle, entry: *mut ProcessEntry32W) -> Bool;
    fn OpenProcess(desired_access: Dword, inherit_handle: Bool, process_id: Dword) -> Handle;
    fn GetProcessTimes(
        process: Handle,
        creation_time: *mut FileTime,
        exit_time: *mut FileTime,
        kernel_time: *mut FileTime,
        user_time: *mut FileTime,
    ) -> Bool;
}
