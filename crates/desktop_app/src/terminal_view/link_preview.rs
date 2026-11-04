//! Text for the link-preview overlay shown while the link modifier is held
//! over a link.

/// Most characters the overlay shows before it elides the rest.
const LINK_PREVIEW_MAX_CHARS: usize = 80;

/// The overlay label for a hovered link target. A `file:///` URL shows as a
/// path, relative to `home` when it lies under it, and keeps its tail; any
/// other URL keeps its head.
///
/// The target comes from terminal output (a detected link or an OSC 8
/// hyperlink), so it can hold any text: the cuts count characters, never
/// bytes, and cannot land inside a multi-byte UTF-8 sequence.
pub(super) fn link_preview_label(target: &str, home: Option<&str>) -> String {
    let Some(path) = target.strip_prefix("file:///") else {
        return elide_end(target, LINK_PREVIEW_MAX_CHARS);
    };
    let path = format!("/{path}").replace("%20", " ");
    let path = match home.and_then(|home| path.strip_prefix(home)) {
        Some(relative) => format!("~{relative}"),
        None => path,
    };
    elide_start(&path, LINK_PREVIEW_MAX_CHARS)
}

/// `text` as its last `max_chars - 1` characters behind a leading `…` when it
/// has more than `max_chars` characters.
fn elide_start(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let skip = count - max_chars.saturating_sub(1);
    let start = text
        .char_indices()
        .nth(skip)
        .map_or(text.len(), |(index, _)| index);
    format!("…{}", &text[start..])
}

/// `text` as its first `max_chars - 1` characters before a trailing `…` when
/// it has more than `max_chars` characters.
fn elide_end(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut elided: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    elided.push('…');
    elided
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_multibyte_file_path_keeps_its_tail_on_a_char_boundary() {
        let name = "é".repeat(50) + &"日".repeat(50);
        let target = format!("file:///data/{name}");
        let path = format!("/data/{name}");
        // The old byte slice `&path[path.len() - 79..]` started mid-character.
        assert!(!path.is_char_boundary(path.len() - 79));

        let label = link_preview_label(&target, None);

        assert_eq!(label.chars().count(), LINK_PREVIEW_MAX_CHARS);
        assert!(label.starts_with('…'));
        assert!(label.ends_with(&"日".repeat(50)));
    }

    #[test]
    fn long_multibyte_url_keeps_its_head_on_a_char_boundary() {
        let target = format!("https://example.com/{}", "🦀".repeat(80));
        // The old byte slice `&url[..79]` ended mid-character.
        assert!(!target.is_char_boundary(79));

        let label = link_preview_label(&target, None);

        assert_eq!(label.chars().count(), LINK_PREVIEW_MAX_CHARS);
        assert!(label.starts_with("https://example.com/🦀"));
        assert!(label.ends_with("🦀…"));
    }

    #[test]
    fn short_targets_are_shown_whole() {
        let url = format!("https://example.com/{}", "a".repeat(60));
        assert_eq!(url.chars().count(), LINK_PREVIEW_MAX_CHARS);
        assert_eq!(link_preview_label(&url, None), url);
        assert_eq!(
            link_preview_label("file:///tmp/My%20Notes.txt", None),
            "/tmp/My Notes.txt"
        );
    }

    #[test]
    fn file_paths_under_home_show_relative_to_it() {
        assert_eq!(
            link_preview_label("file:///home/me/src/main.rs", Some("/home/me")),
            "~/src/main.rs"
        );
        assert_eq!(
            link_preview_label("file:///etc/hosts", Some("/home/me")),
            "/etc/hosts"
        );
    }
}
