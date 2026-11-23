//! Grid storage for engine snapshots.
//!
//! A row stores its explicit cells (everything before its trailing erase
//! blanks) as style runs, scalars and sparse wide-cell flags, followed by its
//! metadata by column, the packed flag and the storage capacities. History
//! keeps its metadata charge but is packed as it is read, with buffers sized
//! to their contents: a snapshot can't make a few bytes take a whole row.
//! History's byte charge doesn't depend on the representation (see
//! `history.rs`), so a decoded grid evicts history like the original either
//! way.

use std::sync::Arc;

use super::super::{
    CellExtra, Cursor,
    snapshot::{Reader, Result, SnapshotError::Corrupt, V1_MAX_HISTORY_ROWS, Writer},
    types::{Pen, Size},
};
use super::{
    Cell, Grid, Row, SavedCursor, Screen,
    history::{DEFAULT_HISTORY_BYTES, history_charge, history_heap},
    ring::Ring,
};

const WRAPPED: u8 = 1;
const PACKED: u8 = 1 << 1;
/// The smallest row encoding: a blank row of fewer than 128 columns.
const MIN_ROW_BYTES: usize = 15;
const CELL_FLAGS: u16 = Cell::WIDE | Cell::WIDE_SPACER | Cell::LEADING_WIDE_SPACER;
/// The largest history byte limit a decoded snapshot's memory is bounded
/// by (256 MiB); snapshots may name a larger or unlimited one.
const MAX_DECODED_HISTORY_BYTES: usize = 16 * DEFAULT_HISTORY_BYTES;
/// What decoded history may take per byte of its limit. A row takes at most
/// four times its charge once packed (a row that resists packing is stored
/// dense), and history may hold more than its limit after a widen until the
/// next scroll (see `Grid::resize`).
const DECODED_HISTORY_SLACK: usize = 8;

fn write_pen(writer: &mut Writer, pen: &Pen) {
    writer.style(pen.style);
    writer.option_extra(pen.extra.as_ref());
}

fn read_pen(reader: &mut Reader<'_>) -> Result<Pen> {
    Ok(Pen {
        style: reader.style()?,
        extra: reader.option_extra()?,
    })
}

fn check_cursor(cursor: Cursor, size: Size) -> Result<Cursor> {
    if cursor.row >= size.rows || cursor.col >= size.cols {
        return Err(Corrupt("cursor position"));
    }
    Ok(cursor)
}

impl Row {
    fn write_snapshot(&self, writer: &mut Writer, scratch: &mut Vec<Cell>) {
        let canonical = writer.canonical();
        self.with_cells(scratch, |cells| {
            let blank = Grid::blank_on(self.clear_background);
            let extras_end = self.extras.last().map_or(0, |(col, _)| col + 1);
            let explicit = if canonical {
                cells
                    .iter()
                    .rposition(|cell| *cell != blank)
                    .map_or(0, |index| index + 1)
                    .max(extras_end)
            } else {
                // Cells past `occupied` are erase blanks; check rather than
                // trust it, since a stale bound only costs bytes here.
                let occupied = self.occupied.min(cells.len());
                if extras_end <= occupied && cells[occupied..].iter().all(|cell| *cell == blank) {
                    occupied
                } else {
                    cells.len()
                }
            };
            let explicit_cells = &cells[..explicit];

            writer.usize(cells.len());
            let mut flags = 0;
            if self.wrapped {
                flags |= WRAPPED;
            }
            if self.packed.is_some() && !canonical {
                flags |= PACKED;
            }
            writer.u8(flags);
            writer.color(self.clear_background);
            writer.usize(if canonical { explicit } else { self.occupied });
            writer.usize(explicit);

            let runs = explicit_cells.chunk_by(|a, b| a.style == b.style).count();
            writer.usize(runs);
            for run in explicit_cells.chunk_by(|a, b| a.style == b.style) {
                writer.usize(run.len());
                writer.style(run[0].style);
            }
            for cell in explicit_cells {
                writer.varint(u64::from(u32::from(cell.character)));
            }
            let flagged = explicit_cells.iter().filter(|cell| cell.flags != 0).count();
            writer.usize(flagged);
            let mut previous = 0;
            for (col, cell) in explicit_cells.iter().enumerate() {
                if cell.flags != 0 {
                    writer.usize(col - previous);
                    writer.varint(u64::from(cell.flags));
                    previous = col;
                }
            }
        });
        writer.usize(self.extras.len());
        let mut previous = 0;
        for (col, extra) in &self.extras {
            writer.usize(col - previous);
            let index = writer.extra(extra);
            writer.varint(u64::from(index));
            previous = *col;
        }
        let canonical = writer.canonical();
        writer.varint(if canonical {
            0
        } else {
            u64::from(self.extra_units)
        });
        for capacity in self.storage_capacities() {
            writer.usize(if canonical { 0 } else { capacity });
        }
    }

