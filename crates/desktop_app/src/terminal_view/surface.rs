use gpui_kit::{Bounds, Hsla, Pixels, Rgba, Size, point, px, size};

/// Only a TUI that paints its own background (see `tui_paints_grid_border`)
/// may recolor the padding. Apps drawn on the terminal theme, like Claude Code
/// and Codex, otherwise tint it whenever a diff or dialog covers most cells.
pub(super) fn tui_paints_grid_border(
    grid: Size<usize>,
    painted: impl Fn(usize, usize) -> bool,
) -> bool {
    let last_row = grid.height.saturating_sub(1);
    let last_col = grid.width.saturating_sub(1);
    (0..grid.height).all(|row| {
        if row == 0 || row == last_row {
            (0..grid.width).all(|col| painted(row, col))
        } else {
            grid.width == 0 || (painted(row, 0) && painted(row, last_col))
        }
    })
}

/// Let an opaque viewport majority fill the TUI's padding. A tab, status bar, or
/// hovered corner must not recolor the entire surface. With no majority, retain
/// the configured background, including its transparency.
pub(super) fn tui_surface_background(
    tui_paints_border: bool,
    backgrounds: impl Iterator<Item = Hsla> + Clone,
    configured_background: Rgba,
) -> Rgba {
    if !tui_paints_border {
        return configured_background;
    }

    // Find a majority candidate without allocating a color histogram on each frame.
    let mut candidate = None;
    let mut votes = 0usize;
    let mut count = 0usize;
    for color in backgrounds.clone() {
        count += 1;
        if votes == 0 {
            candidate = Some(color);
            votes = 1;
        } else if candidate == Some(color) {
            votes += 1;
        } else {
            votes -= 1;
        }
    }

    candidate
        .filter(|color| {
            color.a >= 1.0
                && backgrounds.filter(|background| background == color).count() > count / 2
        })
        .map_or(configured_background, Into::into)
}

#[derive(Debug, PartialEq)]
pub(super) struct EdgeBackground {
    pub bounds: Bounds<Pixels>,
    pub color: Hsla,
}

