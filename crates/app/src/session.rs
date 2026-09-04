use std::{path::PathBuf, sync::Arc};

use engine::{DynamicColor, MousePointerShape, Terminal};
use winit::window::Window;

pub(crate) const MAX_TABS: usize = 16;

pub(crate) struct StoredTab {
    pub(crate) id: u64,
    pub(crate) index: usize,
    pub(crate) window: Arc<Window>,
    pub(crate) terminal: Terminal,
    pub(crate) title: String,
    pub(crate) pointer_shape: MousePointerShape,
    pub(crate) dynamic_colors: [Option<[u8; 3]>; 3],
    pub(crate) current_directory: Option<PathBuf>,
}

pub(crate) const fn dynamic_color_index(target: DynamicColor) -> usize {
    match target {
        DynamicColor::Foreground => 0,
        DynamicColor::Background => 1,
        DynamicColor::Cursor => 2,
    }
}

pub(crate) fn directory_from_osc7(uri: &str) -> Option<PathBuf> {
    let authority_and_path = uri.strip_prefix("file://")?;
    let path_start = authority_and_path.find('/')?;
    let authority = &authority_and_path[..path_start];
    if !authority.is_empty() && !authority.eq_ignore_ascii_case("localhost") {
        return None;
    }
    let encoded_path = &authority_and_path[path_start..];
    let decoded = percent_decode(encoded_path)?;
    if decoded.chars().any(char::is_control) {
        return None;
    }
    let path = PathBuf::from(decoded);
    path.is_absolute().then_some(path)
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = hex(*bytes.get(index + 1)?)?;
            let low = hex(*bytes.get(index + 2)?)?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

const fn hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::directory_from_osc7;

    #[test]
    fn osc7_file_urls_restore_absolute_utf8_working_directories() {
        assert_eq!(
            directory_from_osc7("file://localhost/Users/lasse/My%20Project"),
            Some(PathBuf::from("/Users/lasse/My Project"))
        );
        assert_eq!(
            directory_from_osc7("file:///tmp/%C3%A6ble"),
            Some(PathBuf::from("/tmp/æble"))
        );
    }

    #[test]
    fn osc7_parser_rejects_non_file_relative_and_malformed_paths() {
        assert_eq!(directory_from_osc7("https://example.com/tmp"), None);
        assert_eq!(directory_from_osc7("file://example.com/tmp"), None);
        assert_eq!(directory_from_osc7("file://localhost/ok%ZZ"), None);
        assert_eq!(directory_from_osc7("file:///tmp/%0Ahidden"), None);
        assert_eq!(directory_from_osc7("relative/path"), None);
    }
}
