use std::time::{Duration, Instant};

use gpui_kit::Modifiers;

use crate::commands::CommandAction;

pub(crate) const TAB_SWITCH_HINTED_TAB_COUNT: usize = 9;

const TAB_SWITCH_HINT_HOLD_DELAY: Duration = Duration::from_millis(260);
const TAB_SWITCH_HINT_FADE_DURATION: Duration = Duration::from_millis(140);

#[derive(Clone, Debug)]
pub(crate) struct TabSwitchHintState {
    enabled: bool,
    modifier_held: bool,
    hold_started_at: Option<Instant>,
    suppressed_for_hold: bool,
    animation_scheduled: bool,
}

impl TabSwitchHintState {
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            modifier_held: false,
            hold_started_at: None,
            suppressed_for_hold: false,
            animation_scheduled: false,
        }
    }

    pub(crate) fn sync_enabled(&mut self, enabled: bool) -> bool {
        self.enabled = enabled;
        if enabled {
            return false;
        }
        self.reset_hold_state()
    }

    pub(crate) fn reset_hold_state(&mut self) -> bool {
        let changed =
            self.modifier_held || self.hold_started_at.is_some() || self.suppressed_for_hold;
        self.modifier_held = false;
        self.hold_started_at = None;
        self.suppressed_for_hold = false;
        self.animation_scheduled = false;
        changed
    }

    /// Whether exactly the tab-switch modifier is held: Ctrl+Alt on Windows
    /// (the default `switch_to_tab_*` bindings there are Ctrl+Alt+1..9, since
    /// Alt+1..9 selects panes), the secondary key alone elsewhere.
    pub(crate) fn tab_switch_modifier_held_alone(modifiers: Modifiers) -> bool {
        if cfg!(target_os = "windows") {
            modifiers.control && modifiers.alt && modifiers.number_of_modifiers() == 2
        } else {
            modifiers.secondary() && modifiers.number_of_modifiers() == 1
        }
    }

    pub(crate) fn label_for_index(index: usize) -> Option<String> {
        Self::supports_tab_index(index).then(|| format!("{}{}", Self::label_prefix(), index + 1))
    }

    pub(crate) fn supports_tab_index(index: usize) -> bool {
        index < TAB_SWITCH_HINTED_TAB_COUNT
    }

    pub(crate) fn handle_modifiers_changed(&mut self, modifiers: Modifiers, now: Instant) -> bool {
        if !self.enabled {
            return self.reset_hold_state();
        }

        let next = Self::tab_switch_modifier_held_alone(modifiers);
        if next {
            if self.modifier_held {
                return false;
            }
            self.modifier_held = true;
            self.hold_started_at = Some(now);
            self.suppressed_for_hold = false;
            self.animation_scheduled = false;
            return true;
        }

        self.reset_hold_state()
    }

    pub(crate) fn suppress_for_key_down(
        &mut self,
        key: &str,
        modifiers: Modifiers,
        overlays_blocked: bool,
        now: Instant,
    ) -> bool {
        if !Self::tab_switch_modifier_held_alone(modifiers) || Self::key_keeps_hint(key) {
            return false;
        }

        self.suppress(overlays_blocked, now)
    }

    pub(crate) fn suppress_for_action(
        &mut self,
        action: CommandAction,
        overlays_blocked: bool,
        now: Instant,
    ) -> bool {
        if Self::action_keeps_hint(action) {
            return false;
        }

        self.suppress(overlays_blocked, now)
    }

    pub(crate) fn progress(&self, now: Instant, overlays_blocked: bool) -> f32 {
        if !self.is_active(overlays_blocked) {
            return 0.0;
        }

        let Some(started_at) = self.hold_started_at else {
            return 0.0;
        };
        Self::progress_for_elapsed(now.saturating_duration_since(started_at))
    }

    pub(crate) fn should_render(
        &self,
        index: usize,
        is_renaming: bool,
        overlays_blocked: bool,
        now: Instant,
    ) -> bool {
        !is_renaming
            && Self::supports_tab_index(index)
            && self.progress(now, overlays_blocked) > f32::EPSILON
    }

    pub(crate) fn animation_active(&self, now: Instant, overlays_blocked: bool) -> bool {
        if !self.is_active(overlays_blocked) {
            return false;
        }

        let Some(started_at) = self.hold_started_at else {
            return false;
        };
        now.saturating_duration_since(started_at)
            < TAB_SWITCH_HINT_HOLD_DELAY + TAB_SWITCH_HINT_FADE_DURATION
    }

    pub(crate) fn begin_animation_frame(&mut self, now: Instant, overlays_blocked: bool) -> bool {
        if self.animation_scheduled || !self.animation_active(now, overlays_blocked) {
            return false;
        }

        self.animation_scheduled = true;
        true
    }

    pub(crate) fn finish_animation_frame(&mut self) {
        self.animation_scheduled = false;
    }

    fn is_active(&self, overlays_blocked: bool) -> bool {
        self.enabled && self.modifier_held && !self.suppressed_for_hold && !overlays_blocked
    }

    fn suppress(&mut self, overlays_blocked: bool, now: Instant) -> bool {
        if !self.enabled || !self.modifier_held || self.suppressed_for_hold {
            return false;
        }

        let was_visible = self.progress(now, overlays_blocked) > f32::EPSILON;
        self.suppressed_for_hold = true;
        was_visible
    }

    fn action_keeps_hint(action: CommandAction) -> bool {
        matches!(
            action,
            CommandAction::SwitchToTab1
                | CommandAction::SwitchToTab2
                | CommandAction::SwitchToTab3
                | CommandAction::SwitchToTab4
                | CommandAction::SwitchToTab5
                | CommandAction::SwitchToTab6
                | CommandAction::SwitchToTab7
                | CommandAction::SwitchToTab8
                | CommandAction::SwitchToTab9
        )
    }

    fn key_keeps_hint(key: &str) -> bool {
        matches!(key, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
    }

    fn label_prefix() -> &'static str {
        if cfg!(target_os = "macos") {
            "⌘"
        } else if cfg!(target_os = "windows") {
            "⌃⌥"
        } else {
            "⌃"
        }
    }

    fn progress_for_elapsed(elapsed: Duration) -> f32 {
        if elapsed < TAB_SWITCH_HINT_HOLD_DELAY {
            return 0.0;
        }
        let fade_elapsed = elapsed.saturating_sub(TAB_SWITCH_HINT_HOLD_DELAY);
        if fade_elapsed >= TAB_SWITCH_HINT_FADE_DURATION {
            return 1.0;
        }

        ease_out_cubic(fade_elapsed.as_secs_f32() / TAB_SWITCH_HINT_FADE_DURATION.as_secs_f32())
            .clamp(0.0, 1.0)
    }
}

