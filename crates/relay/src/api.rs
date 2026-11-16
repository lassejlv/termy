use crate::{
    AppState,
    db::Device,
    error::{ApiError, ApiResult},
    hub::{Hub, Inbox, Outbox},
    limit::Client,
    push::{self, Delivery, Environment},
};
use axum::{
    Extension, Json, Router,
    extract::{
        ConnectInfo, FromRequestParts, Path, Request, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{StatusCode, request::Parts},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
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
use std::{net::SocketAddr, time::Duration};
use tokio::time::Instant;
use uuid::Uuid;

/// How long a request may take, body included, before it is answered with 408.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const HEARTBEAT: Duration = Duration::from_secs(20);
/// A socket that sends nothing for this long, not even a pong, is gone.
const SILENCE: Duration = Duration::from_secs(75);
/// How long one frame may take to leave for a device that has stopped reading.
const WRITE_STALL: Duration = Duration::from_secs(60);
/// Sealed bytes a device may send at once, and per second after that. Past
/// it the relay reads the socket more slowly; nothing is dropped.
const SEND_BURST: u64 = 32 << 20;
const SEND_RATE: u64 = 2 << 20;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v2/devices", post(register).get(devices))
        .route("/v2/device", delete(delete_self).put(update_name))
        .route(
            "/v2/device/push",
            put(set_push_token).delete(clear_push_token),
        )
        .route("/v2/links/{id}", delete(unpair))
        .route("/v2/push/{id}", post(push_to))
        .route("/v2/pairings", post(create_pairing))
        .route("/v2/pairings/{id}", get(pairing).delete(cancel_pairing))
        .route("/v2/pairings/{id}/claim", post(claim))
        .route("/v2/pairings/{id}/approve", post(approve))
        .route("/v2/relay", get(relay))
        .layer(middleware::from_fn_with_state(state.clone(), throttle))
        .route("/health", get(|| async { "ok" }))
        .layer(tower_http::limit::RequestBodyLimitLayer::new(16 * 1024))
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}

/// Works out who is asking and turns them away, before authentication costs
/// a database query, once they ask too often.
async fn throttle(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    mut request: Request,
    next: Next,
) -> Response {
    let client = state.limits.client(addr.ip(), request.headers());
    if client.distinct && !state.limits.requests.allow(client.ip) {
        return ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many requests. Try again shortly.",
        )
        .into_response();
    }
    request.extensions_mut().insert(client);
    next.run(request).await
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
    Extension(client): Extension<Client>,
    Json(body): Json<Registration>,
) -> ApiResult<Json<Registered>> {
    if !state.limits.registrations.allow(client.ip) {
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
    let _guard = state.gates.lock(&[d.id, id]).await;
    sqlx::query("DELETE FROM remote_links WHERE (host_id=$1 AND client_id=$2) OR (host_id=$2 AND client_id=$1)").bind(d.id).bind(id).execute(&state.db.pool).await?;
    // Delete pending/approved retrieval too: a delayed poll must not resurrect removed access.
    sqlx::query("DELETE FROM remote_pairings WHERE (host_id=$1 AND client_id=$2) OR (host_id=$2 AND client_id=$1)").bind(d.id).bind(id).execute(&state.db.pool).await?;
    // A phone no Mac can reach any more has nothing to be woken for.
    state.db.drop_unlinked_push_tokens(&[d.id, id]).await?;
    state.hub.revoke(d.id, id).await;
    Ok(Json(Empty {}))
}
async fn delete_self(State(state): State<AppState>, Authed(d): Authed) -> ApiResult<Json<Empty>> {
    // Only this device's gate: a peer connecting meanwhile may keep this ID
    // as a peer, but the ID is dead and can never be online again.
    let _guard = state.gates.lock(&[d.id]).await;
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
#[derive(Deserialize)]
struct PushRegistration {
    token: String,
    environment: String,
}
async fn set_push_token(
    State(state): State<AppState>,
    Authed(d): Authed,
    Json(body): Json<PushRegistration>,
) -> ApiResult<Json<Empty>> {
    if d.kind != "client" {
        return Err(forbidden());
    }
    let token = push::parse_token(&body.token)
        .ok_or_else(|| ApiError::bad_request("Invalid push token."))?;
    let environment = Environment::parse(&body.environment)
        .ok_or_else(|| ApiError::bad_request("Unknown push environment."))?;
    state
        .db
        .set_push_token(d.id, &token, environment.as_str())
        .await?;
    Ok(Json(Empty {}))
}
async fn clear_push_token(
    State(state): State<AppState>,
    Authed(d): Authed,
) -> ApiResult<Json<Empty>> {
    state.db.clear_push_token(d.id).await?;
    Ok(Json(Empty {}))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PushRequest {
    sealed: String,
    collapse_id: Option<String>,
}
#[derive(Serialize)]
struct Pushed {
    sent: bool,
}
/// A Mac wakes a phone it is paired with. The relay can't read `sealed`.
async fn push_to(
    State(state): State<AppState>,
    Authed(d): Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<PushRequest>,
) -> ApiResult<Json<Pushed>> {
    if d.kind != "host" {
        return Err(forbidden());
    }
    let Some(apns) = state.push.clone() else {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Push notifications aren't set up on this relay.",
        ));
    };
    let sealed = STANDARD
        .decode(&body.sealed)
        .ok()
        .filter(|b| (1..=push::MAX_SEALED).contains(&b.len()))
        .ok_or_else(|| ApiError::bad_request("Invalid notification."))?;
    if let Some(id) = &body.collapse_id
        && !push::valid_collapse_id(id)
    {
        return Err(ApiError::bad_request("Invalid collapse ID."));
    }
    if !state.limits.pushes.allow(d.id) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many notifications. Try again shortly.",
        ));
    }
    if !state.db.linked(d.id, id).await? {
        return Err(forbidden());
    }
    let Some((token, environment)) = state.db.push_token(id).await? else {
        return Ok(Json(Pushed { sent: false }));
    };
    let environment = Environment::parse(&environment).unwrap_or(Environment::Production);
    let delivery = apns
        .send(
            &token,
            environment,
            d.id,
            &sealed,
            body.collapse_id.as_deref(),
        )
        .await;
    match delivery {
        Ok(Delivery::Sent) => Ok(Json(Pushed { sent: true })),
        Ok(Delivery::Unregistered) => {
            tracing::info!(
                "apns: token {} is no longer valid; forgetting it",
                push::token_tag(&token)
            );
            state.db.forget_push_token(id, &token).await?;
            Ok(Json(Pushed { sent: false }))
        }
        Err(failure) => {
            tracing::warn!(
                "apns: refused token {} with status {:?}: {}",
                push::token_tag(&token),
                failure.status,
                failure.reason
            );
            Err(ApiError::new(
                StatusCode::BAD_GATEWAY,
                "Apple's push service didn't take the notification. Try again.",
            ))
        }
    }
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
    let _guard = state.gates.lock(&[d.id]).await;
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
    let _guard = state.gates.lock(&[d.id, body.client_id]).await;
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
    let _guard = state.gates.lock(&[d.id]).await;
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
/// What a socket may still send before the relay slows down reading it.
struct SendBudget {
    available: u64,
    at: Instant,
}
impl SendBudget {
    /// Charges a frame. Returns how long to stop reading once the burst is spent.
    fn charge(&mut self, bytes: usize, now: Instant) -> Option<Duration> {
        let earned = now.saturating_duration_since(self.at).as_millis() as u64 * SEND_RATE / 1000;
        self.available = self.available.saturating_add(earned).min(SEND_BURST);
        self.at = self.at.max(now);
        let bytes = bytes as u64;
        if bytes <= self.available {
            self.available -= bytes;
            return None;
        }
        let wait = Duration::from_millis((bytes - self.available) * 1000 / SEND_RATE);
        self.available = 0;
        self.at = now + wait;
        Some(wait)
    }
}
async fn serve(state: AppState, d: Device, socket: WebSocket) {
    let (tx, rx) = state.hub.queue();
    let serial = {
        let _guard = state.gates.lock(&[d.id]).await;
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
    pump(&state.hub, d.id, serial, tx, rx, socket).await;
    state.db.touch(d.id).await;
}
/// Moves frames between a connected device's socket and the hub until either ends.
async fn pump(hub: &Hub, device: Uuid, serial: u64, tx: Outbox, mut rx: Inbox, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let writer = tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            let close = matches!(m, Message::Close(_));
            let sent = tokio::time::timeout(WRITE_STALL, sink.send(m)).await;
            if !matches!(sent, Ok(Ok(()))) || close {
                break;
            }
        }
    });
    let mut heartbeat = tokio::time::interval(HEARTBEAT);
    let mut membership = tokio::time::interval(Duration::from_secs(1));
    let mut heard = Instant::now();
    let mut budget = SendBudget {
        available: SEND_BURST,
        at: heard,
    };
    let mut resume = std::pin::pin!(tokio::time::sleep(Duration::ZERO));
    let mut paused = false;
    loop {
        tokio::select! {
            message=stream.next(), if !paused=>{
                heard = Instant::now();
                match message {
                    Some(Ok(Message::Binary(b)))=>{
                        if let Some(wait) = budget.charge(b.len(), heard) {
                            resume.as_mut().reset(heard + wait);
                            paused = true;
                        }
                        hub.route(device,serial,b.to_vec()).await;
                    }
                    Some(Ok(Message::Close(_)) | Err(_)) | None=>break,
                    _=>{}
                }
            },
            () = &mut resume, if paused=>paused = false,
            _=heartbeat.tick()=> { if heard.elapsed() > SILENCE || !tx.try_send(Message::Ping(Vec::new().into())) { break; } },
            _=membership.tick()=> { if writer.is_finished() || !hub.is_current(device,serial).await { break; } }
        }
    }
    writer.abort();
    hub.disconnect(device, serial).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use std::{collections::HashSet, sync::Arc};
    use tokio_tungstenite::tungstenite::Message as Sent;
    use tower::ServiceExt;

    type Peer = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;

    /// A relay socket without the database: `/{device}/{peer}` connects
    /// `device`, already paired with `peer`.
    async fn sockets(hub: Arc<Hub>) -> SocketAddr {
        async fn open(
            State(hub): State<Arc<Hub>>,
            Path((device, peer)): Path<(Uuid, Uuid)>,
            upgrade: WebSocketUpgrade,
        ) -> Response {
            upgrade
                .max_message_size(4 << 20)
                .max_frame_size(4 << 20)
                .on_upgrade(async move |socket| {
                    let (tx, rx) = hub.queue();
                    let serial = hub.connect(device, HashSet::from([peer]), tx.clone()).await;
                    pump(&hub, device, serial, tx, rx, socket).await;
                })
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/{device}/{peer}", get(open))
            .with_state(hub);
        tokio::spawn(crate::serve(
            listener,
            app,
            Duration::from_secs(5),
            std::future::pending(),
        ));
        addr
    }
    /// Connects `device` and waits until the hub has it: the handshake
    /// finishes before the relay registers the socket.
    async fn dial(hub: &Hub, addr: SocketAddr, device: Uuid, peer: Uuid) -> Peer {
        let url = format!("ws://{addr}/{device}/{peer}");
        let socket = tokio_tungstenite::connect_async(url).await.unwrap().0;
        while !hub.is_online(device).await {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        socket
    }
    /// The next sealed frame, skipping relay events and pings.
    async fn frame(socket: &mut Peer) -> Vec<u8> {
        loop {
            let next = tokio::time::timeout(Duration::from_secs(10), socket.next());
            let message = next.await.expect("no frame arrived").unwrap().unwrap();
            if let Sent::Binary(bytes) = message {
                return bytes.to_vec();
            }
        }
    }
    fn sealed(peer: Uuid, size: usize) -> Vec<u8> {
        let mut out = vec![crate::hub::DATA_KIND];
        out.extend_from_slice(peer.as_bytes());
        out.resize(17 + size, 7);
        out
    }

    #[tokio::test]
    async fn sockets_exchange_frames_until_one_leaves() {
        let hub = Arc::new(Hub::default());
        let addr = sockets(hub.clone()).await;
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let mut left = dial(&hub, addr, a, b).await;
        let mut right = dial(&hub, addr, b, a).await;
        // More than the queue holds at once, both ways, with a full-size frame.
        for size in [6, 1 << 20, (4 << 20) - 17] {
            left.send(Sent::Binary(sealed(b, size).into()))
                .await
                .unwrap();
            assert_eq!(frame(&mut right).await, sealed(a, size));
        }
        for round in 1..=100 {
            right
                .send(Sent::Binary(sealed(a, round).into()))
                .await
                .unwrap();
            assert_eq!(frame(&mut left).await, sealed(b, round));
        }
        assert!(hub.is_online(a).await && hub.is_online(b).await);
        right.close(None).await.unwrap();
        for _ in 0..100 {
            if !hub.is_online(b).await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(!hub.is_online(b).await);
        assert!(hub.is_online(a).await);
    }

    #[tokio::test]
    async fn a_socket_that_goes_silent_is_dropped() {
        let hub = Arc::new(Hub::default());
        let addr = sockets(hub.clone()).await;
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        // Never read, so the relay's pings go unanswered.
        let _silent = dial(&hub, addr, a, b).await;
        tokio::time::pause();
        tokio::time::sleep(SILENCE + HEARTBEAT * 2).await;
        assert!(!hub.is_online(a).await);
    }

    #[test]
    fn a_device_past_its_burst_is_read_at_the_sustained_rate() {
        let start = Instant::now();
        let mut budget = SendBudget {
            available: SEND_BURST,
            at: start,
        };
        assert_eq!(budget.charge(SEND_BURST as usize, start), None);
        let frame = SEND_RATE as usize * 2;
        assert_eq!(budget.charge(frame, start), Some(Duration::from_secs(2)));
        // Reading resumes when the pause ends, with nothing saved up.
        let resumed = start + Duration::from_secs(2);
        assert_eq!(budget.charge(frame, resumed), Some(Duration::from_secs(2)));
        let rested = resumed + Duration::from_secs(2 + 3600);
        assert_eq!(budget.charge(SEND_BURST as usize, rested), None);
    }

    async fn status(app: &Router, peer: &str, forwarded: Option<&str>) -> StatusCode {
        let mut request = Request::builder().uri("/v2/devices");
        if let Some(forwarded) = forwarded {
            request = request.header("X-Forwarded-For", forwarded);
        }
        let mut request = request.body(Body::empty()).unwrap();
        let peer: SocketAddr = format!("{peer}:4000").parse().unwrap();
        request.extensions_mut().insert(ConnectInfo(peer));
        app.clone().oneshot(request).await.unwrap().status()
    }

    /// Unauthenticated requests are refused before any database query, so no
    /// database is needed to count them.
    fn app(proxies: &str) -> Router {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgresql://unused@127.0.0.1:1/unused")
            .unwrap();
        router(AppState::new(
            pool,
            crate::limit::Proxies::parse(proxies).unwrap(),
        ))
    }

    #[tokio::test]
    async fn one_address_cannot_use_up_another_address_requests() {
        let app = app("");
        for _ in 0..600 {
            assert_eq!(
                status(&app, "203.0.113.9", None).await,
                StatusCode::UNAUTHORIZED
            );
        }
        assert_eq!(
            status(&app, "203.0.113.9", None).await,
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            status(&app, "203.0.113.10", None).await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn clients_behind_a_trusted_proxy_are_counted_separately() {
        let app = app("172.16.0.0/12");
        for _ in 0..600 {
            status(&app, "172.18.0.3", Some("203.0.113.9")).await;
        }
        assert_eq!(
            status(&app, "172.18.0.3", Some("203.0.113.9")).await,
            StatusCode::TOO_MANY_REQUESTS
        );
        // Forging the header only changes the part the proxy didn't write.
        assert_eq!(
            status(&app, "172.18.0.3", Some("198.51.100.1, 203.0.113.9")).await,
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            status(&app, "172.18.0.3", Some("203.0.113.10")).await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn an_unlisted_proxy_is_not_rate_limited_as_one_client() {
        let app = app("");
        for _ in 0..601 {
            assert_eq!(
                status(&app, "10.0.0.5", Some("203.0.113.9")).await,
                StatusCode::UNAUTHORIZED
            );
        }
    }
}
