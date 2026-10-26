//! Per-subscription graphics deltas. Geometry travels with every frame;
//! immutable pixels travel once per image generation, each in a message of
//! its own ([`ImageUpload`]), outside painting.
//!
//! Both ends keep the images sent so far in a cache that the encoder bounds
//! (audit I2): an image that scrolls back into view, or an animation frame
//! shown before, is not sent again. The encoder alone decides what to evict,
//! least recently shown first and never an image on screen, and lists the
//! evicted images in the next [`GraphicsUpdate`], so the caches always match.
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

/// The pixels of one image generation, sent before the first frame that
/// shows it.
#[derive(Serialize, Deserialize)]
pub struct ImageUpload {
    key: ImageKey,
    #[serde(with = "super::serde_image::raw")]
    pixels: Arc<GraphicsImage>,
}

#[derive(Serialize, Deserialize)]
pub struct GraphicsUpdate {
    placements: Vec<KittyGraphicsRenderPlacement<ImageKey>>,
    /// Images both ends drop before this update applies.
    evicted: Vec<ImageKey>,
}

/// The pixels a subscription's caches may keep beyond those on screen.
pub const IMAGE_CACHE_BYTES: usize = 32 * 1024 * 1024;

fn image_bytes(image: &GraphicsImage) -> usize {
    image.width as usize * image.height as usize * 4
}

pub struct GraphicsEncoder {
    /// The images the client has, least recently shown first.
    cache: Vec<(ImageKey, usize)>,
    bytes: usize,
    budget: usize,
}

impl Default for GraphicsEncoder {
    fn default() -> Self {
        Self::with_budget(IMAGE_CACHE_BYTES)
    }
}

impl GraphicsEncoder {
    pub fn with_budget(budget: usize) -> Self {
        Self {
            cache: Vec::new(),
            bytes: 0,
            budget,
        }
    }

    /// The images to send before the update, and the update.
    pub fn encode(
        &mut self,
        placements: &[KittyGraphicsRenderPlacement],
    ) -> (Vec<ImageUpload>, GraphicsUpdate) {
        let mut visible = HashSet::new();
        let mut uploads = Vec::new();
        let placements = placements
            .iter()
            .map(|placement| {
                let key = ImageKey::of(placement);
                if visible.insert(key) {
                    if let Some(index) = self.cache.iter().position(|(cached, _)| *cached == key) {
                        let entry = self.cache.remove(index);
                        self.cache.push(entry);
                    } else {
                        let bytes = image_bytes(&placement.image);
                        self.cache.push((key, bytes));
                        self.bytes += bytes;
                        uploads.push(ImageUpload {
                            key,
                            pixels: Arc::clone(&placement.image),
                        });
                    }
                }
                placement.clone().map_image(key)
            })
            .collect();
        let mut evicted = Vec::new();
        while self.bytes > self.budget {
            let Some(index) = self
                .cache
                .iter()
                .position(|(key, _)| !visible.contains(key))
            else {
                break;
            };
            let (key, bytes) = self.cache.remove(index);
            self.bytes -= bytes;
            evicted.push(key);
        }
        (
            uploads,
            GraphicsUpdate {
                placements,
                evicted,
            },
        )
    }
}

#[derive(Default)]
pub struct GraphicsDecoder {
    images: HashMap<ImageKey, Arc<GraphicsImage>>,
}

impl GraphicsDecoder {
    pub fn insert(&mut self, upload: ImageUpload) {
        self.images.insert(upload.key, upload.pixels);
    }

    /// Drops every cached image that `placements` do not show.
    pub fn retain_shown(&mut self, placements: &[KittyGraphicsRenderPlacement]) {
        let shown: HashSet<_> = placements.iter().map(ImageKey::of).collect();
        self.images.retain(|key, _| shown.contains(key));
    }

