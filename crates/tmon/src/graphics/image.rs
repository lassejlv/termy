fn normalize_image(
    command: &GraphicsCommand,
    data: Vec<u8>,
) -> Result<(Vec<u8>, u32, u32), String> {
    match command.u32_value('f').unwrap_or(32) {
        100 => {
            let (data, width, height) = sanitize_png(data)?;
            Ok((data, width, height))
        }
        format @ (24 | 32) => {
            let width = command.u32_value('s').unwrap_or(0);
            let height = command.u32_value('v').unwrap_or(0);
            let channels = if format == 24 { 3 } else { 4 };
            let expected = validate_dimensions(width, height, channels)?;
            if data.len() != expected {
                return Err("EINVAL:pixel data length does not match dimensions".into());
            }
            Ok((
                encode_png(width, height, channels as u8, &data),
                width,
                height,
            ))
        }
        _ => Err("EINVAL:unsupported image format".into()),
    }
}

fn validate_dimensions(width: u32, height: u32, channels: usize) -> Result<usize, String> {
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err("EINVAL:invalid image dimensions".into());
    }
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_PIXELS {
        return Err("EFBIG:image dimensions exceed storage limit".into());
    }
    usize::try_from(pixels)
        .ok()
        .and_then(|pixels| pixels.checked_mul(channels))
        .ok_or_else(|| "EFBIG:image dimensions overflow".into())
}

#[derive(Clone, Copy, Debug)]
struct PngHeader {
    width: u32,
    height: u32,
    bit_depth: u8,
    color_type: u8,
    interlace: u8,
}

#[derive(Clone, Copy, Debug, Default)]
struct PngPass {
    row_bytes: usize,
    rows: usize,
}

fn sanitize_png(mut data: Vec<u8>) -> Result<(Vec<u8>, u32, u32), String> {
    let (width, height) = validate_png(&data)?;
    let mut read_offset = 8usize;
    let mut write_offset = 8usize;

    // The validator established every chunk boundary and CRC. Compact only the chunks that
    // can make a downstream decoder inflate data beyond the validated static IDAT stream.
    while read_offset < data.len() {
        let length =
            u32::from_be_bytes(data[read_offset..read_offset + 4].try_into().unwrap()) as usize;
        let payload_start = read_offset + 8;
        let payload_end = payload_start + length;
        let chunk_end = payload_end + 4;
        let kind: [u8; 4] = data[read_offset + 4..read_offset + 8].try_into().unwrap();
        let strip = match &kind {
            b"iCCP" | b"zTXt" | b"acTL" | b"fcTL" | b"fdAT" => true,
            b"iTXt" => png_itxt_is_compressed(&data[payload_start..payload_end])?,
            _ => false,
        };
        if !strip {
            if write_offset != read_offset {
                data.copy_within(read_offset..chunk_end, write_offset);
            }
            write_offset += chunk_end - read_offset;
        }
        read_offset = chunk_end;
    }
    data.truncate(write_offset);
    Ok((data, width, height))
}

fn png_itxt_is_compressed(payload: &[u8]) -> Result<bool, String> {
    let Some(keyword_end) = payload.iter().position(|byte| *byte == 0) else {
        return Err("EINVAL:invalid PNG image".into());
    };
    if !(1..=79).contains(&keyword_end) {
        return Err("EINVAL:invalid PNG image".into());
    }
    let Some((&compression_flag, rest)) = payload[keyword_end + 1..].split_first() else {
        return Err("EINVAL:invalid PNG image".into());
    };
    let Some((&compression_method, rest)) = rest.split_first() else {
        return Err("EINVAL:invalid PNG image".into());
    };
    if !matches!(compression_flag, 0 | 1) || compression_method != 0 {
        return Err("EINVAL:invalid PNG image".into());
    }
    let Some(language_end) = rest.iter().position(|byte| *byte == 0) else {
        return Err("EINVAL:invalid PNG image".into());
    };
    let translated_keyword_and_text = &rest[language_end + 1..];
    if !translated_keyword_and_text.contains(&0) {
        return Err("EINVAL:invalid PNG image".into());
    }
    Ok(compression_flag == 1)
}

