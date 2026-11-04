//! The stateless half of an image upload: payload decoding, file and
//! shared-memory reads, decompression and image normalization. It touches no
//! terminal state, so the runtime can run it without holding the engine lock.

use super::*;
use crate::terminal_engine::media::GraphicsImage;
use base64::Engine as _;
use flate2::read::ZlibDecoder;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

/// A complete upload whose image has not been decoded yet.
pub(crate) struct KittyUpload {
    pub(super) command: KittyGraphicsCommand,
    pub(super) data: UploadData,
    pub(super) screen: KittyGraphicsScreen,
}

pub(super) enum UploadData {
    /// A single-chunk payload, still base64-encoded.
    Base64(Vec<u8>),
    Decoded(Vec<u8>),
}

/// The result of [`KittyUpload::prepare`], committed by
/// [`KittyGraphicsState::commit_upload`].
pub(crate) struct KittyPreparedUpload {
    pub(super) command: KittyGraphicsCommand,
    pub(super) image: Result<(GraphicsImage, u32, u32), String>,
    pub(super) screen: KittyGraphicsScreen,
}

impl KittyUpload {
    pub(crate) fn prepare(self) -> KittyPreparedUpload {
        let image = decode(&self.command, self.data);
        KittyPreparedUpload {
            command: self.command,
            image,
            screen: self.screen,
        }
    }
}

/// The one reply for every failure of a local medium (`t=f`, `t=t`, `t=s`).
///
/// These media read the local machine, but the client may be a program on a
/// remote host or a byte stream fed to a display-only terminal. Telling it
/// whether a name was missing, special, too large, too short or not an image
/// would let it probe for the existence, type and size of files it cannot
/// read itself, so all of these share one reply, as in kitty. The actual
/// reason is only logged locally.
pub(super) const LOCAL_MEDIUM_ERROR: &str = "EBADF:failed to read image file";

fn decode(
    command: &KittyGraphicsCommand,
    data: UploadData,
) -> Result<(GraphicsImage, u32, u32), String> {
    let decoded = match data {
        UploadData::Base64(encoded) => match PAYLOAD_BASE64.decode(encoded) {
            Ok(decoded) if decoded.len() <= MAX_IMAGE_BYTES => decoded,
            Ok(_) => return Err("EFBIG:image payload exceeds storage limit".into()),
            Err(_) => return Err("EINVAL:invalid base64 payload".into()),
        },
        UploadData::Decoded(decoded) => decoded,
    };
    match command.char_value('t').unwrap_or('d') {
        'd' => decode_image(command, decoded),
        medium @ ('f' | 't' | 's') => {
            // Malformed controls reveal nothing local, so they keep their
            // specific replies, and are rejected before anything is opened.
            validate_local_parameters(command)?;
            decode_local(command, medium, &decoded).map_err(|reason| {
                log::warn!("kitty graphics t={medium} upload failed: {reason}");
                LOCAL_MEDIUM_ERROR.to_string()
            })
        }
        _ => Err("EINVAL:unsupported transmission medium".into()),
    }
}

fn decode_image(
    command: &KittyGraphicsCommand,
    data: Vec<u8>,
) -> Result<(GraphicsImage, u32, u32), String> {
    let data = match command.char_value('o') {
        None => data,
        Some('z') => raw_image_bytes(command).and_then(|raw| decompress_zlib(&data, raw))?,
        Some(_) => return Err("EINVAL:unsupported compression".into()),
    };
    normalize_image(command, data)
}

/// The checks of [`decode_image`] that depend on the controls alone.
fn validate_local_parameters(command: &KittyGraphicsCommand) -> Result<(), String> {
    if !matches!(command.char_value('o'), None | Some('z')) {
        return Err("EINVAL:unsupported compression".into());
    }
    if !matches!(command.u32_value('f').unwrap_or(32), 24 | 32 | 100) {
        return Err("EINVAL:unsupported image format".into());
    }
    raw_image_bytes(command).map(drop)
}

/// Read, decode and then consume a local medium. A temporary file or shared
/// memory object is removed only once the whole upload succeeded: the client
/// can name any object, and a failed upload must not destroy it.
fn decode_local(
    command: &KittyGraphicsCommand,
    medium: char,
    name: &[u8],
) -> Result<(GraphicsImage, u32, u32), String> {
    let offset = u64::from(command.u32_value('O').unwrap_or(0));
    let size = command
        .u32_value('S')
        .filter(|size| *size > 0)
        .map(u64::from);
    if medium == 's' {
        let data = crate::terminal_engine::media::read_graphics_shared_memory(
            name,
            offset,
            size,
            MAX_IMAGE_BYTES,
        )?;
        let image = decode_image(command, data)?;
        // Unlike temporary files, kitty requires no name marker here (its own
        // icat uses `icat-*`), and the protocol has the terminal unlink it.
        crate::terminal_engine::media::unlink_graphics_shared_memory(name);
        return Ok(image);
    }
    let path = Path::new(std::str::from_utf8(name).map_err(|_| "file path is not UTF-8")?);
    let file = read_regular_file(path, offset, size)?;
    let image = decode_image(command, file.data)?;
    if medium == 't' && temporary_path_can_be_removed(&file.canonical) {
        remove_opened_file(&file.canonical, &file.metadata);
    }
    Ok(image)
}

