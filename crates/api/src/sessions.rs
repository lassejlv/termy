//! Sandbox session lifecycle.
//!
//! Starting a session inserts a `pending` row and returns immediately; a
//! background task drives the sandbox through
//! `provisioning → cloning → setting_up → ready`. Clients poll
//! `GET /api/sessions/{id}`. Any failure lands in `failed` with a
//! `status_detail` and a best-effort sandbox destroy.

use std::sync::Arc;

use crate::providers::{
    CreateSandboxRequest, ExecResult, ProviderCtx, ProviderError, SandboxState,
};
use crate::state::CloudState;

pub(crate) const CLONE_TIMEOUT_SECS: u32 = 600;
pub(crate) const SETUP_TIMEOUT_SECS: u32 = 1800;
pub(crate) const WORKSPACE_PATH: &str = "/workspace/app";

/// Statuses that count as "session is live" for the one-active-session rule.
pub(crate) const ACTIVE_STATUSES: &[&str] = &[
    "pending",
    "provisioning",
    "cloning",
    "setting_up",
    "ready",
    "stopping",
];

pub(crate) struct SessionSpec {
    pub(crate) session_id: String,
    pub(crate) project_id: String,
    pub(crate) environment_id: String,
    pub(crate) repo_url: String,
    pub(crate) default_branch: String,
    pub(crate) setup_command: Option<String>,
    pub(crate) idle_timeout_minutes: u32,
    pub(crate) access_token: String,
    /// Checkpoint captured by the last stop; boot from it when present.
    pub(crate) checkpoint: Option<String>,
}

/// Checkpoint name for a project — one slot per project, replaced on stop.
pub(crate) fn checkpoint_name(project_id: &str) -> String {
    format!("termy-{project_id}")
}

/// Shell-quotes a value for safe interpolation into the clone command.
pub(crate) fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

pub(crate) fn clone_command(repo_url: &str, branch: &str) -> String {
    // GIT_TERMINAL_PROMPT=0 makes git fail immediately instead of prompting
    // for credentials when the repository is private or does not exist.
    format!(
        "GIT_TERMINAL_PROMPT=0 git clone --branch {} --single-branch {} {WORKSPACE_PATH}",
        shell_quote(branch),
        shell_quote(repo_url),
    )
}

/// After a checkpoint boot the repository already exists; bring it up to date
/// with origin instead of re-cloning.
pub(crate) fn refresh_command(branch: &str) -> String {
    let branch = shell_quote(branch);
    format!(
        "cd {WORKSPACE_PATH} && GIT_TERMINAL_PROMPT=0 git fetch origin {branch} && git checkout {branch} && git reset --hard origin/{branch}",
    )
}

pub(crate) async fn set_status(
    cloud: &CloudState,
    session_id: &str,
    status: &str,
    detail: Option<&str>,
) {
    let result = neon_serverless_sqlx::sqlx::query(
        "UPDATE sandbox_sessions SET
            status = $2,
            status_detail = $3,
            started_at = CASE WHEN $2 = 'ready' THEN COALESCE(started_at, NOW()) ELSE started_at END,
            ended_at = CASE WHEN $2 IN ('stopped', 'failed') THEN COALESCE(ended_at, NOW()) ELSE ended_at END
         WHERE id = $1",
    )
    .bind(session_id)
    .bind(status)
    .bind(detail)
    .execute(cloud.db.pg())
    .await;
    if let Err(error) = result {
        tracing::error!("failed to update session {session_id} to {status}: {error}");
    }
}

/// Runs the full provisioning pipeline for a freshly inserted session row.
pub(crate) async fn run(cloud: Arc<CloudState>, spec: SessionSpec) {
    if let Err(detail) = drive(&cloud, &spec).await {
        tracing::warn!("session {} failed: {detail}", spec.session_id);
        set_status(&cloud, &spec.session_id, "failed", Some(&detail)).await;
    }
}

