use super::*;

impl Grid {
    /// Consume complete default-style lines only in a steady-state grid.
    /// Any cursor, mode, style, partial-line, or screen ambiguity falls back to
    /// the scalar parser path.
    pub(crate) fn put_default_text_lines(&mut self, bytes: &[u8]) -> (usize, Option<char>) {
        let Some(first_control) = bytes.iter().position(|byte| *byte < 0x20 || *byte == 0x7f)
        else {
            return (0, None);
        };
        if bytes[first_control] != b'\r' || bytes.get(first_control + 1) != Some(&b'\n') {
            return (0, None);
        }
        let Some(dimensions) = self.default_line_fast_path_dimensions() else {
            return (0, None);
        };
        if first_control + 2 == bytes.len() {
            return self.put_default_text_line(bytes, dimensions, first_control);
        }

        let ascii = self.put_plain_ascii_lines(bytes, dimensions);
        if ascii.0 > 0 {
            return ascii;
        }
        let utf8 = self.put_default_utf8_lines(bytes, dimensions);
        if utf8.0 > 0 {
            return utf8;
        }
        (0, None)
    }

    fn default_line_fast_path_dimensions(&self) -> Option<(usize, usize)> {
        if self.display_offset != 0
            || self.pen() != Pen::default()
            || self.insert_mode
            || self.origin_mode
            || !self.auto_wrap
        {
            return None;
        }
        let screen = self.active();
        if screen.cursor_row != screen.rows.saturating_sub(1)
            || screen.cursor_col != 0
            || screen.wrap_pending
            || screen.scroll_top != 0
            || screen.scroll_bottom != screen.rows.saturating_sub(1)
            || screen.row_extent(screen.cursor_row) != 0
        {
            return None;
        }
        Some((screen.cols, screen.rows))
    }

    fn put_plain_ascii_lines(
        &mut self,
        bytes: &[u8],
        (cols, rows): (usize, usize),
    ) -> (usize, Option<char>) {
        let Some((consumed, line_count, last_printed, line_stride)) =
            plain_ascii_line_prefix(bytes, cols)
        else {
            return (0, None);
        };
        if line_count < rows && !self.alternate_active {
            return (0, None);
        }
        self.put_bulk_lines(
            bytes,
            FastLineBatch {
                consumed,
                line_count,
                last_printed,
                line_stride,
                encoding: FastLineEncoding::Ascii,
            },
            (cols, rows),
        )
    }

    fn put_default_utf8_lines(
        &mut self,
        bytes: &[u8],
        (cols, rows): (usize, usize),
    ) -> (usize, Option<char>) {
        let Some((consumed, line_count, last_printed, line_stride)) =
            default_text_line_prefix(bytes, cols)
        else {
            return (0, None);
        };
        if line_count < rows {
            return (0, None);
        }
        self.put_bulk_lines(
            bytes,
            FastLineBatch {
                consumed,
                line_count,
                last_printed,
                line_stride,
                encoding: FastLineEncoding::Utf8,
            },
            (cols, rows),
        )
    }

