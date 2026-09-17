use anyhow::{Context, Result};
use serde::Deserialize;

use crate::release_core::source::{ReleaseAsset, ReleasePayload, ReleaseSource};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubReleaseSource {
    repo: String,
}

impl GithubReleaseSource {
    pub fn new(repo: impl Into<String>) -> Self {
        Self { repo: repo.into() }
    }
}

impl ReleaseSource for GithubReleaseSource {
    fn fetch_latest_release(&self) -> Result<ReleasePayload> {
        let url = format!("https://api.github.com/repos/{}/releases/latest", self.repo);
        let response: GithubRelease = ureq::get(&url)
            .set("User-Agent", "Termy-Updater/1.0")
            .set("Accept", "application/vnd.github+json")
            .call()
            .with_context(|| {
                format!(
                    "Failed to fetch latest release from GitHub for {}",
                    self.repo
                )
            })?
            .into_json()
            .context("Failed to parse GitHub release JSON")?;

        Ok(ReleasePayload {
            tag_name: response.tag_name,
            release_url: response.html_url,
            assets: response
                .assets
                .into_iter()
                .map(|asset| ReleaseAsset {
                    name: asset.name,
                    download_url: asset.browser_download_url,
                })
                .collect(),
        })
    }
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}