async fn drive(cloud: &CloudState, spec: &SessionSpec) -> Result<(), String> {
    let ctx = ProviderCtx {
        access_token: spec.access_token.clone(),
    };

    set_status(cloud, &spec.session_id, "provisioning", None).await;
    let request = |checkpoint: Option<String>| CreateSandboxRequest {
        environment_id: spec.environment_id.clone(),
        idle_timeout_minutes: spec.idle_timeout_minutes,
        variables: Vec::new(),
        checkpoint,
    };
    let mut from_checkpoint = spec.checkpoint.is_some();
    let sandbox = match cloud
        .provider
        .create_sandbox(&ctx, request(spec.checkpoint.clone()))
        .await
    {
        Ok(sandbox) => sandbox,
        // A stale or deleted checkpoint must not brick the project: clear it
        // and boot fresh.
        Err(error) if from_checkpoint => {
            tracing::warn!(
                "checkpoint boot failed for project {} ({error}); booting fresh",
                spec.project_id
            );
            clear_checkpoint(cloud, &spec.project_id).await;
            from_checkpoint = false;
            cloud
                .provider
                .create_sandbox(&ctx, request(None))
                .await
                .map_err(|error| format!("sandbox creation failed: {error}"))?
        }
        Err(error) => return Err(format!("sandbox creation failed: {error}")),
    };

    let store = neon_serverless_sqlx::sqlx::query(
        "UPDATE sandbox_sessions SET provider_sandbox_id = $2 WHERE id = $1",
    )
    .bind(&spec.session_id)
    .bind(&sandbox.id)
    .execute(cloud.db.pg())
    .await;
    if let Err(error) = store {
        // Without the sandbox id recorded, stop/destroy could not find the
        // sandbox later — tear it down now rather than leak it.
        let _ = cloud
            .provider
            .destroy_sandbox(&ctx, &spec.environment_id, &sandbox.id)
            .await;
        return Err(format!("failed to record sandbox id: {error}"));
    }

    let result = wait_until_running(cloud, spec, &ctx, &sandbox.id).await;
    let result = match result {
        Ok(()) => clone_and_setup(cloud, spec, &ctx, &sandbox.id, from_checkpoint).await,
        Err(detail) => Err(detail),
    };
    if let Err(detail) = result {
        let _ = cloud
            .provider
            .destroy_sandbox(&ctx, &spec.environment_id, &sandbox.id)
            .await;
        return Err(detail);
    }
    Ok(())
}

/// `sandboxCreate` returns while the sandbox is still `CREATING`; commands
/// only succeed once it reaches `RUNNING`.
async fn wait_until_running(
    cloud: &CloudState,
    spec: &SessionSpec,
    ctx: &ProviderCtx,
    sandbox_id: &str,
) -> Result<(), String> {
    const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);
    const MAX_POLLS: u32 = 90;
    for _ in 0..MAX_POLLS {
        let info = cloud
            .provider
            .inspect_sandbox(ctx, &spec.environment_id, sandbox_id)
            .await
            .map_err(|error| format!("sandbox status check failed: {error}"))?;
        match info.state {
            SandboxState::Running => return Ok(()),
            SandboxState::Creating => tokio::time::sleep(POLL_INTERVAL).await,
            SandboxState::Destroyed | SandboxState::Failed => {
                return Err(format!("sandbox entered {:?} while starting", info.state));
            }
        }
    }
    Err("sandbox did not reach RUNNING within 3 minutes".to_string())
}

async fn clone_and_setup(
    cloud: &CloudState,
    spec: &SessionSpec,
    ctx: &ProviderCtx,
    sandbox_id: &str,
    from_checkpoint: bool,
) -> Result<(), String> {
    set_status(cloud, &spec.session_id, "cloning", None).await;
    let git_command = if from_checkpoint {
        refresh_command(&spec.default_branch)
    } else {
        clone_command(&spec.repo_url, &spec.default_branch)
    };
    let clone = cloud
        .provider
        .exec(
            ctx,
            &spec.environment_id,
            sandbox_id,
            &git_command,
            CLONE_TIMEOUT_SECS,
        )
        .await
        .map_err(|error| format!("clone failed: {error}"))?;
    if clone.exit_code != 0 {
        let output = failure_output(&clone);
        if output.contains("could not read Username")
            || output.contains("Authentication failed")
            || output.contains("terminal prompts disabled")
        {
            return Err(format!(
                "repository is not publicly accessible: {} (v1 supports public GitHub repos only)",
                spec.repo_url
            ));
        }
        return Err(format!(
            "git clone exited with {}: {output}",
            clone.exit_code
        ));
    }

    // A checkpoint already contains the setup command's results.
    if let Some(setup_command) = spec
        .setup_command
        .as_deref()
        .filter(|command| !from_checkpoint && !command.trim().is_empty())
    {
        set_status(cloud, &spec.session_id, "setting_up", None).await;
        let setup = cloud
            .provider
            .exec(
                ctx,
                &spec.environment_id,
                sandbox_id,
                &format!("cd {WORKSPACE_PATH} && {setup_command}"),
                SETUP_TIMEOUT_SECS,
            )
            .await
            .map_err(|error| format!("setup command failed: {error}"))?;
        if setup.timed_out {
            return Err("setup command timed out".to_string());
        }
        if setup.exit_code != 0 {
            return Err(format!(
                "setup command exited with {}: {}",
                setup.exit_code,
                failure_output(&setup)
            ));
        }
    }

    let connection = cloud
        .provider
        .connection_info(&spec.environment_id, sandbox_id);
    let connection_json = serde_json::to_string(&connection).map_err(|error| error.to_string())?;
    neon_serverless_sqlx::sqlx::query(
        "UPDATE sandbox_sessions SET connection_info = $2::jsonb WHERE id = $1",
    )
    .bind(&spec.session_id)
    .bind(&connection_json)
    .execute(cloud.db.pg())
    .await
    .map_err(|error| format!("failed to store connection info: {error}"))?;

    set_status(cloud, &spec.session_id, "ready", None).await;
    Ok(())
}

