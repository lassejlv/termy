//! Termy cloud API server.
//!
//! Serves the account/auth backend for Termy cloud agents (TRM-1/TRM-2).
//! Auth endpoints are provided by `better-auth` mounted under `/auth`;
//! application routes live under `/api`.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use axum::routing::get;
use axum::Router;
use better_auth::adapters::SqlxAdapter;
use better_auth::handlers::AxumIntegration as _;
use better_auth::plugins::oauth::{OAuthProvider, OAuthUserInfo};
use better_auth::plugins::{
    AccountManagementPlugin, DeviceAuthorizationPlugin, EmailPasswordPlugin, OAuthPlugin,
    PasswordManagementPlugin, SessionManagementPlugin,
};
use better_auth::{AuthConfig, BetterAuth};
use sqlx::postgres::PgPoolOptions;
use tower_http::services::{ServeDir, ServeFile};

mod crypto;
mod db;
mod providers;
mod routes;
mod sessions;
mod state;

use crypto::TokenCipher;
use db::QueryPool;
use providers::railway::RailwayProvider;
use providers::railway_oauth::RailwayOAuthConfig;
use state::CloudState;

pub(crate) type Auth = BetterAuth<SqlxAdapter>;

struct ApiConfig {
    database_url: String,
    secret: String,
    encryption_key: String,
    base_url: String,
    listen_addr: SocketAddr,
    web_dir: PathBuf,
    github_oauth: Option<GithubOAuthConfig>,
    railway_oauth: Option<RailwayOAuthConfig>,
}

struct GithubOAuthConfig {
    client_id: String,
    client_secret: String,
}

impl ApiConfig {
    fn from_env() -> anyhow::Result<Self> {
        let database_url = std::env::var("DATABASE_URL").context("DATABASE_URL must be set")?;
        let secret = std::env::var("TERMY_API_SECRET")
            .context("TERMY_API_SECRET must be set (32+ characters)")?;
        anyhow::ensure!(
            secret.len() >= 32,
            "TERMY_API_SECRET must be at least 32 characters"
        );
        let encryption_key = std::env::var("TERMY_API_ENCRYPTION_KEY")
            .context("TERMY_API_ENCRYPTION_KEY must be set (base64 of 32 random bytes)")?;
        let base_url = std::env::var("TERMY_API_BASE_URL")
            .unwrap_or_else(|_| "https://app.termy.sh".to_string());
        let port: u16 = std::env::var("PORT")
            .unwrap_or_else(|_| "8080".to_string())
            .parse()
            .context("PORT must be a valid u16")?;
        let github_oauth = github_oauth_from_values(
            std::env::var("TERMY_GITHUB_CLIENT_ID").ok(),
            std::env::var("TERMY_GITHUB_CLIENT_SECRET").ok(),
        )?;
        let railway_oauth = railway_oauth_from_values(
            std::env::var("TERMY_RAILWAY_CLIENT_ID").ok(),
            std::env::var("TERMY_RAILWAY_CLIENT_SECRET").ok(),
        )?;
        Ok(Self {
            database_url,
            secret,
            encryption_key,
            base_url,
            listen_addr: SocketAddr::from(([0, 0, 0, 0], port)),
            web_dir: std::env::var_os("TERMY_WEB_DIR")
                .map_or_else(|| PathBuf::from("crates/api/web/dist"), PathBuf::from),
            github_oauth,
            railway_oauth,
        })
    }
}

fn github_oauth_from_values(
    client_id: Option<String>,
    client_secret: Option<String>,
) -> anyhow::Result<Option<GithubOAuthConfig>> {
    let client_id = client_id.filter(|value| !value.trim().is_empty());
    let client_secret = client_secret.filter(|value| !value.trim().is_empty());
    match (client_id, client_secret) {
        (None, None) => Ok(None),
        (Some(client_id), Some(client_secret)) => Ok(Some(GithubOAuthConfig {
            client_id,
            client_secret,
        })),
        _ => anyhow::bail!(
            "TERMY_GITHUB_CLIENT_ID and TERMY_GITHUB_CLIENT_SECRET must be set together"
        ),
    }
}

