use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::process::Command;

const ALLOWED_LICENSES: &[&str] = &[
    "(Apache-2.0 OR MIT) AND BSD-3-Clause",
    "(MIT OR Apache-2.0) AND NCSA",
    "(MIT OR Apache-2.0) AND Unicode-3.0",
    "0BSD OR MIT OR Apache-2.0",
    "Apache-2.0 AND ISC",
    "Apache-2.0 AND MIT",
    "Apache-2.0 / MIT",
    "Apache-2.0 OR BSL-1.0 OR MIT",
    "Apache-2.0 OR BSL-1.0",
    "Apache-2.0 OR GPL-2.0-only",
    "Apache-2.0 OR ISC OR MIT",
    "Apache-2.0 OR MIT",
    "Apache-2.0 WITH LLVM-exception",
    "Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT",
    "Apache-2.0",
    "Apache-2.0/MIT",
    "BSD-2-Clause OR Apache-2.0 OR MIT",
    "BSD-2-Clause OR MIT OR Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause OR Apache-2.0",
    "BSD-3-Clause OR MIT OR Apache-2.0",
    "BSD-3-Clause",
    "bzip2-1.0.6",
    "CC0-1.0 OR Apache-2.0",
    "CC0-1.0 OR MIT-0 OR Apache-2.0",
    "CC0-1.0",
    "CDLA-Permissive-2.0",
    "ISC AND (Apache-2.0 OR ISC)",
    "ISC AND (Apache-2.0 OR ISC) AND Apache-2.0 AND MIT AND BSD-3-Clause AND (Apache-2.0 OR ISC OR MIT) AND (Apache-2.0 OR ISC OR MIT-0)",
    "ISC",
    "MIT / Apache-2.0",
    "MIT AND BSD-3-Clause",
    "MIT OR Apache-2.0 OR LGPL-2.1-or-later",
    "MIT OR Apache-2.0 OR Zlib",
    "MIT OR Apache-2.0",
    "MIT OR Zlib OR Apache-2.0",
    "MIT",
    "MIT/Apache-2.0",
    "MPL-2.0",
    "Unicode-3.0",
    "Unlicense OR MIT",
    "Unlicense/MIT",
    "Zlib OR Apache-2.0 OR MIT",
    "Zlib",
];

const REVIEWED_LICENSE_EXCEPTIONS: &[LicenseException] = &[
    LicenseException {
        name: "clipboard-win",
        version: "5.4.1",
        license: "BSL-1.0",
        source_contains: "crates.io-index",
        reason: "permissive Boost license; Windows backend for clipboard-rs",
    },
    LicenseException {
        name: "error-code",
        version: "3.4.0",
        license: "BSL-1.0",
        source_contains: "crates.io-index",
        reason: "permissive Boost license; clipboard-win error adapter",
    },
    LicenseException {
        name: "windows-win",
        version: "3.0.0",
        license: "BSL-1.0",
        source_contains: "crates.io-index",
        reason: "permissive Boost license; clipboard-win monitor dependency",
    },
];

