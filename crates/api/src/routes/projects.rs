//! Cloud project CRUD. Every handler scopes queries by the session user.

use std::sync::Arc;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use better_auth::adapters::SqlxAdapter;
use better_auth::handlers::CurrentSession;
use better_auth::AuthUser as _;

use crate::providers::ProviderCtx;
use crate::routes::{error_json, internal_error, railway_token_or_response};
use crate::sessions::ACTIVE_STATUSES;
use crate::state::CloudState;

const MAX_NAME_LEN: usize = 64;

pub(crate) fn validate_project_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty() || name.len() > MAX_NAME_LEN {
        return Err("project name must be 1-64 characters");
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' || byte == b'.')
    {
        return Err("project name may only contain letters, digits, '-', '_' and '.'");
    }
    Ok(())
}

/// v1 supports public GitHub repositories cloned over plain https.
pub(crate) fn validate_repo_url(repo_url: &str) -> Result<(), &'static str> {
    let Some(path) = repo_url.strip_prefix("https://github.com/") else {
        return Err("repo_url must be a public https://github.com/ URL");
    };
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut segments = path.split('/');
    let (Some(owner), Some(repo), None) = (segments.next(), segments.next(), segments.next())
    else {
        return Err("repo_url must look like https://github.com/{owner}/{repo}");
    };
    let valid_segment = |segment: &str| {
        !segment.is_empty()
            && segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    };
    if !valid_segment(owner) || !valid_segment(repo) {
        return Err("repo_url must look like https://github.com/{owner}/{repo}");
    }
    Ok(())
}

pub(crate) fn validate_branch(branch: &str) -> Result<(), &'static str> {
    if branch.is_empty() || branch.len() > 200 {
        return Err("default_branch must be 1-200 characters");
    }
    if branch.starts_with('-')
        || !branch
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/'))
    {
        return Err("default_branch contains unsupported characters");
    }
    Ok(())
}

#[derive(serde::Deserialize)]
pub(crate) struct CreateProjectBody {
    name: String,
    repo_url: String,
    #[serde(default = "default_branch")]
    default_branch: String,
    setup_command: Option<String>,
}

fn default_branch() -> String {
    "main".to_string()
}

pub(crate) async fn create_project(
    session: CurrentSession<SqlxAdapter>,
    Extension(cloud): Extension<Arc<CloudState>>,
    Json(body): Json<CreateProjectBody>,
) -> axum::response::Response {
    if let Err(message) = validate_project_name(&body.name)
        .and_then(|()| validate_repo_url(&body.repo_url))
        .and_then(|()| validate_branch(&body.default_branch))
    {
        return error_json(StatusCode::UNPROCESSABLE_ENTITY, "invalid_project", message);
    }
    let user_id = session.user.id();
    let access_token = match railway_token_or_response(&cloud, user_id).await {
        Ok(token) => token,
        Err(response) => return response,
    };
    let ctx = ProviderCtx { access_token };
    let (railway_project_id, railway_environment_id) =
        match cloud.provider.ensure_environment(&ctx, &body.name).await {
            Ok(ids) => ids,
            Err(error) => {
                return error_json(
                    StatusCode::BAD_GATEWAY,
                    "provider_error",
                    &format!("failed to provision Railway project: {error}"),
                )
            }
        };

    let row: Result<Option<(String,)>, _> = neon_serverless_sqlx::sqlx::query_as(
        "INSERT INTO projects
            (user_id, name, repo_url, default_branch, setup_command,
             railway_project_id, railway_environment_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (user_id, name) DO NOTHING
         RETURNING id",
    )
    .bind(user_id)
    .bind(&body.name)
    .bind(&body.repo_url)
    .bind(&body.default_branch)
    .bind(&body.setup_command)
    .bind(&railway_project_id)
    .bind(&railway_environment_id)
    .fetch_optional(cloud.db.pg())
    .await;
    match row {
        Ok(Some((id,))) => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "id": id,
                "name": body.name,
                "repo_url": body.repo_url,
                "default_branch": body.default_branch,
                "setup_command": body.setup_command,
            })),
        )
            .into_response(),
        Ok(None) => error_json(
            StatusCode::CONFLICT,
            "project_exists",
            "a project with this name already exists",
        ),
        Err(error) => internal_error("failed to create project", error),
    }
}

