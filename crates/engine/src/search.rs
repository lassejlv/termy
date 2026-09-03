//! Full-buffer terminal search contracts.

use crate::SelectionRange;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum SearchDirection {
    Forward,
    #[default]
    Backward,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SearchOptions {
    pub direction: SearchDirection,
    pub case_sensitive: bool,
    pub wrap: bool,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            direction: SearchDirection::Backward,
            case_sensitive: false,
            wrap: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SearchMatch {
    pub range: SelectionRange,
    pub display_offset: usize,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct BufferPoint {
    pub(crate) row: usize,
    pub(crate) column: usize,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct BufferRange {
    pub(crate) start: BufferPoint,
    pub(crate) end: BufferPoint,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub(crate) struct SearchState {
    query: String,
    case_sensitive: bool,
    current: Option<BufferRange>,
}

impl SearchState {
    pub(crate) fn origin(&mut self, query: &str, case_sensitive: bool) -> Option<BufferRange> {
        if self.query != query || self.case_sensitive != case_sensitive {
            query.clone_into(&mut self.query);
            self.case_sensitive = case_sensitive;
            self.current = None;
        }
        self.current
    }

    pub(crate) const fn set_current(&mut self, current: BufferRange) {
        self.current = Some(current);
    }

    pub(crate) const fn is_active(&self) -> bool {
        self.current.is_some()
    }

    pub(crate) fn reset(&mut self) {
        self.query.clear();
        self.current = None;
    }
}
