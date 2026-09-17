pub mod policy;
pub mod service;
pub mod source;
pub mod transport;

pub const DEFAULT_GITHUB_REPO: &str = "lassejlv/termy";

pub use policy::{PlatformKind, VersionComparison, compare_versions};
pub use service::{
    ReleaseInfo, UpdateCheck, check_for_updates, check_for_updates_for_repo,
    check_for_updates_with_release, check_for_updates_with_source, fetch_latest_release,
    fetch_latest_release_for_repo, fetch_latest_release_with_source,
};
pub use source::{ReleaseAsset, ReleasePayload, ReleaseSource};
pub use transport::github::GithubReleaseSource;
