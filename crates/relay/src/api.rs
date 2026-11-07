use crate::{
    AppState,
    db::Device,
    error::{ApiError, ApiResult},
    hub::Hub,
};
use axum::{
    Json, Router,
    extract::{
        ConnectInfo, FromRequestParts, Path, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{StatusCode, request::Parts},
    response::Response,
    routing::{delete, get, post},
};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use chrono::{DateTime, SecondsFormat, Utc};
use futures_util::{SinkExt, StreamExt};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    net::SocketAddr,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use uuid::Uuid;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/v2/devices", post(register).get(devices))
        .route("/v2/device", delete(delete_self).put(update_name))
        .route("/v2/links/{id}", delete(unpair))
        .route("/v2/pairings", post(create_pairing))
        .route("/v2/pairings/{id}", get(pairing).delete(cancel_pairing))
        .route("/v2/pairings/{id}/claim", post(claim))
        .route("/v2/pairings/{id}/approve", post(approve))
        .route("/v2/relay", get(relay))
        .layer(tower_http::limit::RequestBodyLimitLayer::new(16 * 1024))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}

#[derive(Default)]
pub struct AttemptLimiter {
    attempts: Mutex<HashMap<String, Vec<Instant>>>,
}
impl AttemptLimiter {
    async fn allow(&self, key: String) -> bool {
        let mut attempts = self.attempts.lock().await;
        attempts.retain(|_, v| {
            v.retain(|t| t.elapsed() < Duration::from_secs(300));
            !v.is_empty()
        });
        if attempts.len() >= 10_000 && !attempts.contains_key(&key) {
            return false;
        }
        let recent = attempts.entry(key).or_default();
        if recent.len() >= 30 {
            return false;
        }
        recent.push(Instant::now());
        true
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Registration {
    encrypted_name: Option<String>,
    kind: String,
    public_key: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceDto {
    id: Uuid,
    name: String,
    kind: String,
    public_key: String,
    online: bool,
    created_at: String,
    last_seen_at: Option<String>,
}
fn date(d: DateTime<Utc>) -> String {
    d.to_rfc3339_opts(SecondsFormat::Secs, true)
}
async fn dto(state: &AppState, d: Device) -> DeviceDto {
    DeviceDto {
        id: d.id,
        online: state.hub.is_online(d.id).await,
        name: if d.kind == "host" { "Mac" } else { "iPhone" }.to_owned(),
        kind: d.kind,
        public_key: STANDARD.encode(d.public_key),
        created_at: date(d.created_at),
        last_seen_at: d.last_seen_at.map(date),
    }
}
#[derive(Serialize)]
struct Registered {
    token: String,
    device: DeviceDto,
}
#[derive(Serialize)]
struct Empty {}
fn gone() -> ApiError {
    ApiError::new(
        StatusCode::GONE,
        "This pairing expired or was cancelled. Scan a new QR code.",
    )
}
fn forbidden() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "This device is not allowed to do that.",
    )
}
fn bytes(s: &str, size: usize) -> ApiResult<Vec<u8>> {
    STANDARD
        .decode(s)
        .ok()
        .filter(|b| b.len() == size)
        .ok_or_else(|| ApiError::bad_request("Invalid pairing data."))
}
fn hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}
async fn register(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(body): Json<Registration>,
) -> ApiResult<Json<Registered>> {
    if !state.limiter.allow(addr.ip().to_string()).await {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many device registrations. Try again later.",
        ));
    }
    if body.kind != "host" && body.kind != "client" {
        return Err(ApiError::bad_request("Unknown device kind."));
    }
    let key = bytes(&body.public_key, 32)?;
    let encrypted_name = body
        .encrypted_name
        .as_deref()
        .map(sealed_name)
        .transpose()?;
    let mut raw = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    let token = URL_SAFE_NO_PAD.encode(raw);
    let d=sqlx::query_as::<_,Device>("INSERT INTO remote_devices(encrypted_name,kind,public_key,token_hash) VALUES($1,$2,$3,$4) RETURNING id,kind,public_key,created_at,last_seen_at")
        .bind(encrypted_name).bind(body.kind).bind(key).bind(hash(&token)).fetch_one(&state.db.pool).await?;
    Ok(Json(Registered {
        token,
        device: dto(&state, d).await,
    }))
}
// A name envelope is: version byte + random 12-byte nonce + 256 padded bytes + tag.
fn sealed_name(s: &str) -> ApiResult<Vec<u8>> {
    let value = bytes(s, 285)?;
    if value[0] != 1 {
        return Err(ApiError::bad_request(
            "Unsupported private metadata format.",
        ));
    }
    Ok(value)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NameUpdate {
    encrypted_name: String,
}
async fn update_name(
    State(state): State<AppState>,
    Authed(d): Authed,
    Json(body): Json<NameUpdate>,
) -> ApiResult<Json<Empty>> {
    let encrypted_name = sealed_name(&body.encrypted_name)?;
    sqlx::query("UPDATE remote_devices SET encrypted_name=$2 WHERE id=$1")
        .bind(d.id)
        .bind(encrypted_name)
        .execute(&state.db.pool)
        .await?;
    Ok(Json(Empty {}))
}
struct Authed(Device);
impl FromRequestParts<AppState> for Authed {
    type Rejection = ApiError;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .ok_or_else(|| ApiError::unauthorized("Pair this device again."))?;
        state
            .db
            .authenticate(&hash(token))
            .await?
            .map(Self)
            .ok_or_else(|| ApiError::unauthorized("This device registration ended. Pair again."))
    }
}
async fn devices(
    State(state): State<AppState>,
    Authed(d): Authed,
) -> ApiResult<Json<Vec<DeviceDto>>> {
    let mut out = vec![];
    for peer in state.db.peers(d.id).await? {
        out.push(dto(&state, peer).await);
    }
    Ok(Json(out))
}
async fn unpair(
    State(state): State<AppState>,
    Authed(d): Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Empty>> {
    let _guard = state.gate.lock().await;
    sqlx::query("DELETE FROM remote_links WHERE (host_id=$1 AND client_id=$2) OR (host_id=$2 AND client_id=$1)").bind(d.id).bind(id).execute(&state.db.pool).await?;
    // Delete pending/approved retrieval too: a delayed poll must not resurrect removed access.
    sqlx::query("DELETE FROM remote_pairings WHERE (host_id=$1 AND client_id=$2) OR (host_id=$2 AND client_id=$1)").bind(d.id).bind(id).execute(&state.db.pool).await?;
    state.hub.revoke(d.id, id).await;
    Ok(Json(Empty {}))
}
async fn delete_self(State(state): State<AppState>, Authed(d): Authed) -> ApiResult<Json<Empty>> {
    let _guard = state.gate.lock().await;
    let peers = state.db.peers(d.id).await?;
    sqlx::query("DELETE FROM remote_devices WHERE id=$1")
        .bind(d.id)
        .execute(&state.db.pool)
        .await?;
    for peer in peers {
        state.hub.revoke(d.id, peer.id).await;
    }
    state.hub.kick(d.id).await;
    Ok(Json(Empty {}))
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PairingDto {
    id: Uuid,
    expires_at: String,
    host: DeviceDto,
    client: Option<DeviceDto>,
    init_packet: Option<String>,
    proof: Option<String>,
    response_packet: Option<String>,
    encrypted_client_name: Option<String>,
}
async fn pairing_dto(state: &AppState, p: crate::db::Pairing) -> ApiResult<PairingDto> {
    Ok(PairingDto {
        id: p.id,
        expires_at: date(p.expires_at),
        host: dto(state, state.db.device(p.host_id).await?).await,
        client: match p.client_id {
            Some(id) => Some(dto(state, state.db.device(id).await?).await),
            None => None,
        },
        init_packet: p.init_packet.map(|b| STANDARD.encode(b)),
        proof: p.proof.map(|b| STANDARD.encode(b)),
        response_packet: p.response_packet.map(|b| STANDARD.encode(b)),
        encrypted_client_name: p.encrypted_client_name.map(|b| STANDARD.encode(b)),
    })
}
async fn create_pairing(
    State(state): State<AppState>,
    Authed(d): Authed,
) -> ApiResult<Json<PairingDto>> {
    if d.kind != "host" {
        return Err(forbidden());
    }
    let _guard = state.gate.lock().await;
    let mut tx = state.db.pool.begin().await?;
    sqlx::query("DELETE FROM remote_pairings WHERE (host_id=$1 AND response_packet IS NULL) OR expires_at<now()-interval '10 minutes'").bind(d.id).execute(&mut *tx).await?;
    let p = sqlx::query_as("INSERT INTO remote_pairings(host_id) VALUES($1) RETURNING *")
        .bind(d.id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(pairing_dto(&state, p).await?))
}
async fn pairing(
    State(state): State<AppState>,
    Authed(d): Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<PairingDto>> {
    let p = state.db.pairing(id).await?.ok_or_else(gone)?;
    if p.host_id != d.id && p.client_id != Some(d.id) {
        return Err(forbidden());
    }
    Ok(Json(pairing_dto(&state, p).await?))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Claim {
    init_packet: String,
    proof: String,
    encrypted_client_name: Option<String>,
}
async fn claim(
    State(state): State<AppState>,
    Authed(d): Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<Claim>,
) -> ApiResult<Json<Empty>> {
    if d.kind != "client" {
        return Err(forbidden());
    }
    let init = bytes(&body.init_packet, 65)?;
    let proof = bytes(&body.proof, 32)?;
    let encrypted_name = body
        .encrypted_client_name
        .as_deref()
        .map(sealed_name)
        .transpose()?;
    if init[0] != 1 || init[33..] != d.public_key {
        return Err(ApiError::bad_request(
            "The pairing identity does not match this device.",
        ));
    }
    let changed=sqlx::query("UPDATE remote_pairings SET client_id=$2,init_packet=$3,proof=$4,encrypted_client_name=$5 WHERE id=$1 AND expires_at>now() AND response_packet IS NULL AND (client_id IS NULL OR (client_id=$2 AND init_packet=$3 AND proof=$4 AND encrypted_client_name IS NOT DISTINCT FROM $5))")
        .bind(id).bind(d.id).bind(init).bind(proof).bind(encrypted_name).execute(&state.db.pool).await?;
    if changed.rows_affected() != 1 {
        return Err(gone());
    }
    Ok(Json(Empty {}))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Approval {
    client_id: Uuid,
    init_packet: String,
    response_packet: String,
}
async fn approve(
    State(state): State<AppState>,
    Authed(d): Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<Approval>,
) -> ApiResult<Json<Empty>> {
    let init = bytes(&body.init_packet, 65)?;
    let response = bytes(&body.response_packet, 59)?; // kind + ephemeral + counter + confirmation + tag
    if response[0] != 2 {
        return Err(ApiError::bad_request("Invalid handshake response."));
    }
    let _guard = state.gate.lock().await;
    let mut tx = state.db.pool.begin().await?;
    let p=sqlx::query_as::<_,crate::db::Pairing>("SELECT * FROM remote_pairings WHERE id=$1 AND host_id=$2 AND expires_at>now() AND response_packet IS NULL FOR UPDATE")
        .bind(id).bind(d.id).fetch_optional(&mut *tx).await?.ok_or_else(gone)?;
    if p.client_id != Some(body.client_id) || p.init_packet.as_deref() != Some(init.as_slice()) {
        return Err(forbidden());
    }
    sqlx::query("INSERT INTO remote_links(host_id,client_id) VALUES($1,$2) ON CONFLICT DO NOTHING")
        .bind(d.id)
        .bind(body.client_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE remote_pairings SET response_packet=$2 WHERE id=$1")
        .bind(id)
        .bind(response)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    state.hub.authorize(d.id, body.client_id).await;
    Ok(Json(Empty {}))
}
async fn cancel_pairing(
    State(state): State<AppState>,
    Authed(d): Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Empty>> {
    let _guard = state.gate.lock().await;
    sqlx::query(
        "DELETE FROM remote_pairings WHERE id=$1 AND host_id=$2 AND response_packet IS NULL",
    )
    .bind(id)
    .bind(d.id)
    .execute(&state.db.pool)
    .await?;
    Ok(Json(Empty {}))
}
async fn relay(
    State(state): State<AppState>,
    Authed(d): Authed,
    upgrade: WebSocketUpgrade,
) -> Response {
    upgrade
        .max_message_size(4 << 20)
        .max_frame_size(4 << 20)
        .on_upgrade(move |socket| serve(state, d, socket))
}
async fn serve(state: AppState, d: Device, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = Hub::queue();
    let serial = {
        let _guard = state.gate.lock().await;
        // Recheck registration after upgrade and serialize peer lookup with revocation.
        let Ok(peers) = state.db.peers(d.id).await else {
            return;
        };
        if state.db.device(d.id).await.is_err() {
            return;
        }
        state
            .hub
            .connect(d.id, peers.into_iter().map(|p| p.id).collect(), tx.clone())
            .await
    };
    state.db.touch(d.id).await;
    let writer = tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            let close = matches!(m, Message::Close(_));
            if sink.send(m).await.is_err() || close {
                break;
            }
        }
    });
    let mut heartbeat = tokio::time::interval(Duration::from_secs(20));
    let mut membership = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            message=stream.next()=>match message {
                Some(Ok(Message::Binary(b)))=>state.hub.route(d.id,serial,b.to_vec()).await,
                Some(Ok(Message::Close(_)) | Err(_)) | None=>break,
                _=>{}
            },
            _=heartbeat.tick()=> { if tx.try_send(Message::Ping(Vec::new().into())).is_err() { break; } },
            _=membership.tick()=> { if writer.is_finished() || !state.hub.is_current(d.id,serial).await { break; } }
        }
    }
    writer.abort();
    state.hub.disconnect(d.id, serial).await;
    state.db.touch(d.id).await;
}
