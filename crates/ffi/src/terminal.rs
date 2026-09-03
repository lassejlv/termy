//! Thin C ABI wrapper around the headless terminal engine.

#![allow(
    clippy::wildcard_imports,
    reason = "the ABI constants and record types intentionally share one generated-style namespace"
)]

use engine::{
    Cell, Color, CursorShape, DynamicColor, FrameUpdate, Key, KeyEvent, KeyEventKind, KeypadKey,
    MediaKey, ModifierKey, Modifiers, MouseButton, MouseEvent, MouseEventKind, MousePointerShape,
    MouseTrackingMode, RowMoveDirection, SearchDirection, SearchOptions, SelectionMode,
    SelectionPoint, Terminal, TerminalConfig, TerminalEvent,
};

use crate::{
    error::{FfiError, ffi_status},
    types::*,
    util::{
        bytes_from_raw, required_mut, required_ref, slice_pointer, slice_view, to_u64,
        utf8_from_view, write_out,
    },
};

#[derive(Debug, Default)]
struct FrameStorage {
    rows: Vec<TmonRowUpdate>,
    moves: Vec<TmonRowMove>,
    cells: Vec<TmonCell>,
    text: Vec<u8>,
}

impl FrameStorage {
    fn pack(&mut self, update: FrameUpdate) -> TmonFrameView {
        self.rows.clear();
        self.moves.clear();
        self.cells.clear();
        self.text.clear();

        self.moves
            .extend(update.row_moves.iter().map(|movement| TmonRowMove {
                start_row: movement.start_row,
                end_row: movement.end_row,
                direction: match movement.direction {
                    RowMoveDirection::Up => TMON_ROW_MOVE_UP,
                    RowMoveDirection::Down => TMON_ROW_MOVE_DOWN,
                },
                count: movement.count,
            }));

        for row in update.row_updates {
            let cell_offset = self.cells.len();
            let cell_count = row.cells.len();
            for cell in row.cells {
                self.push_cell(&cell);
            }
            self.rows.push(TmonRowUpdate {
                row: row.index,
                start_column: row.start_column,
                cell_offset,
                cell_count,
            });
        }

        let selection = update
            .selection
            .map_or_else(TmonSelectionRange::default, |selection| {
                TmonSelectionRange {
                    start: TmonSelectionPoint {
                        column: selection.start.column,
                        row: selection.start.row,
                    },
                    end: TmonSelectionPoint {
                        column: selection.end.column,
                        row: selection.end.row,
                    },
                }
            });

        TmonFrameView {
            columns: update.columns,
            rows: update.rows,
            row_updates: slice_pointer(&self.rows),
            row_update_count: self.rows.len(),
            cells: slice_pointer(&self.cells),
            cell_count: self.cells.len(),
            text: slice_view(&self.text),
            cursor: TmonCursor {
                row: update.cursor.row,
                column: update.cursor.column,
                shape: cursor_shape(update.cursor.shape),
                visible: u8::from(update.cursor.visible),
                blinking: u8::from(update.cursor.blinking),
            },
            selection,
            display_offset: update.display_offset,
            revision: update.revision,
            full: u8::from(update.full),
            metadata_changed: u8::from(update.metadata_changed),
            has_selection: u8::from(update.selection.is_some()),
            row_moves: slice_pointer(&self.moves),
            row_move_count: self.moves.len(),
        }
    }

    fn push_cell(&mut self, cell: &Cell) {
        let text = self.push_bytes(cell.text.as_bytes());
        let (hyperlink, has_hyperlink) = cell.hyperlink.as_ref().map_or_else(
            || (TmonRange::default(), 0),
            |hyperlink| (self.push_bytes(hyperlink.as_bytes()), 1),
        );
        self.cells.push(TmonCell {
            text,
            hyperlink,
            foreground: color(cell.foreground),
            background: color(cell.background),
            flags: cell.flags.bits(),
            has_hyperlink,
        });
    }

    fn push_bytes(&mut self, bytes: &[u8]) -> TmonRange {
        let offset = self.text.len();
        self.text.extend_from_slice(bytes);
        TmonRange {
            offset,
            length: bytes.len(),
        }
    }
}

#[derive(Debug, Default)]
struct EventStorage {
    events: Vec<TmonEvent>,
    data: Vec<u8>,
}

