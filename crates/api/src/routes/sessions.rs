//! Sandbox session routes: start, poll, connect, stop.

use std::sync::Arc;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use better_auth::adapters::SqlxAdapter;
use better_auth::handlers::CurrentSession;
use better_auth::AuthUser as _;

use crate::routes::{error_json, internal_error, railway_token_or_response};
use crate::sessions::{self, SessionSpec, ACTIVE_STATUSES};
use crate::state::CloudState;

const DEFAULT_IDLE_TIMEOUT_MINUTES: u32 = 30;
const MAX_IDLE_TIMEOUT_MINUTES: u32 = 240;
const MIN_IDLE_TIMEOUT_MINUTES: u32 = 5;

#[derive(Default, serde::Deserialize)]
pub(crate) struct StartSessionBody {
    idle_timeout_minutes: Option<u32>,
}

pub(crate) fn clamp_idle_timeout(requested: Option<u32>) -> u32 {
    requested
        .unwrap_or(DEFAULT_IDLE_TIMEOUT_MINUTES)
        .clamp(MIN_IDLE_TIMEOUT_MINUTES, MAX_IDLE_TIMEOUT_MINUTES)
}

pub(crate) async fn start_session(
    session: CurrentSession<SqlxAdapter>,
    Extension(cloud): Extension<Arc<CloudState>>,
    Path(project_id): Path<String>,
    body: Option<Json<StartSessionBody>>,
) -> axum::response::Response {
    let user_id = session.user.id();
    type ProjectRow = (
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let project: Result<Option<ProjectRow>, _> = neon_serverless_sqlx::sqlx::query_as(
        "SELECT repo_url, default_branch, setup_command, railway_environment_id, checkpoint_key
         FROM projects WHERE id = $1 AND user_id = $2",
    )
    .bind(&project_id)
    .bind(user_id)
    .fetch_optional(cloud.db.pg())
    .await;
    let (repo_url, default_branch, setup_command, environment_id, checkpoint) = match project {
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
    let Some(environment_id) = environment_id else {
        return error_json(
            StatusCode::CONFLICT,
            "project_unprovisioned",
            "the project has no Railway environment; recreate it",
        );
    };

    let access_token = match railway_token_or_response(&cloud, user_id).await {
        Ok(token) => token,
        Err(response) => return response,
    };

    let idle_timeout_minutes =
        clamp_idle_timeout(body.and_then(|Json(body)| body.idle_timeout_minutes));

    // One active session per project: the insert races are closed by checking
    // and inserting in one statement.
    let inserted: Result<Option<(String,)>, _> = neon_serverless_sqlx::sqlx::query_as(
        "INSERT INTO sandbox_sessions (project_id, status, idle_timeout_minutes)
         SELECT $1, 'pending', $2
         WHERE NOT EXISTS (
            SELECT 1 FROM sandbox_sessions
            WHERE project_id = $1 AND status = ANY($3)
         )
         RETURNING id",
    )
    .bind(&project_id)
    .bind(idle_timeout_minutes as i32)
    .bind(ACTIVE_STATUSES)
    .fetch_optional(cloud.db.pg())
    .await;
    let session_id = match inserted {
        Ok(Some((id,))) => id,
        Ok(None) => {
            return error_json(
                StatusCode::CONFLICT,
                "session_active",
                "this project already has an active session",
            )
        }
        Err(error) => return internal_error("failed to create session", error),
    };

    let spec = SessionSpec {
        session_id: session_id.clone(),
        project_id: project_id.clone(),
        environment_id,
        repo_url,
        default_branch,
        setup_command,
        idle_timeout_minutes,
        access_token,
        checkpoint,
    };
    tokio::spawn(sessions::run(cloud.clone(), spec));

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "session_id": session_id, "status": "pending" })),
    )
        .into_response()
}

