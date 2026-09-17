//! Terminal search utilities for Termy.

mod engine;
mod matcher;
mod state;

pub use engine::{SearchConfig, SearchEngine, SearchMode};
pub use matcher::{SearchMatch, SearchResults};
pub use state::SearchState;
