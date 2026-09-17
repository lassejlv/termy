#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
use std::{
    collections::HashSet,
    sync::{Mutex, OnceLock},
};

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
use clipboard_rs::{
    Clipboard, ClipboardContent, ClipboardContext, ContentFormat, common::RustImage,
};
#[cfg(target_os = "macos")]
use objc2_foundation::NSString;
#[cfg(target_os = "macos")]
use objc2_uniform_type_identifiers::UTType;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeClipboardContent {
    pub mime_type: String,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NativeClipboardError {
    Unsupported,
    Unavailable,
    InvalidData,
    Io(String),
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
static CLIPBOARD: OnceLock<Mutex<Option<ClipboardContext>>> = OnceLock::new();

pub fn available_clipboard_formats() -> Result<Vec<String>, NativeClipboardError> {
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    {
        with_clipboard(|clipboard| {
            let mut formats = Vec::new();
            if clipboard.has(ContentFormat::Text) {
                push_unique(&mut formats, "text/plain");
            }
            if clipboard.has(ContentFormat::Html) {
                push_unique(&mut formats, "text/html");
            }
            if clipboard.has(ContentFormat::Rtf) {
                push_unique(&mut formats, "text/rtf");
            }
            if clipboard.has(ContentFormat::Image) {
                push_unique(&mut formats, "image/png");
            }
            if clipboard.has(ContentFormat::Files) {
                push_unique(&mut formats, "text/uri-list");
            }
            for native in clipboard.available_formats().map_err(io_error)? {
                if let Some(mime_type) = native_format_to_mime(&native) {
                    push_unique(&mut formats, &mime_type);
                }
            }
            Ok(formats)
        })
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        Err(NativeClipboardError::Unsupported)
    }
}

pub fn read_clipboard_formats(
    requested: &[String],
) -> Result<Vec<NativeClipboardContent>, NativeClipboardError> {
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    {
        with_clipboard(|clipboard| {
            let mut contents = Vec::new();
            for mime_type in requested {
                let data = match mime_type.as_str() {
                    "text/plain" if clipboard.has(ContentFormat::Text) => {
                        clipboard.get_text().map(String::into_bytes).ok()
                    }
                    "text/html" if clipboard.has(ContentFormat::Html) => {
                        clipboard.get_html().map(String::into_bytes).ok()
                    }
                    "text/rtf" | "application/rtf" if clipboard.has(ContentFormat::Rtf) => {
                        clipboard.get_rich_text().map(String::into_bytes).ok()
                    }
                    "image/png" if clipboard.has(ContentFormat::Image) => clipboard
                        .get_image()
                        .and_then(|image| image.to_png())
                        .map(|image| image.get_bytes().to_vec())
                        .ok(),
                    "text/uri-list" if clipboard.has(ContentFormat::Files) => clipboard
                        .get_files()
                        .map(|files| file_paths_to_uri_list(&files).into_bytes())
                        .ok(),
                    mime_type => {
                        let native = mime_to_native_format(mime_type);
                        if clipboard.has(ContentFormat::Other(native.clone())) {
                            clipboard.get_buffer(&native).ok()
                        } else {
                            None
                        }
                    }
                };
                if let Some(data) = data {
                    contents.push(NativeClipboardContent {
                        mime_type: mime_type.clone(),
                        data,
                    });
                }
            }
            if contents.is_empty() {
                Err(NativeClipboardError::Unavailable)
            } else {
                Ok(contents)
            }
        })
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = requested;
        Err(NativeClipboardError::Unsupported)
    }
}

pub fn write_clipboard_contents(
    contents: Vec<NativeClipboardContent>,
) -> Result<(), NativeClipboardError> {
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    {
        with_clipboard(|clipboard| {
            if contents.is_empty() {
                return clipboard.clear().map_err(io_error);
            }

            let mut native_contents = Vec::with_capacity(contents.len());
            let mut formats = HashSet::new();
            for content in contents {
                let native_format = mime_to_native_format(&content.mime_type);
                if !formats.insert(native_format.clone()) {
                    continue;
                }
                let content = match content.mime_type.as_str() {
                    "text/plain" => ClipboardContent::Text(
                        String::from_utf8(content.data)
                            .map_err(|_| NativeClipboardError::InvalidData)?,
                    ),
                    "text/html" => ClipboardContent::Html(
                        String::from_utf8(content.data)
                            .map_err(|_| NativeClipboardError::InvalidData)?,
                    ),
                    "text/rtf" | "application/rtf" => ClipboardContent::Rtf(
                        String::from_utf8(content.data)
                            .map_err(|_| NativeClipboardError::InvalidData)?,
                    ),
                    "text/uri-list" => match uri_list_to_file_paths(&content.data) {
                        Some(files) if !files.is_empty() => ClipboardContent::Files(files),
                        _ => ClipboardContent::Other(native_format, content.data),
                    },
                    _ => ClipboardContent::Other(native_format, content.data),
                };
                native_contents.push(content);
            }
            if native_contents.is_empty() {
                return Err(NativeClipboardError::InvalidData);
            }
            clipboard.set(native_contents).map_err(io_error)
        })
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = contents;
        Err(NativeClipboardError::Unsupported)
    }
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn with_clipboard<T>(
    operation: impl FnOnce(&ClipboardContext) -> Result<T, NativeClipboardError>,
) -> Result<T, NativeClipboardError> {
    let mut clipboard = CLIPBOARD
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|_| NativeClipboardError::Io("clipboard lock poisoned".to_string()))?;
    if clipboard.is_none() {
        *clipboard = Some(ClipboardContext::new().map_err(io_error)?);
    }
    operation(clipboard.as_ref().expect("clipboard context initialized"))
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn io_error(error: impl std::fmt::Display) -> NativeClipboardError {
    NativeClipboardError::Io(error.to_string())
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn push_unique(formats: &mut Vec<String>, format: &str) {
    if !formats.iter().any(|existing| existing == format) {
        formats.push(format.to_string());
    }
}

#[cfg(target_os = "macos")]
fn native_format_to_mime(format: &str) -> Option<String> {
    let mime_type = match format {
        "public.utf8-plain-text" | "public.utf16-external-plain-text" | "NSStringPboardType" => {
            "text/plain"
        }
        "public.html" => "text/html",
        "public.rtf" | "public.rtfd" => "text/rtf",
        "public.png" => "image/png",
        "public.jpeg" => "image/jpeg",
        "public.tiff" => "image/tiff",
        "com.compuserve.gif" => "image/gif",
        "org.webmproject.webp" => "image/webp",
        "public.svg-image" => "image/svg+xml",
        "public.file-url" => "text/uri-list",
        other => {
            return UTType::typeWithIdentifier(&NSString::from_str(other))
                .and_then(|format| format.preferredMIMEType())
                .map(|mime_type| mime_type.to_string());
        }
    };
    Some(mime_type.to_string())
}

#[cfg(target_os = "windows")]
fn native_format_to_mime(format: &str) -> Option<String> {
    let mime_type = match format {
        "HTML Format" => "text/html",
        "Rich Text Format" => "text/rtf",
        "PNG" => "image/png",
        "JFIF" => "image/jpeg",
        other if other.contains('/') => other,
        _ => return None,
    };
    Some(mime_type.to_string())
}

#[cfg(target_os = "linux")]
fn native_format_to_mime(format: &str) -> Option<String> {
    let mime_type = match format {
        "UTF8_STRING" | "TEXT" | "STRING" | "text/plain;charset=utf-8" => "text/plain",
        "text/html" => "text/html",
        "text/rtf" | "application/rtf" => "text/rtf",
        "image/png" => "image/png",
        "text/uri-list" => "text/uri-list",
        other if other.contains('/') => other,
        _ => return None,
    };
    Some(mime_type.to_string())
}

#[cfg(target_os = "macos")]
fn mime_to_native_format(mime_type: &str) -> String {
    match mime_type {
        "text/plain" => "public.utf8-plain-text",
        "text/html" => "public.html",
        "text/rtf" | "application/rtf" => "public.rtf",
        "image/png" => "public.png",
        "image/jpeg" => "public.jpeg",
        "image/tiff" => "public.tiff",
        "image/gif" => "com.compuserve.gif",
        "image/webp" => "org.webmproject.webp",
        "image/svg+xml" => "public.svg-image",
        "text/uri-list" => "public.file-url",
        other => {
            return UTType::typeWithMIMEType(&NSString::from_str(other)).map_or_else(
                || other.to_string(),
                |format| format.identifier().to_string(),
            );
        }
    }
    .to_string()
}

#[cfg(target_os = "windows")]
fn mime_to_native_format(mime_type: &str) -> String {
    match mime_type {
        "text/plain" => "UnicodeText",
        "text/html" => "HTML Format",
        "text/rtf" | "application/rtf" => "Rich Text Format",
        "image/png" => "PNG",
        "image/jpeg" => "JFIF",
        other => other,
    }
    .to_string()
}

#[cfg(target_os = "linux")]
fn mime_to_native_format(mime_type: &str) -> String {
    mime_type.to_string()
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn file_paths_to_uri_list(files: &[String]) -> String {
    files
        .iter()
        .filter_map(|file| url::Url::from_file_path(file).ok())
        .map(|url| url.to_string())
        .collect::<Vec<_>>()
        .join("\r\n")
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn uri_list_to_file_paths(data: &[u8]) -> Option<Vec<String>> {
    let data = std::str::from_utf8(data).ok()?;
    let mut files = Vec::new();
    for line in data.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let url = url::Url::parse(line).ok()?;
        files.push(url.to_file_path().ok()?.to_string_lossy().into_owned());
    }
    Some(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_uri_lists_round_trip() {
        let path = if cfg!(target_os = "windows") {
            "C:\\Users\\Termy\\clipboard.png"
        } else {
            "/tmp/termy clipboard.png"
        };
        let encoded = file_paths_to_uri_list(&[path.to_string()]);
        assert_eq!(
            uri_list_to_file_paths(encoded.as_bytes()),
            Some(vec![path.to_string()])
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_dynamic_mime_types_round_trip_through_valid_utis() {
        for mime_type in ["text/utf8", "application/x-termy-test"] {
            let native = mime_to_native_format(mime_type);
            assert!(
                !native.contains('/'),
                "macOS format must be a UTI: {native}"
            );
            assert_eq!(native_format_to_mime(&native).as_deref(), Some(mime_type));
        }
    }
}
