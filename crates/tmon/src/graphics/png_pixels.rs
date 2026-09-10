// Decode already validated PNG scanlines. Keep this in the engine so terminal
// animations and snapshots never depend on a host's image decoder.
fn decode_png_pixels(data: &[u8], header: PngHeader, filtered: &[u8]) -> Result<Vec<u8>, String> {
    let mut palette = &[][..];
    let mut transparency = &[][..];
    let mut offset = 8;
    while offset + 12 <= data.len() {
        let length = u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        let payload = &data[offset + 8..offset + 8 + length];
        match &data[offset + 4..offset + 8] {
            b"PLTE" => palette = payload,
            b"tRNS" => transparency = payload,
            _ => {}
        }
        offset += length + 12;
    }
    let channels = match header.color_type { 0 | 3 => 1, 2 => 3, 4 => 2, 6 => 4, _ => unreachable!() };
    let bits = header.bit_depth as usize;
    let bpp = (channels * bits).div_ceil(8);
    let patterns: &[(usize, usize, usize, usize)] = if header.interlace == 0 {
        &[(0, 0, 1, 1)]
    } else { &[(0,0,8,8), (4,0,8,8), (0,4,4,8), (2,0,4,4), (0,2,2,4), (1,0,2,2), (0,1,1,2)] };
    let mut rgba = vec![0; header.width as usize * header.height as usize * 4];
    let mut offset = 0;
    for &(start_x, start_y, step_x, step_y) in patterns {
        let width = (header.width as usize).saturating_sub(start_x).div_ceil(step_x);
        let height = (header.height as usize).saturating_sub(start_y).div_ceil(step_y);
        if width == 0 || height == 0 { continue; }
        let row_bytes = (width * channels * bits).div_ceil(8);
        let mut previous = vec![0u8; row_bytes];
        let mut row = vec![0u8; row_bytes];
        for y in 0..height {
            let filter = filtered[offset];
            offset += 1;
            row.copy_from_slice(&filtered[offset..offset + row_bytes]);
            offset += row_bytes;
            for i in 0..row_bytes {
                let left = if i >= bpp { row[i - bpp] } else { 0 };
                let up = previous[i];
                let upper_left = if i >= bpp { previous[i - bpp] } else { 0 };
                let predict = match filter {
                    0 => 0, 1 => left, 2 => up,
                    3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
                    4 => {
                        let p = i32::from(left) + i32::from(up) - i32::from(upper_left);
                        let (a,b,c) = ((p - i32::from(left)).abs(), (p - i32::from(up)).abs(), (p - i32::from(upper_left)).abs());
                        if a <= b && a <= c { left } else if b <= c { up } else { upper_left }
                    }
                    _ => return Err("EINVAL:invalid PNG filter".into()),
                };
                row[i] = row[i].wrapping_add(predict);
            }
            let sample = |index: usize| -> u16 {
                match bits {
                    16 => u16::from_be_bytes([row[index * 2], row[index * 2 + 1]]),
                    8 => u16::from(row[index]),
                    _ => u16::from((row[index * bits / 8] >> (8 - bits - index * bits % 8)) & ((1 << bits) - 1)),
                }
            };
            let byte = |value: u16| -> u8 {
                if bits == 16 { (value >> 8) as u8 } else { (u32::from(value) * 255 / ((1 << bits) - 1)) as u8 }
            };
            for x in 0..width {
                let i = x * channels;
                let pixel = match header.color_type {
                    0 => {
                        let value = sample(i);
                        let transparent = transparency.len() == 2 && value == u16::from_be_bytes(transparency.try_into().unwrap());
                        [byte(value), byte(value), byte(value), if transparent {0} else {255}]
                    }
                    2 => {
                        let rgb = [sample(i), sample(i+1), sample(i+2)];
                        let transparent = transparency.len() == 6 && rgb.iter().zip(transparency.chunks_exact(2)).all(|(value, pair)| *value == u16::from_be_bytes([pair[0], pair[1]]));
                        [byte(rgb[0]), byte(rgb[1]), byte(rgb[2]), if transparent {0} else {255}]
                    }
                    3 => {
                        let index = sample(i) as usize;
                        let color = palette.get(index*3..index*3+3).ok_or("EINVAL:PNG palette index out of range")?;
                        [color[0], color[1], color[2], transparency.get(index).copied().unwrap_or(255)]
                    }
                    4 => [byte(sample(i)), byte(sample(i)), byte(sample(i)), byte(sample(i+1))],
                    6 => [byte(sample(i)), byte(sample(i+1)), byte(sample(i+2)), byte(sample(i+3))],
                    _ => unreachable!(),
                };
                let target = ((start_y + y*step_y) * header.width as usize + start_x + x*step_x) * 4;
                rgba[target..target+4].copy_from_slice(&pixel);
            }
            std::mem::swap(&mut row, &mut previous);
        }
    }
    Ok(rgba)
}
