//! Fuzzy subsequence matching and scoring for command palette rows.
//!
//! A query matches a row when its characters appear in order somewhere in the
//! row text ("nwtb" finds "New Tab"). Scoring favours matches that start on
//! word boundaries, run consecutively, and sit near the start of the text, so
//! callers can rank rows by relevance instead of catalog order.
//!
//! The matcher also reports which byte ranges of the text were matched so the
//! renderer can highlight them.

use std::ops::Range;

const MATCH_SCORE: i32 = 16;
const CONSECUTIVE_BONUS: i32 = 12;
const WORD_START_BONUS: i32 = 14;
/// Charged per text character skipped between two matched characters.
const GAP_PENALTY: i32 = 2;
/// Charged per text character skipped before the first matched character.
const LEADING_GAP_PENALTY: i32 = 1;
const LEADING_GAP_PENALTY_LIMIT: i32 = 12;
const PREFIX_BONUS: i32 = 8;
const EXACT_BONUS: i32 = 32;
/// Unordered term matches ("tab close" against "Close Tab") stay usable but
/// always rank below an ordered subsequence match.
const UNORDERED_PENALTY: i32 = 20;
/// Long titles lose a little score so concise rows win otherwise-equal matches.
const LENGTH_PENALTY_DIVISOR: i32 = 8;
const LENGTH_PENALTY_LIMIT: i32 = 64;
/// Bounds the O(query x text) scoring table for pathological inputs.
const MAX_TEXT_CHARS: usize = 256;
const NEG: i32 = i32::MIN / 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FuzzyQuery {
    /// Whole query, lowercased with separators stripped, matched in order.
    joined: Vec<char>,
    /// Individual terms, used for unordered fallback matching.
    terms: Vec<Vec<char>>,
}

