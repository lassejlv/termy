// Ownership of the ConPTY child process and pseudoconsole handles.
// Included into `windows.rs`, so it shares that module's imports and items.

struct ChildResources {
    process: Option<OwnedHandle>,
    pseudo_console: Option<PseudoConsole>,
    process_exited: bool,
}

impl ChildResources {
    fn new(process: OwnedHandle, pseudo_console: PseudoConsole) -> Self {
        Self {
            process: Some(process),
            pseudo_console: Some(pseudo_console),
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

    fn terminate_and_wait(&mut self) -> io::Result<()> {
        if self.process_exited {
            return Ok(());
        }
        // SAFETY: this wrapper owns the exact process created for the terminal.
        // TerminateProcess is the Windows shutdown fallback when its HPCON
        // session owner is dropped before the client exits naturally.
        let terminated = unsafe { TerminateProcess(self.process().raw(), 1) };
        if terminated == FALSE && !self.process_signaled().unwrap_or(false) {
            return Err(io::Error::last_os_error());
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

    fn close_pseudo_console(&mut self) {
        drop(self.pseudo_console.take());
    }

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
            let _ = self.terminate_and_wait();
        }
        self.close_pseudo_console();
        drop(self.process.take());
    }
}
