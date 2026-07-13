//! Provider-connection routes: link a Railway account to a Termy user.

use std::sync::Arc;

use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect};
use axum::{Extension, Json};
use better_auth::adapters::SqlxAdapter;
use better_auth::handlers::CurrentSession;
use better_auth::AuthUser as _;

use crate::providers::railway_oauth::{self, PROVIDER};
use crate::routes::{error_json, internal_error};
use crate::state::CloudState;

fn railway_unconfigured() -> axum::response::Response {
    error_json(
        StatusCode::NOT_IMPLEMENTED,
        "railway_not_configured",
        "Railway OAuth is not configured on this server",
    )
}

/// Starts the connect flow: persists state + PKCE verifier, redirects to
/// Railway's consent screen.
pub(crate) async fn railway_connect(
    session: CurrentSession<SqlxAdapter>,
    Extension(cloud): Extension<Arc<CloudState>>,
) -> axum::response::Response {
    let Some(config) = &cloud.railway else {
        return railway_unconfigured();
    };
    let request = railway_oauth::authorization_request(config, &cloud.base_url);
    let result = neon_serverless_sqlx::sqlx::query(
        "INSERT INTO provider_oauth_states (state, user_id, provider, pkce_verifier, expires_at)
         VALUES ($1, $2, $3, $4, NOW() + interval '10 minutes')",
    )
    .bind(&request.state)
    .bind(session.user.id())
    .bind(PROVIDER)
    .bind(&request.pkce_verifier)
    .execute(cloud.db.pg())
    .await;
    if let Err(error) = result {
        return internal_error("failed to persist Railway OAuth state", error);
    }
    Redirect::temporary(&request.authorize_url).into_response()
}

#[derive(serde::Deserialize)]
pub(crate) struct RailwayCallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// OAuth callback: consumes the single-use state row, exchanges the code, and
/// stores the encrypted connection. The state row carries the user id, so no
/// browser session is required here.
pub(crate) async fn railway_callback(
    Query(params): Query<RailwayCallbackParams>,
    Extension(cloud): Extension<Arc<CloudState>>,
) -> axum::response::Response {
    let Some(config) = &cloud.railway else {
        return railway_unconfigured();
    };
    if let Some(error) = params.error {
        return Redirect::temporary(&format!("/dashboard/projects?railway_error={error}"))
            .into_response();
    }
    let (Some(code), Some(state)) = (params.code, params.state) else {
        return error_json(
            StatusCode::BAD_REQUEST,
            "invalid_callback",
            "missing code or state parameter",
        );
    };

    let row: Result<Option<(String, String, Option<String>)>, _> =
        neon_serverless_sqlx::sqlx::query_as(
            "DELETE FROM provider_oauth_states
             WHERE state = $1 AND provider = $2 AND expires_at > NOW()
             RETURNING user_id, pkce_verifier, redirect_to",
        )
        .bind(&state)
        .bind(PROVIDER)
        .fetch_optional(cloud.db.pg())
        .await;
    let row = match row {
        Ok(row) => row,
        Err(error) => return internal_error("failed to consume Railway OAuth state", error),
    };
    let Some((user_id, pkce_verifier, redirect_to)) = row else {
        return error_json(
            StatusCode::BAD_REQUEST,
            "invalid_state",
            "unknown or expired OAuth state",
        );
    };

    let tokens = match railway_oauth::exchange_code(
        &cloud.http,
        config,
        &cloud.base_url,
        &code,
        &pkce_verifier,
    )
    .await
    {
        Ok(tokens) => tokens,
        Err(error) => return internal_error("Railway code exchange failed", error),
    };
    if let Err(error) = railway_oauth::store_connection(&cloud, &user_id, &tokens).await {
        return internal_error("failed to store Railway connection", error);
    }
    Redirect::temporary(redirect_to.as_deref().unwrap_or("/dashboard/projects")).into_response()
}

/// Connection status; never returns tokens.
pub(crate) async fn railway_status(
    session: CurrentSession<SqlxAdapter>,
    Extension(cloud): Extension<Arc<CloudState>>,
) -> axum::response::Response {
    type ConnectionRow = (Option<String>, Option<String>, Option<String>);
    let row: Result<Option<ConnectionRow>, _> = neon_serverless_sqlx::sqlx::query_as(
        "SELECT provider_account_name, scopes, access_token_expires_at::text
             FROM provider_connections WHERE user_id = $1 AND provider = $2",
    )
    .bind(session.user.id())
    .bind(PROVIDER)
    .fetch_optional(cloud.db.pg())
    .await;
    match row {
        Ok(Some((account_name, scopes, expires_at))) => Json(serde_json::json!({
            "connected": true,
            "account_name": account_name,
            "scopes": scopes,
            "expires_at": expires_at,
        }))
        .into_response(),
        Ok(None) => Json(serde_json::json!({ "connected": false })).into_response(),
        Err(error) => internal_error("failed to load Railway connection", error),
    }
}

pub(crate) async fn railway_disconnect(
    session: CurrentSession<SqlxAdapter>,
    Extension(cloud): Extension<Arc<CloudState>>,
) -> axum::response::Response {
    let result = neon_serverless_sqlx::sqlx::query(
        "DELETE FROM provider_connections WHERE user_id = $1 AND provider = $2",
    )
    .bind(session.user.id())
    .bind(PROVIDER)
    .execute(cloud.db.pg())
    .await;
    match result {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => internal_error("failed to disconnect Railway", error),
    }
}
