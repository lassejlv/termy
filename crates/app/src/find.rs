const MAX_QUERY_CHARS: usize = 1_024;
#[derive(Debug, Default)]
pub(crate) struct FindState {
    active: bool,
    query: String,
    has_match: Option<bool>,
}

impl FindState {
    pub(crate) const fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) fn activate(&mut self) {
        self.active = true;
        self.has_match = None;
    }

    pub(crate) fn close(&mut self) {
        self.active = false;
        self.has_match = None;
    }

    pub(crate) fn push(&mut self, text: &str) -> bool {
        let remaining = MAX_QUERY_CHARS.saturating_sub(self.query.chars().count());
        let previous = self.query.len();
        self.query.extend(
            text.chars()
                .filter(|character| !character.is_control())
                .take(remaining),
        );
        self.has_match = None;
        self.query.len() != previous
    }

    pub(crate) fn pop(&mut self) -> bool {
        let changed = self.query.pop().is_some();
        if changed {
            self.has_match = None;
        }
        changed
    }

    pub(crate) fn set_has_match(&mut self, has_match: bool) {
        self.has_match = (!self.query.is_empty()).then_some(has_match);
    }

    pub(crate) fn invalidate_match(&mut self) {
        self.has_match = None;
    }

    pub(crate) fn query(&self) -> &str {
        &self.query
    }

    pub(crate) const fn has_match(&self) -> Option<bool> {
        self.has_match
    }
}

#[cfg(test)]
mod tests {
    use super::FindState;

    #[test]
    fn query_editing_is_unicode_safe_and_filters_controls() {
        let mut find = FindState::default();
        find.activate();
        assert!(find.push("界\n面"));
        assert_eq!(find.query(), "界面");
        assert!(find.pop());
        assert_eq!(find.query(), "界");
    }

    #[test]
    fn match_status_is_hidden_for_an_empty_query() {
        let mut find = FindState::default();
        find.activate();
        find.set_has_match(false);
        assert_eq!(find.has_match(), None);
        find.push("needle");
        find.set_has_match(true);
        assert_eq!(find.has_match(), Some(true));
        find.set_has_match(false);
        assert_eq!(find.has_match(), Some(false));
        find.close();
        assert_eq!(find.has_match(), None);
    }
}