impl FuzzyQuery {
    /// Returns `None` when the query has no searchable characters, which
    /// callers treat as "match everything".
    pub(super) fn new(query: &str) -> Option<Self> {
        let terms: Vec<Vec<char>> = query
            .split(|ch: char| !ch.is_alphanumeric())
            .filter(|term| !term.is_empty())
            .map(|term| term.chars().map(lowercase_char).collect())
            .collect();

        if terms.is_empty() {
            return None;
        }

        let joined: Vec<char> = terms.iter().flatten().copied().collect();
        Some(Self { joined, terms })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FuzzyMatch {
    pub(super) score: i32,
    /// Matched byte ranges into the searched text, ascending and disjoint.
    pub(super) ranges: Vec<Range<usize>>,
}

/// Scores `text` against `query`, returning `None` when it does not match.
pub(super) fn match_text(text: &str, query: &FuzzyQuery) -> Option<FuzzyMatch> {
    let candidate = Candidate::new(text);

    if let Some(matched) = candidate.best_match(&query.joined) {
        return Some(candidate.finish(matched, &query.joined));
    }

    if query.terms.len() < 2 {
        return None;
    }

    // Ordered matching failed, so fall back to matching each term on its own.
    // This keeps word-order-insensitive queries working.
    let mut score = 0;
    let mut positions = Vec::new();
    for term in &query.terms {
        let matched = candidate.best_match(term)?;
        score += candidate.score_of(&matched, term);
        positions.extend(matched.positions);
    }
    positions.sort_unstable();
    positions.dedup();

    Some(FuzzyMatch {
        score: score - UNORDERED_PENALTY - candidate.length_penalty(),
        ranges: candidate.byte_ranges(&positions),
    })
}

struct MatchedPositions {
    positions: Vec<usize>,
    score: i32,
}

/// Precomputed per-character data for one searchable string.
struct Candidate {
    chars: Vec<char>,
    lowered: Vec<char>,
    /// Byte offset of each character, plus a trailing end offset.
    offsets: Vec<usize>,
    /// Word-start bonus for each character position.
    bonuses: Vec<i32>,
}

impl Candidate {
    fn new(text: &str) -> Self {
        let chars: Vec<char> = text.chars().take(MAX_TEXT_CHARS).collect();
        let lowered: Vec<char> = chars.iter().copied().map(lowercase_char).collect();

        let mut offsets = Vec::with_capacity(chars.len() + 1);
        let mut offset = 0;
        for ch in &chars {
            offsets.push(offset);
            offset += ch.len_utf8();
        }
        offsets.push(offset);

        let bonuses = chars
            .iter()
            .enumerate()
            .map(|(index, ch)| {
                if index == 0 {
                    return WORD_START_BONUS;
                }
                let previous = chars[index - 1];
                let word_start = !previous.is_alphanumeric()
                    || (ch.is_uppercase() && previous.is_lowercase())
                    || (ch.is_numeric() && !previous.is_numeric());
                if word_start { WORD_START_BONUS } else { 0 }
            })
            .collect();

        Self {
            chars,
            lowered,
            offsets,
            bonuses,
        }
    }

    /// Dynamic-programming search for the highest-scoring subsequence match.
    fn best_match(&self, query: &[char]) -> Option<MatchedPositions> {
        let query_len = query.len();
        let text_len = self.chars.len();
        if query_len == 0 || query_len > text_len {
            return None;
        }

        // `table[i][j]` is the best score for matching `query[..=i]` with the
        // final character landing on text position `j`.
        let mut table: Vec<Vec<i32>> = Vec::with_capacity(query_len);
        let mut previous_row: Vec<i32> = Vec::new();

        for (i, query_char) in query.iter().enumerate() {
            let mut row = vec![NEG; text_len];
            // Best score for the preceding query prefix ending before `j`,
            // already decayed by the gap penalty.
            let mut carry = NEG;
            for j in 0..text_len {
                if i == 0 {
                    carry = -(LEADING_GAP_PENALTY * j as i32).min(LEADING_GAP_PENALTY_LIMIT);
                } else if j > 0 {
                    carry = (carry - GAP_PENALTY).max(previous_row[j - 1]);
                }

                if *query_char != self.lowered[j] {
                    continue;
                }

                let mut base = carry;
                if i > 0 && j > 0 && previous_row[j - 1] > NEG / 2 {
                    base = base.max(previous_row[j - 1] + CONSECUTIVE_BONUS);
                }
                if base > NEG / 2 {
                    row[j] = base + MATCH_SCORE + self.bonuses[j];
                }
            }

            previous_row = row.clone();
            table.push(row);
        }

        let last_row = table.last()?;
        let (best_index, best_score) = last_row
            .iter()
            .enumerate()
            .filter(|(_, score)| **score > NEG / 2)
            .max_by_key(|(_, score)| **score)
            .map(|(index, score)| (index, *score))?;

        Some(MatchedPositions {
            positions: backtrack(&table, self, best_index),
            score: best_score,
        })
    }

    fn score_of(&self, matched: &MatchedPositions, query: &[char]) -> i32 {
        let mut score = matched.score;
        if matched.positions.first() == Some(&0) {
            score += PREFIX_BONUS;
        }
        if query.len() == self.chars.len() {
            score += EXACT_BONUS;
        }
        score
    }

    fn length_penalty(&self) -> i32 {
        (self.chars.len() as i32).min(LENGTH_PENALTY_LIMIT) / LENGTH_PENALTY_DIVISOR
    }

    fn finish(&self, matched: MatchedPositions, query: &[char]) -> FuzzyMatch {
        let score = self.score_of(&matched, query) - self.length_penalty();
        FuzzyMatch {
            score,
            ranges: self.byte_ranges(&matched.positions),
        }
    }

    /// Collapses matched character positions into ascending byte ranges.
    fn byte_ranges(&self, positions: &[usize]) -> Vec<Range<usize>> {
        let mut ranges: Vec<Range<usize>> = Vec::new();
        for position in positions {
            let start = self.offsets[*position];
            let end = self.offsets[position + 1];
            match ranges.last_mut() {
                Some(last) if last.end == start => last.end = end,
                _ => ranges.push(start..end),
            }
        }
        ranges
    }
}

/// Walks the scoring table backwards to recover the matched positions.
fn backtrack(table: &[Vec<i32>], candidate: &Candidate, best_index: usize) -> Vec<usize> {
    let mut positions = vec![0usize; table.len()];
    let mut column = best_index;

    for row in (0..table.len()).rev() {
        positions[row] = column;
        if row == 0 {
            break;
        }

        let target = table[row][column] - MATCH_SCORE - candidate.bonuses[column];
        let previous = &table[row - 1];
        let mut chosen = None;

        if column > 0
            && previous[column - 1] > NEG / 2
            && previous[column - 1] + CONSECUTIVE_BONUS == target
        {
            chosen = Some(column - 1);
        }

        if chosen.is_none() {
            for k in (0..column).rev() {
                if previous[k] > NEG / 2
                    && previous[k] - GAP_PENALTY * (column - 1 - k) as i32 == target
                {
                    chosen = Some(k);
                    break;
                }
            }
        }

        column = chosen.unwrap_or_else(|| column.saturating_sub(1));
    }

    positions
}

fn lowercase_char(ch: char) -> char {
    ch.to_lowercase().next().unwrap_or(ch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(text: &str, query: &str) -> Option<i32> {
        let query = FuzzyQuery::new(query)?;
        match_text(text, &query).map(|matched| matched.score)
    }

    fn ranges(text: &str, query: &str) -> Vec<Range<usize>> {
        let query = FuzzyQuery::new(query).expect("query");
        match_text(text, &query).expect("match").ranges
    }

    #[test]
    fn blank_query_has_no_matcher() {
        assert!(FuzzyQuery::new("").is_none());
        assert!(FuzzyQuery::new("   -- ").is_none());
    }

    #[test]
    fn matches_initials_across_words() {
        assert!(score("New Tab", "nt").is_some());
        assert!(score("Split Pane Right", "spr").is_some());
        assert!(score("Split Pane Right", "sprx").is_none());
    }

    #[test]
    fn prefix_beats_scattered_subsequence() {
        let prefix = score("Restart App", "re").expect("prefix match");
        let scattered = score("Check for Updates", "re").expect("scattered match");
        assert!(
            prefix > scattered,
            "prefix {prefix} should outrank scattered {scattered}"
        );
    }

    #[test]
    fn word_start_beats_mid_word_match() {
        let word_start = score("Close Tab", "ct").expect("word start match");
        let mid_word = score("Character Count", "ct").expect("mid word match");
        assert!(
            word_start > mid_word,
            "word start {word_start} should outrank mid word {mid_word}"
        );
    }

    #[test]
    fn shorter_title_wins_equal_matches() {
        let short = score("New Tab", "new").expect("short match");
        let long = score("New Browser Tab Somewhere", "new").expect("long match");
        assert!(short > long, "short {short} should outrank long {long}");
    }

    #[test]
    fn separators_in_query_are_ignored() {
        assert!(score("Tokyo Night", "tokyo-night").is_some());
        assert!(score("Tokyo Night", "tokyonight").is_some());
    }

    #[test]
    fn unordered_terms_match_below_ordered_terms() {
        let ordered = score("Close Tab", "close tab").expect("ordered match");
        let unordered = score("Close Tab", "tab close").expect("unordered match");
        assert!(
            ordered > unordered,
            "ordered {ordered} should outrank unordered {unordered}"
        );
    }

    #[test]
    fn ranges_cover_matched_characters() {
        assert_eq!(ranges("New Tab", "nt"), vec![0..1, 4..5]);
        assert_eq!(ranges("New Tab", "new"), vec![0..3]);
    }

    #[test]
    fn ranges_stay_on_char_boundaries_for_multibyte_text() {
        let text = "Café Ø Tab";
        let matched = ranges(text, "cøt");
        for range in &matched {
            assert!(text.is_char_boundary(range.start));
            assert!(text.is_char_boundary(range.end));
        }
        let highlighted: String = matched
            .iter()
            .map(|range| &text[range.clone()])
            .collect::<Vec<_>>()
            .concat();
        assert_eq!(highlighted, "CØT");
    }

    #[test]
    fn case_insensitive_matching() {
        assert!(score("Toggle Fullscreen", "FULL").is_some());
        assert!(score("TOGGLE FULLSCREEN", "full").is_some());
    }

    #[test]
    fn exact_title_outranks_longer_container() {
        let exact = score("nord", "nord").expect("exact match");
        let container = score("nord light variant", "nord").expect("container match");
        assert!(exact > container);
    }
}
