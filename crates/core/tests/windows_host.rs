#![cfg(windows)]
//! Audit B8: a session host started from inside a Termy tab, whose shell runs
//! in a job that ends the tab's console programs when it closes, must leave
//! that job, or closing the tab ends every persistent session.

use std::{
    io::Write,
    os::windows::io::AsRawHandle,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};
use termy_core::multiplexer::{SessionClient, connect_or_start, shutdown_host};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    System::{
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob,
            JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
            TerminateJobObject,
        },
        Threading::GetCurrentProcess,
    },
};

// Started by `host_outlives_the_job_of_the_tab_that_started_it` inside its job.
#[test]
fn client_in_job() {
    let Ok(root) = std::env::var("TERMY_MUX_TEST_ROOT") else {
        return;
    };
    // Wait until the parent has put this process in the job.
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).unwrap();
    let executable = std::env::var_os("TERMY_MUX_TEST_HOST_BINARY").map_or_else(
        || Path::new(env!("CARGO_BIN_EXE_termy-session-host")).to_owned(),
        PathBuf::from,
    );
    connect_or_start(Path::new(&root), &executable).unwrap();
}

/// The extended limits of `job`, or of this process's job when it is null.
fn job_limits(job: HANDLE) -> Option<JOBOBJECT_EXTENDED_LIMIT_INFORMATION> {
    // SAFETY: an all-zero limit structure is valid.
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: `limits` has the layout of this information class, and its
    // exact size is passed.
    let queried = unsafe {
        QueryInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&raw mut limits).cast(),
            std::mem::size_of_val(&limits) as u32,
            std::ptr::null_mut(),
        )
    };
    (queried != 0).then_some(limits)
}

/// Whether a job this test process runs in (some CI runners use one)
/// forbids breakaway, which also keeps the host in the nested test job.
fn own_job_forbids_breakaway() -> bool {
    let mut in_job = 0;
    // SAFETY: the pseudo handle needs no closing; `in_job` is writable.
    if unsafe { IsProcessInJob(GetCurrentProcess(), std::ptr::null_mut(), &mut in_job) } == 0
        || in_job == 0
    {
        return false;
    }
    job_limits(std::ptr::null_mut()).is_none_or(|limits| {
        limits.BasicLimitInformation.LimitFlags & JOB_OBJECT_LIMIT_BREAKAWAY_OK == 0
    })
}

/// Stops the host, the client and the job however the test ends.
struct Cleanup {
    root: PathBuf,
    job: HANDLE,
    client: Option<Child>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(client) = &mut self.client {
            let _ = client.kill();
            let _ = client.wait();
        }
        let _ = shutdown_host(&self.root);
        // SAFETY: the job handle was created by this test and is closed once.
        unsafe { CloseHandle(self.job) };
    }
}

#[test]
fn host_outlives_the_job_of_the_tab_that_started_it() {
    if own_job_forbids_breakaway() {
        eprintln!("skipped: this test runs in a job that forbids breakaway");
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    // Like a Termy tab's job: it allows breakaway.
    // SAFETY: no security attributes and no name create a private job.
    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    assert!(!job.is_null());
    let mut cleanup = Cleanup {
        root: temp.path().join("host"),
        job,
        client: None,
    };
    let mut limits = job_limits(job).unwrap();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_BREAKAWAY_OK;
    // SAFETY: `limits` has the layout of this information class, and its
    // exact size is passed.
    let set = unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            std::mem::size_of_val(&limits) as u32,
        )
    };
    assert_ne!(set, 0);
    let client = cleanup.client.insert(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "client_in_job", "--nocapture"])
            .env("TERMY_MUX_TEST_ROOT", &cleanup.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    // SAFETY: both handles are live.
    assert_ne!(
        unsafe { AssignProcessToJobObject(job, client.as_raw_handle()) },
        0
    );
    client.stdin.take().unwrap().write_all(b"start\n").unwrap();
    assert!(client.wait().unwrap().success());
    // Closing the tab ends what is left in its job.
    // SAFETY: the job handle is live.
    assert_ne!(unsafe { TerminateJobObject(job, 1) }, 0);
    std::thread::sleep(Duration::from_millis(500));
    let host =
        SessionClient::connect(&cleanup.root).expect("the host must outlive its launcher's job");
    host.list().unwrap();
}