async fn clear_checkpoint(cloud: &CloudState, project_id: &str) {
    let result = neon_serverless_sqlx::sqlx::query(
        "UPDATE projects SET checkpoint_key = NULL WHERE id = $1",
    )
    .bind(project_id)
    .execute(cloud.db.pg())
    .await;
    if let Err(error) = result {
        tracing::error!("failed to clear checkpoint for project {project_id}: {error}");
    }
}

/// Captures a checkpoint of the sandbox so the next session boots from it.
/// Failure is non-fatal — the next start falls back to a fresh clone.
pub(crate) async fn checkpoint_before_destroy(
    cloud: &CloudState,
    access_token: &str,
    project_id: &str,
    environment_id: &str,
    sandbox_id: &str,
) {
    let ctx = ProviderCtx {
        access_token: access_token.to_string(),
    };
    let name = checkpoint_name(project_id);
    match cloud
        .provider
        .create_checkpoint(&ctx, environment_id, sandbox_id, &name)
        .await
    {
        Ok(()) => {
            let result = neon_serverless_sqlx::sqlx::query(
                "UPDATE projects SET checkpoint_key = $2 WHERE id = $1",
            )
            .bind(project_id)
            .bind(&name)
            .execute(cloud.db.pg())
            .await;
            if let Err(error) = result {
                tracing::error!("failed to record checkpoint for project {project_id}: {error}");
            }
        }
        Err(error) => {
            tracing::warn!("checkpoint capture failed for project {project_id}: {error}");
        }
    }
}

/// Destroys the provider sandbox behind a session; a provider `NotFound`
/// (idle-reaped sandbox) counts as already destroyed.
pub(crate) async fn destroy_sandbox_best_effort(
    cloud: &CloudState,
    access_token: &str,
    environment_id: &str,
    sandbox_id: &str,
) -> Result<(), ProviderError> {
    let ctx = ProviderCtx {
        access_token: access_token.to_string(),
    };
    match cloud
        .provider
        .destroy_sandbox(&ctx, environment_id, sandbox_id)
        .await
    {
        Ok(()) | Err(ProviderError::NotFound(_)) => Ok(()),
        Err(error) => Err(error),
    }
}

/// Prefers stderr for error details, falling back to stdout (some tools
/// report failures on stdout only).
fn failure_output(result: &ExecResult) -> &str {
    if result.stderr.trim().is_empty() {
        tail_of(&result.stdout)
    } else {
        tail_of(&result.stderr)
    }
}

fn tail_of(output: &str) -> &str {
    const MAX: usize = 500;
    let trimmed = output.trim();
    match trimmed.char_indices().nth_back(MAX) {
        Some((index, _)) => &trimmed[index..],
        None => trimmed,
    }
}

#[cfg(test)]
mod tests {
    use super::{clone_command, shell_quote, tail_of, ACTIVE_STATUSES};

    #[test]
    fn clone_command_quotes_url_and_branch() {
        assert_eq!(
            clone_command("https://github.com/foo/bar", "main"),
            "GIT_TERMINAL_PROMPT=0 git clone --branch 'main' --single-branch 'https://github.com/foo/bar' /workspace/app"
        );
    }

    #[test]
    fn refresh_command_updates_in_place() {
        assert_eq!(
            super::refresh_command("main"),
            "cd /workspace/app && GIT_TERMINAL_PROMPT=0 git fetch origin 'main' && git checkout 'main' && git reset --hard origin/'main'"
        );
    }

    #[test]
    fn shell_quote_neutralizes_single_quotes() {
        assert_eq!(shell_quote("a'b"), r"'a'\''b'");
        assert_eq!(shell_quote("plain"), "'plain'");
    }

    #[test]
    fn terminal_statuses_are_not_active() {
        assert!(!ACTIVE_STATUSES.contains(&"stopped"));
        assert!(!ACTIVE_STATUSES.contains(&"failed"));
        assert!(ACTIVE_STATUSES.contains(&"pending"));
        assert!(ACTIVE_STATUSES.contains(&"ready"));
    }

    #[test]
    fn tail_of_bounds_long_output() {
        let long = "x".repeat(2000);
        assert!(tail_of(&long).len() <= 501);
        assert_eq!(tail_of(" short \n"), "short");
    }
}
