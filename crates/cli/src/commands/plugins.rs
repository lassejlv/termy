use std::{
    collections::HashSet,
    fs,
    io::{self, IsTerminal, Read, Write},
    path::{Component, Path, PathBuf},
    sync::mpsc::{self, RecvTimeoutError},
    time::Duration,
};

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use serde::Deserialize;
use tempfile::TempDir;
use termy_core::plugin_runtime::{
    MAX_PLUGIN_SOURCE_BYTES, MAX_PLUGIN_SOURCE_FILES, PluginRuntime, PluginSourceMetadata,
    valid_plugin_id, validate_plugin_capabilities,
};

use crate::PluginCommand;

const MAX_API_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_MANIFEST_BYTES: usize = 64 * 1024;
const MAX_ERROR_RESPONSE_BYTES: usize = 8 * 1024;
const MAX_MANIFEST_CANDIDATES: usize = 128;
const USER_AGENT: &str = "termy-plugin-installer";

pub fn run(command: PluginCommand) {
    let result = match command {
        PluginCommand::Add {
            source,
            reference,
            path,
            yes,
        } => add(&source, reference.as_deref(), path.as_deref(), yes),
        PluginCommand::Init { path, id, name } => init(&path, id.as_deref(), name.as_deref()),
        PluginCommand::Dev { path } => dev(&path),
        PluginCommand::List => list(),
        PluginCommand::Status { id } => status(id.as_deref()),
        PluginCommand::Enable { id } => set_enabled(&id, true),
        PluginCommand::Disable { id } => set_enabled(&id, false),
        PluginCommand::Update { id, yes } => update(&id, yes),
        PluginCommand::Remove { id, yes } => remove(&id, yes),
    };

    if let Err(error) = result {
        eprintln!("Plugin command failed: {error}");
        std::process::exit(1);
    }
}

fn add(
    source: &str,
    explicit_ref: Option<&str>,
    explicit_path: Option<&str>,
    yes: bool,
) -> Result<(), String> {
    if source.contains("://") {
        return add_from_github(source, explicit_ref, explicit_path, yes);
    }
    if explicit_ref.is_some() || explicit_path.is_some() {
        return Err("--ref and --path can only be used with a GitHub source".to_string());
    }

    let installed = plugin_runtime()?.install_from_directory(Path::new(source))?;
    println!(
        "Installed {} ({}) at {}",
        installed.name,
        installed.id,
        installed.path.display()
    );
    println!("It will be available the next time the command palette opens.");
    Ok(())
}

fn add_from_github(
    source_url: &str,
    explicit_ref: Option<&str>,
    explicit_path: Option<&str>,
    yes: bool,
) -> Result<(), String> {
    let source = GitHubSource::parse(source_url, explicit_ref, explicit_path)?;
    let client = GitHubClient::new();
    let repository = client.resolve(&source)?;
    let candidate = client.select_plugin(&source, &repository)?;
    let runtime = plugin_runtime()?;
    let inventory = runtime.installed_plugins()?;
    if inventory
        .plugins
        .iter()
        .any(|plugin| plugin.id == candidate.id)
    {
        return Err(format!(
            "plugin `{}` is already installed; use `termy plugin update {}` instead",
            candidate.id, candidate.id
        ));
    }

    print_trusted_code_warning(&candidate, &source, &repository.revision);
    confirm("Install and enable this plugin?", yes)?;

    let downloaded = client.download_plugin(&source, &repository, &candidate)?;
    let metadata = source.metadata(&repository.revision, &candidate.directory);
    let installed = runtime.install_from_directory_with_source(downloaded.path(), metadata)?;
    println!(
        "Installed {} ({}) at {}",
        installed.name,
        installed.id,
        installed.path.display()
    );
    println!("It will be available the next time the command palette opens.");
    Ok(())
}

fn list() -> Result<(), String> {
    status(None)
}

fn dev(path: &Path) -> Result<(), String> {
    let source = fs::canonicalize(path).map_err(|error| {
        format!(
            "failed to resolve plugin source {}: {error}",
            path.display()
        )
    })?;
    let runtime = plugin_runtime()?;
    let installed = runtime.sync_from_directory(&source)?;
    println!(
        "Synced {} ({}) to {}",
        installed.name,
        installed.id,
        installed.path.display()
    );

    let (sender, receiver) = mpsc::channel::<notify::Result<Event>>();
    let mut watcher = RecommendedWatcher::new(sender, Config::default())
        .map_err(|error| format!("failed to create plugin watcher: {error}"))?;
    watcher
        .watch(&source, RecursiveMode::Recursive)
        .map_err(|error| format!("failed to watch {}: {error}", source.display()))?;
    println!("Watching {}. Press Ctrl+C to stop.", source.display());
    io::stdout()
        .flush()
        .map_err(|error| format!("failed to report watcher readiness: {error}"))?;

    loop {
        let first = receiver
            .recv()
            .map_err(|_| "plugin watcher stopped unexpectedly".to_string())?;
        let mut changed = dev_event_requires_sync(first, &source);
        loop {
            match receiver.recv_timeout(Duration::from_millis(200)) {
                Ok(event) => changed |= dev_event_requires_sync(event, &source),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("plugin watcher stopped unexpectedly".to_string());
                }
            }
        }
        if !changed {
            continue;
        }
        match runtime.sync_from_directory(&source) {
            Ok(plugin) => println!("Synced {} ({})", plugin.name, plugin.id),
            Err(error) => eprintln!("Plugin sync failed: {error}"),
        }
    }
}

