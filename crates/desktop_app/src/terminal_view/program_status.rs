use super::*;
use termy_core::{ProgramState, ProgramStatusKind, ProgramStatusRecord};

fn priority(record: &ProgramStatusRecord) -> u8 {
    match record.state {
        ProgramState::Blocked => 4,
        ProgramState::Error => 3,
        ProgramState::Done => 2,
        ProgramState::Working => 1,
        ProgramState::Idle => 0,
    }
}

fn label(record: &ProgramStatusRecord) -> &'static str {
    match record.state {
        ProgramState::Idle => "Idle",
        ProgramState::Working => "Working",
        ProgramState::Done => "Done",
        ProgramState::Error => "Failed",
        ProgramState::Blocked => match record.kind {
            Some(ProgramStatusKind::Permission) => "Approval needed",
            Some(ProgramStatusKind::Question) => "Answer needed",
            Some(ProgramStatusKind::Auth) => "Sign-in needed",
            None => "Blocked",
        },
    }
}

// Show free text literally and remove invisible direction/format controls from
// chrome. The protocol parser has already rejected C0/C1 control characters.
fn visible_text(text: &str) -> String {
    text.chars()
        .filter(|c| {
            !matches!(c,
                '\u{00ad}' | '\u{061c}' | '\u{180e}' | '\u{200b}'..='\u{200f}' |
                '\u{2028}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}' |
                '\u{fff9}'..='\u{fffb}' | '\u{e0000}'..='\u{e007f}'
            )
        })
        .collect()
}

impl TerminalPane {
    fn active_program_status(&self) -> Option<&ProgramStatusRecord> {
        self.program_status
            .iter()
            .filter(|record| record.state != ProgramState::Idle)
            .max_by_key(|record| priority(record))
    }

    pub(super) fn effective_progress_state(&self) -> ProgressState {
        match self.active_program_status() {
            Some(record) => match record.state {
                ProgramState::Working => record
                    .progress
                    .map_or(ProgressState::Indeterminate, ProgressState::InProgress),
                ProgramState::Blocked => ProgressState::Warning(record.progress.unwrap_or(0)),
                ProgramState::Error => ProgressState::Error(0),
                ProgramState::Done | ProgramState::Idle => ProgressState::Clear,
            },
            None => self.progress_state,
        }
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
    pub(super) fn pane_program_status_element(&self, pane: &TerminalPane) -> Option<AnyElement> {
        if !self.progress_indicator_enabled {
            return None;
        }
        let record = pane.active_program_status()?;
        let mut parts = vec![label(record).to_owned()];
        if let Some(app) = &record.app {
            parts.push(app.clone());
        }
        if let Some(title) = &record.title {
            parts.push(visible_text(title));
        } else if !record.id.is_empty() {
            parts.push(record.id.clone());
        }
        if let Some(progress) = record.progress {
            parts.push(format!("{progress}%"));
        }
        if let Some(msg) = &record.msg {
            parts.push(visible_text(msg));
        }
        if pane.program_status.len() > 1 {
            parts.push(format!("{} tasks", pane.program_status.len()));
        }
        Some(
            div()
                .absolute()
                .top(px(6.0))
                .right(px(8.0))
                .max_w(relative(0.8))
                .px(px(6.0))
                .py(px(2.0))
                .rounded(px(4.0))
                .bg(self.colors.background)
                .text_color(self.colors.foreground)
                .text_size(px(11.0))
                .text_ellipsis()
                .child(parts.join(" · "))
                .into_any_element(),
        )
    }

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
    fn blocked_children_take_priority_and_clear_restores_osc9_progress() {
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
        assert_eq!(label(selected), "Approval needed");
        assert_eq!(selected.app.as_deref(), Some("deploy"));
        assert_eq!(pane.effective_progress_state(), ProgressState::Warning(70));
        engine.feed(b"\x1b]7501;state=clear:id=eu\x07");
        pane.program_status = engine.program_status();
        assert_eq!(
            pane.effective_progress_state(),
            ProgressState::InProgress(20)
        );
        engine.feed(b"\x1b]7501;state=working\x07");
        pane.program_status = engine.program_status();
        assert_eq!(
            pane.effective_progress_state(),
            ProgressState::Indeterminate
        );
        engine.feed(b"\x1b]7501;state=done\x07");
        pane.program_status = engine.program_status();
        assert_eq!(label(pane.active_program_status().unwrap()), "Done");
        assert_eq!(pane.effective_progress_state(), ProgressState::Clear);
        pane.program_status.clear();
        assert_eq!(
            pane.effective_progress_state(),
            ProgressState::InProgress(10)
        );
    }

    #[test]
    fn status_chrome_disarms_invisible_formatting_and_keeps_literal_text() {
        assert_eq!(
            visible_text("hello\u{202e}txt\u{2066} <b>world</b>"),
            "hellotxt <b>world</b>"
        );
    }
}
