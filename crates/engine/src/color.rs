//! Terminal color values and defaults.

use serde::{Deserialize, Serialize};

pub const DEFAULT_FOREGROUND_RGB: [u8; 3] = [216, 222, 233];
pub const DEFAULT_BACKGROUND_RGB: [u8; 3] = [11, 13, 15];
pub const DEFAULT_CURSOR_RGB: [u8; 3] = [192, 202, 245];

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum Color {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

impl Color {
    #[must_use]
    pub const fn rgb(red: u8, green: u8, blue: u8) -> Self {
        Self::Rgb(red, green, blue)
    }
}