    fn put_bulk_lines(
        &mut self,
        bytes: &[u8],
        batch: FastLineBatch,
        dimensions: (usize, usize),
    ) -> (usize, Option<char>) {
        let FastLineBatch {
            consumed,
            line_count,
            last_printed,
            line_stride,
            encoding,
        } = batch;
        let (cols, rows) = dimensions;
        let alternate = self.alternate_active;
        if alternate && matches!(encoding, FastLineEncoding::Ascii) {
            return self.put_bulk_alternate_ascii_lines(
                bytes,
                FastLineBatch {
                    consumed,
                    line_count,
                    last_printed,
                    line_stride,
                    encoding,
                },
                dimensions,
            );
        }
        let history_before = self.history.len();
        let (old_cells, old_extents) = {
            let screen = self.active_mut();
            (
                std::mem::take(&mut screen.cells),
                std::mem::take(&mut screen.row_extents),
            )
        };
        let mut old_cells = old_cells.into_iter();
        let mut old_extents = old_extents.into_iter();
        let mut recycled = Vec::with_capacity(rows);
        if alternate {
            for (mut row, extent) in old_cells.zip(old_extents) {
                clear_default_row(&mut row, extent);
                recycled.push(row);
            }
        } else {
            for _ in 0..rows.saturating_sub(1) {
                let row = old_cells.next().expect("primary grid has every row");
                let extent = old_extents.next().expect("primary grid has every extent");
                let (mut row, clear_len) = self.push_history(row, extent);
                clear_default_row(&mut row, clear_len);
                recycled.push(row);
            }

            for (mut row, extent) in old_cells.zip(old_extents) {
                clear_default_row(&mut row, extent);
                recycled.push(row);
            }
        }

        let direct_history = line_count.saturating_sub(rows.saturating_sub(1));
        let mut line_offset = 0usize;
        for _ in 0..direct_history {
            let end = line_end(bytes, line_offset, consumed, line_stride);
            if !alternate {
                match encoding {
                    FastLineEncoding::Ascii => {
                        self.push_plain_history(&bytes[line_offset..end], cols);
                    }
                    FastLineEncoding::Utf8 => self.push_default_text_history(
                        std::str::from_utf8(&bytes[line_offset..end])
                            .expect("validated default text is UTF-8"),
                        cols,
                    ),
                }
            }
            line_offset = end + 2;
        }

        let mut cells = Vec::with_capacity(rows);
        let mut row_extents = Vec::with_capacity(rows);
        while line_offset < consumed {
            let end = line_end(bytes, line_offset, consumed, line_stride);
            let line = &bytes[line_offset..end];
            let mut row = recycled.pop().expect("bulk scroll recycles enough rows");
            let extent = match encoding {
                FastLineEncoding::Ascii => {
                    for (cell, byte) in row.iter_mut().zip(line) {
                        cell.character = char::from(*byte);
                    }
                    line.len()
                }
                FastLineEncoding::Utf8 => fill_default_text_row(
                    &mut row,
                    std::str::from_utf8(line).expect("validated default text is UTF-8"),
                ),
            };
            cells.push(row);
            row_extents.push(extent);
            line_offset = end + 2;
        }
        let mut blank = recycled
            .pop()
            .expect("bulk scroll retains a blank cursor row");
        blank.fill(DEFAULT_CELL);
        cells.push(blank);
        row_extents.push(0);
        debug_assert_eq!(cells.len(), rows);
        debug_assert!(recycled.is_empty());
        let screen = self.active_mut();
        screen.cells = cells;
        screen.row_extents = row_extents;
        screen.cursor_col = 0;
        screen.cursor_row = rows.saturating_sub(1);
        screen.wrap_pending = false;

        let history_after = self.history.len();
        self.effects.push_back(GridEffect::ScrollUp {
            alternate,
            top: 0,
            bottom: rows.saturating_sub(1),
            count: line_count,
            full_screen_region: true,
            recorded_history: !alternate && self.history_limit > 0,
            history_before,
            history_after,
        });
        self.damage.mark_full();
        (consumed, last_printed)
    }

    fn put_bulk_alternate_ascii_lines(
        &mut self,
        bytes: &[u8],
        batch: FastLineBatch,
        (cols, rows): (usize, usize),
    ) -> (usize, Option<char>) {
        let retained_new_lines = batch.line_count.min(rows.saturating_sub(1));
        let direct_discard = batch.line_count.saturating_sub(retained_new_lines);
        let mut line_offset = 0usize;
        for _ in 0..direct_discard {
            line_offset = line_end(bytes, line_offset, batch.consumed, batch.line_stride) + 2;
        }

        let reusable = self.active().plain_ascii_cells;
        {
            let screen = self.active_mut();
            if batch.line_count < rows {
                screen.cells.rotate_left(batch.line_count);
                screen.row_extents.rotate_left(batch.line_count);
            }
            let target_start = rows.saturating_sub(1 + retained_new_lines);
            for target in target_start..rows.saturating_sub(1) {
                let end = line_end(bytes, line_offset, batch.consumed, batch.line_stride);
                let line = &bytes[line_offset..end];
                let old_extent = screen.row_extents[target];
                fill_plain_ascii_row(&mut screen.cells[target], old_extent, line, reusable);
                screen.row_extents[target] = line.len();
                line_offset = end + 2;
            }
            let blank_target = rows.saturating_sub(1);
            let blank_extent = screen.row_extents[blank_target];
            clear_default_row(&mut screen.cells[blank_target], blank_extent);
            screen.row_extents[blank_target] = 0;
            screen.cursor_col = 0;
            screen.cursor_row = blank_target;
            screen.wrap_pending = false;
            screen.plain_ascii_cells = reusable || batch.line_count >= rows;
            debug_assert_eq!(screen.cols, cols);
        }

        let history_size = self.history.len();
        self.effects.push_back(GridEffect::ScrollUp {
            alternate: true,
            top: 0,
            bottom: rows.saturating_sub(1),
            count: batch.line_count,
            full_screen_region: true,
            recorded_history: false,
            history_before: history_size,
            history_after: history_size,
        });
        self.damage.mark_full();
        (batch.consumed, batch.last_printed)
    }

