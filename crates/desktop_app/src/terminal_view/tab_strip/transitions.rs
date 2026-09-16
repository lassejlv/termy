//! Smooth top-strip transitions: collapsing close overlays. Purely visual —
//! the tab model is updated immediately and these animations only affect
//! rendering.

use super::super::*;

pub(super) const TAB_TRANSITION_FRAME_MS: u64 = 16;
const TAB_CLOSE_ANIMATION_DURATION: Duration = Duration::from_millis(180);
const TAB_CLOSE_OVERLAY_MAX: usize = 8;

/// Snapshot of a just-closed tab, rendered as a collapsing chip at its old
/// position until the close animation finishes.
#[derive(Clone, Debug)]
pub(crate) struct ClosingTabOverlay {
    index: usize,
    title: String,
    width: f32,
    was_active: bool,
    started_at: Instant,
}

/// Per-frame render snapshot of a live close overlay.
#[derive(Clone, Debug)]
pub(crate) struct ClosingTabOverlaySlot {
    pub(crate) index: usize,
    pub(crate) title: String,
    pub(crate) width: f32,
    pub(crate) alpha: f32,
    pub(crate) was_active: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct TabStripTransitions {
    closing_overlays: Vec<ClosingTabOverlay>,
    frame_scheduled: bool,
}

fn ease_in_out_cubic(raw: f32) -> f32 {
    let raw = raw.clamp(0.0, 1.0);
    if raw < 0.5 {
        4.0 * raw * raw * raw
    } else {
        1.0 - (-2.0 * raw + 2.0).powi(3) / 2.0
    }
}

/// Elapsed fraction of an animation, or `None` once it has finished.
fn animation_raw(started_at: Instant, now: Instant, duration: Duration) -> Option<f32> {
    let elapsed = now.saturating_duration_since(started_at);
    if elapsed >= duration {
        return None;
    }
    let total = duration.as_secs_f32();
    if total <= f32::EPSILON {
        return None;
    }
    Some((elapsed.as_secs_f32() / total).clamp(0.0, 1.0))
}

impl TabStripTransitions {
    /// Current render width of each live close overlay, sorted by strip
    /// index. Overlay indices are clamped so tabs removed while others were
    /// mid-collapse still render inside the strip.
    pub(crate) fn overlay_slots(
        &self,
        tab_count: usize,
        now: Instant,
    ) -> Vec<ClosingTabOverlaySlot> {
        let mut slots: Vec<ClosingTabOverlaySlot> = self
            .closing_overlays
            .iter()
            .filter_map(|overlay| {
                let raw = animation_raw(overlay.started_at, now, TAB_CLOSE_ANIMATION_DURATION)?;
                Some(ClosingTabOverlaySlot {
                    index: overlay.index.min(tab_count),
                    title: overlay.title.clone(),
                    width: overlay.width * (1.0 - ease_in_out_cubic(raw)),
                    alpha: 1.0 - raw,
                    was_active: overlay.was_active,
                })
            })
            .collect();
        slots.sort_by_key(|slot| slot.index);
        slots
    }

    pub(crate) fn push_closing_overlay(
        &mut self,
        index: usize,
        title: String,
        width: f32,
        was_active: bool,
        now: Instant,
    ) {
        if self.closing_overlays.len() >= TAB_CLOSE_OVERLAY_MAX {
            self.closing_overlays.remove(0);
        }
        self.closing_overlays.push(ClosingTabOverlay {
            index,
            title,
            width: width.max(0.0),
            was_active,
            started_at: now,
        });
    }

