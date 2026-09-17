//! Fetch GitHub release notes over HTTP and remember which version was shown.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use image::RgbaImage;
use termy_core::release_core::{self, ReleaseNotes, ReleaseSummary};

const GITHUB_USER_AGENT: &str = "Termy-Updater/1.0";
const SEEN_RELEASE_NOTES_ENV: &str = "TERMY_SEEN_RELEASE_NOTES_PATH";
const MAX_MARKDOWN_IMAGE_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhatsNewAction {
    None,
    Seed { version: String },
    Open { version: String },
}

pub fn fetch_release_notes(version: &str) -> Result<ReleaseNotes, String> {
    release_core::fetch_release_notes(version).map_err(user_facing_error)
}

pub fn fetch_release_list() -> Result<Vec<ReleaseSummary>, String> {
    release_core::fetch_release_list().map_err(user_facing_error)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseListRow {
    pub tag: String,
    pub title: String,
    pub keywords: String,
    pub status_hint: Option<String>,
}

pub fn release_list_rows(
    releases: &[ReleaseSummary],
    current_version: &str,
) -> Vec<ReleaseListRow> {
    let current = canonical_release_version(current_version);
    let latest_tag = releases
        .iter()
        .find(|release| !release.prerelease)
        .map(|release| canonical_release_version(&release.tag));

    releases
        .iter()
        .map(|release| {
            let canonical = canonical_release_version(&release.tag);
            let is_latest = latest_tag.as_deref() == Some(canonical.as_str());
            let is_installed = !current.is_empty() && canonical == current;
            let status_hint = release_status_hint(is_latest, is_installed, release.prerelease);
            let mut keywords = format!("release notes changelog {} {}", release.title, release.tag);
            if is_latest {
                keywords.push_str(" latest");
            }
            if is_installed {
                keywords.push_str(" installed current");
            }
            if release.prerelease {
                keywords.push_str(" prerelease pre-release");
            } else if !is_latest {
                keywords.push_str(" past");
            }
            ReleaseListRow {
                tag: release.tag.clone(),
                title: release.title.clone(),
                keywords,
                status_hint,
            }
        })
        .collect()
}

fn release_status_hint(is_latest: bool, is_installed: bool, prerelease: bool) -> Option<String> {
    let mut parts = Vec::new();
    if is_latest {
        parts.push("Latest");
    }
    if is_installed {
        parts.push("Installed");
    }
    if prerelease {
        parts.push("Pre-release");
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" · "))
    }
}

pub fn canonical_release_version(version: &str) -> String {
    version
        .trim()
        .trim_start_matches(['v', 'V'])
        .trim()
        .to_string()
}

pub fn whats_new_on_launch(current: &str, seen: Option<&str>) -> WhatsNewAction {
    let current = canonical_release_version(current);
    if current.is_empty() {
        return WhatsNewAction::None;
    }
    match seen.map(str::trim).filter(|seen| !seen.is_empty()) {
        None => WhatsNewAction::Seed { version: current },
        Some(seen) if canonical_release_version(seen) == current => WhatsNewAction::None,
        Some(_) => WhatsNewAction::Open { version: current },
    }
}

#[cfg(not(test))]
pub fn load_seen_release_notes_version() -> Option<String> {
    load_seen_version_from(&seen_release_notes_path()?)
}

#[cfg(not(test))]
pub fn store_seen_release_notes_version(version: &str) {
    let Some(path) = seen_release_notes_path() else {
        return;
    };
    let _ = store_seen_version_to(&path, version);
}

pub fn fetch_markdown_image_rgba(url: &str) -> Result<RgbaImage, String> {
    if !crate::ui::markdown::is_http_url(url) {
        return Err("Only http(s) images can be loaded.".to_string());
    }

    let response = ureq::get(url)
        .set("User-Agent", GITHUB_USER_AGENT)
        .call()
        .map_err(user_facing_error)?;
    let mut reader = response.into_reader().take(MAX_MARKDOWN_IMAGE_BYTES + 1);
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not read image: {error}"))?;
    if bytes.len() as u64 > MAX_MARKDOWN_IMAGE_BYTES {
        return Err("Image is larger than 5 MB.".to_string());
    }
    image::load_from_memory(&bytes)
        .map(|decoded| decoded.to_rgba8())
        .map_err(|error| format!("Could not decode image: {error}"))
}

