const MAX_PREEDIT_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ImeState {
    enabled: bool,
    text: String,
    cursor: Option<(usize, usize)>,
}

impl ImeState {
    pub(crate) fn enable(&mut self) {
        self.enabled = true;
    }

    pub(crate) fn disable(&mut self) {
        self.enabled = false;
        self.clear_preedit();
    }

    pub(crate) fn set_preedit(&mut self, mut text: String, cursor: Option<(usize, usize)>) {
        if text.len() > MAX_PREEDIT_BYTES {
            let mut end = MAX_PREEDIT_BYTES;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
        }
        self.cursor = cursor.map(|(start, end)| {
            let start = clamped_char_boundary(&text, start);
            let end = clamped_char_boundary(&text, end);
            (start.min(end), start.max(end))
        });
        self.text = text;
        if self.text.is_empty() {
            self.cursor = None;
        }
    }

    pub(crate) fn clear_preedit(&mut self) {
        self.text.clear();
        self.cursor = None;
    }

    pub(crate) const fn enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn preedit(&self) -> Option<(&str, Option<(usize, usize)>)> {
        (!self.text.is_empty()).then_some((self.text.as_str(), self.cursor))
    }
}

fn clamped_char_boundary(text: &str, requested: usize) -> usize {
    let mut position = requested.min(text.len());
    while !text.is_char_boundary(position) {
        position -= 1;
    }
    position
}

#[cfg(test)]
mod tests {
    use super::{ImeState, MAX_PREEDIT_BYTES};

    #[test]
    fn preedit_is_bounded_and_cursor_ranges_remain_utf8_safe() {
        let mut state = ImeState::default();
        state.enable();
        state.set_preedit(
            format!("{}界", "x".repeat(MAX_PREEDIT_BYTES)),
            Some((usize::MAX, 2)),
        );

        let (text, cursor) = state.preedit().expect("bounded preedit");
        assert_eq!(text.len(), MAX_PREEDIT_BYTES);
        assert_eq!(cursor, Some((2, MAX_PREEDIT_BYTES)));
        assert!(state.enabled());
    }

    #[test]
    fn disable_and_empty_preedit_clear_transient_composition() {
        let mut state = ImeState::default();
        state.enable();
        state.set_preedit("拼音".to_owned(), Some((3, 3)));
        state.set_preedit(String::new(), None);
        assert_eq!(state.preedit(), None);

        state.set_preedit("かな".to_owned(), Some((3, 3)));
        state.disable();
        assert!(!state.enabled());
        assert_eq!(state.preedit(), None);
    }
}
