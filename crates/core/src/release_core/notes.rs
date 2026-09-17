use anyhow::{Context, Result};
use serde::Deserialize;

use crate::release_core::DEFAULT_GITHUB_REPO;

const GITHUB_USER_AGENT: &str = "Termy-Updater/1.0";
const RELEASE_LIST_PAGE_SIZE: u8 = 30;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseNotes {
    pub tag: String,
    pub title: String,
    pub markdown: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseSummary {
    pub tag: String,
    pub title: String,
    pub prerelease: bool,
}

pub fn fetch_release_notes(version: &str) -> Result<ReleaseNotes> {
    fetch_release_notes_for_repo(DEFAULT_GITHUB_REPO, version)
}

pub fn fetch_release_list() -> Result<Vec<ReleaseSummary>> {
    fetch_release_list_for_repo(DEFAULT_GITHUB_REPO)
}

pub fn fetch_release_list_for_repo(repo: &str) -> Result<Vec<ReleaseSummary>> {
    fetch_release_list_with(repo, github_get)
}

pub(crate) fn fetch_release_list_with(
    repo: &str,
    get: impl FnOnce(&str) -> Result<String>,
) -> Result<Vec<ReleaseSummary>> {
    let url =
        format!("https://api.github.com/repos/{repo}/releases?per_page={RELEASE_LIST_PAGE_SIZE}");
    parse_release_list_json(&get(&url)?)
}

pub fn fetch_release_notes_for_repo(repo: &str, version: &str) -> Result<ReleaseNotes> {
    fetch_release_notes_with(repo, version, github_get)
}

pub fn release_tag_candidates(version: &str) -> Vec<String> {
    let trimmed = version.trim();
    let stripped = trimmed.trim_start_matches(['v', 'V']).trim().to_string();
    let with_v = format!("v{stripped}");
    let mut tags = Vec::new();
    if trimmed.starts_with('v') || trimmed.starts_with('V') {
        push_unique(&mut tags, trimmed.to_string());
        push_unique(&mut tags, stripped);
    } else {
        push_unique(&mut tags, with_v);
        push_unique(&mut tags, stripped);
    }
    tags
}

pub(crate) fn fetch_release_notes_with(
    repo: &str,
    version: &str,
    mut get: impl FnMut(&str) -> Result<String>,
) -> Result<ReleaseNotes> {
    let mut last_error = anyhow::anyhow!("GitHub could not load this release.");
    for tag in release_tag_candidates(version) {
        let url = format!("https://api.github.com/repos/{repo}/releases/tags/{tag}");
        match get(&url) {
            Ok(body) => return parse_release_json(&body, &tag),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

fn github_get(url: &str) -> Result<String> {
    let response = ureq::get(url)
        .set("User-Agent", GITHUB_USER_AGENT)
        .set("Accept", "application/vnd.github+json")
        .set("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .with_context(|| format!("Failed to fetch GitHub release from {url}"))?;
    response
        .into_string()
        .context("Failed to read GitHub release JSON")
}

#[derive(Debug, Deserialize)]
struct GithubReleasePayload {
    body: Option<String>,
    name: Option<String>,
    #[serde(alias = "tagName")]
    tag_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubReleaseListItem {
    name: Option<String>,
    #[serde(alias = "tagName")]
    tag_name: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

fn parse_release_list_json(stdout: &str) -> Result<Vec<ReleaseSummary>> {
    let payload: Vec<GithubReleaseListItem> = serde_json::from_str(stdout.trim())
        .context("GitHub returned a response that was not a release list.")?;
    Ok(payload
        .into_iter()
        .filter(|item| !item.draft)
        .filter_map(|item| {
            let tag = item.tag_name.filter(|tag| !tag.trim().is_empty())?;
            let title = item
                .name
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| tag.clone());
            Some(ReleaseSummary {
                tag,
                title,
                prerelease: item.prerelease,
            })
        })
        .collect())
}

fn parse_release_json(stdout: &str, fallback_tag: &str) -> Result<ReleaseNotes> {
    let payload: GithubReleasePayload = serde_json::from_str(stdout.trim())
        .context("GitHub returned a response that was not release JSON.")?;
    let tag = payload
        .tag_name
        .filter(|tag| !tag.trim().is_empty())
        .unwrap_or_else(|| fallback_tag.to_string());
    let title = payload
        .name
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| tag.clone());
    Ok(ReleaseNotes {
        tag,
        title,
        markdown: payload.body.unwrap_or_default(),
    })
}

fn push_unique(tags: &mut Vec<String>, tag: String) {
    if !tag.is_empty() && !tags.iter().any(|existing| existing == &tag) {
        tags.push(tag);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ReleaseNotes, ReleaseSummary, fetch_release_list_with, fetch_release_notes_with,
        parse_release_json, parse_release_list_json, release_tag_candidates,
    };
    use anyhow::anyhow;

    #[test]
    fn release_tag_candidates_try_v_prefix_first() {
        assert_eq!(
            release_tag_candidates("1.2.3"),
            vec!["v1.2.3".to_string(), "1.2.3".to_string()]
        );
        assert_eq!(
            release_tag_candidates("v1.2.3"),
            vec!["v1.2.3".to_string(), "1.2.3".to_string()]
        );
    }

    #[test]
    fn parses_github_cli_and_api_payloads() {
        let from_cli = parse_release_json(
            r#"{"tagName":"v1.2.3","name":"1.2.3","body":"hello"}"#,
            "v1.2.3",
        )
        .expect("cli payload");
        assert_eq!(
            from_cli,
            ReleaseNotes {
                tag: "v1.2.3".to_string(),
                title: "1.2.3".to_string(),
                markdown: "hello".to_string(),
            }
        );

        let from_api = parse_release_json(
            r#"{"tag_name":"v1.2.3","name":"Termy 1.2.3","body":"hello from api"}"#,
            "v1.2.3",
        )
        .expect("api payload");
        assert_eq!(from_api.markdown, "hello from api");
        assert_eq!(from_api.title, "Termy 1.2.3");
    }

    #[test]
    fn tries_fallback_tag_when_first_request_fails() {
        let mut urls = Vec::new();
        let notes = fetch_release_notes_with("lassejlv/termy", "1.2.3", |url| {
            urls.push(url.to_string());
            if url.ends_with("/v1.2.3") {
                Err(anyhow!("missing"))
            } else {
                Ok(r#"{"tag_name":"1.2.3","name":"1.2.3","body":"fallback"}"#.to_string())
            }
        })
        .expect("notes");

        assert_eq!(notes.markdown, "fallback");
        assert_eq!(
            urls,
            vec![
                "https://api.github.com/repos/lassejlv/termy/releases/tags/v1.2.3".to_string(),
                "https://api.github.com/repos/lassejlv/termy/releases/tags/1.2.3".to_string(),
            ]
        );
    }

    #[test]
    fn empty_body_is_valid() {
        let notes = parse_release_json(r#"{"tag_name":"v1.0.0"}"#, "v1.0.0").expect("notes");
        assert_eq!(notes.title, "v1.0.0");
        assert!(notes.markdown.is_empty());
    }

    #[test]
    fn lists_published_releases_and_skips_drafts() {
        let releases = parse_release_list_json(
            r#"[
                {"tag_name":"v2.0.0","name":"Termy 2.0.0","prerelease":false},
                {"tag_name":"v2.0.0-rc.1","name":"","prerelease":true},
                {"tag_name":"v1.9.0","name":"1.9.0","draft":true},
                {"tag_name":"","name":"ignored"}
            ]"#,
        )
        .expect("list");

        assert_eq!(
            releases,
            vec![
                ReleaseSummary {
                    tag: "v2.0.0".to_string(),
                    title: "Termy 2.0.0".to_string(),
                    prerelease: false,
                },
                ReleaseSummary {
                    tag: "v2.0.0-rc.1".to_string(),
                    title: "v2.0.0-rc.1".to_string(),
                    prerelease: true,
                },
            ]
        );
    }

    #[test]
    fn fetch_release_list_requests_github_collection_url() {
        let mut urls = Vec::new();
        let releases = fetch_release_list_with("lassejlv/termy", |url| {
            urls.push(url.to_string());
            Ok(r#"[{"tag_name":"v1.0.0","name":"1.0.0"}]"#.to_string())
        })
        .expect("list");

        assert_eq!(releases[0].tag, "v1.0.0");
        assert_eq!(
            urls,
            vec!["https://api.github.com/repos/lassejlv/termy/releases?per_page=30".to_string()]
        );
    }
}
