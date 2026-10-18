//! Local theme files: `<themes dir>/<name>.json`. A file's theme id is its
//! normalized file name, so `My Theme.json` is the theme `my-theme`.

use super::{ThemeColors, normalize_theme_id, parse_theme_colors_json};
use std::path::{Path, PathBuf};

/// The theme files in `themes_dir` with their ids, in directory order.
fn theme_files(themes_dir: &Path) -> impl Iterator<Item = (String, PathBuf)> {
    std::fs::read_dir(themes_dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let is_json = path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("json"));
            if !is_json {
                return None;
            }
            let id = normalize_theme_id(path.file_stem()?.to_str()?);
            (!id.is_empty()).then_some((id, path))
        })
}

/// Ids of the theme files in `themes_dir`, sorted and without duplicates.
pub fn local_theme_ids(themes_dir: &Path) -> Vec<String> {
    let mut ids: Vec<String> = theme_files(themes_dir).map(|(id, _)| id).collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// The file for `theme_id` in `themes_dir`: `<id>.json` when it exists, else
/// any file whose name normalizes to the id.
pub fn local_theme_file(themes_dir: &Path, theme_id: &str) -> Option<PathBuf> {
    let id = normalize_theme_id(theme_id);
    if id.is_empty() {
        return None;
    }
    let exact = themes_dir.join(format!("{id}.json"));
    if exact.is_file() {
        return Some(exact);
    }
    theme_files(themes_dir)
        .find(|(file_id, _)| *file_id == id)
        .map(|(_, path)| path)
}

/// Colors of the local theme `theme_id`, when its file exists and parses.
pub fn load_local_theme(themes_dir: &Path, theme_id: &str) -> Option<ThemeColors> {
    let path = local_theme_file(themes_dir, theme_id)?;
    let contents = std::fs::read_to_string(path).ok()?;
    parse_theme_colors_json(&contents).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_theme(dir: &Path, file: &str) {
        let json = crate::themes::theme_colors_json_pretty(&crate::themes::nord(), None)
            .expect("theme json");
        std::fs::write(dir.join(file), json).expect("write theme");
    }

    #[test]
    fn local_themes_load_by_normalized_file_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_theme(dir.path(), "My Theme.JSON");
        write_theme(dir.path(), "plain.json");
        std::fs::write(dir.path().join("notes.txt"), "").expect("write notes");

        assert_eq!(local_theme_ids(dir.path()), ["my-theme", "plain"]);
        assert_eq!(
            load_local_theme(dir.path(), "my-theme"),
            Some(crate::themes::nord())
        );
        assert_eq!(
            load_local_theme(dir.path(), "My Theme"),
            Some(crate::themes::nord())
        );
        assert!(load_local_theme(dir.path(), "missing").is_none());
    }
}