    /// Drops finished animations. Returns whether any animation still needs
    /// frames.
    pub(crate) fn advance(&mut self, now: Instant) -> bool {
        self.closing_overlays.retain(|overlay| {
            animation_raw(overlay.started_at, now, TAB_CLOSE_ANIMATION_DURATION).is_some()
        });
        !self.closing_overlays.is_empty()
    }
}

impl TerminalView {
    /// Stable (non-animated) horizontal render width for a tab.
    pub(crate) fn stable_tab_render_width(display_width: f32) -> f32 {
        if display_width.is_finite() {
            display_width.max(TAB_MIN_WIDTH)
        } else {
            TAB_MIN_WIDTH
        }
    }

    pub(crate) fn push_closing_tab_overlay(
        &mut self,
        index: usize,
        title: String,
        width: f32,
        was_active: bool,
        cx: &mut Context<Self>,
    ) {
        self.tab_strip.transitions.push_closing_overlay(
            index,
            title,
            width,
            was_active,
            Instant::now(),
        );
        self.schedule_tab_transitions_frame(cx);
    }

    pub(crate) fn schedule_tab_transitions_frame(&mut self, cx: &mut Context<Self>) {
        if self.tab_strip.transitions.frame_scheduled {
            return;
        }
        self.tab_strip.transitions.frame_scheduled = true;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            smol::Timer::after(Duration::from_millis(TAB_TRANSITION_FRAME_MS)).await;
            let _ = cx.update(|cx| {
                this.update(cx, |view, cx| {
                    view.tab_strip.transitions.frame_scheduled = false;
                    let still_animating = view.tab_strip.transitions.advance(Instant::now());
                    if still_animating {
                        view.schedule_tab_transitions_frame(cx);
                    }
                    cx.notify();
                })
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_width_clamps_to_minimum_and_rejects_non_finite() {
        assert_eq!(
            TerminalView::stable_tab_render_width(TAB_MAX_WIDTH),
            TAB_MAX_WIDTH
        );
        assert_eq!(TerminalView::stable_tab_render_width(12.0), TAB_MIN_WIDTH);
        assert_eq!(
            TerminalView::stable_tab_render_width(f32::NAN),
            TAB_MIN_WIDTH
        );
        assert_eq!(
            TerminalView::stable_tab_render_width(f32::INFINITY),
            TAB_MIN_WIDTH
        );
    }

    #[test]
    fn overlay_slots_shrink_fade_and_clamp_index() {
        let mut transitions = TabStripTransitions::default();
        let start = Instant::now();
        transitions.push_closing_overlay(7, String::from("shell"), 100.0, true, start);

        // Beyond the end of a shorter strip the overlay clamps into view.
        let slots = transitions.overlay_slots(2, start);
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].index, 2);
        assert_eq!(slots[0].width, 100.0);
        assert_eq!(slots[0].alpha, 1.0);
        assert!(slots[0].was_active);

        let mid = transitions.overlay_slots(2, start + TAB_CLOSE_ANIMATION_DURATION / 2);
        assert!(mid[0].width > 0.0 && mid[0].width < 100.0);
        assert!(mid[0].alpha > 0.0 && mid[0].alpha < 1.0);

        assert!(
            transitions
                .overlay_slots(2, start + TAB_CLOSE_ANIMATION_DURATION)
                .is_empty()
        );
        assert!(!transitions.advance(start + TAB_CLOSE_ANIMATION_DURATION));
    }

    #[test]
    fn overlay_push_is_bounded_and_sorted_by_index() {
        let mut transitions = TabStripTransitions::default();
        let start = Instant::now();
        for index in 0..TAB_CLOSE_OVERLAY_MAX + 3 {
            transitions.push_closing_overlay(
                TAB_CLOSE_OVERLAY_MAX + 3 - index,
                format!("tab-{index}"),
                100.0,
                false,
                start,
            );
        }
        let slots = transitions.overlay_slots(100, start);
        assert_eq!(slots.len(), TAB_CLOSE_OVERLAY_MAX);
        let indices: Vec<usize> = slots.iter().map(|slot| slot.index).collect();
        let mut sorted = indices.clone();
        sorted.sort_unstable();
        assert_eq!(indices, sorted);
    }
}