fn validate_png(data: &[u8]) -> Result<(u32, u32), String> {
    if data.len() < 8 || &data[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err("EINVAL:invalid PNG image".into());
    }
    let mut offset = 8usize;
    let mut header = None;
    let mut palette_entries = None;
    let mut saw_transparency = false;
    let mut saw_idat = false;
    let mut idat_ended = false;
    let mut idat_bytes = 0usize;
    while offset.checked_add(12).is_some_and(|end| end <= data.len()) {
        let length = u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        let kind = &data[offset + 4..offset + 8];
        if !kind.iter().all(u8::is_ascii_alphabetic) || !kind[2].is_ascii_uppercase() {
            return Err("EINVAL:invalid PNG image".into());
        }
        let payload_start = offset + 8;
        let Some(payload_end) = payload_start.checked_add(length) else {
            return Err("EINVAL:invalid PNG image".into());
        };
        let Some(chunk_end) = payload_end.checked_add(4) else {
            return Err("EINVAL:invalid PNG image".into());
        };
        if chunk_end > data.len() {
            return Err("EINVAL:invalid PNG image".into());
        }
        let expected_crc = u32::from_be_bytes(data[payload_end..chunk_end].try_into().unwrap());
        if crc32_parts(&[kind, &data[payload_start..payload_end]]) != expected_crc {
            return Err("EINVAL:invalid PNG image".into());
        }

        if saw_idat && kind != b"IDAT" {
            idat_ended = true;
        }
        match kind {
            b"IHDR" if offset == 8 && length == 13 && header.is_none() => {
                let payload = &data[payload_start..payload_end];
                let width = u32::from_be_bytes(payload[0..4].try_into().unwrap());
                let height = u32::from_be_bytes(payload[4..8].try_into().unwrap());
                if !valid_png_format(payload[8], payload[9])
                    || payload[10] != 0
                    || payload[11] != 0
                    || payload[12] > 1
                {
                    return Err("EINVAL:invalid PNG image".into());
                }
                validate_dimensions(width, height, 4)?;
                header = Some(PngHeader {
                    width,
                    height,
                    bit_depth: payload[8],
                    color_type: payload[9],
                    interlace: payload[12],
                });
            }
            b"IHDR" => return Err("EINVAL:invalid PNG image".into()),
            b"PLTE" => {
                let Some(header) = header else {
                    return Err("EINVAL:invalid PNG image".into());
                };
                if palette_entries.is_some()
                    || saw_idat
                    || matches!(header.color_type, 0 | 4)
                    || !(3..=768).contains(&length)
                    || !length.is_multiple_of(3)
                    || length / 3 > 1usize << header.bit_depth
                {
                    return Err("EINVAL:invalid PNG image".into());
                }
                palette_entries = Some(length / 3);
            }
            b"tRNS" => {
                let Some(header) = header else {
                    return Err("EINVAL:invalid PNG image".into());
                };
                if saw_transparency || saw_idat {
                    return Err("EINVAL:invalid PNG image".into());
                }
                let payload = &data[payload_start..payload_end];
                let valid = match header.color_type {
                    0 if length == 2 => {
                        u16::from_be_bytes(payload.try_into().unwrap())
                            <= png_sample_max(header.bit_depth)
                    }
                    2 if length == 6 => payload.as_chunks::<2>().0.iter().all(|sample| {
                        u16::from_be_bytes(*sample)
                            <= png_sample_max(header.bit_depth)
                    }),
                    3 => palette_entries.is_some_and(|entries| length > 0 && length <= entries),
                    _ => false,
                };
                if !valid {
                    return Err("EINVAL:invalid PNG image".into());
                }
                saw_transparency = true;
            }
            b"IDAT" if header.is_some() && !idat_ended => {
                if header.is_some_and(|header| header.color_type == 3) && palette_entries.is_none()
                {
                    return Err("EINVAL:invalid PNG image".into());
                }
                saw_idat = true;
                idat_bytes = idat_bytes
                    .checked_add(length)
                    .ok_or_else(|| "EFBIG:PNG payload exceeds storage limit".to_string())?;
            }
            b"IDAT" => return Err("EINVAL:invalid PNG image".into()),
            b"IEND" if length == 0 && header.is_some() && saw_idat => {
                if chunk_end != data.len() {
                    return Err("EINVAL:invalid PNG image".into());
                }
                let header = header.unwrap();
                if header.color_type == 3 && palette_entries.is_none() {
                    return Err("EINVAL:invalid PNG image".into());
                }
                let (expected, passes, pass_count) = png_scanline_layout(header)?;
                let compressed = png_idat_data(data, idat_bytes)?;
                let filtered = decompress_png_idat(compressed.as_ref(), expected)?;
                validate_png_filters(&filtered, &passes[..pass_count])?;
                return Ok((header.width, header.height));
            }
            b"IEND" => return Err("EINVAL:invalid PNG image".into()),
            _ if kind[0].is_ascii_uppercase() => {
                return Err("EINVAL:invalid PNG image".into());
            }
            _ => {}
        }
        offset = chunk_end;
    }
    Err("EINVAL:invalid PNG image".into())
}