    /// Screen rows (`screen`) must be dense.
    fn read_snapshot(reader: &mut Reader<'_>, width: usize, screen: bool) -> Result<Self> {
        let len = reader.usize()?;
        if len == 0 || len > Size::MAX_COLS || len != width {
            return Err(Corrupt("row width"));
        }
        let flags = reader.u8()?;
        // Screen rows are always dense: writes index their cells directly.
        if flags & !(WRAPPED | PACKED) != 0 || (screen && flags & PACKED != 0) {
            return Err(Corrupt("row flags"));
        }
        let clear_background = reader.color()?;
        let occupied = reader.usize()?;
        let explicit = reader.usize()?;
        if occupied > len || explicit > len {
            return Err(Corrupt("row length"));
        }

        let mut cells = Vec::with_capacity(len);
        let runs = reader.count(11)?;
        for _ in 0..runs {
            let run = reader.usize()?;
            let style = reader.style()?;
            if run == 0 || run > explicit - cells.len() {
                return Err(Corrupt("style run"));
            }
            cells.resize(
                cells.len() + run,
                Cell {
                    style,
                    ..Cell::default()
                },
            );
        }
        if cells.len() != explicit {
            return Err(Corrupt("style runs"));
        }
        for cell in &mut cells {
            cell.character = u32::try_from(reader.varint()?)
                .ok()
                .and_then(char::from_u32)
                .ok_or(Corrupt("character"))?;
        }
        let flagged = reader.count(2)?;
        let mut col = 0_usize;
        for index in 0..flagged {
            let delta = reader.usize()?;
            if index != 0 && delta == 0 {
                return Err(Corrupt("cell flags order"));
            }
            col = col.checked_add(delta).ok_or(Corrupt("cell flags"))?;
            let value = reader.u16_value()?;
            let cell = cells.get_mut(col).ok_or(Corrupt("cell flags column"))?;
            if value == 0 || value & !CELL_FLAGS != 0 {
                return Err(Corrupt("cell flags"));
            }
            cell.flags = value;
        }
        // Printing and reflow index a wide glyph's spacer through its glyph
        // and back, so a broken pair would panic later instead of here.
        for (col, cell) in cells.iter().enumerate() {
            let broken = (cell.flags & Cell::WIDE != 0
                && cells
                    .get(col + 1)
                    .is_none_or(|next| next.flags & Cell::WIDE_SPACER == 0))
                || (cell.flags & Cell::WIDE_SPACER != 0
                    && (col == 0 || cells[col - 1].flags & Cell::WIDE == 0))
                || (cell.flags & Cell::LEADING_WIDE_SPACER != 0
                    && (flags & WRAPPED == 0 || col + 1 != len));
            if broken {
                return Err(Corrupt("wide cell"));
            }
        }
        cells.resize(len, Grid::blank_on(clear_background));

        let count = reader.count(2)?;
        let mut extras: Vec<(usize, Arc<CellExtra>)> = Vec::with_capacity(count);
        let mut col = 0_usize;
        for index in 0..count {
            let delta = reader.usize()?;
            if index != 0 && delta == 0 {
                return Err(Corrupt("cell metadata order"));
            }
            col = col.checked_add(delta).ok_or(Corrupt("cell metadata"))?;
            // Encoders count metadata columns as explicit cells.
            if col >= explicit {
                return Err(Corrupt("cell metadata column"));
            }
            extras.push((col, reader.extra()?));
        }
        let extra_units = reader.u16_value()?;
        // Buffers are sized to what the row holds: capacities from the
        // snapshot would let a few bytes reserve whole rows of memory.
        for _ in 0..3 {
            reader.usize()?;
        }

        let mut row = Self {
            cells,
            extras,
            packed: None,
            // Everything past the explicit cells is erase blanks, so this
            // bound is exact; a stale one would let a recycled row keep this
            // row's text.
            occupied: explicit,
            clear_background,
            wrapped: flags & WRAPPED != 0,
            // Snapshots don't carry prompt marks.
            prompt: 0,
            extra_units,
        };
        if !screen {
            // History is packed as it is read, so a row takes memory for
            // the cells it holds rather than for its width. Packing measures
            // the metadata; keep the original's charge.
            row.compact();
            row.extra_units = extra_units;
        }
        Ok(row)
    }
}

