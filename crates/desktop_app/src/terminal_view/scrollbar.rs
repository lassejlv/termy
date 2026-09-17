use crate::ui::scrollbar;

const MARKER_TOP_LIMIT_BUCKET_STEP: f32 = 0.5;

#[derive(Clone, Copy, Debug)]
pub(super) struct TerminalScrollbarLayout {
    pub(super) range: scrollbar::ScrollbarRange,
    pub(super) metrics: scrollbar::ScrollbarMetrics,
    pub(super) history_size: usize,
    pub(super) viewport_rows: usize,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct TerminalScrollbarDragState {
    pub(super) thumb_grab_offset: f32,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct TerminalScrollbarTrackHoldState {
    pub(super) local_y: f32,
    pub(super) track_height: f32,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct TerminalScrollbarHit {
    pub(super) local_y: f32,
    pub(super) thumb_hit: bool,
    pub(super) thumb_top: f32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TerminalScrollbarMarkerCacheKey {
    pub(super) results_revision: u64,
    pub(super) history_size: usize,
    pub(super) viewport_rows: usize,
    pub(super) marker_top_limit_bucket: i32,
}

#[derive(Clone, Debug, Default)]
pub(super) struct TerminalScrollbarMarkerCache {
    pub(super) key: Option<TerminalScrollbarMarkerCacheKey>,
    pub(super) marker_tops: Vec<f32>,
}

impl TerminalScrollbarMarkerCache {
    pub(super) fn clear(&mut self) {
        self.key = None;
        self.marker_tops.clear();
    }
}

pub(super) fn compute_layout(
    display_offset: usize,
    history_size: usize,
    viewport_rows: usize,
    line_height: f32,
    track_height: f32,
    min_thumb_height: f32,
) -> Option<TerminalScrollbarLayout> {
    if viewport_rows == 0
        || line_height <= f32::EPSILON
        || track_height <= f32::EPSILON
        || history_size == 0
    {
        return None;
    }

    let max_offset = history_size as f32 * line_height;
    let range = scrollbar::ScrollbarRange {
        offset: scrollbar::invert_offset_axis(display_offset as f32 * line_height, max_offset),
        max_offset,
        viewport_extent: viewport_rows as f32 * line_height,
        track_extent: track_height,
    };
    let metrics = scrollbar::compute_metrics(range, min_thumb_height)?;

    Some(TerminalScrollbarLayout {
        range,
        metrics,
        history_size,
        viewport_rows,
    })
}

pub(super) fn marker_top_limit(track_height: f32, marker_height: f32) -> f32 {
    (track_height - marker_height.max(0.0)).max(0.0)
}

pub(super) fn marker_top_limit_bucket(marker_top_limit: f32) -> i32 {
    (marker_top_limit.max(0.0) / MARKER_TOP_LIMIT_BUCKET_STEP).round() as i32
}

pub(super) fn marker_top_for_line(
    line: i32,
    history_size: usize,
    viewport_rows: usize,
    marker_top_limit: f32,
) -> f32 {
    if marker_top_limit <= f32::EPSILON {
        return 0.0;
    }

    let content_line_count = history_size.saturating_add(viewport_rows).max(1);
    let max_index = (content_line_count.saturating_sub(1)) as f32;
    if max_index <= f32::EPSILON {
        return 0.0;
    }

    let line_index = (line as f32 + history_size as f32).clamp(0.0, max_index);
    (line_index / max_index) * marker_top_limit
}

/// Maps line positions to marker tops and deduplicates adjacent marker buckets.
///
/// The input `lines` should be ordered (for example ascending line numbers or the
/// stable search-result order) because deduplication only compares each bucket
/// to the previous bucket using `dedupe_bucket_size` derived from `marker_height`.
/// Callers with unsorted input should sort first or use a different strategy.
pub(super) fn deduped_marker_tops<I>(
    lines: I,
    history_size: usize,
    viewport_rows: usize,
    marker_height: f32,
    marker_top_limit: f32,
) -> Vec<f32>
where
    I: IntoIterator<Item = i32>,
{
    let dedupe_bucket_size = marker_height.max(1.0);
    let mut marker_tops = Vec::new();
    let mut last_bucket = None;
    let mut previous_line = None;

    for line in lines {
        debug_assert!(previous_line.is_none_or(|previous| previous <= line));
        previous_line = Some(line);
        let top = marker_top_for_line(line, history_size, viewport_rows, marker_top_limit);
        let bucket = (top / dedupe_bucket_size).round() as i32;
        if last_bucket == Some(bucket) {
            continue;
        }
        last_bucket = Some(bucket);
        marker_tops.push(top);
    }

    marker_tops
}

#[cfg(test)]
mod tests {
    use super::*;
    use termy_core::search_engine::{SearchMatch, SearchResults};

    #[test]
    fn marker_top_for_line_maps_bounds() {
        let history_size = 100;
        let viewport_rows = 20;
        let marker_top_limit = 500.0;

        let top_history = marker_top_for_line(-100, history_size, viewport_rows, marker_top_limit);
        let bottom_viewport =
            marker_top_for_line(19, history_size, viewport_rows, marker_top_limit);

        assert!((top_history - 0.0).abs() < f32::EPSILON);
        assert!((bottom_viewport - marker_top_limit).abs() < f32::EPSILON);
    }

    #[test]
    fn marker_top_for_line_matches_current_after_jump_to_nearest() {
        let mut results = SearchResults::from_matches(vec![
            SearchMatch::new(-40, 0, 1),
            SearchMatch::new(-8, 0, 1),
            SearchMatch::new(-5, 0, 1),
            SearchMatch::new(6, 0, 1),
        ]);
        results.jump_to_nearest(-7);
        let current = results.current().expect("current match expected");

        let history_size = 80;
        let viewport_rows = 24;
        let marker_top_limit = 240.0;
        let expected = marker_top_for_line(-5, history_size, viewport_rows, marker_top_limit);
        let current_top =
            marker_top_for_line(current.line, history_size, viewport_rows, marker_top_limit);

        assert!((current_top - expected).abs() < f32::EPSILON);
    }

    #[test]
    fn deduped_marker_tops_collapse_adjacent_buckets() {
        let lines = [-500, -499, -498, -420];
        let tops = deduped_marker_tops(lines, 1000, 50, 2.0, 100.0);

        assert!(tops.len() < 4);
        assert!(!tops.is_empty());
    }

    #[test]
    fn deduped_marker_tops_matches_vec_and_iterator_sources() {
        let lines = vec![-500, -499, -498, -420, -419];
        let from_vec = deduped_marker_tops(lines.clone(), 1000, 50, 2.0, 100.0);
        let from_iter = deduped_marker_tops(lines.iter().copied(), 1000, 50, 2.0, 100.0);

        assert_eq!(from_vec, from_iter);
        assert!(from_iter.len() < lines.len());
    }

    #[test]
    fn marker_top_limit_bucket_quantizes_stably_around_boundary() {
        let boundary = MARKER_TOP_LIMIT_BUCKET_STEP * 2.5;
        let below = marker_top_limit_bucket(boundary - 0.001);
        let at_boundary = marker_top_limit_bucket(boundary);
        let above = marker_top_limit_bucket(boundary + 0.001);

        assert_eq!(below, 2);
        assert_eq!(at_boundary, 3);
        assert_eq!(above, 3);
        assert_eq!(marker_top_limit_bucket(-10.0), 0);
    }
}
