use crate::tmon::GraphicsImage;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};
use std::sync::Arc;

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

fn validate_dimensions<E: Error>(width: u32, height: u32) -> Result<(), E> {
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 64 * 1024 * 1024 {
        return Err(E::custom("invalid terminal image dimensions"));
    }
    Ok(())
}

/// The negotiated graphics stream transfers decoded pixels once. Preserve the
/// legacy PNG representation above for clients and hosts using protocol v1.
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
        let rgba = image.rgba();
        let bytes = rgba.unwrap_or_else(|| image.png());
        (image.width, image.height, rgba.is_some(), Bytes(bytes)).serialize(serializer)
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
