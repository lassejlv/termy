//! "Login with Railway" provider connection (OAuth 2.0 + OIDC, PKCE S256).
//!
//! This is a custom provider-connection flow, separate from better-auth's
//! login OAuth: it links a Railway account to an existing Termy user and
//! stores the tokens encrypted for server-side provider calls. Endpoints are
//! fixed from Railway's OIDC discovery document
//! (`https://backboard.railway.com/oauth/.well-known/openid-configuration`).

use anyhow::Context as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use chacha20poly1305::aead::rand_core::RngCore as _;
use chacha20poly1305::aead::OsRng;
use sha2::{Digest as _, Sha256};

use crate::state::CloudState;

pub(crate) const AUTHORIZE_ENDPOINT: &str = "https://backboard.railway.com/oauth/auth";
pub(crate) const TOKEN_ENDPOINT: &str = "https://backboard.railway.com/oauth/token";

/// Workspace write access is required to create projects and sandboxes;
/// `ssh_keys` lets the CLI register the user's key for sandbox SSH.
pub(crate) const SCOPES: &str = "openid profile email offline_access workspace:admin ssh_keys";

pub(crate) const PROVIDER: &str = "railway";

#[derive(Clone)]
pub(crate) struct RailwayOAuthConfig {
    pub(crate) client_id: String,
    pub(crate) client_secret: String,
}

pub(crate) struct AuthorizationRequest {
    pub(crate) state: String,
    pub(crate) pkce_verifier: String,
    pub(crate) authorize_url: String,
}

#[derive(serde::Deserialize)]
pub(crate) struct TokenResponse {
    pub(crate) access_token: String,
    pub(crate) refresh_token: Option<String>,
    pub(crate) expires_in: Option<i64>,
    pub(crate) id_token: Option<String>,
    pub(crate) scope: Option<String>,
}

#[derive(Default)]
pub(crate) struct IdTokenClaims {
    pub(crate) subject: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) email: Option<String>,
}

pub(crate) fn callback_url(base_url: &str) -> String {
    format!(
        "{}/api/providers/railway/callback",
        base_url.trim_end_matches('/')
    )
}

/// Builds the authorize redirect plus the state/verifier pair to persist.
pub(crate) fn authorization_request(
    config: &RailwayOAuthConfig,
    base_url: &str,
) -> AuthorizationRequest {
    let state = random_token();
    let pkce_verifier = random_token();
    let challenge = pkce_challenge(&pkce_verifier);
    let authorize_url = format!(
        "{AUTHORIZE_ENDPOINT}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&code_challenge={}&code_challenge_method=S256",
        urlencode(&config.client_id),
        urlencode(&callback_url(base_url)),
        urlencode(SCOPES),
        urlencode(&state),
        urlencode(&challenge),
    );
    AuthorizationRequest {
        state,
        pkce_verifier,
        authorize_url,
    }
}

pub(crate) async fn exchange_code(
    http: &reqwest::Client,
    config: &RailwayOAuthConfig,
    base_url: &str,
    code: &str,
    pkce_verifier: &str,
) -> anyhow::Result<TokenResponse> {
    request_token(
        http,
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", &callback_url(base_url)),
            ("client_id", &config.client_id),
            ("client_secret", &config.client_secret),
            ("code_verifier", pkce_verifier),
        ],
    )
    .await
    .context("Railway authorization-code exchange failed")
}

pub(crate) async fn refresh_tokens(
    http: &reqwest::Client,
    config: &RailwayOAuthConfig,
    refresh_token: &str,
) -> anyhow::Result<TokenResponse> {
    request_token(
        http,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", &config.client_id),
            ("client_secret", &config.client_secret),
        ],
    )
    .await
    .context("Railway token refresh failed")
}

async fn request_token(
    http: &reqwest::Client,
    form: &[(&str, &str)],
) -> anyhow::Result<TokenResponse> {
    let response = http.post(TOKEN_ENDPOINT).form(form).send().await?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("token endpoint returned HTTP {status}: {body}");
    }
    Ok(response.json().await?)
}

/// Extracts identity claims from the `id_token` payload without verifying the
/// signature — acceptable here because the token arrives directly from
/// Railway's token endpoint over TLS, never from the client.
pub(crate) fn id_token_claims(id_token: &str) -> IdTokenClaims {
    let Some(payload) = id_token.split('.').nth(1) else {
        return IdTokenClaims::default();
    };
    let Ok(bytes) = URL_SAFE_NO_PAD.decode(payload) else {
        return IdTokenClaims::default();
    };
    let Ok(claims) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return IdTokenClaims::default();
    };
    let text = |key: &str| claims[key].as_str().map(String::from);
    IdTokenClaims {
        subject: text("sub"),
        name: text("name").or_else(|| text("preferred_username")),
        email: text("email"),
    }
}

