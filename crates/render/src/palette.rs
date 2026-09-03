use engine::{
    Cell, CellFlags, Color, DEFAULT_BACKGROUND_RGB, DEFAULT_CURSOR_RGB, DEFAULT_FOREGROUND_RGB,
    DynamicColor,
};

const DEFAULT_ANSI: [[u8; 3]; 16] = [
    [46, 52, 64],
    [191, 97, 106],
    [163, 190, 140],
    [235, 203, 139],
    [129, 161, 193],
    [180, 142, 173],
    [136, 192, 208],
    DEFAULT_FOREGROUND_RGB,
    [76, 86, 106],
    [191, 97, 106],
    [163, 190, 140],
    [235, 203, 139],
    [129, 161, 193],
    [180, 142, 173],
    [143, 188, 187],
    [236, 239, 244],
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Theme {
    pub foreground: [u8; 3],
    pub background: [u8; 3],
    pub cursor: [u8; 3],
    pub selection: [u8; 3],
    pub search_background: [u8; 3],
    pub search_foreground: [u8; 3],
    pub search_border: [u8; 3],
    pub search_no_match: [u8; 3],
    pub ansi: [[u8; 3]; 16],
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            foreground: DEFAULT_FOREGROUND_RGB,
            background: DEFAULT_BACKGROUND_RGB,
            cursor: DEFAULT_CURSOR_RGB,
            selection: [76, 110, 175],
            search_background: [30, 34, 42],
            search_foreground: [236, 239, 244],
            search_border: [76, 86, 106],
            search_no_match: [191, 97, 106],
            ansi: DEFAULT_ANSI,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Palette {
    theme: Theme,
    foreground: [u8; 3],
    background: [u8; 3],
    cursor: [u8; 3],
}

impl Default for Palette {
    fn default() -> Self {
        Self::new(Theme::default())
    }
}

impl Palette {
    pub(crate) const fn new(theme: Theme) -> Self {
        Self {
            foreground: theme.foreground,
            background: theme.background,
            cursor: theme.cursor,
            theme,
        }
    }

    pub(crate) const fn foreground(self) -> [u8; 3] {
        self.foreground
    }

    pub(crate) const fn background(self) -> [u8; 3] {
        self.background
    }

    pub(crate) const fn cursor(self) -> [u8; 3] {
        self.cursor
    }

    pub(crate) const fn selection(self) -> [u8; 3] {
        self.theme.selection
    }

    pub(crate) const fn search_background(self) -> [u8; 3] {
        self.theme.search_background
    }

    pub(crate) const fn search_foreground(self) -> [u8; 3] {
        self.theme.search_foreground
    }

    pub(crate) const fn search_border(self) -> [u8; 3] {
        self.theme.search_border
    }

    pub(crate) const fn search_no_match(self) -> [u8; 3] {
        self.theme.search_no_match
    }

    pub(crate) fn set(&mut self, target: DynamicColor, color: [u8; 3]) -> bool {
        let slot = self.slot_mut(target);
        if *slot == color {
            return false;
        }
        *slot = color;
        true
    }

    pub(crate) fn reset(&mut self, target: DynamicColor) -> bool {
        self.set(target, self.theme_color(target))
    }

    pub(crate) fn resolved_colors(self, cell: &Cell) -> ([u8; 3], [u8; 3]) {
        let mut foreground = self.resolve(cell.foreground, self.foreground);
        let mut background = self.resolve(cell.background, self.background);
        if cell.flags.contains(CellFlags::INVERSE) {
            std::mem::swap(&mut foreground, &mut background);
        }
        if cell.flags.contains(CellFlags::HIDDEN) {
            foreground = background;
        } else if cell.flags.contains(CellFlags::DIM) {
            foreground = [
                foreground[0].midpoint(background[0]),
                foreground[1].midpoint(background[1]),
                foreground[2].midpoint(background[2]),
            ];
        }
        (foreground, background)
    }

    fn slot_mut(&mut self, target: DynamicColor) -> &mut [u8; 3] {
        match target {
            DynamicColor::Foreground => &mut self.foreground,
            DynamicColor::Background => &mut self.background,
            DynamicColor::Cursor => &mut self.cursor,
        }
    }

    const fn theme_color(self, target: DynamicColor) -> [u8; 3] {
        match target {
            DynamicColor::Foreground => self.theme.foreground,
            DynamicColor::Background => self.theme.background,
            DynamicColor::Cursor => self.theme.cursor,
        }
    }

    fn resolve(self, color: Color, default: [u8; 3]) -> [u8; 3] {
        match color {
            Color::Default => default,
            Color::Rgb(red, green, blue) => [red, green, blue],
            Color::Indexed(index @ 0..=15) => self.theme.ansi[usize::from(index)],
            Color::Indexed(index @ 16..=231) => {
                let value = index - 16;
                let red = value / 36;
                let green = (value % 36) / 6;
                let blue = value % 6;
                [cube(red), cube(green), cube(blue)]
            }
            Color::Indexed(index) => {
                let gray = 8_u8.saturating_add((index - 232).saturating_mul(10));
                [gray, gray, gray]
            }
        }
    }
}

pub(crate) fn srgb_to_linear(rgb: [u8; 3]) -> [f32; 3] {
    rgb.map(|channel| {
        let channel = f32::from(channel) / 255.0;
        if channel <= 0.040_45 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    })
}

fn cube(value: u8) -> u8 {
    if value == 0 { 0 } else { 55 + value * 40 }
}

#[cfg(test)]
mod tests {
    use engine::{
        Cell, CellFlags, Color, DEFAULT_BACKGROUND_RGB, DEFAULT_CURSOR_RGB, DEFAULT_FOREGROUND_RGB,
        DynamicColor,
    };

    use super::{Palette, Theme};

    #[test]
    fn resolves_default_indexed_cube_and_grayscale_colors() {
        let palette = Palette::default();
        let mut cell = Cell::default();
        assert_eq!(palette.resolved_colors(&cell).1, DEFAULT_BACKGROUND_RGB);
        cell.foreground = Color::Indexed(9);
        assert_eq!(palette.resolved_colors(&cell).0, [191, 97, 106]);
        cell.foreground = Color::Indexed(16);
        assert_eq!(palette.resolved_colors(&cell).0, [0, 0, 0]);
        cell.foreground = Color::Indexed(231);
        assert_eq!(palette.resolved_colors(&cell).0, [255, 255, 255]);
        cell.foreground = Color::Indexed(232);
        assert_eq!(palette.resolved_colors(&cell).0, [8, 8, 8]);
    }

    #[test]
    fn dynamic_defaults_update_and_reset_without_changing_indexed_colors() {
        let mut palette = Palette::default();
        assert_eq!(palette.foreground(), DEFAULT_FOREGROUND_RGB);
        assert_eq!(palette.background(), DEFAULT_BACKGROUND_RGB);
        assert_eq!(palette.cursor(), DEFAULT_CURSOR_RGB);

        assert!(palette.set(DynamicColor::Foreground, [1, 2, 3]));
        assert!(palette.set(DynamicColor::Background, [4, 5, 6]));
        assert!(palette.set(DynamicColor::Cursor, [7, 8, 9]));
        assert!(!palette.set(DynamicColor::Cursor, [7, 8, 9]));

        let mut cell = Cell::default();
        assert_eq!(palette.resolved_colors(&cell), ([1, 2, 3], [4, 5, 6]));
        cell.foreground = Color::Indexed(1);
        assert_eq!(palette.resolved_colors(&cell).0, [191, 97, 106]);

        assert!(palette.reset(DynamicColor::Foreground));
        assert!(palette.reset(DynamicColor::Background));
        assert!(palette.reset(DynamicColor::Cursor));
        assert_eq!(palette, Palette::default());
    }

    #[test]
    fn background_updates_feed_inverse_dim_and_hidden_foregrounds() {
        let mut palette = Palette::default();
        palette.set(DynamicColor::Foreground, [100, 120, 140]);
        palette.set(DynamicColor::Background, [20, 40, 60]);

        let mut cell = Cell {
            flags: CellFlags::INVERSE,
            ..Cell::default()
        };
        assert_eq!(
            palette.resolved_colors(&cell),
            ([20, 40, 60], [100, 120, 140])
        );

        cell.flags = CellFlags::DIM;
        assert_eq!(palette.resolved_colors(&cell).0, [60, 80, 100]);

        cell.flags = CellFlags::HIDDEN;
        assert_eq!(palette.resolved_colors(&cell).0, [20, 40, 60]);
    }

    #[test]
    fn custom_theme_controls_defaults_ansi_and_dynamic_resets() {
        let mut theme = Theme {
            foreground: [1, 2, 3],
            background: [4, 5, 6],
            cursor: [7, 8, 9],
            ..Theme::default()
        };
        theme.ansi[1] = [10, 20, 30];
        let mut palette = Palette::new(theme);
        let mut cell = Cell::default();
        assert_eq!(palette.resolved_colors(&cell), ([1, 2, 3], [4, 5, 6]));
        cell.foreground = Color::Indexed(1);
        assert_eq!(palette.resolved_colors(&cell).0, [10, 20, 30]);

        palette.set(DynamicColor::Foreground, [90, 91, 92]);
        assert!(palette.reset(DynamicColor::Foreground));
        assert_eq!(palette.foreground(), [1, 2, 3]);
    }
}
