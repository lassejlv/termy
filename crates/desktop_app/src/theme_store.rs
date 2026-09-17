use crate::config;
use std::collections::HashMap;
use std::path::PathBuf;
pub(crate) use termy_core::themes::ThemeStoreTheme;
use termy_core::themes::{
    THEME_REGISTRY_CACHE_VERSION, ThemeColors, ThemeRegistryCache, ThemeRegistryIndex,
    normalize_theme_id, parse_theme_colors_json, registry_file_url,
};

const DEFAULT_THEME_STORE_API_URL: &str = "https://api.termy.sh";
const DEFAULT_THEME_REGISTRY_URL: &str =
    "https://raw.githubusercontent.com/termy-org/themes/main/index.json";

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyThemeStoreTheme {
    name: String,
    slug: String,
    #[serde(default)]
    description: String,
    latest_version: Option<String>,
    file_url: Option<String>,
}

impl LegacyThemeStoreTheme {
    fn into_theme(self) -> Option<ThemeStoreTheme> {
        let name = self.name.trim().to_string();
        let slug = self.slug.trim().to_string();
        if name.is_empty() || slug.is_empty() {
            return None;
        }

        Some(ThemeStoreTheme {
            name,
            slug,
            description: self.description,
            latest_version: self.latest_version,
            file_url: self.file_url,
        })
    }
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum ThemeStorePayload {
    Legacy(Vec<LegacyThemeStoreTheme>),
    Registry(ThemeRegistryIndex),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InstalledTheme {
    pub(crate) slug: String,
    pub(crate) version: String,
    pub(crate) message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct ThemeStoreAuthUser {
    pub(crate) id: String,
    pub(crate) github_user_id: i64,
    pub(crate) github_login: String,
    pub(crate) avatar_url: Option<String>,
    pub(crate) name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct ThemeStoreAuthSession {
    pub(crate) session_token: String,
    pub(crate) user: ThemeStoreAuthUser,
}

pub(crate) fn theme_store_api_base_url() -> String {
    std::env::var("THEME_STORE_API_URL").unwrap_or_else(|_| DEFAULT_THEME_STORE_API_URL.into())
}

pub(crate) fn theme_store_registry_url() -> String {
    std::env::var("TERMY_THEME_REGISTRY_URL")
        .or_else(|_| std::env::var("THEME_STORE_REGISTRY_URL"))
        .unwrap_or_else(|_| DEFAULT_THEME_REGISTRY_URL.into())
}

fn current_unix_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Fetches themes from the repo registry. Returns `(themes, from_cache)` where `from_cache`
/// is `true` when the registry was unreachable and a saved cache was used as a fallback.
/// Uses ETag-based conditional requests to avoid re-downloading unchanged registries.
pub(crate) fn fetch_theme_store_themes_blocking(
    registry_url: &str,
) -> Result<(Vec<ThemeStoreTheme>, bool), String> {
    let cached = load_theme_store_cache();

    let cached_etag = cached.as_ref().and_then(|c| {
        if c.registry_url == registry_url {
            c.etag.clone()
        } else {
            None
        }
    });

    let fetch_url = theme_registry_fetch_url(registry_url);

    let mut request = ureq::get(&fetch_url)
        .set("Accept", "application/json")
        .set("Cache-Control", "no-cache")
        .set("Pragma", "no-cache");

    if let Some(ref etag) = cached_etag {
        request = request.set("If-None-Match", etag);
    }

    let fetch_result = request.call();

    match fetch_result {
        Ok(response) if response.status() == 304 => {
            // Not modified — ureq 2.x returns Ok for any status < 400, so 304
            // lands here (with an empty body) instead of in the Err arm. Fall
            // back to the cache and refresh its timestamp.
            if let Some(mut cache) = cached
                && cache.registry_url == registry_url
            {
                cache.fetched_at = current_unix_timestamp();
                if let Some(path) = theme_store_cache_path()
                    && let Ok(bytes) = bincode::serialize(&cache)
                {
                    let _ = std::fs::write(&path, bytes);
                }
                return Ok((cache.themes, false));
            }
            Err("Server returned 304 Not Modified but no matching local cache exists".to_string())
        }
        Ok(response) => {
            let etag = response.header("etag").map(|s| s.to_string());
            let raw = response
                .into_string()
                .map_err(|error| format!("Invalid theme registry response: {error}"))?;
            let parsed = parse_theme_store_payload(&raw, registry_url)?;

            save_theme_store_cache(&parsed, registry_url, etag);

            Ok((parsed, false))
        }
        Err(error) => {
            if let Some(cache) = cached.filter(|c| c.registry_url == registry_url) {
                Ok((cache.themes, true))
            } else {
                Err(format!("Failed to fetch store themes: {error}"))
            }
        }
    }
}

fn theme_registry_fetch_url(registry_url: &str) -> String {
    if !registry_url.contains("raw.githubusercontent.com") {
        return registry_url.to_string();
    }

    let cache_buster = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let separator = if registry_url.contains('?') { '&' } else { '?' };
    format!("{registry_url}{separator}termy_cache_bust={cache_buster}")
}

fn theme_store_cache_path() -> Option<PathBuf> {
    let config_path = config::ensure_config_file().ok()?;
    let parent = config_path.parent()?;
    Some(parent.join("theme_registry.cache"))
}

fn save_theme_store_cache(themes: &[ThemeStoreTheme], registry_url: &str, etag: Option<String>) {
    let Some(path) = theme_store_cache_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // Clean up legacy JSON cache to avoid confusion.
    if let Some(legacy_path) = path.parent().map(|p| p.join("theme_store_cache.json"))
        && legacy_path.exists()
    {
        let _ = std::fs::remove_file(&legacy_path);
    }

    let cache = ThemeRegistryCache {
        version: THEME_REGISTRY_CACHE_VERSION,
        fetched_at: current_unix_timestamp(),
        registry_url: registry_url.to_string(),
        etag,
        themes: themes.to_vec(),
    };

    match bincode::serialize(&cache) {
        Ok(bytes) => {
            if let Err(error) = std::fs::write(&path, bytes) {
                log::debug!(
                    "Failed to write theme store cache to {}: {}",
                    path.display(),
                    error
                );
            }
        }
        Err(error) => {
            log::debug!("Failed to serialize theme store cache: {error}");
        }
    }
}

pub(crate) fn load_cached_theme_store_themes(registry_url: &str) -> Option<Vec<ThemeStoreTheme>> {
    load_theme_store_cache().and_then(|cache| cached_themes_for_registry(cache, registry_url))
}

fn cached_themes_for_registry(
    cache: ThemeRegistryCache,
    registry_url: &str,
) -> Option<Vec<ThemeStoreTheme>> {
    (cache.registry_url == registry_url).then_some(cache.themes)
}

fn load_theme_store_cache() -> Option<ThemeRegistryCache> {
    let path = theme_store_cache_path()?;
    let bytes = std::fs::read(path).ok()?;
    let cache: ThemeRegistryCache = bincode::deserialize(&bytes).ok()?;

    if cache.version != THEME_REGISTRY_CACHE_VERSION {
        return None;
    }

    Some(cache)
}

pub(crate) fn fetch_theme_for_deeplink_blocking(slug: &str) -> Result<ThemeStoreTheme, String> {
    let slug = normalize_slug(slug)?;
    let registry_url = theme_store_registry_url();
    let (themes, _) = fetch_theme_store_themes_blocking(&registry_url)?;
    themes
        .into_iter()
        .find(|theme| theme.slug.eq_ignore_ascii_case(&slug))
        .ok_or_else(|| format!("Theme '{slug}' was not found in the theme registry"))
}

pub(crate) fn logout_auth_session_blocking(
    api_base: &str,
    session_token: &str,
) -> Result<(), String> {
    let base = api_base.trim_end_matches('/');
    let url = format!("{base}/auth/logout");
    let response = ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", session_token.trim()))
        .call();

    match response {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(401, _)) => Ok(()),
        Err(error) => Err(format!("Failed to logout from theme store: {error}")),
    }
}

pub(crate) fn clear_auth_session() -> Result<(), String> {
    let Some(path) = auth_session_path() else {
        return Ok(());
    };
    if path.exists() {
        std::fs::remove_file(path)
            .map_err(|error| format!("Failed to clear auth session: {error}"))?;
    }
    Ok(())
}

pub(crate) fn load_installed_theme_versions() -> HashMap<String, String> {
    let Some(path) = installed_theme_state_path() else {
        return HashMap::new();
    };
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return HashMap::new();
    };

    if let Ok(parsed_map) = serde_json::from_str::<HashMap<String, String>>(&contents) {
        return parsed_map
            .into_iter()
            .map(|(slug, version)| (slug.trim().to_ascii_lowercase(), version.trim().to_string()))
            .filter(|(slug, _)| !slug.is_empty())
            .collect();
    }

    if let Ok(parsed_list) = serde_json::from_str::<Vec<String>>(&contents) {
        return parsed_list
            .into_iter()
            .map(|slug| (slug.trim().to_ascii_lowercase(), String::new()))
            .filter(|(slug, _)| !slug.is_empty())
            .collect();
    }

    HashMap::new()
}

pub(crate) fn load_installed_theme_ids() -> Vec<String> {
    let mut ids = Vec::new();
    let Some(dir) = installed_themes_dir_path() else {
        return ids;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return ids;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        let normalized = normalize_theme_id(stem);
        if !normalized.is_empty() {
            ids.push(normalized);
        }
    }

    ids.sort_unstable();
    ids.dedup();
    ids
}

pub(crate) fn load_installed_theme_colors(theme_id: &str) -> Option<ThemeColors> {
    let normalized = normalize_theme_id(theme_id);
    if normalized.is_empty() {
        return None;
    }

    let path = installed_theme_file_path(&normalized)?;
    let contents = std::fs::read_to_string(path).ok()?;
    parse_theme_colors_json(&contents).ok()
}

pub(crate) fn persist_installed_theme_versions(
    versions: &HashMap<String, String>,
) -> Result<(), String> {
    let Some(path) = installed_theme_state_path() else {
        return Err("Config path unavailable".to_string());
    };
    let Some(parent) = path.parent() else {
        return Err("Invalid installed-theme metadata path".to_string());
    };
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("Failed to create metadata directory: {error}"))?;

    let mut sorted_entries: Vec<(String, String)> = versions
        .iter()
        .map(|(slug, version)| (slug.clone(), version.clone()))
        .collect();
    sorted_entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    let normalized: HashMap<String, String> = sorted_entries.into_iter().collect();
    let contents = serde_json::to_string_pretty(&normalized)
        .map_err(|error| format!("Failed to serialize installed themes: {error}"))?;
    std::fs::write(&path, contents)
        .map_err(|error| format!("Failed to write installed themes metadata: {error}"))?;
    Ok(())
}

pub(crate) fn install_theme_from_store_blocking(
    theme: ThemeStoreTheme,
) -> Result<InstalledTheme, String> {
    let file_url = theme
        .file_url
        .clone()
        .ok_or_else(|| format!("Theme '{}' has no downloadable file URL", theme.slug))?;

    let response = ureq::get(&file_url)
        .set("Accept", "application/json")
        .call()
        .map_err(|error| format!("Failed to download theme '{}': {error}", theme.slug))?;
    let contents = response
        .into_string()
        .map_err(|error| format!("Failed to read theme '{}': {error}", theme.slug))?;

    parse_theme_colors_json(&contents)
        .map_err(|error| format!("Failed to validate theme '{}': {error}", theme.name))?;

    let normalized_slug = theme.slug.trim().to_ascii_lowercase();
    let installed_version = theme.latest_version.clone().unwrap_or_default();

    let path = installed_theme_file_path(&normalized_slug)
        .ok_or_else(|| "Config path unavailable".to_string())?;
    let Some(parent) = path.parent() else {
        return Err("Invalid installed theme path".to_string());
    };
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("Failed to create installed theme directory: {error}"))?;
    std::fs::write(&path, contents)
        .map_err(|error| format!("Failed to write installed theme file: {error}"))?;

