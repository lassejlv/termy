use super::{ParseOutput, Progress, Rgb};

pub(super) fn push_color_reply(
    output: &mut ParseOutput,
    prefix: &str,
    color: Rgb,
    terminator: &[u8],
) {
    output.replies.extend_from_slice(
        format!(
            "\x1b]{prefix};rgb:{0:02x}{0:02x}/{1:02x}{1:02x}/{2:02x}{2:02x}",
            color.r, color.g, color.b,
        )
        .as_bytes(),
    );
    output.replies.extend_from_slice(terminator);
}

pub(super) fn parse_x_color(value: &str) -> Option<Rgb> {
    if let Some(value) = value.strip_prefix('#') {
        let component_len = value.len() / 3;
        if component_len == 0 || component_len > 4 || component_len * 3 != value.len() {
            return None;
        }
        let component = |start: usize| {
            let digits = &value[start..start + component_len];
            let parsed = u32::from_str_radix(digits, 16).ok()?;
            let maximum = 16u32.pow(component_len as u32).checked_sub(1)?;
            Some((255 * parsed / maximum) as u8)
        };
        return Some(Rgb {
            r: component(0)?,
            g: component(component_len)?,
            b: component(component_len * 2)?,
        });
    }

    let value = value.strip_prefix("rgb:")?;
    let mut components = value.split('/');
    let scale = |digits: &str| {
        if digits.is_empty() || digits.len() > 4 {
            return None;
        }
        let maximum = 16u32.pow(digits.len() as u32).checked_sub(1)?;
        let parsed = u32::from_str_radix(digits, 16).ok()?;
        Some((255 * parsed / maximum) as u8)
    };
    let color = Rgb {
        r: scale(components.next()?)?,
        g: scale(components.next()?)?,
        b: scale(components.next()?)?,
    };
    components.next().is_none().then_some(color)
}

pub(super) fn dec_special_character(character: char) -> char {
    match character {
        '_' => ' ',
        '`' => '◆',
        'a' => '▒',
        'b' => '␉',
        'c' => '␌',
        'd' => '␍',
        'e' => '␊',
        'f' => '°',
        'g' => '±',
        'h' => '␤',
        'i' => '␋',
        'j' => '┘',
        'k' => '┐',
        'l' => '┌',
        'm' => '└',
        'n' => '┼',
        'o' => '⎺',
        'p' => '⎻',
        'q' => '─',
        'r' => '⎼',
        's' => '⎽',
        't' => '├',
        'u' => '┤',
        'v' => '┴',
        'w' => '┬',
        'x' => '│',
        'y' => '≤',
        'z' => '≥',
        '{' => 'π',
        '|' => '≠',
        '}' => '£',
        '~' => '·',
        _ => character,
    }
}

pub(super) fn progress_state(state: u8, progress: u8) -> Progress {
    let progress = progress.min(100);
    match state {
        1 => Progress::InProgress(progress),
        2 => Progress::Error(progress),
        3 => Progress::Indeterminate,
        4 => Progress::Warning(progress),
        _ => Progress::Clear,
    }
}

pub(super) fn osc7_path(value: &str) -> String {
    value
        .strip_prefix("file://")
        .and_then(|value| value.find('/').map(|index| &value[index..]))
        .unwrap_or(value)
        .to_string()
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub(super) fn decode_hex_ascii(input: &[u8]) -> Option<String> {
    if input.is_empty() || !input.len().is_multiple_of(2) {
        return None;
    }
    let mut decoded = Vec::with_capacity(input.len() / 2);
    for pair in input.as_chunks::<2>().0 {
        decoded.push((hex(pair[0])? << 4) | hex(pair[1])?);
    }
    String::from_utf8(decoded).ok()
}

pub(super) fn push_hex_ascii(output: &mut Vec<u8>, input: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    output.reserve(input.len().saturating_mul(2));
    for byte in input {
        output.push(HEX[usize::from(byte >> 4)]);
        output.push(HEX[usize::from(byte & 0x0f)]);
    }
}
