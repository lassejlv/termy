use std::{
    fs,
    path::{Path, PathBuf},
};

use semver::Version;
use sha2::{Digest, Sha256};
use termy_core::theme_core::{
    ThemeMetadata, ThemeMetadataVersion, ThemeRegistryEntry, ThemeRegistryIndex,
    normalize_theme_id, parse_theme_colors_json, theme_colors_json_pretty,
};

const THEME_SCHEMA_REF: &str = "../../../schemas/theme.schema.json";
const METADATA_SCHEMA_REF: &str = "../../schemas/theme-metadata.schema.json";

pub fn export_theme(
    repo: PathBuf,
    slug: String,
    name: String,
    version: String,
    description: String,
    force: bool,
) {
    match export_theme_impl(&repo, &slug, &name, &version, &description, force) {
        Ok(()) => {
            println!("Theme exported to {}", repo.display());
            println!(
                "Run `termy-cli -validate-theme-repo --repo {}` before opening a PR.",
                repo.display()
            );
        }
        Err(error) => {
            eprintln!("Failed to export theme: {error}");
            std::process::exit(1);
        }
    }
}

pub fn validate_theme_repo(repo: PathBuf) {
    match validate_theme_repo_impl(&repo) {
        Ok(()) => println!("Theme repo is valid: {}", repo.display()),
        Err(errors) => {
            eprintln!("Theme repo is invalid: {}", repo.display());
            for error in errors {
                eprintln!("  {error}");
            }
            std::process::exit(1);
        }
    }
}

fn export_theme_impl(
    repo: &Path,
    raw_slug: &str,
    name: &str,
    version: &str,
    description: &str,
    force: bool,
) -> Result<(), String> {
    let slug = normalize_theme_id(raw_slug);
    if slug.is_empty() {
        return Err("slug must contain at least one ASCII letter or digit".to_string());
    }
    if name.trim().is_empty() {
        return Err("name is required".to_string());
    }
    Version::parse(version)
        .map_err(|error| format!("version must be valid semver like 1.0.0: {error}"))?;

    let theme_dir = repo.join("themes").join(&slug);
    let files_dir = theme_dir.join("files");
    fs::create_dir_all(&files_dir)
        .map_err(|error| format!("failed to create theme directory: {error}"))?;

    let file_path = files_dir.join(format!("{version}.json"));
    if file_path.exists() && !force {
        return Err(format!(
            "{} already exists; pass --force to overwrite it",
            file_path.display()
        ));
    }

    let colors = super::providers::active_theme_colors()?;
    let theme_json = theme_colors_json_pretty(&colors, Some(THEME_SCHEMA_REF))?;
    parse_theme_colors_json(&theme_json)?;
    fs::write(&file_path, &theme_json)
        .map_err(|error| format!("failed to write theme file: {error}"))?;

    let checksum = sha256_hex(theme_json.as_bytes());
    let metadata_path = theme_dir.join("metadata.json");
    let mut metadata = if metadata_path.exists() {
        let contents = fs::read_to_string(&metadata_path)
            .map_err(|error| format!("failed to read metadata.json: {error}"))?;
        serde_json::from_str::<ThemeMetadata>(&contents)
            .map_err(|error| format!("failed to parse metadata.json: {error}"))?
    } else {
        ThemeMetadata {
            schema: Some(METADATA_SCHEMA_REF.to_string()),
            name: name.trim().to_string(),
            slug: slug.clone(),
            description: description.to_string(),
            latest_version: version.to_string(),
            versions: Vec::new(),
        }
    };

    metadata.schema = Some(METADATA_SCHEMA_REF.to_string());
    metadata.name = name.trim().to_string();
    metadata.slug = slug;
    metadata.description = description.to_string();
    upsert_version(
        &mut metadata.versions,
        ThemeMetadataVersion {
            version: version.to_string(),
            file: format!("files/{version}.json"),
            changelog: None,
            checksum_sha256: Some(checksum),
        },
    );
    metadata.latest_version = latest_version(&metadata.versions)?;

    let metadata_json = serde_json::to_string_pretty(&metadata)
        .map_err(|error| format!("failed to serialize metadata.json: {error}"))?;
    fs::write(&metadata_path, format!("{metadata_json}\n"))
        .map_err(|error| format!("failed to write metadata.json: {error}"))?;

    regenerate_index(repo)?;
    validate_theme_repo_impl(repo).map_err(|errors| errors.join("; "))?;
    Ok(())
}