pub(super) fn validate_dimensions(
    width: u32,
    height: u32,
    channels: usize,
) -> Result<usize, String> {
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err("EINVAL:invalid image dimensions".into());
    }
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_PIXELS {
        return Err("EFBIG:image dimensions exceed storage limit".into());
    }
    usize::try_from(pixels)
        .ok()
        .and_then(|pixels| pixels.checked_mul(channels))
        .ok_or_else(|| "EFBIG:image dimensions overflow".into())
}

/// Exact decoded length of raw RGB/RGBA data, which bounds decompression.
fn raw_image_bytes(command: &KittyGraphicsCommand) -> Result<Option<usize>, String> {
    match command.u32_value('f').unwrap_or(32) {
        format @ (24 | 32) => validate_dimensions(
            command.u32_value('s').unwrap_or(0),
            command.u32_value('v').unwrap_or(0),
            if format == 24 { 3 } else { 4 },
        )
        .map(Some),
        _ => Ok(None),
    }
}

/// Inflate at most the declared raw size (or the storage limit for PNG), so a
/// small compressed payload cannot expand to the limit before validation.
pub(super) fn decompress_zlib(data: &[u8], raw_bytes: Option<usize>) -> Result<Vec<u8>, String> {
    let limit = raw_bytes.unwrap_or(MAX_IMAGE_BYTES).min(MAX_IMAGE_BYTES);
    // A known raw size fits exactly; growing by doubling would leave up to
    // half the buffer unused (and uncharged) in the stored image. Deflate
    // expands at most ~1032x, so a tiny payload can't reserve the limit.
    let mut output =
        Vec::with_capacity(raw_bytes.map_or(0, |_| limit.min(data.len().saturating_mul(1032))));
    ZlibDecoder::new(data)
        .take(limit as u64 + 1)
        .read_to_end(&mut output)
        .map_err(|_| "EINVAL:invalid zlib payload".to_string())?;
    if output.len() > limit {
        return Err(if raw_bytes.is_some() {
            "EINVAL:pixel data length does not match dimensions".into()
        } else {
            "EFBIG:decompressed image exceeds storage limit".into()
        });
    }
    Ok(output)
}

/// Bytes read through `t=f`/`t=t`, with the identity of the file they came
/// from, so a temporary file is removed only if it is that same file.
pub(super) struct LocalFile {
    pub(super) data: Vec<u8>,
    /// Where the opened file was found once every link was resolved.
    pub(super) canonical: PathBuf,
    pub(super) metadata: std::fs::Metadata,
}

/// Read a regular file for `t=f`/`t=t`. The errors are logged locally only;
/// the client always gets [`LOCAL_MEDIUM_ERROR`].
pub(super) fn read_regular_file(
    path: &Path,
    offset: u64,
    size: Option<u64>,
) -> Result<LocalFile, String> {
    // Skip FIFOs, devices and pseudo-files without opening them; opening a
    // device can have side effects. A child controls this path and may swap it
    // at any time, so the opened file is checked again below.
    let initial_metadata = std::fs::metadata(path).map_err(|_| "unable to inspect image file")?;
    if !initial_metadata.is_file() {
        return Err("image file is not a regular file".into());
    }
    if is_pseudo_file(path) {
        return Err("image file is a kernel pseudo-file".into());
    }
    let mut file = open_image_file(path)?;
    let metadata = file
        .metadata()
        .map_err(|_| "unable to inspect opened image file")?;
    if !metadata.is_file() {
        return Err("opened image file is not a regular file".into());
    }
    let canonical =
        opened_file_is_allowed(path, &metadata).ok_or("image file changed while it was opened")?;
    let available = metadata
        .len()
        .checked_sub(offset)
        .ok_or("image file offset is past its end")?;
    let length = size.unwrap_or(available).min(available);
    if length > MAX_IMAGE_BYTES as u64 {
        return Err("image file exceeds storage limit".into());
    }
    file.seek(SeekFrom::Start(offset))
        .map_err(|_| "unable to seek image file")?;
    let mut data = Vec::with_capacity(length as usize);
    file.take(length)
        .read_to_end(&mut data)
        .map_err(|_| "unable to read image file")?;
    Ok(LocalFile {
        data,
        canonical,
        metadata,
    })
}