impl Screen {
    fn write_snapshot(&self, writer: &mut Writer, scratch: &mut Vec<Cell>) {
        writer.usize(self.rows.len());
        for row in &self.rows {
            row.write_snapshot(writer, scratch);
        }
        let saved = &self.saved;
        writer.cursor(saved.cursor);
        write_pen(writer, &saved.pen);
        writer.bool(saved.pending_wrap);
        writer.bool(saved.origin_mode);
        writer.bool(saved.autowrap);
        writer.bool(saved.valid);
        writer.cursor(self.cursor);
        writer.bool(self.pending_wrap);
    }

    fn read_snapshot(reader: &mut Reader<'_>, size: Size) -> Result<Self> {
        if reader.usize()? != size.rows {
            return Err(Corrupt("screen height"));
        }
        let mut rows = Ring::with_capacity(size.rows);
        for _ in 0..size.rows {
            rows.push_back(Row::read_snapshot(reader, size.cols, true)?);
        }
        let saved = SavedCursor {
            cursor: reader.cursor()?,
            pen: read_pen(reader)?,
            pending_wrap: reader.bool()?,
            origin_mode: reader.bool()?,
            autowrap: reader.bool()?,
            valid: reader.bool()?,
        };
        Ok(Self {
            rows,
            saved,
            cursor: check_cursor(reader.cursor()?, size)?,
            pending_wrap: reader.bool()?,
        })
    }
}

impl Grid {
    /// A rough encoded size, to reserve the output once.
    pub(in crate::terminal_engine) fn snapshot_size_hint(&self) -> usize {
        let rows = self.history.len() + 2 * self.size.rows;
        rows.saturating_mul(self.size.cols.min(160) + 24)
    }

    #[cfg(test)]
    pub(in crate::terminal_engine) fn history_bytes(&self) -> usize {
        self.history_bytes
    }

    pub(in crate::terminal_engine) fn write_snapshot(&self, writer: &mut Writer) {
        let mut scratch = Vec::new();
        writer.usize(self.size.cols);
        writer.usize(self.size.rows);
        // Version 1 readers keep at most 20,000 rows; send them the newest.
        let (limit, skipped) = if writer.version() < 2 {
            let limit = self.history_limit.min(V1_MAX_HISTORY_ROWS);
            (limit, self.history.len().saturating_sub(limit))
        } else {
            (self.history_limit, 0)
        };
        let history = self.history.len() - skipped;
        writer.usize(limit);
        if writer.version() >= 2 {
            writer.usize(self.history_byte_limit);
        }
        writer.bool(self.alternate_active);
        self.primary.write_snapshot(writer, &mut scratch);
        writer.bool(self.alternate.is_some());
        if let Some(alternate) = &self.alternate {
            alternate.write_snapshot(writer, &mut scratch);
        }
        writer.usize(history);
        for row in self.history.iter().skip(skipped) {
            row.write_snapshot(writer, &mut scratch);
        }
        writer.usize(self.pending_compaction.min(history));
        writer.bool(self.compact_on_scroll);
        writer.usize(self.output_since_compaction);
        writer.usize(self.display_offset.min(history));
        writer.u64(self.scrolled_into_history);
        writer.usize(self.tabs.len());
        for stops in self.tabs.chunks(8) {
            let byte = stops
                .iter()
                .enumerate()
                .fold(0, |byte, (bit, stop)| byte | (u8::from(*stop) << bit));
            writer.u8(byte);
        }
        writer.usize(self.scroll_top);
        writer.usize(self.scroll_bottom);
        writer.usize(self.left_margin);
        writer.usize(self.right_margin);
        writer.bool(self.lr_margin_mode);
        writer.bool(self.pending_wrap);
        writer.bool(self.clear_anchor);
        writer.bool(self.grapheme_open);
        writer.bool(self.grapheme_ordinary);
        writer.bool(self.grapheme_at_margin);
        writer.cursor(self.cursor);
        write_pen(writer, &self.pen);
        writer.bool(self.autowrap);
        writer.bool(self.origin_mode);
        writer.bool(self.insert_mode);
        writer.bool(self.newline_mode);
    }