fn dev_event_requires_sync(event: notify::Result<Event>, source: &Path) -> bool {
    let event = match event {
        Ok(event) => event,
        Err(error) => {
            eprintln!("Plugin watcher warning: {error}");
            return false;
        }
    };
    if event.kind.is_access() {
        return false;
    }
    event.paths.is_empty()
        || event
            .paths
            .iter()
            .any(|path| !dev_path_is_ignored(source, path))
}

fn dev_path_is_ignored(source: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(source) else {
        return false;
    };
    relative.components().any(|component| {
        matches!(
            component,
            Component::Normal(name)
                if matches!(name.to_str(), Some(".git" | "node_modules"))
        )
    }) || matches!(
        relative.file_name().and_then(|name| name.to_str()),
        Some(".termy-disabled" | ".termy-source.json")
    )
}

fn status(selected_id: Option<&str>) -> Result<(), String> {
    if let Some(id) = selected_id
        && !valid_plugin_id(id)
    {
        return Err(format!("invalid plugin ID `{id}`"));
    }
    let inventory = plugin_runtime()?.installed_plugins()?;
    for error in inventory.errors {
        eprintln!("Plugin inventory warning: {error}");
    }
    let plugins = inventory
        .plugins
        .into_iter()
        .filter(|plugin| selected_id.is_none_or(|id| plugin.id == id))
        .collect::<Vec<_>>();
    if plugins.is_empty() {
        if let Some(id) = selected_id {
            return Err(format!("plugin `{id}` is not installed"));
        }
        println!("No plugins installed.");
        return Ok(());
    }

    for plugin in plugins {
        let status = if plugin.error.is_some() {
            "invalid"
        } else if plugin.enabled {
            "enabled"
        } else {
            "disabled"
        };
        let version = plugin
            .version
            .as_deref()
            .map(|version| format!(" v{version}"))
            .unwrap_or_default();
        println!("{}{}  {}  {}", plugin.id, version, status, plugin.name);
        if let Some(source) = plugin.source {
            println!(
                "  {}{} @ {}",
                source.repository_url,
                display_subdirectory(&source.subdirectory),
                short_revision(&source.revision)
            );
        } else {
            println!("  local installation");
        }
        if let Some(error) = plugin.error {
            println!("  error: {error}");
        }
    }
    Ok(())
}

fn init(
    path: &Path,
    requested_id: Option<&str>,
    requested_name: Option<&str>,
) -> Result<(), String> {
    let id = match requested_id {
        Some(id) => id.to_string(),
        None => inferred_directory_name(path)
            .ok_or_else(|| "pass --id when the target directory has no usable name".to_string())?
            .to_ascii_lowercase()
            .replace(' ', "-"),
    };
    if !valid_plugin_id(&id) {
        return Err(format!(
            "invalid plugin ID `{id}`; use lowercase letters, numbers, dots, underscores, or hyphens"
        ));
    }
    let name = requested_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map_or_else(|| title_from_id(&id), str::to_string);
    if name.chars().count() > 200 {
        return Err("plugin name must contain at most 200 characters".to_string());
    }

    let created_directory = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(format!(
                "plugin target must be a real directory: {}",
                path.display()
            ));
        }
        Ok(_) => false,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(|error| {
                format!(
                    "failed to create plugin directory {}: {error}",
                    path.display()
                )
            })?;
            true
        }
        Err(error) => {
            return Err(format!(
                "failed to inspect plugin directory {}: {error}",
                path.display()
            ));
        }
    };
    let manifest_path = path.join("plugin.json");
    let entrypoint_path = path.join("plugin.ts");
    if manifest_path.exists() || entrypoint_path.exists() {
        if created_directory {
            let _ = fs::remove_dir(path);
        }
        return Err(
            "plugin.json or plugin.ts already exists; Termy will not overwrite plugin source"
                .to_string(),
        );
    }

    let mut manifest = serde_json::to_vec_pretty(&serde_json::json!({
        "$schema": "https://termy.sh/schemas/plugin.schema.json",
        "apiVersion": 1,
        "id": &id,
        "name": &name,
        "version": "0.1.0",
        "capabilities": []
    }))
    .map_err(|error| format!("failed to create plugin.json: {error}"))?;
    manifest.push(b'\n');
    if let Err(error) = write_new_file(&manifest_path, &manifest) {
        if created_directory {
            let _ = fs::remove_dir(path);
        }
        return Err(error);
    }
    if let Err(error) = write_new_file(&entrypoint_path, plugin_template().as_bytes()) {
        let _ = fs::remove_file(&manifest_path);
        if created_directory {
            let _ = fs::remove_dir(path);
        }
        return Err(error);
    }

    println!("Initialized plugin `{id}` in {}", path.display());
    println!(
        "Edit plugin.ts, then run `termy plugin dev {}`.",
        path.display()
    );
    Ok(())
}

fn inferred_directory_name(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty() && *name != ".")
        .map(str::to_string)
        .or_else(|| {
            (path == Path::new("."))
                .then(|| std::env::current_dir().ok())
                .flatten()
                .and_then(|directory| {
                    directory
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(str::to_string)
                })
        })
}

