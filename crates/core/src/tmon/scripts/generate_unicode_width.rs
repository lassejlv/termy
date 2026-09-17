use std::{env, fmt::Write as _, fs, path::PathBuf};

const ROOT_LEN: usize = 256;
const MIDDLE_ROWS: usize = 17;
const MIDDLE_COLS: usize = 64;
const LEAF_ROWS: usize = 157;
const LEAF_COLS: usize = 32;

fn extract_hex_bytes(source: &str, marker: &str, count: usize) -> Vec<u8> {
    let tail = source
        .split_once(marker)
        .unwrap_or_else(|| panic!("missing marker {:?}", marker))
        .1
        .as_bytes();
    let mut values = Vec::with_capacity(count);
    let mut index = 0;
    while values.len() < count && index + 4 <= tail.len() {
        if tail[index] == b'0' && tail[index + 1] == b'x' {
            let digits = std::str::from_utf8(&tail[index + 2..index + 4]).unwrap();
            values.push(u8::from_str_radix(digits, 16).unwrap());
            index += 4;
        } else {
            index += 1;
        }
    }
    assert_eq!(values.len(), count, "incomplete data after {marker:?}");
    values
}

fn write_flat_array(output: &mut String, name: &str, alignment: usize, values: &[u8]) {
    writeln!(
        output,
        "static {name}: Align{alignment}<[u8; {}]> = Align{alignment}([",
        values.len()
    )
    .unwrap();
    for chunk in values.chunks(16) {
        output.push_str("    ");
        for value in chunk {
            write!(output, "0x{value:02x}, ").unwrap();
        }
        output.push('\n');
    }
    output.push_str("]);\n\n");
}

fn write_nested_array(
    output: &mut String,
    name: &str,
    alignment: usize,
    rows: usize,
    cols: usize,
    values: &[u8],
) {
    writeln!(
        output,
        "static {name}: Align{alignment}<[[u8; {cols}]; {rows}]> = Align{alignment}(["
    )
    .unwrap();
    for row in values.chunks_exact(cols) {
        output.push_str("    [\n");
        for chunk in row.chunks(16) {
            output.push_str("        ");
            for value in chunk {
                write!(output, "0x{value:02x}, ").unwrap();
            }
            output.push('\n');
        }
        output.push_str("    ],\n");
    }
    output.push_str("]);\n");
}

fn main() {
    let mut args = env::args_os().skip(1).map(PathBuf::from);
    let source_path = args
        .next()
        .expect("usage: generate_unicode_width SOURCE OUTPUT");
    let output_path = args
        .next()
        .expect("usage: generate_unicode_width SOURCE OUTPUT");
    assert!(args.next().is_none(), "too many arguments");

    let source = fs::read_to_string(source_path).unwrap();
    let root = extract_hex_bytes(&source, "static WIDTH_ROOT:", ROOT_LEN);
    let middle = extract_hex_bytes(&source, "static WIDTH_MIDDLE:", MIDDLE_ROWS * MIDDLE_COLS);
    let leaves = extract_hex_bytes(&source, "static WIDTH_LEAVES:", LEAF_ROWS * LEAF_COLS);

    let mut output = String::from(
        r#"// Generated from unicode-width 0.2.0's Unicode 15.1.0 scalar-width table.
// Copyright 2012-2022 The Rust Project Developers.
// Licensed under Apache-2.0 OR MIT. See UNICODE_WIDTH_LICENSE-MIT.
//
// This keeps tmon dependency-free while matching the width source used by
// alacritty_terminal 0.26.0. Regenerate with scripts/generate_unicode_width.rs.

#[repr(align(32))]
struct Align32<T>(T);

#[repr(align(64))]
struct Align64<T>(T);

#[repr(align(128))]
struct Align128<T>(T);

#[inline]
pub(crate) fn character_width(character: char) -> usize {
    if character < '\u{7f}' {
        return usize::from(character >= ' ');
    }
    if character < '\u{a0}' {
        return 0;
    }

    let code = character as usize;
    let root_offset = WIDTH_ROOT.0[code >> 13];
    let middle_offset = WIDTH_MIDDLE.0[usize::from(root_offset)][code >> 7 & 0x3f];
    let packed = WIDTH_LEAVES.0[usize::from(middle_offset)][code >> 2 & 0x1f];
    let width = packed >> (2 * (code & 0b11)) & 0b11;
    match width {
        0..=2 => usize::from(width),
        _ => special_width(character),
    }
}

#[inline]
fn special_width(character: char) -> usize {
    match character {
        '\u{05dc}'
        | '\u{0622}'..='\u{0882}'
        | '\u{1780}'..='\u{17af}'
        | '\u{1a10}'
        | '\u{2d31}'..='\u{2d6f}'
        | '\u{a4fc}'..='\u{a4fd}'
        | '\u{10c03}'
        | '\u{1f1e6}'..='\u{1f1ff}' => 1,
        '\u{fe0e}' | '\u{fe0f}' => 0,
        _ => 2,
    }
}

"#,
    );

    write_flat_array(&mut output, "WIDTH_ROOT", 128, &root);
    write_nested_array(
        &mut output,
        "WIDTH_MIDDLE",
        64,
        MIDDLE_ROWS,
        MIDDLE_COLS,
        &middle,
    );
    write_nested_array(
        &mut output,
        "WIDTH_LEAVES",
        32,
        LEAF_ROWS,
        LEAF_COLS,
        &leaves,
    );
    output.push_str(
        r#"

#[cfg(test)]
mod tests {
    use super::character_width;

    #[test]
    fn representative_scalar_widths_match_unicode_15_1() {
        for (character, width) in [
            ('a', 1),
            ('\u{0301}', 0),
            ('\u{093f}', 1),
            ('\u{200d}', 0),
            ('\u{feff}', 0),
            ('\u{1e944}', 0),
            ('界', 2),
            ('🙂', 2),
        ] {
            assert_eq!(character_width(character), width, "U+{:04X}", character as u32);
        }
    }
}
"#,
    );

    fs::write(output_path, output).unwrap();
}
