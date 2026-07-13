//! Shared context for cloud-project routes.
//!
//! better-auth's extractors require the router state to be exactly
//! `Arc<BetterAuth<_>>`, so everything else the `/api` handlers need rides in
//! an axum `Extension` layer instead of router state.

use std::sync::Arc;

use crate::crypto::TokenCipher;
use crate::db::QueryPool;
use crate::providers::railway_oauth::RailwayOAuthConfig;
use crate::providers::SandboxProvider;

pub(crate) struct CloudState {
    pub(crate) db: Arc<QueryPool>,
    pub(crate) crypto: TokenCipher,
    pub(crate) http: reqwest::Client,
    pub(crate) base_url: String,
    /// `None` hides the Railway connect flow (same pattern as GitHub login).
    pub(crate) railway: Option<RailwayOAuthConfig>,
    pub(crate) provider: Arc<dyn SandboxProvider>,
}