fn write_new_file(path: &Path, contents: &[u8]) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("refusing to overwrite {}: {error}", path.display()))?;
    if let Err(error) = file.write_all(contents) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(format!("failed to write {}: {error}", path.display()));
    }
    Ok(())
}

fn title_from_id(id: &str) -> String {
    id.split(['-', '_', '.'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            let first = characters
                .next()
                .map(|character| character.to_ascii_uppercase())
                .unwrap_or_default();
            format!("{first}{}", characters.as_str())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn plugin_template() -> &'static str {
    r#"export default definePlugin({
  settings: {
    greeting: {
      type: "text",
      title: "Greeting",
      defaultValue: "Hello from Termy",
    },
  },
  commands: [
    {
      id: "hello",
      title: "Hello from my plugin",
      keywords: ["hello"],
      icon: "info",
      run({ context }) {
        context.toasts.success(context.settings.get("greeting") ?? "Hello from Termy");
      },
    },
  ],
} satisfies TermyPlugin);
"#
}

fn update(id: &str, yes: bool) -> Result<(), String> {
    if !valid_plugin_id(id) {
        return Err(format!("invalid plugin ID `{id}`"));
    }
    let runtime = plugin_runtime()?;
    let inventory = runtime.installed_plugins()?;
    let installed = inventory
        .plugins
        .into_iter()
        .find(|plugin| plugin.id == id)
        .ok_or_else(|| format!("plugin `{id}` is not installed"))?;
    if let Some(error) = installed.error {
        return Err(format!("plugin `{id}` cannot be updated: {error}"));
    }
    let current_source = installed.source.ok_or_else(|| {
        format!("plugin `{id}` was not installed from GitHub and has no update source")
    })?;
    let source = GitHubSource::from_metadata(&current_source)?;
    let client = GitHubClient::new();
    let repository = client.resolve(&source)?;
    if repository.revision == current_source.revision {
        println!("Plugin `{id}` is already up to date.");
        return Ok(());
    }
    let candidate = client.select_plugin(&source, &repository)?;
    if candidate.id != id {
        return Err(format!(
            "the updated plugin declares ID `{}`, expected `{id}`; the installed plugin was left untouched",
            candidate.id
        ));
    }

    print_trusted_code_warning(&candidate, &source, &repository.revision);
    confirm(
        &format!(
            "Update `{id}` from {} to {}?",
            short_revision(&current_source.revision),
            short_revision(&repository.revision)
        ),
        yes,
    )?;

    let downloaded = client.download_plugin(&source, &repository, &candidate)?;
    let metadata = source.metadata(&repository.revision, &candidate.directory);
    let updated = runtime.update_plugin_from_directory(id, downloaded.path(), metadata)?;
    println!(
        "Updated {} ({}) to {}.",
        updated.name,
        updated.id,
        short_revision(&repository.revision)
    );
    Ok(())
}

fn set_enabled(id: &str, enabled: bool) -> Result<(), String> {
    if !valid_plugin_id(id) {
        return Err(format!("invalid plugin ID `{id}`"));
    }
    let runtime = plugin_runtime()?;
    let inventory = runtime.installed_plugins()?;
    if !inventory.plugins.iter().any(|plugin| plugin.id == id) {
        return Err(format!("plugin `{id}` is not installed"));
    }
    runtime.set_plugin_enabled(id, enabled)?;
    println!(
        "Plugin `{id}` is now {}.",
        if enabled { "enabled" } else { "disabled" }
    );
    Ok(())
}

fn remove(id: &str, yes: bool) -> Result<(), String> {
    if !valid_plugin_id(id) {
        return Err(format!("invalid plugin ID `{id}`"));
    }
    let runtime = plugin_runtime()?;
    let inventory = runtime.installed_plugins()?;
    if !inventory.plugins.iter().any(|plugin| plugin.id == id) {
        return Err(format!("plugin `{id}` is not installed"));
    }
    confirm(&format!("Remove plugin `{id}`?"), yes)?;
    runtime.uninstall_plugin(id)?;
    println!("Removed plugin `{id}`.");
    Ok(())
}

fn plugin_runtime() -> Result<PluginRuntime, String> {
    let config_path = termy_core::config_core::config_path()
        .ok_or_else(|| "Termy config path is unavailable".to_string())?;
    Ok(PluginRuntime::new(Some(&config_path)))
}

fn print_trusted_code_warning(candidate: &PluginCandidate, source: &GitHubSource, revision: &str) {
    eprintln!("WARNING: Termy plugins are trusted code.");
    eprintln!(
        "`{}` can run arbitrary Bun code with your user permissions.",
        candidate.name
    );
    eprintln!(
        "Source: {}{} @ {}",
        source.repository_url(),
        display_subdirectory(&candidate.directory),
        short_revision(revision)
    );
}

fn confirm(message: &str, yes: bool) -> Result<(), String> {
    if yes {
        return Ok(());
    }
    if !io::stdin().is_terminal() {
        return Err(
            "confirmation is required; review the source and rerun with `--yes`".to_string(),
        );
    }
    print!("{message} [y/N] ");
    io::stdout()
        .flush()
        .map_err(|error| format!("failed to show confirmation prompt: {error}"))?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .map_err(|error| format!("failed to read confirmation: {error}"))?;
    if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        Err("cancelled; no plugin files were changed".to_string())
    }
}