fn open_image_file(path: &Path) -> Result<File, String> {
    // Non-blocking, so a FIFO swapped in after the type check can't stall the
    // parser thread in `open`, and never adopting a terminal as controlling tty.
    #[cfg(unix)]
    {
        use std::{fs::OpenOptions, os::unix::fs::OpenOptionsExt};

        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
            .open(path)
            .map_err(|_| "unable to open image file".to_string())
    }

    #[cfg(not(unix))]
    File::open(path).map_err(|_| "unable to open image file".to_string())
}

/// The canonical path of the file opened from `path`, if it may be read:
/// `path` must still resolve to that same file, outside the kernel
/// pseudo-filesystems. Checking before `open` alone would let a symlink
/// swapped in between pass.
pub(super) fn opened_file_is_allowed(path: &Path, opened: &std::fs::Metadata) -> Option<PathBuf> {
    let canonical = path.canonicalize().ok()?;
    if is_pseudo_path(&canonical) {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let resolved = std::fs::metadata(&canonical).ok()?;
        ((resolved.dev(), resolved.ino()) == (opened.dev(), opened.ino())).then_some(canonical)
    }
    #[cfg(not(unix))]
    {
        let _ = opened;
        Some(canonical)
    }
}

pub(super) fn is_pseudo_file(path: &Path) -> bool {
    path.canonicalize().is_ok_and(|path| is_pseudo_path(&path))
}

/// Kernel pseudo-files report as regular files but expose process and
/// system state, so a program must not read them through `t=f` (as in kitty).
fn is_pseudo_path(canonical: &Path) -> bool {
    ["/proc", "/sys", "/dev"]
        .iter()
        .any(|root| canonical.starts_with(root))
        && !canonical.starts_with("/dev/shm")
}

/// Whether a `t=t` file may be deleted after it was read: its resolved name
/// must carry the protocol's `tty-graphics-protocol` marker and lie in a
/// temporary directory (the same roots kitty accepts).
pub(super) fn temporary_path_can_be_removed(path: &Path) -> bool {
    let Ok(canonical) = path.canonicalize() else {
        return false;
    };
    // Check the resolved name: `.../tty-graphics-protocol-dir/../victim`
    // carries the marker only in a directory that is no longer on the path.
    if !canonical
        .file_name()
        .is_some_and(|name| name.to_string_lossy().contains("tty-graphics-protocol"))
    {
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

/// Remove `canonical` only if it is still the very file that was read.
///
/// The directory entry is inspected without following links and removed
/// through the same directory descriptor, so a symlink (or any other file)
/// put in its place is left alone, and so is whatever a symlink points to.
pub(super) fn remove_opened_file(canonical: &Path, opened: &std::fs::Metadata) {
    #[cfg(unix)]
    {
        use std::{
            ffi::CString,
            fs::OpenOptions,
            os::{
                fd::AsRawFd,
                unix::{ffi::OsStrExt, fs::MetadataExt, fs::OpenOptionsExt},
            },
        };

        let (Some(parent), Some(name)) = (canonical.parent(), canonical.file_name()) else {
            return;
        };
        let Ok(name) = CString::new(name.as_bytes()) else {
            return;
        };
        let Ok(directory) = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(parent)
        else {
            return;
        };
        let mut entry = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: A live directory descriptor, a NUL-terminated name and a
        // writable stat buffer; AT_SYMLINK_NOFOLLOW inspects the entry itself.
        let status = unsafe {
            libc::fstatat(
                directory.as_raw_fd(),
                name.as_ptr(),
                entry.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if status != 0 {
            return;
        }
        // SAFETY: fstatat succeeded and initialized the buffer.
        let entry = unsafe { entry.assume_init() };
        // The field types vary by platform (`st_dev` is i32 on Darwin).
        #[allow(clippy::unnecessary_cast)]
        let same_file = (entry.st_mode & libc::S_IFMT) == libc::S_IFREG
            && (entry.st_dev as u64, entry.st_ino as u64) == (opened.dev(), opened.ino());
        if same_file {
            // SAFETY: The same live descriptor and NUL-terminated name; flags
            // of 0 remove a non-directory entry and never follow a link.
            unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) };
        }
    }
    #[cfg(not(unix))]
    {
        let _ = opened;
        // `symlink_metadata` describes a link itself, which is never removed.
        if std::fs::symlink_metadata(canonical).is_ok_and(|entry| entry.is_file()) {
            let _ = std::fs::remove_file(canonical);
        }
    }
}
