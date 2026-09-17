use super::*;

pub(super) fn resolve_transmission_data(
    command: &GraphicsCommand,
    decoded: Vec<u8>,
) -> Result<Vec<u8>, String> {
    match command.char_value('t').unwrap_or('d') {
        'd' => Ok(decoded),
        'f' | 't' => {
            let temporary = command.char_value('t') == Some('t');
            let path = PathBuf::from(
                std::str::from_utf8(&decoded).map_err(|_| "EINVAL:file path is not UTF-8")?,
            );
            let result = read_regular_file(
                &path,
                u64::from(command.u32_value('O').unwrap_or(0)),
                command
                    .u32_value('S')
                    .filter(|size| *size > 0)
                    .map(u64::from),
            );
            if temporary && result.is_ok() && temporary_path_can_be_removed(&path) {
                let _ = std::fs::remove_file(&path);
            }
            result
        }
        's' => crate::tmon::read_graphics_shared_memory(
            &decoded,
            u64::from(command.u32_value('O').unwrap_or(0)),
            command
                .u32_value('S')
                .filter(|size| *size > 0)
                .map(u64::from),
            MAX_UPLOAD_BYTES,
        ),
        _ => Err("EINVAL:unsupported transmission medium".into()),
    }
}

pub(super) fn read_regular_file(
    path: &Path,
    offset: u64,
    size: Option<u64>,
) -> Result<Vec<u8>, String> {
    // Reject FIFOs and devices before opening: a child controls this path and a
    // blocking FIFO open would otherwise stall the parser thread indefinitely.
    let initial_metadata =
        std::fs::metadata(path).map_err(|_| "ENOENT:unable to open image file".to_string())?;
    if !initial_metadata.is_file() {
        return Err("EINVAL:invalid image file".into());
    }
    let mut file = open_image_file(path)?;
    let metadata = file
        .metadata()
        .map_err(|_| "EIO:unable to inspect image file".to_string())?;
    if !metadata.is_file() || offset > metadata.len() {
        return Err("EINVAL:invalid image file".into());
    }
    let length = size
        .unwrap_or(metadata.len() - offset)
        .min(metadata.len() - offset);
    if length > MAX_UPLOAD_BYTES as u64 {
        return Err("EFBIG:image file exceeds storage limit".into());
    }
    file.seek(SeekFrom::Start(offset))
        .map_err(|_| "EIO:unable to seek image file".to_string())?;
    let mut output = Vec::with_capacity(length as usize);
    file.take(length)
        .read_to_end(&mut output)
        .map_err(|_| "EIO:unable to read image file".to_string())?;
    Ok(output)
}

pub(super) fn open_image_file(path: &Path) -> Result<File, String> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const NONBLOCK: i32 = 0o4000;
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    const NONBLOCK: i32 = 0x0004;

    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    {
        use std::{fs::OpenOptions, os::unix::fs::OpenOptionsExt};

        OpenOptions::new()
            .read(true)
            .custom_flags(NONBLOCK)
            .open(path)
            .map_err(|_| "ENOENT:unable to open image file".to_string())
    }

    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    )))]
    File::open(path).map_err(|_| "ENOENT:unable to open image file".to_string())
}

pub(super) fn temporary_path_can_be_removed(path: &Path) -> bool {
    let Ok(canonical) = path.canonicalize() else {
        return false;
    };
    if !canonical.components().any(|component| {
        component
            .as_os_str()
            .to_string_lossy()
            .contains("tty-graphics-protocol")
    }) {
        return false;
    }
    let mut roots = vec![
        PathBuf::from("/tmp"),
        PathBuf::from("/private/tmp"),
        PathBuf::from("/dev/shm"),
    ];
    roots.push(std::env::temp_dir());
    roots.into_iter().any(|root| {
        root.canonicalize()
            .is_ok_and(|root| canonical.starts_with(root))
    })
}