fn short_revision(revision: &str) -> &str {
    revision.get(..12).unwrap_or(revision)
}

fn display_subdirectory(path: &str) -> String {
    if path.is_empty() {
        String::new()
    } else {
        format!("/{path}")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GitHubSource {
    owner: String,
    repository: String,
    requested_ref: Option<String>,
    subdirectory: Option<String>,
}

impl GitHubSource {
    fn parse(
        source_url: &str,
        explicit_ref: Option<&str>,
        explicit_path: Option<&str>,
    ) -> Result<Self, String> {
        let url =
            url::Url::parse(source_url).map_err(|error| format!("invalid GitHub URL: {error}"))?;
        if url.scheme() != "https"
            || !url
                .host_str()
                .is_some_and(|host| host.eq_ignore_ascii_case("github.com"))
            || url.port().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(
                "plugin source must be a plain HTTPS URL on github.com without credentials, query, or fragment"
                    .to_string(),
            );
        }
        let segments = url
            .path_segments()
            .into_iter()
            .flatten()
            .filter(|segment| !segment.is_empty())
            .map(decode_url_segment)
            .collect::<Result<Vec<_>, _>>()?;
        if segments.len() < 2 {
            return Err("GitHub URL must include an owner and repository".to_string());
        }
        let owner = segments[0].clone();
        let repository = segments[1]
            .strip_suffix(".git")
            .unwrap_or(&segments[1])
            .to_string();
        validate_github_name(&owner, "owner")?;
        validate_github_name(&repository, "repository")?;

        let (url_ref, url_path) = match segments.get(2).map(String::as_str) {
            None => (None, None),
            Some("tree") if segments.len() >= 4 => {
                let path = (segments.len() > 4).then(|| segments[4..].join("/"));
                (Some(segments[3].clone()), path)
            }
            Some("tree") => {
                return Err("GitHub tree URL must include a branch, tag, or commit".to_string());
            }
            Some(_) => {
                return Err(
                    "use a GitHub repository URL or a /tree/<ref>/<plugin-path> URL".to_string(),
                );
            }
        };

        let requested_ref = merge_selector("ref", url_ref, explicit_ref.map(str::to_string))?;
        if let Some(reference) = requested_ref.as_deref() {
            validate_ref(reference)?;
        }
        let explicit_path = explicit_path
            .map(|path| normalize_repo_path(path, false))
            .transpose()?;
        let url_path = url_path
            .map(|path| normalize_repo_path(&path, false))
            .transpose()?;
        let subdirectory = merge_selector("path", url_path, explicit_path)?;

        Ok(Self {
            owner,
            repository,
            requested_ref,
            subdirectory,
        })
    }

    fn from_metadata(metadata: &PluginSourceMetadata) -> Result<Self, String> {
        let mut source = Self::parse(&metadata.repository_url, None, None)?;
        if let Some(reference) = metadata.requested_ref.as_deref() {
            validate_ref(reference)?;
            source.requested_ref = Some(reference.to_string());
        }
        source.subdirectory = Some(if metadata.subdirectory.is_empty() {
            String::new()
        } else {
            normalize_repo_path(&metadata.subdirectory, false)?
        });
        Ok(source)
    }

    fn repository_url(&self) -> String {
        format!("https://github.com/{}/{}", self.owner, self.repository)
    }

    fn metadata(&self, revision: &str, directory: &str) -> PluginSourceMetadata {
        PluginSourceMetadata {
            repository_url: self.repository_url(),
            requested_ref: self.requested_ref.clone(),
            revision: revision.to_string(),
            subdirectory: directory.to_string(),
        }
    }
}

fn decode_url_segment(segment: &str) -> Result<String, String> {
    percent_decode_str(segment)
        .decode_utf8()
        .map(|segment| segment.into_owned())
        .map_err(|_| "GitHub URL path must be valid UTF-8".to_string())
}

fn merge_selector(
    label: &str,
    from_url: Option<String>,
    explicit: Option<String>,
) -> Result<Option<String>, String> {
    match (from_url, explicit) {
        (Some(from_url), Some(explicit)) if from_url != explicit => Err(format!(
            "GitHub URL {label} `{from_url}` conflicts with --{label} `{explicit}`"
        )),
        (Some(value), _) | (_, Some(value)) => Ok(Some(value)),
        (None, None) => Ok(None),
    }
}

fn validate_github_name(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 100
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        return Err(format!("invalid GitHub {label} `{value}`"));
    }
    Ok(())
}

fn validate_ref(reference: &str) -> Result<(), String> {
    if reference.trim().is_empty()
        || reference.len() > 256
        || reference.chars().any(char::is_control)
        || reference.contains("..")
        || reference.contains('\\')
        || reference.starts_with('-')
    {
        return Err(format!("invalid Git ref `{reference}`"));
    }
    Ok(())
}