fn railway_oauth_from_values(
    client_id: Option<String>,
    client_secret: Option<String>,
) -> anyhow::Result<Option<RailwayOAuthConfig>> {
    let client_id = client_id.filter(|value| !value.trim().is_empty());
    let client_secret = client_secret.filter(|value| !value.trim().is_empty());
    match (client_id, client_secret) {
        (None, None) => Ok(None),
        (Some(client_id), Some(client_secret)) => Ok(Some(RailwayOAuthConfig {
            client_id,
            client_secret,
        })),
        _ => anyhow::bail!(
            "TERMY_RAILWAY_CLIENT_ID and TERMY_RAILWAY_CLIENT_SECRET must be set together"
        ),
    }
}

async fn build_auth(config: &ApiConfig) -> anyhow::Result<Arc<Auth>> {
    let adapter = SqlxAdapter::new(&config.database_url)
        .await
        .context("failed to connect better-auth to postgres")?;
    let mut auth_config = AuthConfig::new(config.secret.clone())
        .app_name("Termy Cloud")
        .base_url(config.base_url.clone())
        .base_path("/auth");
    // Secure cookies are mandatory in production but browsers reject them on
    // plain HTTP, so localhost development follows the configured base URL.
    auth_config.session.cookie_secure = config.base_url.starts_with("https://");

    let verification_uri = format!("{}/device", config.base_url.trim_end_matches('/'));
    let mut builder = BetterAuth::<SqlxAdapter>::new(auth_config)
        .database(adapter)
        .plugin(EmailPasswordPlugin::new().enable_signup(true))
        .plugin(
            DeviceAuthorizationPlugin::new()
                .enabled(true)
                .verification_uri(verification_uri),
        )
        .plugin(SessionManagementPlugin::new())
        .plugin(PasswordManagementPlugin::new())
        .plugin(AccountManagementPlugin::new());
    if let Some(github) = &config.github_oauth {
        builder = builder
            .plugin(OAuthPlugin::new().add_provider("github", github_oauth_provider(github)));
    }
    let auth = builder
        .build()
        .await
        .context("failed to build better-auth")?;
    Ok(Arc::new(auth))
}

fn github_oauth_provider(config: &GithubOAuthConfig) -> OAuthProvider {
    let mut provider = OAuthProvider::github(&config.client_id, &config.client_secret);
    provider.map_user_info = map_github_user;
    provider
}

fn map_github_user(value: serde_json::Value) -> Result<OAuthUserInfo, String> {
    let id = value["id"]
        .as_i64()
        .map(|id| id.to_string())
        .or_else(|| value["id"].as_str().map(String::from))
        .ok_or("missing id")?;
    let login = value["login"].as_str().ok_or("missing login")?;
    // GitHub returns `email: null` when the user hides their public email.
    // The OAuth plugin only performs one user-info request, so use GitHub's
    // stable no-reply address format as the account identifier in that case.
    let email = value["email"].as_str().map_or_else(
        || format!("{id}+{login}@users.noreply.github.com"),
        String::from,
    );
    Ok(OAuthUserInfo {
        id,
        email,
        name: value["name"]
            .as_str()
            .or_else(|| value["login"].as_str())
            .map(String::from),
        image: value["avatar_url"].as_str().map(String::from),
        email_verified: true,
    })
}

async fn run_migrations(database_url: &str) -> anyhow::Result<()> {
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(database_url)
        .await
        .context("failed to connect to postgres for migrations")?;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("failed to run database migrations")?;
    pool.close().await;
    Ok(())
}

