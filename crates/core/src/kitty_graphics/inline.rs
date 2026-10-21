//! iTerm2 inline images (OSC 1337 `File=`), stored and placed as quiet,
//! anonymous kitty images. The OSC's sizing arguments resolve to kitty's cell
//! counts once the image is decoded and its pixel size is known.

use super::*;
use crate::terminal_engine::media::GraphicsImage;

/// One `width` or `height` argument.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Dimension {
    #[default]
    Auto,
    Cells(u32),
    Pixels(u32),
    Percent(u32),
}

impl Dimension {
    fn parse(value: &str) -> Self {
        let number = |text: &str| text.parse::<u32>().ok().filter(|n| *n > 0);
        if let Some(pixels) = value.strip_suffix("px") {
            number(pixels).map_or(Self::Auto, Self::Pixels)
        } else if let Some(percent) = value.strip_suffix('%') {
            number(percent).map_or(Self::Auto, Self::Percent)
        } else {
            number(value).map_or(Self::Auto, Self::Cells)
        }
    }

    /// The length in pixels, or `None` for `auto`.
    fn pixels(self, cell: f32, screen: f32) -> Option<f32> {
        match self {
            Self::Auto => None,
            Self::Cells(cells) => Some(cells as f32 * cell),
            Self::Pixels(pixels) => Some(pixels as f32),
            Self::Percent(percent) => Some(screen * percent.min(100) as f32 / 100.0),
        }
    }
}

/// The sizing arguments of an inline image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InlineImage {
    width: Dimension,
    height: Dimension,
    preserve_aspect_ratio: bool,
}

impl InlineImage {
    /// Kitty's `c` and `r` for an image of `width`×`height` pixels placed at
    /// `cursor_col`. Like iTerm2, an `auto` image shrinks to fit the space to
    /// its right and the screen height, and the aspect ratio is kept unless
    /// `preserveAspectRatio=0`.
    pub(super) fn display_cells(
        &self,
        width: u32,
        height: u32,
        size: TerminalSize,
        cursor_col: usize,
    ) -> (Option<u32>, Option<u32>) {
        let (cell_width, cell_height) = (size.cell_width.max(1.0), size.cell_height.max(1.0));
        let screen_height = f32::from(size.rows) * cell_height;
        let (natural_width, natural_height) = (width.max(1) as f32, height.max(1) as f32);
        let cells = |pixels: f32, cell: f32| (pixels / cell).round().max(1.0) as u32;
        let requested = (
            self.width
                .pixels(cell_width, f32::from(size.cols) * cell_width),
            self.height.pixels(cell_height, screen_height),
        );
        let (width, height) = match requested {
            (None, None) => {
                let available = usize::from(size.cols).saturating_sub(cursor_col).max(1);
                let scale = (available as f32 * cell_width / natural_width)
                    .min(screen_height / natural_height);
                if scale >= 1.0 {
                    return (None, None);
                }
                (natural_width * scale, natural_height * scale)
            }
            (Some(width), None) if self.preserve_aspect_ratio => {
                (width, width * natural_height / natural_width)
            }
            (None, Some(height)) if self.preserve_aspect_ratio => {
                (height * natural_width / natural_height, height)
            }
            (Some(width), Some(height)) if self.preserve_aspect_ratio => {
                let scale = (width / natural_width).min(height / natural_height);
                (natural_width * scale, natural_height * scale)
            }
            (width, height) => {
                return (
                    Some(cells(width.unwrap_or(natural_width), cell_width)),
                    Some(cells(height.unwrap_or(natural_height), cell_height)),
                );
            }
        };
        // Kitty derives the other dimension from the aspect ratio, so name
        // only the one spanning more cells: its rounding distorts the least.
        if width / cell_width >= height / cell_height {
            (Some(cells(width, cell_width)), None)
        } else {
            (None, Some(cells(height, cell_height)))
        }
    }
}

impl KittyGraphicsCommand {
    /// An OSC 1337 file with `args` (`key=value` pairs separated by `;`) and
    /// a base64 `payload`, as a quiet, anonymous kitty transmit-and-place.
    /// `None` unless the file is shown inline: iTerm2 offers other files as
    /// downloads.
    pub(crate) fn inline_image(args: &[u8], payload: Vec<u8>) -> Option<Self> {
        let mut inline = false;
        let mut move_cursor = true;
        let mut image = InlineImage {
            width: Dimension::Auto,
            height: Dimension::Auto,
            preserve_aspect_ratio: true,
        };
        for (key, value) in std::str::from_utf8(args)
            .ok()?
            .split(';')
            .filter_map(|arg| arg.split_once('='))
        {
            match key {
                "inline" => inline = value == "1",
                "width" => image.width = Dimension::parse(value),
                "height" => image.height = Dimension::parse(value),
                "preserveAspectRatio" => image.preserve_aspect_ratio = value != "0",
                "doNotMoveCursor" => move_cursor = value != "1",
                _ => {}
            }
        }
        inline.then(|| Self {
            control: vec![
                ('a', "T".into()),
                ('q', "2".into()),
                ('C', if move_cursor { "0" } else { "1" }.into()),
            ],
            payload,
            oversized: false,
            inline: Some(image),
        })
    }
}

/// An OSC 1337 `MultipartFile=` upload collecting its `FilePart=`s, which
/// split one base64 stream at arbitrary points.
pub(crate) struct InlineMultipart {
    args: Vec<u8>,
    encoded: Vec<u8>,
}

impl InlineMultipart {
    pub(crate) fn new(args: &[u8]) -> Self {
        Self {
            args: args.to_vec(),
            encoded: Vec::new(),
        }
    }

    /// Append one part; false once the file exceeds the image storage limit.
    pub(crate) fn push(&mut self, part: &[u8]) -> bool {
        if self.encoded.len().saturating_add(part.len()) / 4 * 3 > MAX_IMAGE_BYTES {
            return false;
        }
        self.encoded.extend_from_slice(part);
        true
    }

    pub(crate) fn finish(self) -> Option<KittyGraphicsCommand> {
        KittyGraphicsCommand::inline_image(&self.args, self.encoded)
    }
}

/// Decode an inline image file: PNG, JPEG, GIF (its first frame), WebP,
/// TIFF or BMP, recognized by its contents.
pub(super) fn decode(data: Vec<u8>) -> Result<(GraphicsImage, u32, u32), String> {
    let mut reader = ::image::ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(|_| "EINVAL:unreadable image file")?;
    let mut limits = ::image::Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_IMAGE_BYTES as u64);
    reader.limits(limits);
    let rgba = reader
        .decode()
        .map_err(|_| "EINVAL:invalid or unsupported image file")?
        .into_rgba8();
    let (width, height) = rgba.dimensions();
    validate_dimensions(width, height, 4)?;
    Ok((
        GraphicsImage::from_rgba(width, height, rgba.into_raw()),
        width,
        height,
    ))
}

#[cfg(test)]
mod tests;