pub(crate) async fn list_projects(
    session: CurrentSession<SqlxAdapter>,
    Extension(cloud): Extension<Arc<CloudState>>,
) -> axum::response::Response {
    type Row = (
        String,
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let rows: Result<Vec<Row>, _> = neon_serverless_sqlx::sqlx::query_as(
        "SELECT p.id, p.name, p.repo_url, p.default_branch, p.setup_command,
                s.id, s.status
         FROM projects p
         LEFT JOIN LATERAL (
            SELECT id, status FROM sandbox_sessions
            WHERE project_id = p.id AND status = ANY($2)
            ORDER BY created_at DESC LIMIT 1
         ) s ON TRUE
         WHERE p.user_id = $1
         ORDER BY p.created_at",
    )
    .bind(session.user.id())
    .bind(ACTIVE_STATUSES)
    .fetch_all(cloud.db.pg())
    .await;
    match rows {
        Ok(rows) => Json(serde_json::json!({
            "projects": rows
                .into_iter()
                .map(|(id, name, repo_url, branch, setup, session_id, session_status)| {
                    serde_json::json!({
                        "id": id,
                        "name": name,
                        "repo_url": repo_url,
                        "default_branch": branch,
                        "setup_command": setup,
                        "active_session": session_id.map(|sid| serde_json::json!({
                            "id": sid,
                            "status": session_status,
                        })),
                    })
                })
                .collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(error) => internal_error("failed to list projects", error),
    }
}

pub(crate) async fn get_project(
    session: CurrentSession<SqlxAdapter>,
    Extension(cloud): Extension<Arc<CloudState>>,
    Path(project_id): Path<String>,
) -> axum::response::Response {
    type ProjectRow = (String, String, String, String, Option<String>);
    let project: Result<Option<ProjectRow>, _> = neon_serverless_sqlx::sqlx::query_as(
        "SELECT id, name, repo_url, default_branch, setup_command
         FROM projects WHERE id = $1 AND user_id = $2",
    )
    .bind(&project_id)
    .bind(session.user.id())
    .fetch_optional(cloud.db.pg())
    .await;
    let (id, name, repo_url, branch, setup) = match project {
        Ok(Some(row)) => row,
        Ok(None) => {
            return error_json(
                StatusCode::NOT_FOUND,
                "project_not_found",
                "unknown project",
            )
        }
        Err(error) => return internal_error("failed to load project", error),
    };

    type SessionRow = (String, String, Option<String>, String);
    let recent: Result<Vec<SessionRow>, _> = neon_serverless_sqlx::sqlx::query_as(
        "SELECT id, status, status_detail, created_at::text
         FROM sandbox_sessions WHERE project_id = $1
         ORDER BY created_at DESC LIMIT 5",
    )
    .bind(&id)
    .fetch_all(cloud.db.pg())
    .await;
    let recent = match recent {
        Ok(rows) => rows,
        Err(error) => return internal_error("failed to load sessions", error),
    };
    Json(serde_json::json!({
        "id": id,
        "name": name,
        "repo_url": repo_url,
        "default_branch": branch,
        "setup_command": setup,
        "recent_sessions": recent
            .into_iter()
            .map(|(sid, status, detail, created_at)| serde_json::json!({
                "id": sid,
                "status": status,
                "status_detail": detail,
                "created_at": created_at,
            }))
            .collect::<Vec<_>>(),
    }))
    .into_response()
}

#[derive(serde::Deserialize)]
pub(crate) struct UpdateProjectBody {
    repo_url: Option<String>,
    default_branch: Option<String>,
    setup_command: Option<String>,
}

pub(crate) async fn update_project(
    session: CurrentSession<SqlxAdapter>,
    Extension(cloud): Extension<Arc<CloudState>>,
    Path(project_id): Path<String>,
    Json(body): Json<UpdateProjectBody>,
) -> axum::response::Response {
    if let Some(repo_url) = &body.repo_url {
        if let Err(message) = validate_repo_url(repo_url) {
            return error_json(StatusCode::UNPROCESSABLE_ENTITY, "invalid_project", message);
        }
    }
    if let Some(branch) = &body.default_branch {
        if let Err(message) = validate_branch(branch) {
            return error_json(StatusCode::UNPROCESSABLE_ENTITY, "invalid_project", message);
        }
    }
    let result = neon_serverless_sqlx::sqlx::query(
        "UPDATE projects SET
            repo_url = COALESCE($3, repo_url),
            default_branch = COALESCE($4, default_branch),
            setup_command = COALESCE($5, setup_command),
            updated_at = NOW()
         WHERE id = $1 AND user_id = $2",
    )
    .bind(&project_id)
    .bind(session.user.id())
    .bind(&body.repo_url)
    .bind(&body.default_branch)
    .bind(&body.setup_command)
    .execute(cloud.db.pg())
    .await;
    match result {
        Ok(outcome) if outcome.rows_affected() == 0 => error_json(
            StatusCode::NOT_FOUND,
            "project_not_found",
            "unknown project",
        ),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => internal_error("failed to update project", error),
    }
}

pub(crate) async fn delete_project(
    session: CurrentSession<SqlxAdapter>,
    Extension(cloud): Extension<Arc<CloudState>>,
    Path(project_id): Path<String>,
) -> axum::response::Response {
    let active: Result<Option<(String,)>, _> = neon_serverless_sqlx::sqlx::query_as(
        "SELECT s.id FROM sandbox_sessions s
         JOIN projects p ON p.id = s.project_id
         WHERE p.id = $1 AND p.user_id = $2 AND s.status = ANY($3)
         LIMIT 1",
    )
    .bind(&project_id)
    .bind(session.user.id())
    .bind(ACTIVE_STATUSES)
    .fetch_optional(cloud.db.pg())
    .await;
    match active {
        Ok(Some(_)) => {
            return error_json(
                StatusCode::CONFLICT,
                "session_active",
                "stop the active session before deleting the project",
            )
        }
        Ok(None) => {}
        Err(error) => return internal_error("failed to check active sessions", error),
    }
    let result =
        neon_serverless_sqlx::sqlx::query("DELETE FROM projects WHERE id = $1 AND user_id = $2")
            .bind(&project_id)
            .bind(session.user.id())
            .execute(cloud.db.pg())
            .await;
    match result {
        Ok(outcome) if outcome.rows_affected() == 0 => error_json(
            StatusCode::NOT_FOUND,
            "project_not_found",
            "unknown project",
        ),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => internal_error("failed to delete project", error),
    }
}

#[cfg(test)]
mod tests {
    use super::{validate_branch, validate_project_name, validate_repo_url};

    #[test]
    fn project_names_are_validated() {
        assert!(validate_project_name("termy").is_ok());
        assert!(validate_project_name("my-project_1.0").is_ok());
        assert!(validate_project_name("").is_err());
        assert!(validate_project_name("has space").is_err());
        assert!(validate_project_name(&"x".repeat(65)).is_err());
    }

    #[test]
    fn only_public_github_https_urls_are_accepted() {
        assert!(validate_repo_url("https://github.com/lasse/termy").is_ok());
        assert!(validate_repo_url("https://github.com/lasse/termy.git").is_ok());
        assert!(validate_repo_url("https://gitlab.com/lasse/termy").is_err());
        assert!(validate_repo_url("git@github.com:lasse/termy.git").is_err());
        assert!(validate_repo_url("https://github.com/lasse").is_err());
        assert!(validate_repo_url("https://github.com/lasse/termy/extra").is_err());
        assert!(validate_repo_url("https://github.com/la sse/termy").is_err());
    }

    #[test]
    fn branches_are_validated() {
        assert!(validate_branch("main").is_ok());
        assert!(validate_branch("feature/foo-1.2").is_ok());
        assert!(validate_branch("").is_err());
        assert!(validate_branch("-flag-injection").is_err());
        assert!(validate_branch("bad branch").is_err());
    }
}
