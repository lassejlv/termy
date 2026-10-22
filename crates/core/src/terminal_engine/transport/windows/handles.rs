// Win32 pipe and pseudoconsole handle ownership.
// Included into windows.rs to share its ABI declarations.

fn read_handle(handle: &OwnedHandle, buffer: &mut [u8]) -> io::Result<usize> {
    let requested = buffer.len().min(Dword::MAX as usize) as Dword;
    let mut read = 0;
    // SAFETY: `handle` owns a synchronous pipe handle and `buffer` is writable
    // for `requested` bytes. A null OVERLAPPED pointer selects synchronous IO.
    let succeeded = unsafe {
        ReadFile(
            handle.raw(),
            buffer.as_mut_ptr().cast::<c_void>(),
            requested,
            &mut read,
            ptr::null_mut(),
        )
    };
    if succeeded == FALSE {
        return Err(io::Error::last_os_error());
    }
    Ok(read as usize)
}

fn write_handle(handle: &OwnedHandle, bytes: &[u8]) -> io::Result<usize> {
    let requested = bytes.len().min(Dword::MAX as usize) as Dword;
    let mut written = 0;
    // SAFETY: `handle` owns a synchronous pipe handle and `bytes` is readable
    // for `requested` bytes. The written count is valid for the duration.
    let succeeded = unsafe {
        WriteFile(
            handle.raw(),
            bytes.as_ptr().cast::<c_void>(),
            requested,
            &mut written,
            ptr::null_mut(),
        )
    };
    if succeeded == FALSE {
        return Err(io::Error::last_os_error());
    }
    if written == 0 {
        return Err(io::Error::new(
            ErrorKind::WriteZero,
            "ConPTY input pipe accepted zero bytes",
        ));
    }
    Ok(written as usize)
}

fn is_pipe_shutdown(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(ERROR_BROKEN_PIPE | ERROR_NO_DATA | ERROR_OPERATION_ABORTED)
    )
}

struct OwnedHandle(Handle);

// SAFETY: Windows kernel handles are process-wide opaque values. Ownership is
// transferred to exactly one worker thread and CloseHandle remains valid there.
unsafe impl Send for OwnedHandle {}

impl OwnedHandle {
    fn new(handle: Handle) -> Option<Self> {
        (!handle.is_null() && handle as isize != -1).then_some(Self(handle))
    }

    fn raw(&self) -> Handle {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        close_raw_handle(self.0);
    }
}

fn close_raw_handle(handle: Handle) {
    if !handle.is_null() && handle as isize != -1 {
        // SAFETY: callers pass an owned Win32 HANDLE exactly once. Return value
        // cannot be acted on during Drop and does not change ownership.
        unsafe {
            let _ = CloseHandle(handle);
        }
    }
}

fn create_pipe() -> io::Result<(OwnedHandle, OwnedHandle)> {
    let mut read = ptr::null_mut();
    let mut write = ptr::null_mut();
    // SAFETY: both output pointers are valid. Null security attributes produce
    // non-inheritable synchronous anonymous-pipe handles as required here.
    if unsafe { CreatePipe(&mut read, &mut write, ptr::null_mut(), 0) } == FALSE {
        return Err(io::Error::last_os_error());
    }
    let Some(read) = OwnedHandle::new(read) else {
        close_raw_handle(write);
        return Err(io::Error::other(
            "CreatePipe returned an invalid read handle",
        ));
    };
    let Some(write) = OwnedHandle::new(write) else {
        return Err(io::Error::other(
            "CreatePipe returned an invalid write handle",
        ));
    };
    Ok((read, write))
}

struct PseudoConsole {
    handle: HpCon,
    api: ConPtyApi,
}

// SAFETY: HPCON is an opaque process-wide kernel object. This wrapper transfers
// its sole ClosePseudoConsole responsibility to the control thread.
unsafe impl Send for PseudoConsole {}

impl PseudoConsole {
    fn create(
        api: ConPtyApi,
        size: Coord,
        input: &OwnedHandle,
        output: &OwnedHandle,
        flags: Dword,
    ) -> io::Result<Self> {
        let mut handle = ptr::null_mut();
        // SAFETY: both pipe handles remain live through this call and `handle`
        // is valid output storage. The dynamically loaded function has the
        // documented CreatePseudoConsole system ABI.
        let result = unsafe { (api.create)(size, input.raw(), output.raw(), flags, &mut handle) };
        if result < 0 {
            return Err(hresult_error("CreatePseudoConsole", result));
        }
        if handle.is_null() {
            return Err(io::Error::other(
                "CreatePseudoConsole returned a null HPCON",
            ));
        }
        Ok(Self { handle, api })
    }

    fn raw(&self) -> HpCon {
        self.handle
    }

    fn resize(&self, size: Coord) -> io::Result<()> {
        // SAFETY: this wrapper owns a live HPCON, the control thread serializes
        // resize with close, and the function pointer has the documented ABI.
        let result = unsafe { (self.api.resize)(self.handle, size) };
        if result < 0 {
            return Err(hresult_error("ResizePseudoConsole", result));
        }
        Ok(())
    }
}

impl Drop for PseudoConsole {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            // SAFETY: this is the unique owner of a live HPCON. Close is called
            // exactly once and the handle is cleared before Drop can finish.
            unsafe { (self.api.close)(self.handle) };
            self.handle = ptr::null_mut();
        }
    }
}

