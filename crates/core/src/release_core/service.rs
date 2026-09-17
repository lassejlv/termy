use anyhow::{Context, Result};

use crate::release_core::DEFAULT_GITHUB_REPO;
use crate::release_core::policy::{
    VersionComparison, compare_versions, current_arch, current_platform, extension_for_asset_name,
    normalize_release_version, select_checksum_asset, select_platform_asset,
};
use crate::release_core::source::ReleaseSource;
use crate::release_core::transport::github::GithubReleaseSource;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseInfo {
    pub version: String,
    pub release_url: String,
    pub asset_name: String,
    pub download_url: String,
    pub checksum_asset_name: Option<String>,
    pub checksum_url: Option<String>,
    pub extension: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheck {
    UpToDate,
    UpdateAvailable(ReleaseInfo),
}

pub fn fetch_latest_release() -> Result<ReleaseInfo> {
    fetch_latest_release_for_repo(DEFAULT_GITHUB_REPO)
}

pub fn fetch_latest_release_for_repo(repo: &str) -> Result<ReleaseInfo> {
    let source = GithubReleaseSource::new(repo);
    fetch_latest_release_with_source(&source)
}

pub fn fetch_latest_release_with_source(source: &impl ReleaseSource) -> Result<ReleaseInfo> {
    let payload = source.fetch_latest_release()?;
    let version = normalize_release_version(&payload.tag_name);

    let arch = current_arch();
    let asset = select_platform_asset(&payload.assets, current_platform(), arch)
        .with_context(|| format!("No installer asset found for this platform (arch: '{arch}')"))?;
    let checksum_asset = select_checksum_asset(&payload.assets, &asset.name);

    Ok(ReleaseInfo {
        version,
        release_url: payload.release_url,
        asset_name: asset.name.clone(),
        download_url: asset.download_url.clone(),
        checksum_asset_name: checksum_asset.map(|asset| asset.name.clone()),
        checksum_url: checksum_asset.map(|asset| asset.download_url.clone()),
        extension: extension_for_asset_name(&asset.name),
    })
}

pub fn check_for_updates(current_version: &str) -> Result<UpdateCheck> {
    let latest = fetch_latest_release()?;
    check_for_updates_with_release(current_version, latest)
}

pub fn check_for_updates_for_repo(current_version: &str, repo: &str) -> Result<UpdateCheck> {
    let latest = fetch_latest_release_for_repo(repo)?;
    check_for_updates_with_release(current_version, latest)
}

pub fn check_for_updates_with_source(
    current_version: &str,
    source: &impl ReleaseSource,
) -> Result<UpdateCheck> {
    let latest = fetch_latest_release_with_source(source)?;
    check_for_updates_with_release(current_version, latest)
}

pub fn check_for_updates_with_release(
    current_version: &str,
    latest_release: ReleaseInfo,
) -> Result<UpdateCheck> {
    match compare_versions(current_version, &latest_release.version)? {
        VersionComparison::UpToDate => Ok(UpdateCheck::UpToDate),
        VersionComparison::UpdateAvailable => Ok(UpdateCheck::UpdateAvailable(latest_release)),
    }
}

#[cfg(test)]
mod tests {
    use super::{UpdateCheck, check_for_updates_with_source};
    use crate::release_core::source::{ReleaseAsset, ReleasePayload, ReleaseSource};
    use anyhow::Result;

    struct StaticSource {
        payload: ReleasePayload,
    }

    impl ReleaseSource for StaticSource {
        fn fetch_latest_release(&self) -> Result<ReleasePayload> {
            Ok(self.payload.clone())
        }
    }

    #[test]
    fn reports_update_available_when_latest_is_newer() {
        let source = StaticSource {
            payload: fixture_payload("v2.0.0"),
        };

        let result = check_for_updates_with_source("1.0.0", &source).expect("update check");
        assert!(matches!(result, UpdateCheck::UpdateAvailable(_)));
    }

    #[test]
    fn reports_up_to_date_when_versions_match() {
        let source = StaticSource {
            payload: fixture_payload("v1.0.0"),
        };

        let result = check_for_updates_with_source("1.0.0", &source).expect("update check");
        assert_eq!(result, UpdateCheck::UpToDate);
    }

    fn fixture_payload(tag_name: &str) -> ReleasePayload {
        ReleasePayload {
            tag_name: tag_name.to_string(),
            release_url: "https://example.com/release".to_string(),
            assets: vec![
                asset("Termy-v1.0.0-macos-arm64.dmg"),
                asset("Termy-v1.0.0-macos-x86_64.dmg"),
                asset("Termy-v1.0.0-windows-x64.msi"),
                asset("Termy-v1.0.0-linux-x86_64.tar.gz"),
                asset("checksums.txt"),
            ],
        }
    }

    fn asset(name: &str) -> ReleaseAsset {
        ReleaseAsset {
            name: name.to_string(),
            download_url: format!("https://example.com/{name}"),
        }
    }
}
