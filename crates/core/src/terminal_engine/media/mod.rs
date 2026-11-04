//! Renderer-neutral image ownership, animation, layout and shared-memory IO.
//! These utilities are independent of both the grid and the terminal parser.

mod animation;
mod geometry;
mod image;
mod shared_memory;

pub use animation::{
    GraphicsAnimation, GraphicsAnimationControl, GraphicsComposition, GraphicsFrame,
    GraphicsFrameUpdate,
};
pub use geometry::{
    GraphicsDisplayLayout, GraphicsRowSpan, graphics_display_layout, graphics_display_size,
};
pub use image::GraphicsImage;
pub use shared_memory::{read_graphics_shared_memory, unlink_graphics_shared_memory};

/// Decode a PNG this module encoded: 8-bit RGBA of exactly `width` x `height`.
pub(crate) fn decode_rgba_png(png: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(png))
        .read_info()
        .ok()?;
    let info = reader.info();
    if (info.width, info.height, info.color_type, info.bit_depth)
        != (width, height, png::ColorType::Rgba, png::BitDepth::Eight)
    {
        return None;
    }
    let mut pixels = vec![0; reader.output_buffer_size()?];
    let frame = reader.next_frame(&mut pixels).ok()?;
    pixels.truncate(frame.buffer_size());
    (pixels.len() as u64 == u64::from(width) * u64::from(height) * 4).then_some(pixels)
}

pub(crate) fn encode_png(width: u32, height: u32, channels: u8, pixels: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut output, width, height);
        encoder.set_color(if channels == 4 {
            png::ColorType::Rgba
        } else {
            png::ColorType::Rgb
        });
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("validated image dimensions");
        writer
            .write_image_data(pixels)
            .expect("validated image pixels");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lazy_png_export_round_trips_pixels_and_reuses_encoded_allocation() {
        let image = GraphicsImage::from_rgba(1, 1, vec![12, 34, 56, 255]);
        let first = image.png();
        assert!(std::sync::Arc::ptr_eq(&first, &image.png()));
        let mut decoder = png::Decoder::new(std::io::Cursor::new(first.as_ref()))
            .read_info()
            .unwrap();
        let mut pixels = vec![0; decoder.output_buffer_size().unwrap()];
        decoder.next_frame(&mut pixels).unwrap();
        assert_eq!(pixels, vec![12, 34, 56, 255]);
    }

    #[test]
    fn rgba_png_exports_release_encoded_storage_after_the_last_owner_drops() {
        let image = GraphicsImage::from_rgba(1, 1, vec![12, 34, 56, 255]);
        let first = image.png();
        let second = image.png();
        let weak = std::sync::Arc::downgrade(&first);
        assert!(std::sync::Arc::ptr_eq(&first, &second));
        assert_eq!(image.png_encoding_count(), 1);
        assert_eq!(image.byte_len(), 4);
        drop(first);
        assert!(weak.upgrade().is_some());
        drop(second);
        assert!(weak.upgrade().is_none());
        assert_eq!(image.byte_len(), 4);

        let exported_again = image.png();
        assert!(exported_again.starts_with(b"\x89PNG"));
        assert_eq!(image.png_encoding_count(), 2);
    }

    #[test]
    fn png_source_storage_remains_owned_and_charged() {
        let bytes = encode_png(1, 1, 4, &[12, 34, 56, 255]);
        let encoded_capacity = bytes.capacity();
        let image = GraphicsImage::from_png(1, 1, bytes);
        let exported = image.png();
        let weak = std::sync::Arc::downgrade(&exported);
        drop(exported);
        assert!(weak.upgrade().is_some());
        assert_eq!(image.byte_len(), 4 + encoded_capacity);
        assert_eq!(image.png_encoding_count(), 0);
    }

    #[test]
    fn png_source_accounts_for_spare_vector_capacity() {
        let mut bytes = Vec::with_capacity(4096);
        bytes.extend_from_slice(&encode_png(1, 1, 4, &[12, 34, 56, 255]));
        let capacity = bytes.capacity();
        assert!(capacity > bytes.len());
        let image = GraphicsImage::from_png(1, 1, bytes);
        assert_eq!(image.byte_len(), 4 + capacity);
        assert!(image.png().starts_with(b"\x89PNG"));
    }
}