    let mut installed_versions = load_installed_theme_versions();
    installed_versions.insert(normalized_slug.clone(), installed_version.clone());
    persist_installed_theme_versions(&installed_versions)?;

    Ok(InstalledTheme {
        slug: normalized_slug,
        version: installed_version,
        message: format!("Installed theme '{}'", theme.name),
    })
}

fn installed_theme_state_path() -> Option<PathBuf> {
    let config_path = config::ensure_config_file().ok()?;
    let parent = config_path.parent()?;
    Some(parent.join("theme_store_installed.json"))
}

fn auth_session_path() -> Option<PathBuf> {
    let config_path = config::ensure_config_file().ok()?;
    let parent = config_path.parent()?;
    Some(parent.join("theme_store_auth.json"))
}

fn installed_themes_dir_path() -> Option<PathBuf> {
    let config_path = config::ensure_config_file().ok()?;
    let parent = config_path.parent()?;
    Some(parent.join("themes"))
}

fn installed_theme_file_path(slug: &str) -> Option<PathBuf> {
    let normalized = normalize_slug(slug).ok()?;
    Some(installed_themes_dir_path()?.join(format!("{normalized}.json")))
}

pub(crate) fn uninstall_installed_theme(slug: &str) -> Result<bool, String> {
    let key = normalize_slug(slug)?;
    let mut installed_versions = load_installed_theme_versions();
    let removed = installed_versions.remove(&key).is_some();

    if let Some(path) = installed_theme_file_path(&key)
        && path.exists()
    {
        std::fs::remove_file(&path)
            .map_err(|error| format!("Failed to remove installed theme file: {error}"))?;
    }

    persist_installed_theme_versions(&installed_versions)?;
    Ok(removed)
}

