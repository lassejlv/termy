use gpui::{App, ImageSource, RenderImage};
use std::{collections::HashMap, rc::Rc, sync::Arc};
use termy_core::KittyGraphicsRenderPlacement;

struct Texture {
    source: ImageSource,
    bytes: usize,
    cx: gpui::AsyncApp,
}

impl Drop for Texture {
    fn drop(&mut self) {
        if let ImageSource::Render(image) = &self.source {
            let image = image.clone();
            // Cache invalidation and pane removal happen while App is borrowed.
            // Release atlas entries on the next foreground turn, after painting.
            self.cx
                .spawn(async move |cx| {
                    let _ = cx.update(|cx| cx.drop_image(image, None));
                })
                .detach();
        }
    }
}

#[derive(Clone, Default)]
pub(super) struct KittyImageCache {
    entries: HashMap<(u32, u64), (Rc<Texture>, u64)>,
    clock: u64,
    bytes: usize,
}

impl KittyImageCache {
    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    pub(super) fn get(
        &mut self,
        placement: &KittyGraphicsRenderPlacement,
        cx: &App,
    ) -> ImageSource {
        self.clock = self.clock.wrapping_add(1);
        let key = (placement.image_id, placement.image_generation);
        if let Some((texture, used)) = self.entries.get_mut(&key) {
            *used = self.clock;
            return texture.source.clone();
        }
        let source = if let Some(rgba) = placement.image.rgba() {
            let bgra = padded_bgra(rgba, placement.image_width, placement.image_height);
            let buffer = image::RgbaImage::from_raw(
                placement.image_width + 2,
                placement.image_height + 2,
                bgra,
            )
            .expect("validated Kitty image dimensions");
            ImageSource::Render(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
        } else {
            Arc::new(gpui::Image::from_bytes(
                gpui::ImageFormat::Png,
                placement.image.png().to_vec(),
            ))
            .into()
        };
        let bytes = if placement.image.rgba().is_some() {
            (placement.image_width as usize + 2) * (placement.image_height as usize + 2) * 4
        } else {
            placement.image.byte_len()
        };
        self.entries.insert(
            key,
            (
                Rc::new(Texture {
                    source: source.clone(),
                    bytes,
                    cx: cx.to_async(),
                }),
                self.clock,
            ),
        );
        self.bytes = self.bytes.saturating_add(bytes);
        while self.entries.len() > 256 || (self.entries.len() > 1 && self.bytes > 128 * 1024 * 1024)
        {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (_, used))| used)
                .map(|(key, _)| *key)
            else {
                break;
            };
            if let Some((texture, _)) = self.entries.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(texture.bytes);
            }
        }
        source
    }
}

// GPUI linearly samples atlas tiles at their pixel edges. A one-pixel repeated
// border keeps that sampling inside the image, including when a 1x1 image is
// enlarged over many terminal cells. The renderer clips this border away.
fn padded_bgra(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let width = width as usize;
    let height = height as usize;
    let stride = (width + 2) * 4;
    let mut output = vec![0; stride * (height + 2)];
    for y in 0..height {
        let row = &mut output[(y + 1) * stride..(y + 2) * stride];
        let source = &rgba[y * width * 4..(y + 1) * width * 4];
        for (source, destination) in source
            .chunks_exact(4)
            .zip(row[4..4 + width * 4].chunks_exact_mut(4))
        {
            destination.copy_from_slice(&[source[2], source[1], source[0], source[3]]);
        }
        row.copy_within(4..8, 0);
        row.copy_within(width * 4..width * 4 + 4, (width + 1) * 4);
    }
    output.copy_within(stride..2 * stride, 0);
    output.copy_within(
        height * stride..(height + 1) * stride,
        (height + 1) * stride,
    );
    output
}

#[cfg(test)]
mod tests {
    use super::padded_bgra;

    #[test]
    fn kitty_one_pixel_texture_has_no_transparent_sampling_border() {
        let texture = padded_bgra(&[240, 160, 50, 255], 1, 1);
        assert_eq!(texture.len(), 3 * 3 * 4);
        assert!(
            texture
                .chunks_exact(4)
                .all(|pixel| pixel == [50, 160, 240, 255])
        );
    }

    #[test]
    fn kitty_texture_border_repeats_each_edge_and_preserves_alpha() {
        let texture = padded_bgra(&[1, 2, 3, 40, 5, 6, 7, 80], 2, 1);
        let row = [3, 2, 1, 40, 3, 2, 1, 40, 7, 6, 5, 80, 7, 6, 5, 80];
        assert_eq!(texture, row.repeat(3));
    }
}
