use std::{cmp::Ordering, collections::HashMap};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchMatch {
    /// Line index in Alacritty coordinates (`0..rows` for viewport, negative for history).
    pub line: i32,
    /// Start column in terminal cell coordinates (end-exclusive with `end_col`).
    pub start_col: usize,
    /// End column in terminal cell coordinates (exclusive).
    pub end_col: usize,
}

impl SearchMatch {
    pub fn new(line: i32, start_col: usize, end_col: usize) -> Self {
        Self {
            line,
            start_col,
            end_col,
        }
    }

    pub fn contains(&self, line: i32, col: usize) -> bool {
        self.line == line && col >= self.start_col && col < self.end_col
    }
}

#[derive(Debug, Clone)]
pub struct SearchResults {
    matches: Vec<SearchMatch>,
    current_index: Option<usize>,
    match_ranges_by_line: HashMap<i32, Vec<(usize, usize)>>,
}

impl Default for SearchResults {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchResults {
    pub fn new() -> Self {
        Self {
            matches: Vec::new(),
            current_index: None,
            match_ranges_by_line: HashMap::new(),
        }
    }

    pub fn from_matches(matches: Vec<SearchMatch>) -> Self {
        let current_index = if matches.is_empty() { None } else { Some(0) };
        let match_ranges_by_line = Self::build_match_ranges_by_line(&matches);
        Self {
            matches,
            current_index,
            match_ranges_by_line,
        }
    }

    fn build_match_ranges_by_line(matches: &[SearchMatch]) -> HashMap<i32, Vec<(usize, usize)>> {
        let mut ranges_by_line: HashMap<i32, Vec<(usize, usize)>> = HashMap::new();
        for m in matches {
            ranges_by_line
                .entry(m.line)
                .or_default()
                .push((m.start_col, m.end_col));
        }
        for ranges in ranges_by_line.values_mut() {
            sort_and_merge_ranges(ranges);
        }
        ranges_by_line
    }

    pub fn count(&self) -> usize {
        self.matches.len()
    }

    pub fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }

    pub fn matches(&self) -> &[SearchMatch] {
        &self.matches
    }

    pub fn current(&self) -> Option<&SearchMatch> {
        self.current_index.and_then(|i| self.matches.get(i))
    }

    pub fn position(&self) -> Option<(usize, usize)> {
        self.current_index.map(|i| (i + 1, self.matches.len()))
    }

    pub fn next_match(&mut self) -> Option<&SearchMatch> {
        if self.matches.is_empty() {
            return None;
        }
        let next_index = match self.current_index {
            Some(i) => (i + 1) % self.matches.len(),
            None => 0,
        };
        self.current_index = Some(next_index);
        self.matches.get(next_index)
    }

    pub fn previous(&mut self) -> Option<&SearchMatch> {
        if self.matches.is_empty() {
            return None;
        }
        let prev_index = match self.current_index {
            Some(i) => {
                if i == 0 {
                    self.matches.len() - 1
                } else {
                    i - 1
                }
            }
            None => self.matches.len() - 1,
        };
        self.current_index = Some(prev_index);
        self.matches.get(prev_index)
    }

    pub fn jump_to(&mut self, index: usize) -> Option<&SearchMatch> {
        if index < self.matches.len() {
            self.current_index = Some(index);
            self.matches.get(index)
        } else {
            None
        }
    }

    pub fn jump_to_match(&mut self, line: i32, start_col: usize) -> bool {
        let Some(index) = self.matches.iter().position(|search_match| {
            search_match.line == line && search_match.start_col == start_col
        }) else {
            return false;
        };
        self.current_index = Some(index);
        true
    }

    pub fn jump_to_first(&mut self) -> Option<&SearchMatch> {
        self.jump_to(0)
    }

    pub fn jump_to_last(&mut self) -> Option<&SearchMatch> {
        let index = self.matches.len().checked_sub(1)?;
        self.jump_to(index)
    }

    pub fn jump_to_nearest(&mut self, target_line: i32) -> Option<&SearchMatch> {
        if self.matches.is_empty() {
            return None;
        }

        let index = self
            .matches
            .iter()
            .position(|m| m.line >= target_line)
            .unwrap_or(0);

        self.current_index = Some(index);
        self.matches.get(index)
    }

    pub fn is_current_match(&self, line: i32, col: usize) -> bool {
        self.current().is_some_and(|m| m.contains(line, col))
    }

    pub fn is_any_match(&self, line: i32, col: usize) -> bool {
        self.match_ranges_by_line
            .get(&line)
            .is_some_and(|ranges| ranges_contain_col(ranges, col))
    }

