use std::time::{Duration, Instant};

use engine::{Modifiers, MouseButton, MouseTrackingMode, SelectionMode, SelectionPoint, Terminal};

const MULTI_CLICK_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MouseRoute {
    Application,
    Selection,
}

pub(crate) fn mouse_route(mode: MouseTrackingMode, modifiers: Modifiers) -> MouseRoute {
    if mode == MouseTrackingMode::Disabled || modifiers.contains(Modifiers::SHIFT) {
        MouseRoute::Selection
    } else {
        MouseRoute::Application
    }
}

pub(crate) fn mouse_button_route(
    mode: MouseTrackingMode,
    modifiers: Modifiers,
    selection_dragging: bool,
    application_dragging: bool,
    button: MouseButton,
) -> MouseRoute {
    if selection_dragging && button == MouseButton::Left {
        MouseRoute::Selection
    } else if application_dragging {
        MouseRoute::Application
    } else {
        mouse_route(mode, modifiers)
    }
}

pub(crate) fn motion_button(pressed: Option<MouseButton>) -> MouseButton {
    pressed.unwrap_or(MouseButton::None)
}

pub(crate) fn selected_text_for_clipboard(terminal: &Terminal) -> Option<String> {
    terminal.selected_text().filter(|text| !text.is_empty())
}

#[derive(Debug, Default)]
pub(crate) struct ClickTracker {
    last_at: Option<Instant>,
    last_point: Option<SelectionPoint>,
    count: u8,
}

impl ClickTracker {
    pub(crate) fn register(&mut self, now: Instant, point: SelectionPoint) -> SelectionMode {
        let continues = self.last_point == Some(point)
            && self.last_at.is_some_and(|last_at| {
                now.saturating_duration_since(last_at) <= MULTI_CLICK_INTERVAL
            })
            && self.count < 3;
        self.count = if continues { self.count + 1 } else { 1 };
        self.last_at = Some(now);
        self.last_point = Some(point);
        match self.count {
            2 => SelectionMode::Word,
            3 => SelectionMode::Line,
            _ => SelectionMode::Character,
        }
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
}

#[derive(Debug, Default)]
pub(crate) struct ScrollAccumulator {
    remainder: f64,
}

impl ScrollAccumulator {
    pub(crate) fn consume(&mut self, delta: f64, unit: f64) -> isize {
        if !delta.is_finite() || !unit.is_finite() || unit <= 0.0 {
            return 0;
        }
        self.remainder += delta;
        let lines = (self.remainder / unit).trunc() as isize;
        self.remainder -= lines as f64 * unit;
        lines
    }

