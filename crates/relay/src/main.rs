//! The Termy relay: accountless device pairing and a WebSocket hub that
//! routes sealed packets only between explicitly paired phones and Macs.
//!
//! Terminal sessions are end-to-end encrypted between the phone and the
//! computer (`remote/Sources/TermyRemoteKit`). The relay only sees who talks
//! to whom and how much; it holds no key that opens a packet.

mod api;
mod db;
mod error;
mod hub;

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::EnvFilter;

#[derive(Clone)]
pub struct AppState {
    pub db: db::Db,
    pub hub: Arc<hub::Hub>,
    pub gate: Arc<tokio::sync::Mutex<()>>,
    pub limiter: Arc<api::AttemptLimiter>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn")),
        )
        .init();

    let database_url = std::env::var("DATABASE_URL")
        .context("set DATABASE_URL to a Postgres connection string")?;
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await
        .context("connecting to Postgres")?;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("running migrations")?;

    let state = AppState {
        db: db::Db::new(pool),
        hub: Arc::new(hub::Hub::default()),
        gate: Arc::new(tokio::sync::Mutex::new(())),
        limiter: Arc::new(api::AttemptLimiter::default()),
    };
    let host = std::env::var("HOST").unwrap_or_else(|_| "0.0.0.0".into());
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let addr: SocketAddr = format!("{host}:{port}").parse().context("HOST and PORT")?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("relay listening on {addr}");
    axum::serve(
        listener,
        api::router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
    Ok(())
}

#[cfg(all(test, feature = "integration-tests"))]
mod tests;