fn png_sample_max(bit_depth: u8) -> u16 {
    if bit_depth == 16 {
        u16::MAX
    } else {
        (1u16 << bit_depth) - 1
    }
}

fn png_scanline_layout(header: PngHeader) -> Result<(usize, [PngPass; 7], usize), String> {
    const ADAM7: [(u64, u64, u64, u64); 7] = [
        (0, 0, 8, 8),
        (4, 0, 8, 8),
        (0, 4, 4, 8),
        (2, 0, 4, 4),
        (0, 2, 2, 4),
        (1, 0, 2, 2),
        (0, 1, 1, 2),
    ];
    const NON_INTERLACED: [(u64, u64, u64, u64); 1] = [(0, 0, 1, 1)];

    let samples = match header.color_type {
        0 | 3 => 1u64,
        2 => 3,
        4 => 2,
        6 => 4,
        _ => return Err("EINVAL:invalid PNG image".into()),
    };
    let patterns = if header.interlace == 0 {
        NON_INTERLACED.as_slice()
    } else {
        ADAM7.as_slice()
    };
    let mut passes = [PngPass::default(); 7];
    let mut expected = 0u64;
    let mut sample_bytes = 0u64;
    let width = u64::from(header.width);
    let height = u64::from(header.height);
    for (index, &(start_x, start_y, step_x, step_y)) in patterns.iter().enumerate() {
        let pass_width = pass_extent(width, start_x, step_x)?;
        let pass_height = pass_extent(height, start_y, step_y)?;
        if pass_width == 0 || pass_height == 0 {
            continue;
        }
        let row_bits = pass_width
            .checked_mul(samples)
            .and_then(|bits| bits.checked_mul(u64::from(header.bit_depth)))
            .ok_or_else(|| "EFBIG:PNG dimensions overflow".to_string())?;
        let row_bytes = row_bits
            .checked_add(7)
            .ok_or_else(|| "EFBIG:PNG dimensions overflow".to_string())?
            / 8;
        let pass_bytes = row_bytes
            .checked_add(1)
            .and_then(|bytes| bytes.checked_mul(pass_height))
            .ok_or_else(|| "EFBIG:PNG dimensions overflow".to_string())?;
        sample_bytes = sample_bytes
            .checked_add(
                row_bytes
                    .checked_mul(pass_height)
                    .ok_or_else(|| "EFBIG:PNG dimensions overflow".to_string())?,
            )
            .ok_or_else(|| "EFBIG:PNG dimensions overflow".to_string())?;
        expected = expected
            .checked_add(pass_bytes)
            .ok_or_else(|| "EFBIG:PNG dimensions overflow".to_string())?;
        if sample_bytes > MAX_UPLOAD_BYTES as u64 || expected > MAX_PNG_SCANLINE_BYTES as u64 {
            return Err("EFBIG:PNG scanlines exceed storage limit".into());
        }
        passes[index] = PngPass {
            row_bytes: usize::try_from(row_bytes)
                .map_err(|_| "EFBIG:PNG dimensions overflow".to_string())?,
            rows: usize::try_from(pass_height)
                .map_err(|_| "EFBIG:PNG dimensions overflow".to_string())?,
        };
    }
    let expected =
        usize::try_from(expected).map_err(|_| "EFBIG:PNG dimensions overflow".to_string())?;
    Ok((expected, passes, patterns.len()))
}

fn pass_extent(total: u64, start: u64, step: u64) -> Result<u64, String> {
    if total <= start {
        return Ok(0);
    }
    total
        .checked_sub(start)
        .and_then(|remaining| remaining.checked_add(step - 1))
        .map(|remaining| remaining / step)
        .ok_or_else(|| "EFBIG:PNG dimensions overflow".into())
}

