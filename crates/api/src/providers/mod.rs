//! Sandbox provider integrations.
//!
//! Provider-specific behavior (OAuth connection, sandbox lifecycle) lives
//! behind this module so the project/session routes stay provider-neutral.

pub(crate) mod railway;
pub(crate) mod railway_oauth;

pub(crate) struct ProviderCtx {
    pub(crate) access_token: String,
}

pub(crate) struct CreateSandboxRequest {
    pub(crate) environment_id: String,
    pub(crate) idle_timeout_minutes: u32,
    pub(crate) variables: Vec<(String, String)>,
    /// Boot from this named checkpoint instead of the base image.
    pub(crate) checkpoint: Option<String>,
}

#[derive(Debug)]
pub(crate) enum SandboxState {
    Creating,
    Running,
    Destroyed,
    Failed,
}

pub(crate) struct SandboxInfo {
    pub(crate) id: String,
    pub(crate) state: SandboxState,
}

pub(crate) struct ExecResult {
    pub(crate) exit_code: i32,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) timed_out: bool,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct SshConnectionInfo {
    pub(crate) ssh_host: String,
    pub(crate) ssh_user: String,
    pub(crate) ssh_command: String,
}

#[derive(Debug)]
pub(crate) enum ProviderError {
    /// The provider rejected the access token; the user must reconnect.
    ReauthRequired(String),
    /// The requested resource is gone (e.g. idle-reaped sandbox).
    NotFound(String),
    RateLimited,
    Api(String),
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ReauthRequired(message) => write!(f, "reauth required: {message}"),
            Self::NotFound(message) => write!(f, "not found: {message}"),
            Self::RateLimited => write!(f, "provider rate limited"),
            Self::Api(message) => write!(f, "{message}"),
        }
    }
}

/// Provider-neutral sandbox lifecycle. Environment ids are provider-scoped
/// opaque strings (for Railway: the Railway environment id).
#[async_trait::async_trait]
pub(crate) trait SandboxProvider: Send + Sync {
    /// Creates (or provisions) the provider-side project/environment pair for
    /// a Termy project; returns `(provider_project_id, environment_id)`.
    async fn ensure_environment(
        &self,
        ctx: &ProviderCtx,
        project_name: &str,
    ) -> Result<(String, String), ProviderError>;

    async fn create_sandbox(
        &self,
        ctx: &ProviderCtx,
        request: CreateSandboxRequest,
    ) -> Result<SandboxInfo, ProviderError>;

    async fn inspect_sandbox(
        &self,
        ctx: &ProviderCtx,
        environment_id: &str,
        sandbox_id: &str,
    ) -> Result<SandboxInfo, ProviderError>;

    /// Blocking exec used for clone/setup; not for interactive terminals.
    async fn exec(
        &self,
        ctx: &ProviderCtx,
        environment_id: &str,
        sandbox_id: &str,
        command: &str,
        timeout_secs: u32,
    ) -> Result<ExecResult, ProviderError>;

    async fn destroy_sandbox(
        &self,
        ctx: &ProviderCtx,
        environment_id: &str,
        sandbox_id: &str,
    ) -> Result<(), ProviderError>;

    /// Captures the sandbox disk under `name`, replacing any prior checkpoint
    /// with the same name.
    async fn create_checkpoint(
        &self,
        ctx: &ProviderCtx,
        environment_id: &str,
        sandbox_id: &str,
        name: &str,
    ) -> Result<(), ProviderError>;

    /// Mints a short-lived shell token scoped to one sandbox, plus the
    /// websocket endpoint it authorizes.
    async fn shell_token(
        &self,
        ctx: &ProviderCtx,
        environment_id: &str,
        sandbox_id: &str,
    ) -> Result<ShellConnection, ProviderError>;

    fn connection_info(&self, environment_id: &str, sandbox_id: &str) -> SshConnectionInfo;
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct ShellConnection {
    pub(crate) token: String,
    pub(crate) ws_url: String,
}
