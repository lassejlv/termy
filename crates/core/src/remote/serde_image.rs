use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};
use std::sync::Arc;
use tmon::GraphicsImage;

pub fn serialize<S: Serializer>(
    image: &Arc<GraphicsImage>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    (image.width, image.height, image.png().as_ref()).serialize(serializer)
}

pub fn deserialize<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Arc<GraphicsImage>, D::Error> {
    let (width, height, png) = <(u32, u32, Vec<u8>)>::deserialize(deserializer)?;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 64 * 1024 * 1024 {
        return Err(D::Error::custom("invalid terminal image dimensions"));
    }
    Ok(Arc::new(GraphicsImage::from_png(width, height, png)))
}
