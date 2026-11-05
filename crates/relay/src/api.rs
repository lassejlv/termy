use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use argon2::Argon2;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, FromRequestParts, Path, State};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::Response;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use chrono::{DateTime, SecondsFormat, Utc};
use futures_util::{SinkExt, StreamExt};
use rand::RngCore;
use serde::{Deserialize, Serialize, Serializer};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::AppState;
use crate::db::{Device, User};
use crate::error::{ApiError, ApiResult};
use crate::hub::Hub;

/// Largest relay frame: a compressed screen is a few KiB, a big paste more.
const MAX_FRAME: usize = 4 << 20;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/v1/auth/signup", post(sign_up))
        .route("/v1/auth/login", post(log_in))
        .route("/v1/auth/logout", post(log_out))
        .route("/v1/me", get(me).delete(delete_account))
        .route("/v1/devices", get(devices))
        .route("/v1/devices/{id}", delete(remove_device))
        .route("/v1/relay", get(relay))
        .layer(tower_http::limit::RequestBodyLimitLayer::new(64 * 1024))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}

// MARK: Wire types (camelCase, dates as RFC 3339 seconds, matching TermyRemoteKit)

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceRegistration {
    name: String,
    kind: String,
    public_key: String,
}

#[derive(Deserialize)]
struct Credentials {
    email: String,
    password: String,
    device: DeviceRegistration,
}

#[derive(Serialize)]
struct UserDto {
    id: Uuid,
    email: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceDto {
    id: Uuid,
    name: String,
    kind: String,
    public_key: String,
    online: bool,
    #[serde(serialize_with = "rfc3339")]
    created_at: DateTime<Utc>,
    #[serde(
        serialize_with = "rfc3339_opt",
        skip_serializing_if = "Option::is_none"
    )]
    last_seen_at: Option<DateTime<Utc>>,
}

fn rfc3339<S: Serializer>(date: &DateTime<Utc>, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&date.to_rfc3339_opts(SecondsFormat::Secs, true))
}

fn rfc3339_opt<S: Serializer>(date: &Option<DateTime<Utc>>, s: S) -> Result<S::Ok, S::Error> {
    match date {
        Some(date) => rfc3339(date, s),
        None => s.serialize_none(),
    }
}

#[derive(Serialize)]
struct AuthResponse {
    token: String,
    user: UserDto,
    device: DeviceDto,
}

#[derive(Serialize)]
struct MeResponse {
    user: UserDto,
    device: DeviceDto,
}

#[derive(Serialize)]
struct Empty {}

// MARK: Sign-in throttling

/// Ten sign-in attempts per address and email every five minutes.
#[derive(Default)]
pub struct AttemptLimiter {
    attempts: Mutex<HashMap<String, Vec<Instant>>>,
}

impl AttemptLimiter {
    const LIMIT: usize = 10;
    const WINDOW: Duration = Duration::from_secs(300);

    async fn allow(&self, key: String) -> bool {
        let mut attempts = self.attempts.lock().await;
        if attempts.len() > 10_000 {
            attempts.retain(|_, v| v.iter().any(|t| t.elapsed() < Self::WINDOW));
        }
        let recent = attempts.entry(key).or_default();
        recent.retain(|t| t.elapsed() < Self::WINDOW);
        if recent.len() >= Self::LIMIT {
            return false;
        }
        recent.push(Instant::now());
        true
    }
}

// MARK: Accounts

struct ValidDevice {
    name: String,
    kind: String,
    key: Vec<u8>,
}

fn validate_device(device: DeviceRegistration) -> ApiResult<ValidDevice> {
    if device.kind != "host" && device.kind != "client" {
        return Err(ApiError::bad_request("Unknown device kind."));
    }
    let key = STANDARD
        .decode(device.public_key.as_bytes())
        .ok()
        .filter(|k| k.len() == 32)
        .ok_or_else(|| ApiError::bad_request("The device key must be 32 bytes."))?;
    let name: String = device.name.trim().chars().take(80).collect();
    Ok(ValidDevice {
        name: if name.is_empty() {
            "Device".into()
        } else {
            name
        },
        kind: device.kind,
        key,
    })
}

async fn hash_password(password: String) -> ApiResult<String> {
    tokio::task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
    })
    .await
    .ok()
    .and_then(Result::ok)
    .ok_or_else(|| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Couldn't secure the password. Try again.",
        )
    })
}

