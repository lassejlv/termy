use super::*;

pub(super) fn normalize_image(
    command: &KittyGraphicsCommand,
    data: Vec<u8>,
) -> Result<(crate::tmon::GraphicsImage, u32, u32), String> {
    let (rgba, width, height) = match command.u32_value('f').unwrap_or(32) {
        100 => {
            let mut decoder = png::Decoder::new(Cursor::new(data));
            decoder
                .set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
            let mut reader = decoder
                .read_info()
                .map_err(|_| "EINVAL:invalid PNG image")?;
            let (width, height) = (reader.info().width, reader.info().height);
            validate_dimensions(width, height, 4)?;
            let length = reader
                .output_buffer_size()
                .filter(|n| *n <= MAX_IMAGE_BYTES)
                .ok_or("EFBIG:decoded PNG exceeds storage limit")?;
            let mut pixels = vec![0; length];
            let info = reader
                .next_frame(&mut pixels)
                .map_err(|_| "EINVAL:invalid PNG image")?;
            pixels.truncate(info.buffer_size());
            reader.finish().map_err(|_| "EINVAL:invalid PNG image")?;
            // Kitty's PNG transport contains a single static frame; animation uses a=f.
            let rgba = match info.color_type {
                png::ColorType::Rgba => pixels,
                png::ColorType::Rgb => pixels
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .flat_map(|&[r, g, b]| [r, g, b, 255])
                    .collect(),
                png::ColorType::Grayscale => {
                    pixels.into_iter().flat_map(|g| [g, g, g, 255]).collect()
                }
                png::ColorType::GrayscaleAlpha => pixels
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .flat_map(|&[g, a]| [g, g, g, a])
                    .collect(),
                png::ColorType::Indexed => return Err("EINVAL:unexpanded PNG palette".into()),
            };
            (rgba, width, height)
        }
        format @ (24 | 32) => {
            let width = command.u32_value('s').unwrap_or(0);
            let height = command.u32_value('v').unwrap_or(0);
            let channels = if format == 24 { 3 } else { 4 };
            if data.len() != validate_dimensions(width, height, channels)? {
                return Err("EINVAL:pixel data length does not match dimensions".into());
            }
            let rgba = if channels == 4 {
                data
            } else {
                data.as_chunks::<3>()
                    .0
                    .iter()
                    .flat_map(|&[r, g, b]| [r, g, b, 255])
                    .collect()
            };
            (rgba, width, height)
        }
        _ => return Err("EINVAL:unsupported image format".into()),
    };
    Ok((
        crate::tmon::GraphicsImage::from_rgba(width, height, rgba),
        width,
        height,
    ))
}
