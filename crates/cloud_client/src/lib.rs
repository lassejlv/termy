//! Client for the Termy cloud API (`termy_api`).
//!
//! Owns the device-authorization login flow, the on-disk session file shared
//! with the desktop app (`cloud_auth.json`, 0600), and typed blocking calls
//! for the `/api` surface (projects, sandbox sessions, provider status).
//! Blocking by design: callers are CLIs or already run off the UI thread.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use url::Url;

pub const DEFAULT_CLOUD_API_URL: &str = "https://app.termy.sh";
const SESSION_FILE_NAME: &str = "cloud_auth.json";

pub fn api_base_url() -> String {
    std::env::var("TERMY_CLOUD_API_URL").unwrap_or_else(|_| DEFAULT_CLOUD_API_URL.to_string())
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct CloudUser {
    pub id: String,
    pub email: String,
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct CloudSession {
    pub token: String,
    pub user: CloudUser,
}

#[derive(Clone, Debug)]
pub struct DeviceAuthorization {
    device_code: String,
    pub user_code: String,
    pub verification_url: String,
    expires_in: u64,
    interval: u64,
}

#[derive(serde::Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
}

#[derive(serde::Deserialize)]
struct DeviceTokenResponse {
    access_token: String,
}

#[derive(serde::Deserialize)]
struct ErrorResponse {
    message: Option<String>,
    code: Option<String>,
}

#[derive(Debug)]
pub struct ApiError {
    pub status: u16,
    pub code: Option<String>,
    pub message: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ApiError {}

// ---------------------------------------------------------------------------
// Session file (shared with the desktop app)

/// `cloud_auth.json` next to the Termy config file; pass the config file path
/// (e.g. from `termy_config_core::config_path()`).
pub fn session_path_for(config_path: &Path) -> Option<PathBuf> {
    Some(config_path.parent()?.join(SESSION_FILE_NAME))
}

pub fn load_session(session_path: &Path) -> Result<Option<CloudSession>, String> {
    if !session_path.exists() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(session_path)
        .map_err(|error| format!("Failed to read cloud session: {error}"))?;
    serde_json::from_str(&contents)
        .map(Some)
        .map_err(|error| format!("Invalid cloud session: {error}"))
}

pub fn save_session(session_path: &Path, session: &CloudSession) -> Result<(), String> {
    if let Some(parent) = session_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create config directory: {error}"))?;
    }
    let contents = serde_json::to_string_pretty(session)
        .map_err(|error| format!("Failed to serialize cloud session: {error}"))?;
    std::fs::write(session_path, contents)
        .map_err(|error| format!("Failed to save cloud session: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(session_path, std::fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("Failed to protect cloud session: {error}"))?;
    }
    Ok(())
}

pub fn clear_session(session_path: &Path) -> Result<(), String> {
    if session_path.exists() {
        std::fs::remove_file(session_path)
            .map_err(|error| format!("Failed to clear cloud session: {error}"))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Device-authorization login

pub fn start_device_login(api_base: &str, client_id: &str) -> Result<DeviceAuthorization, String> {
    let url = format!("{}/auth/device/code", api_base.trim_end_matches('/'));
    let response = ureq::post(&url)
        .set("Accept", "application/json")
        .send_json(serde_json::json!({
            "client_id": client_id,
            "scope": "profile email",
        }))
        .map_err(|error| request_error("Could not start browser login", error))?;
    let response: DeviceCodeResponse = response
        .into_json()
        .map_err(|error| format!("Invalid device login response: {error}"))?;
    if response.device_code.is_empty() || response.user_code.is_empty() {
        return Err("The server returned an invalid device code".to_string());
    }
    Ok(DeviceAuthorization {
        verification_url: device_verification_url(
            api_base,
            &response.verification_uri,
            &response.user_code,
        )?,
        device_code: response.device_code,
        user_code: response.user_code,
        expires_in: response.expires_in,
        interval: response.interval.max(1),
    })
}

pub fn poll_device_login(
    api_base: &str,
    authorization: &DeviceAuthorization,
) -> Result<CloudSession, String> {
    let token_url = format!("{}/auth/device/token", api_base.trim_end_matches('/'));
    let deadline = Instant::now() + Duration::from_secs(authorization.expires_in);
    let mut interval = authorization.interval;

    let token = loop {
        std::thread::sleep(Duration::from_secs(interval));
        if Instant::now() >= deadline {
            return Err("Browser login expired. Try again.".to_string());
        }
        match ureq::post(&token_url)
            .set("Accept", "application/json")
            .send_json(serde_json::json!({ "device_code": authorization.device_code }))
        {
            Ok(response) => {
                let response: DeviceTokenResponse = response
                    .into_json()
                    .map_err(|error| format!("Invalid device token response: {error}"))?;
                if response.access_token.is_empty() {
                    return Err("The server returned an empty session token".to_string());
                }
                break response.access_token;
            }
            Err(ureq::Error::Status(400, response)) => {
                let code = response_error_message(response);
                match code.as_str() {
                    "authorization_pending" => {}
                    "slow_down" => interval = interval.saturating_add(5),
                    "access_denied" => return Err("Browser login was denied".to_string()),
                    "expired_token" | "invalid_grant" => {
                        return Err("Browser login expired. Try again.".to_string());
                    }
                    _ => return Err(format!("Browser login failed: {code}")),
                }
            }
            Err(error) => return Err(request_error("Browser login failed", error)),
        }
    };

    let client = CloudClient::new(api_base.to_string(), token.clone());
    let user = client
        .me()
        .map_err(|error| format!("Could not load cloud account: {error}"))?;
    Ok(CloudSession { token, user })
}

pub fn sign_out(api_base: &str, token: &str) -> Result<(), String> {
    let url = format!("{}/auth/sign-out", api_base.trim_end_matches('/'));
    match ureq::post(&url)
        .set("Accept", "application/json")
        .set("Authorization", &format!("Bearer {}", token.trim()))
        .send_json(serde_json::json!({}))
    {
        Ok(_) | Err(ureq::Error::Status(401, _)) => Ok(()),
        Err(error) => Err(request_error("Sign out failed", error)),
    }
}

// ---------------------------------------------------------------------------
// Typed API client

pub struct CloudClient {
    base_url: String,
    token: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct RailwayStatus {
    pub connected: bool,
    pub account_name: Option<String>,
    pub expires_at: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct ActiveSessionSummary {
    pub id: String,
    pub status: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub repo_url: String,
    pub default_branch: String,
    pub setup_command: Option<String>,
    pub active_session: Option<ActiveSessionSummary>,
}

#[derive(Debug, serde::Deserialize)]
struct ProjectList {
    projects: Vec<Project>,
}

#[derive(Debug, serde::Deserialize)]
pub struct ProjectDetail {
    pub id: String,
    pub name: String,
    pub repo_url: String,
    pub default_branch: String,
    pub setup_command: Option<String>,
    #[serde(default)]
    pub recent_sessions: Vec<SessionStatus>,
}

#[derive(Debug, serde::Deserialize)]
pub struct SessionStatus {
    pub id: String,
    pub status: String,
    pub status_detail: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub ended_at: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct SessionList {
    sessions: Vec<SessionStatus>,
}

#[derive(Debug, serde::Deserialize)]
pub struct StartedSession {
    pub session_id: String,
    pub status: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct ConnectionInfo {
    pub ssh_host: String,
    pub ssh_user: String,
    pub ssh_command: String,
}

/// Everything needed to open the server-side terminal relay: the websocket
/// url plus the bearer token to authenticate the handshake.
pub struct TerminalEndpoint {
    pub ws_url: String,
    pub bearer_token: String,
    pub workspace_path: String,
}

pub struct CreateProject<'a> {
    pub name: &'a str,
    pub repo_url: &'a str,
    pub default_branch: Option<&'a str>,
    pub setup_command: Option<&'a str>,
}

impl CloudClient {
    pub fn new(base_url: String, token: String) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
        }
    }

    pub fn railway_connect_url(&self) -> String {
        format!("{}/api/providers/railway/connect", self.base_url)
    }

    pub fn me(&self) -> Result<CloudUser, ApiError> {
        self.get("/api/me")
    }

    pub fn railway_status(&self) -> Result<RailwayStatus, ApiError> {
        self.get("/api/providers/railway")
    }

    pub fn create_project(
        &self,
        request: &CreateProject<'_>,
    ) -> Result<serde_json::Value, ApiError> {
        self.post(
            "/api/projects",
            serde_json::json!({
                "name": request.name,
                "repo_url": request.repo_url,
                "default_branch": request.default_branch.unwrap_or("main"),
                "setup_command": request.setup_command,
            }),
        )
    }

    pub fn list_projects(&self) -> Result<Vec<Project>, ApiError> {
        self.get::<ProjectList>("/api/projects")
            .map(|list| list.projects)
    }

    pub fn get_project(&self, project_id: &str) -> Result<ProjectDetail, ApiError> {
        self.get(&format!("/api/projects/{project_id}"))
    }

    pub fn delete_project(&self, project_id: &str) -> Result<(), ApiError> {
        self.delete(&format!("/api/projects/{project_id}"))
    }

    pub fn start_session(
        &self,
        project_id: &str,
        idle_timeout_minutes: Option<u32>,
    ) -> Result<StartedSession, ApiError> {
        let response = self.post(
            &format!("/api/projects/{project_id}/sessions"),
            serde_json::json!({ "idle_timeout_minutes": idle_timeout_minutes }),
        )?;
        serde_json::from_value(response).map_err(invalid_body)
    }

    pub fn list_sessions(&self, project_id: &str) -> Result<Vec<SessionStatus>, ApiError> {
        self.get::<SessionList>(&format!("/api/projects/{project_id}/sessions"))
            .map(|list| list.sessions)
    }

    pub fn get_session(&self, session_id: &str) -> Result<SessionStatus, ApiError> {
        self.get(&format!("/api/sessions/{session_id}"))
    }

    pub fn get_connection(&self, session_id: &str) -> Result<ConnectionInfo, ApiError> {
        self.get(&format!("/api/sessions/{session_id}/connection"))
    }

    /// Builds the terminal-relay endpoint for a session. The relay lives on the
    /// Termy API (`ws(s)://<host>/api/sessions/{id}/terminal`); the server dials
    /// the provider, so no provider token is ever handed to the client.
    pub fn terminal_endpoint(&self, session_id: &str) -> Result<TerminalEndpoint, ApiError> {
        let ws_base = if let Some(rest) = self.base_url.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = self.base_url.strip_prefix("http://") {
            format!("ws://{rest}")
        } else {
            return Err(ApiError {
                status: 0,
                code: None,
                message: format!("unsupported API base url: {}", self.base_url),
            });
        };
        Ok(TerminalEndpoint {
            ws_url: format!("{ws_base}/api/sessions/{session_id}/terminal"),
            bearer_token: self.token.clone(),
            workspace_path: "/workspace/app".to_string(),
        })
    }

    pub fn stop_session(&self, session_id: &str) -> Result<(), ApiError> {
        self.delete(&format!("/api/sessions/{session_id}"))
    }

    fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, ApiError> {
        let response = ureq::get(&format!("{}{path}", self.base_url))
            .set("Accept", "application/json")
            .set("Authorization", &format!("Bearer {}", self.token))
            .call()
            .map_err(api_error)?;
        response.into_json().map_err(invalid_body)
    }

    fn post(&self, path: &str, body: serde_json::Value) -> Result<serde_json::Value, ApiError> {
        let response = ureq::post(&format!("{}{path}", self.base_url))
            .set("Accept", "application/json")
            .set("Authorization", &format!("Bearer {}", self.token))
            .send_json(body)
            .map_err(api_error)?;
        response.into_json().map_err(invalid_body)
    }

    fn delete(&self, path: &str) -> Result<(), ApiError> {
        ureq::delete(&format!("{}{path}", self.base_url))
            .set("Accept", "application/json")
            .set("Authorization", &format!("Bearer {}", self.token))
            .call()
            .map_err(api_error)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Error helpers

fn invalid_body(error: impl std::fmt::Display) -> ApiError {
    ApiError {
        status: 0,
        code: None,
        message: format!("Invalid response from the cloud API: {error}"),
    }
}

fn api_error(error: ureq::Error) -> ApiError {
    match error {
        ureq::Error::Status(status, response) => {
            let parsed: Option<ErrorResponse> = response.into_json().ok();
            let (code, message) = parsed.map_or((None, None), |body| (body.code, body.message));
            ApiError {
                status,
                code,
                message: message.unwrap_or_else(|| format!("the server returned HTTP {status}")),
            }
        }
        error => ApiError {
            status: 0,
            code: None,
            message: error.to_string(),
        },
    }
}

fn response_error_message(response: ureq::Response) -> String {
    response
        .into_json::<ErrorResponse>()
        .ok()
        .and_then(|body| body.message)
        .filter(|message| !message.trim().is_empty())
        .unwrap_or_else(|| "server rejected the request".to_string())
}

fn request_error(context: &str, error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(status, response) => {
            let message = response_error_message(response);
            if message == "server rejected the request" {
                format!("{context}: server returned HTTP {status}")
            } else {
                format!("{context}: {message}")
            }
        }
        error => format!("{context}: {error}"),
    }
}

fn device_verification_url(
    api_base: &str,
    verification_uri: &str,
    user_code: &str,
) -> Result<String, String> {
    let mut url = match Url::parse(verification_uri) {
        Ok(url) => url,
        Err(_) => Url::parse(&format!("{}/", api_base.trim_end_matches('/')))
            .and_then(|base| base.join(verification_uri.trim_start_matches('/')))
            .map_err(|error| format!("Invalid device verification URL: {error}"))?,
    };
    url.query_pairs_mut().append_pair("user_code", user_code);
    Ok(url.into())
}

#[cfg(test)]
mod tests {
    use super::{device_verification_url, load_session, save_session, session_path_for};
    use super::{CloudSession, CloudUser};
    use std::path::Path;

    #[test]
    fn session_path_sits_next_to_config_file() {
        let path = session_path_for(Path::new("/home/user/.config/termy/config.txt")).unwrap();
        assert_eq!(path, Path::new("/home/user/.config/termy/cloud_auth.json"));
    }

    #[test]
    fn session_roundtrips_through_disk() {
        let dir = std::env::temp_dir().join(format!("termy-cloud-client-{}", std::process::id()));
        let path = dir.join("cloud_auth.json");
        let session = CloudSession {
            token: "tok".to_string(),
            user: CloudUser {
                id: "u1".to_string(),
                email: "a@b.c".to_string(),
                name: None,
            },
        };
        save_session(&path, &session).unwrap();
        assert_eq!(load_session(&path).unwrap(), Some(session));
        super::clear_session(&path).unwrap();
        assert_eq!(load_session(&path).unwrap(), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn device_verification_url_supports_absolute_and_relative_paths() {
        assert_eq!(
            device_verification_url("https://app.termy.sh", "/device", "ABCD1234").unwrap(),
            "https://app.termy.sh/device?user_code=ABCD1234"
        );
        assert_eq!(
            device_verification_url(
                "http://127.0.0.1:8080",
                "http://127.0.0.1:8080/device",
                "EFGH5678"
            )
            .unwrap(),
            "http://127.0.0.1:8080/device?user_code=EFGH5678"
        );
    }
}