    fn put_default_text_line(
        &mut self,
        bytes: &[u8],
        (cols, rows): (usize, usize),
        end: usize,
    ) -> (usize, Option<char>) {
        if rows < 2 {
            return (0, None);
        }
        if bytes.get(end + 1) != Some(&b'\n') || end + 2 != bytes.len() {
            return (0, None);
        }
        let Ok(text) = std::str::from_utf8(&bytes[..end]) else {
            return (0, None);
        };
        let Some((used, last_printed)) = default_text_metrics(text, cols) else {
            return (0, None);
        };

        self.shift_rows_up(0, rows - 1, 1, !self.alternate_active);
        let target = rows - 2;
        debug_assert!(
            self.active()
                .row(target)
                .iter()
                .all(|cell| *cell == DEFAULT_CELL)
        );
        let decoded_used = fill_default_text_row(&mut self.active_mut().cells[target], text);
        debug_assert_eq!(decoded_used, used);
        self.active_mut().row_extents[target] = used;
        if used > 0 {
            self.damage.mark(target, 0, used - 1);
        }
        (end + 2, last_printed)
    }

    fn push_plain_history(&mut self, bytes: &[u8], width: usize) {
        if self.history_limit == 0 {
            return;
        }
        let mut storage = None;
        while self.history.len() >= self.history_limit {
            storage = Some(
                self.pop_history_front()
                    .expect("a full history contains a row to evict")
                    .into_storage(),
            );
        }
        self.history
            .push_back(HistoryRow::from_ascii(bytes, width, storage));
    }

    fn push_default_text_history(&mut self, text: &str, width: usize) {
        if self.history_limit == 0 {
            return;
        }
        let mut storage = None;
        while self.history.len() >= self.history_limit {
            storage = Some(
                self.pop_history_front()
                    .expect("a full history contains a row to evict")
                    .into_storage(),
            );
        }
        self.history
            .push_back(HistoryRow::from_default_text(text, width, storage));
    }
}

impl HistoryRow {
    fn from_default_text(text: &str, width: usize, storage: Option<Vec<u8>>) -> Self {
        debug_assert!(default_text_metrics(text, width).is_some());
        let text = text.trim_end_matches(' ');
        let mut cells = storage.unwrap_or_default();
        cells.clear();
        cells.reserve(text.len());
        let mut stored = 0usize;
        let mut characters = text.chars().peekable();
        while let Some(character) = characters.next() {
            let cell_width = if character_width(character) == 1 {
                1
            } else {
                2
            };
            if characters
                .peek()
                .is_some_and(|next| character_width(*next) == 0)
            {
                let combining = characters
                    .next()
                    .expect("peeked combining character exists");
                cells.push(HISTORY_TAG_INLINE_COMBINING);
                encode_history_character(character, &mut cells);
                encode_history_character(combining, &mut cells);
            } else {
                encode_history_character(character, &mut cells);
            }
            stored += cell_width;
            if cell_width == 2 {
                cells.push(HISTORY_TAG_WIDE_SPACER);
            }
        }
        if cells.capacity() > cells.len().saturating_mul(2).max(64) {
            cells.shrink_to_fit();
        }
        Self {
            cells,
            width: u16::try_from(width).expect("history rows fit the bounded grid width"),
            stored: u16::try_from(stored).expect("stored history fits the bounded grid width"),
            has_pooled_extras: false,
        }
    }
}

#[derive(Clone, Copy)]
enum FastLineEncoding {
    Ascii,
    Utf8,
}

#[derive(Clone, Copy)]
struct FastLineBatch {
    consumed: usize,
    line_count: usize,
    last_printed: Option<char>,
    line_stride: Option<usize>,
    encoding: FastLineEncoding,
}

fn line_end(bytes: &[u8], offset: usize, consumed: usize, line_stride: Option<usize>) -> usize {
    if let Some(line_stride) = line_stride {
        return offset + line_stride - 2;
    }
    bytes[offset..consumed]
        .iter()
        .position(|byte| *byte == b'\r')
        .map(|relative| offset + relative)
        .expect("validated default lines end in CRLF")
}

fn clear_default_row(row: &mut [Cell], extent: usize) {
    let clear_len = extent.min(row.len());
    row[..clear_len].fill(DEFAULT_CELL);
}

