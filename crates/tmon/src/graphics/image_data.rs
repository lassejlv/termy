use std::sync::{Arc, OnceLock};

/// Immutable image pixels shared by protocol storage, snapshots and renderers.
/// PNG encoding is deferred until an export or clipboard consumer requests it.
#[derive(Debug)]
pub struct GraphicsImage {
    pub width: u32,
    pub height: u32,
    rgba: Option<Arc<[u8]>>,
    png: OnceLock<Arc<[u8]>>,
    encoded_source_bytes: usize,
}

impl PartialEq for GraphicsImage {
    fn eq(&self, other: &Self) -> bool {
        self.width == other.width
            && self.height == other.height
            && self.rgba == other.rgba
            && (self.rgba.is_some() || self.png.get() == other.png.get())
    }
}
impl Eq for GraphicsImage {}

impl GraphicsImage {
    pub fn from_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        assert_eq!(u64::from(width) * u64::from(height) * 4, rgba.len() as u64);
        Self {
            width,
            height,
            rgba: Some(rgba.into()),
            png: OnceLock::new(),
            encoded_source_bytes: 0,
        }
    }

    pub fn from_png(width: u32, height: u32, png: Vec<u8>) -> Self {
        let encoded_source_bytes = png.len();
        Self {
            width,
            height,
            rgba: None,
            encoded_source_bytes,
            png: OnceLock::from(Arc::from(png)),
        }
    }

    pub fn rgba(&self) -> Option<&[u8]> {
        self.rgba.as_deref()
    }

    /// Decoded size is charged even for compressed PNGs, bounding texture memory.
    pub fn byte_len(&self) -> usize {
        (self.width as usize)
            .saturating_mul(self.height as usize)
            .saturating_mul(4)
            .saturating_add(self.encoded_source_bytes)
    }

    pub fn png(&self) -> &Arc<[u8]> {
        self.png.get_or_init(|| {
            super::encode_png(self.width, self.height, 4, self.rgba().unwrap_or_default()).into()
        })
    }
}
