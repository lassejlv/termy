//! Synchronized output stages bytes, never copies the screen or scrollback.
//!
//! A second streaming parser recognizes mode markers without dispatching screen
//! mutations. This keeps fragmented CSI, UTF-8 and string termination semantics
//! identical to normal parsing while holding a strictly bounded byte buffer.

use std::time::Duration;
use web_time::Instant;

use super::parser::{Handler, MAX_APC_BYTES, Param, Parser};

pub(super) const MAX_SYNC_BYTES: usize = 2 * 1024 * 1024;
pub(super) const SYNC_TIMEOUT: Duration = Duration::from_millis(150);
/// Repeated begin markers extend the timeout, but never past this hold time
/// from the first one, so a program cannot keep the screen frozen.
pub(super) const SYNC_MAX_HOLD: Duration = Duration::from_secs(1);
/// A committed update's buffer is kept for the next one up to this size; a
/// rare larger frame doesn't pin up to [`MAX_SYNC_BYTES`] per terminal.
const RETAINED_SYNC_BYTES: usize = 64 * 1024;

pub(super) struct SynchronizedUpdate {
    scanner: Parser,
    bytes: Vec<u8>,
    deadline: Option<Instant>,
    started: Option<Instant>,
    saved_mode: Option<bool>,
    /// A committed update whose replay stopped at an image upload, so it
    /// could be decoded without the engine lock: its bytes and how many of
    /// them have been applied.
    replay: Option<(Vec<u8>, usize)>,
}

impl Default for SynchronizedUpdate {
    fn default() -> Self {
        Self {
            scanner: Parser::scanner(MAX_APC_BYTES),
            bytes: Vec::new(),
            deadline: None,
            started: None,
            saved_mode: None,
            replay: None,
        }
    }
}

pub(super) struct Buffered {
    pub(super) consumed: usize,
    pub(super) commit: bool,
}

impl SynchronizedUpdate {
    pub(super) fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// Neither buffering an update nor partway through replaying one.
    #[inline]
    pub(super) fn idle(&self) -> bool {
        self.deadline.is_none() && self.replay.is_none()
    }

    pub(super) fn begin(&mut self, now: Instant, saved_mode: Option<bool>) {
        self.bytes.clear();
        self.scanner.reset();
        self.deadline = Some(now + SYNC_TIMEOUT);
        self.started = Some(now);
        self.saved_mode = saved_mode;
    }

    pub(super) fn push(&mut self, bytes: &[u8], now: Instant) -> Buffered {
        debug_assert!(self.deadline.is_some());
        let admitted = bytes.len().min(MAX_SYNC_BYTES - self.bytes.len());
        let mut marker = Marker {
            saved_mode: self.saved_mode,
            ..Marker::default()
        };
        let consumed = self.scanner.advance(&mut marker, &bytes[..admitted]);
        self.saved_mode = marker.saved_mode;
        let required = self.bytes.len() + consumed;
        if required > self.bytes.capacity() {
            let capacity = required.next_power_of_two().min(MAX_SYNC_BYTES);
            self.bytes.reserve_exact(capacity - self.bytes.len());
        }
        self.bytes.extend_from_slice(&bytes[..consumed]);
        if marker.refreshed {
            let limit = self.started.map_or(now, |started| started + SYNC_MAX_HOLD);
            self.deadline = Some((now + SYNC_TIMEOUT).min(limit.max(now)));
        }
        Buffered {
            consumed,
            commit: marker.ended || self.bytes.len() == MAX_SYNC_BYTES,
        }
    }

    pub(super) fn take_buffer(&mut self) -> Option<Vec<u8>> {
        self.deadline.take()?;
        self.scanner.reset();
        Some(std::mem::take(&mut self.bytes))
    }

    pub(super) fn recycle_buffer(&mut self, mut bytes: Vec<u8>) {
        bytes.clear();
        bytes.shrink_to(RETAINED_SYNC_BYTES);
        self.bytes = bytes;
    }

    /// Stop buffering and hold the bytes for [`Self::replay_rest`]. False
    /// when no update is pending.
    pub(super) fn begin_replay(&mut self) -> bool {
        debug_assert!(self.replay.is_none());
        let Some(bytes) = self.take_buffer() else {
            return false;
        };
        self.replay = Some((bytes, 0));
        true
    }

    pub(super) fn replaying(&self) -> bool {
        self.replay.is_some()
    }

