//! Smooth top-strip transitions: the sliding active indicator and the
//! collapsing close overlays. Both are purely visual — the tab model is
//! updated immediately and these animations only affect rendering.

use super::super::*;

pub(super) const TAB_TRANSITION_FRAME_MS: u64 = 16;
const TAB_INDICATOR_SLIDE_DURATION: Duration = Duration::from_millis(220);
const TAB_CLOSE_ANIMATION_DURATION: Duration = Duration::from_millis(180);
const TAB_CLOSE_OVERLAY_MAX: usize = 8;
// Below this travel distance the indicator snaps instead of sliding.
const TAB_INDICATOR_SLIDE_SNAP_PX: f32 = 0.5;

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

#[derive(Clone, Copy, Debug)]
struct TabIndicatorSlide {
    from_x: f32,
    to_x: f32,
    target_tab: TabId,
    started_at: Instant,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct TabStripTransitions {
    indicator_slide: Option<TabIndicatorSlide>,
    indicator_last_active: Option<(TabId, f32)>,
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
        if self.indicator_slide.is_some_and(|slide| {
            animation_raw(slide.started_at, now, TAB_INDICATOR_SLIDE_DURATION).is_none()
        }) {
            self.indicator_slide = None;
        }
        self.closing_overlays.retain(|overlay| {
            animation_raw(overlay.started_at, now, TAB_CLOSE_ANIMATION_DURATION).is_some()
        });
        self.indicator_slide.is_some() || !self.closing_overlays.is_empty()
    }

