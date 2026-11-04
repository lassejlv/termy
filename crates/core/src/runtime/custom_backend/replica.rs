//! Display terminals as replicas: snapshots, replies returned from the
//! feed, and a mode that parses queries without answering them.

use super::{CustomBackend, Engine, TerminalSize, cursor_shape, engine};
use crate::TerminalProtocolReplies;

impl CustomBackend {
    /// Display terminals: parse output and return every queued protocol
    /// reply, in the order the engine produced them.
    pub(in crate::runtime) fn feed_output_with_replies(&self, bytes: &[u8]) -> Vec<u8> {
        self.feed_output(bytes);
        if self.transport.is_some() {
            return Vec::new();
        }
        std::mem::take(&mut self.shared.state().replies)
    }

    /// Replicas parse queries without answering them; multiplexer panes
    /// answer only what the multiplexer passes through.
    pub(in crate::runtime) fn set_protocol_replies(&self, replies: TerminalProtocolReplies) {
        let mut state = self.shared.state();
        state.replies_enabled = replies != TerminalProtocolReplies::None;
        state.passthrough_replies_only = replies == TerminalProtocolReplies::Passthrough;
        let passthrough_only = state.passthrough_replies_only;
        state.engine.set_passthrough_replies_only(passthrough_only);
        if !state.replies_enabled {
            state.replies.clear();
            state.engine.discard_replies();
        }
    }

    pub(in crate::runtime) fn encode_snapshot(&self) -> Vec<u8> {
        self.shared.state().engine.encode_snapshot()
    }

    /// Replace the terminal state with a snapshot's. The terminal keeps its
    /// configured query colors, default cursor style and cell size.
    pub(in crate::runtime) fn restore_snapshot(
        &self,
        bytes: &[u8],
    ) -> Result<(), engine::SnapshotError> {
        let mut engine = Engine::decode_snapshot(bytes)?;
        let mut state = self.shared.state();
        engine.set_query_colors(state.query_colors);
        engine.set_default_cursor_shape(cursor_shape(state.default_cursor_style));
        engine.set_cell_pixels(state.size.cell_width, state.size.cell_height);
        engine.set_passthrough_replies_only(state.passthrough_replies_only);
        let actual = engine.size();
        state.engine = engine;
        state.size = TerminalSize {
            cols: actual.cols as u16,
            rows: actual.rows as u16,
            ..state.size
        };
        state.replies.clear();
        // Events and deadlines describe the replaced state. Keep only what
        // the host must still hear: clipboard invalidation, paste mode, exit.
        state.compact_saturated_events();
        state.history_deadline = None;
        state.history_pending_since = None;
        state.defer_history_compaction(std::time::Instant::now());
        state.generation = state.generation.wrapping_add(1);
        state.palette_epoch = state.palette_epoch.wrapping_add(1);
        state.force_full_damage = true;
        state.title_changed = true;
        self.shared.schedule_maintenance(&mut state);
        drop(state);
        self.shared.notify();
        Ok(())
    }
}