    /// The bytes of the update being replayed that are not applied yet.
    pub(super) fn replay_rest(&self) -> &[u8] {
        self.replay
            .as_ref()
            .map_or(&[], |(bytes, applied)| &bytes[*applied..])
    }

    pub(super) fn replayed(&mut self, consumed: usize) {
        if let Some((_, applied)) = &mut self.replay {
            *applied += consumed;
        }
    }

    pub(super) fn finish_replay(&mut self) {
        if let Some((bytes, _)) = self.replay.take() {
            self.recycle_buffer(bytes);
        }
    }
}

impl SynchronizedUpdate {
    /// A pending update: its buffered bytes, marker scanner and the time
    /// left before it must commit. Timing is relative, so the decoded
    /// update commits no later than the original would have.
    ///
    /// The unapplied rest of a paused replay is written as an update that is
    /// already due, so the decoded engine applies it at its next feed. The
    /// upload the replay stopped at is being decoded outside the engine, and
    /// is left out like any upload in flight.
    pub(super) fn write_snapshot(&self, writer: &mut super::snapshot::Writer) {
        if self.replay.is_some() {
            debug_assert!(self.deadline.is_none());
            writer.bool(true);
            writer.bytes(self.replay_rest());
            self.scanner.write_snapshot(writer);
            writer.option_bool(self.saved_mode);
            writer.varint(0);
            writer.varint(0);
            return;
        }
        let Some(deadline) = self.deadline else {
            writer.bool(false);
            return;
        };
        writer.bool(true);
        writer.bytes(&self.bytes);
        self.scanner.write_snapshot(writer);
        writer.option_bool(self.saved_mode);
        if writer.canonical() {
            writer.varint(0);
            writer.varint(0);
            return;
        }
        let now = Instant::now();
        let millis = |duration: Duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
        writer.varint(millis(deadline.saturating_duration_since(now)));
        writer.varint(
            self.started
                .map_or(0, |started| millis(now.saturating_duration_since(started))),
        );
    }

    pub(super) fn read_snapshot(
        reader: &mut super::snapshot::Reader<'_>,
    ) -> super::snapshot::Result<Self> {
        use super::snapshot::SnapshotError::Corrupt;
        let mut update = Self::default();
        if !reader.bool()? {
            return Ok(update);
        }
        update.bytes = reader.bytes()?.to_vec();
        if update.bytes.len() > MAX_SYNC_BYTES {
            return Err(Corrupt("synchronized update"));
        }
        update.scanner = Parser::read_snapshot(reader)?;
        update.saved_mode = reader.option_bool()?;
        let remaining = Duration::from_millis(reader.varint()?).min(SYNC_TIMEOUT);
        let held = Duration::from_millis(reader.varint()?).min(SYNC_MAX_HOLD);
        let now = Instant::now();
        update.deadline = Some(now + remaining);
        update.started = Some(now.checked_sub(held).unwrap_or(now));
        Ok(update)
    }
}

#[derive(Default)]
struct Marker {
    refreshed: bool,
    ended: bool,
    saved_mode: Option<bool>,
}

impl Handler for Marker {
    fn print(&mut self, _character: char) {}
    fn print_ascii(&mut self, _bytes: &[u8]) {}
    fn execute(&mut self, _byte: u8) {}
    fn escape(&mut self, intermediates: &[u8], final_byte: u8) {
        // A full terminal reset also resets synchronized-update mode.
        if intermediates.is_empty() && final_byte == b'c' {
            self.ended = true;
        }
    }
    fn osc(&mut self, _bytes: &[u8]) {}
    fn dcs(&mut self, _bytes: &[u8]) {}
    fn apc(&mut self, _bytes: &mut Vec<u8>) {}

    fn csi(&mut self, params: &[Param], private: Option<u8>, intermediates: &[u8], final_byte: u8) {
        if private != Some(b'?') || !intermediates.is_empty() {
            return;
        }
        if !params
            .iter()
            .any(|param| param.value() == Some(2026) && param.subparams().is_empty())
        {
            return;
        }
        match final_byte {
            b'h' => self.refreshed = true,
            b'l' => self.ended = true,
            b's' => self.saved_mode = Some(true),
            b'r' => match self.saved_mode {
                Some(true) => self.refreshed = true,
                Some(false) => self.ended = true,
                None => {}
            },
            _ => {}
        }
    }

    fn pause_requested(&self) -> bool {
        self.ended
    }
}

#[cfg(test)]
#[path = "sync/tests.rs"]
mod tests;
