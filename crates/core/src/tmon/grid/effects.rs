#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GridEffect {
    ScrollUp {
        alternate: bool,
        top: usize,
        bottom: usize,
        count: usize,
        full_screen_region: bool,
        recorded_history: bool,
        history_before: usize,
        history_after: usize,
    },
    ScrollDown {
        alternate: bool,
        top: usize,
        bottom: usize,
        count: usize,
        history_size: usize,
    },
    EnteredAlternate,
    ExitedAlternate,
    ClearViewport {
        alternate: bool,
        history_size: usize,
        rows: usize,
        cols: usize,
    },
    RebaseHistory {
        dropped: usize,
    },
    Reset,
}
