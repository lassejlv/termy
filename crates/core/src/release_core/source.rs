use anyhow::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAsset {
    pub name: String,
    pub download_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleasePayload {
    pub tag_name: String,
    pub release_url: String,
    pub assets: Vec<ReleaseAsset>,
}

pub trait ReleaseSource {
    fn fetch_latest_release(&self) -> Result<ReleasePayload>;
}
