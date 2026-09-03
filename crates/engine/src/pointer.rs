//! OSC pointer-shape stack handling.

use std::collections::VecDeque;

use crate::MousePointerShape;
use serde::{Deserialize, Serialize};

pub(crate) const TERMINAL_DEFAULT_POINTER: MousePointerShape = MousePointerShape::Text;
pub(crate) const GRABBED_POINTER: MousePointerShape = MousePointerShape::Default;

const STACK_LIMIT: usize = 16;

/// The Kitty pointer stack distinguishes an empty application shape from the
/// terminal's configured default. `None` is therefore a valid stack entry.
#[derive(Debug, Default, Deserialize, Serialize)]
pub(crate) struct PointerShapeStack {
    entries: VecDeque<Option<MousePointerShape>>,
}

impl PointerShapeStack {
    pub(crate) fn current(&self) -> Option<MousePointerShape> {
        self.entries.back().copied().flatten()
    }

    pub(crate) fn host_shape(&self) -> MousePointerShape {
        self.current().unwrap_or(TERMINAL_DEFAULT_POINTER)
    }

    pub(crate) fn set(&mut self, shape: Option<MousePointerShape>) {
        if let Some(current) = self.entries.back_mut() {
            *current = shape;
        } else {
            self.entries.push_back(shape);
        }
    }

    pub(crate) fn push(&mut self, shape: MousePointerShape) {
        if self.entries.len() == STACK_LIMIT {
            self.entries.pop_front();
        }
        self.entries.push_back(Some(shape));
    }

    pub(crate) fn pop(&mut self) {
        self.entries.pop_back();
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_the_oldest_entry_at_the_sixteen_shape_limit() {
        let mut stack = PointerShapeStack::default();
        stack.push(MousePointerShape::Alias);
        for _ in 0..16 {
            stack.push(MousePointerShape::Wait);
        }
        for _ in 0..15 {
            stack.pop();
        }
        assert_eq!(stack.current(), Some(MousePointerShape::Wait));
        stack.pop();
        assert_eq!(stack.current(), None);
    }
}
