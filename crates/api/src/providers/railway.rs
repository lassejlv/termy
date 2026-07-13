//! Railway sandbox adapter.
//!
//! Talks to Railway's public GraphQL API (`backboard.railway.com/graphql/v2`)
//! with the user's OAuth access token. Sandboxes are a Priority Boarding
//! feature; the account must be enrolled. Operation shapes were pinned against
//! the open-source `railway-ts-sdk` and `railwayapp/cli` sources (TRM-34
//! spike notes).

use super::{
    CreateSandboxRequest, ExecResult, ProviderCtx, ProviderError, SandboxInfo, SandboxProvider,
    SandboxState, ShellConnection, SshConnectionInfo,
};

pub(crate) const GRAPHQL_ENDPOINT: &str = "https://backboard.railway.com/graphql/v2";
const SSH_RELAY_HOST: &str = "ssh.railway.com";
/// tcp-proxy exec bridge; a `shell`-scoped JWT rides as the last
/// `Sec-WebSocket-Protocol` value alongside `railway-shell`.
const WS_EXEC_ENDPOINT: &str = "wss://ssh.railway.com:2226/ws/exec";

pub(crate) struct RailwayProvider {
    http: reqwest::Client,
    endpoint: String,
}

impl RailwayProvider {
    pub(crate) fn new(http: reqwest::Client) -> Self {
        Self::with_endpoint(http, GRAPHQL_ENDPOINT.to_string())
    }

    fn with_endpoint(http: reqwest::Client, endpoint: String) -> Self {
        Self { http, endpoint }
    }

    async fn graphql(
        &self,
        ctx: &ProviderCtx,
        query: &str,
        variables: serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        let response = self
            .http
            .post(&self.endpoint)
            .bearer_auth(&ctx.access_token)
            .json(&serde_json::json!({ "query": query, "variables": variables }))
            .send()
            .await
            .map_err(|error| ProviderError::Api(format!("Railway request failed: {error}")))?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(ProviderError::ReauthRequired(format!(
                "Railway rejected the access token (HTTP {status})"
            )));
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(ProviderError::RateLimited);
        }
        let body: serde_json::Value = response.json().await.map_err(|error| {
            ProviderError::Api(format!("Railway returned invalid JSON: {error}"))
        })?;
        if let Some(errors) = body["errors"].as_array().filter(|list| !list.is_empty()) {
            let message = errors[0]["message"].as_str().unwrap_or("unknown error");
            if message.to_ascii_lowercase().contains("not found") {
                return Err(ProviderError::NotFound(message.to_string()));
            }
            return Err(ProviderError::Api(format!("Railway error: {message}")));
        }
        Ok(body["data"].clone())
    }
}

pub(crate) fn parse_sandbox_state(status: &str) -> SandboxState {
    match status {
        "CREATING" => SandboxState::Creating,
        "RUNNING" => SandboxState::Running,
        "DESTROYING" | "DESTROYED" => SandboxState::Destroyed,
        _ => SandboxState::Failed,
    }
}

fn sandbox_info_from(node: &serde_json::Value) -> Result<SandboxInfo, ProviderError> {
    let id = node["id"]
        .as_str()
        .ok_or_else(|| ProviderError::Api("sandbox response missing id".to_string()))?;
    let status = node["status"].as_str().unwrap_or("FAILED");
    Ok(SandboxInfo {
        id: id.to_string(),
        state: parse_sandbox_state(status),
    })
}

/// SSH target the railway CLI uses: `sbx:{environmentId}:{sandboxId}@ssh.railway.com`.
pub(crate) fn ssh_connection_info(environment_id: &str, sandbox_id: &str) -> SshConnectionInfo {
    let user = format!("sbx:{environment_id}:{sandbox_id}");
    SshConnectionInfo {
        ssh_command: format!("ssh {user}@{SSH_RELAY_HOST}"),
        ssh_host: SSH_RELAY_HOST.to_string(),
        ssh_user: user,
    }
}