fn png_idat_data(data: &[u8], total: usize) -> Result<Cow<'_, [u8]>, String> {
    let mut first = None;
    let mut combined: Option<Vec<u8>> = None;
    let mut offset = 8usize;
    while offset + 12 <= data.len() {
        let length = u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        let payload_start = offset + 8;
        let payload_end = payload_start + length;
        if &data[offset + 4..offset + 8] == b"IDAT" {
            let payload = &data[payload_start..payload_end];
            if let Some(combined) = combined.as_mut() {
                combined.extend_from_slice(payload);
            } else if let Some(first) = first {
                let mut bytes = Vec::new();
                bytes
                    .try_reserve_exact(total)
                    .map_err(|_| "ENOMEM:unable to validate PNG image".to_string())?;
                bytes.extend_from_slice(first);
                bytes.extend_from_slice(payload);
                combined = Some(bytes);
            } else {
                first = Some(payload);
            }
        }
        offset = payload_end + 4;
    }

    if let Some(combined) = combined {
        if combined.len() == total {
            Ok(Cow::Owned(combined))
        } else {
            Err("EINVAL:invalid PNG image".into())
        }
    } else if first.is_some_and(|first| first.len() == total) {
        Ok(Cow::Borrowed(first.unwrap()))
    } else {
        Err("EINVAL:invalid PNG image".into())
    }
}

fn decompress_png_idat(compressed: &[u8], expected: usize) -> Result<Vec<u8>, String> {
    inflate_zlib(compressed, OutputLimit::Exact(expected)).map_err(|error| match error {
        InflateError::OutOfMemory => "ENOMEM:unable to validate PNG image".to_string(),
        InflateError::WorkLimitExceeded => "EFBIG:PNG payload exceeds processing limit".to_string(),
        InflateError::InvalidData | InflateError::OutputTooLarge => {
            "EINVAL:invalid PNG image".to_string()
        }
    })
}

fn validate_png_filters(filtered: &[u8], passes: &[PngPass]) -> Result<(), String> {
    let mut offset = 0usize;
    for pass in passes {
        let scanline_bytes = pass
            .row_bytes
            .checked_add(1)
            .ok_or_else(|| "EFBIG:PNG dimensions overflow".to_string())?;
        for _ in 0..pass.rows {
            if filtered.get(offset).is_none_or(|filter| *filter > 4) {
                return Err("EINVAL:invalid PNG image".into());
            }
            offset = offset
                .checked_add(scanline_bytes)
                .ok_or_else(|| "EFBIG:PNG dimensions overflow".to_string())?;
        }
    }
    if offset != filtered.len() {
        return Err("EINVAL:invalid PNG image".into());
    }
    Ok(())
}

fn valid_png_format(bit_depth: u8, color_type: u8) -> bool {
    match color_type {
        0 => matches!(bit_depth, 1 | 2 | 4 | 8 | 16),
        2 => matches!(bit_depth, 8 | 16),
        3 => matches!(bit_depth, 1 | 2 | 4 | 8),
        4 | 6 => matches!(bit_depth, 8 | 16),
        _ => false,
    }
}

fn encode_png(width: u32, height: u32, channels: u8, pixels: &[u8]) -> Vec<u8> {
    let row_bytes = width as usize * usize::from(channels);
    let filtered_len = pixels.len().saturating_add(height as usize);
    let block_count = filtered_len.div_ceil(65_535).max(1);
    let zlib_len = 2usize
        .saturating_add(block_count.saturating_mul(5))
        .saturating_add(filtered_len)
        .saturating_add(4);
    let mut png = Vec::with_capacity(zlib_len.saturating_add(57));
    png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut header = [0; 13];
    header[..4].copy_from_slice(&width.to_be_bytes());
    header[4..8].copy_from_slice(&height.to_be_bytes());
    header[8] = 8;
    header[9] = if channels == 3 { 2 } else { 6 };
    push_png_chunk(&mut png, *b"IHDR", &header);

    png.extend_from_slice(
        &u32::try_from(zlib_len)
            .expect("bounded Tmon image must fit in one PNG IDAT chunk")
            .to_be_bytes(),
    );
    png.extend_from_slice(b"IDAT");
    let mut idat_crc = crc32_update(u32::MAX, b"IDAT");
    push_crc_bytes(&mut png, &mut idat_crc, &[0x78, 0x01]);

    let mut adler = Adler32::new();
    let mut pixel_offset = 0usize;
    let mut filter_pending = true;
    let mut filtered_remaining = filtered_len;
    for block_index in 0..block_count {
        let block_len = filtered_remaining.min(65_535);
        let length = block_len as u16;
        let block_header = [
            u8::from(block_index + 1 == block_count),
            length as u8,
            (length >> 8) as u8,
            (!length) as u8,
            ((!length) >> 8) as u8,
        ];
        push_crc_bytes(&mut png, &mut idat_crc, &block_header);

        let mut block_remaining = block_len;
        while block_remaining > 0 {
            if filter_pending {
                push_crc_bytes(&mut png, &mut idat_crc, &[0]);
                adler.update(&[0]);
                filter_pending = false;
                block_remaining -= 1;
                continue;
            }

            let within_row = pixel_offset % row_bytes;
            let count = block_remaining.min(row_bytes - within_row);
            let pixels = &pixels[pixel_offset..pixel_offset + count];
            push_crc_bytes(&mut png, &mut idat_crc, pixels);
            adler.update(pixels);
            pixel_offset += count;
            block_remaining -= count;
            if pixel_offset.is_multiple_of(row_bytes) {
                filter_pending = true;
            }
        }
        filtered_remaining -= block_len;
    }
    debug_assert_eq!(pixel_offset, pixels.len());
    debug_assert_eq!(filtered_remaining, 0);

    push_crc_bytes(&mut png, &mut idat_crc, &adler.finish().to_be_bytes());
    png.extend_from_slice(&(!idat_crc).to_be_bytes());
    push_png_chunk(&mut png, *b"IEND", &[]);
    png
}