fn normalize_repo_path(path: &str, allow_root: bool) -> Result<String, String> {
    if path.len() > 1_024 || path.contains('\\') || path.chars().any(char::is_control) {
        return Err(format!("invalid plugin path `{path}`"));
    }
    let components = path
        .split('/')
        .filter(|component| !component.is_empty())
        .map(|component| match component {
            "." | ".." => Err(format!(
                "plugin path must stay inside the repository: `{path}`"
            )),
            _ => Ok(component),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if components.is_empty() && !allow_root {
        return Err("plugin path cannot be empty".to_string());
    }
    Ok(components.join("/"))
}

struct GitHubClient {
    agent: ureq::Agent,
    token: Option<String>,
}

impl GitHubClient {
    fn new() -> Self {
        let token = std::env::var("GITHUB_TOKEN")
            .or_else(|_| std::env::var("GH_TOKEN"))
            .ok()
            .filter(|token| !token.trim().is_empty());
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(15))
                .timeout(Duration::from_secs(45))
                .build(),
            token,
        }
    }

    fn resolve(&self, source: &GitHubSource) -> Result<ResolvedRepository, String> {
        let reference = if let Some(reference) = source.requested_ref.as_deref() {
            reference.to_string()
        } else {
            let endpoint = format!(
                "https://api.github.com/repos/{}/{}",
                source.owner, source.repository
            );
            self.api_json::<RepositoryResponse>(&endpoint)?
                .default_branch
        };
        validate_ref(&reference)?;
        let encoded_ref = utf8_percent_encode(&reference, NON_ALPHANUMERIC);
        let commit_endpoint = format!(
            "https://api.github.com/repos/{}/{}/commits/{encoded_ref}",
            source.owner, source.repository
        );
        let commit = self.api_json::<CommitResponse>(&commit_endpoint)?;
        if commit.sha.len() != 40
            || !commit
                .sha
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        {
            return Err("GitHub returned an invalid commit revision".to_string());
        }
        let tree_endpoint = format!(
            "https://api.github.com/repos/{}/{}/git/trees/{}?recursive=1",
            source.owner, source.repository, commit.commit.tree.sha
        );
        let tree = self.api_json::<TreeResponse>(&tree_endpoint)?;
        if tree.truncated {
            return Err(
                "GitHub truncated the repository tree; pass --path to a smaller plugin repository or split the plugin out"
                    .to_string(),
            );
        }
        Ok(ResolvedRepository {
            revision: commit.sha,
            entries: tree.tree,
        })
    }

    fn select_plugin(
        &self,
        source: &GitHubSource,
        repository: &ResolvedRepository,
    ) -> Result<PluginCandidate, String> {
        if let Some(directory) = source.subdirectory.as_deref() {
            let manifest_path = joined_repo_path(directory, "plugin.json");
            let entry = repository
                .entries
                .iter()
                .find(|entry| entry.path == manifest_path)
                .ok_or_else(|| {
                    format!(
                        "no plugin.json found at `{manifest_path}`; check --path and the selected ref"
                    )
                })?;
            require_regular_blob(entry, "plugin.json")?;
            let contents = self.raw_file(
                source,
                &repository.revision,
                &manifest_path,
                MAX_MANIFEST_BYTES,
            )?;
            return parse_candidate(directory, &contents, &repository.entries);
        }

        let manifest_entries = repository
            .entries
            .iter()
            .filter(|entry| {
                entry.kind == "blob"
                    && entry.path.rsplit('/').next() == Some("plugin.json")
                    && !ignored_repo_path(&entry.path)
            })
            .collect::<Vec<_>>();
        if manifest_entries.is_empty() {
            return Err(
                "no plugin.json found in the repository; point --path at the plugin directory"
                    .to_string(),
            );
        }
        if manifest_entries.len() > MAX_MANIFEST_CANDIDATES {
            return Err(format!(
                "repository contains more than {MAX_MANIFEST_CANDIDATES} plugin.json files; choose one with --path"
            ));
        }

        let mut candidates = Vec::new();
        let mut invalid = Vec::new();
        for entry in manifest_entries {
            let directory = entry.path.strip_suffix("/plugin.json").unwrap_or("");
            let result = require_regular_blob(entry, "plugin.json").and_then(|()| {
                let contents = self.raw_file(
                    source,
                    &repository.revision,
                    &entry.path,
                    MAX_MANIFEST_BYTES,
                )?;
                parse_candidate(directory, &contents, &repository.entries)
            });
            match result {
                Ok(candidate) => candidates.push(candidate),
                Err(error) => invalid.push(format!("{}: {error}", entry.path)),
            }
        }

        match candidates.len() {
            1 => Ok(candidates.remove(0)),
            0 => {
                let details = invalid.into_iter().take(4).collect::<Vec<_>>().join("; ");
                let details = if details.is_empty() {
                    String::new()
                } else {
                    format!(": {details}")
                };
                Err(format!("no valid Termy plugin found{details}"))
            }
            _ => {
                candidates.sort_by(|left, right| left.directory.cmp(&right.directory));
                let choices = candidates
                    .iter()
                    .map(|candidate| {
                        let directory = if candidate.directory.is_empty() {
                            "."
                        } else {
                            &candidate.directory
                        };
                        format!("{directory} ({})", candidate.id)
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                Err(format!(
                    "repository contains multiple Termy plugins: {choices}; rerun with --path <directory>"
                ))
            }
        }
    }

    fn download_plugin(
        &self,
        source: &GitHubSource,
        repository: &ResolvedRepository,
        candidate: &PluginCandidate,
    ) -> Result<DownloadedPlugin, String> {
        let files = plugin_files(&repository.entries, &candidate.directory)?;
        let temporary = tempfile::tempdir()
            .map_err(|error| format!("failed to prepare plugin download: {error}"))?;
        let root = temporary.path().join("plugin");
        fs::create_dir(&root)
            .map_err(|error| format!("failed to prepare plugin download: {error}"))?;

        for file in files {
            let repository_path = joined_repo_path(&candidate.directory, &file.relative_path);
            let expected_size = usize::try_from(file.size)
                .map_err(|_| format!("plugin file is too large: {repository_path}"))?;
            let contents = self.raw_file(
                source,
                &repository.revision,
                &repository_path,
                expected_size,
            )?;
            if contents.len() != expected_size {
                return Err(format!(
                    "GitHub returned {} bytes for `{repository_path}`, expected {expected_size}",
                    contents.len()
                ));
            }
            let destination = safe_destination(&root, &file.relative_path)?;
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    format!("failed to create plugin download directory: {error}")
                })?;
            }
            fs::write(&destination, contents).map_err(|error| {
                format!(
                    "failed to save plugin file {}: {error}",
                    destination.display()
                )
            })?;
        }

        Ok(DownloadedPlugin {
            _temporary: temporary,
            root,
        })
    }

    fn api_json<T: for<'de> Deserialize<'de>>(&self, endpoint: &str) -> Result<T, String> {
        let bytes = self.request_bytes(
            endpoint,
            "application/vnd.github+json",
            MAX_API_RESPONSE_BYTES,
        )?;
        serde_json::from_slice(&bytes)
            .map_err(|error| format!("GitHub returned invalid JSON: {error}"))
    }

    fn raw_file(
        &self,
        source: &GitHubSource,
        revision: &str,
        path: &str,
        maximum_bytes: usize,
    ) -> Result<Vec<u8>, String> {
        let encoded_path = path
            .split('/')
            .map(|segment| utf8_percent_encode(segment, NON_ALPHANUMERIC).to_string())
            .collect::<Vec<_>>()
            .join("/");
        let endpoint = format!(
            "https://raw.githubusercontent.com/{}/{}/{revision}/{encoded_path}",
            source.owner, source.repository
        );
        self.request_bytes(&endpoint, "application/octet-stream", maximum_bytes)
    }

    fn request_bytes(
        &self,
        endpoint: &str,
        accept: &str,
        maximum_bytes: usize,
    ) -> Result<Vec<u8>, String> {
        let mut request = self
            .agent
            .get(endpoint)
            .set("User-Agent", USER_AGENT)
            .set("Accept", accept)
            .set("X-GitHub-Api-Version", "2022-11-28");
        if let Some(token) = self.token.as_deref() {
            request = request.set("Authorization", &format!("Bearer {token}"));
        }
        let response = request.call().map_err(github_request_error)?;
        read_limited(response, maximum_bytes)
    }
}

