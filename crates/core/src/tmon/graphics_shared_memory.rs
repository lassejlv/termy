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
    use std::{
        ffi::{CString, c_char, c_int, c_void},
        fs::File,
        os::fd::FromRawFd,
    };

    #[cfg_attr(target_os = "linux", link(name = "rt"))]
    unsafe extern "C" {
        fn shm_open(name: *const c_char, flags: c_int, ...) -> c_int;
        fn shm_unlink(name: *const c_char) -> c_int;
        fn mmap(
            address: *mut c_void,
            length: usize,
            protection: c_int,
            flags: c_int,
            fd: c_int,
            offset: i64,
        ) -> *mut c_void;
        fn munmap(address: *mut c_void, length: usize) -> c_int;
        fn getpagesize() -> c_int;
    }

    struct SharedObject(CString);
    impl Drop for SharedObject {
        fn drop(&mut self) {
            // SAFETY: The owned CString remains valid throughout this call.
            unsafe {
                shm_unlink(self.0.as_ptr());
            }
        }
    }
    struct Mapping(*mut c_void, usize);
    impl Drop for Mapping {
        fn drop(&mut self) {
            // SAFETY: This is the exact successful mapping and its original length.
            unsafe {
                munmap(self.0, self.1);
            }
        }
    }

    pub(super) fn read(
        name: &[u8],
        offset: u64,
        size: Option<u64>,
        limit: usize,
    ) -> Result<Vec<u8>, String> {
        use std::os::fd::AsRawFd;
        let name = CString::new(name).map_err(|_| "EINVAL:invalid shared-memory name")?;
        // SAFETY: A NUL-terminated name, O_RDONLY (zero), and no O_CREAT.
        let fd = unsafe { shm_open(name.as_ptr(), 0) };
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
        // SAFETY: getpagesize has no arguments or ownership requirements.
        let page = unsafe { getpagesize() }.max(1) as u64;
        let aligned = offset / page * page;
        let skip = (offset - aligned) as usize;
        let map_len = skip
            .checked_add(length as usize)
            .ok_or("EFBIG:shared-memory range overflow")?;
        // SAFETY: Bounds were checked against the descriptor's object size. The
        // mapping is read-only, page aligned, and kept alive until the copy ends.
        let address = unsafe {
            mmap(
                std::ptr::null_mut(),
                map_len,
                1,
                1,
                file.as_raw_fd(),
                aligned as i64,
            )
        };
        if address as isize == -1 {
            return Err("EIO:unable to map shared-memory object".into());
        }
        let mapping = Mapping(address, map_len);
        // SAFETY: The bounded range lies inside the live read-only mapping.
        Ok(
            unsafe {
                std::slice::from_raw_parts(mapping.0.cast::<u8>().add(skip), length as usize)
            }
            .to_vec(),
        )
    }

    #[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
    mod tests {
        use super::*;
        use std::os::fd::AsRawFd;
        unsafe extern "C" {
            fn ftruncate(fd: c_int, length: i64) -> c_int;
        }

        #[test]
        fn kitty_shared_memory_reads_an_unaligned_range_and_unlinks_it() {
            let name = CString::new(format!("/termy-kitty-{}", std::process::id())).unwrap();
            #[cfg(target_os = "linux")]
            let flags = 2 | 64 | 128;
            #[cfg(target_os = "macos")]
            let flags = 2 | 0x200 | 0x800;
            // SAFETY: A unique name, read/write + create/exclusive flags and mode 0600.
            let fd = unsafe { shm_open(name.as_ptr(), flags, 0o600u32) };
            assert!(fd >= 0);
            // SAFETY: The new file descriptor is owned by this test.
            let file = unsafe { File::from_raw_fd(fd) };
            let _cleanup = SharedObject(name.clone());
            // SAFETY: Resize our own shared object before mapping it.
            assert_eq!(unsafe { ftruncate(file.as_raw_fd(), 8) }, 0);
            // SAFETY: The object is eight bytes long and opened read/write.
            let address = unsafe { mmap(std::ptr::null_mut(), 8, 3, 1, file.as_raw_fd(), 0) };
            assert_ne!(address as isize, -1);
            let mapping = Mapping(address, 8);
            // SAFETY: Write exactly the bounds of the live writable mapping.
            unsafe {
                std::slice::from_raw_parts_mut(mapping.0.cast::<u8>(), 8)
                    .copy_from_slice(&[9, 1, 2, 3, 255, 8, 7, 6]);
            }
            assert_eq!(
                read(name.as_bytes(), 1, Some(4), 16).unwrap(),
                [1, 2, 3, 255]
            );
            // SAFETY: Read-only open without creation; the terminal must have unlinked it.
            assert!(unsafe { shm_open(name.as_ptr(), 0) } < 0);
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
