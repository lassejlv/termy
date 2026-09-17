use crate::search_engine::engine::{SearchConfig, SearchEngine, SearchMode};
use crate::search_engine::matcher::SearchResults;

pub struct SearchState {
    engine: SearchEngine,
    results: SearchResults,
    results_revision: u64,
    query: String,
    is_active: bool,
    error: Option<String>,
}

impl Default for SearchState {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchState {
    pub fn new() -> Self {
        Self {
            engine: SearchEngine::new(SearchConfig::default()),
            results: SearchResults::new(),
            results_revision: 0,
            query: String::new(),
            is_active: false,
            error: None,
        }
    }

    pub fn open(&mut self) {
        self.is_active = true;
    }

    pub fn hide(&mut self) {
        self.is_active = false;
        self.clear_results_preserving_query();
    }

    pub fn close(&mut self) {
        self.is_active = false;
        self.clear();
    }

    pub fn is_active(&self) -> bool {
        self.is_active
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn set_query(&mut self, query: &str) {
        if self.query != query {
            self.query.clear();
            self.query.push_str(query);
        }
        match self.engine.set_pattern(query) {
            Ok(()) => self.error = None,
            Err(e) => self.error = Some(e),
        }
    }

    pub fn clear(&mut self) {
        self.query.clear();
        let _ = self.engine.set_pattern("");
        self.results = SearchResults::new();
        self.results_revision = self.results_revision.wrapping_add(1);
        self.error = None;
    }

    pub fn clear_results_preserving_query(&mut self) {
        self.results = SearchResults::new();
        self.results_revision = self.results_revision.wrapping_add(1);
    }

    pub fn results(&self) -> &SearchResults {
        &self.results
    }

    pub fn results_revision(&self) -> u64 {
        self.results_revision
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn has_valid_pattern(&self) -> bool {
        self.engine.has_pattern()
    }

    pub fn toggle_case_sensitive(&mut self) {
        let mut config = self.config();
        config.case_sensitive = !config.case_sensitive;
        self.engine.set_config(config);
    }

    pub fn toggle_regex_mode(&mut self) {
        let mut config = self.config();
        config.mode = match config.mode {
            SearchMode::Literal => SearchMode::Regex,
            SearchMode::Regex => SearchMode::Literal,
        };
        self.engine.set_config(config);
        let query = self.query.clone();
        self.set_query(&query);
    }

    pub fn config(&self) -> SearchConfig {
        self.engine.config()
    }

    pub fn is_case_sensitive(&self) -> bool {
        self.config().case_sensitive
    }

    pub fn mode(&self) -> SearchMode {
        self.config().mode
    }

    pub fn search<'a, F>(&mut self, start_line: i32, end_line: i32, line_provider: F)
    where
        F: Fn(i32) -> Option<&'a str>,
    {
        self.results = self.engine.search(start_line, end_line, line_provider);
        self.results_revision = self.results_revision.wrapping_add(1);
    }

    pub fn next_match(&mut self) {
        self.results.next_match();
    }

    pub fn previous_match(&mut self) {
        self.results.previous();
    }

    pub fn jump_to_nearest(&mut self, line: i32) {
        self.results.jump_to_nearest(line);
    }

    pub fn jump_to_first(&mut self) {
        self.results.jump_to_first();
    }

    pub fn restore_current_match(&mut self, line: i32, start_col: usize) -> bool {
        self.results.jump_to_match(line, start_col)
    }

    pub fn jump_to_last(&mut self) {
        self.results.jump_to_last();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_revision_changes_on_search_and_clear() {
        let mut state = SearchState::new();
        let baseline = state.results_revision();

        state.search(0, 2, |line| match line {
            0 => Some("alpha"),
            1 => Some("beta"),
            2 => Some("gamma"),
            _ => None,
        });
        assert_eq!(state.results_revision(), baseline.wrapping_add(1));

        state.clear();
        assert_eq!(state.results_revision(), baseline.wrapping_add(2));
    }

    #[test]
    fn results_revision_does_not_change_for_selection_navigation() {
        let mut state = SearchState::new();
        state.search(0, 3, |line| match line {
            0 => Some("match"),
            1 => Some("x"),
            2 => Some("match"),
            3 => Some("y"),
            _ => None,
        });

        let revision = state.results_revision();
        state.next_match();
        state.previous_match();
        state.jump_to_nearest(10);

        assert_eq!(state.results_revision(), revision);
    }

    #[test]
    fn hide_keeps_query_and_clears_results() {
        let mut state = SearchState::new();
        state.open();
        state.set_query("match");
        state.search(0, 0, |_| Some("match"));
        assert!(!state.results().is_empty());

        state.hide();
        assert!(!state.is_active());
        assert_eq!(state.query(), "match");
        assert!(state.results().is_empty());
        assert!(state.has_valid_pattern());
    }

    #[test]
    fn close_advances_results_revision_via_clear() {
        let mut state = SearchState::new();
        state.open();
        let baseline = state.results_revision();
        state.close();
        assert_eq!(state.results_revision(), baseline.wrapping_add(1));
    }

    #[test]
    fn clear_results_preserving_query_keeps_query_and_error() {
        let mut state = SearchState::new();
        state.set_query("match");
        state.search(0, 1, |line| match line {
            0 => Some("match"),
            1 => Some("x"),
            _ => None,
        });
        assert!(!state.results().is_empty());

        let revision = state.results_revision();
        state.clear_results_preserving_query();
        assert_eq!(state.query(), "match");
        assert!(state.error().is_none());
        assert!(state.results().is_empty());
        assert_eq!(state.results_revision(), revision.wrapping_add(1));

        state.toggle_regex_mode();
        state.set_query("[");
        assert!(state.error().is_some());
        let error = state.error().map(str::to_string);

        let revision = state.results_revision();
        state.clear_results_preserving_query();
        assert_eq!(state.query(), "[");
        assert_eq!(state.error().map(str::to_string), error);
        assert!(state.results().is_empty());
        assert_eq!(state.results_revision(), revision.wrapping_add(1));
    }

    #[test]
    fn toggle_case_sensitive_updates_config_and_recompiles_pattern() {
        let mut state = SearchState::new();
        state.set_query("hello");

        state.search(0, 0, |_| Some("Hello"));
        assert_eq!(state.results().count(), 1);

        state.toggle_case_sensitive();
        assert!(state.is_case_sensitive());

        state.search(0, 0, |_| Some("Hello"));
        assert_eq!(state.results().count(), 0);
    }

    #[test]
    fn toggle_regex_mode_updates_config_and_recompiles_pattern() {
        let mut state = SearchState::new();
        state.set_query("a.c");

        state.search(0, 0, |_| Some("abc"));
        assert_eq!(state.results().count(), 0);

        state.toggle_regex_mode();
        assert_eq!(state.mode(), SearchMode::Regex);

        state.search(0, 0, |_| Some("abc"));
        assert_eq!(state.results().count(), 1);
    }
}
