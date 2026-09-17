use super::*;
use alacritty_terminal::{
    event::{Event as AlacrittyEvent, EventListener, WindowSize},
    grid::Dimensions,
    index::{Column, Line},
    term::{Config as AlacrittyConfig, Term as AlacrittyTerm},
    vte::ansi,
};

#[derive(Clone, Default)]
struct ReplyListener(Arc<Mutex<Vec<AlacrittyEvent>>>);

impl EventListener for ReplyListener {
    fn send_event(&self, event: AlacrittyEvent) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event);
    }
}

#[derive(Clone, Copy)]
struct AlacrittySize(Size);

impl Dimensions for AlacrittySize {
    fn total_lines(&self) -> usize {
        usize::from(self.0.rows)
    }

    fn screen_lines(&self) -> usize {
        usize::from(self.0.rows)
    }

    fn columns(&self) -> usize {
        usize::from(self.0.cols)
    }

    fn last_column(&self) -> Column {
        Column(usize::from(self.0.cols.saturating_sub(1)))
    }

    fn bottommost_line(&self) -> Line {
        Line(i32::from(self.0.rows.saturating_sub(1)))
    }

    fn topmost_line(&self) -> Line {
        Line(0)
    }
}

impl From<AlacrittySize> for WindowSize {
    fn from(size: AlacrittySize) -> Self {
        WindowSize {
            num_cols: size.0.cols,
            num_lines: size.0.rows,
            cell_width: size.0.cell_width.round().clamp(1.0, u16::MAX as f32) as u16,
            cell_height: size.0.cell_height.round().clamp(1.0, u16::MAX as f32) as u16,
        }
    }
}

fn alacritty_query_replies(bytes: &[u8], size: Size) -> Vec<u8> {
    let dimensions = AlacrittySize(size);
    let listener = ReplyListener::default();
    let mut term = AlacrittyTerm::new(
        AlacrittyConfig {
            kitty_keyboard: true,
            ..AlacrittyConfig::default()
        },
        &dimensions,
        listener.clone(),
    );
    let mut parser: ansi::Processor = ansi::Processor::new();
    parser.advance(&mut term, bytes);
    let events = std::mem::take(
        &mut *listener
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    let window_size = WindowSize::from(dimensions);
    let mut replies = Vec::new();
    for event in events {
        match event {
            AlacrittyEvent::PtyWrite(text) => replies.extend_from_slice(text.as_bytes()),
            AlacrittyEvent::TextAreaSizeRequest(formatter) => {
                replies.extend_from_slice(formatter(window_size).as_bytes());
            }
            AlacrittyEvent::ColorRequest(index, formatter) => {
                if let Some(color) = term.colors()[index] {
                    replies.extend_from_slice(formatter(color).as_bytes());
                }
            }
            _ => {}
        }
    }
    replies
}

mod events_runtime;
mod terminal;