    /// Tracks the active tab across renders and returns the sliding
    /// indicator's current x while a switch animation is running. Retargets
    /// from the current presentation position when the active tab changes
    /// mid-flight; snaps when travel is negligible.
    pub(crate) fn sync_slide(
        &mut self,
        active_tab: TabId,
        active_x: f32,
        now: Instant,
    ) -> Option<f32> {
        if let Some(slide) = self.indicator_slide {
            match animation_raw(slide.started_at, now, TAB_INDICATOR_SLIDE_DURATION) {
                None => self.indicator_slide = None,
                Some(raw) if slide.target_tab == active_tab => {
                    let current_x =
                        slide.from_x + (active_x - slide.from_x) * ease_in_out_cubic(raw);
                    self.indicator_slide = Some(TabIndicatorSlide {
                        to_x: active_x,
                        ..slide
                    });
                    self.indicator_last_active = Some((active_tab, active_x));
                    return Some(current_x);
                }
                _ => {}
            }
        }

        let from_x = match self.indicator_slide {
            // Retarget: continue from the current presentation position.
            Some(slide) => {
                let raw = animation_raw(slide.started_at, now, TAB_INDICATOR_SLIDE_DURATION)
                    .unwrap_or(1.0);
                Some(slide.from_x + (slide.to_x - slide.from_x) * ease_in_out_cubic(raw))
            }
            None => match self.indicator_last_active {
                Some((last_tab, last_x)) if last_tab != active_tab => Some(last_x),
                _ => None,
            },
        };
        self.indicator_last_active = Some((active_tab, active_x));
        let from_x = from_x?;
        if (from_x - active_x).abs() < TAB_INDICATOR_SLIDE_SNAP_PX {
            self.indicator_slide = None;
            return None;
        }
        self.indicator_slide = Some(TabIndicatorSlide {
            from_x,
            to_x: active_x,
            target_tab: active_tab,
            started_at: now,
        });
        Some(from_x)
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

    /// Content-relative x of a tab in the horizontal strip, mirroring the
    /// flex layout: leading spacer, then each tab preceded by the close
    /// overlays collapsing at its index, with a gap between every child.
    pub(crate) fn content_x_for_tab(
        tab_index: usize,
        tab_widths: &[f32],
        overlays: &[ClosingTabOverlaySlot],
    ) -> f32 {
        let mut x = TAB_HORIZONTAL_PADDING + TAB_ITEM_GAP;
        for (index, width) in tab_widths.iter().enumerate() {
            for overlay in overlays.iter().filter(|slot| slot.index == index) {
                x += overlay.width + TAB_ITEM_GAP;
            }
            if index == tab_index {
                return x;
            }
            x += width + TAB_ITEM_GAP;
        }
        x
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

    fn slot(index: usize, width: f32) -> ClosingTabOverlaySlot {
        ClosingTabOverlaySlot {
            index,
            title: String::from("closed"),
            width,
            alpha: 1.0,
            was_active: false,
        }
    }

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
    fn content_x_accounts_for_spacer_gaps_and_overlays() {
        let widths = vec![100.0, 120.0];
        // First tab sits after the leading spacer plus one gap.
        assert_eq!(
            TerminalView::content_x_for_tab(0, &widths, &[]),
            TAB_HORIZONTAL_PADDING + TAB_ITEM_GAP
        );
        assert_eq!(
            TerminalView::content_x_for_tab(1, &widths, &[]),
            TAB_HORIZONTAL_PADDING + TAB_ITEM_GAP + 100.0 + TAB_ITEM_GAP
        );

        // An overlay collapsing ahead of the second tab shifts it right.
        let overlays = vec![slot(1, 40.0)];
        assert_eq!(
            TerminalView::content_x_for_tab(1, &widths, &overlays),
            TAB_HORIZONTAL_PADDING + TAB_ITEM_GAP + 100.0 + TAB_ITEM_GAP + 40.0 + TAB_ITEM_GAP
        );
        // Overlays at or after an index do not move earlier tabs.
        assert_eq!(
            TerminalView::content_x_for_tab(0, &widths, &overlays),
            TAB_HORIZONTAL_PADDING + TAB_ITEM_GAP
        );
    }

    #[test]
    fn slide_starts_on_active_change_and_settles_at_target() {
        let mut transitions = TabStripTransitions::default();
        let start = Instant::now();

        assert_eq!(transitions.sync_slide(1, 12.0, start), None);
        // Same tab re-rendered: no animation.
        assert_eq!(
            transitions.sync_slide(1, 12.0, start + Duration::from_millis(50)),
            None
        );

        // Switch to a tab further right: slide starts at the old position.
        let switch_at = start + Duration::from_millis(50);
        assert_eq!(transitions.sync_slide(2, 112.0, switch_at), Some(12.0));

        // Mid-flight the indicator sits between the endpoints.
        let mid = transitions
            .sync_slide(2, 112.0, switch_at + Duration::from_millis(110))
            .expect("slide in flight");
        assert!(mid > 12.0 && mid < 112.0, "mid-flight x: {mid}");

        // After the duration the slide is over and the static indicator resumes.
        assert_eq!(
            transitions.sync_slide(2, 112.0, switch_at + TAB_INDICATOR_SLIDE_DURATION),
            None
        );
        assert!(!transitions.advance(switch_at + TAB_INDICATOR_SLIDE_DURATION));
    }

    #[test]
    fn slide_retargets_from_presentation_position_mid_flight() {
        let mut transitions = TabStripTransitions::default();
        let start = Instant::now();
        assert_eq!(transitions.sync_slide(1, 0.0, start), None);

        let switch_at = start + Duration::from_millis(10);
        assert_eq!(transitions.sync_slide(2, 100.0, switch_at), Some(0.0));

        // Switch again mid-flight: the new slide continues from wherever the
        // indicator currently is instead of jumping back to an endpoint.
        let retarget_at = switch_at + Duration::from_millis(110);
        let resumed = transitions.sync_slide(3, 200.0, retarget_at);
        let resumed = resumed.expect("retargeted slide");
        assert!(
            resumed > 0.0 && resumed < 100.0,
            "retarget origin: {resumed}"
        );
    }

    #[test]
    fn slide_snaps_for_negligible_travel() {
        let mut transitions = TabStripTransitions::default();
        let start = Instant::now();
        assert_eq!(transitions.sync_slide(1, 12.0, start), None);
        assert_eq!(
            transitions.sync_slide(
                2,
                12.0 + TAB_INDICATOR_SLIDE_SNAP_PX / 2.0,
                start + Duration::from_millis(10)
            ),
            None
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
