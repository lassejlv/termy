use regex::{Regex, RegexBuilder};
use unicode_width::UnicodeWidthChar;

use crate::matcher::{SearchMatch, SearchResults};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchMode {
    #[default]
    Literal,
    Regex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchConfig {
    pub case_sensitive: bool,
    pub mode: SearchMode,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            case_sensitive: false,
            mode: SearchMode::Literal,
        }
    }
}

pub struct SearchEngine {
    config: SearchConfig,
    compiled_regex: Option<Regex>,
    pattern: String,
}

impl SearchEngine {
    pub fn new(config: SearchConfig) -> Self {
        Self {
            config,
            compiled_regex: None,
            pattern: String::new(),
        }
    }

    pub fn set_pattern(&mut self, pattern: &str) -> Result<(), String> {
        if pattern == self.pattern {
            return Ok(());
        }

        self.pattern = pattern.to_string();

        if pattern.is_empty() {
            self.compiled_regex = None;
            return Ok(());
        }

        let regex_pattern = match self.config.mode {
            SearchMode::Literal => regex::escape(pattern),
            SearchMode::Regex => pattern.to_string(),
        };

        match RegexBuilder::new(&regex_pattern)
            .case_insensitive(!self.config.case_sensitive)
            .build()
        {
            Ok(regex) => {
                self.compiled_regex = Some(regex);
                Ok(())
            }
            Err(e) => {
                self.compiled_regex = None;
                Err(e.to_string())
            }
        }
    }

    pub fn set_config(&mut self, config: SearchConfig) {
        if self.config != config {
            self.config = config;
            let pattern = std::mem::take(&mut self.pattern);
            let _ = self.set_pattern(&pattern);
        }
    }

    pub fn config(&self) -> SearchConfig {
        self.config
    }

    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    pub fn has_pattern(&self) -> bool {
        self.compiled_regex.is_some()
    }

    pub fn search_line(&self, line_idx: i32, text: &str) -> Vec<SearchMatch> {
        let Some(regex) = &self.compiled_regex else {
            return Vec::new();
        };

        let mut byte_matches = regex.find_iter(text).peekable();
        if byte_matches.peek().is_none() {
            return Vec::new();
        }

        let (utf8_char_boundaries, cell_columns) = compute_cell_columns(text);

        byte_matches
            .map(|m| {
                SearchMatch::new(
                    line_idx,
                    byte_offset_to_cell_column(m.start(), &utf8_char_boundaries, &cell_columns),
                    byte_offset_to_cell_column(m.end(), &utf8_char_boundaries, &cell_columns),
                )
            })
            .collect()
    }

    pub fn search<'a, F>(&self, start_line: i32, end_line: i32, line_provider: F) -> SearchResults
    where
        F: Fn(i32) -> Option<&'a str>,
    {
        if !self.has_pattern() {
            return SearchResults::new();
        }

        let mut matches = Vec::new();

        for line_idx in start_line..=end_line {
            if let Some(text) = line_provider(line_idx) {
                if self.config.mode == SearchMode::Literal
                    && literal_line_can_skip(text, &self.pattern)
                {
                    continue;
                }
                let line_matches = self.search_line(line_idx, text);
                matches.extend(line_matches);
            }
        }

        // Reverse so index 0 = bottom (newest) match, matching terminal convention
        // where the most recent output is at the bottom.
        matches.reverse();
        SearchResults::from_matches(matches)
    }
}

fn literal_line_can_skip(text: &str, pattern: &str) -> bool {
    pattern.bytes().any(|byte| byte != b' ') && text.bytes().all(|byte| byte == b' ')
}

fn compute_cell_columns(text: &str) -> (Vec<usize>, Vec<usize>) {
    let char_count = text.chars().count() + 1;
    let mut utf8_char_boundaries = Vec::with_capacity(char_count);
    let mut cell_columns = Vec::with_capacity(char_count);
    let mut cell_col = 0usize;

    for (idx, ch) in text.char_indices() {
        utf8_char_boundaries.push(idx);
        cell_columns.push(cell_col);
        cell_col += UnicodeWidthChar::width(ch).unwrap_or(0);
    }

    utf8_char_boundaries.push(text.len());
    cell_columns.push(cell_col);

    (utf8_char_boundaries, cell_columns)
}

