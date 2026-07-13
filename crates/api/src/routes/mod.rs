use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use better_auth::adapters::SqlxAdapter;
use better_auth::handlers::CurrentSession;
use better_auth::AuthUser as _;

use crate::db::QueryPool;
use crate::providers::railway_oauth::{self, AccessTokenError};
use crate::state::CloudState;

pub(crate) mod projects;
pub(crate) mod providers;
pub(crate) mod sessions;
pub(crate) mod terminal;

pub(crate) fn error_json(
    status: StatusCode,
    code: &str,
    message: &str,
) -> axum::response::Response {
    (
        status,
        Json(serde_json::json!({ "code": code, "message": message })),
    )
        .into_response()
}

pub(crate) fn internal_error(
    context: &str,
    error: impl std::fmt::Display,
) -> axum::response::Response {
    tracing::error!("{context}: {error}");
    error_json(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", context)
}

/// Fetches a valid Railway access token for `user_id`, mapping the failure
/// modes to machine-readable HTTP errors for the CLI.
pub(crate) async fn railway_token_or_response(
    cloud: &CloudState,
    user_id: &str,
) -> Result<String, axum::response::Response> {
    railway_oauth::access_token_for(cloud, user_id)
        .await
        .map_err(|error| match error {
            AccessTokenError::NotConnected => error_json(
                StatusCode::CONFLICT,
                "railway_not_connected",
                "Connect your Railway account first (termy cloud connect railway)",
            ),
            AccessTokenError::ReauthRequired(message) => error_json(
                StatusCode::CONFLICT,
                "railway_reauth_required",
                &format!("Reconnect your Railway account: {message}"),
            ),
            AccessTokenError::Internal(error) => {
                internal_error("failed to load Railway credentials", error)
            }
        })
}

pub(crate) async fn not_found() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "message": "Not found" })),
    )
}

pub(crate) fn auth_config(github_enabled: bool) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "github": github_enabled,
    }))
}

pub(crate) async fn health(State(db): State<Arc<QueryPool>>) -> impl IntoResponse {
    let db_ok = neon_serverless_sqlx::sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(db.pg())
        .await
        .is_ok();
    let status = if db_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(serde_json::json!({
            "status": if db_ok { "ok" } else { "degraded" },
            "database": db_ok,
        })),
    )
}

pub(crate) async fn me(session: CurrentSession<SqlxAdapter>) -> impl IntoResponse {
    Json(serde_json::json!({
        "id": session.user.id(),
        "email": session.user.email(),
        "name": session.user.name(),
    }))
}
