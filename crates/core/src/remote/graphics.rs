//! Per-subscription graphics deltas. Geometry travels with every viewport;
//! immutable pixels travel once per visible image generation, outside painting.
use crate::{GraphicsImage, KittyGraphicsRenderPlacement};
use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, Serialize, Deserialize)]
struct ImageKey(u32, u64);

impl ImageKey {
    fn of<I>(placement: &KittyGraphicsRenderPlacement<I>) -> Self {
        Self(placement.image_id, placement.image_generation)
    }
}

#[derive(Serialize, Deserialize)]
struct Image {
    key: ImageKey,
    #[serde(with = "super::serde_image::raw")]
    pixels: Arc<GraphicsImage>,
}

#[derive(Serialize, Deserialize)]
pub struct GraphicsUpdate {
    placements: Vec<KittyGraphicsRenderPlacement<ImageKey>>,
    images: Vec<Image>,
}

#[derive(Default)]
pub struct GraphicsEncoder {
    known: HashSet<ImageKey>,
}

impl GraphicsEncoder {
    pub fn encode(&mut self, placements: &[KittyGraphicsRenderPlacement]) -> GraphicsUpdate {
        let mut visible = HashSet::new();
        let mut images = Vec::new();
        let placements = placements
            .iter()
            .map(|placement| {
                let key = ImageKey::of(placement);
                if visible.insert(key) && !self.known.contains(&key) {
                    images.push(Image {
                        key,
                        pixels: Arc::clone(&placement.image),
                    });
                }
                placement.clone().map_image(key)
            })
            .collect();
        // Matching eviction on both ends bounds retention to the current frame.
        // A reattached subscriber starts with an empty dictionary.
        self.known = visible;
        GraphicsUpdate { placements, images }
    }
}

#[derive(Default)]
pub struct GraphicsDecoder {
    images: HashMap<ImageKey, Arc<GraphicsImage>>,
}

impl GraphicsDecoder {
    pub fn decode(
        &mut self,
        update: GraphicsUpdate,
    ) -> anyhow::Result<Vec<KittyGraphicsRenderPlacement>> {
        let visible: HashSet<_> = update.placements.iter().map(ImageKey::of).collect();
        self.images.retain(|key, _| visible.contains(key));
        for image in update.images {
            ensure!(visible.contains(&image.key), "unreferenced terminal image");
            self.images.insert(image.key, image.pixels);
        }
        update
            .placements
            .into_iter()
            .map(|placement| {
                ensure!(
                    placement.image == ImageKey::of(&placement),
                    "invalid terminal image identity"
                );
                let image = self
                    .images
                    .get(&placement.image)
                    .context("missing terminal image pixels")?;
                ensure!(
                    image.width == placement.image_width && image.height == placement.image_height,
                    "invalid terminal image geometry"
                );
                Ok(placement.map_image(Arc::clone(image)))
            })
            .collect()
    }
}
