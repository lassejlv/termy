//! Server-side terminal relay.
//!
//! The browser and CLI connect here; the Termy server mints the sandbox shell
//! token and dials the provider's exec websocket. Clients never see the
//! provider endpoint or the token — they speak the exec wire protocol
//! (`init_exec`, tagged stdin/stdout frames) through this transparent relay.

use std::sync::Arc;

use axum::extract::ws::{Message as AxumMessage, WebSocket, WebSocketUpgrade};
use axum::extract::Path;
use axum::http::StatusCode;
use axum::Extension;
use better_auth::adapters::SqlxAdapter;
use better_auth::handlers::CurrentSession;
use better_auth::AuthUser as _;
use futures_util::{SinkExt as _, StreamExt as _};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::providers::{ProviderCtx, ShellConnection};
use crate::routes::{error_json, railway_token_or_response};
use crate::state::CloudState;

const SHELL_SUBPROTOCOL: &str = "railway-shell";

pub(crate) async fn terminal_ws(
    ws: WebSocketUpgrade,
    session: CurrentSession<SqlxAdapter>,
    Extension(cloud): Extension<Arc<CloudState>>,
    Path(session_id): Path<String>,
) -> axum::response::Response {
    // Authorize and mint the shell token before upgrading so failures return a
    // normal HTTP error instead of a silent socket close.
    let shell = match authorize(&cloud, session.user.id(), &session_id).await {
        Ok(shell) => shell,
        Err(response) => return response,
    };
    ws.on_upgrade(move |socket| relay(socket, shell))
}

async fn authorize(
    cloud: &CloudState,
    user_id: &str,
    session_id: &str,
) -> Result<ShellConnection, axum::response::Response> {
    type SessionRow = (String, Option<String>, Option<String>);
    let row: Result<Option<SessionRow>, _> = neon_serverless_sqlx::sqlx::query_as(
        "SELECT s.status, s.provider_sandbox_id, p.railway_environment_id
             FROM sandbox_sessions s
             JOIN projects p ON p.id = s.project_id
             WHERE s.id = $1 AND p.user_id = $2",
    )
    .bind(session_id)
    .bind(user_id)
    .fetch_optional(cloud.db.pg())
    .await;
    let (status, sandbox_id, environment_id) = match row {
        Ok(Some(row)) => row,
        Ok(None) => {
            return Err(error_json(
                StatusCode::NOT_FOUND,
                "session_not_found",
                "unknown session",
            ))
        }
        Err(error) => {
            tracing::error!("failed to load session for terminal: {error}");
            return Err(error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "failed to load session",
            ));
        }
    };
    if status != "ready" {
        return Err(error_json(
            StatusCode::CONFLICT,
            "session_not_ready",
            &format!("session is {status}"),
        ));
    }
    let (Some(sandbox_id), Some(environment_id)) = (sandbox_id, environment_id) else {
        return Err(error_json(
            StatusCode::CONFLICT,
            "session_not_ready",
            "the session has no sandbox recorded",
        ));
    };
    let access_token = railway_token_or_response(cloud, user_id).await?;
    let ctx = ProviderCtx { access_token };
    cloud
        .provider
        .shell_token(&ctx, &environment_id, &sandbox_id)
        .await
        .map_err(|error| {
            error_json(
                StatusCode::BAD_GATEWAY,
                "provider_error",
                &format!("failed to mint shell token: {error}"),
            )
        })
}

/// Pipes frames between the client socket and the provider exec socket until
/// either side closes.
async fn relay(client: WebSocket, shell: ShellConnection) {
    let provider = match dial_provider(&shell).await {
        Ok(provider) => provider,
        Err(error) => {
            tracing::warn!("terminal relay could not reach provider: {error}");
            let mut client = client;
            let _ = client
                .send(AxumMessage::Close(Some(axum::extract::ws::CloseFrame {
                    code: 1011,
                    reason: "could not reach the sandbox".into(),
                })))
                .await;
            return;
        }
    };

    let (mut client_tx, mut client_rx) = client.split();
    let (mut provider_tx, mut provider_rx) = provider.split();

    // Client → provider.
    let to_provider = async {
        while let Some(Ok(message)) = client_rx.next().await {
            let forwarded = match message {
                AxumMessage::Text(text) => WsMessage::Text(text.as_str().into()),
                AxumMessage::Binary(bytes) => WsMessage::Binary(bytes.to_vec()),
                AxumMessage::Close(_) => break,
                // Ping/pong are handled by each hop's own keepalive.
                AxumMessage::Ping(_) | AxumMessage::Pong(_) => continue,
            };
            if provider_tx.send(forwarded).await.is_err() {
                break;
            }
        }
        let _ = provider_tx.send(WsMessage::Close(None)).await;
    };

    // Provider → client.
    let to_client = async {
        while let Some(Ok(message)) = provider_rx.next().await {
            let forwarded = match message {
                WsMessage::Text(text) => AxumMessage::Text(text.as_str().into()),
                WsMessage::Binary(bytes) => AxumMessage::Binary(bytes.clone().into()),
                WsMessage::Close(_) => break,
                WsMessage::Ping(_) | WsMessage::Pong(_) | WsMessage::Frame(_) => continue,
            };
            if client_tx.send(forwarded).await.is_err() {
                break;
            }
        }
        let _ = client_tx.send(AxumMessage::Close(None)).await;
    };

    tokio::select! {
        () = to_provider => {}
        () = to_client => {}
    }
}

async fn dial_provider(
    shell: &ShellConnection,
) -> anyhow::Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
> {
    let mut request = shell.ws_url.as_str().into_client_request()?;
    // The JWT rides as the last Sec-WebSocket-Protocol value.
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        format!("{SHELL_SUBPROTOCOL}, {}", shell.token).parse()?,
    );
    let (stream, _response) = tokio_tungstenite::connect_async(request).await?;
    Ok(stream)
}