#[async_trait::async_trait]
impl SandboxProvider for RailwayProvider {
    async fn ensure_environment(
        &self,
        ctx: &ProviderCtx,
        project_name: &str,
    ) -> Result<(String, String), ProviderError> {
        // OAuth tokens carry no default workspace, so projectCreate requires
        // an explicit workspaceId; use the first workspace the consent grant
        // exposes.
        let me = self
            .graphql(
                ctx,
                "query TermyWorkspaces { me { workspaces { id name } } }",
                serde_json::json!({}),
            )
            .await?;
        let workspace_id = me["me"]["workspaces"][0]["id"].as_str().ok_or_else(|| {
            ProviderError::Api(
                "the Railway connection has no accessible workspace; reconnect and grant a workspace".to_string(),
            )
        })?;

        let data = self
            .graphql(
                ctx,
                "mutation TermyProjectCreate($input: ProjectCreateInput!) {
                    projectCreate(input: $input) { id primaryEnvironmentId }
                }",
                serde_json::json!({
                    "input": {
                        "name": format!("termy-{project_name}"),
                        "description": "Managed by Termy Cloud Projects",
                        "workspaceId": workspace_id,
                    }
                }),
            )
            .await?;
        let project = &data["projectCreate"];
        let project_id = project["id"]
            .as_str()
            .ok_or_else(|| ProviderError::Api("projectCreate returned no id".to_string()))?;
        let environment_id = project["primaryEnvironmentId"].as_str().ok_or_else(|| {
            ProviderError::Api("projectCreate returned no primary environment".to_string())
        })?;
        Ok((project_id.to_string(), environment_id.to_string()))
    }

    async fn create_sandbox(
        &self,
        ctx: &ProviderCtx,
        request: CreateSandboxRequest,
    ) -> Result<SandboxInfo, ProviderError> {
        let mut input = serde_json::json!({
            "environmentId": request.environment_id,
            "idleTimeoutMinutes": request.idle_timeout_minutes,
        });
        if let Some(checkpoint) = &request.checkpoint {
            // A checkpoint is referenced server-side by name alone, riding the
            // template input (name and instructions are mutually exclusive).
            input["template"] = serde_json::json!({ "name": checkpoint });
        }
        if !request.variables.is_empty() {
            input["variables"] = request
                .variables
                .iter()
                .map(|(key, value)| (key.clone(), serde_json::Value::from(value.clone())))
                .collect::<serde_json::Map<_, _>>()
                .into();
        }
        let data = self
            .graphql(
                ctx,
                "mutation TermySandboxCreate($input: SandboxCreateInput!) {
                    sandboxCreate(input: $input) { id status environmentId }
                }",
                serde_json::json!({ "input": input }),
            )
            .await?;
        sandbox_info_from(&data["sandboxCreate"])
    }

    async fn inspect_sandbox(
        &self,
        ctx: &ProviderCtx,
        environment_id: &str,
        sandbox_id: &str,
    ) -> Result<SandboxInfo, ProviderError> {
        let data = self
            .graphql(
                ctx,
                "query TermySandbox($environmentId: String!, $id: String!) {
                    sandbox(environmentId: $environmentId, id: $id) { id status }
                }",
                serde_json::json!({ "environmentId": environment_id, "id": sandbox_id }),
            )
            .await?;
        if data["sandbox"].is_null() {
            return Err(ProviderError::NotFound(format!(
                "sandbox {sandbox_id} not found"
            )));
        }
        sandbox_info_from(&data["sandbox"])
    }

    async fn exec(
        &self,
        ctx: &ProviderCtx,
        environment_id: &str,
        sandbox_id: &str,
        command: &str,
        timeout_secs: u32,
    ) -> Result<ExecResult, ProviderError> {
        let data = self
            .graphql(
                ctx,
                "mutation TermySandboxExec($id: String!, $environmentId: String!, $command: String!, $timeoutSec: Int) {
                    sandboxExec(id: $id, environmentId: $environmentId, command: $command, timeoutSec: $timeoutSec) {
                        exitCode stdout stderr timedOut truncated
                    }
                }",
                serde_json::json!({
                    "id": sandbox_id,
                    "environmentId": environment_id,
                    "command": command,
                    "timeoutSec": timeout_secs,
                }),
            )
            .await?;
        let result = &data["sandboxExec"];
        Ok(ExecResult {
            exit_code: result["exitCode"].as_i64().unwrap_or(-1) as i32,
            stdout: result["stdout"].as_str().unwrap_or_default().to_string(),
            stderr: result["stderr"].as_str().unwrap_or_default().to_string(),
            timed_out: result["timedOut"].as_bool().unwrap_or(false),
        })
    }

    async fn destroy_sandbox(
        &self,
        ctx: &ProviderCtx,
        environment_id: &str,
        sandbox_id: &str,
    ) -> Result<(), ProviderError> {
        self.graphql(
            ctx,
            "mutation TermySandboxDestroy($id: String!, $environmentId: String!) {
                sandboxDestroy(id: $id, environmentId: $environmentId) { id status }
            }",
            serde_json::json!({ "id": sandbox_id, "environmentId": environment_id }),
        )
        .await?;
        Ok(())
    }

    async fn create_checkpoint(
        &self,
        ctx: &ProviderCtx,
        environment_id: &str,
        sandbox_id: &str,
        name: &str,
    ) -> Result<(), ProviderError> {
        // Checkpoint names are unique per environment; drop a stale one with
        // the same name first so stop can always capture fresh state.
        let delete = self
            .graphql(
                ctx,
                "mutation TermyCheckpointDelete($environmentId: String!, $id: ID!) {
                    sandboxCheckpointDelete(environmentId: $environmentId, id: $id)
                }",
                serde_json::json!({ "environmentId": environment_id, "id": name }),
            )
            .await;
        if let Err(error) = delete {
            if !matches!(error, ProviderError::NotFound(_)) {
                tracing::debug!("stale checkpoint delete failed (continuing): {error}");
            }
        }
        self.graphql(
            ctx,
            "mutation TermyCheckpointCreate($environmentId: String!, $name: String!, $sandboxId: String!) {
                sandboxCheckpointCreate(environmentId: $environmentId, name: $name, sandboxId: $sandboxId) {
                    id key environmentId
                }
            }",
            serde_json::json!({
                "environmentId": environment_id,
                "name": name,
                "sandboxId": sandbox_id,
            }),
        )
        .await?;
        Ok(())
    }

    async fn shell_token(
        &self,
        ctx: &ProviderCtx,
        environment_id: &str,
        sandbox_id: &str,
    ) -> Result<ShellConnection, ProviderError> {
        let data = self
            .graphql(
                ctx,
                "mutation TermyShellToken($input: ShellTokenInput!) {
                    generateShellToken(input: $input)
                }",
                serde_json::json!({
                    "input": {
                        "environmentId": environment_id,
                        "instanceId": sandbox_id,
                        "kind": "sandbox",
                        "scope": "shell",
                    }
                }),
            )
            .await?;
        let token = data["generateShellToken"].as_str().ok_or_else(|| {
            ProviderError::Api("generateShellToken returned no token".to_string())
        })?;
        Ok(ShellConnection {
            token: token.to_string(),
            ws_url: WS_EXEC_ENDPOINT.to_string(),
        })
    }

    fn connection_info(&self, environment_id: &str, sandbox_id: &str) -> SshConnectionInfo {
        ssh_connection_info(environment_id, sandbox_id)
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_sandbox_state, sandbox_info_from, ssh_connection_info};
    use crate::providers::SandboxState;

    #[test]
    fn ssh_target_matches_railway_cli_format() {
        let info = ssh_connection_info("env-123", "sbx-456");
        assert_eq!(info.ssh_user, "sbx:env-123:sbx-456");
        assert_eq!(info.ssh_host, "ssh.railway.com");
        assert_eq!(info.ssh_command, "ssh sbx:env-123:sbx-456@ssh.railway.com");
    }

    #[test]
    fn sandbox_states_map_to_lifecycle() {
        assert!(matches!(
            parse_sandbox_state("CREATING"),
            SandboxState::Creating
        ));
        assert!(matches!(
            parse_sandbox_state("RUNNING"),
            SandboxState::Running
        ));
        assert!(matches!(
            parse_sandbox_state("DESTROYED"),
            SandboxState::Destroyed
        ));
        assert!(matches!(
            parse_sandbox_state("DESTROYING"),
            SandboxState::Destroyed
        ));
        assert!(matches!(
            parse_sandbox_state("FAILED"),
            SandboxState::Failed
        ));
        assert!(matches!(parse_sandbox_state("???"), SandboxState::Failed));
    }

    #[test]
    fn sandbox_info_parses_fixture_response() {
        // Fixture shape from railway-ts-sdk RailwaySandboxFields fragment.
        let node = serde_json::json!({
            "id": "sbx-1",
            "status": "RUNNING",
            "environmentId": "env-1",
            "region": "us-west2",
            "idleTimeoutMinutes": 30,
            "createdAt": "2026-07-13T00:00:00Z"
        });
        let info = sandbox_info_from(&node).unwrap();
        assert_eq!(info.id, "sbx-1");
        assert!(matches!(info.state, SandboxState::Running));
    }

    #[test]
    fn sandbox_info_requires_id() {
        assert!(sandbox_info_from(&serde_json::json!({ "status": "RUNNING" })).is_err());
    }
}
