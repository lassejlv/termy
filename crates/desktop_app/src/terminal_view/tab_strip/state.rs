use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use gpui::ScrollHandle;

use super::hints::TabSwitchHintState;
use super::layout::TabStripLayoutSnapshot;

const TAB_TITLE_WIDTH_CACHE_MAX_ENTRIES: usize = 512;
const TAB_INTERACTION_ANIMATION_DURATION: Duration = Duration::from_millis(200);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TabStripOrientation {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TabDropMarkerSide {
    Leading,
    Trailing,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TabStripOverflowState {
    pub(crate) left: bool,
    pub(crate) right: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct TabTitleWidthCache {
    font_family: String,
    font_size_bits: u32,
    widths: HashMap<String, f32>,
}

impl TabTitleWidthCache {
    pub(crate) fn clear(&mut self) {
        self.widths.clear();
    }

    pub(crate) fn invalidate_title(&mut self, title: &str) {
        self.widths.remove(title);
    }

    pub(crate) fn get(&self, title: &str, font_family: &str, font_size_bits: u32) -> Option<f32> {
        if self.font_family != font_family || self.font_size_bits != font_size_bits {
            return None;
        }

        self.widths.get(title).copied()
    }

    pub(crate) fn insert(
        &mut self,
        title: &str,
        font_family: &str,
        font_size_bits: u32,
        width: f32,
    ) {
        if self.font_family != font_family || self.font_size_bits != font_size_bits {
            self.font_family.clear();
            self.font_family.push_str(font_family);
            self.font_size_bits = font_size_bits;
            self.widths.clear();
        }

        if self.widths.len() >= TAB_TITLE_WIDTH_CACHE_MAX_ENTRIES
            && !self.widths.contains_key(title)
        {
            self.widths.clear();
        }
        self.widths.insert(title.to_string(), width.max(0.0));
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.widths.len()
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TabDragState {
    pub(crate) source_index: usize,
    pub(crate) drop_slot: Option<usize>,
    pub(crate) orientation: TabStripOrientation,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct TabDragPreviewState {
    pointer_primary_axis: Option<f32>,
    viewport_extent: f32,
    autoscroll_animating: bool,
}

impl TabDragPreviewState {
    pub(crate) fn clear(&mut self) {
        self.pointer_primary_axis = None;
        self.viewport_extent = 0.0;
        self.autoscroll_animating = false;
    }

    pub(crate) fn set_pointer_preview(&mut self, pointer_primary_axis: f32, viewport_extent: f32) {
        self.pointer_primary_axis = Some(pointer_primary_axis);
        self.viewport_extent = viewport_extent.max(0.0);
    }

    pub(crate) fn pointer_primary_axis(self) -> Option<f32> {
        self.pointer_primary_axis
    }

    pub(crate) fn viewport_extent(self) -> f32 {
        self.viewport_extent
    }

    pub(crate) fn autoscroll_animating(self) -> bool {
        self.autoscroll_animating
    }

    pub(crate) fn start_autoscroll_animation(&mut self) {
        self.autoscroll_animating = true;
    }

    pub(crate) fn stop_autoscroll_animation(&mut self) {
        self.autoscroll_animating = false;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TabStripTitlebarState {
    move_armed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TitlebarMouseDownOutcome {
    pub(crate) arm_move: bool,
    pub(crate) trigger_window_action: bool,
}

impl TabStripTitlebarState {
    pub(crate) fn on_mouse_down(
        &mut self,
        interactive_hit: bool,
        click_count: usize,
    ) -> TitlebarMouseDownOutcome {
        let trigger_window_action = !interactive_hit && click_count == 2;
        let arm_move = !interactive_hit && click_count != 2;
        self.move_armed = arm_move;
        TitlebarMouseDownOutcome {
            arm_move,
            trigger_window_action,
        }
    }

    pub(crate) fn on_mouse_up(&mut self) {
        self.move_armed = false;
    }

    pub(crate) fn disarm(&mut self) {
        self.move_armed = false;
    }

    pub(crate) fn should_start_window_move(self, dragging: bool, tab_drag_active: bool) -> bool {
        self.move_armed && dragging && !tab_drag_active
    }

    pub(crate) fn take_window_move_request(
        &mut self,
        dragging: bool,
        tab_drag_active: bool,
    ) -> bool {
        if tab_drag_active {
            self.disarm();
            return false;
        }

        if !self.should_start_window_move(dragging, tab_drag_active) {
            return false;
        }
        self.disarm();
        true
    }
}

pub(crate) struct TabStripState {
    pub(crate) horizontal_scroll_handle: ScrollHandle,
    // Separate handle for the vertical sidebar; never shares offset with the
    // horizontal handle to avoid cross-axis contamination.
    pub(crate) vertical_scroll_handle: ScrollHandle,
    pub(crate) switch_hints: TabSwitchHintState,
    pub(crate) hovered_tab: Option<usize>,
    pub(crate) hovered_tab_close: Option<usize>,
    hover_animation_tab: Option<usize>,
    hover_animation_started_at: Option<Instant>,
    // Tab whose hover highlight is currently fading out after the pointer left
    // it; mirrors the fade-in pair above so hover exits do not snap.
    unhover_animation_tab: Option<usize>,
    unhover_animation_started_at: Option<Instant>,
    pub(crate) interaction_animation_scheduled: bool,
    pub(crate) drag: Option<TabDragState>,
    pub(crate) drag_preview: TabDragPreviewState,
    pub(crate) horizontal_layout_revision: u64,
    pub(crate) horizontal_layout_last_synced_revision: u64,
    pub(crate) horizontal_layout_last_synced_viewport_width: f32,
    pub(crate) horizontal_layout_snapshot: Option<TabStripLayoutSnapshot>,
    // Last rendered height of the vertical sidebar's scrollable tabs region,
    // used by vertical scroll-into-view / autoscroll math. NaN until first render.
    pub(crate) vertical_layout_last_synced_viewport_height: f32,
    pub(crate) title_width_cache: TabTitleWidthCache,
    pub(crate) titlebar: TabStripTitlebarState,
}

impl TabStripState {
    pub(crate) fn new(show_tab_switch_modifier_hints: bool) -> Self {
        Self {
            horizontal_scroll_handle: ScrollHandle::new(),
            vertical_scroll_handle: ScrollHandle::new(),
            switch_hints: TabSwitchHintState::new(show_tab_switch_modifier_hints),
            hovered_tab: None,
            hovered_tab_close: None,
            hover_animation_tab: None,
            hover_animation_started_at: None,
            unhover_animation_tab: None,
            unhover_animation_started_at: None,
            interaction_animation_scheduled: false,
            drag: None,
            drag_preview: TabDragPreviewState::default(),
            horizontal_layout_revision: 0,
            horizontal_layout_last_synced_revision: 0,
            horizontal_layout_last_synced_viewport_width: f32::NAN,
            horizontal_layout_snapshot: None,
            vertical_layout_last_synced_viewport_height: f32::NAN,
            title_width_cache: TabTitleWidthCache::default(),
            titlebar: TabStripTitlebarState::default(),
        }
    }

    pub(crate) fn invalidate_layouts(&mut self) {
        self.horizontal_layout_revision = self.horizontal_layout_revision.wrapping_add(1);
    }

    pub(crate) fn start_hover_animation(&mut self, tab_index: usize, now: Instant) -> bool {
        if self.unhover_animation_tab == Some(tab_index) {
            self.unhover_animation_tab = None;
            self.unhover_animation_started_at = None;
        }
        if self.hover_animation_tab == Some(tab_index)
            && self.hover_animation_started_at.is_some_and(|started| {
                now.saturating_duration_since(started) < TAB_INTERACTION_ANIMATION_DURATION
            })
        {
            return false;
        }
        self.hover_animation_tab = Some(tab_index);
        self.hover_animation_started_at = Some(now);
        true
    }

    pub(crate) fn start_unhover_animation(&mut self, tab_index: usize, now: Instant) {
        self.unhover_animation_tab = Some(tab_index);
        self.unhover_animation_started_at = Some(now);
        if self.hover_animation_tab == Some(tab_index) {
            self.hover_animation_tab = None;
            self.hover_animation_started_at = None;
        }
    }

    pub(crate) fn clear_unhover_animation(&mut self) {
        self.unhover_animation_tab = None;
        self.unhover_animation_started_at = None;
    }

    fn progress_for(started_at: Option<Instant>, now: Instant) -> f32 {
        let Some(started_at) = started_at else {
            return 1.0;
        };
        let elapsed = now.saturating_duration_since(started_at).as_secs_f32();
        let total = TAB_INTERACTION_ANIMATION_DURATION.as_secs_f32();
        if total <= f32::EPSILON {
            return 1.0;
        }
        let raw = (elapsed / total).clamp(0.0, 1.0);
        1.0 - (1.0 - raw).powi(3)
    }

    pub(crate) fn hover_progress(&self, tab_index: usize, now: Instant) -> f32 {
        if self.hovered_tab == Some(tab_index) {
            return if self.hover_animation_tab == Some(tab_index) {
                Self::progress_for(self.hover_animation_started_at, now)
            } else {
                1.0
            };
        }
        if self.unhover_animation_tab == Some(tab_index) {
            return 1.0 - Self::progress_for(self.unhover_animation_started_at, now);
        }
        0.0
    }

    pub(crate) fn interaction_animation_active(&self, now: Instant) -> bool {
        [
            self.hover_animation_started_at,
            self.unhover_animation_started_at,
        ]
        .into_iter()
        .flatten()
        .any(|started| now.saturating_duration_since(started) < TAB_INTERACTION_ANIMATION_DURATION)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_width_cache_hits_for_matching_font_context() {
        let mut cache = TabTitleWidthCache::default();
        cache.insert("~/projects/termy", "JetBrains Mono", 12f32.to_bits(), 142.0);
        assert_eq!(
            cache.get("~/projects/termy", "JetBrains Mono", 12f32.to_bits()),
            Some(142.0)
        );
    }

    #[test]
    fn title_width_cache_misses_when_font_context_changes() {
        let mut cache = TabTitleWidthCache::default();
        cache.insert("title", "JetBrains Mono", 12f32.to_bits(), 64.0);
        assert_eq!(cache.get("title", "JetBrains Mono", 13f32.to_bits()), None);
        assert_eq!(cache.get("title", "Fira Code", 12f32.to_bits()), None);
    }

    #[test]
    fn title_width_cache_invalidate_title_removes_entry() {
        let mut cache = TabTitleWidthCache::default();
        cache.insert("title", "JetBrains Mono", 12f32.to_bits(), 64.0);
        cache.invalidate_title("title");
        assert_eq!(cache.get("title", "JetBrains Mono", 12f32.to_bits()), None);
    }

    #[test]
    fn title_width_cache_bounds_growth_by_clearing_at_capacity() {
        let mut cache = TabTitleWidthCache::default();
        for index in 0..TAB_TITLE_WIDTH_CACHE_MAX_ENTRIES {
            cache.insert(
                format!("tab-{index}").as_str(),
                "JetBrains Mono",
                12f32.to_bits(),
                index as f32,
            );
        }
        assert_eq!(cache.len(), TAB_TITLE_WIDTH_CACHE_MAX_ENTRIES);

        cache.insert("tab-overflow", "JetBrains Mono", 12f32.to_bits(), 1.0);
        assert_eq!(cache.len(), 1);
        assert_eq!(
            cache.get("tab-overflow", "JetBrains Mono", 12f32.to_bits()),
            Some(1.0)
        );
    }

    #[test]
    fn titlebar_state_mouse_down_arms_only_noninteractive_single_clicks() {
        let mut state = TabStripTitlebarState::default();
        assert_eq!(
            state.on_mouse_down(true, 1),
            TitlebarMouseDownOutcome {
                arm_move: false,
                trigger_window_action: false,
            }
        );
        assert_eq!(
            state.on_mouse_down(false, 2),
            TitlebarMouseDownOutcome {
                arm_move: false,
                trigger_window_action: true,
            }
        );
        assert_eq!(
            state.on_mouse_down(false, 1),
            TitlebarMouseDownOutcome {
                arm_move: true,
                trigger_window_action: false,
            }
        );
    }

    #[test]
    fn titlebar_state_window_move_request_disarms_after_take() {
        let mut state = TabStripTitlebarState::default();
        assert!(state.on_mouse_down(false, 1).arm_move);
        assert!(state.take_window_move_request(true, false));
        assert!(!state.take_window_move_request(true, false));
    }

    #[test]
    fn titlebar_state_disarm_cancels_pending_window_move() {
        let mut state = TabStripTitlebarState::default();
        assert!(state.on_mouse_down(false, 1).arm_move);

        state.disarm();

        assert!(!state.take_window_move_request(true, false));
    }

    #[test]
    fn titlebar_state_tab_drag_cancels_pending_window_move() {
        let mut state = TabStripTitlebarState::default();
        assert!(state.on_mouse_down(false, 1).arm_move);

        assert!(!state.take_window_move_request(true, true));
        assert!(!state.take_window_move_request(true, false));
    }
}
