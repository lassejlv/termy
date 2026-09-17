use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;
use uuid::Uuid;

pub const KEYRING_SERVICE: &str = "com.lassevestergaard.termy.ssh";
pub const ASKPASS_MODE_ENV: &str = "TERMY_SSH_ASKPASS";
pub const ASKPASS_HOST_ID_ENV: &str = "TERMY_SSH_ASKPASS_HOST_ID";
pub const ASKPASS_SECRET_KIND_ENV: &str = "TERMY_SSH_ASKPASS_SECRET_KIND";
pub const ASKPASS_PARENT_PID_ENV: &str = "TERMY_SSH_ASKPASS_PARENT_PID";
pub const HOSTS_FILE_NAME: &str = "ssh_hosts.json";

const HOSTS_FILE_VERSION: u32 = 1;
const MAX_DISPLAY_NAME_BYTES: usize = 100;
const MAX_HOSTNAME_BYTES: usize = 255;
const MAX_USERNAME_BYTES: usize = 255;
const MAX_IDENTITY_FILE_BYTES: usize = 4096;
const MAX_SECRET_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SshAuthenticationType {
    Key,
    Password,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SshAuthentication {
    Key { identity_file: String },
    Password,
}

impl SshAuthentication {
    pub fn authentication_type(&self) -> SshAuthenticationType {
        match self {
            Self::Key { .. } => SshAuthenticationType::Key,
            Self::Password => SshAuthenticationType::Password,
        }
    }

    pub fn secret_kind(&self) -> SshSecretKind {
        match self {
            Self::Key { .. } => SshSecretKind::KeyPassphrase,
            Self::Password => SshSecretKind::Password,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SshHost {
    pub id: String,
    pub display_name: String,
    pub hostname: String,
    pub port: u16,
    pub username: String,
    pub authentication: SshAuthentication,
}

impl SshHost {
    pub fn validate(&self) -> Result<(), String> {
        validate_host_id(&self.id)?;
        validate_display_name(&self.display_name)?;
        validate_hostname(&self.hostname)?;
        validate_port(self.port)?;
        validate_username(&self.username)?;
        if let SshAuthentication::Key { identity_file } = &self.authentication {
            validate_identity_file(identity_file)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SshHostInput {
    pub display_name: String,
    pub hostname: String,
    pub port: u16,
    pub username: String,
    pub authentication: SshAuthentication,
}

impl SshHostInput {
    fn into_host(self, id: String) -> SshHost {
        let authentication = match self.authentication {
            SshAuthentication::Key { identity_file } => SshAuthentication::Key {
                identity_file: identity_file.trim().to_string(),
            },
            SshAuthentication::Password => SshAuthentication::Password,
        };
        SshHost {
            id,
            display_name: self.display_name.trim().to_string(),
            hostname: self.hostname.trim().to_string(),
            port: self.port,
            username: self.username.trim().to_string(),
            authentication,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        validate_display_name(&self.display_name)?;
        validate_hostname(&self.hostname)?;
        validate_port(self.port)?;
        validate_username(&self.username)?;
        if let SshAuthentication::Key { identity_file } = &self.authentication {
            validate_identity_file(identity_file)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SshSecretKind {
    Password,
    KeyPassphrase,
}

impl SshSecretKind {
    pub const fn as_key(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::KeyPassphrase => "key_passphrase",
        }
    }

    pub fn from_key(value: &str) -> Option<Self> {
        match value {
            "password" => Some(Self::Password),
            "key_passphrase" => Some(Self::KeyPassphrase),
            _ => None,
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub enum SecretUpdate {
    Keep,
    Set(String),
    Clear,
}

pub trait KeyringBackend {
    fn get_password(&self, service: &str, account: &str) -> Result<Option<String>, String>;
    fn set_password(&self, service: &str, account: &str, secret: &str) -> Result<(), String>;
    fn delete_password(&self, service: &str, account: &str) -> Result<(), String>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemKeyringBackend;

impl SystemKeyringBackend {
    fn entry(service: &str, account: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(service, account)
            .map_err(|error| format!("Failed to access the system keychain: {error}"))
    }
}

impl KeyringBackend for SystemKeyringBackend {
    fn get_password(&self, service: &str, account: &str) -> Result<Option<String>, String> {
        match Self::entry(service, account)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(format!("Failed to read the SSH credential: {error}")),
        }
    }

    fn set_password(&self, service: &str, account: &str, secret: &str) -> Result<(), String> {
        Self::entry(service, account)?
            .set_password(secret)
            .map_err(|error| format!("Failed to save the SSH credential: {error}"))
    }

    fn delete_password(&self, service: &str, account: &str) -> Result<(), String> {
        match Self::entry(service, account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(format!("Failed to delete the SSH credential: {error}")),
        }
    }
}

pub struct SshSecretStore<B> {
    backend: B,
}

impl<B: KeyringBackend> SshSecretStore<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }

    pub fn read(&self, host_id: &str, kind: SshSecretKind) -> Result<Option<String>, String> {
        let account = secret_account(host_id, kind)?;
        self.backend.get_password(KEYRING_SERVICE, &account)
    }

    pub fn has(&self, host_id: &str, kind: SshSecretKind) -> Result<bool, String> {
        Ok(self.read(host_id, kind)?.is_some())
    }

    pub fn write(&self, host_id: &str, kind: SshSecretKind, secret: &str) -> Result<(), String> {
        validate_secret(secret)?;
        let account = secret_account(host_id, kind)?;
        self.backend.set_password(KEYRING_SERVICE, &account, secret)
    }

    pub fn clear(&self, host_id: &str, kind: SshSecretKind) -> Result<(), String> {
        let account = secret_account(host_id, kind)?;
        self.backend.delete_password(KEYRING_SERVICE, &account)
    }
}

pub fn secret_account(host_id: &str, kind: SshSecretKind) -> Result<String, String> {
    validate_host_id(host_id)?;
    Ok(format!("host.{host_id}.{}", kind.as_key()))
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct HostsFile {
    version: u32,
    hosts: Vec<SshHost>,
}

pub struct SshHostManager<B> {
    path: PathBuf,
    hosts: Vec<SshHost>,
    secrets: SshSecretStore<B>,
}

impl<B: KeyringBackend> SshHostManager<B> {
    pub fn open(path: impl Into<PathBuf>, backend: B) -> Result<Self, String> {
        let path = path.into();
        let hosts = load_hosts(&path)?;
        Ok(Self {
            path,
            hosts,
            secrets: SshSecretStore::new(backend),
        })
    }

    pub fn hosts(&self) -> &[SshHost] {
        &self.hosts
    }

    pub fn has_secret(&self, host_id: &str, kind: SshSecretKind) -> Result<bool, String> {
        self.secrets.has(host_id, kind)
    }

    pub fn create(
        &mut self,
        input: SshHostInput,
        secret_update: SecretUpdate,
    ) -> Result<SshHost, String> {
        let host = input.into_host(Uuid::new_v4().to_string());
        host.validate()?;
        self.ensure_unique_display_name(&host.display_name, None)?;

        let uses_keyring = !matches!(&secret_update, SecretUpdate::Keep);
        let snapshot = uses_keyring
            .then(|| self.secret_snapshot(&host.id))
            .transpose()?;
        if uses_keyring
            && let Err(error) =
                self.apply_secret_transition(&host.id, None, &host.authentication, secret_update)
        {
            return Err(snapshot.as_ref().map_or(error.clone(), |snapshot| {
                self.rollback_error(&host.id, snapshot, error)
            }));
        }

        let mut next_hosts = self.hosts.clone();
        next_hosts.push(host.clone());
        if let Err(error) = write_hosts(&self.path, &next_hosts) {
            return Err(snapshot.as_ref().map_or(error.clone(), |snapshot| {
                self.rollback_error(&host.id, snapshot, error)
            }));
        }
        self.hosts = next_hosts;
        Ok(host)
    }

    pub fn update(
        &mut self,
        host_id: &str,
        input: SshHostInput,
        secret_update: SecretUpdate,
    ) -> Result<SshHost, String> {
        validate_host_id(host_id)?;
        let index = self
            .hosts
            .iter()
            .position(|host| host.id == host_id)
            .ok_or_else(|| "The saved SSH host no longer exists".to_string())?;
        let previous = self.hosts[index].clone();
        let host = input.into_host(host_id.to_string());
        host.validate()?;
        self.ensure_unique_display_name(&host.display_name, Some(host_id))?;

        let uses_keyring = previous.authentication.secret_kind()
            != host.authentication.secret_kind()
            || !matches!(&secret_update, SecretUpdate::Keep);
        let snapshot = uses_keyring
            .then(|| self.secret_snapshot(host_id))
            .transpose()?;
        if uses_keyring
            && let Err(error) = self.apply_secret_transition(
                host_id,
                Some(&previous.authentication),
                &host.authentication,
                secret_update,
            )
        {
            return Err(snapshot.as_ref().map_or(error.clone(), |snapshot| {
                self.rollback_error(host_id, snapshot, error)
            }));
        }

        let mut next_hosts = self.hosts.clone();
        next_hosts[index] = host.clone();
        if let Err(error) = write_hosts(&self.path, &next_hosts) {
            return Err(snapshot.as_ref().map_or(error.clone(), |snapshot| {
                self.rollback_error(host_id, snapshot, error)
            }));
        }
        self.hosts = next_hosts;
        Ok(host)
    }

    pub fn delete(&mut self, host_id: &str) -> Result<SshHost, String> {
        validate_host_id(host_id)?;
        let index = self
            .hosts
            .iter()
            .position(|host| host.id == host_id)
            .ok_or_else(|| "The saved SSH host no longer exists".to_string())?;
        let removed = self.hosts[index].clone();
        let snapshot = self.secret_snapshot(host_id)?;

        if let Err(error) = self.clear_all_secrets(host_id) {
            return Err(self.rollback_error(host_id, &snapshot, error));
        }

        let mut next_hosts = self.hosts.clone();
        next_hosts.remove(index);
        if let Err(error) = write_hosts(&self.path, &next_hosts) {
            return Err(self.rollback_error(host_id, &snapshot, error));
        }
        self.hosts = next_hosts;
        Ok(removed)
    }

    fn ensure_unique_display_name(
        &self,
        display_name: &str,
        except_host_id: Option<&str>,
    ) -> Result<(), String> {
        if self.hosts.iter().any(|host| {
            Some(host.id.as_str()) != except_host_id
                && host.display_name.eq_ignore_ascii_case(display_name)
        }) {
            return Err("An SSH host with that display name already exists".to_string());
        }
        Ok(())
    }

    fn apply_secret_transition(
        &self,
        host_id: &str,
        previous: Option<&SshAuthentication>,
        next: &SshAuthentication,
        update: SecretUpdate,
    ) -> Result<(), String> {
        let next_kind = next.secret_kind();
        let kind_changed = previous.is_some_and(|auth| auth.secret_kind() != next_kind);
        if kind_changed {
            self.clear_all_secrets(host_id)?;
        }
        match update {
            SecretUpdate::Keep => Ok(()),
            SecretUpdate::Set(secret) => self.secrets.write(host_id, next_kind, &secret),
            SecretUpdate::Clear => self.secrets.clear(host_id, next_kind),
        }
    }

    fn secret_snapshot(&self, host_id: &str) -> Result<SecretSnapshot, String> {
        Ok(SecretSnapshot {
            password: self.secrets.read(host_id, SshSecretKind::Password)?,
            key_passphrase: self.secrets.read(host_id, SshSecretKind::KeyPassphrase)?,
        })
    }

    fn restore_secret_snapshot(
        &self,
        host_id: &str,
        snapshot: &SecretSnapshot,
    ) -> Result<(), String> {
        self.clear_all_secrets(host_id)?;
        if let Some(password) = snapshot.password.as_deref() {
            self.secrets
                .write(host_id, SshSecretKind::Password, password)?;
        }
        if let Some(passphrase) = snapshot.key_passphrase.as_deref() {
            self.secrets
                .write(host_id, SshSecretKind::KeyPassphrase, passphrase)?;
        }
        Ok(())
    }

    fn clear_all_secrets(&self, host_id: &str) -> Result<(), String> {
        self.secrets.clear(host_id, SshSecretKind::Password)?;
        self.secrets.clear(host_id, SshSecretKind::KeyPassphrase)
    }

    fn rollback_error(&self, host_id: &str, snapshot: &SecretSnapshot, error: String) -> String {
        match self.restore_secret_snapshot(host_id, snapshot) {
            Ok(()) => error,
            Err(rollback_error) => {
                format!("{error}. The SSH credential rollback also failed: {rollback_error}")
            }
        }
    }
}

#[derive(Clone, Default)]
struct SecretSnapshot {
    password: Option<String>,
    key_passphrase: Option<String>,
}

pub fn load_hosts(path: &Path) -> Result<Vec<SshHost>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("Failed to read saved SSH hosts: {error}"))?;
    let file: HostsFile = serde_json::from_str(&contents)
        .map_err(|error| format!("Failed to parse saved SSH hosts: {error}"))?;
    if file.version != HOSTS_FILE_VERSION {
        return Err(format!(
            "Unsupported saved SSH host format version {}",
            file.version
        ));
    }
    validate_host_collection(&file.hosts)?;
    Ok(file.hosts)
}

fn write_hosts(path: &Path, hosts: &[SshHost]) -> Result<(), String> {
    validate_host_collection(hosts)?;
    let parent = path
        .parent()
        .ok_or_else(|| "Saved SSH host path has no parent directory".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Failed to create the SSH settings directory: {error}"))?;
    let payload = serde_json::to_string_pretty(&HostsFile {
        version: HOSTS_FILE_VERSION,
        hosts: hosts.to_vec(),
    })
    .map_err(|error| format!("Failed to serialize saved SSH hosts: {error}"))?;
    let mut temp = NamedTempFile::new_in(parent)
        .map_err(|error| format!("Failed to create a temporary SSH settings file: {error}"))?;
    temp.write_all(payload.as_bytes())
        .map_err(|error| format!("Failed to write saved SSH hosts: {error}"))?;
    temp.write_all(b"\n")
        .map_err(|error| format!("Failed to finish saved SSH hosts: {error}"))?;
    temp.flush()
        .map_err(|error| format!("Failed to flush saved SSH hosts: {error}"))?;
    temp.as_file()
        .sync_all()
        .map_err(|error| format!("Failed to sync saved SSH hosts: {error}"))?;
    temp.persist(path)
        .map_err(|error| format!("Failed to replace saved SSH hosts: {}", error.error))?;
    Ok(())
}

fn validate_host_collection(hosts: &[SshHost]) -> Result<(), String> {
    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    for host in hosts {
        host.validate()?;
        if !ids.insert(host.id.as_str()) {
            return Err("Saved SSH hosts contain a duplicate stable ID".to_string());
        }
        if !names.insert(host.display_name.to_ascii_lowercase()) {
            return Err("Saved SSH hosts contain a duplicate display name".to_string());
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SshProcessLaunch {
    pub program: String,
    pub args: Vec<String>,
}

pub fn openssh_launch(host: &SshHost) -> Result<SshProcessLaunch, String> {
    host.validate()?;
    let mut args = vec![
        "-p".to_string(),
        host.port.to_string(),
        "-l".to_string(),
        host.username.clone(),
    ];
    match &host.authentication {
        SshAuthentication::Key { identity_file } => {
            args.extend([
                "-i".to_string(),
                identity_file.clone(),
                "-o".to_string(),
                "IdentitiesOnly=yes".to_string(),
            ]);
        }
        SshAuthentication::Password => {
            args.extend([
                "-o".to_string(),
                "PreferredAuthentications=password,keyboard-interactive".to_string(),
                "-o".to_string(),
                "PubkeyAuthentication=no".to_string(),
            ]);
        }
    }
    args.push("--".to_string());
    args.push(host.hostname.clone());
    Ok(SshProcessLaunch {
        program: if cfg!(target_os = "windows") {
            "ssh.exe".to_string()
        } else {
            "ssh".to_string()
        },
        args,
    })
}

pub fn askpass_environment(
    executable: &Path,
    parent_pid: u32,
    host: &SshHost,
) -> Result<HashMap<String, String>, String> {
    host.validate()?;
    if parent_pid == 0 {
        return Err("Unable to configure SSH credential access for this process".to_string());
    }
    let executable = executable
        .to_str()
        .filter(|path| !path.is_empty())
        .ok_or_else(|| "The Termy executable path is not valid UTF-8".to_string())?;
    Ok(HashMap::from([
        ("SSH_ASKPASS".to_string(), executable.to_string()),
        ("SSH_ASKPASS_REQUIRE".to_string(), "force".to_string()),
        (ASKPASS_MODE_ENV.to_string(), "1".to_string()),
        (ASKPASS_HOST_ID_ENV.to_string(), host.id.clone()),
        (
            ASKPASS_SECRET_KIND_ENV.to_string(),
            host.authentication.secret_kind().as_key().to_string(),
        ),
        (ASKPASS_PARENT_PID_ENV.to_string(), parent_pid.to_string()),
    ]))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AskpassPromptKind {
    Authentication,
    HostKeyConfirmation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AskpassRequest {
    pub host_id: String,
    pub secret_kind: SshSecretKind,
    pub expected_parent_pid: u32,
    pub prompt_kind: AskpassPromptKind,
}

pub fn parse_askpass_request(
    mut get_env: impl FnMut(&str) -> Option<String>,
    prompt: &str,
) -> Result<Option<AskpassRequest>, String> {
    if get_env(ASKPASS_MODE_ENV).as_deref() != Some("1") {
        return Ok(None);
    }
    let prompt_kind = classify_askpass_prompt(prompt)
        .ok_or_else(|| "Termy refused an unsupported SSH prompt".to_string())?;
    let host_id = get_env(ASKPASS_HOST_ID_ENV)
        .ok_or_else(|| "The SSH credential request is missing its host ID".to_string())?;
    validate_host_id(&host_id)?;
    let secret_kind = get_env(ASKPASS_SECRET_KIND_ENV)
        .as_deref()
        .and_then(SshSecretKind::from_key)
        .ok_or_else(|| "The SSH credential request has an invalid credential type".to_string())?;
    let expected_parent_pid = get_env(ASKPASS_PARENT_PID_ENV)
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|pid| *pid > 0)
        .ok_or_else(|| "The SSH credential request has an invalid parent process".to_string())?;
    Ok(Some(AskpassRequest {
        host_id,
        secret_kind,
        expected_parent_pid,
        prompt_kind,
    }))
}

pub fn resolve_askpass_secret<B: KeyringBackend>(
    request: &AskpassRequest,
    backend: B,
) -> Result<String, String> {
    SshSecretStore::new(backend)
        .read(&request.host_id, request.secret_kind)?
        .ok_or_else(|| "No saved SSH credential was found in the system keychain".to_string())
}

pub fn is_authentication_prompt(prompt: &str) -> bool {
    let prompt = prompt.to_ascii_lowercase();
    prompt.contains("password") || prompt.contains("passphrase")
}

pub fn is_host_key_confirmation_prompt(prompt: &str) -> bool {
    let prompt = prompt.to_ascii_lowercase();
    prompt.contains("are you sure you want to continue connecting")
        || prompt.contains("please type 'yes', 'no' or the fingerprint")
}

fn classify_askpass_prompt(prompt: &str) -> Option<AskpassPromptKind> {
    if is_host_key_confirmation_prompt(prompt) {
        Some(AskpassPromptKind::HostKeyConfirmation)
    } else if is_authentication_prompt(prompt) {
        Some(AskpassPromptKind::Authentication)
    } else {
        None
    }
}

fn validate_host_id(host_id: &str) -> Result<(), String> {
    let parsed = Uuid::parse_str(host_id)
        .map_err(|_| "Saved SSH host ID is not a valid UUID".to_string())?;
    if parsed.hyphenated().to_string() != host_id.to_ascii_lowercase() {
        return Err("Saved SSH host ID is not in canonical UUID form".to_string());
    }
    Ok(())
}

fn validate_display_name(value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("Display name is required".to_string());
    }
    if value.len() > MAX_DISPLAY_NAME_BYTES {
        return Err(format!(
            "Display name must be at most {MAX_DISPLAY_NAME_BYTES} bytes"
        ));
    }
    if value.chars().any(char::is_control) {
        return Err("Display name cannot contain control characters".to_string());
    }
    Ok(())
}

fn validate_hostname(value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("Hostname is required".to_string());
    }
    if value.len() > MAX_HOSTNAME_BYTES {
        return Err(format!(
            "Hostname must be at most {MAX_HOSTNAME_BYTES} bytes"
        ));
    }
    if value.starts_with('-') {
        return Err("Hostname cannot start with '-'".to_string());
    }
    if !value.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_' | ':' | '%')
    }) {
        return Err(
            "Hostname may only contain letters, numbers, '.', '-', '_', ':', and '%'".to_string(),
        );
    }
    Ok(())
}

fn validate_port(port: u16) -> Result<(), String> {
    if port == 0 {
        return Err("Port must be between 1 and 65535".to_string());
    }
    Ok(())
}

fn validate_username(value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("Username is required".to_string());
    }
    if value.len() > MAX_USERNAME_BYTES {
        return Err(format!(
            "Username must be at most {MAX_USERNAME_BYTES} bytes"
        ));
    }
    if value.starts_with('-') {
        return Err("Username cannot start with '-'".to_string());
    }
    if !value.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_' | '@' | '+' | '\\')
    }) {
        return Err("Username contains unsupported whitespace or punctuation".to_string());
    }
    Ok(())
}

fn validate_identity_file(value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("Identity file is required for key authentication".to_string());
    }
    if value.len() > MAX_IDENTITY_FILE_BYTES {
        return Err(format!(
            "Identity file path must be at most {MAX_IDENTITY_FILE_BYTES} bytes"
        ));
    }
    if value
        .chars()
        .any(|character| matches!(character, '\0' | '\n' | '\r'))
    {
        return Err("Identity file path cannot contain line breaks or NUL bytes".to_string());
    }
    Ok(())
}

fn validate_secret(secret: &str) -> Result<(), String> {
    if secret.is_empty() {
        return Err("SSH credential cannot be empty".to_string());
    }
    if secret.len() > MAX_SECRET_BYTES {
        return Err(format!(
            "SSH credential must be at most {MAX_SECRET_BYTES} bytes"
        ));
    }
    if secret
        .chars()
        .any(|character| matches!(character, '\0' | '\n' | '\r'))
    {
        return Err("SSH credential cannot contain line breaks or NUL bytes".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct MockKeyring {
        values: Arc<Mutex<HashMap<(String, String), String>>>,
    }

    impl MockKeyring {
        fn value(&self, account: &str) -> Option<String> {
            self.values
                .lock()
                .expect("mock keyring lock")
                .get(&(KEYRING_SERVICE.to_string(), account.to_string()))
                .cloned()
        }
    }

    impl KeyringBackend for MockKeyring {
        fn get_password(&self, service: &str, account: &str) -> Result<Option<String>, String> {
            Ok(self
                .values
                .lock()
                .expect("mock keyring lock")
                .get(&(service.to_string(), account.to_string()))
                .cloned())
        }

        fn set_password(&self, service: &str, account: &str, secret: &str) -> Result<(), String> {
            self.values.lock().expect("mock keyring lock").insert(
                (service.to_string(), account.to_string()),
                secret.to_string(),
            );
            Ok(())
        }

        fn delete_password(&self, service: &str, account: &str) -> Result<(), String> {
            self.values
                .lock()
                .expect("mock keyring lock")
                .remove(&(service.to_string(), account.to_string()));
            Ok(())
        }
    }

    #[derive(Clone, Copy)]
    struct UnavailableKeyring;

    impl KeyringBackend for UnavailableKeyring {
        fn get_password(&self, _service: &str, _account: &str) -> Result<Option<String>, String> {
            Err("keychain unavailable".to_string())
        }

        fn set_password(
            &self,
            _service: &str,
            _account: &str,
            _secret: &str,
        ) -> Result<(), String> {
            Err("keychain unavailable".to_string())
        }

        fn delete_password(&self, _service: &str, _account: &str) -> Result<(), String> {
            Err("keychain unavailable".to_string())
        }
    }

    #[derive(Clone, Copy)]
    struct FailingMutationKeyring;

    impl KeyringBackend for FailingMutationKeyring {
        fn get_password(&self, _service: &str, _account: &str) -> Result<Option<String>, String> {
            Ok(None)
        }

        fn set_password(
            &self,
            _service: &str,
            _account: &str,
            _secret: &str,
        ) -> Result<(), String> {
            Err("keychain mutation failed".to_string())
        }

        fn delete_password(&self, _service: &str, _account: &str) -> Result<(), String> {
            Err("keychain mutation failed".to_string())
        }
    }

    fn key_input(identity_file: &str) -> SshHostInput {
        SshHostInput {
            display_name: "Production".to_string(),
            hostname: "prod.example.com".to_string(),
            port: 22,
            username: "deploy".to_string(),
            authentication: SshAuthentication::Key {
                identity_file: identity_file.to_string(),
            },
        }
    }

    fn password_input() -> SshHostInput {
        SshHostInput {
            authentication: SshAuthentication::Password,
            ..key_input("unused")
        }
    }

    #[test]
    fn validates_hosts_and_rejects_shell_shaped_hostname_input() {
        key_input("~/.ssh/id_ed25519")
            .validate()
            .expect("valid key host");

        let mut hostile = password_input();
        hostile.hostname = "example.com;touch /tmp/pwned".to_string();
        assert_eq!(
            hostile.validate().unwrap_err(),
            "Hostname may only contain letters, numbers, '.', '-', '_', ':', and '%'"
        );
    }

    #[test]
    fn serializes_only_non_secret_host_data() {
        let host = key_input("/Users/alice/.ssh/id_ed25519").into_host(Uuid::new_v4().to_string());
        let json = serde_json::to_string(&host).expect("serialize host");
        assert!(json.contains("identity_file"));
        assert!(!json.contains("password"));
        assert!(!json.contains("passphrase"));
    }

    #[test]
    fn keyring_accounts_are_stable_and_scoped_by_secret_kind() {
        let id = "550e8400-e29b-41d4-a716-446655440000";
        assert_eq!(
            secret_account(id, SshSecretKind::Password).unwrap(),
            "host.550e8400-e29b-41d4-a716-446655440000.password"
        );
        assert_eq!(
            secret_account(id, SshSecretKind::KeyPassphrase).unwrap(),
            "host.550e8400-e29b-41d4-a716-446655440000.key_passphrase"
        );
    }

    #[test]
    fn crud_and_secret_lifecycle_use_injectable_keyring() {
        let dir = tempfile::tempdir().expect("temp dir");
        let hosts_path = dir.path().join(HOSTS_FILE_NAME);
        let keyring = MockKeyring::default();
        let mut manager = SshHostManager::open(&hosts_path, keyring.clone()).expect("open manager");

        let created = manager
            .create(password_input(), SecretUpdate::Set("hunter2".to_string()))
            .expect("create host");
        let password_account = secret_account(&created.id, SshSecretKind::Password).unwrap();
        assert_eq!(keyring.value(&password_account).as_deref(), Some("hunter2"));
        assert!(
            !fs::read_to_string(&hosts_path)
                .expect("saved host file")
                .contains("hunter2")
        );
        assert_eq!(manager.hosts().len(), 1);

        let updated = manager
            .update(
                &created.id,
                key_input("/tmp/key with spaces"),
                SecretUpdate::Set("key phrase".to_string()),
            )
            .expect("update host");
        let passphrase_account = secret_account(&created.id, SshSecretKind::KeyPassphrase).unwrap();
        assert_eq!(
            updated.authentication.authentication_type(),
            SshAuthenticationType::Key
        );
        assert_eq!(keyring.value(&password_account), None);
        assert_eq!(
            keyring.value(&passphrase_account).as_deref(),
            Some("key phrase")
        );

        manager.delete(&created.id).expect("delete host");
        assert!(manager.hosts().is_empty());
        assert_eq!(keyring.value(&password_account), None);
        assert_eq!(keyring.value(&passphrase_account), None);
    }

    #[test]
    fn saved_hosts_round_trip_and_state_transitions_persist() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(HOSTS_FILE_NAME);
        let mut manager =
            SshHostManager::open(&path, MockKeyring::default()).expect("open manager");
        let created = manager
            .create(key_input("~/.ssh/id_ed25519"), SecretUpdate::Keep)
            .expect("create host");
        drop(manager);

        let mut reloaded =
            SshHostManager::open(&path, MockKeyring::default()).expect("reload manager");
        assert_eq!(reloaded.hosts(), std::slice::from_ref(&created));
        let mut changed = key_input("~/.ssh/id_ed25519");
        changed.hostname = "10.0.0.8".to_string();
        reloaded
            .update(&created.id, changed, SecretUpdate::Keep)
            .expect("update host");
        assert_eq!(load_hosts(&path).unwrap()[0].hostname, "10.0.0.8");
        reloaded.delete(&created.id).expect("delete host");
        assert!(load_hosts(&path).unwrap().is_empty());
    }

    #[test]
    fn interactive_only_create_and_edit_do_not_require_keychain_access() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut manager =
            SshHostManager::open(dir.path().join(HOSTS_FILE_NAME), UnavailableKeyring)
                .expect("open manager");
        let created = manager
            .create(password_input(), SecretUpdate::Keep)
            .expect("create interactive-only host");
        let mut changed = password_input();
        changed.hostname = "staging.example.com".to_string();
        manager
            .update(&created.id, changed, SecretUpdate::Keep)
            .expect("edit interactive-only host");
        assert_eq!(manager.hosts()[0].hostname, "staging.example.com");
    }

    #[test]
    fn credential_mutation_errors_include_failed_rollback() {
        const EXPECTED: &str = "keychain mutation failed. The SSH credential rollback also failed: keychain mutation failed";

        let create_dir = tempfile::tempdir().expect("create temp dir");
        let mut create_manager = SshHostManager::open(
            create_dir.path().join(HOSTS_FILE_NAME),
            FailingMutationKeyring,
        )
        .expect("open create manager");
        let create_error = create_manager
            .create(password_input(), SecretUpdate::Set("hunter2".to_string()))
            .expect_err("create should report the failed credential rollback");
        assert_eq!(create_error, EXPECTED);

        let update_dir = tempfile::tempdir().expect("update temp dir");
        let mut update_manager = SshHostManager::open(
            update_dir.path().join(HOSTS_FILE_NAME),
            FailingMutationKeyring,
        )
        .expect("open update manager");
        let update_host = update_manager
            .create(password_input(), SecretUpdate::Keep)
            .expect("create update fixture");
        let update_error = update_manager
            .update(
                &update_host.id,
                key_input("~/.ssh/id_ed25519"),
                SecretUpdate::Keep,
            )
            .expect_err("update should report the failed credential rollback");
        assert_eq!(update_error, EXPECTED);

        let delete_dir = tempfile::tempdir().expect("delete temp dir");
        let mut delete_manager = SshHostManager::open(
            delete_dir.path().join(HOSTS_FILE_NAME),
            FailingMutationKeyring,
        )
        .expect("open delete manager");
        let delete_host = delete_manager
            .create(password_input(), SecretUpdate::Keep)
            .expect("create delete fixture");
        let delete_error = delete_manager
            .delete(&delete_host.id)
            .expect_err("delete should report the failed credential rollback");
        assert_eq!(delete_error, EXPECTED);
    }

    #[test]
    fn openssh_argv_keeps_hostile_key_path_in_one_argument() {
        let hostile_path = "/tmp/key $(touch /tmp/pwned); 'quoted value'";
        let host = key_input(hostile_path).into_host(Uuid::new_v4().to_string());
        let launch = openssh_launch(&host).expect("build launch");
        assert_eq!(
            launch.program,
            if cfg!(windows) { "ssh.exe" } else { "ssh" }
        );
        assert_eq!(
            launch.args,
            vec![
                "-p",
                "22",
                "-l",
                "deploy",
                "-i",
                hostile_path,
                "-o",
                "IdentitiesOnly=yes",
                "--",
                "prod.example.com",
            ]
        );
        assert!(
            !launch
                .args
                .iter()
                .any(|arg| arg.contains("StrictHostKeyChecking"))
        );
    }

    #[test]
    fn password_argv_is_exact_and_preserves_host_key_defaults() {
        let host = password_input().into_host(Uuid::new_v4().to_string());
        let launch = openssh_launch(&host).expect("build launch");
        assert_eq!(
            launch.args,
            vec![
                "-p",
                "22",
                "-l",
                "deploy",
                "-o",
                "PreferredAuthentications=password,keyboard-interactive",
                "-o",
                "PubkeyAuthentication=no",
                "--",
                "prod.example.com",
            ]
        );
        assert!(
            !launch
                .args
                .iter()
                .any(|arg| arg.contains("StrictHostKeyChecking"))
        );
    }

    #[test]
    fn askpass_environment_contains_only_safe_references() {
        let host = password_input().into_host(Uuid::new_v4().to_string());
        let environment = askpass_environment(Path::new("/Applications/Termy"), 42, &host)
            .expect("askpass environment");
        assert_eq!(environment.get(ASKPASS_HOST_ID_ENV), Some(&host.id));
        assert_eq!(
            environment.get(ASKPASS_SECRET_KIND_ENV).map(String::as_str),
            Some("password")
        );
        assert_eq!(
            environment.get(ASKPASS_PARENT_PID_ENV).map(String::as_str),
            Some("42")
        );
        assert!(!environment.contains_key("DISPLAY"));
        assert!(!environment.values().any(|value| value == "hunter2"));
    }

    #[test]
    fn askpass_distinguishes_authentication_and_host_key_prompts() {
        assert!(is_authentication_prompt("deploy@example.com's password:"));
        assert!(is_authentication_prompt(
            "Enter passphrase for key '/tmp/id':"
        ));
        let host_key_prompt =
            "Are you sure you want to continue connecting (yes/no/[fingerprint])?";
        assert!(!is_authentication_prompt(host_key_prompt));
        assert!(is_host_key_confirmation_prompt(host_key_prompt));
    }

    #[test]
    fn askpass_request_classifies_host_key_confirmation_without_disabling_checks() {
        let host = password_input().into_host(Uuid::new_v4().to_string());
        let environment = askpass_environment(Path::new("/Applications/Termy"), 42, &host).unwrap();
        let request = parse_askpass_request(
            |key| environment.get(key).cloned(),
            "Are you sure you want to continue connecting (yes/no/[fingerprint])?",
        )
        .unwrap()
        .unwrap();
        assert_eq!(request.prompt_kind, AskpassPromptKind::HostKeyConfirmation);
    }
}