fn user_facing_error(error: impl std::fmt::Display) -> String {
    truncate_message(&error.to_string())
}

#[cfg(not(test))]
fn seen_release_notes_path() -> Option<PathBuf> {
    seen_release_notes_path_with(|name| std::env::var(name).ok(), dirs::data_local_dir())
}

fn seen_release_notes_path_with(
    get_var: impl Fn(&str) -> Option<String>,
    data_local_dir: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(path) = get_var(SEEN_RELEASE_NOTES_ENV).filter(|path| !path.trim().is_empty()) {
        return Some(PathBuf::from(path));
    }
    Some(
        data_local_dir?
            .join("termy")
            .join("seen_release_notes_version"),
    )
}

fn load_seen_version_from(path: &Path) -> Option<String> {
    let value = fs::read_to_string(path).ok()?;
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn store_seen_version_to(path: &Path, version: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, format!("{}\n", canonical_release_version(version)))
}

fn truncate_message(message: &str) -> String {
    const LIMIT: usize = 240;
    if message.len() <= LIMIT {
        message.to_string()
    } else {
        format!("{}…", &message[..LIMIT])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_launch_seeds_without_opening() {
        assert_eq!(
            whats_new_on_launch("0.2.71", None),
            WhatsNewAction::Seed {
                version: "0.2.71".to_string()
            }
        );
    }

    #[test]
    fn matching_seen_version_stays_quiet() {
        assert_eq!(
            whats_new_on_launch("v0.2.71", Some("0.2.71")),
            WhatsNewAction::None
        );
    }

    #[test]
    fn upgraded_version_opens_notes() {
        assert_eq!(
            whats_new_on_launch("0.2.72", Some("0.2.71")),
            WhatsNewAction::Open {
                version: "0.2.72".to_string()
            }
        );
    }

    #[test]
    fn seen_release_notes_path_prefers_env_override() {
        let path = seen_release_notes_path_with(
            |name| (name == SEEN_RELEASE_NOTES_ENV).then(|| "/tmp/seen".to_string()),
            Some(PathBuf::from("/tmp/data")),
        );
        assert_eq!(path, Some(PathBuf::from("/tmp/seen")));
    }

    #[test]
    fn stores_and_loads_seen_version() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("seen");
        store_seen_version_to(&path, "v1.2.3").expect("store");
        assert_eq!(load_seen_version_from(&path).as_deref(), Some("1.2.3"));
    }

    #[test]
    fn release_list_rows_mark_latest_installed_and_prerelease() {
        let rows = release_list_rows(
            &[
                ReleaseSummary {
                    tag: "v1.3.0-rc.1".to_string(),
                    title: "1.3.0-rc.1".to_string(),
                    prerelease: true,
                },
                ReleaseSummary {
                    tag: "v1.2.3".to_string(),
                    title: "Termy 1.2.3".to_string(),
                    prerelease: false,
                },
                ReleaseSummary {
                    tag: "v1.2.2".to_string(),
                    title: "1.2.2".to_string(),
                    prerelease: false,
                },
            ],
            "1.2.2",
        );

        assert_eq!(rows[0].status_hint.as_deref(), Some("Pre-release"));
        assert_eq!(rows[1].title, "Termy 1.2.3");
        assert_eq!(rows[1].status_hint.as_deref(), Some("Latest"));
        assert_eq!(rows[2].status_hint.as_deref(), Some("Installed"));
        assert!(rows[1].keywords.contains("latest"));
        assert!(rows[2].keywords.contains("past"));
        assert!(!rows[1].keywords.contains("past"));
    }

    #[test]
    fn release_list_rows_combine_latest_and_installed() {
        let rows = release_list_rows(
            &[ReleaseSummary {
                tag: "v0.2.71".to_string(),
                title: "0.2.71".to_string(),
                prerelease: false,
            }],
            "v0.2.71",
        );
        assert_eq!(rows[0].status_hint.as_deref(), Some("Latest · Installed"));
    }
}
