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
    let data = resolve_transmission_data(command, decoded)?;
    let data = match command.char_value('o') {
        None => data,
        Some('z') => raw_image_bytes(command).and_then(|raw| decompress_zlib(&data, raw))?,
        Some(_) => return Err("EINVAL:unsupported compression".into()),
    };
    normalize_image(command, data)
}

fn resolve_transmission_data(
    command: &KittyGraphicsCommand,
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
                command.u32_value('O').unwrap_or(0) as u64,
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
        's' => crate::terminal_engine::media::read_graphics_shared_memory(
            &decoded,
            u64::from(command.u32_value('O').unwrap_or(0)),
            command
                .u32_value('S')
                .filter(|size| *size > 0)
                .map(u64::from),
            MAX_IMAGE_BYTES,
        ),
        _ => Err("EINVAL:unsupported transmission medium".into()),
    }
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

pub(super) fn read_regular_file(
    path: &Path,
    offset: u64,
    size: Option<u64>,
) -> Result<Vec<u8>, String> {
    // Skip FIFOs, devices and pseudo-files without opening them; opening a
    // device can have side effects. A child controls this path and may swap it
    // at any time, so the opened file is checked again below.
    let initial_metadata =
        std::fs::metadata(path).map_err(|_| "ENOENT:unable to open image file".to_string())?;
    if !initial_metadata.is_file() || is_pseudo_file(path) {
        return Err("EINVAL:invalid image file".into());
    }
    let mut file = open_image_file(path)?;
    let metadata = file
        .metadata()
        .map_err(|_| "EIO:unable to inspect image file".to_string())?;
    if !metadata.is_file() || !opened_file_is_allowed(path, &metadata) || offset > metadata.len() {
        return Err("EINVAL:invalid image file".into());
    }
    let length = size
        .unwrap_or(metadata.len() - offset)
        .min(metadata.len() - offset);
    if length > MAX_IMAGE_BYTES as u64 {
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
            .map_err(|_| "ENOENT:unable to open image file".to_string())
    }

    #[cfg(not(unix))]
    File::open(path).map_err(|_| "ENOENT:unable to open image file".to_string())
}

/// Whether the file opened from `path` may be read: `path` must still resolve
/// to that same file, outside the kernel pseudo-filesystems. Checking before
/// `open` alone would let a symlink swapped in between pass.
pub(super) fn opened_file_is_allowed(path: &Path, opened: &std::fs::Metadata) -> bool {
    let Ok(canonical) = path.canonicalize() else {
        return false;
    };
    if is_pseudo_path(&canonical) {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        std::fs::metadata(&canonical)
            .is_ok_and(|resolved| (resolved.dev(), resolved.ino()) == (opened.dev(), opened.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = opened;
        true
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
    let mut roots = vec![PathBuf::from("/tmp"), PathBuf::from("/private/tmp")];
    roots.push(std::env::temp_dir());
    roots.into_iter().any(|root| {
        root.canonicalize()
            .is_ok_and(|root| canonical.starts_with(root))
    })
}
