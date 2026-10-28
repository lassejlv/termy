use crate::KittyGraphicsRenderPlacement;
use crate::terminal_engine::media::GraphicsImage;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};
use std::{collections::HashMap, sync::Arc};

impl Serialize for GraphicsImage {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (self.width, self.height, self.png().as_ref()).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for GraphicsImage {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (width, height, png) = <(u32, u32, Vec<u8>)>::deserialize(deserializer)?;
        validate_dimensions::<D::Error>(width, height)?;
        Ok(GraphicsImage::from_png(width, height, png))
    }
}

pub(crate) fn retain_png_exports(
    placements: &[KittyGraphicsRenderPlacement],
) -> HashMap<*const GraphicsImage, Arc<Vec<u8>>> {
    let mut exports = HashMap::new();
    for placement in placements {
        exports
            .entry(Arc::as_ptr(&placement.image))
            .or_insert_with(|| placement.image.png());
    }
    exports
}

/// Preserve the legacy placement wire format while sharing a PNG export among
/// every placement of an image for the duration of a serialization pass.
pub(crate) mod placements {
    use super::*;

    pub fn serialize<S: Serializer>(
        placements: &[KittyGraphicsRenderPlacement],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let _exports = retain_png_exports(placements);
        placements.serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<KittyGraphicsRenderPlacement>, D::Error> {
        Vec::deserialize(deserializer)
    }
}

fn validate_dimensions<E: Error>(width: u32, height: u32) -> Result<(), E> {
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 64 * 1024 * 1024 {
        return Err(E::custom("invalid terminal image dimensions"));
    }
    Ok(())
}

/// The graphics stream (`Update::GraphicsState`) transfers decoded pixels
/// once. The PNG representation above remains for `RemoteReply::Graphics`,
/// the snapshot a `RemoteBackend` requests when its state carries no graphics.
pub(crate) mod raw {
    use super::*;

    struct Bytes<'a>(&'a [u8]);
    impl Serialize for Bytes<'_> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_bytes(self.0)
        }
    }

    struct OwnedBytes(Vec<u8>);
    impl<'de> Deserialize<'de> for OwnedBytes {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct Visitor;
            impl<'de> serde::de::Visitor<'de> for Visitor {
                type Value = OwnedBytes;
                fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                    formatter.write_str("terminal image bytes")
                }
                fn visit_bytes<E: Error>(self, bytes: &[u8]) -> Result<Self::Value, E> {
                    Ok(OwnedBytes(bytes.to_vec()))
                }
                fn visit_byte_buf<E: Error>(self, bytes: Vec<u8>) -> Result<Self::Value, E> {
                    Ok(OwnedBytes(bytes))
                }
            }
            deserializer.deserialize_byte_buf(Visitor)
        }
    }

    pub fn serialize<S: Serializer>(
        image: &Arc<GraphicsImage>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        if let Some(rgba) = image.rgba() {
            (image.width, image.height, true, Bytes(rgba)).serialize(serializer)
        } else {
            let png = image.png();
            (image.width, image.height, false, Bytes(png.as_slice())).serialize(serializer)
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Arc<GraphicsImage>, D::Error> {
        let (width, height, rgba, OwnedBytes(bytes)) =
            <(u32, u32, bool, OwnedBytes)>::deserialize(deserializer)?;
        validate_dimensions::<D::Error>(width, height)?;
        let image = if rgba {
            if bytes.len() as u64 != u64::from(width) * u64::from(height) * 4 {
                return Err(D::Error::custom("invalid terminal image pixel count"));
            }
            GraphicsImage::from_rgba(width, height, bytes)
        } else {
            GraphicsImage::from_png(width, height, bytes)
        };
        Ok(Arc::new(image))
    }
}