fn github_request_error(error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(status, response) => {
            let body = read_limited(response, MAX_ERROR_RESPONSE_BYTES).unwrap_or_default();
            let message = serde_json::from_slice::<GitHubErrorResponse>(&body)
                .ok()
                .map(|error| error.message)
                .or_else(|| {
                    String::from_utf8(body)
                        .ok()
                        .map(|body| body.trim().to_string())
                        .filter(|body| !body.is_empty())
                })
                .unwrap_or_else(|| "request failed".to_string());
            format!("GitHub request failed with HTTP {status}: {message}")
        }
        ureq::Error::Transport(error) => format!("GitHub request failed: {error}"),
    }
}

fn read_limited(response: ureq::Response, maximum_bytes: usize) -> Result<Vec<u8>, String> {
    let limit = maximum_bytes
        .checked_add(1)
        .ok_or_else(|| "response size limit is invalid".to_string())?;
    let mut contents = Vec::new();
    response
        .into_reader()
        .take(limit as u64)
        .read_to_end(&mut contents)
        .map_err(|error| format!("failed to read GitHub response: {error}"))?;
    if contents.len() > maximum_bytes {
        return Err(format!(
            "GitHub response exceeds the {maximum_bytes}-byte limit"
        ));
    }
    Ok(contents)
}

#[derive(Deserialize)]
struct RepositoryResponse {
    default_branch: String,
}

#[derive(Deserialize)]
struct CommitResponse {
    sha: String,
    commit: CommitDetails,
}

#[derive(Deserialize)]
struct CommitDetails {
    tree: CommitTree,
}

#[derive(Deserialize)]
struct CommitTree {
    sha: String,
}

