use crate::config_core::types::{CustomColors, Rgb8};
use crate::config_core::{ColorSettingId, color_setting_from_key, color_setting_spec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorEntryError {
    UnknownKey,
    InvalidValue,
}

#[derive(Clone, Copy)]
enum ColorSlot {
    Foreground,
    Background,
    Cursor,
    Ansi(usize),
}

pub fn canonical_color_key(key: &str) -> Option<&'static str> {
    let id = color_setting_from_key(key)?;
    Some(color_setting_spec(id).key)
}

pub fn apply_color_entry(
    colors: &mut CustomColors,
    key: &str,
    value: &str,
) -> Result<(), ColorEntryError> {
    let slot = color_slot(key).ok_or(ColorEntryError::UnknownKey)?;
    let color = Rgb8::from_hex(value).ok_or(ColorEntryError::InvalidValue)?;

    match slot {
        ColorSlot::Foreground => colors.foreground = Some(color),
        ColorSlot::Background => colors.background = Some(color),
        ColorSlot::Cursor => colors.cursor = Some(color),
        ColorSlot::Ansi(index) => colors.ansi[index] = Some(color),
    }

    Ok(())
}

fn color_slot(key: &str) -> Option<ColorSlot> {
    let id = color_setting_from_key(key)?;
    match id {
        ColorSettingId::Foreground => Some(ColorSlot::Foreground),
        ColorSettingId::Background => Some(ColorSlot::Background),
        ColorSettingId::Cursor => Some(ColorSlot::Cursor),
        ColorSettingId::Black => Some(ColorSlot::Ansi(0)),
        ColorSettingId::Red => Some(ColorSlot::Ansi(1)),
        ColorSettingId::Green => Some(ColorSlot::Ansi(2)),
        ColorSettingId::Yellow => Some(ColorSlot::Ansi(3)),
        ColorSettingId::Blue => Some(ColorSlot::Ansi(4)),
        ColorSettingId::Magenta => Some(ColorSlot::Ansi(5)),
        ColorSettingId::Cyan => Some(ColorSlot::Ansi(6)),
        ColorSettingId::White => Some(ColorSlot::Ansi(7)),
        ColorSettingId::BrightBlack => Some(ColorSlot::Ansi(8)),
        ColorSettingId::BrightRed => Some(ColorSlot::Ansi(9)),
        ColorSettingId::BrightGreen => Some(ColorSlot::Ansi(10)),
        ColorSettingId::BrightYellow => Some(ColorSlot::Ansi(11)),
        ColorSettingId::BrightBlue => Some(ColorSlot::Ansi(12)),
        ColorSettingId::BrightMagenta => Some(ColorSlot::Ansi(13)),
        ColorSettingId::BrightCyan => Some(ColorSlot::Ansi(14)),
        ColorSettingId::BrightWhite => Some(ColorSlot::Ansi(15)),
    }
}
