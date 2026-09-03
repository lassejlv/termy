#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LineStyle {
    None,
    Light,
    Heavy,
    Double,
}

impl LineStyle {
    const fn is_double(self) -> bool {
        matches!(self, Self::Double)
    }

    const fn is_heavy(self) -> bool {
        matches!(self, Self::Heavy)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BoxSegments {
    up: LineStyle,
    down: LineStyle,
    left: LineStyle,
    right: LineStyle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BoxDrawing {
    Rectilinear(BoxSegments),
    Rounded(RoundedCorner),
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RoundedCorner {
    UpperLeft,
    UpperRight,
    LowerRight,
    LowerLeft,
}

impl RoundedCorner {
    pub(crate) const fn shader_orientation(self) -> f32 {
        self as u32 as f32
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct BoxRect {
    pub(crate) left: f32,
    pub(crate) top: f32,
    pub(crate) right: f32,
    pub(crate) bottom: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BoxMetrics {
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) font_size: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct BoxRectangles {
    rectangles: [BoxRect; 8],
    len: usize,
}

impl BoxRectangles {
    pub(crate) fn as_slice(&self) -> &[BoxRect] {
        &self.rectangles[..self.len]
    }

    fn push(&mut self, rectangle: BoxRect) {
        if self.len < self.rectangles.len() {
            self.rectangles[self.len] = rectangle;
            self.len += 1;
        }
    }

    fn merge_collinear(&mut self) {
        let same = |left: f32, right: f32| (left - right).abs() < f32::EPSILON;
        let mut first = 0;
        while first < self.len {
            let mut second = first + 1;
            while second < self.len {
                let a = self.rectangles[first];
                let b = self.rectangles[second];
                let horizontal = same(a.top, b.top)
                    && same(a.bottom, b.bottom)
                    && a.left <= b.right
                    && b.left <= a.right;
                let vertical = same(a.left, b.left)
                    && same(a.right, b.right)
                    && a.top <= b.bottom
                    && b.top <= a.bottom;
                if horizontal || vertical {
                    self.rectangles[first] = BoxRect {
                        left: a.left.min(b.left),
                        top: a.top.min(b.top),
                        right: a.right.max(b.right),
                        bottom: a.bottom.max(b.bottom),
                    };
                    self.len -= 1;
                    self.rectangles[second] = self.rectangles[self.len];
                } else {
                    second += 1;
                }
            }
            first += 1;
        }
    }
}

const fn segments(up: LineStyle, down: LineStyle, left: LineStyle, right: LineStyle) -> BoxDrawing {
    BoxDrawing::Rectilinear(BoxSegments {
        up,
        down,
        left,
        right,
    })
}

/// Returns retained geometry for box-drawing characters. Rectilinear joins use pixel-aligned
/// rectangles, while rounded corners use an analytic GPU stroke. Diagonals retain font fallback.
#[allow(clippy::too_many_lines)]
pub(crate) fn box_drawing(character: char) -> Option<BoxDrawing> {
    use LineStyle::{Double, Heavy, Light, None as Empty};

    Some(match character {
        '\u{2500}' | '\u{2504}' | '\u{2508}' | '\u{254C}' => segments(Empty, Empty, Light, Light),
        '\u{2501}' | '\u{2505}' | '\u{2509}' | '\u{254D}' => segments(Empty, Empty, Heavy, Heavy),
        '\u{2502}' | '\u{2506}' | '\u{250A}' | '\u{254E}' => segments(Light, Light, Empty, Empty),
        '\u{2503}' | '\u{2507}' | '\u{250B}' | '\u{254F}' => segments(Heavy, Heavy, Empty, Empty),
        '\u{250C}' => segments(Empty, Light, Empty, Light),
        '\u{250D}' => segments(Empty, Light, Empty, Heavy),
        '\u{250E}' => segments(Empty, Heavy, Empty, Light),
        '\u{250F}' => segments(Empty, Heavy, Empty, Heavy),
        '\u{2510}' => segments(Empty, Light, Light, Empty),
        '\u{2511}' => segments(Empty, Light, Heavy, Empty),
        '\u{2512}' => segments(Empty, Heavy, Light, Empty),
        '\u{2513}' => segments(Empty, Heavy, Heavy, Empty),
        '\u{2514}' => segments(Light, Empty, Empty, Light),
        '\u{2515}' => segments(Light, Empty, Empty, Heavy),
        '\u{2516}' => segments(Heavy, Empty, Empty, Light),
        '\u{2517}' => segments(Heavy, Empty, Empty, Heavy),
        '\u{2518}' => segments(Light, Empty, Light, Empty),
        '\u{2519}' => segments(Light, Empty, Heavy, Empty),
        '\u{251A}' => segments(Heavy, Empty, Light, Empty),
        '\u{251B}' => segments(Heavy, Empty, Heavy, Empty),
        '\u{251C}' => segments(Light, Light, Empty, Light),
        '\u{251D}' => segments(Light, Light, Empty, Heavy),
        '\u{251E}' => segments(Heavy, Light, Empty, Light),
        '\u{251F}' => segments(Light, Heavy, Empty, Light),
        '\u{2520}' => segments(Heavy, Heavy, Empty, Light),
        '\u{2521}' => segments(Light, Heavy, Empty, Heavy),
        '\u{2522}' => segments(Heavy, Light, Empty, Heavy),
        '\u{2523}' => segments(Heavy, Heavy, Empty, Heavy),
        '\u{2524}' => segments(Light, Light, Light, Empty),
        '\u{2525}' => segments(Light, Light, Heavy, Empty),
        '\u{2526}' => segments(Heavy, Light, Light, Empty),
        '\u{2527}' => segments(Light, Heavy, Light, Empty),
        '\u{2528}' => segments(Heavy, Heavy, Light, Empty),
        '\u{2529}' => segments(Light, Heavy, Heavy, Empty),
        '\u{252A}' => segments(Heavy, Light, Heavy, Empty),
        '\u{252B}' => segments(Heavy, Heavy, Heavy, Empty),
        '\u{252C}' => segments(Empty, Light, Light, Light),
        '\u{252D}' => segments(Empty, Light, Heavy, Light),
        '\u{252E}' => segments(Empty, Light, Light, Heavy),
        '\u{252F}' => segments(Empty, Light, Heavy, Heavy),
        '\u{2530}' => segments(Empty, Heavy, Light, Light),
        '\u{2531}' => segments(Empty, Heavy, Heavy, Light),
        '\u{2532}' => segments(Empty, Heavy, Light, Heavy),
        '\u{2533}' => segments(Empty, Heavy, Heavy, Heavy),
        '\u{2534}' => segments(Light, Empty, Light, Light),
        '\u{2535}' => segments(Light, Empty, Heavy, Light),
        '\u{2536}' => segments(Light, Empty, Light, Heavy),
        '\u{2537}' => segments(Light, Empty, Heavy, Heavy),
        '\u{2538}' => segments(Heavy, Empty, Light, Light),
        '\u{2539}' => segments(Heavy, Empty, Heavy, Light),
        '\u{253A}' => segments(Heavy, Empty, Light, Heavy),
        '\u{253B}' => segments(Heavy, Empty, Heavy, Heavy),
        '\u{253C}' => segments(Light, Light, Light, Light),
        '\u{253D}' => segments(Light, Light, Heavy, Light),
        '\u{253E}' => segments(Light, Light, Light, Heavy),
        '\u{253F}' => segments(Light, Light, Heavy, Heavy),
        '\u{2540}' => segments(Heavy, Light, Light, Light),
        '\u{2541}' => segments(Light, Heavy, Light, Light),
        '\u{2542}' => segments(Heavy, Heavy, Light, Light),
        '\u{2543}' => segments(Heavy, Light, Heavy, Light),
        '\u{2544}' => segments(Heavy, Light, Light, Heavy),
        '\u{2545}' => segments(Light, Heavy, Heavy, Light),
        '\u{2546}' => segments(Light, Heavy, Light, Heavy),
        '\u{2547}' => segments(Light, Heavy, Heavy, Heavy),
        '\u{2548}' => segments(Heavy, Light, Heavy, Heavy),
        '\u{2549}' => segments(Heavy, Heavy, Heavy, Light),
        '\u{254A}' => segments(Heavy, Heavy, Light, Heavy),
        '\u{254B}' => segments(Heavy, Heavy, Heavy, Heavy),
        '\u{2550}' => segments(Empty, Empty, Double, Double),
        '\u{2551}' => segments(Double, Double, Empty, Empty),
        '\u{2552}' => segments(Empty, Light, Empty, Double),
        '\u{2553}' => segments(Empty, Double, Empty, Light),
        '\u{2554}' => segments(Empty, Double, Empty, Double),
        '\u{2555}' => segments(Empty, Light, Double, Empty),
        '\u{2556}' => segments(Empty, Double, Light, Empty),
        '\u{2557}' => segments(Empty, Double, Double, Empty),
        '\u{2558}' => segments(Light, Empty, Empty, Double),
        '\u{2559}' => segments(Double, Empty, Empty, Light),
        '\u{255A}' => segments(Double, Empty, Empty, Double),
        '\u{255B}' => segments(Light, Empty, Double, Empty),
        '\u{255C}' => segments(Double, Empty, Light, Empty),
        '\u{255D}' => segments(Double, Empty, Double, Empty),
        '\u{255E}' => segments(Light, Light, Empty, Double),
        '\u{255F}' => segments(Double, Double, Empty, Light),
        '\u{2560}' => segments(Double, Double, Empty, Double),
        '\u{2561}' => segments(Light, Light, Double, Empty),
        '\u{2562}' => segments(Double, Double, Light, Empty),
        '\u{2563}' => segments(Double, Double, Double, Empty),
        '\u{2564}' => segments(Empty, Light, Double, Double),
        '\u{2565}' => segments(Empty, Double, Light, Light),
        '\u{2566}' => segments(Empty, Double, Double, Double),
        '\u{2567}' => segments(Light, Empty, Double, Double),
        '\u{2568}' => segments(Double, Empty, Light, Light),
        '\u{2569}' => segments(Double, Empty, Double, Double),
        '\u{256A}' => segments(Light, Light, Double, Double),
        '\u{256B}' => segments(Double, Double, Light, Light),
        '\u{256C}' => segments(Double, Double, Double, Double),
        '\u{256D}' => BoxDrawing::Rounded(RoundedCorner::UpperLeft),
        '\u{256E}' => BoxDrawing::Rounded(RoundedCorner::UpperRight),
        '\u{256F}' => BoxDrawing::Rounded(RoundedCorner::LowerRight),
        '\u{2570}' => BoxDrawing::Rounded(RoundedCorner::LowerLeft),
        '\u{2574}' => segments(Empty, Empty, Light, Empty),
        '\u{2575}' => segments(Light, Empty, Empty, Empty),
        '\u{2576}' => segments(Empty, Empty, Empty, Light),
        '\u{2577}' => segments(Empty, Light, Empty, Empty),
        '\u{2578}' => segments(Empty, Empty, Heavy, Empty),
        '\u{2579}' => segments(Heavy, Empty, Empty, Empty),
        '\u{257A}' => segments(Empty, Empty, Empty, Heavy),
        '\u{257B}' => segments(Empty, Heavy, Empty, Empty),
        '\u{257C}' => segments(Empty, Empty, Light, Heavy),
        '\u{257D}' => segments(Light, Heavy, Empty, Empty),
        '\u{257E}' => segments(Empty, Empty, Heavy, Light),
        '\u{257F}' => segments(Heavy, Light, Empty, Empty),
        _ => return None,
    })
}

fn push_rect(
    rectangles: &mut BoxRectangles,
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
    metrics: BoxMetrics,
) {
    let rectangle = BoxRect {
        left: left.clamp(0.0, metrics.width),
        top: top.clamp(0.0, metrics.height),
        right: right.clamp(0.0, metrics.width),
        bottom: bottom.clamp(0.0, metrics.height),
    };
    if rectangle.right > rectangle.left && rectangle.bottom > rectangle.top {
        rectangles.push(rectangle);
    }
}

pub(crate) fn box_rectangles(shape: BoxDrawing, metrics: BoxMetrics) -> BoxRectangles {
    use LineStyle::{Double, Heavy, Light, None as Empty};

    let BoxDrawing::Rectilinear(shape) = shape else {
        return BoxRectangles::default();
    };

    let light = (metrics.font_size * 0.0675).ceil().max(1.0);
    let heavy = light * 2.0;
    let centered_bounds = |size: f32, stroke: f32| {
        let start = ((size - stroke).max(0.0) / 2.0).round();
        (start, (start + stroke).min(size))
    };
    let (h_light_top, h_light_bottom) = centered_bounds(metrics.height, light);
    let (h_heavy_top, h_heavy_bottom) = centered_bounds(metrics.height, heavy);
    let h_double_top = (h_light_top - light).max(0.0);
    let h_double_bottom = (h_light_bottom + light).min(metrics.height);
    let (v_light_left, v_light_right) = centered_bounds(metrics.width, light);
    let (v_heavy_left, v_heavy_right) = centered_bounds(metrics.width, heavy);
    let v_double_left = (v_light_left - light).max(0.0);
    let v_double_right = (v_light_right + light).min(metrics.width);

    let up_bottom = if shape.left.is_heavy() || shape.right.is_heavy() {
        h_heavy_bottom
    } else if shape.left != shape.right || shape.down == shape.up {
        if shape.left.is_double() || shape.right.is_double() {
            h_double_bottom
        } else {
            h_light_bottom
        }
    } else if shape.left == Empty && shape.right == Empty {
        h_light_bottom
    } else {
        h_light_top
    };
    let down_top = if shape.left.is_heavy() || shape.right.is_heavy() {
        h_heavy_top
    } else if shape.left != shape.right || shape.up == shape.down {
        if shape.left.is_double() || shape.right.is_double() {
            h_double_top
        } else {
            h_light_top
        }
    } else if shape.left == Empty && shape.right == Empty {
        h_light_top
    } else {
        h_light_bottom
    };
    let left_right = if shape.up.is_heavy() || shape.down.is_heavy() {
        v_heavy_right
    } else if shape.up != shape.down || shape.left == shape.right {
        if shape.up.is_double() || shape.down.is_double() {
            v_double_right
        } else {
            v_light_right
        }
    } else if shape.up == Empty && shape.down == Empty {
        v_light_right
    } else {
        v_light_left
    };
    let right_left = if shape.up.is_heavy() || shape.down.is_heavy() {
        v_heavy_left
    } else if shape.up != shape.down || shape.right == shape.left {
        if shape.up.is_double() || shape.down.is_double() {
            v_double_left
        } else {
            v_light_left
        }
    } else if shape.up == Empty && shape.down == Empty {
        v_light_left
    } else {
        v_light_right
    };

    let mut rectangles = BoxRectangles::default();
    match shape.up {
        Empty => {}
        Light => push_rect(
            &mut rectangles,
            v_light_left,
            0.0,
            v_light_right,
            up_bottom,
            metrics,
        ),
        Heavy => push_rect(
            &mut rectangles,
            v_heavy_left,
            0.0,
            v_heavy_right,
            up_bottom,
            metrics,
        ),
        Double => {
            let left_bottom = if shape.left == Double {
                h_light_top
            } else {
                up_bottom
            };
            let right_bottom = if shape.right == Double {
                h_light_top
            } else {
                up_bottom
            };
            push_rect(
                &mut rectangles,
                v_double_left,
                0.0,
                v_light_left,
                left_bottom,
                metrics,
            );
            push_rect(
                &mut rectangles,
                v_light_right,
                0.0,
                v_double_right,
                right_bottom,
                metrics,
            );
        }
    }
    match shape.right {
        Empty => {}
        Light => push_rect(
            &mut rectangles,
            right_left,
            h_light_top,
            metrics.width,
            h_light_bottom,
            metrics,
        ),
        Heavy => push_rect(
            &mut rectangles,
            right_left,
            h_heavy_top,
            metrics.width,
            h_heavy_bottom,
            metrics,
        ),
        Double => {
            let top_left = if shape.up == Double {
                v_light_right
            } else {
                right_left
            };
            let bottom_left = if shape.down == Double {
                v_light_right
            } else {
                right_left
            };
            push_rect(
                &mut rectangles,
                top_left,
                h_double_top,
                metrics.width,
                h_light_top,
                metrics,
            );
            push_rect(
                &mut rectangles,
                bottom_left,
                h_light_bottom,
                metrics.width,
                h_double_bottom,
                metrics,
            );
        }
    }
    match shape.down {
        Empty => {}
        Light => push_rect(
            &mut rectangles,
            v_light_left,
            down_top,
            v_light_right,
            metrics.height,
            metrics,
        ),
        Heavy => push_rect(
            &mut rectangles,
            v_heavy_left,
            down_top,
            v_heavy_right,
            metrics.height,
            metrics,
        ),
        Double => {
            let left_top = if shape.left == Double {
                h_light_bottom
            } else {
                down_top
            };
            let right_top = if shape.right == Double {
                h_light_bottom
            } else {
                down_top
            };
            push_rect(
                &mut rectangles,
                v_double_left,
                left_top,
                v_light_left,
                metrics.height,
                metrics,
            );
            push_rect(
                &mut rectangles,
                v_light_right,
                right_top,
                v_double_right,
                metrics.height,
                metrics,
            );
        }
    }
    match shape.left {
        Empty => {}
        Light => push_rect(
            &mut rectangles,
            0.0,
            h_light_top,
            left_right,
            h_light_bottom,
            metrics,
        ),
        Heavy => push_rect(
            &mut rectangles,
            0.0,
            h_heavy_top,
            left_right,
            h_heavy_bottom,
            metrics,
        ),
        Double => {
            let top_right = if shape.up == Double {
                v_light_left
            } else {
                left_right
            };
            let bottom_right = if shape.down == Double {
                v_light_left
            } else {
                left_right
            };
            push_rect(
                &mut rectangles,
                0.0,
                h_double_top,
                top_right,
                h_light_top,
                metrics,
            );
            push_rect(
                &mut rectangles,
                0.0,
                h_light_bottom,
                bottom_right,
                h_double_bottom,
                metrics,
            );
        }
    }
    rectangles.merge_collinear();
    rectangles
}

#[cfg(test)]
mod tests {
    use super::*;

    const METRICS: BoxMetrics = BoxMetrics {
        width: 18.0,
        height: 36.0,
        font_size: 30.0,
    };

    #[test]
    fn common_box_drawing_ranges_have_pixel_geometry() {
        for codepoint in 0x2500..=0x256C {
            let character = char::from_u32(codepoint).expect("box-drawing codepoint");
            assert!(
                box_drawing(character).is_some(),
                "missing U+{codepoint:04X}"
            );
        }
        for codepoint in 0x2574..=0x257F {
            let character = char::from_u32(codepoint).expect("box-drawing codepoint");
            assert!(
                box_drawing(character).is_some(),
                "missing U+{codepoint:04X}"
            );
        }
    }

    #[test]
    fn straight_lines_span_the_entire_cell_without_join_gaps() {
        let horizontal = box_rectangles(box_drawing('─').expect("horizontal"), METRICS);
        assert_eq!(horizontal.as_slice().len(), 1);
        assert!(horizontal.as_slice()[0].left.abs() < f32::EPSILON);
        assert!((horizontal.as_slice()[0].right - METRICS.width).abs() < f32::EPSILON);

        let vertical = box_rectangles(box_drawing('│').expect("vertical"), METRICS);
        assert_eq!(vertical.as_slice().len(), 1);
        assert!(vertical.as_slice()[0].top.abs() < f32::EPSILON);
        assert!((vertical.as_slice()[0].bottom - METRICS.height).abs() < f32::EPSILON);
    }

    #[test]
    fn rounded_characters_use_oriented_retained_geometry() {
        assert_eq!(
            box_drawing('╭'),
            Some(BoxDrawing::Rounded(RoundedCorner::UpperLeft))
        );
        assert_eq!(
            box_drawing('╮'),
            Some(BoxDrawing::Rounded(RoundedCorner::UpperRight))
        );
        assert_eq!(
            box_drawing('╯'),
            Some(BoxDrawing::Rounded(RoundedCorner::LowerRight))
        );
        assert_eq!(
            box_drawing('╰'),
            Some(BoxDrawing::Rounded(RoundedCorner::LowerLeft))
        );
    }

    #[test]
    fn diagonal_characters_keep_font_fallback() {
        for character in ['╱', '╲', '╳'] {
            assert_eq!(box_drawing(character), None);
        }
    }
}
