// Owned Win32 handles, anonymous pipes, the pseudoconsole and the process
// attribute list that hands it to the child.
// Included into `windows.rs`, so it shares that module's imports and items.

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

fn hresult_error(operation: &str, result: HResult) -> io::Error {
    let facility = ((result as u32) >> 16) & 0x1fff;
    if facility == 7 {
        let code = (result as u32 & 0xffff) as i32;
        return io::Error::new(
            io::Error::from_raw_os_error(code).kind(),
            format!("{operation} failed: {}", io::Error::from_raw_os_error(code)),
        );
    }
    io::Error::other(format!(
        "{operation} failed with HRESULT 0x{:08X}",
        result as u32
    ))
}

#[repr(C, align(16))]
#[derive(Clone, Copy, Default)]
struct AttributeStorageBlock([u8; 16]);

struct AttributeList {
    storage: Vec<AttributeStorageBlock>,
    initialized: bool,
}

impl AttributeList {
    fn new(attribute_count: Dword) -> io::Result<Self> {
        let mut bytes = 0usize;
        // SAFETY: the first documented probe uses a null list and valid size
        // output to obtain the allocation requirement for `attribute_count`.
        unsafe {
            let _ =
                InitializeProcThreadAttributeList(ptr::null_mut(), attribute_count, 0, &mut bytes);
        }
        if bytes == 0 {
            return Err(io::Error::last_os_error());
        }
        let blocks = bytes.div_ceil(mem::size_of::<AttributeStorageBlock>());
        let mut list = Self {
            // HeapAlloc, used by Microsoft's sample, is 16-byte aligned on
            // Win64. Preserve that stronger alignment instead of relying on a
            // Vec<u8> or pointer-sized allocation for this opaque SDK object.
            storage: vec![AttributeStorageBlock::default(); blocks],
            initialized: false,
        };
        // SAFETY: `storage` is pointer-aligned and contains at least `bytes`
        // writable bytes. It is not moved or resized until the list is deleted.
        if unsafe {
            InitializeProcThreadAttributeList(list.as_mut_ptr(), attribute_count, 0, &mut bytes)
        } == FALSE
        {
            return Err(io::Error::last_os_error());
        }
        list.initialized = true;
        Ok(list)
    }

    fn as_mut_ptr(&mut self) -> *mut c_void {
        self.storage.as_mut_ptr().cast::<c_void>()
    }

    fn set_pseudo_console(&mut self, console: HpCon) -> io::Result<()> {
        // SAFETY: the attribute list is initialized for one entry. Unlike
        // scalar process attributes, Microsoft's pseudoconsole contract passes
        // the opaque HPCON value itself as `lpValue` (not the address of an
        // HPCON variable), while `cbSize` is still `sizeof(HPCON)`.
        if unsafe {
            UpdateProcThreadAttribute(
                self.as_mut_ptr(),
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
                console.cast::<c_void>(),
                mem::size_of::<HpCon>(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        } == FALSE
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        if self.initialized {
            // SAFETY: this list was successfully initialized, has not been
            // deleted, and its backing storage is still live and unmoved.
            unsafe { DeleteProcThreadAttributeList(self.as_mut_ptr()) };
        }
    }
}