impl EventStorage {
    fn pack(&mut self, events: Vec<TerminalEvent>) -> TmonEventBatchView {
        self.events.clear();
        self.data.clear();
        for event in events {
            let event = match event {
                TerminalEvent::Bell => TmonEvent {
                    kind: TMON_EVENT_BELL,
                    ..TmonEvent::default()
                },
                TerminalEvent::Title(title) => TmonEvent {
                    kind: TMON_EVENT_TITLE,
                    primary: self.push_bytes(title.as_bytes()),
                    ..TmonEvent::default()
                },
                TerminalEvent::ResetTitle => TmonEvent {
                    kind: TMON_EVENT_RESET_TITLE,
                    ..TmonEvent::default()
                },
                TerminalEvent::CurrentDirectory(directory) => TmonEvent {
                    kind: TMON_EVENT_CURRENT_DIRECTORY,
                    primary: self.push_bytes(directory.as_bytes()),
                    ..TmonEvent::default()
                },
                TerminalEvent::ClipboardStore { selection, text } => TmonEvent {
                    kind: TMON_EVENT_CLIPBOARD_STORE,
                    primary: self.push_bytes(selection.as_bytes()),
                    secondary: self.push_bytes(text.as_bytes()),
                    ..TmonEvent::default()
                },
                TerminalEvent::SetDynamicColor { target, color } => TmonEvent {
                    kind: TMON_EVENT_SET_DYNAMIC_COLOR,
                    value: dynamic_color(target),
                    color: TmonColor {
                        kind: TMON_COLOR_RGB,
                        red: color[0],
                        green: color[1],
                        blue: color[2],
                        index: 0,
                    },
                    ..TmonEvent::default()
                },
                TerminalEvent::ResetDynamicColor { target } => TmonEvent {
                    kind: TMON_EVENT_RESET_DYNAMIC_COLOR,
                    value: dynamic_color(target),
                    ..TmonEvent::default()
                },
                TerminalEvent::MousePointerShape(shape) => TmonEvent {
                    kind: TMON_EVENT_MOUSE_POINTER_SHAPE,
                    value: pointer_shape(shape),
                    ..TmonEvent::default()
                },
                TerminalEvent::Reply(bytes) => TmonEvent {
                    kind: TMON_EVENT_REPLY,
                    primary: self.push_bytes(&bytes),
                    ..TmonEvent::default()
                },
            };
            self.events.push(event);
        }
        TmonEventBatchView {
            events: slice_pointer(&self.events),
            event_count: self.events.len(),
            data: slice_view(&self.data),
        }
    }

    fn push_bytes(&mut self, bytes: &[u8]) -> TmonRange {
        let offset = self.data.len();
        self.data.extend_from_slice(bytes);
        TmonRange {
            offset,
            length: bytes.len(),
        }
    }
}

/// An opaque headless terminal handle. Calls for one handle must be serialized by the host.
#[derive(Debug)]
pub struct TmonTerminal {
    terminal: Terminal,
    frame: FrameStorage,
    events: EventStorage,
    encoded: Vec<u8>,
    selected: Vec<u8>,
}

impl TmonTerminal {
    fn new(config: TerminalConfig) -> Self {
        Self {
            terminal: Terminal::new(config),
            frame: FrameStorage::default(),
            events: EventStorage::default(),
            encoded: Vec::new(),
            selected: Vec::new(),
        }
    }