pub(crate) async fn list_sessions(
    session: CurrentSession<SqlxAdapter>,
    Extension(cloud): Extension<Arc<CloudState>>,
    Path(project_id): Path<String>,
) -> axum::response::Response {
    type Row = (String, String, Option<String>, String, Option<String>);
    let rows: Result<Vec<Row>, _> = neon_serverless_sqlx::sqlx::query_as(
        "SELECT s.id, s.status, s.status_detail, s.created_at::text, s.ended_at::text
         FROM sandbox_sessions s
         JOIN projects p ON p.id = s.project_id
         WHERE p.id = $1 AND p.user_id = $2
         ORDER BY s.created_at DESC LIMIT 20",
    )
    .bind(&project_id)
    .bind(session.user.id())
    .fetch_all(cloud.db.pg())
    .await;
    match rows {
        Ok(rows) => Json(serde_json::json!({
            "sessions": rows
                .into_iter()
                .map(|(id, status, detail, created_at, ended_at)| serde_json::json!({
                    "id": id,
                    "status": status,
                    "status_detail": detail,
                    "created_at": created_at,
                    "ended_at": ended_at,
                }))
                .collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(error) => internal_error("failed to list sessions", error),
    }
}

type OwnedSessionRow = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
);

async fn owned_session(
    cloud: &CloudState,
    user_id: &str,
    session_id: &str,
) -> Result<Option<OwnedSessionRow>, axum::response::Response> {
    let row: Result<Option<OwnedSessionRow>, _> = neon_serverless_sqlx::sqlx::query_as(
        "SELECT s.status, COALESCE(s.status_detail, ''), s.provider_sandbox_id,
                p.railway_environment_id, s.connection_info::text, p.id
         FROM sandbox_sessions s
         JOIN projects p ON p.id = s.project_id
         WHERE s.id = $1 AND p.user_id = $2",
    )
    .bind(session_id)
    .bind(user_id)
    .fetch_optional(cloud.db.pg())
    .await;
    row.map_err(|error| internal_error("failed to load session", error))
}

pub(crate) async fn get_session(
    session: CurrentSession<SqlxAdapter>,
    Extension(cloud): Extension<Arc<CloudState>>,
    Path(session_id): Path<String>,
) -> axum::response::Response {
    match owned_session(&cloud, session.user.id(), &session_id).await {
        Ok(Some((status, detail, ..))) => Json(serde_json::json!({
            "id": session_id,
            "status": status,
            "status_detail": if detail.is_empty() { None } else { Some(detail) },
        }))
        .into_response(),
        Ok(None) => error_json(
            StatusCode::NOT_FOUND,
            "session_not_found",
            "unknown session",
        ),
        Err(response) => response,
    }
}

pub(crate) async fn get_session_connection(
    session: CurrentSession<SqlxAdapter>,
    Extension(cloud): Extension<Arc<CloudState>>,
    Path(session_id): Path<String>,
) -> axum::response::Response {
    match owned_session(&cloud, session.user.id(), &session_id).await {
        Ok(Some((status, _, _, _, connection_info, _))) => {
            if status != "ready" {
                return error_json(
                    StatusCode::CONFLICT,
                    "session_not_ready",
                    &format!("session is {status}"),
                );
            }
            let Some(connection_info) = connection_info else {
                return error_json(
                    StatusCode::CONFLICT,
                    "session_not_ready",
                    "connection info is not recorded yet",
                );
            };
            match serde_json::from_str::<serde_json::Value>(&connection_info) {
                Ok(value) => Json(value).into_response(),
                Err(error) => internal_error("stored connection info is invalid", error),
            }
        }
        Ok(None) => error_json(
            StatusCode::NOT_FOUND,
            "session_not_found",
            "unknown session",
        ),
        Err(response) => response,
    }
}

pub(crate) async fn stop_session(
    session: CurrentSession<SqlxAdapter>,
    Extension(cloud): Extension<Arc<CloudState>>,
    Path(session_id): Path<String>,
) -> axum::response::Response {
    let user_id = session.user.id();
    let row = match owned_session(&cloud, user_id, &session_id).await {
        Ok(Some(row)) => row,
        Ok(None) => {
            return error_json(
                StatusCode::NOT_FOUND,
                "session_not_found",
                "unknown session",
            )
        }
        Err(response) => return response,
    };
    let (status, _, provider_sandbox_id, environment_id, _, project_id) = row;
    if status == "stopped" || status == "failed" {
        return StatusCode::NO_CONTENT.into_response();
    }

    sessions::set_status(&cloud, &session_id, "stopping", None).await;
    if let (Some(sandbox_id), Some(environment_id)) = (provider_sandbox_id, environment_id) {
        let access_token = match railway_token_or_response(&cloud, user_id).await {
            Ok(token) => token,
            Err(response) => return response,
        };
        // Capture the disk before destroying so the next session boots fast
        // (skips clone + setup). Only worthwhile once the workspace exists.
        if status == "ready" {
            sessions::checkpoint_before_destroy(
                &cloud,
                &access_token,
                &project_id,
                &environment_id,
                &sandbox_id,
            )
            .await;
        }
        if let Err(error) = sessions::destroy_sandbox_best_effort(
            &cloud,
            &access_token,
            &environment_id,
            &sandbox_id,
        )
        .await
        {
            sessions::set_status(&cloud, &session_id, "ready", None).await;
            return error_json(
                StatusCode::BAD_GATEWAY,
                "provider_error",
                &format!("failed to destroy sandbox: {error}"),
            );
        }
    }
    sessions::set_status(&cloud, &session_id, "stopped", None).await;
    StatusCode::NO_CONTENT.into_response()
}

#[cfg(test)]
mod tests {
    use super::clamp_idle_timeout;

    #[test]
    fn idle_timeout_is_clamped() {
        assert_eq!(clamp_idle_timeout(None), 30);
        assert_eq!(clamp_idle_timeout(Some(0)), 5);
        assert_eq!(clamp_idle_timeout(Some(60)), 60);
        assert_eq!(clamp_idle_timeout(Some(100_000)), 240);
    }
}