fn build_router(
    auth: Arc<Auth>,
    db: Arc<QueryPool>,
    cloud: Arc<CloudState>,
    web_dir: &Path,
    github_enabled: bool,
) -> Router {
    // better-auth's extractors require the router state to be exactly
    // `Arc<BetterAuth<_>>`, so auth-backed and db-backed routes are built
    // as separate routers and merged. Everything else the `/api` handlers
    // need (query pool, token cipher, provider clients) rides in a
    // `CloudState` extension layer.
    let api_routes = Router::new()
        .route("/me", get(routes::me))
        .route(
            "/auth-config",
            get(move || async move { routes::auth_config(github_enabled) }),
        )
        .route(
            "/providers/railway",
            get(routes::providers::railway_status).delete(routes::providers::railway_disconnect),
        )
        .route(
            "/providers/railway/connect",
            get(routes::providers::railway_connect),
        )
        .route(
            "/providers/railway/callback",
            get(routes::providers::railway_callback),
        )
        .route(
            "/projects",
            get(routes::projects::list_projects).post(routes::projects::create_project),
        )
        .route(
            "/projects/{project_id}",
            get(routes::projects::get_project)
                .patch(routes::projects::update_project)
                .delete(routes::projects::delete_project),
        )
        .route(
            "/projects/{project_id}/sessions",
            get(routes::sessions::list_sessions).post(routes::sessions::start_session),
        )
        .route(
            "/sessions/{session_id}",
            get(routes::sessions::get_session).delete(routes::sessions::stop_session),
        )
        .route(
            "/sessions/{session_id}/connection",
            get(routes::sessions::get_session_connection),
        )
        .route(
            "/sessions/{session_id}/terminal",
            get(routes::terminal::terminal_ws),
        )
        .layer(axum::Extension(cloud))
        .fallback(routes::not_found);
    let auth_routes = Router::new().nest("/api", api_routes).nest(
        "/auth",
        auth.clone().axum_router().fallback(routes::not_found),
    );
    let auth_routes = auth_routes.with_state(auth);
    let db_routes = Router::new()
        .route("/health", get(routes::health))
        .with_state(db);
    auth_routes
        .merge(db_routes)
        .fallback_service(static_file_service(web_dir))
}

fn static_file_service(web_dir: &Path) -> ServeDir<ServeFile> {
    ServeDir::new(web_dir).fallback(ServeFile::new(web_dir.join("index.html")))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // The Neon websocket transport uses rustls without a compiled-in default
    // crypto provider; a process-level provider must be installed before any
    // TLS connection is made.
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("failed to install rustls crypto provider"))?;

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config = ApiConfig::from_env()?;
    anyhow::ensure!(
        config.web_dir.join("index.html").is_file(),
        "Termy web assets are missing from {} (run `bun --cwd crates/api/web run build`)",
        config.web_dir.display()
    );
    run_migrations(&config.database_url).await?;
    let auth = build_auth(&config).await?;
    let db = Arc::new(QueryPool::connect(&config.database_url).await?);
    let http = reqwest::Client::new();
    let cloud = Arc::new(CloudState {
        db: db.clone(),
        crypto: TokenCipher::from_base64_key(&config.encryption_key)?,
        provider: Arc::new(RailwayProvider::new(http.clone())),
        http,
        base_url: config.base_url.clone(),
        railway: config.railway_oauth.clone(),
    });
    let app = build_router(
        auth,
        db,
        cloud,
        &config.web_dir,
        config.github_oauth.is_some(),
    );

    let listener = tokio::net::TcpListener::bind(config.listen_addr)
        .await
        .with_context(|| format!("failed to bind {}", config.listen_addr))?;
    tracing::info!("termy-api listening on {}", config.listen_addr);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install ctrl-c handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}

#[cfg(test)]
mod tests {
    use super::{github_oauth_from_values, map_github_user, static_file_service};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::Router;
    use tower::ServiceExt as _;

    #[tokio::test]
    async fn spa_route_falls_back_to_index_with_ok_status() {
        let directory = tempfile::tempdir().expect("temp web directory");
        std::fs::write(
            directory.path().join("index.html"),
            "<title>Termy Cloud</title>",
        )
        .expect("write test index");
        let app = Router::new().fallback_service(static_file_service(directory.path()));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/device?user_code=ABCD1234")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("SPA response");

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn github_oauth_requires_both_credentials() {
        assert!(github_oauth_from_values(None, None).unwrap().is_none());
        assert!(
            github_oauth_from_values(Some("client".to_string()), Some("secret".to_string()))
                .unwrap()
                .is_some()
        );
        assert!(github_oauth_from_values(Some("client".to_string()), None).is_err());
        assert!(github_oauth_from_values(None, Some("secret".to_string())).is_err());
    }

    #[test]
    fn github_oauth_supports_private_email_profiles() {
        let user = map_github_user(serde_json::json!({
            "id": 42,
            "login": "octocat",
            "name": null,
            "email": null,
            "avatar_url": "https://example.com/avatar.png"
        }))
        .expect("map GitHub user");

        assert_eq!(user.email, "42+octocat@users.noreply.github.com");
        assert_eq!(user.name.as_deref(), Some("octocat"));
    }
}