#[derive(Deserialize)]
struct TreeResponse {
    #[serde(default)]
    truncated: bool,
    tree: Vec<TreeEntry>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct TreeEntry {
    path: String,
    mode: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    size: Option<u64>,
}

#[derive(Deserialize)]
struct GitHubErrorResponse {
    message: String,
}

struct ResolvedRepository {
    revision: String,
    entries: Vec<TreeEntry>,
}

#[derive(Debug, PartialEq, Eq)]
struct PluginCandidate {
    directory: String,
    id: String,
    name: String,
    main: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CandidateManifest {
    api_version: u32,
    id: String,
    name: String,
    #[serde(default)]
    main: Option<String>,
    #[serde(default)]
    capabilities: Vec<String>,
}

fn parse_candidate(
    directory: &str,
    contents: &[u8],
    entries: &[TreeEntry],
) -> Result<PluginCandidate, String> {
    let manifest: CandidateManifest = serde_json::from_slice(contents)
        .map_err(|error| format!("invalid plugin.json: {error}"))?;
    if manifest.api_version != 1 {
        return Err("plugin.json apiVersion must be 1".to_string());
    }
    if !valid_plugin_id(&manifest.id) {
        return Err(format!("invalid plugin ID `{}`", manifest.id));
    }
    if manifest.name.trim().is_empty() || manifest.name.chars().count() > 200 {
        return Err("plugin name must contain 1 to 200 characters".to_string());
    }
    validate_plugin_capabilities(&manifest.capabilities)?;
    let main = normalize_repo_path(manifest.main.as_deref().unwrap_or("plugin.ts"), false)
        .map_err(|_| "plugin.json main must stay inside the plugin directory".to_string())?;
    let main_path = joined_repo_path(directory, &main);
    let main_entry = entries
        .iter()
        .find(|entry| entry.path == main_path)
        .ok_or_else(|| format!("plugin entrypoint does not exist: {main}"))?;
    require_regular_blob(main_entry, "plugin entrypoint")?;
    Ok(PluginCandidate {
        directory: directory.to_string(),
        id: manifest.id,
        name: manifest.name,
        main,
    })
}

fn require_regular_blob(entry: &TreeEntry, label: &str) -> Result<(), String> {
    if entry.kind != "blob" || !matches!(entry.mode.as_str(), "100644" | "100755") {
        return Err(format!(
            "{label} must be a regular file, not a symlink or submodule"
        ));
    }
    Ok(())
}

#[derive(Debug)]
struct PluginFile {
    relative_path: String,
    size: u64,
}

fn plugin_files(entries: &[TreeEntry], directory: &str) -> Result<Vec<PluginFile>, String> {
    let prefix = (!directory.is_empty()).then(|| format!("{directory}/"));
    let mut files = Vec::new();
    let mut total_bytes = 0_u64;
    let mut seen = HashSet::new();

    for entry in entries {
        let relative_path = match prefix.as_deref() {
            Some(prefix) => match entry.path.strip_prefix(prefix) {
                Some(path) if !path.is_empty() => path,
                _ => continue,
            },
            None => entry.path.as_str(),
        };
        if ignored_plugin_path(relative_path) || entry.kind == "tree" {
            continue;
        }
        require_regular_blob(entry, &format!("plugin source `{relative_path}`"))?;
        let normalized = normalize_repo_path(relative_path, false)?;
        if normalized != relative_path || !seen.insert(normalized.clone()) {
            return Err(format!(
                "plugin contains an unsafe or duplicate path `{relative_path}`"
            ));
        }
        let size = entry
            .size
            .ok_or_else(|| format!("GitHub did not report a size for `{relative_path}`"))?;
        total_bytes = total_bytes.saturating_add(size);
        if total_bytes > MAX_PLUGIN_SOURCE_BYTES {
            return Err("plugin exceeds the 16 MiB source-tree limit".to_string());
        }
        files.push(PluginFile {
            relative_path: normalized,
            size,
        });
        if files.len() > MAX_PLUGIN_SOURCE_FILES {
            return Err(format!(
                "plugin exceeds the {MAX_PLUGIN_SOURCE_FILES}-file source-tree limit"
            ));
        }
    }

    if !files.iter().any(|file| file.relative_path == "plugin.json") {
        return Err("plugin source is missing plugin.json".to_string());
    }
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(files)
}

fn ignored_repo_path(path: &str) -> bool {
    path.split('/').any(|component| {
        matches!(
            component,
            ".git" | "node_modules" | "target" | ".termy-cache"
        )
    })
}

fn ignored_plugin_path(path: &str) -> bool {
    ignored_repo_path(path)
        || path
            .split('/')
            .any(|component| matches!(component, ".termy-disabled" | ".termy-source.json"))
}

fn joined_repo_path(directory: &str, path: &str) -> String {
    if directory.is_empty() {
        path.to_string()
    } else {
        format!("{directory}/{path}")
    }
}

fn safe_destination(root: &Path, path: &str) -> Result<PathBuf, String> {
    let normalized = normalize_repo_path(path, false)?;
    if normalized != path {
        return Err(format!("plugin path is not normalized: `{path}`"));
    }
    Ok(path
        .split('/')
        .fold(root.to_path_buf(), |destination, component| {
            destination.join(component)
        }))
}

struct DownloadedPlugin {
    _temporary: TempDir,
    root: PathBuf,
}

impl DownloadedPlugin {
    fn path(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blob(path: &str, size: u64) -> TreeEntry {
        TreeEntry {
            path: path.to_string(),
            mode: "100644".to_string(),
            kind: "blob".to_string(),
            size: Some(size),
        }
    }

    #[test]
    fn parses_repository_and_tree_urls() {
        let source = GitHubSource::parse("https://github.com/termy-org/git-tools.git", None, None)
            .expect("repository URL");
        assert_eq!(source.owner, "termy-org");
        assert_eq!(source.repository, "git-tools");
        assert_eq!(source.requested_ref, None);
        assert_eq!(source.subdirectory, None);

        let source = GitHubSource::parse(
            "https://github.com/termy-org/plugins/tree/v1/git-tools",
            None,
            None,
        )
        .expect("tree URL");
        assert_eq!(source.requested_ref.as_deref(), Some("v1"));
        assert_eq!(source.subdirectory.as_deref(), Some("git-tools"));

        let tracked_root = GitHubSource::from_metadata(&PluginSourceMetadata {
            repository_url: "https://github.com/termy-org/root-plugin".to_string(),
            requested_ref: None,
            revision: "1111111111111111111111111111111111111111".to_string(),
            subdirectory: String::new(),
        })
        .expect("root source metadata");
        assert_eq!(tracked_root.subdirectory.as_deref(), Some(""));
    }

    #[test]
    fn rejects_conflicting_or_escaping_selectors() {
        let conflict = GitHubSource::parse(
            "https://github.com/termy-org/plugins/tree/main/git-tools",
            Some("next"),
            None,
        )
        .expect_err("conflicting ref");
        assert!(conflict.contains("conflicts"));

        let escaping = GitHubSource::parse(
            "https://github.com/termy-org/plugins",
            None,
            Some("../git-tools"),
        )
        .expect_err("escaping path");
        assert!(escaping.contains("stay inside"));
    }

    #[test]
    fn validates_manifest_and_default_entrypoint() {
        let entries = vec![
            blob("plugins/git/plugin.json", 70),
            blob("plugins/git/plugin.ts", 20),
        ];
        let candidate = parse_candidate(
            "plugins/git",
            br#"{"apiVersion":1,"id":"git-tools","name":"Git Tools","capabilities":["storage","native-ui"]}"#,
            &entries,
        )
        .expect("candidate");
        assert_eq!(candidate.id, "git-tools");
        assert_eq!(candidate.main, "plugin.ts");
    }

    #[test]
    fn rejects_invalid_manifest_capabilities_before_download() {
        let entries = vec![
            blob("plugins/git/plugin.json", 100),
            blob("plugins/git/plugin.ts", 20),
        ];
        let error = parse_candidate(
            "plugins/git",
            br#"{"apiVersion":1,"id":"git-tools","name":"Git Tools","capabilities":["root-access"]}"#,
            &entries,
        )
        .expect_err("unknown capability");
        assert!(error.contains("capability `root-access` is not supported"));

        let error = parse_candidate(
            "plugins/git",
            br#"{"apiVersion":1,"id":"git-tools","name":"Git Tools","capabilities":["storage","storage"]}"#,
            &entries,
        )
        .expect_err("duplicate capability");
        assert!(error.contains("capability `storage` is duplicated"));

        let error = parse_candidate(
            "plugins/git",
            br#"{"apiVersion":1,"id":"git-tools","name":"Git Tools","capabilities":"storage"}"#,
            &entries,
        )
        .expect_err("capabilities must be an array");
        assert!(error.contains("invalid plugin.json"));
    }

    #[test]
    fn plans_only_safe_bounded_plugin_files() {
        let entries = vec![
            blob("plugins/git/plugin.json", 70),
            blob("plugins/git/plugin.ts", 20),
            blob("plugins/git/node_modules/nope.ts", 100),
            TreeEntry {
                path: "plugins/git/src".to_string(),
                mode: "040000".to_string(),
                kind: "tree".to_string(),
                size: None,
            },
            blob("plugins/git/src/lib.ts", 30),
        ];
        let files = plugin_files(&entries, "plugins/git").expect("file plan");
        assert_eq!(
            files
                .iter()
                .map(|file| file.relative_path.as_str())
                .collect::<Vec<_>>(),
            vec!["plugin.json", "plugin.ts", "src/lib.ts"]
        );
    }

    #[test]
    fn rejects_symlinks_inside_plugin_source() {
        let entries = vec![
            blob("plugin.json", 70),
            blob("plugin.ts", 20),
            TreeEntry {
                path: "secret.ts".to_string(),
                mode: "120000".to_string(),
                kind: "blob".to_string(),
                size: Some(12),
            },
        ];
        let error = plugin_files(&entries, "").expect_err("symlink");
        assert!(error.contains("regular file"));
    }

    #[test]
    fn init_creates_plain_typescript_plugin_without_overwriting() {
        let temporary = tempfile::tempdir().expect("temp directory");
        let target = temporary.path().join("my-plugin");
        init(&target, None, None).expect("initialize plugin");

        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(target.join("plugin.json")).expect("read manifest"))
                .expect("parse manifest");
        assert_eq!(manifest["id"], "my-plugin");
        assert_eq!(manifest["name"], "My Plugin");
        let plugin = fs::read_to_string(target.join("plugin.ts")).expect("read plugin");
        assert!(plugin.contains("definePlugin"));
        assert!(plugin.contains("context.toasts.success"));

        fs::write(target.join("plugin.ts"), "keep me").expect("customize plugin");
        let error = init(&target, None, None).expect_err("refuse overwrite");
        assert!(error.contains("will not overwrite"));
        assert_eq!(
            fs::read_to_string(target.join("plugin.ts")).expect("preserved plugin"),
            "keep me"
        );
    }
}