fn normalize_slug(slug: &str) -> Result<String, String> {
    let slug = slug.trim().to_ascii_lowercase();
    if slug.is_empty() {
        return Err("Theme install deeplink is missing a slug".to_string());
    }
    if !slug.chars().all(|character| {
        character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
    }) {
        return Err(format!("Invalid theme slug '{slug}'"));
    }
    Ok(slug)
}

fn parse_theme_store_payload(
    raw_json: &str,
    registry_url: &str,
) -> Result<Vec<ThemeStoreTheme>, String> {
    let payload: ThemeStorePayload = serde_json::from_str(raw_json)
        .map_err(|error| format!("Invalid theme registry response: {error}"))?;

    let mut parsed: Vec<ThemeStoreTheme> = match payload {
        ThemeStorePayload::Legacy(themes) => themes
            .into_iter()
            .filter_map(LegacyThemeStoreTheme::into_theme)
            .collect(),
        ThemeStorePayload::Registry(index) => index
            .themes
            .into_iter()
            .filter_map(|theme| {
                let slug = normalize_slug(&theme.slug).ok()?;
                Some(ThemeStoreTheme {
                    name: theme.name.trim().to_string(),
                    slug,
                    description: theme.description,
                    latest_version: Some(theme.latest_version),
                    file_url: Some(registry_file_url(registry_url, &theme.file)),
                })
            })
            .filter(|theme| !theme.name.is_empty())
            .collect(),
    };

    parsed.sort_unstable_by(|left: &ThemeStoreTheme, right: &ThemeStoreTheme| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
    });
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::{
        THEME_REGISTRY_CACHE_VERSION, ThemeRegistryCache, ThemeStoreTheme,
        cached_themes_for_registry,
    };

    #[derive(serde::Serialize)]
    struct LegacyThemeStoreTheme {
        name: String,
        slug: String,
        description: String,
        latest_version: Option<String>,
        file_url: Option<String>,
    }

    #[derive(serde::Serialize)]
    struct LegacyThemeRegistryCache {
        version: u32,
        fetched_at: u64,
        registry_url: String,
        etag: Option<String>,
        themes: Vec<LegacyThemeStoreTheme>,
    }

    fn theme(slug: &str) -> ThemeStoreTheme {
        ThemeStoreTheme {
            name: slug.to_string(),
            slug: slug.to_string(),
            description: String::new(),
            latest_version: Some("1.0.0".to_string()),
            file_url: Some(format!("https://example.com/{slug}.json")),
        }
    }

    #[test]
    fn cached_themes_only_load_for_matching_registry_url() {
        let cached_themes = vec![theme("tokyo-night")];
        let cache = ThemeRegistryCache {
            version: 1,
            fetched_at: 0,
            registry_url: "https://example.com/index.json".to_string(),
            etag: Some("etag".to_string()),
            themes: cached_themes.clone(),
        };

        assert_eq!(
            cached_themes_for_registry(cache, "https://example.com/index.json"),
            Some(cached_themes)
        );

        let cache = ThemeRegistryCache {
            version: 1,
            fetched_at: 0,
            registry_url: "https://example.com/index.json".to_string(),
            etag: Some("etag".to_string()),
            themes: vec![theme("tokyo-night")],
        };

        assert_eq!(
            cached_themes_for_registry(cache, "https://other.example.com/index.json"),
            None
        );
    }

    #[test]
    fn shared_registry_cache_preserves_v1_bincode_layout_and_round_trips() {
        let cache = ThemeRegistryCache {
            version: THEME_REGISTRY_CACHE_VERSION,
            fetched_at: 1_700_000_000,
            registry_url: "https://example.com/index.json".to_string(),
            etag: Some("etag-v1".to_string()),
            themes: vec![theme("tokyo-night")],
        };
        let legacy = LegacyThemeRegistryCache {
            version: 1,
            fetched_at: 1_700_000_000,
            registry_url: "https://example.com/index.json".to_string(),
            etag: Some("etag-v1".to_string()),
            themes: vec![LegacyThemeStoreTheme {
                name: "tokyo-night".to_string(),
                slug: "tokyo-night".to_string(),
                description: String::new(),
                latest_version: Some("1.0.0".to_string()),
                file_url: Some("https://example.com/tokyo-night.json".to_string()),
            }],
        };

        let encoded = bincode::serialize(&cache).expect("serialize shared cache");
        assert_eq!(
            encoded,
            bincode::serialize(&legacy).expect("serialize legacy cache layout")
        );
        assert_eq!(
            bincode::deserialize::<ThemeRegistryCache>(&encoded).expect("deserialize shared cache"),
            cache
        );
    }
}