/// Terminal dimensions are whole cells. Extend the adjacent cell backgrounds
/// over the fractional cell left at the right and bottom of the viewport.
pub(super) fn terminal_edge_backgrounds(
    grid: Size<usize>,
    cell: Size<Pixels>,
    surface: Size<Pixels>,
    background_at: impl Fn(usize, usize) -> Hsla,
) -> Vec<EdgeBackground> {
    if grid.width == 0 || grid.height == 0 || cell.width <= px(0.0) || cell.height <= px(0.0) {
        return Vec::new();
    }
    let grid_width = cell.width * grid.width as f32;
    let grid_height = cell.height * grid.height as f32;
    let right_width = (surface.width - grid_width).max(px(0.0));
    let bottom_height = (surface.height - grid_height).max(px(0.0));
    let mut fills = Vec::new();
    if right_width > px(0.0) {
        for row in 0..grid.height {
            let top = cell.height * row as f32;
            let height = cell.height.min(surface.height - top);
            if height > px(0.0) {
                fills.push(EdgeBackground {
                    bounds: Bounds::new(point(grid_width, top), size(right_width, height)),
                    color: background_at(row, grid.width - 1),
                });
            }
        }
    }
    if bottom_height > px(0.0) {
        for col in 0..grid.width {
            let left = cell.width * col as f32;
            let width = cell.width.min(surface.width - left);
            if width > px(0.0) {
                fills.push(EdgeBackground {
                    bounds: Bounds::new(point(left, grid_height), size(width, bottom_height)),
                    color: background_at(grid.height - 1, col),
                });
            }
        }
        if right_width > px(0.0) {
            fills.push(EdgeBackground {
                bounds: Bounds::new(
                    point(grid_width, grid_height),
                    size(right_width, bottom_height),
                ),
                color: background_at(grid.height - 1, grid.width - 1),
            });
        }
    }
    fills
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_tui_background_fills_terminal_padding_and_shell_restores_its_theme() {
        let theme = gpui_kit::rgb(0x0b1020);
        let tui: Hsla = gpui_kit::rgb(0xfff6ef).into();
        assert_eq!(
            tui_surface_background(true, [tui].into_iter(), theme),
            tui.into()
        );
        assert_eq!(
            tui_surface_background(false, [tui].into_iter(), theme),
            theme
        );
        assert_eq!(tui_surface_background(true, [].into_iter(), theme), theme);
        assert_eq!(
            tui_surface_background(true, [Hsla { a: 0.5, ..tui }].into_iter(), theme),
            theme,
        );
    }

    #[test]
    fn only_tuis_that_paint_the_whole_border_may_recolor_the_padding() {
        // An indented diff covering most of a Claude Code screen, above a
        // status line on the theme background.
        let mut painted = vec![vec![false; 10]; 6];
        for row in &mut painted[..4] {
            row[2..].fill(true);
        }
        let paints_border = |painted: &[Vec<bool>]| {
            tui_paints_grid_border(size(10, 6), |row, col| painted[row][col])
        };
        assert!(!paints_border(&painted));

        for row in &mut painted {
            row.fill(true);
        }
        assert!(paints_border(&painted));
        painted[3][9] = false;
        assert!(!paints_border(&painted));
        painted[3][9] = true;
        painted[3][5] = false;
        assert!(paints_border(&painted), "interior cells do not matter");

        assert!(tui_paints_grid_border(size(0, 0), |_, _| unreachable!()));
        assert!(tui_paints_grid_border(size(1, 1), |_, _| true));
        assert!(!tui_paints_grid_border(size(1, 1), |_, _| false));
    }

    #[test]
    fn hovering_tui_tabs_does_not_recolor_the_surrounding_background() {
        let theme = gpui_kit::rgb(0x0b1020);
        let body: Hsla = gpui_kit::rgb(0x1a1b26).into();
        let tab: Hsla = gpui_kit::rgb(0x202231).into();
        let hover: Hsla = gpui_kit::rgb(0x33364d).into();
        let mut cells = vec![vec![body; 8]; 4];
        cells[0].fill(tab);
        let background = |cells: &[Vec<Hsla>]| {
            tui_surface_background(true, cells.iter().flatten().copied(), theme)
        };
        let before_hover = background(&cells);
        cells[0][..4].fill(hover);
        assert_eq!(background(&cells), before_hover);
        cells[0].fill(tab);
        cells[0][4..].fill(hover);
        assert_eq!(background(&cells), before_hover);
        assert_eq!(before_hover, body.into());
    }

    #[test]
    fn tui_surface_requires_an_opaque_majority_and_tracks_theme_changes() {
        let theme = gpui_kit::rgb(0x0b1020);
        let dark: Hsla = gpui_kit::rgb(0x1a1b26).into();
        let light: Hsla = gpui_kit::rgb(0xfff6ef).into();
        let transparent = Hsla { a: 0.5, ..dark };

        for backgrounds in [
            [dark, light],
            [transparent, light],
            [transparent, transparent],
        ] {
            assert_eq!(
                tui_surface_background(true, backgrounds.into_iter(), theme),
                theme
            );
        }
        assert_eq!(
            tui_surface_background(true, [dark, dark, light].into_iter(), theme),
            dark.into()
        );
        assert_eq!(
            tui_surface_background(true, [dark, light, light].into_iter(), theme),
            light.into()
        );
    }

    #[test]
    fn fractional_edges_follow_adjacent_cells_without_gaps_or_overdraw() {
        let light: Hsla = gpui_kit::rgb(0xfff6ef).into();
        let dark: Hsla = gpui_kit::rgb(0x222222).into();
        let colors = [[light, dark], [dark, light]];
        let fills = terminal_edge_backgrounds(
            size(2, 2),
            size(px(8.0), px(20.0)),
            size(px(19.0), px(45.0)),
            |row, col| colors[row][col],
        );
        assert_eq!(
            fills,
            vec![
                EdgeBackground {
                    bounds: Bounds::new(point(px(16.0), px(0.0)), size(px(3.0), px(20.0))),
                    color: dark
                },
                EdgeBackground {
                    bounds: Bounds::new(point(px(16.0), px(20.0)), size(px(3.0), px(20.0))),
                    color: light
                },
                EdgeBackground {
                    bounds: Bounds::new(point(px(0.0), px(40.0)), size(px(8.0), px(5.0))),
                    color: dark
                },
                EdgeBackground {
                    bounds: Bounds::new(point(px(8.0), px(40.0)), size(px(8.0), px(5.0))),
                    color: light
                },
                EdgeBackground {
                    bounds: Bounds::new(point(px(16.0), px(40.0)), size(px(3.0), px(5.0))),
                    color: light
                },
            ]
        );
        let area: f32 = fills
            .iter()
            .map(|fill| f32::from(fill.bounds.size.width) * f32::from(fill.bounds.size.height))
            .sum();
        assert_eq!(area, 19.0 * 45.0 - 16.0 * 40.0);
    }

    #[test]
    fn no_edge_fill_when_the_grid_fills_or_exceeds_the_surface() {
        for surface in [size(px(16.0), px(40.0)), size(px(10.0), px(30.0))] {
            assert!(
                terminal_edge_backgrounds(
                    size(2, 2),
                    size(px(8.0), px(20.0)),
                    surface,
                    |_, _| unreachable!()
                )
                .is_empty()
            );
        }
        assert!(
            terminal_edge_backgrounds(
                size(0, 0),
                size(px(8.0), px(20.0)),
                size(px(19.0), px(45.0)),
                |_, _| unreachable!()
            )
            .is_empty()
        );
    }
}