fn ease_out_cubic(progress: f32) -> f32 {
    1.0 - (1.0 - progress).powi(3)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The modifiers that show the hints on the platform under test.
    fn hint_modifiers() -> Modifiers {
        if cfg!(target_os = "windows") {
            Modifiers {
                control: true,
                alt: true,
                ..Modifiers::default()
            }
        } else {
            Modifiers::secondary_key()
        }
    }

    #[test]
    fn tab_switch_modifier_requires_exact_modifier_set() {
        assert!(TabSwitchHintState::tab_switch_modifier_held_alone(
            hint_modifiers()
        ));

        let with_shift = Modifiers {
            shift: true,
            ..hint_modifiers()
        };
        assert!(!TabSwitchHintState::tab_switch_modifier_held_alone(
            with_shift
        ));
        assert!(!TabSwitchHintState::tab_switch_modifier_held_alone(
            Modifiers::default()
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_hints_need_ctrl_and_alt() {
        assert!(!TabSwitchHintState::tab_switch_modifier_held_alone(
            Modifiers::control()
        ));
        assert!(!TabSwitchHintState::tab_switch_modifier_held_alone(
            Modifiers::alt()
        ));
    }

    #[test]
    fn progress_respects_hold_delay_and_fade() {
        assert_eq!(
            TabSwitchHintState::progress_for_elapsed(Duration::from_millis(0)),
            0.0
        );
        assert_eq!(
            TabSwitchHintState::progress_for_elapsed(
                TAB_SWITCH_HINT_HOLD_DELAY.saturating_sub(Duration::from_millis(1))
            ),
            0.0
        );
        assert!(
            TabSwitchHintState::progress_for_elapsed(
                TAB_SWITCH_HINT_HOLD_DELAY + Duration::from_millis(40)
            ) > 0.0
        );
        assert_eq!(
            TabSwitchHintState::progress_for_elapsed(
                TAB_SWITCH_HINT_HOLD_DELAY + TAB_SWITCH_HINT_FADE_DURATION
            ),
            1.0
        );
    }

    #[test]
    fn labels_cover_first_nine_tabs_only() {
        assert_eq!(
            TabSwitchHintState::label_for_index(0).as_deref(),
            Some(if cfg!(target_os = "macos") {
                "⌘1"
            } else if cfg!(target_os = "windows") {
                "⌃⌥1"
            } else {
                "⌃1"
            })
        );
        assert_eq!(
            TabSwitchHintState::label_for_index(8).as_deref(),
            Some(if cfg!(target_os = "macos") {
                "⌘9"
            } else if cfg!(target_os = "windows") {
                "⌃⌥9"
            } else {
                "⌃9"
            })
        );
        assert_eq!(TabSwitchHintState::label_for_index(9), None);
    }

    #[test]
    fn should_render_requires_supported_visible_non_renaming_state() {
        let mut state = TabSwitchHintState::new(true);
        let now = Instant::now();
        assert!(state.handle_modifiers_changed(hint_modifiers(), now));
        let visible_at = now + TAB_SWITCH_HINT_HOLD_DELAY + TAB_SWITCH_HINT_FADE_DURATION;

        assert!(state.should_render(0, false, false, visible_at));
        assert!(!state.should_render(0, true, false, visible_at));
        assert!(!state.should_render(9, false, false, visible_at));
        assert!(!state.should_render(0, false, true, visible_at));
    }

    #[test]
    fn non_digit_key_suppresses_visible_hint_immediately() {
        let mut state = TabSwitchHintState::new(true);
        let now = Instant::now();
        state.handle_modifiers_changed(hint_modifiers(), now);
        let visible_at = now + TAB_SWITCH_HINT_HOLD_DELAY + TAB_SWITCH_HINT_FADE_DURATION;

        assert!(state.progress(visible_at, false) > 0.0);
        assert!(state.suppress_for_key_down("k", hint_modifiers(), false, visible_at));
        assert_eq!(state.progress(visible_at, false), 0.0);
    }

    #[test]
    fn numeric_tab_switch_actions_do_not_suppress_hint() {
        let mut state = TabSwitchHintState::new(true);
        let now = Instant::now();
        state.handle_modifiers_changed(hint_modifiers(), now);
        let visible_at = now + TAB_SWITCH_HINT_HOLD_DELAY + TAB_SWITCH_HINT_FADE_DURATION;

        assert!(!state.suppress_for_action(CommandAction::SwitchToTab4, false, visible_at));
        assert!(state.progress(visible_at, false) > 0.0);
    }

    #[test]
    fn reset_and_disable_clear_hold_state() {
        let mut state = TabSwitchHintState::new(true);
        let now = Instant::now();
        assert!(state.handle_modifiers_changed(hint_modifiers(), now));
        assert!(state.reset_hold_state());

        let mut state = TabSwitchHintState::new(true);
        assert!(state.handle_modifiers_changed(hint_modifiers(), now));
        assert!(state.sync_enabled(false));
        assert_eq!(state.progress(now, false), 0.0);
    }
}