pub fn run() -> Result<()> {
    let metadata = cargo_metadata()?;
    let mut checked_packages = 0usize;
    let mut exception_count = 0usize;
    let mut failures = Vec::new();

    for package in metadata
        .packages
        .iter()
        .filter(|package| package.source.is_some())
    {
        checked_packages += 1;
        match dependency_license_decision(package) {
            LicenseDecision::Allowed => {}
            LicenseDecision::ReviewedException(exception) => {
                exception_count += 1;
                println!(
                    "license exception: {} {} ({})",
                    package.name, package.version, exception.reason
                );
            }
            LicenseDecision::Denied(reason) => {
                failures.push(format!("{} {}: {}", package.name, package.version, reason));
            }
        }
    }

    if !failures.is_empty() {
        bail!(
            "dependency policy failed:\n{}",
            failures
                .iter()
                .map(|failure| format!("- {failure}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    println!(
        "dependency policy passed ({checked_packages} third-party packages, {exception_count} reviewed exception(s))"
    );
    Ok(())
}

fn cargo_metadata() -> Result<CargoMetadata> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--locked"])
        .current_dir(crate::workspace_root()?)
        .output()
        .context("failed to run cargo metadata")?;

    if !output.status.success() {
        bail!(
            "cargo metadata failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    serde_json::from_slice(&output.stdout).context("failed to parse cargo metadata")
}

fn dependency_license_decision(package: &PackageMetadata) -> LicenseDecision<'_> {
    let Some(license) = package.license.as_deref() else {
        return if package.license_file.is_some() {
            LicenseDecision::Allowed
        } else {
            LicenseDecision::Denied("missing license metadata".to_string())
        };
    };

    if ALLOWED_LICENSES.contains(&license) {
        return LicenseDecision::Allowed;
    }

    if let Some(exception) = REVIEWED_LICENSE_EXCEPTIONS
        .iter()
        .find(|exception| exception.matches(package, license))
    {
        return LicenseDecision::ReviewedException(exception);
    }

    LicenseDecision::Denied(format!("unreviewed license expression `{license}`"))
}

#[derive(Debug, Deserialize)]
struct CargoMetadata {
    packages: Vec<PackageMetadata>,
}

#[derive(Debug, Deserialize)]
struct PackageMetadata {
    name: String,
    version: String,
    license: Option<String>,
    license_file: Option<String>,
    source: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum LicenseDecision<'a> {
    Allowed,
    ReviewedException(&'a LicenseException),
    Denied(String),
}

#[derive(Debug, PartialEq, Eq)]
struct LicenseException {
    name: &'static str,
    version: &'static str,
    license: &'static str,
    source_contains: &'static str,
    reason: &'static str,
}

impl LicenseException {
    fn matches(&self, package: &PackageMetadata, license: &str) -> bool {
        package.name == self.name
            && package.version == self.version
            && package
                .source
                .as_deref()
                .is_some_and(|source| source.contains(self.source_contains))
            && license == self.license
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(name: &str, version: &str, license: Option<&str>, source: &str) -> PackageMetadata {
        PackageMetadata {
            name: name.to_string(),
            version: version.to_string(),
            license: license.map(str::to_string),
            license_file: None,
            source: Some(source.to_string()),
        }
    }

    #[test]
    fn dependency_policy_allows_known_permissive_expression() {
        let package = package(
            "serde",
            "1.0.0",
            Some("MIT OR Apache-2.0"),
            "registry+https://github.com/rust-lang/crates.io-index",
        );

        assert_eq!(
            dependency_license_decision(&package),
            LicenseDecision::Allowed
        );
    }

    #[test]
    fn dependency_policy_allows_license_file_metadata() {
        let package = PackageMetadata {
            name: "custom".to_string(),
            version: "1.0.0".to_string(),
            license: None,
            license_file: Some("LICENSE".to_string()),
            source: Some("registry+https://github.com/rust-lang/crates.io-index".to_string()),
        };

        assert_eq!(
            dependency_license_decision(&package),
            LicenseDecision::Allowed
        );
    }

    #[test]
    fn dependency_policy_requires_license_metadata() {
        let package = package(
            "unknown",
            "1.0.0",
            None,
            "registry+https://github.com/rust-lang/crates.io-index",
        );

        assert!(matches!(
            dependency_license_decision(&package),
            LicenseDecision::Denied(_)
        ));
    }

    #[test]
    fn dependency_policy_denies_unreviewed_copyleft_expression() {
        let package = package(
            "new-gpl-crate",
            "1.0.0",
            Some("GPL-3.0-or-later"),
            "registry+https://github.com/rust-lang/crates.io-index",
        );

        assert!(matches!(
            dependency_license_decision(&package),
            LicenseDecision::Denied(_)
        ));
    }
}