    pub(crate) fn reset(&mut self) {
        self.remainder = 0.0;
    }
}

pub(crate) fn repeat_mouse_report(report: &[u8], count: usize) -> Vec<u8> {
    report.repeat(count)
}

pub(crate) fn clear_mouse_state_on_focus_loss(
    focused: bool,
    pressed_button: &mut Option<MouseButton>,
    selection_dragging: &mut bool,
) {
    if !focused {
        *pressed_button = None;
        *selection_dragging = false;
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use engine::{
        Modifiers, MouseButton, MouseTrackingMode, SelectionMode, SelectionPoint, Terminal,
        TerminalConfig,
    };

    use super::{
        ClickTracker, MouseRoute, ScrollAccumulator, clear_mouse_state_on_focus_loss,
        motion_button, mouse_button_route, mouse_route, repeat_mouse_report,
        selected_text_for_clipboard,
    };

    #[test]
    fn shift_overrides_application_mouse_tracking_for_local_selection() {
        assert_eq!(
            mouse_route(MouseTrackingMode::AnyMotion, Modifiers::SHIFT),
            MouseRoute::Selection
        );
        assert_eq!(
            mouse_route(MouseTrackingMode::AnyMotion, Modifiers::empty()),
            MouseRoute::Application
        );
        assert_eq!(
            mouse_route(MouseTrackingMode::Disabled, Modifiers::empty()),
            MouseRoute::Selection
        );
    }

    #[test]
    fn unpressed_hover_uses_the_protocol_no_button_code() {
        assert_eq!(motion_button(None), MouseButton::None);
        assert_eq!(motion_button(Some(MouseButton::Right)), MouseButton::Right);
    }

    #[test]
    fn local_drag_release_stays_local_after_shift_is_released() {
        assert_eq!(
            mouse_button_route(
                MouseTrackingMode::AnyMotion,
                Modifiers::empty(),
                true,
                false,
                MouseButton::Left,
            ),
            MouseRoute::Selection
        );
        assert_eq!(
            mouse_button_route(
                MouseTrackingMode::AnyMotion,
                Modifiers::SHIFT,
                false,
                true,
                MouseButton::Left,
            ),
            MouseRoute::Application
        );
    }

    #[test]
    fn fractional_trackpad_deltas_accumulate_instead_of_disappearing() {
        let mut accumulator = ScrollAccumulator::default();
        for _ in 0..5 {
            assert_eq!(accumulator.consume(3.0, 18.0), 0);
        }
        assert_eq!(accumulator.consume(3.0, 18.0), 1);
        assert_eq!(accumulator.consume(7.0, 18.0), 0);
        assert_eq!(accumulator.consume(-10.0, 18.0), 0);
        assert_eq!(accumulator.consume(-15.0, 18.0), -1);
    }

    #[test]
    fn fractional_line_deltas_accumulate_instead_of_rounding_each_event() {
        let mut accumulator = ScrollAccumulator::default();
        for _ in 0..3 {
            assert_eq!(accumulator.consume(0.25, 1.0), 0);
        }
        assert_eq!(accumulator.consume(0.25, 1.0), 1);
        assert_eq!(accumulator.consume(-0.4, 1.0), 0);
        assert_eq!(accumulator.consume(-0.6, 1.0), -1);
    }

    #[test]
    fn application_wheel_reports_are_batched_without_an_eight_line_cap() {
        let report = b"\x1b[<64;4;2M";
        assert_eq!(repeat_mouse_report(report, 12), report.repeat(12));
    }

    #[test]
    fn focus_loss_clears_stale_application_and_selection_drags() {
        let mut pressed = Some(MouseButton::Left);
        let mut selecting = true;

        clear_mouse_state_on_focus_loss(true, &mut pressed, &mut selecting);
        assert_eq!(pressed, Some(MouseButton::Left));
        assert!(selecting);

        clear_mouse_state_on_focus_loss(false, &mut pressed, &mut selecting);
        assert_eq!(pressed, None);
        assert!(!selecting);
    }

    #[test]
    fn clipboard_copy_uses_the_core_selection_text() {
        let mut terminal = Terminal::new(TerminalConfig {
            columns: 8,
            rows: 2,
            scrollback_limit: 10,
        });
        terminal.feed(b"copy me");
        terminal.begin_selection(SelectionPoint { column: 0, row: 0 });
        terminal.update_selection(SelectionPoint { column: 6, row: 0 });
        assert_eq!(
            selected_text_for_clipboard(&terminal).as_deref(),
            Some("copy me")
        );
    }

    #[test]
    fn click_tracker_maps_single_double_and_triple_clicks_to_selection_modes() {
        let mut tracker = ClickTracker::default();
        let now = Instant::now();
        let point = SelectionPoint { column: 4, row: 2 };

        assert_eq!(tracker.register(now, point), SelectionMode::Character);
        assert_eq!(
            tracker.register(now + Duration::from_millis(100), point),
            SelectionMode::Word
        );
        assert_eq!(
            tracker.register(now + Duration::from_millis(200), point),
            SelectionMode::Line
        );
        assert_eq!(
            tracker.register(now + Duration::from_millis(300), point),
            SelectionMode::Character
        );
    }

    #[test]
    fn click_tracker_resets_after_timeout_cell_change_or_explicit_reset() {
        let mut tracker = ClickTracker::default();
        let now = Instant::now();
        let first = SelectionPoint { column: 1, row: 1 };
        let second = SelectionPoint { column: 2, row: 1 };

        assert_eq!(tracker.register(now, first), SelectionMode::Character);
        assert_eq!(
            tracker.register(now + Duration::from_millis(501), first),
            SelectionMode::Character
        );
        assert_eq!(
            tracker.register(now + Duration::from_millis(550), second),
            SelectionMode::Character
        );
        tracker.reset();
        assert_eq!(
            tracker.register(now + Duration::from_millis(600), second),
            SelectionMode::Character
        );
    }
}
