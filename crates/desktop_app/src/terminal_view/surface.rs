use gpui::{Bounds, Hsla, Pixels, Rgba, Size, point, px, size};

/// An opaque TUI background can continue through the window chrome. Keep
/// translucent cells on the configured surface so their alpha is not applied twice.
pub(super) fn tui_surface_background(
    alternate_screen: bool,
    edge_background: Option<Hsla>,
    configured_background: Rgba,
) -> Rgba {
    edge_background
        .filter(|color| alternate_screen && color.a >= 1.0)
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
    fn opaque_tui_background_reaches_chrome_and_shell_restores_its_theme() {
        let theme = gpui::rgb(0x0b1020);
        let tui: Hsla = gpui::rgb(0xfff6ef).into();
        assert_eq!(tui_surface_background(true, Some(tui), theme), tui.into());
        assert_eq!(tui_surface_background(false, Some(tui), theme), theme);
        assert_eq!(tui_surface_background(true, None, theme), theme);
        assert_eq!(
            tui_surface_background(true, Some(Hsla { a: 0.5, ..tui }), theme),
            theme,
        );
    }

    #[test]
    fn fractional_edges_follow_adjacent_cells_without_gaps_or_overdraw() {
        let light: Hsla = gpui::rgb(0xfff6ef).into();
        let dark: Hsla = gpui::rgb(0x222222).into();
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