fn fill_plain_ascii_row(row: &mut [Cell], old_extent: usize, bytes: &[u8], reusable: bool) {
    let old_extent = old_extent.min(row.len());
    if reusable {
        if bytes.len() < old_extent {
            row[bytes.len()..old_extent].fill(DEFAULT_CELL);
        }
    } else {
        row[..old_extent].fill(DEFAULT_CELL);
    }
    for (cell, byte) in row.iter_mut().zip(bytes) {
        let character = char::from(*byte);
        if cell.character != character {
            cell.character = character;
        }
    }
}

fn default_text_line_prefix(
    bytes: &[u8],
    cols: usize,
) -> Option<(usize, usize, Option<char>, Option<usize>)> {
    let mut offset = 0usize;
    let mut complete_end = 0usize;
    let mut lines = 0usize;
    let mut last_printed = None;
    let mut line_stride = None;
    while offset < bytes.len() {
        let start = offset;
        let Some(relative_end) = bytes[offset..].iter().position(|byte| *byte == b'\r') else {
            break;
        };
        let end = offset + relative_end;
        if bytes.get(end + 1) != Some(&b'\n') {
            break;
        }
        let Ok(text) = std::str::from_utf8(&bytes[offset..end]) else {
            break;
        };
        let Some((_, line_last_printed)) = default_text_metrics(text, cols) else {
            break;
        };
        offset = end + 2;
        complete_end = offset;
        lines += 1;
        let stride = offset - start;
        line_stride = match line_stride {
            None => Some(stride),
            Some(previous) if previous == stride => Some(previous),
            Some(_) => Some(0),
        };
        if line_last_printed.is_some() {
            last_printed = line_last_printed;
        }
    }
    (lines > 0).then_some((
        complete_end,
        lines,
        last_printed,
        line_stride.filter(|stride| *stride > 0),
    ))
}

fn default_text_metrics(text: &str, cols: usize) -> Option<(usize, Option<char>)> {
    let mut used = 0usize;
    let mut has_base = false;
    let mut base_has_combining = false;
    let mut last_printed = None;
    for character in text.chars() {
        if character.is_control() {
            return None;
        }
        let reported_width = character_width(character);
        if reported_width == 0 {
            if !has_base || base_has_combining {
                return None;
            }
            base_has_combining = true;
        } else {
            used += if reported_width == 1 { 1 } else { 2 };
            if used >= cols {
                return None;
            }
            has_base = true;
            base_has_combining = false;
        }
        last_printed = Some(character);
    }
    Some((used, last_printed))
}

fn fill_default_text_row(row: &mut [Cell], text: &str) -> usize {
    let mut col = 0usize;
    for character in text.chars() {
        let reported_width = character_width(character);
        if reported_width == 0 {
            let lead = if row[col - 1].wide_spacer() {
                col - 2
            } else {
                col - 1
            };
            row[lead].set_has_combining(true);
            row[lead].metadata_id = Some(inline_combining_metadata_id(character));
            continue;
        }
        let width = if reported_width == 1 { 1 } else { 2 };
        row[col].character = character;
        if width == 2 {
            row[col + 1].set_wide_spacer(true);
        }
        col += width;
    }
    col
}

fn plain_ascii_line_prefix(
    bytes: &[u8],
    cols: usize,
) -> Option<(usize, usize, Option<char>, Option<usize>)> {
    let mut offset = 0usize;
    let mut complete_end = 0usize;
    let mut lines = 0usize;
    let mut last_printed = None;
    let mut line_stride = None;
    while offset < bytes.len() {
        let start = offset;
        let mut line_last_printed = None;
        while offset < bytes.len()
            && offset - start < cols
            && (0x20..=0x7e).contains(&bytes[offset])
        {
            line_last_printed = Some(char::from(bytes[offset]));
            offset += 1;
        }
        if offset - start >= cols
            || bytes.get(offset) != Some(&b'\r')
            || bytes.get(offset + 1) != Some(&b'\n')
        {
            break;
        }
        offset += 2;
        complete_end = offset;
        lines += 1;
        let stride = offset - start;
        line_stride = match line_stride {
            None => Some(stride),
            Some(previous) if previous == stride => Some(previous),
            Some(_) => Some(0),
        };
        if line_last_printed.is_some() {
            last_printed = line_last_printed;
        }
    }
    (lines > 0).then_some((
        complete_end,
        lines,
        last_printed,
        line_stride.filter(|stride| *stride > 0),
    ))
}