    pub fn matches_in_range(&self, min_line: i32, max_line: i32) -> Vec<&SearchMatch> {
        self.matches
            .iter()
            .filter(|m| m.line >= min_line && m.line <= max_line)
            .collect()
    }
}

fn sort_and_merge_ranges(ranges: &mut Vec<(usize, usize)>) {
    ranges.sort_unstable_by_key(|&(start, end)| (start, end));
    let mut write = 0usize;
    for read in 0..ranges.len() {
        let (start, end) = ranges[read];
        if start >= end {
            continue;
        }
        if write == 0 || start > ranges[write - 1].1 {
            ranges[write] = (start, end);
            write += 1;
        } else if end > ranges[write - 1].1 {
            ranges[write - 1].1 = end;
        }
    }
    ranges.truncate(write);
}

fn ranges_contain_col(ranges: &[(usize, usize)], col: usize) -> bool {
    ranges
        .binary_search_by(|(start_col, end_col)| {
            if *end_col <= col {
                Ordering::Less
            } else if *start_col > col {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        })
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_match_contains() {
        let m = SearchMatch::new(5, 10, 15);
        assert!(m.contains(5, 10));
        assert!(m.contains(5, 14));
        assert!(!m.contains(5, 15));
        assert!(!m.contains(5, 9));
        assert!(!m.contains(4, 12));
    }

    #[test]
    fn test_empty_results() {
        let results = SearchResults::new();
        assert!(results.is_empty());
        assert_eq!(results.count(), 0);
        assert!(results.current().is_none());
        assert!(results.position().is_none());
    }

    #[test]
    fn test_navigation() {
        let matches = vec![
            SearchMatch::new(0, 0, 5),
            SearchMatch::new(1, 10, 15),
            SearchMatch::new(2, 5, 10),
        ];
        let mut results = SearchResults::from_matches(matches);

        assert_eq!(results.position(), Some((1, 3)));
        assert_eq!(results.current().unwrap().line, 0);

        results.next_match();
        assert_eq!(results.position(), Some((2, 3)));
        assert_eq!(results.current().unwrap().line, 1);

        results.next_match();
        assert_eq!(results.position(), Some((3, 3)));

        results.next_match();
        assert_eq!(results.position(), Some((1, 3)));

        results.previous();
        assert_eq!(results.position(), Some((3, 3)));
    }

    #[test]
    fn match_ranges_merge_and_support_binary_lookup() {
        let results = SearchResults::from_matches(vec![
            SearchMatch::new(2, 8, 10),
            SearchMatch::new(2, 0, 3),
            SearchMatch::new(2, 2, 6),
            SearchMatch::new(2, 14, 16),
        ]);

        assert!(results.is_any_match(2, 0));
        assert!(results.is_any_match(2, 5));
        assert!(results.is_any_match(2, 9));
        assert!(!results.is_any_match(2, 6));
        assert!(!results.is_any_match(2, 13));
        assert!(results.is_any_match(2, 15));
    }

    #[test]
    fn test_jump_to_nearest() {
        let matches = vec![
            SearchMatch::new(-10, 0, 5),
            SearchMatch::new(-5, 0, 5),
            SearchMatch::new(0, 0, 5),
            SearchMatch::new(5, 0, 5),
        ];
        let mut results = SearchResults::from_matches(matches);

        results.jump_to_nearest(-7);
        assert_eq!(results.current().unwrap().line, -5);

        results.jump_to_nearest(0);
        assert_eq!(results.current().unwrap().line, 0);

        results.jump_to_nearest(100);
        assert_eq!(results.current().unwrap().line, -10);
    }

    #[test]
    fn test_jump_to_last() {
        let matches = vec![
            SearchMatch::new(-10, 0, 5),
            SearchMatch::new(-5, 0, 5),
            SearchMatch::new(0, 0, 5),
            SearchMatch::new(5, 0, 5),
        ];
        let mut results = SearchResults::from_matches(matches);

        results.jump_to_last();
        assert_eq!(results.current().unwrap().line, 5);

        results.jump_to_first();
        assert_eq!(results.current().unwrap().line, -10);
    }

    #[test]
    fn jump_to_match_restores_the_same_hit() {
        let matches = vec![
            SearchMatch::new(0, 0, 5),
            SearchMatch::new(2, 4, 8),
            SearchMatch::new(2, 10, 12),
        ];
        let mut results = SearchResults::from_matches(matches);
        assert!(results.jump_to_match(2, 10));
        assert_eq!(results.position(), Some((3, 3)));
        assert!(!results.jump_to_match(9, 0));
        assert_eq!(results.position(), Some((3, 3)));
    }
}