    fn store_encoded(&mut self, bytes: Vec<u8>) -> TmonByteSlice {
        self.encoded = bytes;
        slice_view(&self.encoded)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn tmon_terminal_config_default() -> TmonTerminalConfig {
    let config = TerminalConfig::default();
    TmonTerminalConfig {
        columns: config.columns,
        rows: config.rows,
        scrollback_limit: config.scrollback_limit,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_terminal_new(
    config: *const TmonTerminalConfig,
    out_terminal: *mut *mut TmonTerminal,
) -> u32 {
    ffi_status(|| {
        // SAFETY: Checked pointer access is confined to this FFI call.
        let config = unsafe { required_ref(config, "config")? };
        let terminal = Box::new(TmonTerminal::new(TerminalConfig {
            columns: config.columns,
            rows: config.rows,
            scrollback_limit: config.scrollback_limit,
        }));
        // SAFETY: `out_terminal` is required to point to one writable handle slot.
        unsafe { write_out(out_terminal, Box::into_raw(terminal), "out_terminal") }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_terminal_free(terminal: *mut TmonTerminal) -> u32 {
    if terminal.is_null() {
        return TMON_OK;
    }
    ffi_status(|| {
        // SAFETY: Ownership of the handle is transferred back exactly once by the ABI contract.
        drop(unsafe { Box::from_raw(terminal) });
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_terminal_feed(
    terminal: *mut TmonTerminal,
    bytes: *const u8,
    length: usize,
) -> u32 {
    ffi_status(|| {
        // SAFETY: Pointers are borrowed only for this call and checked for null where required.
        let terminal = unsafe { required_mut(terminal, "terminal")? };
        // SAFETY: The caller provides `length` readable bytes.
        let bytes = unsafe { bytes_from_raw(bytes, length, "bytes")? };
        terminal.terminal.feed(bytes);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_terminal_resize(
    terminal: *mut TmonTerminal,
    columns: usize,
    rows: usize,
) -> u32 {
    ffi_status(|| {
        // SAFETY: Exclusive handle access is part of the ABI contract.
        unsafe { required_mut(terminal, "terminal")? }
            .terminal
            .resize(columns, rows);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_terminal_set_pixel_size(
    terminal: *mut TmonTerminal,
    width: u32,
    height: u32,
) -> u32 {
    ffi_status(|| {
        // SAFETY: Exclusive handle access is part of the ABI contract.
        unsafe { required_mut(terminal, "terminal")? }
            .terminal
            .set_pixel_size(width, height);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_terminal_set_scrollback_limit(
    terminal: *mut TmonTerminal,
    limit: usize,
) -> u32 {
    ffi_status(|| {
        // SAFETY: Exclusive handle access is part of the ABI contract.
        unsafe { required_mut(terminal, "terminal")? }
            .terminal
            .set_scrollback_limit(limit);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_terminal_frame_update(
    terminal: *mut TmonTerminal,
    force_full: u8,
    out_frame: *mut TmonFrameView,
) -> u32 {
    ffi_status(|| {
        // SAFETY: Exclusive handle access is part of the ABI contract.
        let terminal = unsafe { required_mut(terminal, "terminal")? };
        let update = terminal.terminal.frame_update(force_full != 0);
        let frame = terminal.frame.pack(update);
        // SAFETY: The output points to one writable frame view.
        unsafe { write_out(out_frame, frame, "out_frame") }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_terminal_drain_events(
    terminal: *mut TmonTerminal,
    out_events: *mut TmonEventBatchView,
) -> u32 {
    ffi_status(|| {
        // SAFETY: Exclusive handle access is part of the ABI contract.
        let terminal = unsafe { required_mut(terminal, "terminal")? };
        let events = terminal.terminal.drain_events();
        let view = terminal.events.pack(events);
        // SAFETY: The output points to one writable event view.
        unsafe { write_out(out_events, view, "out_events") }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_terminal_encode_key(
    terminal: *mut TmonTerminal,
    event: *const TmonKeyEvent,
    out_bytes: *mut TmonByteSlice,
) -> u32 {
    ffi_status(|| {
        // SAFETY: Pointers are checked and borrowed only for this call.
        let terminal = unsafe { required_mut(terminal, "terminal")? };
        // SAFETY: `event` must point to one readable input record.
        let event = unsafe { required_ref(event, "event")? };
        // SAFETY: Any text view belongs to the same call.
        let event = unsafe { key_event(event)? };
        let bytes = terminal.terminal.encode_key(&event);
        let view = terminal.store_encoded(bytes);
        // SAFETY: The output points to one writable byte view.
        unsafe { write_out(out_bytes, view, "out_bytes") }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_terminal_encode_text(
    terminal: *mut TmonTerminal,
    text: TmonByteSlice,
    out_bytes: *mut TmonByteSlice,
) -> u32 {
    ffi_status(|| {
        // SAFETY: Pointers are checked and borrowed only for this call.
        let terminal = unsafe { required_mut(terminal, "terminal")? };
        // SAFETY: The caller supplies a valid UTF-8 view for this call.
        let text = unsafe { utf8_from_view(text, "text")? };
        let bytes = terminal.terminal.encode_text(text);
        let view = terminal.store_encoded(bytes);
        // SAFETY: The output points to one writable byte view.
        unsafe { write_out(out_bytes, view, "out_bytes") }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_terminal_encode_paste(
    terminal: *mut TmonTerminal,
    text: TmonByteSlice,
    out_bytes: *mut TmonByteSlice,
) -> u32 {
    ffi_status(|| {
        // SAFETY: Pointers are checked and borrowed only for this call.
        let terminal = unsafe { required_mut(terminal, "terminal")? };
        // SAFETY: The caller supplies a valid UTF-8 view for this call.
        let text = unsafe { utf8_from_view(text, "text")? };
        let bytes = terminal.terminal.encode_paste(text);
        let view = terminal.store_encoded(bytes);
        // SAFETY: The output points to one writable byte view.
        unsafe { write_out(out_bytes, view, "out_bytes") }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_terminal_encode_mouse(
    terminal: *mut TmonTerminal,
    event: *const TmonMouseEvent,
    out_bytes: *mut TmonByteSlice,
    out_has_value: *mut u8,
) -> u32 {
    ffi_status(|| {
        // SAFETY: Pointers are checked and borrowed only for this call.
        let terminal = unsafe { required_mut(terminal, "terminal")? };
        // SAFETY: `event` points to one readable input record.
        let event = unsafe { mouse_event(required_ref(event, "event")?)? };
        let encoded = terminal.terminal.encode_mouse(event);
        let has_value = encoded.is_some();
        let view = terminal.store_encoded(encoded.unwrap_or_default());
        // SAFETY: Both outputs point to writable values.
        unsafe { write_out(out_bytes, view, "out_bytes")? };
        // SAFETY: See above.
        unsafe { write_out(out_has_value, u8::from(has_value), "out_has_value") }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_terminal_focus_changed(
    terminal: *mut TmonTerminal,
    focused: u8,
    out_bytes: *mut TmonByteSlice,
    out_has_value: *mut u8,
) -> u32 {
    ffi_status(|| {
        // SAFETY: Exclusive handle access is part of the ABI contract.
        let terminal = unsafe { required_mut(terminal, "terminal")? };
        let encoded = terminal.terminal.focus_changed(focused != 0);
        let has_value = encoded.is_some();
        let view = terminal.store_encoded(encoded.unwrap_or_default());
        // SAFETY: Both outputs point to writable values.
        unsafe { write_out(out_bytes, view, "out_bytes")? };
        // SAFETY: See above.
        unsafe { write_out(out_has_value, u8::from(has_value), "out_has_value") }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_terminal_scroll_display(
    terminal: *mut TmonTerminal,
    lines: i64,
    out_changed: *mut u8,
) -> u32 {
    ffi_status(|| {
        let lines = isize::try_from(lines)
            .map_err(|_| FfiError::invalid("lines does not fit the host pointer width"))?;
        // SAFETY: Exclusive handle access is part of the ABI contract.
        let changed = unsafe { required_mut(terminal, "terminal")? }
            .terminal
            .scroll_display(lines);
        // SAFETY: The output points to one writable byte.
        unsafe { write_out(out_changed, u8::from(changed), "out_changed") }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_terminal_scroll_to_bottom(terminal: *mut TmonTerminal) -> u32 {
    ffi_status(|| {
        // SAFETY: Exclusive handle access is part of the ABI contract.
        unsafe { required_mut(terminal, "terminal")? }
            .terminal
            .scroll_to_bottom();
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_terminal_begin_selection(
    terminal: *mut TmonTerminal,
    point: TmonSelectionPoint,
    out_changed: *mut u8,
) -> u32 {
    // SAFETY: The helper enforces the same handle and out-pointer contract as this entry point.
    unsafe { selection_call(terminal, point, out_changed, Terminal::begin_selection) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_terminal_begin_selection_with_mode(
    terminal: *mut TmonTerminal,
    point: TmonSelectionPoint,
    mode: u32,
    out_changed: *mut u8,
) -> u32 {
    ffi_status(|| {
        let mode = match mode {
            TMON_SELECTION_CHARACTER => SelectionMode::Character,
            TMON_SELECTION_WORD => SelectionMode::Word,
            TMON_SELECTION_LINE => SelectionMode::Line,
            _ => return Err(FfiError::invalid("unknown selection mode")),
        };
        // SAFETY: Exclusive handle access is part of the ABI contract.
        let terminal = unsafe { required_mut(terminal, "terminal")? };
        let changed = terminal.terminal.begin_selection_with_mode(
            SelectionPoint {
                column: point.column,
                row: point.row,
            },
            mode,
        );
        // SAFETY: The output points to one writable byte.
        unsafe { write_out(out_changed, u8::from(changed), "out_changed") }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_terminal_update_selection(
    terminal: *mut TmonTerminal,
    point: TmonSelectionPoint,
    out_changed: *mut u8,
) -> u32 {
    // SAFETY: The helper enforces the same handle and out-pointer contract as this entry point.
    unsafe { selection_call(terminal, point, out_changed, Terminal::update_selection) }
}

unsafe fn selection_call(
    terminal: *mut TmonTerminal,
    point: TmonSelectionPoint,
    out_changed: *mut u8,
    operation: fn(&mut Terminal, SelectionPoint) -> bool,
) -> u32 {
    ffi_status(|| {
        // SAFETY: Exclusive handle access is part of the ABI contract.
        let terminal = unsafe { required_mut(terminal, "terminal")? };
        let changed = operation(
            &mut terminal.terminal,
            SelectionPoint {
                column: point.column,
                row: point.row,
            },
        );
        // SAFETY: The output points to one writable byte.
        unsafe { write_out(out_changed, u8::from(changed), "out_changed") }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_terminal_clear_selection(
    terminal: *mut TmonTerminal,
    out_changed: *mut u8,
) -> u32 {
    ffi_status(|| {
        // SAFETY: Exclusive handle access is part of the ABI contract.
        let changed = unsafe { required_mut(terminal, "terminal")? }
            .terminal
            .clear_selection();
        // SAFETY: The output points to one writable byte.
        unsafe { write_out(out_changed, u8::from(changed), "out_changed") }
    })
}

#[unsafe(no_mangle)]
pub const extern "C" fn tmon_search_options_default() -> TmonSearchOptions {
    TmonSearchOptions {
        direction: TMON_SEARCH_BACKWARD,
        case_sensitive: 0,
        wrap: 1,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_terminal_search(
    terminal: *mut TmonTerminal,
    query: TmonByteSlice,
    options: TmonSearchOptions,
    out_match: *mut TmonSearchMatch,
    out_has_value: *mut u8,
) -> u32 {
    ffi_status(|| {
        // SAFETY: The query view is readable for the duration of this call.
        let query = unsafe { utf8_from_view(query, "query")? };
        let direction = match options.direction {
            TMON_SEARCH_FORWARD => SearchDirection::Forward,
            TMON_SEARCH_BACKWARD => SearchDirection::Backward,
            _ => return Err(FfiError::invalid("unknown search direction")),
        };
        if options.case_sensitive > 1 || options.wrap > 1 {
            return Err(FfiError::invalid(
                "search case_sensitive and wrap must be 0 or 1",
            ));
        }
        // SAFETY: Exclusive handle access is part of the ABI contract.
        let terminal = unsafe { required_mut(terminal, "terminal")? };
        let found = terminal.terminal.search_with_options(
            query,
            SearchOptions {
                direction,
                case_sensitive: options.case_sensitive != 0,
                wrap: options.wrap != 0,
            },
        );
        let value = found.map_or_else(TmonSearchMatch::default, |found| TmonSearchMatch {
            selection: TmonSelectionRange {
                start: TmonSelectionPoint {
                    column: found.range.start.column,
                    row: found.range.start.row,
                },
                end: TmonSelectionPoint {
                    column: found.range.end.column,
                    row: found.range.end.row,
                },
            },
            display_offset: found.display_offset,
        });
        // SAFETY: Both outputs point to writable values.
        unsafe { write_out(out_match, value, "out_match")? };
        // SAFETY: See above.
        unsafe { write_out(out_has_value, u8::from(found.is_some()), "out_has_value") }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_terminal_reset_search(
    terminal: *mut TmonTerminal,
    out_changed: *mut u8,
) -> u32 {
    ffi_status(|| {
        // SAFETY: Exclusive handle access is part of the ABI contract.
        let changed = unsafe { required_mut(terminal, "terminal")? }
            .terminal
            .reset_search();
        // SAFETY: The output points to one writable byte.
        unsafe { write_out(out_changed, u8::from(changed), "out_changed") }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_terminal_selected_text(
    terminal: *mut TmonTerminal,
    out_text: *mut TmonByteSlice,
    out_has_value: *mut u8,
) -> u32 {
    ffi_status(|| {
        // SAFETY: Exclusive handle access is part of the ABI contract because the borrowed result
        // is stored on the handle.
        let terminal = unsafe { required_mut(terminal, "terminal")? };
        let selected = terminal.terminal.selected_text();
        let has_value = selected.is_some();
        terminal.selected = selected.map_or_else(Vec::new, String::into_bytes);
        let view = slice_view(&terminal.selected);
        // SAFETY: Both outputs point to writable values.
        unsafe { write_out(out_text, view, "out_text")? };
        // SAFETY: See above.
        unsafe { write_out(out_has_value, u8::from(has_value), "out_has_value") }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_terminal_mouse_tracking_mode(
    terminal: *const TmonTerminal,
    out_mode: *mut u32,
) -> u32 {
    ffi_status(|| {
        // SAFETY: Shared handle access is sufficient for this query.
        let terminal = unsafe { required_ref(terminal, "terminal")? };
        let mode = match terminal.terminal.mouse_tracking_mode() {
            MouseTrackingMode::Disabled => TMON_MOUSE_TRACKING_DISABLED,
            MouseTrackingMode::Press => TMON_MOUSE_TRACKING_PRESS,
            MouseTrackingMode::ButtonMotion => TMON_MOUSE_TRACKING_BUTTON_MOTION,
            MouseTrackingMode::AnyMotion => TMON_MOUSE_TRACKING_ANY_MOTION,
        };
        // SAFETY: The output points to one writable integer.
        unsafe { write_out(out_mode, mode, "out_mode") }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_terminal_metrics(
    terminal: *const TmonTerminal,
    out_metrics: *mut TmonTerminalMetrics,
) -> u32 {
    ffi_status(|| {
        // SAFETY: Shared handle access is sufficient for this query.
        let metrics = unsafe { required_ref(terminal, "terminal")? }
            .terminal
            .metrics();
        let metrics = TmonTerminalMetrics {
            feed_calls: metrics.feed_calls,
            bytes_fed: metrics.bytes_fed,
            frame_requests: metrics.frame_requests,
            damaged_frames: metrics.damaged_frames,
            full_frames: metrics.full_frames,
            row_moves: metrics.row_moves,
            rows_moved: metrics.rows_moved,
            row_updates: metrics.row_updates,
            cells_copied: metrics.cells_copied,
        };
        // SAFETY: The output points to one writable metrics record.
        unsafe { write_out(out_metrics, metrics, "out_metrics") }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_terminal_reset_metrics(terminal: *mut TmonTerminal) -> u32 {
    ffi_status(|| {
        // SAFETY: Exclusive handle access is part of the ABI contract.
        unsafe { required_mut(terminal, "terminal")? }
            .terminal
            .reset_metrics();
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_terminal_memory_stats(
    terminal: *const TmonTerminal,
    out_stats: *mut TmonMemoryStats,
) -> u32 {
    ffi_status(|| {
        // SAFETY: Shared handle access is sufficient for this query.
        let stats = unsafe { required_ref(terminal, "terminal")? }
            .terminal
            .memory_stats();
        let stats = TmonMemoryStats {
            live_rows: to_u64(stats.live_rows),
            scrollback_rows: to_u64(stats.scrollback_rows),
            spare_rows: to_u64(stats.spare_rows),
            live_cell_capacity: to_u64(stats.live_cell_capacity),
            scrollback_cell_capacity: to_u64(stats.scrollback_cell_capacity),
            spare_cell_capacity: to_u64(stats.spare_cell_capacity),
            live_row_capacity: to_u64(stats.live_row_capacity),
            scrollback_row_capacity: to_u64(stats.scrollback_row_capacity),
            damage_row_capacity: to_u64(stats.damage_row_capacity),
            damage_snapshot_capacity: to_u64(stats.damage_snapshot_capacity),
            total_cell_capacity: to_u64(stats.total_cell_capacity()),
            cell_capacity_bytes: to_u64(stats.cell_capacity_bytes()),
        };
        // SAFETY: The output points to one writable stats record.
        unsafe { write_out(out_stats, stats, "out_stats") }
    })
}

unsafe fn key_event(event: &TmonKeyEvent) -> Result<KeyEvent, FfiError> {
    let text = if event.has_text == 0 {
        None
    } else {
        // SAFETY: The event's text view must be readable for this call.
        Some(unsafe { utf8_from_view(event.text, "event.text")? }.to_owned())
    };
    let modifiers = u8::try_from(event.modifiers)
        .ok()
        .and_then(Modifiers::from_bits)
        .ok_or_else(|| FfiError::invalid("event.modifiers contains unsupported bits"))?;
    let kind = match event.event_kind {
        TMON_KEY_PRESS => KeyEventKind::Press,
        TMON_KEY_REPEAT => KeyEventKind::Repeat,
        TMON_KEY_RELEASE => KeyEventKind::Release,
        _ => return Err(FfiError::invalid("event.event_kind is unknown")),
    };
    let shifted_key = optional_codepoint(event.has_shifted_key, event.shifted_key, "shifted_key")?;
    let base_layout = optional_codepoint(
        event.has_base_layout_key,
        event.base_layout_key,
        "base_layout_key",
    )?;
    Ok(KeyEvent {
        key: key(event.key_kind, event.key_value)?,
        text,
        shifted_key,
        base_layout,
        modifiers,
        kind,
    })
}

fn optional_codepoint(present: u8, value: u32, name: &str) -> Result<Option<u32>, FfiError> {
    if present == 0 {
        return Ok(None);
    }
    char::from_u32(value)
        .map(u32::from)
        .map(Some)
        .ok_or_else(|| FfiError::invalid(format!("{name} is not a Unicode scalar value")))
}

fn key(kind: u32, value: u32) -> Result<Key, FfiError> {
    Ok(match kind {
        TMON_KEY_CHARACTER => Key::Character(
            char::from_u32(value)
                .ok_or_else(|| FfiError::invalid("character key is not a Unicode scalar value"))?,
        ),
        TMON_KEY_ESCAPE => Key::Escape,
        TMON_KEY_ENTER => Key::Enter,
        TMON_KEY_TAB => Key::Tab,
        TMON_KEY_BACKTAB => Key::Backtab,
        TMON_KEY_BACKSPACE => Key::Backspace,
        TMON_KEY_INSERT => Key::Insert,
        TMON_KEY_DELETE => Key::Delete,
        TMON_KEY_UP => Key::Up,
        TMON_KEY_DOWN => Key::Down,
        TMON_KEY_LEFT => Key::Left,
        TMON_KEY_RIGHT => Key::Right,
        TMON_KEY_PAGE_UP => Key::PageUp,
        TMON_KEY_PAGE_DOWN => Key::PageDown,
        TMON_KEY_HOME => Key::Home,
        TMON_KEY_END => Key::End,
        TMON_KEY_FUNCTION => Key::Function(
            u8::try_from(value)
                .ok()
                .filter(|number| *number != 0)
                .ok_or_else(|| {
                    FfiError::invalid("function key number must be between 1 and 255")
                })?,
        ),
        TMON_KEY_KEYPAD => Key::Keypad(keypad(value)?),
        TMON_KEY_CAPS_LOCK => Key::CapsLock,
        TMON_KEY_SCROLL_LOCK => Key::ScrollLock,
        TMON_KEY_NUM_LOCK => Key::NumLock,
        TMON_KEY_PRINT_SCREEN => Key::PrintScreen,
        TMON_KEY_PAUSE => Key::Pause,
        TMON_KEY_MENU => Key::Menu,
        TMON_KEY_MEDIA => Key::Media(media(value)?),
        TMON_KEY_MODIFIER => Key::Modifier(modifier_key(value)?),
        _ => return Err(FfiError::invalid("event.key_kind is unknown")),
    })
}

fn keypad(value: u32) -> Result<KeypadKey, FfiError> {
    Ok(match value {
        TMON_KEYPAD_DIGIT_0..=TMON_KEYPAD_DIGIT_9 => {
            KeypadKey::Digit(u8::try_from(value).expect("keypad digit range fits u8"))
        }
        TMON_KEYPAD_DECIMAL => KeypadKey::Decimal,
        TMON_KEYPAD_DIVIDE => KeypadKey::Divide,
        TMON_KEYPAD_MULTIPLY => KeypadKey::Multiply,
        TMON_KEYPAD_SUBTRACT => KeypadKey::Subtract,
        TMON_KEYPAD_ADD => KeypadKey::Add,
        TMON_KEYPAD_ENTER => KeypadKey::Enter,
        TMON_KEYPAD_EQUAL => KeypadKey::Equal,
        TMON_KEYPAD_SEPARATOR => KeypadKey::Separator,
        TMON_KEYPAD_LEFT => KeypadKey::Left,
        TMON_KEYPAD_RIGHT => KeypadKey::Right,
        TMON_KEYPAD_UP => KeypadKey::Up,
        TMON_KEYPAD_DOWN => KeypadKey::Down,
        TMON_KEYPAD_PAGE_UP => KeypadKey::PageUp,
        TMON_KEYPAD_PAGE_DOWN => KeypadKey::PageDown,
        TMON_KEYPAD_HOME => KeypadKey::Home,
        TMON_KEYPAD_END => KeypadKey::End,
        TMON_KEYPAD_INSERT => KeypadKey::Insert,
        TMON_KEYPAD_DELETE => KeypadKey::Delete,
        TMON_KEYPAD_BEGIN => KeypadKey::Begin,
        _ => return Err(FfiError::invalid("event.key_value is not a keypad key")),
    })
}

fn media(value: u32) -> Result<MediaKey, FfiError> {
    Ok(match value {
        TMON_MEDIA_PLAY => MediaKey::Play,
        TMON_MEDIA_PAUSE => MediaKey::Pause,
        TMON_MEDIA_PLAY_PAUSE => MediaKey::PlayPause,
        TMON_MEDIA_REVERSE => MediaKey::Reverse,
        TMON_MEDIA_STOP => MediaKey::Stop,
        TMON_MEDIA_FAST_FORWARD => MediaKey::FastForward,
        TMON_MEDIA_REWIND => MediaKey::Rewind,
        TMON_MEDIA_TRACK_NEXT => MediaKey::TrackNext,
        TMON_MEDIA_TRACK_PREVIOUS => MediaKey::TrackPrevious,
        TMON_MEDIA_RECORD => MediaKey::Record,
        TMON_MEDIA_LOWER_VOLUME => MediaKey::LowerVolume,
        TMON_MEDIA_RAISE_VOLUME => MediaKey::RaiseVolume,
        TMON_MEDIA_MUTE => MediaKey::Mute,
        _ => return Err(FfiError::invalid("event.key_value is not a media key")),
    })
}

fn modifier_key(value: u32) -> Result<ModifierKey, FfiError> {
    Ok(match value {
        TMON_MODIFIER_KEY_LEFT_SHIFT => ModifierKey::LeftShift,
        TMON_MODIFIER_KEY_LEFT_CONTROL => ModifierKey::LeftControl,
        TMON_MODIFIER_KEY_LEFT_ALT => ModifierKey::LeftAlt,
        TMON_MODIFIER_KEY_LEFT_SUPER => ModifierKey::LeftSuper,
        TMON_MODIFIER_KEY_LEFT_HYPER => ModifierKey::LeftHyper,
        TMON_MODIFIER_KEY_LEFT_META => ModifierKey::LeftMeta,
        TMON_MODIFIER_KEY_RIGHT_SHIFT => ModifierKey::RightShift,
        TMON_MODIFIER_KEY_RIGHT_CONTROL => ModifierKey::RightControl,
        TMON_MODIFIER_KEY_RIGHT_ALT => ModifierKey::RightAlt,
        TMON_MODIFIER_KEY_RIGHT_SUPER => ModifierKey::RightSuper,
        TMON_MODIFIER_KEY_RIGHT_HYPER => ModifierKey::RightHyper,
        TMON_MODIFIER_KEY_RIGHT_META => ModifierKey::RightMeta,
        TMON_MODIFIER_KEY_ISO_LEVEL3_SHIFT => ModifierKey::IsoLevel3Shift,
        TMON_MODIFIER_KEY_ISO_LEVEL5_SHIFT => ModifierKey::IsoLevel5Shift,
        _ => return Err(FfiError::invalid("event.key_value is not a modifier key")),
    })
}

unsafe fn mouse_event(event: &TmonMouseEvent) -> Result<MouseEvent, FfiError> {
    let button = match event.button {
        TMON_MOUSE_BUTTON_NONE => MouseButton::None,
        TMON_MOUSE_BUTTON_LEFT => MouseButton::Left,
        TMON_MOUSE_BUTTON_MIDDLE => MouseButton::Middle,
        TMON_MOUSE_BUTTON_RIGHT => MouseButton::Right,
        TMON_MOUSE_BUTTON_WHEEL_UP => MouseButton::WheelUp,
        TMON_MOUSE_BUTTON_WHEEL_DOWN => MouseButton::WheelDown,
        _ => return Err(FfiError::invalid("event.button is unknown")),
    };
    let kind = match event.kind {
        TMON_MOUSE_PRESS => MouseEventKind::Press,
        TMON_MOUSE_RELEASE => MouseEventKind::Release,
        TMON_MOUSE_MOTION => MouseEventKind::Motion,
        _ => return Err(FfiError::invalid("event.kind is unknown")),
    };
    let modifiers = u8::try_from(event.modifiers)
        .ok()
        .and_then(Modifiers::from_bits)
        .ok_or_else(|| FfiError::invalid("event.modifiers contains unsupported bits"))?;
    Ok(MouseEvent {
        button,
        kind,
        column: event.column,
        row: event.row,
        pixel_x: event.pixel_x,
        pixel_y: event.pixel_y,
        modifiers,
    })
}

const fn color(color: Color) -> TmonColor {
    match color {
        Color::Default => TmonColor {
            kind: TMON_COLOR_DEFAULT,
            red: 0,
            green: 0,
            blue: 0,
            index: 0,
        },
        Color::Indexed(index) => TmonColor {
            kind: TMON_COLOR_INDEXED,
            red: 0,
            green: 0,
            blue: 0,
            index,
        },
        Color::Rgb(red, green, blue) => TmonColor {
            kind: TMON_COLOR_RGB,
            red,
            green,
            blue,
            index: 0,
        },
    }
}

const fn cursor_shape(shape: CursorShape) -> u32 {
    match shape {
        CursorShape::Block => TMON_CURSOR_BLOCK,
        CursorShape::Underline => TMON_CURSOR_UNDERLINE,
        CursorShape::Bar => TMON_CURSOR_BAR,
    }
}

const fn dynamic_color(target: DynamicColor) -> u32 {
    match target {
        DynamicColor::Foreground => TMON_DYNAMIC_COLOR_FOREGROUND,
        DynamicColor::Background => TMON_DYNAMIC_COLOR_BACKGROUND,
        DynamicColor::Cursor => TMON_DYNAMIC_COLOR_CURSOR,
    }
}

const fn pointer_shape(shape: MousePointerShape) -> u32 {
    match shape {
        MousePointerShape::Default => TMON_POINTER_DEFAULT,
        MousePointerShape::Pointer => TMON_POINTER_POINTER,
        MousePointerShape::Text => TMON_POINTER_TEXT,
        MousePointerShape::Crosshair => TMON_POINTER_CROSSHAIR,
        MousePointerShape::Move => TMON_POINTER_MOVE,
        MousePointerShape::NotAllowed => TMON_POINTER_NOT_ALLOWED,
        MousePointerShape::Help => TMON_POINTER_HELP,
        MousePointerShape::Progress => TMON_POINTER_PROGRESS,
        MousePointerShape::Wait => TMON_POINTER_WAIT,
        MousePointerShape::Cell => TMON_POINTER_CELL,
        MousePointerShape::VerticalText => TMON_POINTER_VERTICAL_TEXT,
        MousePointerShape::Alias => TMON_POINTER_ALIAS,
        MousePointerShape::Copy => TMON_POINTER_COPY,
        MousePointerShape::NoDrop => TMON_POINTER_NO_DROP,
        MousePointerShape::Grab => TMON_POINTER_GRAB,
        MousePointerShape::Grabbing => TMON_POINTER_GRABBING,
        MousePointerShape::EResize => TMON_POINTER_E_RESIZE,
        MousePointerShape::NResize => TMON_POINTER_N_RESIZE,
        MousePointerShape::NeResize => TMON_POINTER_NE_RESIZE,
        MousePointerShape::NwResize => TMON_POINTER_NW_RESIZE,
        MousePointerShape::SResize => TMON_POINTER_S_RESIZE,
        MousePointerShape::SeResize => TMON_POINTER_SE_RESIZE,
        MousePointerShape::SwResize => TMON_POINTER_SW_RESIZE,
        MousePointerShape::WResize => TMON_POINTER_W_RESIZE,
        MousePointerShape::EwResize => TMON_POINTER_EW_RESIZE,
        MousePointerShape::NsResize => TMON_POINTER_NS_RESIZE,
        MousePointerShape::NeswResize => TMON_POINTER_NESW_RESIZE,
        MousePointerShape::NwseResize => TMON_POINTER_NWSE_RESIZE,
        MousePointerShape::ZoomIn => TMON_POINTER_ZOOM_IN,
        MousePointerShape::ZoomOut => TMON_POINTER_ZOOM_OUT,
    }
}