#[cfg(test)]
fn zlib_store(data: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(data.len().saturating_add(data.len() / 65_535 * 5 + 8));
    output.extend_from_slice(&[0x78, 0x01]);
    if data.is_empty() {
        output.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    } else {
        let chunks = data.chunks(65_535);
        let count = chunks.len();
        for (index, chunk) in chunks.enumerate() {
            output.push(u8::from(index + 1 == count));
            let length = chunk.len() as u16;
            output.extend_from_slice(&length.to_le_bytes());
            output.extend_from_slice(&(!length).to_le_bytes());
            output.extend_from_slice(chunk);
        }
    }
    output.extend_from_slice(&adler32(data).to_be_bytes());
    output
}

fn push_png_chunk(output: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) {
    output.extend_from_slice(&(data.len() as u32).to_be_bytes());
    output.extend_from_slice(&kind);
    output.extend_from_slice(data);
    output.extend_from_slice(&crc32_parts(&[&kind, data]).to_be_bytes());
}

fn push_crc_bytes(output: &mut Vec<u8>, crc: &mut u32, data: &[u8]) {
    output.extend_from_slice(data);
    *crc = crc32_update(*crc, data);
}

fn crc32_parts(parts: &[&[u8]]) -> u32 {
    let mut crc = u32::MAX;
    for data in parts {
        crc = crc32_update(crc, data);
    }
    !crc
}

fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    for byte in data {
        crc = CRC32_TABLE[((crc ^ u32::from(*byte)) & 0xff) as usize] ^ (crc >> 8);
    }
    crc
}

const CRC32_TABLE: [u32; 256] = make_crc32_table();

const fn make_crc32_table() -> [u32; 256] {
    let mut table = [0; 256];
    let mut index = 0;
    while index < table.len() {
        let mut crc = index as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
            bit += 1;
        }
        table[index] = crc;
        index += 1;
    }
    table
}

#[derive(Clone, Copy, Debug)]
struct Adler32 {
    a: u32,
    b: u32,
}

impl Adler32 {
    const fn new() -> Self {
        Self { a: 1, b: 0 }
    }

    fn update(&mut self, data: &[u8]) {
        // 5,552 bytes is the largest conventional chunk that keeps both sums in u32.
        for chunk in data.chunks(5_552) {
            for byte in chunk {
                self.a += u32::from(*byte);
                self.b += self.a;
            }
            self.a %= 65_521;
            self.b %= 65_521;
        }
    }

    const fn finish(self) -> u32 {
        (self.b << 16) | self.a
    }
}

#[cfg(test)]
fn adler32(data: &[u8]) -> u32 {
    let mut adler = Adler32::new();
    adler.update(data);
    adler.finish()
}

fn decompress_zlib(command: &GraphicsCommand, data: Vec<u8>) -> Result<Vec<u8>, String> {
    let limit = match command.u32_value('f').unwrap_or(32) {
        format @ (24 | 32) => command
            .u32_value('s')
            .zip(command.u32_value('v'))
            .and_then(|(width, height)| {
                validate_dimensions(width, height, if format == 24 { 3 } else { 4 }).ok()
            })
            .map_or(OutputLimit::AtMost(MAX_UPLOAD_BYTES), OutputLimit::Exact),
        _ => OutputLimit::AtMost(MAX_UPLOAD_BYTES),
    };
    inflate_zlib(&data, limit).map_err(|error| match error {
        InflateError::OutOfMemory => "ENOMEM:unable to decompress image".to_string(),
        InflateError::WorkLimitExceeded => {
            "EFBIG:zlib payload exceeds processing limit".to_string()
        }
        InflateError::InvalidData | InflateError::OutputTooLarge => {
            "EINVAL:invalid zlib payload".to_string()
        }
    })
}
