use alacritty_terminal::{term::color::Colors, vte::ansi::NamedColor};

use crate::{TerminalColor, backend};

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalQueryColors {
    pub ansi: [TerminalColor; 16],
    pub foreground: TerminalColor,
    pub background: TerminalColor,
    // Retained as part of the query fallback contract, but OSC 12 replies still
    // only come from an explicit live cursor override in the terminal state.
    pub cursor: Option<TerminalColor>,
}

impl Default for TerminalQueryColors {
    fn default() -> Self {
        Self {
            ansi: [
                TerminalColor {
                    r: 0x00,
                    g: 0x00,
                    b: 0x00,
                },
                TerminalColor {
                    r: 0xcd,
                    g: 0x00,
                    b: 0x00,
                },
                TerminalColor {
                    r: 0x00,
                    g: 0xcd,
                    b: 0x00,
                },
                TerminalColor {
                    r: 0xcd,
                    g: 0xcd,
                    b: 0x00,
                },
                TerminalColor {
                    r: 0x00,
                    g: 0x00,
                    b: 0xee,
                },
                TerminalColor {
                    r: 0xcd,
                    g: 0x00,
                    b: 0xcd,
                },
                TerminalColor {
                    r: 0x00,
                    g: 0xcd,
                    b: 0xcd,
                },
                TerminalColor {
                    r: 0xe5,
                    g: 0xe5,
                    b: 0xe5,
                },
                TerminalColor {
                    r: 0x7f,
                    g: 0x7f,
                    b: 0x7f,
                },
                TerminalColor {
                    r: 0xff,
                    g: 0x00,
                    b: 0x00,
                },
                TerminalColor {
                    r: 0x00,
                    g: 0xff,
                    b: 0x00,
                },
                TerminalColor {
                    r: 0xff,
                    g: 0xff,
                    b: 0x00,
                },
                TerminalColor {
                    r: 0x5c,
                    g: 0x5c,
                    b: 0xff,
                },
                TerminalColor {
                    r: 0xff,
                    g: 0x00,
                    b: 0xff,
                },
                TerminalColor {
                    r: 0x00,
                    g: 0xff,
                    b: 0xff,
                },
                TerminalColor {
                    r: 0xff,
                    g: 0xff,
                    b: 0xff,
                },
            ],
            foreground: TerminalColor {
                r: 0xe5,
                g: 0xe5,
                b: 0xe5,
            },
            background: TerminalColor {
                r: 0x1e,
                g: 0x1e,
                b: 0x1e,
            },
            cursor: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueryColorSlot {
    Indexed(u8),
    Foreground,
    Background,
    Cursor,
    DimAnsi(u8),
    BrightForeground,
    DimForeground,
}

impl QueryColorSlot {
    fn from_index(index: usize) -> Option<Self> {
        match index {
            0..=255 => Some(Self::Indexed(index as u8)),
            value if value == NamedColor::Foreground as usize => Some(Self::Foreground),
            value if value == NamedColor::Background as usize => Some(Self::Background),
            value if value == NamedColor::Cursor as usize => Some(Self::Cursor),
            value if value == NamedColor::DimBlack as usize => Some(Self::DimAnsi(0)),
            value if value == NamedColor::DimRed as usize => Some(Self::DimAnsi(1)),
            value if value == NamedColor::DimGreen as usize => Some(Self::DimAnsi(2)),
            value if value == NamedColor::DimYellow as usize => Some(Self::DimAnsi(3)),
            value if value == NamedColor::DimBlue as usize => Some(Self::DimAnsi(4)),
            value if value == NamedColor::DimMagenta as usize => Some(Self::DimAnsi(5)),
            value if value == NamedColor::DimCyan as usize => Some(Self::DimAnsi(6)),
            value if value == NamedColor::DimWhite as usize => Some(Self::DimAnsi(7)),
            value if value == NamedColor::BrightForeground as usize => Some(Self::BrightForeground),
            value if value == NamedColor::DimForeground as usize => Some(Self::DimForeground),
            _ => None,
        }
    }

    fn live_color(self, colors: &Colors) -> Option<TerminalColor> {
        let index = match self {
            Self::Indexed(index) => index as usize,
            Self::Foreground => NamedColor::Foreground as usize,
            Self::Background => NamedColor::Background as usize,
            Self::Cursor => NamedColor::Cursor as usize,
            Self::DimAnsi(offset) => NamedColor::DimBlack as usize + offset as usize,
            Self::BrightForeground => NamedColor::BrightForeground as usize,
            Self::DimForeground => NamedColor::DimForeground as usize,
        };
        backend::live_color(colors, index)
    }
}

impl TerminalQueryColors {
    pub(crate) fn resolve_color(self, live_colors: &Colors, index: usize) -> Option<TerminalColor> {
        let slot = QueryColorSlot::from_index(index)?;

        slot.live_color(live_colors)
            .or_else(|| self.fallback_color(slot))
    }

    fn fallback_color(self, slot: QueryColorSlot) -> Option<TerminalColor> {
        match slot {
            QueryColorSlot::Indexed(idx) => Some(self.indexed_color(idx)),
            QueryColorSlot::Foreground
            | QueryColorSlot::BrightForeground
            | QueryColorSlot::DimForeground => Some(self.foreground),
            QueryColorSlot::Background => Some(self.background),
            // Upstream Alacritty only answers OSC 12 when the cursor color was explicitly
            // overridden by terminal state. The configured theme cursor color does not count,
            // so there is intentionally no fallback for cursor queries here.
            QueryColorSlot::Cursor => None,
            QueryColorSlot::DimAnsi(offset) => Some(self.ansi[offset as usize]),
        }
    }

    pub(crate) fn indexed_color(self, idx: u8) -> TerminalColor {
        match idx {
            0..=15 => self.ansi[idx as usize],
            16..=231 => {
                let idx = idx - 16;
                let r = (idx / 36) % 6;
                let g = (idx / 6) % 6;
                let b = idx % 6;
                let to_component = |value: u8| if value == 0 { 0 } else { 55 + (value * 40) };
                TerminalColor {
                    r: to_component(r),
                    g: to_component(g),
                    b: to_component(b),
                }
            }
            232..=255 => {
                let gray = 8 + ((idx - 232) * 10);
                TerminalColor {
                    r: gray,
                    g: gray,
                    b: gray,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TerminalQueryColors;
    use alacritty_terminal::{
        event::VoidListener,
        term::color,
        term::{Config as TermConfig, Term},
        vte::ansi::{self, NamedColor},
    };

    use crate::{TerminalColor, runtime::TerminalSize};
    fn test_terminal_size() -> TerminalSize {
        TerminalSize {
            cols: 32,
            rows: 4,
            cell_width: 9.0,
            cell_height: 18.0,
        }
    }

    fn term_colors_after_bytes(input: &[u8]) -> alacritty_terminal::term::color::Colors {
        let size = test_terminal_size();
        let mut term: Term<VoidListener> = Term::new(TermConfig::default(), &size, VoidListener);
        let mut parser: ansi::Processor = ansi::Processor::new();
        parser.advance(&mut term, input);
        *term.colors()
    }

    #[test]
    fn indexed_colors_fall_back_to_generated_palette() {
        let colors = TerminalQueryColors::default();
        let live = term_colors_after_bytes(b"");
        assert_eq!(
            colors.resolve_color(&live, 16),
            Some(TerminalColor {
                r: 0x00,
                g: 0x00,
                b: 0x00,
            })
        );
        assert_eq!(
            colors.resolve_color(&live, 232),
            Some(TerminalColor {
                r: 0x08,
                g: 0x08,
                b: 0x08,
            })
        );
    }

    #[test]
    fn live_foreground_override_wins_over_fallback() {
        let defaults = TerminalQueryColors {
            foreground: TerminalColor {
                r: 0xaa,
                g: 0xbb,
                b: 0xcc,
            },
            ..TerminalQueryColors::default()
        };
        let live = term_colors_after_bytes(b"\x1b]10;#123456\x07");
        assert_eq!(
            defaults.resolve_color(&live, NamedColor::Foreground as usize),
            Some(TerminalColor {
                r: 0x12,
                g: 0x34,
                b: 0x56,
            })
        );
    }

    #[test]
    fn reset_foreground_reverts_to_fallback() {
        let defaults = TerminalQueryColors {
            foreground: TerminalColor {
                r: 0x44,
                g: 0x55,
                b: 0x66,
            },
            ..TerminalQueryColors::default()
        };
        let live = term_colors_after_bytes(b"\x1b]10;#123456\x07\x1b]110\x07");
        assert_eq!(
            defaults.resolve_color(&live, NamedColor::Foreground as usize),
            Some(defaults.foreground)
        );
    }

    #[test]
    fn resolves_dim_and_bright_named_slots() {
        let defaults = TerminalQueryColors::default();
        let live = term_colors_after_bytes(b"");
        assert_eq!(
            defaults.resolve_color(&live, NamedColor::BrightForeground as usize),
            Some(defaults.foreground)
        );
        assert_eq!(
            defaults.resolve_color(&live, NamedColor::DimForeground as usize),
            Some(defaults.foreground)
        );
        assert_eq!(
            defaults.resolve_color(&live, NamedColor::DimBlue as usize),
            Some(defaults.ansi[4])
        );
    }

    #[test]
    fn cursor_queries_require_live_override() {
        let defaults = TerminalQueryColors {
            cursor: Some(TerminalColor {
                r: 0xab,
                g: 0xcd,
                b: 0xef,
            }),
            ..TerminalQueryColors::default()
        };
        let empty_live = term_colors_after_bytes(b"");
        assert_eq!(
            defaults.resolve_color(&empty_live, NamedColor::Cursor as usize),
            None
        );

        let live = term_colors_after_bytes(b"\x1b]12;#102030\x07");
        assert_eq!(
            defaults.resolve_color(&live, NamedColor::Cursor as usize),
            Some(TerminalColor {
                r: 0x10,
                g: 0x20,
                b: 0x30,
            })
        );
    }

    #[test]
    fn unsupported_index_returns_none() {
        let defaults = TerminalQueryColors::default();
        let live = term_colors_after_bytes(b"");
        assert_eq!(defaults.resolve_color(&live, color::COUNT), None);
    }
}
