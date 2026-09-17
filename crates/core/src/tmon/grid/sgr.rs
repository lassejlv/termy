use super::Color;

pub(super) fn push_sgr_color(codes: &mut Vec<String>, color: Color, foreground: bool) {
    match color {
        Color::Default => {}
        Color::Indexed(index @ 0..=7) => {
            let base = if foreground { 30 } else { 40 };
            codes.push((base + u16::from(index)).to_string());
        }
        Color::Indexed(index @ 8..=15) => {
            let base = if foreground { 90 } else { 100 };
            codes.push((base + u16::from(index - 8)).to_string());
        }
        Color::Indexed(index) => {
            codes.push(format!("{};5;{index}", if foreground { 38 } else { 48 }));
        }
        Color::Rgb { r, g, b } => {
            codes.push(format!(
                "{};2;{r};{g};{b}",
                if foreground { 38 } else { 48 }
            ));
        }
    }
}

pub(super) fn push_underline_sgr_color(codes: &mut Vec<String>, color: Option<Color>) {
    match color {
        None | Some(Color::Default) => {}
        Some(Color::Indexed(index)) => codes.push(format!("58;5;{index}")),
        Some(Color::Rgb { r, g, b }) => codes.push(format!("58;2;{r};{g};{b}")),
    }
}

pub(super) fn extended_color(params: &[u16], index: &mut usize) -> Option<Color> {
    *index = index.saturating_add(1);
    match params.get(*index).copied()? {
        5 => {
            *index = index.saturating_add(1);
            Some(Color::Indexed(u8::try_from(*params.get(*index)?).ok()?))
        }
        2 => {
            *index = index.saturating_add(1);
            let r = u8::try_from(*params.get(*index)?).ok()?;
            *index = index.saturating_add(1);
            let g = u8::try_from(*params.get(*index)?).ok()?;
            *index = index.saturating_add(1);
            let b = u8::try_from(*params.get(*index)?).ok()?;
            Some(Color::Rgb { r, g, b })
        }
        _ => None,
    }
}
