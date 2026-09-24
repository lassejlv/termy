use anyhow::{Context, Result};
use semver::Version;

use crate::release_core::source::ReleaseAsset;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionComparison {
    UpToDate,
    UpdateAvailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformKind {
    MacOs,
    Windows,
    Linux,
    Other,
}

pub fn compare_versions(current_version: &str, latest_version: &str) -> Result<VersionComparison> {
    let current = Version::parse(current_version)
        .with_context(|| format!("Invalid current version: {current_version}"))?;
    let latest = Version::parse(latest_version)
        .with_context(|| format!("Invalid latest version: {latest_version}"))?;

    if latest > current {
        Ok(VersionComparison::UpdateAvailable)
    } else {
        Ok(VersionComparison::UpToDate)
    }
}

pub fn normalize_release_version(tag_name: &str) -> String {
    tag_name.strip_prefix('v').unwrap_or(tag_name).to_string()
}

pub fn current_arch() -> &'static str {
    #[cfg(target_arch = "aarch64")]
    {
        "arm64"
    }
    #[cfg(target_arch = "x86_64")]
    {
        "x86_64"
    }
    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        "unknown"
    }
}

pub fn current_platform() -> PlatformKind {
    #[cfg(target_os = "macos")]
    {
        PlatformKind::MacOs
    }
    #[cfg(target_os = "windows")]
    {
        PlatformKind::Windows
    }
    #[cfg(target_os = "linux")]
    {
        PlatformKind::Linux
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        PlatformKind::Other
    }
}

pub fn select_platform_asset<'a>(
    assets: &'a [ReleaseAsset],
    platform: PlatformKind,
    arch: &str,
) -> Option<&'a ReleaseAsset> {
    match platform {
        PlatformKind::MacOs => find_macos_asset(assets, arch),
        PlatformKind::Windows => find_windows_asset(assets, arch),
        PlatformKind::Linux => find_linux_asset(assets, arch),
        PlatformKind::Other => None,
    }
}

pub fn select_checksum_asset<'a>(
    assets: &'a [ReleaseAsset],
    selected_asset_name: &str,
) -> Option<&'a ReleaseAsset> {
    let selected_checksum_name = format!("{selected_asset_name}.sha256");

    assets
        .iter()
        .find(|asset| asset.name.eq_ignore_ascii_case(&selected_checksum_name))
        .or_else(|| {
            assets
                .iter()
                .find(|asset| is_checksum_manifest_name(&asset.name))
        })
}

pub fn extension_for_asset_name(name: &str) -> String {
    if name.ends_with(".tar.gz") {
        "tar.gz".to_string()
    } else if name.ends_with(".dmg") {
        "dmg".to_string()
    } else if name.ends_with(".msi") {
        "msi".to_string()
    } else if name.ends_with(".exe") {
        "exe".to_string()
    } else {
        "bin".to_string()
    }
}

fn is_checksum_manifest_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "checksums.txt" | "sha256sums" | "sha256sums.txt" | "sha256sum.txt"
    )
}

fn find_macos_asset<'a>(assets: &'a [ReleaseAsset], arch: &str) -> Option<&'a ReleaseAsset> {
    let dmg_suffix = format!("{arch}.dmg");
    assets
        .iter()
        .find(|asset| asset.name.contains(arch) && asset.name.ends_with(".dmg"))
        .or_else(|| {
            assets
                .iter()
                .find(|asset| asset.name.ends_with(&dmg_suffix))
        })
        .or_else(|| assets.iter().find(|asset| asset.name.ends_with(".dmg")))
}

fn find_windows_asset<'a>(assets: &'a [ReleaseAsset], arch: &str) -> Option<&'a ReleaseAsset> {
    let win_arch = match arch {
        "x86_64" => "x64",
        "arm64" => "arm64",
        _ => arch,
    };

    assets
        .iter()
        .find(|asset| {
            (asset.name.contains(arch) || asset.name.contains(win_arch))
                && asset.name.ends_with(".msi")
        })
        .or_else(|| assets.iter().find(|asset| asset.name.ends_with(".msi")))
        .or_else(|| {
            assets.iter().find(|asset| {
                (asset.name.contains(arch) || asset.name.contains(win_arch))
                    && asset.name.ends_with(".exe")
            })
        })
        .or_else(|| assets.iter().find(|asset| asset.name.ends_with(".exe")))
}

fn find_linux_asset<'a>(assets: &'a [ReleaseAsset], arch: &str) -> Option<&'a ReleaseAsset> {
    let linux_arch = match arch {
        "arm64" => "aarch64",
        _ => arch,
    };

    assets
        .iter()
        .find(|asset| {
            asset.name.contains("linux")
                && (asset.name.contains(arch) || asset.name.contains(linux_arch))
                && asset.name.ends_with(".tar.gz")
        })
        .or_else(|| {
            assets
                .iter()
                .find(|asset| asset.name.contains("linux") && asset.name.ends_with(".tar.gz"))
        })
}