fn validate_theme_repo_impl(repo: &Path) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    let Some(generated_index) = build_index(repo, &mut errors) else {
        return Err(errors);
    };

    let index_path = repo.join("index.json");
    match fs::read_to_string(&index_path) {
        Ok(contents) => match serde_json::from_str::<ThemeRegistryIndex>(&contents) {
            Ok(index) if index == generated_index => {}
            Ok(_) => {
                errors.push("index.json is stale; regenerate it with -export-theme".to_string());
            }
            Err(error) => errors.push(format!("index.json is invalid JSON: {error}")),
        },
        Err(error) => errors.push(format!("failed to read index.json: {error}")),
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn regenerate_index(repo: &Path) -> Result<(), String> {
    let mut errors = Vec::new();
    let index = build_index(repo, &mut errors).ok_or_else(|| errors.join("; "))?;
    if !errors.is_empty() {
        return Err(errors.join("; "));
    }
    let index_json = serde_json::to_string_pretty(&index)
        .map_err(|error| format!("failed to serialize index.json: {error}"))?;
    fs::write(repo.join("index.json"), format!("{index_json}\n"))
        .map_err(|error| format!("failed to write index.json: {error}"))
}

fn build_index(repo: &Path, errors: &mut Vec<String>) -> Option<ThemeRegistryIndex> {
    let themes_dir = repo.join("themes");
    let entries = match fs::read_dir(&themes_dir) {
        Ok(entries) => entries,
        Err(error) => {
            errors.push(format!("failed to read themes directory: {error}"));
            return None;
        }
    };

    let mut themes = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let dir_slug = entry.file_name().to_string_lossy().to_string();
        let metadata_path = path.join("metadata.json");
        let Some(metadata) = read_metadata(&metadata_path, errors) else {
            continue;
        };
        validate_metadata(repo, &dir_slug, &metadata, errors);
        if let Some(version) = metadata
            .versions
            .iter()
            .find(|version| version.version == metadata.latest_version)
        {
            themes.push(ThemeRegistryEntry {
                name: metadata.name,
                slug: metadata.slug.clone(),
                description: metadata.description,
                latest_version: metadata.latest_version,
                file: format!(
                    "themes/{}/{}",
                    metadata.slug,
                    version.file.trim_start_matches('/')
                ),
                checksum_sha256: version.checksum_sha256.clone(),
            });
        } else {
            errors.push(format!(
                "themes/{dir_slug}/metadata.json latestVersion does not exist in versions"
            ));
        }
    }

    themes.sort_unstable_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
            .then_with(|| left.slug.cmp(&right.slug))
    });

    Some(ThemeRegistryIndex { version: 1, themes })
}

fn read_metadata(path: &Path, errors: &mut Vec<String>) -> Option<ThemeMetadata> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) => {
            errors.push(format!("failed to read {}: {error}", path.display()));
            return None;
        }
    };
    match serde_json::from_str::<ThemeMetadata>(&contents) {
        Ok(metadata) => Some(metadata),
        Err(error) => {
            errors.push(format!("failed to parse {}: {error}", path.display()));
            None
        }
    }
}

fn validate_metadata(
    repo: &Path,
    dir_slug: &str,
    metadata: &ThemeMetadata,
    errors: &mut Vec<String>,
) {
    if normalize_theme_id(&metadata.slug) != metadata.slug {
        errors.push(format!("themes/{dir_slug}/metadata.json has invalid slug"));
    }
    if metadata.slug != dir_slug {
        errors.push(format!(
            "themes/{dir_slug}/metadata.json slug must match directory name"
        ));
    }
    if metadata.name.trim().is_empty() {
        errors.push(format!("themes/{dir_slug}/metadata.json name is required"));
    }

    for version in &metadata.versions {
        if let Err(error) = Version::parse(&version.version) {
            errors.push(format!(
                "themes/{dir_slug}/metadata.json version '{}' is invalid: {error}",
                version.version
            ));
        }
        let file_path = repo.join("themes").join(dir_slug).join(&version.file);
        let contents = match fs::read_to_string(&file_path) {
            Ok(contents) => contents,
            Err(error) => {
                errors.push(format!("failed to read {}: {error}", file_path.display()));
                continue;
            }
        };
        if let Err(error) = parse_theme_colors_json(&contents) {
            errors.push(format!(
                "{} is not valid theme JSON: {error}",
                file_path.display()
            ));
        }
        if let Some(expected) = &version.checksum_sha256 {
            let actual = sha256_hex(contents.as_bytes());
            if !expected.eq_ignore_ascii_case(&actual) {
                errors.push(format!(
                    "{} checksum mismatch: expected {expected}, got {actual}",
                    file_path.display()
                ));
            }
        }
    }
}

fn upsert_version(versions: &mut Vec<ThemeMetadataVersion>, next: ThemeMetadataVersion) {
    if let Some(existing) = versions
        .iter_mut()
        .find(|existing| existing.version == next.version)
    {
        *existing = next;
    } else {
        versions.push(next);
    }
    versions.sort_unstable_by(|left, right| {
        Version::parse(&left.version)
            .ok()
            .cmp(&Version::parse(&right.version).ok())
    });
}

fn latest_version(versions: &[ThemeMetadataVersion]) -> Result<String, String> {
    versions
        .iter()
        .filter_map(|version| {
            Version::parse(&version.version)
                .ok()
                .map(|parsed| (parsed, version.version.clone()))
        })
        .max_by(|left, right| left.0.cmp(&right.0))
        .map(|(_, raw)| raw)
        .ok_or_else(|| "theme has no valid versions".to_string())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}
