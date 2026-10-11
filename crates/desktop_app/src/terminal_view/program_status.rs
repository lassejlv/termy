use super::*;
use termy_core::{ProgramState, ProgramStatusRecord};

// OSC 7501 program status is shown only as a tab badge. Pane progress bars stay
// driven by OSC 9;4 alone, so programs that disable their progress reporting do
// not get a bar back through their status updates.

fn priority(record: &ProgramStatusRecord) -> u8 {
    match record.state {
        ProgramState::Blocked => 4,
        ProgramState::Error => 3,
        ProgramState::Done => 2,
        ProgramState::Working => 1,
        ProgramState::Idle => 0,
    }
}

impl TerminalPane {
    fn active_program_status(&self) -> Option<&ProgramStatusRecord> {
        self.program_status
            .iter()
            .filter(|record| record.state != ProgramState::Idle)
            .max_by_key(|record| priority(record))
    }
}

impl TerminalTab {
    pub(super) fn program_status_state(&self) -> Option<ProgramState> {
        self.panes
            .iter()
            .filter_map(TerminalPane::active_program_status)
            .max_by_key(|record| priority(record))
            .map(|record| record.state)
    }
}

impl TerminalView {
    pub(super) fn render_program_status_badge(
        state: ProgramState,
        colors: &TerminalColors,
    ) -> AnyElement {
        let (glyph, color) = match state {
            ProgramState::Working => ("●", colors.ansi[4]),
            ProgramState::Blocked => ("?", colors.ansi[3]),
            ProgramState::Done => ("✓", colors.ansi[2]),
            ProgramState::Error => ("!", colors.ansi[1]),
            ProgramState::Idle => ("", colors.foreground),
        };
        div()
            .text_size(px(11.0))
            .text_color(color)
            .child(glyph)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_children_take_priority_without_touching_osc9_progress() {
        let mut engine =
            termy_core::terminal_engine::Engine::new(Default::default(), Default::default());
        engine.feed(b"\x1b]7501;state=working:app=deploy:progress=20\x07\x1b]7501;state=blocked:id=eu:kind=permission:progress=70\x07");
        let mut pane = TerminalPane::new_native(
            "pane".into(),
            0,
            0,
            80,
            24,
            Terminal::new_test_display(TerminalSize::default()),
        );
        pane.progress_state = ProgressState::InProgress(10);
        pane.program_status = engine.program_status();
        let selected = pane.active_program_status().unwrap();
        assert_eq!(selected.id, "eu");
        assert_eq!(selected.state, ProgramState::Blocked);
        assert_eq!(pane.progress_state, ProgressState::InProgress(10));
        engine.feed(b"\x1b]7501;state=clear:id=eu\x07");
        pane.program_status = engine.program_status();
        assert_eq!(
            pane.active_program_status().map(|record| record.state),
            Some(ProgramState::Working)
        );
        engine.feed(b"\x1b]7501;state=done\x07");
        pane.program_status = engine.program_status();
        assert_eq!(
            pane.active_program_status().map(|record| record.state),
            Some(ProgramState::Done)
        );
        assert_eq!(pane.progress_state, ProgressState::InProgress(10));
    }
}