    pub(in crate::terminal_engine) fn read_snapshot(reader: &mut Reader<'_>) -> Result<Self> {
        let size = Size {
            cols: reader.usize()?,
            rows: reader.usize()?,
        };
        if size.clamped() != size {
            return Err(Corrupt("grid size"));
        }
        let limit = reader.limit()?;
        let byte_limit = if reader.version() >= 2 {
            reader.limit()?
        } else {
            DEFAULT_HISTORY_BYTES
        };
        let mut grid = Self::with_limits(size, limit, byte_limit);
        grid.alternate_active = reader.bool()?;
        grid.primary = Screen::read_snapshot(reader, size)?;
        grid.alternate = if reader.bool()? {
            Some(Screen::read_snapshot(reader, size)?)
        } else {
            None
        };
        if grid.alternate_active && grid.alternate.is_none() {
            return Err(Corrupt("alternate screen"));
        }

        let history = reader.count(MIN_ROW_BYTES)?;
        if history > grid.history_limit {
            return Err(Corrupt("history length"));
        }
        // Bound what decoding allocates by its output, not its input: the
        // limit comes from the snapshot too, so it is clamped.
        let budget = grid
            .history_byte_limit
            .min(MAX_DECODED_HISTORY_BYTES)
            .saturating_mul(DECODED_HISTORY_SLACK);
        let mut heap = 0_usize;
        let mut rows = Vec::with_capacity(history);
        for _ in 0..history {
            // Resizes give history rows the grid's width, and moving one onto
            // the screen relies on it.
            let row = Row::read_snapshot(reader, size.cols, false)?;
            heap += history_heap(&row);
            if heap > budget {
                return Err(Corrupt("history size"));
            }
            rows.push(row);
        }
        grid.history = rows.into_iter().collect::<Ring<Row>>();
        grid.history_bytes = grid.history.iter().map(history_charge).sum();
        grid.pending_compaction = reader.usize()?.min(history);
        grid.compact_on_scroll = reader.bool()?;
        grid.output_since_compaction = reader.usize()?;
        grid.display_offset = reader.usize()?;
        if grid.display_offset > history {
            return Err(Corrupt("display offset"));
        }
        grid.scrolled_into_history = reader.u64()?;

        let tabs = reader.usize()?;
        if tabs < size.cols || tabs > Size::MAX_COLS {
            return Err(Corrupt("tab stops"));
        }
        let bytes = reader.take(tabs.div_ceil(8))?;
        grid.tabs = (0..tabs)
            .map(|col| bytes[col / 8] & (1 << (col % 8)) != 0)
            .collect();
        grid.scroll_top = reader.usize()?;
        grid.scroll_bottom = reader.usize()?;
        grid.left_margin = reader.usize()?;
        grid.right_margin = reader.usize()?;
        grid.lr_margin_mode = reader.bool()?;
        // Like DECSLRM, a left/right margin region is at least two columns
        // wide, and only DECLRMM narrows it from the full width: printing a
        // wide glyph into a narrower one would index past it.
        let lr_margins_valid = grid.margins_span_screen()
            || (grid.lr_margin_mode
                && grid.right_margin <= size.cols
                && grid
                    .right_margin
                    .checked_sub(grid.left_margin)
                    .is_some_and(|width| width >= 2));
        if grid.scroll_top >= grid.scroll_bottom
            || grid.scroll_bottom > size.rows
            || !lr_margins_valid
        {
            return Err(Corrupt("margins"));
        }
        grid.pending_wrap = reader.bool()?;
        grid.clear_anchor = reader.bool()?;
        grid.grapheme_open = reader.bool()?;
        grid.grapheme_ordinary = reader.bool()?;
        grid.grapheme_at_margin = reader.bool()?;
        grid.cursor = check_cursor(reader.cursor()?, size)?;
        grid.pen = read_pen(reader)?;
        grid.autowrap = reader.bool()?;
        grid.origin_mode = reader.bool()?;
        grid.insert_mode = reader.bool()?;
        grid.newline_mode = reader.bool()?;
        // History over its budget (after a widen, or from a release that
        // charged rows differently) is evicted by the next scroll, as in the
        // original. Packing changes no charge.
        grid.pack_history_to_budget();
        grid.mark_full_damage();
        Ok(grid)
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::{
        Engine, Options,
        snapshot::SnapshotError::{self, Corrupt},
    };
    use super::*;

    fn engine(cols: usize, rows: usize, history: usize) -> Engine {
        Engine::new(
            Size { cols, rows },
            Options {
                scrollback_history: history,
                ..Default::default()
            },
        )
    }

    fn decode(engine: &mut Engine) -> std::result::Result<Engine, SnapshotError> {
        Engine::decode_snapshot(&engine.encode_snapshot())
    }

    fn text(row: &Row) -> String {
        row.cells().iter().map(|cell| cell.character).collect()
    }

    #[test]
    fn blank_history_rows_take_the_smallest_row_encoding() {
        let mut source = engine(80, 5, 5000);
        source.feed(&b"\n".repeat(3000));
        assert_eq!(source.history_size(), 2996);
        let mut decoded = decode(&mut source).expect("decode");
        assert_eq!(decoded.history_size(), source.history_size());
        assert_eq!(decoded.canonical_snapshot(), source.canonical_snapshot());

        // The bound is tight: one more blank row takes exactly that much.
        let before = source.encode_snapshot().len();
        source.feed(b"\n");
        assert_eq!(source.encode_snapshot().len(), before + MIN_ROW_BYTES);
    }

    #[test]
    fn packed_screen_rows_are_rejected() {
        let mut source = engine(40, 5, 100);
        source.feed(b"some text");
        source.state.grid.primary.rows[0].compact();
        assert!(source.state.grid.primary.rows[0].is_packed());
        assert_eq!(
            decode(&mut source).err(),
            Some(Corrupt("row flags")),
            "a packed screen row would panic on the next write"
        );
    }

    #[test]
    fn history_rows_of_another_width_are_rejected() {
        let mut source = engine(40, 5, 100);
        source.feed(&b"line\r\n".repeat(20));
        source.state.grid.history[0] = Row::new(10, &Cell::default());
        assert_eq!(decode(&mut source).err(), Some(Corrupt("row width")));
    }

    #[test]
    fn left_right_margins_narrower_than_two_columns_are_rejected() {
        for (cols, mode, left, right, valid) in [
            // DECSLRM needs two columns, so the next wide glyph fits.
            (2, true, 1, 2, false),
            (3, true, 2, 3, false),
            (4, true, 1, 3, true),
            // Without DECLRMM the margins span the screen.
            (4, false, 1, 3, false),
            (4, false, 0, 3, false),
            (4, false, 0, 4, true),
            (4, true, usize::MAX, 4, false),
            // A one-column screen's full-width margins are one column wide.
            (1, false, 0, 1, true),
            (1, true, 0, 1, true),
        ] {
            let mut source = engine(cols, 3, 10);
            source.state.grid.lr_margin_mode = mode;
            source.state.grid.left_margin = left;
            source.state.grid.right_margin = right;
            let context = format!("{cols} columns, DECLRMM {mode}, margins {left}..{right}");
            match decode(&mut source) {
                Ok(mut decoded) => {
                    assert!(valid, "{context} decoded");
                    decoded.feed("\x1b[H界界界\r\n界".as_bytes());
                }
                Err(error) => {
                    assert!(!valid, "{context}: {error}");
                    assert_eq!(error, Corrupt("margins"), "{context}");
                }
            }
        }
    }

    #[test]
    fn unpaired_wide_cells_are_rejected() {
        let mut source = engine(6, 3, 10);
        source.feed("界 x".as_bytes());
        assert!(decode(&mut source).is_ok());
        let row = &source.state.grid.primary.rows[0];
        assert_eq!(row.cells[0].flags, Cell::WIDE);
        assert_eq!(row.cells[1].flags, Cell::WIDE_SPACER);
        for (col, flags, wrapped) in [
            // A glyph without its spacer, and a spacer without its glyph.
            (1, 0, false),
            (0, 0, false),
            (3, Cell::WIDE, false),
            (2, Cell::WIDE_SPACER, false),
            // A leading spacer only ends a wrapped row.
            (5, Cell::LEADING_WIDE_SPACER, false),
            (4, Cell::LEADING_WIDE_SPACER, true),
        ] {
            let mut damaged = engine(6, 3, 10);
            damaged.feed("界 x".as_bytes());
            let row = &mut damaged.state.grid.primary.rows[0];
            row.cells[col].flags = flags;
            row.cells[col].character = 'y';
            row.occupied = row.cells.len();
            row.wrapped = wrapped;
            assert_eq!(
                decode(&mut damaged).err(),
                Some(Corrupt("wide cell")),
                "column {col} flags {flags:#x}"
            );
        }
        // A wrapped row may end in a leading spacer.
        let mut wrapped = engine(6, 3, 10);
        wrapped.feed("abcde界".as_bytes());
        let row = &wrapped.state.grid.primary.rows[0];
        assert!(row.wrapped);
        assert_eq!(row.cells[5].flags, Cell::LEADING_WIDE_SPACER);
        assert!(decode(&mut wrapped).is_ok());
    }

    /// The audit's amplification probe: blank rows of the widest screen take
    /// a few bytes each, and decoding used to expand each to its full width
    /// and to the buffer capacities the snapshot named.
    #[test]
    fn decoded_history_takes_memory_in_proportion_to_the_snapshot() {
        const ROWS: usize = 2000;
        let unlimited = Options {
            scrollback_history: usize::MAX,
            scrollback_bytes: usize::MAX,
            ..Options::default()
        };
        let mut source = Engine::new(
            Size {
                cols: Size::MAX_COLS,
                rows: 1,
            },
            unlimited,
        );
        source.feed(&b"\n".repeat(ROWS));
        assert_eq!(source.history_size(), ROWS);
        for row in &mut source.state.grid.history {
            // Dense, with spare capacity in every buffer.
            let mut dense = Row::new(Size::MAX_COLS, &Cell::default());
            dense.cells.reserve_exact(Size::MAX_COLS);
            dense.extras.reserve_exact(2 * Size::MAX_COLS);
            *row = dense;
        }
        let bytes = source.encode_snapshot();
        let decoded = Engine::decode_snapshot(&bytes).expect("decode");
        assert_eq!(decoded.history_size(), ROWS);
        let heap: usize = decoded.state.grid.history.iter().map(history_heap).sum();
        assert!(
            heap < 16 * bytes.len(),
            "{} bytes of snapshot decoded to {heap} bytes of history",
            bytes.len()
        );
    }

    #[test]
    fn history_beyond_its_byte_limit_is_rejected() {
        let mut source = engine(100, 4, 5000);
        for line in 0..2000 {
            source.feed(format!("\x1b[3{}m{line:0>99}\r\n", line % 8).as_bytes());
        }
        assert_eq!(source.history_size(), 1997);
        assert!(decode(&mut source).is_ok());
        source.state.grid.history_byte_limit = 4096;
        assert_eq!(decode(&mut source).err(), Some(Corrupt("history size")));
    }

    #[test]
    fn history_widened_past_its_byte_limit_still_decodes() {
        // A widen evicts nothing, so history's charge can pass its limit
        // until the next scroll; the snapshot must still restore it.
        let mut source = Engine::new(
            Size { cols: 20, rows: 4 },
            Options {
                scrollback_history: usize::MAX,
                scrollback_bytes: 1 << 20,
                ..Options::default()
            },
        );
        for line in 0..20_000 {
            source.feed(format!("line {line}\r\n").as_bytes());
        }
        source.resize(Size {
            cols: Size::MAX_COLS,
            rows: 4,
        });
        let history = source.history_size();
        assert!(source.state.grid.history_bytes() > source.scrollback_byte_limit());
        let mut decoded = decode(&mut source).expect("decode widened history");
        assert_eq!(decoded.history_size(), history);
        assert_eq!(decoded.canonical_snapshot(), source.canonical_snapshot());
    }

    #[test]
    fn decoded_rows_bound_their_text_and_metadata() {
        let mut source = engine(40, 5, 0);
        source.feed(b"\x1b]8;;https://example.test\x07linked\x1b]8;;\x07 stale text");
        // A stale bound below the text and its metadata.
        source.state.grid.primary.rows[0].occupied = 0;
        let mut decoded = decode(&mut source).expect("decode");
        let row = &decoded.state.grid.primary.rows[0];
        assert!(row.occupied >= "linked stale text".len());
        let _ = row.blank_suffix();
        // Scrolling recycles the row as the new bottom line, blanked.
        decoded.feed(b"\x1b[5;1H\n");
        let bottom = decoded.state.grid.primary.rows.back().unwrap();
        assert_eq!(text(bottom).trim_end(), "");
        assert!(bottom.extras.is_empty());
    }
}