#[cfg(test)]
mod tests {
    use super::{
        PlatformKind, VersionComparison, compare_versions, extension_for_asset_name,
        select_checksum_asset, select_platform_asset,
    };
    use crate::release_core::source::ReleaseAsset;

    #[test]
    fn compare_versions_detects_update() {
        assert_eq!(
            compare_versions("0.1.0", "0.2.0").expect("version compare"),
            VersionComparison::UpdateAvailable
        );
        assert_eq!(
            compare_versions("0.2.0", "0.2.0").expect("version compare"),
            VersionComparison::UpToDate
        );
        assert_eq!(
            compare_versions("0.3.0", "0.2.0").expect("version compare"),
            VersionComparison::UpToDate
        );
    }

    #[test]
    fn extension_for_asset_name_supports_known_formats() {
        assert_eq!(extension_for_asset_name("foo.tar.gz"), "tar.gz");
        assert_eq!(extension_for_asset_name("foo.dmg"), "dmg");
        assert_eq!(extension_for_asset_name("foo.msi"), "msi");
        assert_eq!(extension_for_asset_name("foo.exe"), "exe");
        assert_eq!(extension_for_asset_name("foo.bin"), "bin");
    }

    #[test]
    fn selects_platform_assets_by_convention() {
        let assets = vec![
            asset("Termy-v1.0.0-macos-arm64.dmg"),
            asset("Termy-v1.0.0-macos-x86_64.dmg"),
            asset("Termy-v1.0.0-windows-x64.msi"),
            asset("Termy-v1.0.0-windows-arm64.exe"),
            asset("Termy-v1.0.0-linux-aarch64.tar.gz"),
            asset("Termy-v1.0.0-linux-x86_64.tar.gz"),
        ];

        assert_eq!(
            select_platform_asset(&assets, PlatformKind::MacOs, "arm64")
                .expect("macOS asset")
                .name,
            "Termy-v1.0.0-macos-arm64.dmg"
        );
        assert_eq!(
            select_platform_asset(&assets, PlatformKind::Windows, "x86_64")
                .expect("Windows asset")
                .name,
            "Termy-v1.0.0-windows-x64.msi"
        );
        assert_eq!(
            select_platform_asset(&assets, PlatformKind::Linux, "arm64")
                .expect("Linux asset")
                .name,
            "Termy-v1.0.0-linux-aarch64.tar.gz"
        );
        assert!(select_platform_asset(&assets, PlatformKind::Other, "x86_64").is_none());
    }

    #[test]
    fn selects_signed_macos_assets_and_checksum_manifest() {
        let assets = vec![
            asset("checksums.txt"),
            asset("Termy-v0.2.75-macos-x86_64-signed.dmg"),
            asset("Termy-v0.2.75-macos-arm64-signed.dmg"),
        ];

        for (arch, expected_name) in [
            ("arm64", "Termy-v0.2.75-macos-arm64-signed.dmg"),
            ("x86_64", "Termy-v0.2.75-macos-x86_64-signed.dmg"),
        ] {
            let selected = select_platform_asset(&assets, PlatformKind::MacOs, arch)
                .expect("signed macOS asset");
            assert_eq!(selected.name, expected_name);
            assert_eq!(extension_for_asset_name(&selected.name), "dmg");
            assert_eq!(
                select_checksum_asset(&assets, &selected.name)
                    .expect("checksum manifest")
                    .name,
                "checksums.txt"
            );
        }
    }

    #[test]
    fn selects_asset_specific_checksum_before_manifest() {
        let assets = vec![
            asset("checksums.txt"),
            asset("Termy-v1.0.0-windows-x64-Setup.exe.sha256"),
        ];

        assert_eq!(
            select_checksum_asset(&assets, "Termy-v1.0.0-windows-x64-Setup.exe")
                .expect("checksum asset")
                .name,
            "Termy-v1.0.0-windows-x64-Setup.exe.sha256"
        );
    }

    #[test]
    fn selects_checksum_manifest_when_asset_specific_checksum_is_missing() {
        let assets = vec![
            asset("Termy-v1.0.0-windows-x64-Setup.exe"),
            asset("checksums.txt"),
        ];

        assert_eq!(
            select_checksum_asset(&assets, "Termy-v1.0.0-windows-x64-Setup.exe")
                .expect("checksum manifest")
                .name,
            "checksums.txt"
        );
    }

    fn asset(name: &str) -> ReleaseAsset {
        ReleaseAsset {
            name: name.to_string(),
            download_url: format!("https://example.com/{name}"),
        }
    }
}