fn byte_offset_to_cell_column(
    byte_offset: usize,
    utf8_char_boundaries: &[usize],
    cell_columns: &[usize],
) -> usize {
    match utf8_char_boundaries.binary_search(&byte_offset) {
        Ok(index) => cell_columns[index],
        Err(0) => 0,
        Err(index) => cell_columns[index - 1],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_literal_search() {
        let mut engine = SearchEngine::new(SearchConfig::default());
        engine.set_pattern("hello").unwrap();

        let matches = engine.search_line(0, "hello world, hello!");
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].start_col, 0);
        assert_eq!(matches[0].end_col, 5);
        assert_eq!(matches[1].start_col, 13);
        assert_eq!(matches[1].end_col, 18);
    }

    #[test]
    fn test_case_insensitive() {
        let mut engine = SearchEngine::new(SearchConfig {
            case_sensitive: false,
            mode: SearchMode::Literal,
        });
        engine.set_pattern("HELLO").unwrap();

        let matches = engine.search_line(0, "Hello World");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_case_sensitive() {
        let mut engine = SearchEngine::new(SearchConfig {
            case_sensitive: true,
            mode: SearchMode::Literal,
        });
        engine.set_pattern("HELLO").unwrap();

        let matches = engine.search_line(0, "Hello World");
        assert_eq!(matches.len(), 0);

        let matches = engine.search_line(0, "HELLO World");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_regex_mode() {
        let mut engine = SearchEngine::new(SearchConfig {
            case_sensitive: false,
            mode: SearchMode::Regex,
        });
        engine.set_pattern(r"\d+").unwrap();

        let matches = engine.search_line(0, "foo 123 bar 456");
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].start_col, 4);
        assert_eq!(matches[0].end_col, 7);
    }

    #[test]
    fn test_literal_escapes_regex() {
        let mut engine = SearchEngine::new(SearchConfig {
            case_sensitive: false,
            mode: SearchMode::Literal,
        });
        // These would be regex metacharacters
        engine.set_pattern("foo.*bar").unwrap();

        // Should NOT match "fooXXXbar"
        let matches = engine.search_line(0, "fooXXXbar");
        assert_eq!(matches.len(), 0);

        // Should match literal "foo.*bar"
        let matches = engine.search_line(0, "foo.*bar");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_invalid_regex() {
        let mut engine = SearchEngine::new(SearchConfig {
            case_sensitive: false,
            mode: SearchMode::Regex,
        });
        let result = engine.set_pattern("[invalid");
        assert!(result.is_err());
        assert!(!engine.has_pattern());
    }

    #[test]
    fn test_search_with_provider() {
        let mut engine = SearchEngine::new(SearchConfig::default());
        engine.set_pattern("test").unwrap();

        let lines = [
            "line 0 with test",
            "line 1 no match",
            "line 2 test test",
            "line 3 testing",
        ];

        let results = engine.search(0, 3, |idx| lines.get(idx as usize).copied());

        assert_eq!(results.count(), 4);
    }

    #[test]
    fn test_empty_pattern() {
        let mut engine = SearchEngine::new(SearchConfig::default());
        engine.set_pattern("").unwrap();

        assert!(!engine.has_pattern());
        let results = engine.search(0, 10, |_| Some("test"));
        assert!(results.is_empty());
    }

    #[test]
    fn literal_search_skips_empty_lines() {
        let mut engine = SearchEngine::new(SearchConfig::default());
        engine.set_pattern("x").unwrap();
        let results = engine.search(0, 2, |idx| match idx {
            0 => Some(""),
            1 => Some("x"),
            2 => Some("    "),
            _ => None,
        });
        assert_eq!(results.count(), 1);
        assert_eq!(results.current().unwrap().line, 1);
    }

    #[test]
    fn literal_search_keeps_blank_lines_when_query_is_spaces() {
        let mut engine = SearchEngine::new(SearchConfig::default());
        engine.set_pattern("  ").unwrap();
        let results = engine.search(0, 1, |idx| match idx {
            0 => Some("  "),
            1 => Some("x"),
            _ => None,
        });
        assert_eq!(results.count(), 1);
        assert_eq!(results.current().unwrap().line, 0);
    }

    #[test]
    fn test_unicode_search() {
        let mut engine = SearchEngine::new(SearchConfig::default());
        engine.set_pattern("\u{1F600}").unwrap();

        let matches = engine.search_line(0, "Hello \u{1F600} World \u{1F600}");
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].start_col, 6);
        assert_eq!(matches[0].end_col, 8);
        assert_eq!(matches[1].start_col, 15);
        assert_eq!(matches[1].end_col, 17);
    }

    #[test]
    fn test_literal_search_uses_columns_with_multibyte_prefix() {
        let mut engine = SearchEngine::new(SearchConfig::default());
        engine.set_pattern("cost").unwrap();

        let matches = engine.search_line(0, "││ cost");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].start_col, 3);
        assert_eq!(matches[0].end_col, 7);
    }

    #[test]
    fn test_regex_search_uses_columns_with_multibyte_prefix() {
        let mut engine = SearchEngine::new(SearchConfig {
            case_sensitive: false,
            mode: SearchMode::Regex,
        });
        engine.set_pattern(r"co.t").unwrap();

        let matches = engine.search_line(0, "││ cost");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].start_col, 3);
        assert_eq!(matches[0].end_col, 7);
    }

    #[test]
    fn test_literal_search_uses_cell_columns_for_cjk_wide_characters() {
        let mut engine = SearchEngine::new(SearchConfig::default());
        engine.set_pattern("界").unwrap();

        let matches = engine.search_line(0, "a界b界");
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].start_col, 1);
        assert_eq!(matches[0].end_col, 3);
        assert_eq!(matches[1].start_col, 4);
        assert_eq!(matches[1].end_col, 6);
    }

    #[test]
    fn test_literal_search_uses_cell_columns_for_combining_characters() {
        let mut engine = SearchEngine::new(SearchConfig::default());
        engine.set_pattern("a\u{0301}").unwrap();

        let matches = engine.search_line(0, "x a\u{0301} z");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].start_col, 2);
        assert_eq!(matches[0].end_col, 3);
    }

    #[test]
    fn test_regex_search_uses_cell_columns_for_emoji_with_combining_mark() {
        let mut engine = SearchEngine::new(SearchConfig {
            case_sensitive: false,
            mode: SearchMode::Regex,
        });
        engine.set_pattern("\u{1F600}\u{0301}").unwrap();

        let matches = engine.search_line(0, "ab\u{1F600}\u{0301}cd");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].start_col, 2);
        assert_eq!(matches[0].end_col, 4);
    }
}