/// Persists a completed token exchange for `user_id`, encrypting both tokens.
pub(crate) async fn store_connection(
    cloud: &CloudState,
    user_id: &str,
    tokens: &TokenResponse,
) -> anyhow::Result<()> {
    let claims = tokens
        .id_token
        .as_deref()
        .map(id_token_claims)
        .unwrap_or_default();
    let access_enc = cloud.crypto.encrypt(&tokens.access_token);
    let refresh_enc = tokens
        .refresh_token
        .as_deref()
        .map(|token| cloud.crypto.encrypt(token));
    let expires_in = tokens.expires_in.unwrap_or(3600).max(0);
    neon_serverless_sqlx::sqlx::query(
        "INSERT INTO provider_connections
            (user_id, provider, access_token_enc, refresh_token_enc,
             access_token_expires_at, provider_account_id, provider_account_name, scopes)
         VALUES ($1, $2, $3, $4, NOW() + make_interval(secs => $5::double precision), $6, $7, $8)
         ON CONFLICT (user_id, provider) DO UPDATE SET
            access_token_enc = EXCLUDED.access_token_enc,
            refresh_token_enc = COALESCE(EXCLUDED.refresh_token_enc, provider_connections.refresh_token_enc),
            access_token_expires_at = EXCLUDED.access_token_expires_at,
            provider_account_id = COALESCE(EXCLUDED.provider_account_id, provider_connections.provider_account_id),
            provider_account_name = COALESCE(EXCLUDED.provider_account_name, provider_connections.provider_account_name),
            scopes = EXCLUDED.scopes,
            updated_at = NOW()",
    )
    .bind(user_id)
    .bind(PROVIDER)
    .bind(&access_enc)
    .bind(&refresh_enc)
    .bind(expires_in)
    .bind(&claims.subject)
    .bind(claims.name.as_deref().or(claims.email.as_deref()))
    .bind(tokens.scope.as_deref().unwrap_or(SCOPES))
    .execute(cloud.db.pg())
    .await
    .context("failed to store Railway connection")?;
    Ok(())
}

pub(crate) enum AccessTokenError {
    NotConnected,
    ReauthRequired(String),
    Internal(anyhow::Error),
}

/// Returns a decrypted, currently valid Railway access token for `user_id`,
/// refreshing (and re-encrypting) it when within two minutes of expiry.
pub(crate) async fn access_token_for(
    cloud: &CloudState,
    user_id: &str,
) -> Result<String, AccessTokenError> {
    let row: Option<(String, Option<String>, Option<bool>)> = neon_serverless_sqlx::sqlx::query_as(
        "SELECT access_token_enc, refresh_token_enc,
                access_token_expires_at < NOW() + interval '2 minutes'
         FROM provider_connections WHERE user_id = $1 AND provider = $2",
    )
    .bind(user_id)
    .bind(PROVIDER)
    .fetch_optional(cloud.db.pg())
    .await
    .map_err(|error| AccessTokenError::Internal(error.into()))?;
    let Some((access_enc, refresh_enc, near_expiry)) = row else {
        return Err(AccessTokenError::NotConnected);
    };

    if near_expiry != Some(true) {
        return cloud
            .crypto
            .decrypt(&access_enc)
            .map_err(AccessTokenError::Internal);
    }

    let Some(config) = &cloud.railway else {
        return Err(AccessTokenError::Internal(anyhow::anyhow!(
            "Railway OAuth is not configured on this server"
        )));
    };
    let Some(refresh_enc) = refresh_enc else {
        return Err(AccessTokenError::ReauthRequired(
            "Railway access token expired and no refresh token is stored".to_string(),
        ));
    };
    let refresh_token = cloud
        .crypto
        .decrypt(&refresh_enc)
        .map_err(AccessTokenError::Internal)?;
    let tokens = match refresh_tokens(&cloud.http, config, &refresh_token).await {
        Ok(tokens) => tokens,
        Err(error) => return Err(AccessTokenError::ReauthRequired(error.to_string())),
    };
    store_connection(cloud, user_id, &tokens)
        .await
        .map_err(AccessTokenError::Internal)?;
    Ok(tokens.access_token)
}

fn random_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn urlencode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::{
        authorization_request, callback_url, id_token_claims, pkce_challenge, random_token,
        urlencode, RailwayOAuthConfig,
    };
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine as _;

    fn config() -> RailwayOAuthConfig {
        RailwayOAuthConfig {
            client_id: "client-id".to_string(),
            client_secret: "client-secret".to_string(),
        }
    }

    #[test]
    fn authorize_url_carries_pkce_state_and_redirect() {
        let request = authorization_request(&config(), "https://app.termy.sh/");
        assert!(request
            .authorize_url
            .starts_with("https://backboard.railway.com/oauth/auth?response_type=code"));
        assert!(request.authorize_url.contains("client_id=client-id"));
        assert!(request
            .authorize_url
            .contains(&format!("state={}", urlencode(&request.state))));
        assert!(request.authorize_url.contains(&format!(
            "code_challenge={}",
            urlencode(&pkce_challenge(&request.pkce_verifier))
        )));
        assert!(request.authorize_url.contains("code_challenge_method=S256"));
        assert!(request.authorize_url.contains(&urlencode(
            "https://app.termy.sh/api/providers/railway/callback"
        )));
    }

    #[test]
    fn callback_url_normalizes_trailing_slash() {
        assert_eq!(
            callback_url("http://127.0.0.1:8080/"),
            "http://127.0.0.1:8080/api/providers/railway/callback"
        );
    }

    #[test]
    fn pkce_challenge_matches_rfc7636_appendix_b() {
        // Test vector from RFC 7636 appendix B.
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn random_tokens_are_unique_and_url_safe() {
        let one = random_token();
        let two = random_token();
        assert_ne!(one, two);
        assert!(URL_SAFE_NO_PAD.decode(&one).is_ok());
    }

    #[test]
    fn id_token_claims_are_extracted_without_verification() {
        let payload = URL_SAFE_NO_PAD.encode(
            serde_json::json!({"sub": "user-1", "name": "Lasse", "email": "l@example.com"})
                .to_string(),
        );
        let claims = id_token_claims(&format!("header.{payload}.signature"));
        assert_eq!(claims.subject.as_deref(), Some("user-1"));
        assert_eq!(claims.name.as_deref(), Some("Lasse"));
        assert_eq!(claims.email.as_deref(), Some("l@example.com"));
    }

    #[test]
    fn malformed_id_tokens_yield_empty_claims() {
        assert!(id_token_claims("garbage").subject.is_none());
        assert!(id_token_claims("a.!!!.c").subject.is_none());
    }
}
