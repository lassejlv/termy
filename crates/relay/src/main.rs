//! The Termy relay: accountless device pairing and a WebSocket hub that
//! routes sealed packets only between explicitly paired phones and Macs.
//!
//! Terminal sessions are end-to-end encrypted between the phone and the
//! computer (`remote/Sources/TermyRemoteKit`). The relay only sees who talks
//! to whom and how much; it holds no key that opens a packet.

mod api;
mod db;
mod error;
mod gate;
mod hub;
mod limit;

use std::future::Future;
use std::net::SocketAddr;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axum::{Extension, Router, extract::ConnectInfo};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::{conn::auto::Builder, graceful::GracefulShutdown};
use hyper_util::service::TowerToHyperService;
use sqlx::postgres::{PgPool, PgPoolOptions};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

/// How long a client may take to send its request headers.
const HEADER_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone)]
pub struct AppState {
    pub db: db::Db,
    pub hub: Arc<hub::Hub>,
    pub gates: Arc<gate::Gates>,
    pub limits: Arc<limit::Limits>,
}

impl AppState {
    pub fn new(pool: PgPool, proxies: limit::Proxies) -> Self {
        Self {
            db: db::Db::new(pool),
            hub: Arc::new(hub::Hub::default()),
            gates: Arc::new(gate::Gates::default()),
            limits: Arc::new(limit::Limits::new(proxies)),
        }
    }
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
    let proxies = limit::Proxies::parse(&std::env::var("TRUSTED_PROXIES").unwrap_or_default())?;
    let pool = PgPoolOptions::new()
        .max_connections(10)
        // Fail a request rather than hold its place in line when the pool is busy.
        .acquire_timeout(Duration::from_secs(5))
        .connect(&database_url)
        .await
        .context("connecting to Postgres")?;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("running migrations")?;

    let state = AppState::new(pool, proxies);
    tokio::spawn(prune(state.clone()));
    let host = std::env::var("HOST").unwrap_or_else(|_| "0.0.0.0".into());
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let addr: SocketAddr = format!("{host}:{port}").parse().context("HOST and PORT")?;
    let listener = TcpListener::bind(addr).await?;
    tracing::info!("relay listening on {addr}");
    serve(listener, api::router(state), HEADER_TIMEOUT, shutdown()).await;
    Ok(())
}

/// Hourly, forgets registrations nobody uses, so the open registration
/// endpoint can't grow the database without end.
async fn prune(state: AppState) {
    let mut hourly = tokio::time::interval(Duration::from_secs(3600));
    loop {
        hourly.tick().await;
        match state.db.prune(&state.hub.online().await).await {
            Ok(removed) => {
                for device in &removed {
                    state.hub.kick(*device).await;
                }
                if !removed.is_empty() {
                    tracing::info!("removed {} unused registrations", removed.len());
                }
            }
            Err(error) => tracing::warn!("pruning registrations: {error}"),
        }
    }
}

async fn shutdown() {
    let interrupt = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut terminate) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = interrupt => {}
                _ = terminate.recv() => {}
            }
            return;
        }
    }
    let _ = interrupt.await;
}

/// Serves `app` until `shutdown` resolves. This is `axum::serve` with a
/// timer: without one hyper never times out a client that opens a connection
/// and doesn't finish its request headers.
async fn serve(
    listener: TcpListener,
    app: Router,
    header_timeout: Duration,
    shutdown: impl Future<Output = ()>,
) {
    let mut builder = Builder::new(TokioExecutor::new());
    builder
        .http1()
        .timer(TokioTimer::new())
        .header_read_timeout(header_timeout);
    let graceful = GracefulShutdown::new();
    let mut shutdown = pin!(shutdown);
    loop {
        let (stream, addr) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(accepted) => accepted,
                Err(error) => {
                    // Out of file descriptors, most likely: let some close.
                    tracing::warn!("accepting a connection: {error}");
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    continue;
                }
            },
            () = &mut shutdown => break,
        };
        let service = TowerToHyperService::new(app.clone().layer(Extension(ConnectInfo(addr))));
        let (builder, watcher) = (builder.clone(), graceful.watcher());
        tokio::spawn(async move {
            let connection = builder.serve_connection_with_upgrades(TokioIo::new(stream), service);
            let _ = watcher.watch(connection).await;
        });
    }
    // Let requests in flight finish; open relay sockets end with the process.
    let _ = tokio::time::timeout(Duration::from_secs(10), graceful.shutdown()).await;
}

#[cfg(test)]
mod serve_tests {
    use super::*;
    use axum::routing::get;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    async fn start() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().route(
            "/peer",
            get(|ConnectInfo(peer): ConnectInfo<SocketAddr>| async move { peer.ip().to_string() }),
        );
        tokio::spawn(serve(
            listener,
            app,
            Duration::from_millis(300),
            std::future::pending(),
        ));
        addr
    }

    async fn exchange(addr: SocketAddr, request: &str) -> String {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = String::new();
        tokio::time::timeout(Duration::from_secs(5), stream.read_to_string(&mut response))
            .await
            .expect("the relay kept a stalled connection open")
            .unwrap();
        response
    }

    #[tokio::test]
    async fn answers_requests_and_knows_the_peer() {
        let request = "GET /peer HTTP/1.1\r\nHost: relay\r\nConnection: close\r\n\r\n";
        let response = exchange(start().await, request).await;
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.ends_with("127.0.0.1"), "{response}");
    }

    #[tokio::test]
    async fn closes_a_connection_that_never_finishes_its_headers() {
        let response = exchange(start().await, "GET /peer HTTP/1.1\r\nHost: relay\r\n").await;
        assert!(!response.starts_with("HTTP/1.1 200"), "{response}");
    }
}

#[cfg(all(test, feature = "integration-tests"))]
mod tests;
