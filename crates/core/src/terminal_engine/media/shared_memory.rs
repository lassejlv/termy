/// Read a bounded Kitty shared-memory transfer, releasing the named object.
pub fn read_graphics_shared_memory(
    name: &[u8],
    offset: u64,
    size: Option<u64>,
    limit: usize,
) -> Result<Vec<u8>, String> {
    platform::read(name, offset, size, limit)
}

#[cfg(unix)]
mod platform {
    use std::{ffi::CString, fs::File, os::fd::FromRawFd};

    struct SharedObject(CString);
    impl Drop for SharedObject {
        fn drop(&mut self) {
            // SAFETY: The owned CString remains valid throughout this call.
            unsafe { libc::shm_unlink(self.0.as_ptr()) };
        }
    }

    pub(super) fn read(
        name: &[u8],
        offset: u64,
        size: Option<u64>,
        limit: usize,
    ) -> Result<Vec<u8>, String> {
        let name = CString::new(name).map_err(|_| "EINVAL:invalid shared-memory name")?;
        // SAFETY: A NUL-terminated name, read-only access, and no O_CREAT.
        let fd = unsafe { libc::shm_open(name.as_ptr(), libc::O_RDONLY, 0) };
        if fd < 0 {
            return Err("ENOENT:unable to open shared-memory object".into());
        }
        // SAFETY: shm_open returned a new owned descriptor.
        let file = unsafe { File::from_raw_fd(fd) };
        let _object = SharedObject(name);
        let object_len = file
            .metadata()
            .map_err(|_| "EIO:unable to inspect shared-memory object")?
            .len();
        let available = object_len
            .checked_sub(offset)
            .ok_or("EINVAL:shared-memory offset is out of bounds")?;
        let length = size.unwrap_or(available);
        if length > available {
            return Err("EINVAL:shared-memory range is out of bounds".into());
        }
        if length > limit as u64 {
            return Err("EFBIG:shared-memory transfer exceeds storage limit".into());
        }
        if length == 0 {
            return Ok(Vec::new());
        }
        read_range(&file, offset, length as usize)
    }

    #[cfg(not(target_os = "macos"))]
    fn read_range(file: &File, offset: u64, length: usize) -> Result<Vec<u8>, String> {
        use std::os::unix::fs::FileExt;
        // The producer can truncate the object after metadata validation, even
        // after unlink. A descriptor read returns EOF instead of faulting in a
        // userspace memcpy from an invalidated mapping.
        let mut bytes = vec![0; length];
        file.read_exact_at(&mut bytes, offset)
            .map_err(|_| "EIO:unable to read shared-memory range")?;
        Ok(bytes)
    }

    #[cfg(any(target_os = "macos", test))]
    struct Mapping(*mut libc::c_void, usize);
    #[cfg(any(target_os = "macos", test))]
    impl Drop for Mapping {
        fn drop(&mut self) {
            // SAFETY: This is the exact successful mapping and its original length.
            unsafe { libc::munmap(self.0, self.1) };
        }
    }