    pub fn decode(
        &mut self,
        update: GraphicsUpdate,
    ) -> anyhow::Result<Vec<KittyGraphicsRenderPlacement>> {
        for key in &update.evicted {
            self.images.remove(key);
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

    #[cfg(test)]
    pub(crate) fn cached(&self) -> usize {
        self.images.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Terminal, TerminalSize};

    /// One subscription: an encoder, the wire and a decoder, counting the
    /// image bytes sent.
    struct Stream {
        encoder: GraphicsEncoder,
        decoder: GraphicsDecoder,
        sent: usize,
    }

    impl Stream {
        fn new(budget: usize) -> Self {
            Self {
                encoder: GraphicsEncoder::with_budget(budget),
                decoder: GraphicsDecoder::default(),
                sent: 0,
            }
        }

        /// Sends the terminal's placements and checks the client sees the
        /// same images.
        fn frame(&mut self, terminal: &Terminal) -> Vec<KittyGraphicsRenderPlacement> {
            let (_, placements) = terminal.kitty_graphics_snapshot();
            let (uploads, update) = self.encoder.encode(&placements);
            for upload in uploads {
                let wire = bincode::serialize(&upload).unwrap();
                self.sent += wire.len();
                self.decoder.insert(bincode::deserialize(&wire).unwrap());
            }
            let wire = bincode::serialize(&update).unwrap();
            let decoded = self
                .decoder
                .decode(bincode::deserialize(&wire).unwrap())
                .unwrap();
            assert_eq!(decoded.len(), placements.len());
            for (decoded, placement) in decoded.iter().zip(&placements) {
                assert_eq!(decoded.image.rgba(), placement.image.rgba());
                assert_eq!(decoded.viewport_row, placement.viewport_row);
            }
            decoded
        }
    }

    fn terminal() -> Terminal {
        Terminal::new_display(
            TerminalSize {
                cols: 20,
                rows: 6,
                cell_width: 10.0,
                cell_height: 20.0,
            },
            None,
        )
    }

    /// A 4x4 image `id` of one color, placed at the cursor.
    fn image(id: u32, color: u8) -> Vec<u8> {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        let pixels = STANDARD.encode([color; 64]);
        format!("\x1b_Ga=T,i={id},f=32,s=4,v=4,c=1,r=1,C=1,q=2;{pixels}\x1b\\").into_bytes()
    }

    #[test]
    fn animation_frames_shown_before_are_not_sent_again() {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        let terminal = terminal();
        terminal.feed_output(&image(1, 10));
        for color in [20u8, 30] {
            let pixels = STANDARD.encode([color; 64]);
            terminal
                .feed_output(format!("\x1b_Ga=f,i=1,f=32,s=4,v=4,q=2;{pixels}\x1b\\").as_bytes());
        }
        let mut stream = Stream::new(IMAGE_CACHE_BYTES);
        for frame in 1..=3 {
            terminal.feed_output(format!("\x1b_Ga=a,i=1,c={frame},q=2\x1b\\").as_bytes());
            stream.frame(&terminal);
        }
        let first_cycle = stream.sent;
        assert!(first_cycle >= 3 * 64);
        // The loop repeats: every frame is cached at the client.
        for _ in 0..3 {
            for frame in 1..=3 {
                terminal.feed_output(format!("\x1b_Ga=a,i=1,c={frame},q=2\x1b\\").as_bytes());
                stream.frame(&terminal);
            }
        }
        assert_eq!(stream.sent, first_cycle);
    }

    #[test]
    fn composed_frames_send_their_new_pixels() {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        let terminal = terminal();
        terminal.feed_output(&image(1, 10));
        let pixels = STANDARD.encode([20u8; 64]);
        terminal.feed_output(format!("\x1b_Ga=f,i=1,f=32,s=4,v=4,q=2;{pixels}\x1b\\").as_bytes());
        terminal.feed_output(b"\x1b_Ga=a,i=1,c=1,q=2\x1b\\");
        let mut stream = Stream::new(IMAGE_CACHE_BYTES);
        assert_eq!(stream.frame(&terminal)[0].image.rgba().unwrap()[0], 10);
        let before = stream.sent;
        // Composing frame 2 into the shown frame 1 changes its pixels.
        terminal.feed_output(b"\x1b_Ga=c,i=1,r=2,c=1,w=1,h=1,C=1,q=2\x1b\\");
        terminal.feed_output(b"\x1b_Ga=a,i=1,c=1,q=2\x1b\\");
        let decoded = stream.frame(&terminal);
        assert_eq!(decoded[0].image.rgba().unwrap()[0], 20);
        assert!(stream.sent > before, "the composed pixels were not sent");
    }

    #[test]
    fn images_survive_an_alternate_screen_and_go_after_a_reset() {
        let terminal = terminal();
        terminal.feed_output(&image(1, 10));
        let mut stream = Stream::new(IMAGE_CACHE_BYTES);
        assert_eq!(stream.frame(&terminal).len(), 1);
        let sent = stream.sent;
        // A full-screen program hides the image, then gives the screen back.
        terminal.feed_output(b"\x1b[?1049h");
        assert!(stream.frame(&terminal).is_empty());
        terminal.feed_output(b"\x1b[?1049l");
        assert_eq!(stream.frame(&terminal).len(), 1);
        assert_eq!(stream.sent, sent, "the image was sent again");
        // A reset deletes it; a new image with the same id is a new one.
        terminal.feed_output(b"\x1bc");
        assert!(stream.frame(&terminal).is_empty());
        terminal.feed_output(&image(1, 99));
        let decoded = stream.frame(&terminal);
        assert_eq!(decoded[0].image.rgba().unwrap()[0], 99);
        assert!(stream.sent > sent);
    }

    #[test]
    fn the_cache_evicts_least_recently_shown_images_on_both_ends() {
        let terminal = terminal();
        // Room for two of the 64-byte images besides those on screen.
        let mut stream = Stream::new(128);
        for id in 1..=4 {
            terminal.feed_output(b"\x1b[2J\x1b[H");
            terminal.feed_output(b"\x1b_Ga=d,d=A,q=2\x1b\\");
            terminal.feed_output(&image(id, id as u8));
            stream.frame(&terminal);
        }
        assert_eq!(stream.decoder.cached(), 2);
        assert_eq!(stream.encoder.cache.len(), 2);
        assert!(stream.encoder.bytes <= 128);
        // Images on screen are kept even past the budget.
        let mut tight = Stream::new(0);
        terminal.feed_output(&image(5, 5));
        tight.frame(&terminal);
        assert_eq!(tight.decoder.cached(), 2);
    }
}