async fn verify_password(password: String, hash: String) -> bool {
    tokio::task::spawn_blocking(move || {
        PasswordHash::new(&hash).is_ok_and(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
    })
    .await
    .unwrap_or(false)
}

fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

async fn device_dto(state: &AppState, device: Device) -> DeviceDto {
    DeviceDto {
        id: device.id,
        online: state.hub.is_online(device.id).await,
        name: device.name,
        kind: device.kind,
        public_key: STANDARD.encode(&device.public_key),
        created_at: device.created_at,
        last_seen_at: device.last_seen_at,
    }
}

async fn start_session(
    state: &AppState,
    user: User,
    device: ValidDevice,
) -> ApiResult<Json<AuthResponse>> {
    let row = state
        .db
        .upsert_device(user.id, &device.name, &device.kind, &device.key)
        .await?;
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let token = URL_SAFE_NO_PAD.encode(bytes);
    state
        .db
        .create_session(&token_hash(&token), user.id, row.id)
        .await?;
    Ok(Json(AuthResponse {
        token,
        user: UserDto {
            id: user.id,
            email: user.email,
        },
        device: device_dto(state, row).await,
    }))
}

async fn sign_up(
    State(state): State<AppState>,
    Json(body): Json<Credentials>,
) -> ApiResult<Json<AuthResponse>> {
    let email = body.email.trim().to_string();
    let valid_email = email.len() <= 254
        && email
            .split_once('@')
            .is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.'));
    if !valid_email {
        return Err(ApiError::bad_request("Enter a valid email address."));
    }
    if body.password.chars().count() < 8 {
        return Err(ApiError::bad_request(
            "Use a password of at least 8 characters.",
        ));
    }
    if body.password.len() > 256 {
        return Err(ApiError::bad_request(
            "Use a password of at most 256 bytes.",
        ));
    }
    let device = validate_device(body.device)?;
    let hash = hash_password(body.password).await?;
    let user = state.db.create_user(&email, &hash).await?.ok_or_else(|| {
        ApiError::new(
            StatusCode::CONFLICT,
            "An account with this email already exists.",
        )
    })?;
    start_session(&state, user, device).await
}

async fn log_in(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(body): Json<Credentials>,
) -> ApiResult<Json<AuthResponse>> {
    let email = body.email.trim().to_string();
    if !state
        .limiter
        .allow(format!("{}|{}", addr.ip(), email.to_lowercase()))
        .await
    {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many attempts. Try again in a few minutes.",
        ));
    }
    let device = validate_device(body.device)?;
    let wrong = || ApiError::unauthorized("Wrong email or password.");
    let user = state.db.user_by_email(&email).await?.ok_or_else(wrong)?;
    if !verify_password(body.password, user.password_hash.clone()).await {
        return Err(wrong());
    }
    start_session(&state, user, device).await
}

/// The signed-in device, from `Authorization: Bearer <token>`.
pub struct Authed {
    pub user_id: Uuid,
    pub device_id: Uuid,
}

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
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or_else(|| ApiError::unauthorized("Sign in first."))?;
        let (user_id, device_id) = state
            .db
            .session(&token_hash(token))
            .await?
            .ok_or_else(|| ApiError::unauthorized("Your session ended. Sign in again."))?;
        Ok(Self { user_id, device_id })
    }
}

async fn log_out(State(state): State<AppState>, auth: Authed) -> ApiResult<Json<Empty>> {
    state.db.delete_device(auth.device_id, auth.user_id).await?;
    state.hub.kick(auth.device_id).await;
    Ok(Json(Empty {}))
}

async fn me(State(state): State<AppState>, auth: Authed) -> ApiResult<Json<MeResponse>> {
    let user = state
        .db
        .user(auth.user_id)
        .await?
        .ok_or_else(|| ApiError::unauthorized("Sign in again."))?;
    let device = state
        .db
        .devices(auth.user_id)
        .await?
        .into_iter()
        .find(|d| d.id == auth.device_id)
        .ok_or_else(|| ApiError::unauthorized("Sign in again."))?;
    Ok(Json(MeResponse {
        user: UserDto {
            id: user.id,
            email: user.email,
        },
        device: device_dto(&state, device).await,
    }))
}

async fn delete_account(State(state): State<AppState>, auth: Authed) -> ApiResult<Json<Empty>> {
    let devices = state.db.devices(auth.user_id).await?;
    state.db.delete_user(auth.user_id).await?;
    for device in devices {
        state.hub.kick(device.id).await;
    }
    Ok(Json(Empty {}))
}

async fn devices(State(state): State<AppState>, auth: Authed) -> ApiResult<Json<Vec<DeviceDto>>> {
    let mut out = vec![];
    for device in state.db.devices(auth.user_id).await? {
        out.push(device_dto(&state, device).await);
    }
    Ok(Json(out))
}

async fn remove_device(
    State(state): State<AppState>,
    auth: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Empty>> {
    if !state.db.delete_device(id, auth.user_id).await? {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "No such device."));
    }
    state.hub.kick(id).await;
    Ok(Json(Empty {}))
}

// MARK: Relay socket

async fn relay(State(state): State<AppState>, auth: Authed, upgrade: WebSocketUpgrade) -> Response {
    upgrade
        .max_message_size(MAX_FRAME)
        .max_frame_size(MAX_FRAME)
        .on_upgrade(move |socket| serve(state, auth, socket))
}

async fn serve(state: AppState, auth: Authed, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = Hub::queue();
    let serial = state
        .hub
        .connect(auth.device_id, auth.user_id, tx.clone())
        .await;
    let _ = state.db.touch_device(auth.device_id).await;

    let writer = tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            let closing = matches!(message, Message::Close(_));
            if sink.send(message).await.is_err() || closing {
                break;
            }
        }
    });
    // Keep idle sockets open through proxies that drop quiet connections.
    let pinger = {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(25));
            loop {
                interval.tick().await;
                if tx.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
        })
    };

    while let Some(Ok(message)) = stream.next().await {
        match message {
            Message::Binary(bytes) => state.hub.route(auth.device_id, bytes.to_vec()).await,
            Message::Close(_) => break,
            _ => {}
        }
        if writer.is_finished() {
            break;
        }
    }
    pinger.abort();
    writer.abort();
    state.hub.disconnect(auth.device_id, serial).await;
    let _ = state.db.touch_device(auth.device_id).await;
}