    #[cfg(target_os = "macos")]
    fn read_range(file: &File, offset: u64, length: usize) -> Result<Vec<u8>, String> {
        use std::os::fd::AsRawFd;
        unsafe extern "C" {
            static mach_task_self_: u32;
            fn mach_vm_read_overwrite(
                task: u32,
                address: u64,
                size: u64,
                data: u64,
                out_size: *mut u64,
            ) -> i32;
        }
        // Darwin POSIX shared-memory descriptors do not support pread. Let
        // the kernel copy the mapping so inaccessible pages produce an error,
        // rather than dereferencing externally owned storage in Rust.
        // SAFETY: sysconf has no pointer arguments or ownership requirements.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(1) as u64;
        let aligned = offset / page * page;
        let skip = (offset - aligned) as usize;
        let map_len = skip
            .checked_add(length)
            .ok_or("EFBIG:shared-memory range overflow")?;
        let aligned =
            libc::off_t::try_from(aligned).map_err(|_| "EFBIG:shared-memory offset overflow")?;
        // SAFETY: The descriptor is live and the offset page aligned. The
        // producer may change the backing storage, so we never dereference it.
        let address = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                map_len,
                libc::PROT_READ,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                aligned,
            )
        };
        if address == libc::MAP_FAILED {
            return Err("EIO:unable to map shared-memory object".into());
        }
        let mapping = Mapping(address, map_len);
        let mut bytes = vec![0; length];
        let mut copied = 0;
        // SAFETY: The kernel reads our live mapping into an exclusively owned
        // buffer of exactly `length` bytes. It reports mapping faults via the
        // return code; `copied` is writable for the duration of the call.
        let status = unsafe {
            mach_vm_read_overwrite(
                mach_task_self_,
                (mapping.0 as usize + skip) as u64,
                length as u64,
                bytes.as_mut_ptr() as u64,
                &mut copied,
            )
        };
        if status != 0 || copied != length as u64 {
            return Err("EIO:unable to read shared-memory range".into());
        }
        Ok(bytes)
    }

    #[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
    mod tests {
        use super::*;
        use std::os::fd::AsRawFd;

        #[test]
        fn kitty_shared_memory_reads_an_unaligned_range_and_unlinks_it() {
            let name = CString::new(format!("/termy-kitty-{}", std::process::id())).unwrap();
            // SAFETY: A unique name, read/write + create/exclusive flags and mode 0600.
            let fd = unsafe {
                libc::shm_open(
                    name.as_ptr(),
                    libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
                    0o600,
                )
            };
            assert!(fd >= 0);
            // SAFETY: The new file descriptor is owned by this test.
            let file = unsafe { File::from_raw_fd(fd) };
            let _cleanup = SharedObject(name.clone());
            // SAFETY: Resize our own shared object before mapping it.
            assert_eq!(unsafe { libc::ftruncate(file.as_raw_fd(), 8) }, 0);
            // SAFETY: The object is eight bytes long and opened read/write.
            let address = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    8,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    file.as_raw_fd(),
                    0,
                )
            };
            assert_ne!(address, libc::MAP_FAILED);
            let mapping = Mapping(address, 8);
            // SAFETY: Our exclusively owned object has no other writers and is
            // not truncated while this test initializes its eight bytes.
            unsafe {
                std::slice::from_raw_parts_mut(mapping.0.cast::<u8>(), 8)
                    .copy_from_slice(&[9, 1, 2, 3, 255, 8, 7, 6]);
            }
            assert_eq!(
                read(name.as_bytes(), 1, Some(4), 16).unwrap(),
                [1, 2, 3, 255]
            );
            // SAFETY: Read-only open without creation; read must have unlinked it.
            assert!(unsafe { libc::shm_open(name.as_ptr(), libc::O_RDONLY, 0) } < 0);
        }

        #[test]
        fn shared_memory_copy_returns_an_error_after_backing_storage_is_truncated() {
            let file = tempfile::tempfile().unwrap();
            file.set_len(8192).unwrap();
            let validated_length = file.metadata().unwrap().len() as usize;
            // Reproduce the race deterministically at the boundary between
            // validation and copying. A regular file also allows truncation on
            // Darwin, whose shared-memory objects cannot be resized after creation.
            file.set_len(0).unwrap();
            assert!(read_range(&file, 1, validated_length - 1).is_err());
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::{ffi::c_void, os::windows::ffi::OsStrExt};
    #[repr(C)]
    struct MemoryInfo {
        base_address: *mut c_void,
        allocation_base: *mut c_void,
        allocation_protect: u32,
        #[cfg(target_pointer_width = "64")]
        partition_id: u16,
        region_size: usize,
        state: u32,
        protect: u32,
        kind: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenFileMappingW(access: u32, inherit: i32, name: *const u16) -> *mut c_void;
        fn MapViewOfFile(
            mapping: *mut c_void,
            access: u32,
            high: u32,
            low: u32,
            bytes: usize,
        ) -> *mut c_void;
        fn UnmapViewOfFile(address: *const c_void) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn VirtualQuery(address: *const c_void, info: *mut MemoryInfo, length: usize) -> usize;
    }
    struct Mapping {
        handle: *mut c_void,
        view: *mut c_void,
    }
    impl Drop for Mapping {
        fn drop(&mut self) {
            // SAFETY: These handles came from successful Windows API calls.
            unsafe {
                if !self.view.is_null() {
                    UnmapViewOfFile(self.view);
                }
                CloseHandle(self.handle);
            }
        }
    }
    pub(super) fn read(
        name: &[u8],
        offset: u64,
        size: Option<u64>,
        limit: usize,
    ) -> Result<Vec<u8>, String> {
        let name = std::str::from_utf8(name).map_err(|_| "EINVAL:invalid shared-memory name")?;
        if name.contains('\0') {
            return Err("EINVAL:invalid shared-memory name".into());
        }
        let name: Vec<_> = std::ffi::OsStr::new(name)
            .encode_wide()
            .chain(Some(0))
            .collect();
        // SAFETY: NUL-terminated UTF-16 name, read-only access, no handle inheritance.
        let handle = unsafe { OpenFileMappingW(4, 0, name.as_ptr()) };
        if handle.is_null() {
            return Err("ENOENT:unable to open shared-memory object".into());
        }
        let mut mapping = Mapping {
            handle,
            view: std::ptr::null_mut(),
        };
        // SAFETY: The owned mapping handle is valid; request the existing full view.
        mapping.view = unsafe { MapViewOfFile(handle, 4, 0, 0, 0) };
        if mapping.view.is_null() {
            return Err("EIO:unable to map shared-memory object".into());
        }
        let mut info = std::mem::MaybeUninit::<MemoryInfo>::zeroed();
        // SAFETY: A live mapped address and writable buffer of the declared size.
        if unsafe {
            VirtualQuery(
                mapping.view,
                info.as_mut_ptr(),
                std::mem::size_of::<MemoryInfo>(),
            )
        } == 0
        {
            return Err("EIO:unable to inspect shared-memory object".into());
        }
        // SAFETY: VirtualQuery succeeded and initialized the output structure.
        let info = unsafe { info.assume_init() };
        let available = (info.region_size as u64)
            .checked_sub(offset)
            .ok_or("EINVAL:shared-memory offset is out of bounds")?;
        let length = size.unwrap_or(available);
        if length > limit as u64 {
            return Err("EFBIG:shared-memory transfer exceeds storage limit".into());
        }
        if length > available {
            return Err("EINVAL:shared-memory range is out of bounds".into());
        }
        // SAFETY: The requested range is bounded by the mapped region and the
        // protocol quota, and the Mapping guard keeps the view alive during copying.
        Ok(unsafe {
            std::slice::from_raw_parts(
                mapping.view.cast::<u8>().add(offset as usize),
                length as usize,
            )
        }
        .to_vec())
    }
}

#[cfg(not(any(unix, windows)))]
mod platform {
    pub(super) fn read(_: &[u8], _: u64, _: Option<u64>, _: usize) -> Result<Vec<u8>, String> {
        Err("ENOTSUP:shared-memory transport unavailable on this platform".into())
    }
}
