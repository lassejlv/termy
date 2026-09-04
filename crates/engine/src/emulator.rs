//! Stateful VT emulator and control-sequence handling.

use std::{collections::VecDeque, sync::Arc};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde::{Deserialize, Serialize};
use vte::{Params, Perform};

use crate::{
    CellFlags, Color, CursorShape, CursorState, DEFAULT_BACKGROUND_RGB, DEFAULT_CURSOR_RGB,
    DEFAULT_FOREGROUND_RGB, DynamicColor, FrameUpdate, KittyKeyboardFlags, MousePointerShape,
    MouseTrackingMode, RowUpdate, SelectionPoint, TerminalEvent,
    cell::CellTemplate,
    grid::Grid,
    input::ModifyOtherKeysState,
    pointer::{GRABBED_POINTER, PointerShapeStack, TERMINAL_DEFAULT_POINTER},
    search::{SearchMatch, SearchOptions, SearchState},
    selection::{Selection, SelectionMode, SelectionRange, clamp_point},
};

const KEYBOARD_STACK_LIMIT: usize = 64;
const OSC_CLIPBOARD_LIMIT: usize = 1024 * 1024;
const OSC_TITLE_LIMIT: usize = 1024;
const OSC_DIRECTORY_LIMIT: usize = 4096;
const OSC_HYPERLINK_LIMIT: usize = 8192;
const OSC_DYNAMIC_COLOR_LIMIT: usize = 64;
const OSC_POINTER_SHAPE_LIMIT: usize = 512;
const OSC_CLIPBOARD_SELECTION_LIMIT: usize = 16;

#[derive(Debug, Default, Deserialize, Serialize)]
struct KeyboardMode {
    flags: KittyKeyboardFlags,
    stack: VecDeque<KittyKeyboardFlags>,
}

impl KeyboardMode {
    fn apply(&mut self, flags: KittyKeyboardFlags, mode: u16) {
        match mode {
            2 => self.flags.insert(flags),
            3 => self.flags.remove(flags),
            _ => self.flags = flags,
        }
    }

    fn push(&mut self, flags: KittyKeyboardFlags) {
        if self.stack.len() == KEYBOARD_STACK_LIMIT {
            self.stack.pop_front();
        }
        self.stack.push_back(self.flags);
        self.flags = flags;
    }

    fn pop(&mut self, count: usize) {
        for _ in 0..count.max(1) {
            self.flags = self.stack.pop_back().unwrap_or_default();
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct Emulator {
    main: Grid,
    alternate: Grid,
    alternate_active: bool,
    template: CellTemplate,
    events: VecDeque<TerminalEvent>,
    application_cursor: bool,
    application_keypad: bool,
    insert_mode: bool,
    autowrap: bool,
    origin_mode: bool,
    bracketed_paste: bool,
    focus_reporting: bool,
    mouse_tracking: MouseTrackingMode,
    sgr_mouse: bool,
    pixel_mouse: bool,
    pixel_width: u32,
    pixel_height: u32,
    cursor_visible: bool,
    cursor_shape: CursorShape,
    cursor_blinking: bool,
    synchronized_output: bool,
    dynamic_foreground: [u8; 3],
    dynamic_background: [u8; 3],
    dynamic_cursor: [u8; 3],
    xtgettcap_query: Option<Vec<u8>>,
    selection: Option<Selection>,
    selection_dirty: bool,
    search: SearchState,
    main_keyboard: KeyboardMode,
    alternate_keyboard: KeyboardMode,
    modify_other_keys: ModifyOtherKeysState,
    main_pointer_shapes: PointerShapeStack,
    alternate_pointer_shapes: PointerShapeStack,
}

impl Emulator {
    pub(crate) fn new(columns: usize, rows: usize, scrollback_limit: usize) -> Self {
        Self {
            main: Grid::new(columns, rows, scrollback_limit, true),
            alternate: Grid::new(columns, rows, 0, false),
            alternate_active: false,
            template: CellTemplate::default(),
            events: VecDeque::new(),
            application_cursor: false,
            application_keypad: false,
            insert_mode: false,
            autowrap: true,
            origin_mode: false,
            bracketed_paste: false,
            focus_reporting: false,
            mouse_tracking: MouseTrackingMode::Disabled,
            sgr_mouse: false,
            pixel_mouse: false,
            pixel_width: 0,
            pixel_height: 0,
            cursor_visible: true,
            cursor_shape: CursorShape::Block,
            cursor_blinking: true,
            synchronized_output: false,
            dynamic_foreground: DEFAULT_FOREGROUND_RGB,
            dynamic_background: DEFAULT_BACKGROUND_RGB,
            dynamic_cursor: DEFAULT_CURSOR_RGB,
            xtgettcap_query: None,
            selection: None,
            selection_dirty: false,
            search: SearchState::default(),
            main_keyboard: KeyboardMode::default(),
            alternate_keyboard: KeyboardMode::default(),
            modify_other_keys: ModifyOtherKeysState::default(),
            main_pointer_shapes: PointerShapeStack::default(),
            alternate_pointer_shapes: PointerShapeStack::default(),
        }
    }

    fn active(&self) -> &Grid {
        if self.alternate_active {
            &self.alternate
        } else {
            &self.main
        }
    }

    fn active_mut(&mut self) -> &mut Grid {
        if self.alternate_active {
            &mut self.alternate
        } else {
            &mut self.main
        }
    }

    pub(crate) fn viewport_revision(&self) -> u64 {
        self.active().viewport_revision()
    }

    pub(crate) fn dimensions(&self) -> (usize, usize) {
        let grid = self.active();
        (grid.columns(), grid.rows())
    }

    pub(crate) fn clear_selection_if_viewport_changed(&mut self, previous_revision: u64) {
        if self.active().viewport_revision() != previous_revision {
            self.clear_selection();
        }
    }

    fn keyboard(&self) -> &KeyboardMode {
        if self.alternate_active {
            &self.alternate_keyboard
        } else {
            &self.main_keyboard
        }
    }

    fn keyboard_mut(&mut self) -> &mut KeyboardMode {
        if self.alternate_active {
            &mut self.alternate_keyboard
        } else {
            &mut self.main_keyboard
        }
    }

    fn pointer_shapes(&self) -> &PointerShapeStack {
        if self.alternate_active {
            &self.alternate_pointer_shapes
        } else {
            &self.main_pointer_shapes
        }
    }

    fn pointer_shapes_mut(&mut self) -> &mut PointerShapeStack {
        if self.alternate_active {
            &mut self.alternate_pointer_shapes
        } else {
            &mut self.main_pointer_shapes
        }
    }

    fn emit_pointer_shape(&mut self) {
        let shape = self.pointer_shapes().host_shape();
        self.events
            .push_back(TerminalEvent::MousePointerShape(shape));
    }

    pub(crate) fn resize(&mut self, columns: usize, rows: usize) {
        let columns = columns.max(2);
        let rows = rows.max(1);
        let dimensions_changed = {
            let grid = self.active();
            grid.columns() != columns || grid.rows() != rows
        };
        self.main.resize(columns, rows);
        self.alternate.resize(columns, rows);
        if dimensions_changed {
            self.clear_selection();
        }
    }

    pub(crate) const fn set_pixel_size(&mut self, width: u32, height: u32) {
        self.pixel_width = width;
        self.pixel_height = height;
    }

    pub(crate) fn frame_update(&mut self, force_full: bool) -> FrameUpdate {
        let cursor_visible = self.cursor_visible;
        let cursor_shape = self.cursor_shape;
        let cursor_blinking = self.cursor_blinking;
        let selection = self.selection.map(Selection::range);
        if self.synchronized_output && !force_full {
            let selection_dirty = std::mem::take(&mut self.selection_dirty);
            let grid = self.active();
            return FrameUpdate {
                full: false,
                columns: grid.columns(),
                rows: grid.rows(),
                row_moves: Vec::new(),
                row_updates: Vec::new(),
                metadata_changed: selection_dirty,
                cursor: CursorState {
                    row: grid.cursor.row,
                    column: grid.cursor.column,
                    visible: cursor_visible && grid.display_offset() == 0,
                    shape: cursor_shape,
                    blinking: cursor_blinking,
                },
                selection,
                display_offset: grid.display_offset(),
                revision: grid.damage.revision(),
            };
        }
        let selection_dirty = std::mem::take(&mut self.selection_dirty);
        let grid = self.active_mut();
        let columns = grid.columns();
        let rows = grid.rows();
        let snapshot = grid.damage.take(force_full, columns, rows);
        let full = snapshot.full;
        let metadata = snapshot.metadata;
        let revision = snapshot.revision;
        let spans = snapshot.spans;
        let row_moves = snapshot.moves.clone();
        let row_updates = spans
            .iter()
            .map(|span| RowUpdate {
                index: span.row,
                start_column: span.start,
                cells: grid.copy_view_span(span.row, span.start, span.end),
            })
            .collect();
        grid.damage.recycle_snapshot(spans, snapshot.moves);
        FrameUpdate {
            full,
            columns: grid.columns(),
            rows: grid.rows(),
            row_moves,
            row_updates,
            metadata_changed: metadata || selection_dirty,
            cursor: CursorState {
                row: grid.cursor.row,
                column: grid.cursor.column,
                visible: cursor_visible && grid.display_offset() == 0,
                shape: cursor_shape,
                blinking: cursor_blinking,
            },
            selection,
            display_offset: grid.display_offset(),
            revision,
        }
    }

    pub(crate) fn drain_events(&mut self) -> Vec<TerminalEvent> {
        self.events.drain(..).collect()
    }

    pub(crate) fn scroll_display(&mut self, lines: isize) -> bool {
        self.search.reset();
        let selection_changed = self.clear_selection();
        self.active_mut().scroll_display(lines) || selection_changed
    }

    pub(crate) fn scroll_to_bottom(&mut self) {
        self.search.reset();
        self.clear_selection();
        self.active_mut().scroll_to_bottom();
    }

    pub(crate) fn set_scrollback_limit(&mut self, limit: usize) {
        self.main.set_scrollback_limit(limit);
    }

    pub(crate) fn memory_stats(&self) -> crate::TerminalMemoryStats {
        let mut stats = crate::TerminalMemoryStats::default();
        self.main.add_memory_stats(&mut stats);
        self.alternate.add_memory_stats(&mut stats);
        stats
    }

    pub(crate) fn begin_selection(&mut self, point: SelectionPoint) -> bool {
        self.begin_selection_with_mode(point, SelectionMode::Character)
    }

    pub(crate) fn begin_selection_with_mode(
        &mut self,
        point: SelectionPoint,
        mode: SelectionMode,
    ) -> bool {
        self.search.reset();
        let range = self.selection_range(point, mode);
        self.selection = Some(Selection::new(range, mode));
        self.selection_dirty = true;
        true
    }

    pub(crate) fn update_selection(&mut self, point: SelectionPoint) -> bool {
        let Some(mode) = self.selection.map(Selection::mode) else {
            return false;
        };
        let range = self.selection_range(point, mode);
        let Some(selection) = &mut self.selection else {
            return false;
        };
        if !selection.set_head(range) {
            return false;
        }
        self.selection_dirty = true;
        true
    }

    fn selection_range(&self, point: SelectionPoint, mode: SelectionMode) -> SelectionRange {
        let point = self.normalize_selection_point(point);
        match mode {
            SelectionMode::Character => SelectionRange::point(point),
            SelectionMode::Word => self.active().word_selection_range(point),
            SelectionMode::Line => SelectionRange {
                start: SelectionPoint {
                    column: 0,
                    row: point.row,
                },
                end: SelectionPoint {
                    column: self.active().columns().saturating_sub(1),
                    row: point.row,
                },
            },
        }
    }

    fn normalize_selection_point(&self, point: SelectionPoint) -> SelectionPoint {
        let grid = self.active();
        let mut point = clamp_point(point, grid.columns(), grid.rows());
        point.column = grid.normalize_selection_column(point.row, point.column);
        point
    }

    pub(crate) fn hyperlink_at(&self, point: SelectionPoint) -> Option<Arc<String>> {
        let point = self.normalize_selection_point(point);
        self.active().hyperlink_at(point)
    }

    pub(crate) fn clear_selection(&mut self) -> bool {
        self.search.reset();
        if self.selection.take().is_some() {
            self.selection_dirty = true;
            true
        } else {
            false
        }
    }

    pub(crate) fn search_with_options(
        &mut self,
        query: &str,
        options: SearchOptions,
    ) -> Option<SearchMatch> {
        if query.is_empty() {
            self.reset_search();
            return None;
        }
        let origin = self.search.origin(query, options.case_sensitive);
        let Some((buffer_range, visible_range)) = self.active_mut().search(query, options, origin)
        else {
            if self.selection.take().is_some() {
                self.selection_dirty = true;
            }
            return None;
        };
        self.selection = Some(Selection::new(visible_range, SelectionMode::Character));
        self.selection_dirty = true;
        self.search.set_current(buffer_range);
        Some(SearchMatch {
            range: visible_range,
            display_offset: self.active().display_offset(),
        })
    }

    pub(crate) fn reset_search(&mut self) -> bool {
        self.search.reset();
        if self.selection.take().is_some() {
            self.selection_dirty = true;
            true
        } else {
            false
        }
    }

    pub(crate) fn invalidate_search(&mut self) {
        if self.search.is_active() {
            self.reset_search();
        }
    }

    pub(crate) fn selected_text(&self) -> Option<String> {
        let range = self.selection?.range();
        let grid = self.active();
        let mut selected = String::new();
        for row in range.start.row..=range.end.row {
            let start = if row == range.start.row {
                range.start.column
            } else {
                0
            };
            let end = if row == range.end.row {
                range.end.column.saturating_add(1)
            } else {
                grid.columns()
            };
            let mut line = String::new();
            for cell in grid.copy_view_span(row, start.min(grid.columns()), end.min(grid.columns()))
            {
                if !cell.flags.contains(CellFlags::WIDE_SPACER) {
                    line.push_str(cell.text.as_str());
                }
            }
            selected.push_str(line.trim_end_matches(' '));
            if row != range.end.row {
                selected.push('\n');
            }
        }
        Some(selected)
    }

    pub(crate) fn keyboard_flags(&self) -> KittyKeyboardFlags {
        self.keyboard().flags
    }

    pub(crate) const fn application_cursor(&self) -> bool {
        self.application_cursor
    }

    pub(crate) const fn application_keypad(&self) -> bool {
        self.application_keypad
    }

    pub(crate) const fn modify_other_keys(&self) -> ModifyOtherKeysState {
        self.modify_other_keys
    }

    pub(crate) const fn bracketed_paste(&self) -> bool {
        self.bracketed_paste
    }

    pub(crate) const fn focus_reporting(&self) -> bool {
        self.focus_reporting
    }

    pub(crate) const fn mouse_tracking(&self) -> MouseTrackingMode {
        self.mouse_tracking
    }

    pub(crate) const fn sgr_mouse(&self) -> bool {
        self.sgr_mouse
    }

    pub(crate) const fn pixel_mouse(&self) -> bool {
        self.pixel_mouse
    }

    fn reset(&mut self) {
        self.main.reset();
        self.alternate.reset();
        self.alternate_active = false;
        self.template = CellTemplate::default();
        self.application_cursor = false;
        self.application_keypad = false;
        self.insert_mode = false;
        self.autowrap = true;
        self.origin_mode = false;
        self.bracketed_paste = false;
        self.focus_reporting = false;
        self.mouse_tracking = MouseTrackingMode::Disabled;
        self.sgr_mouse = false;
        self.pixel_mouse = false;
        self.cursor_visible = true;
        self.cursor_shape = CursorShape::Block;
        self.cursor_blinking = true;
        self.synchronized_output = false;
        self.dynamic_foreground = DEFAULT_FOREGROUND_RGB;
        self.dynamic_background = DEFAULT_BACKGROUND_RGB;
        self.dynamic_cursor = DEFAULT_CURSOR_RGB;
        self.xtgettcap_query = None;
        self.selection = None;
        self.selection_dirty = true;
        self.main_keyboard = KeyboardMode::default();
        self.alternate_keyboard = KeyboardMode::default();
        self.modify_other_keys = ModifyOtherKeysState::default();
        self.main_pointer_shapes.clear();
        self.alternate_pointer_shapes.clear();
        self.events.push_back(TerminalEvent::ResetTitle);
        for target in [
            DynamicColor::Foreground,
            DynamicColor::Background,
            DynamicColor::Cursor,
        ] {
            self.events
                .push_back(TerminalEvent::ResetDynamicColor { target });
        }
        self.events
            .push_back(TerminalEvent::MousePointerShape(TERMINAL_DEFAULT_POINTER));
    }

    fn set_alternate_screen(&mut self, enabled: bool, clear: bool, save_cursor: bool) {
        if enabled == self.alternate_active {
            return;
        }
        let previous_pointer = self.pointer_shapes().host_shape();
        self.clear_selection();
        if enabled {
            if save_cursor {
                self.main.save_cursor();
            }
            if clear {
                self.alternate.reset();
            }
            self.alternate_active = true;
            self.alternate.damage.full();
        } else {
            self.alternate_active = false;
            if save_cursor {
                self.main.restore_cursor();
            }
            self.main.damage.full();
        }
        let current_pointer = self.pointer_shapes().host_shape();
        if current_pointer != previous_pointer {
            self.events
                .push_back(TerminalEvent::MousePointerShape(current_pointer));
        }
    }

    fn set_mode(&mut self, private: bool, mode: u16, enabled: bool) {
        if !private {
            if mode == 4 {
                self.insert_mode = enabled;
            }
            return;
        }
        match mode {
            1 => self.application_cursor = enabled,
            66 => self.application_keypad = enabled,
            6 => {
                self.origin_mode = enabled;
                self.active_mut().set_cursor(0, 0, enabled);
            }
            7 => self.autowrap = enabled,
            25 => {
                self.cursor_visible = enabled;
                self.active_mut().damage.metadata();
            }
            47 | 1047 => self.set_alternate_screen(enabled, enabled, false),
            1049 => self.set_alternate_screen(enabled, enabled, true),
            1000 => {
                self.mouse_tracking = if enabled {
                    MouseTrackingMode::Press
                } else {
                    MouseTrackingMode::Disabled
                };
            }
            1002 => {
                self.mouse_tracking = if enabled {
                    MouseTrackingMode::ButtonMotion
                } else {
                    MouseTrackingMode::Disabled
                };
            }
            1003 => {
                self.mouse_tracking = if enabled {
                    MouseTrackingMode::AnyMotion
                } else {
                    MouseTrackingMode::Disabled
                };
            }
            1004 => self.focus_reporting = enabled,
            1006 => self.sgr_mouse = enabled,
            1016 => self.pixel_mouse = enabled,
            2004 => self.bracketed_paste = enabled,
            2026 => self.synchronized_output = enabled,
            _ => {}
        }
    }

    fn csi(&mut self, params: &Params, intermediates: &[u8], action: char) {
        let groups = parameter_groups(params);
        let private = intermediates.contains(&b'?');
        let first = parameter(&groups, 0, 1);
        let origin_mode = self.origin_mode;
        match action {
            'A' => self.active_mut().move_cursor_relative(
                -(isize::try_from(first).unwrap_or(1)),
                0,
                origin_mode,
            ),
            'B' | 'e' => self.active_mut().move_cursor_relative(
                isize::try_from(first).unwrap_or(1),
                0,
                origin_mode,
            ),
            'C' | 'a' => self.active_mut().move_cursor_relative(
                0,
                isize::try_from(first).unwrap_or(1),
                origin_mode,
            ),
            'D' => self.active_mut().move_cursor_relative(
                0,
                -(isize::try_from(first).unwrap_or(1)),
                origin_mode,
            ),
            'E' => {
                self.active_mut().move_cursor_relative(
                    isize::try_from(first).unwrap_or(1),
                    0,
                    origin_mode,
                );
                self.active_mut().set_cursor_column(0);
            }
            'F' => {
                self.active_mut().move_cursor_relative(
                    -(isize::try_from(first).unwrap_or(1)),
                    0,
                    origin_mode,
                );
                self.active_mut().set_cursor_column(0);
            }
            'G' | '`' => self
                .active_mut()
                .set_cursor_column(usize::from(first.saturating_sub(1))),
            'f' if intermediates.contains(&b'>') => self.set_key_format(&groups),
            'H' | 'f' => {
                let row = usize::from(parameter(&groups, 0, 1).saturating_sub(1));
                let column = usize::from(parameter(&groups, 1, 1).saturating_sub(1));
                self.active_mut().set_cursor(row, column, origin_mode);
            }
            'd' => self
                .active_mut()
                .set_cursor_row(usize::from(first.saturating_sub(1)), origin_mode),
            'J' => {
                let template = self.template.clone();
                self.active_mut()
                    .erase_display(first_or_zero(&groups), &template);
            }
            'K' => {
                let template = self.template.clone();
                self.active_mut()
                    .erase_line(first_or_zero(&groups), &template);
            }
            'X' => {
                let template = self.template.clone();
                self.active_mut()
                    .erase_characters(usize::from(first), &template);
            }
            '@' => {
                let template = self.template.clone();
                self.active_mut()
                    .insert_blank_characters(usize::from(first), &template);
            }
            'P' => {
                let template = self.template.clone();
                self.active_mut()
                    .delete_characters(usize::from(first), &template);
            }
            'L' => {
                let template = self.template.clone();
                self.active_mut()
                    .insert_lines(usize::from(first), &template);
            }
            'M' => {
                let template = self.template.clone();
                self.active_mut()
                    .delete_lines(usize::from(first), &template);
            }
            'S' => {
                let template = self.template.clone();
                self.active_mut().scroll_up(usize::from(first), &template);
            }
            'T' => {
                let template = self.template.clone();
                self.active_mut().scroll_down(usize::from(first), &template);
            }
            'm' if intermediates.contains(&b'>') => self.set_key_modifier(&groups),
            'm' if private => self.query_key_modifier(&groups),
            'm' if intermediates.is_empty() => self.sgr(&groups),
            'h' | 'l' => {
                let enabled = action == 'h';
                for group in &groups {
                    self.set_mode(private, group.first().copied().unwrap_or(0), enabled);
                }
            }
            'r' => {
                if groups.is_empty() {
                    self.active_mut().reset_scroll_region();
                } else {
                    let top = usize::from(parameter(&groups, 0, 1).saturating_sub(1));
                    let default_bottom = u16::try_from(self.active().rows()).unwrap_or(u16::MAX);
                    let bottom =
                        usize::from(parameter(&groups, 1, default_bottom).saturating_sub(1));
                    self.active_mut().set_scroll_region(top, bottom);
                }
                self.active_mut().set_cursor(0, 0, origin_mode);
            }
            's' => self.active_mut().save_cursor(),
            'u' => self.kitty_or_restore_cursor(&groups, intermediates),
            'g' if private => self.query_key_format(&groups),
            'g' => self
                .active_mut()
                .clear_tab_stop(first_or_zero(&groups) == 3),
            'n' if intermediates.contains(&b'>') => self.disable_key_modifier(&groups),
            'n' => self.device_status(&groups, private),
            'p' if private && intermediates.contains(&b'$') => {
                self.request_private_mode(&groups);
            }
            'c' => self.device_attributes(intermediates),
            'q' if intermediates.contains(&b' ') => self.set_cursor_style(first_or_zero(&groups)),
            'q' if intermediates.contains(&b'>') => self
                .events
                .push_back(TerminalEvent::Reply(b"\x1bP>|Tmon 0.1.0\x1b\\".to_vec())),
            't' => self.window_operation(&groups),
            _ => {}
        }
    }

    fn kitty_or_restore_cursor(&mut self, groups: &[Vec<u16>], intermediates: &[u8]) {
        if intermediates.contains(&b'=') {
            let flags = known_kitty_keyboard_flags(parameter(groups, 0, 0));
            let mode = parameter(groups, 1, 1);
            self.keyboard_mut().apply(flags, mode);
        } else if intermediates.contains(&b'>') {
            let flags = known_kitty_keyboard_flags(parameter(groups, 0, 0));
            self.keyboard_mut().push(flags);
        } else if intermediates.contains(&b'<') {
            self.keyboard_mut()
                .pop(usize::from(parameter(groups, 0, 1)));
        } else if intermediates.contains(&b'?') {
            let flags = self.keyboard().flags.bits();
            self.events
                .push_back(TerminalEvent::Reply(format!("\x1b[?{flags}u").into_bytes()));
        } else {
            self.active_mut().restore_cursor();
        }
    }

    fn set_key_modifier(&mut self, groups: &[Vec<u16>]) {
        if groups.is_empty() {
            self.modify_other_keys.level = Some(0);
            self.modify_other_keys.excluded_modifiers = crate::Modifiers::empty();
            return;
        }
        let Some(resource) = groups.first() else {
            return;
        };
        if resource.first().copied().unwrap_or_default() != 4 {
            return;
        }
        if let Some(mask) = resource.get(1).copied() {
            self.modify_other_keys.excluded_modifiers = x_modifier_mask(mask);
            return;
        }
        if let Some(level) = groups.get(1).and_then(|group| group.first()).copied() {
            self.modify_other_keys.level = Some(level.min(3) as u8);
        } else {
            self.modify_other_keys.level = Some(0);
            self.modify_other_keys.excluded_modifiers = crate::Modifiers::empty();
        }
    }

    fn query_key_modifier(&mut self, groups: &[Vec<u16>]) {
        if first_or_zero(groups) != 4 {
            return;
        }
        let reply = if let Some(level) = self.modify_other_keys.level {
            format!("\x1b[>4;{level}m")
        } else {
            "\x1b[>4n".to_owned()
        };
        self.events
            .push_back(TerminalEvent::Reply(reply.into_bytes()));
    }

    fn disable_key_modifier(&mut self, groups: &[Vec<u16>]) {
        if first_or_zero(groups) == 4 {
            self.modify_other_keys.level = None;
        }
    }

    fn set_key_format(&mut self, groups: &[Vec<u16>]) {
        if groups.is_empty() {
            self.modify_other_keys.format = 0;
            return;
        }
        if first_or_zero(groups) != 4 {
            return;
        }
        self.modify_other_keys.format = groups
            .get(1)
            .and_then(|group| group.first())
            .copied()
            .unwrap_or_default()
            .min(1) as u8;
    }

    fn query_key_format(&mut self, groups: &[Vec<u16>]) {
        if first_or_zero(groups) == 4 {
            self.events.push_back(TerminalEvent::Reply(
                format!("\x1b[>4;{}f", self.modify_other_keys.format).into_bytes(),
            ));
        }
    }

    fn device_status(&mut self, groups: &[Vec<u16>], private: bool) {
        let value = parameter(groups, 0, 0);
        let reply = if private && value == 6 {
            let cursor = self.active().cursor;
            Some(format!("\x1b[?{};{}R", cursor.row + 1, cursor.column + 1))
        } else if value == 5 {
            Some("\x1b[0n".to_owned())
        } else if value == 6 {
            let cursor = self.active().cursor;
            Some(format!("\x1b[{};{}R", cursor.row + 1, cursor.column + 1))
        } else {
            None
        };
        if let Some(reply) = reply {
            self.events
                .push_back(TerminalEvent::Reply(reply.into_bytes()));
        }
    }

    fn request_private_mode(&mut self, groups: &[Vec<u16>]) {
        let mode = parameter(groups, 0, 0);
        let status = match mode {
            1 => mode_status(self.application_cursor),
            66 => mode_status(self.application_keypad),
            6 => mode_status(self.origin_mode),
            7 => mode_status(self.autowrap),
            25 => mode_status(self.cursor_visible),
            47 | 1047 | 1049 => mode_status(self.alternate_active),
            1000 => mode_status(self.mouse_tracking == MouseTrackingMode::Press),
            1002 => mode_status(self.mouse_tracking == MouseTrackingMode::ButtonMotion),
            1003 => mode_status(self.mouse_tracking == MouseTrackingMode::AnyMotion),
            1004 => mode_status(self.focus_reporting),
            1006 => mode_status(self.sgr_mouse),
            1016 => mode_status(self.pixel_mouse),
            2004 => mode_status(self.bracketed_paste),
            2026 => mode_status(self.synchronized_output),
            _ => 0,
        };
        self.events.push_back(TerminalEvent::Reply(
            format!("\x1b[?{mode};{status}$y").into_bytes(),
        ));
    }

    fn device_attributes(&mut self, intermediates: &[u8]) {
        let reply = if intermediates.contains(&b'>') {
            b"\x1b[>1;0;0c".to_vec()
        } else {
            b"\x1b[?62;22c".to_vec()
        };
        self.events.push_back(TerminalEvent::Reply(reply));
    }

    fn window_operation(&mut self, groups: &[Vec<u16>]) {
        let reply = match parameter(groups, 0, 0) {
            14 => Some(format!(
                "\x1b[4;{};{}t",
                self.pixel_height, self.pixel_width
            )),
            18 => {
                let grid = self.active();
                Some(format!("\x1b[8;{};{}t", grid.rows(), grid.columns()))
            }
            _ => None,
        };
        if let Some(reply) = reply {
            self.events
                .push_back(TerminalEvent::Reply(reply.into_bytes()));
        }
    }

    fn set_cursor_style(&mut self, value: u16) {
        self.cursor_shape = match value {
            3 | 4 => CursorShape::Underline,
            5 | 6 => CursorShape::Bar,
            _ => CursorShape::Block,
        };
        self.cursor_blinking = matches!(value, 0 | 1 | 3 | 5);
        self.active_mut().damage.metadata();
    }

    fn sgr(&mut self, groups: &[Vec<u16>]) {
        if groups.is_empty() {
            self.template = CellTemplate::default();
            return;
        }
        let mut index = 0;
        while index < groups.len() {
            let group = &groups[index];
            let code = group.first().copied().unwrap_or(0);
            match code {
                0 => self.template = CellTemplate::default(),
                1 => self.template.flags.insert(CellFlags::BOLD),
                2 => self.template.flags.insert(CellFlags::DIM),
                3 => self.template.flags.insert(CellFlags::ITALIC),
                4 => {
                    self.template.flags.insert(CellFlags::UNDERLINE);
                    if group.get(1) == Some(&2) {
                        self.template.flags.insert(CellFlags::DOUBLE_UNDERLINE);
                    }
                }
                5 | 6 => self.template.flags.insert(CellFlags::BLINK),
                7 => self.template.flags.insert(CellFlags::INVERSE),
                8 => self.template.flags.insert(CellFlags::HIDDEN),
                9 => self.template.flags.insert(CellFlags::STRIKEOUT),
                21 => self.template.flags.insert(CellFlags::DOUBLE_UNDERLINE),
                22 => self.template.flags.remove(CellFlags::BOLD | CellFlags::DIM),
                23 => self.template.flags.remove(CellFlags::ITALIC),
                24 => self
                    .template
                    .flags
                    .remove(CellFlags::UNDERLINE | CellFlags::DOUBLE_UNDERLINE),
                25 => self.template.flags.remove(CellFlags::BLINK),
                27 => self.template.flags.remove(CellFlags::INVERSE),
                28 => self.template.flags.remove(CellFlags::HIDDEN),
                29 => self.template.flags.remove(CellFlags::STRIKEOUT),
                30..=37 => self.template.foreground = Color::Indexed(to_u8(code - 30)),
                39 => self.template.foreground = Color::Default,
                40..=47 => self.template.background = Color::Indexed(to_u8(code - 40)),
                49 => self.template.background = Color::Default,
                90..=97 => self.template.foreground = Color::Indexed(to_u8(code - 90 + 8)),
                100..=107 => self.template.background = Color::Indexed(to_u8(code - 100 + 8)),
                38 | 48 => {
                    if let Some((color, consumed)) = extended_color(groups, index) {
                        if code == 38 {
                            self.template.foreground = color;
                        } else {
                            self.template.background = color;
                        }
                        index += consumed;
                    }
                }
                _ => {}
            }
            index += 1;
        }
    }

    fn osc(&mut self, params: &[&[u8]]) {
        let Some(command) = params
            .first()
            .and_then(|value| std::str::from_utf8(value).ok())
        else {
            return;
        };
        match command {
            "0" | "2" => {
                if let Some(mut title) = join_osc_limited(params, 1, OSC_TITLE_LIMIT) {
                    title.retain(|character| !character.is_control());
                    self.events.push_back(TerminalEvent::Title(title));
                }
            }
            "10" => self.osc_dynamic_color(DynamicColor::Foreground, 10, params),
            "11" => self.osc_dynamic_color(DynamicColor::Background, 11, params),
            "12" => self.osc_dynamic_color(DynamicColor::Cursor, 12, params),
            "110" => self.reset_dynamic_color(DynamicColor::Foreground),
            "111" => self.reset_dynamic_color(DynamicColor::Background),
            "112" => self.reset_dynamic_color(DynamicColor::Cursor),
            "7" => {
                if let Some(directory) = join_osc_limited(params, 1, OSC_DIRECTORY_LIMIT) {
                    self.events
                        .push_back(TerminalEvent::CurrentDirectory(directory));
                }
            }
            "8" => {
                let uri = join_osc_limited(params, 2, OSC_HYPERLINK_LIMIT);
                self.template.hyperlink = uri.filter(|uri| !uri.is_empty()).map(Arc::new);
            }
            "22" => self.osc_pointer_shape(params),
            "52" => self.osc_clipboard(params),
            _ => {}
        }
    }

    fn osc_dynamic_color(&mut self, target: DynamicColor, command: u8, params: &[&[u8]]) {
        let Some(value) = join_osc_limited(params, 1, OSC_DYNAMIC_COLOR_LIMIT) else {
            return;
        };
        if value == "?" {
            let [red, green, blue] = self.dynamic_color(target);
            let reply = format!(
                "\x1b]{command};rgb:{:04x}/{:04x}/{:04x}\x1b\\",
                u16::from(red) * 257,
                u16::from(green) * 257,
                u16::from(blue) * 257,
            );
            self.events
                .push_back(TerminalEvent::Reply(reply.into_bytes()));
        } else if value.eq_ignore_ascii_case("default") {
            self.reset_dynamic_color(target);
        } else if let Some(color) = parse_dynamic_color(&value) {
            *self.dynamic_color_mut(target) = color;
            self.events
                .push_back(TerminalEvent::SetDynamicColor { target, color });
        }
    }

    const fn dynamic_color(&self, target: DynamicColor) -> [u8; 3] {
        match target {
            DynamicColor::Foreground => self.dynamic_foreground,
            DynamicColor::Background => self.dynamic_background,
            DynamicColor::Cursor => self.dynamic_cursor,
        }
    }

    fn dynamic_color_mut(&mut self, target: DynamicColor) -> &mut [u8; 3] {
        match target {
            DynamicColor::Foreground => &mut self.dynamic_foreground,
            DynamicColor::Background => &mut self.dynamic_background,
            DynamicColor::Cursor => &mut self.dynamic_cursor,
        }
    }

    fn reset_dynamic_color(&mut self, target: DynamicColor) {
        *self.dynamic_color_mut(target) = default_dynamic_color(target);
        self.events
            .push_back(TerminalEvent::ResetDynamicColor { target });
    }

    fn osc_clipboard(&mut self, params: &[&[u8]]) {
        let selection = params
            .get(1)
            .and_then(|value| std::str::from_utf8(value).ok())
            .unwrap_or("c");
        if selection.len() > OSC_CLIPBOARD_SELECTION_LIMIT {
            return;
        }
        let Some(payload) = params.get(2) else {
            return;
        };
        if *payload == b"?" {
            return;
        }
        if payload.len() > OSC_CLIPBOARD_LIMIT.saturating_mul(2) {
            return;
        }
        if let Ok(decoded) = BASE64.decode(payload)
            && decoded.len() <= OSC_CLIPBOARD_LIMIT
            && let Ok(text) = String::from_utf8(decoded)
        {
            self.events.push_back(TerminalEvent::ClipboardStore {
                selection: selection.to_owned(),
                text,
            });
        }
    }

    fn osc_pointer_shape(&mut self, params: &[&[u8]]) {
        let Some(value) = join_osc_limited(params, 1, OSC_POINTER_SHAPE_LIMIT) else {
            return;
        };
        let (operation, names) = match value.as_bytes().first().copied() {
            Some(operation @ (b'=' | b'>' | b'<' | b'?')) => {
                (operation, value.get(1..).unwrap_or_default())
            }
            _ => (b'=', value.as_str()),
        };

        match operation {
            b'=' => {
                let mut changed = false;
                for name in names.split(',') {
                    let shape = if name.is_empty() {
                        None
                    } else if let Some(shape) = MousePointerShape::from_name(name) {
                        Some(shape)
                    } else {
                        continue;
                    };
                    self.pointer_shapes_mut().set(shape);
                    changed = true;
                }
                if changed {
                    self.emit_pointer_shape();
                }
            }
            b'>' => {
                let mut changed = false;
                for shape in names.split(',').filter_map(MousePointerShape::from_name) {
                    self.pointer_shapes_mut().push(shape);
                    changed = true;
                }
                if changed {
                    self.emit_pointer_shape();
                }
            }
            b'<' => {
                self.pointer_shapes_mut().pop();
                self.emit_pointer_shape();
            }
            b'?' => {
                let reply = names
                    .split(',')
                    .map(|name| match name {
                        "__current__" => self
                            .pointer_shapes()
                            .current()
                            .map_or("0", |shape| shape.name()),
                        "__default__" => TERMINAL_DEFAULT_POINTER.name(),
                        "__grabbed__" => GRABBED_POINTER.name(),
                        name => MousePointerShape::from_name(name).map_or("0", |_| "1"),
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                self.events.push_back(TerminalEvent::Reply(
                    format!("\x1b]22;{reply}\x1b\\").into_bytes(),
                ));
            }
            _ => unreachable!("OSC 22 operation was normalized"),
        }
    }
}

impl Perform for Emulator {
    fn print(&mut self, character: char) {
        let template = self.template.clone();
        if self.insert_mode {
            self.active_mut().insert_blank_characters(1, &template);
        }
        let autowrap = self.autowrap;
        self.active_mut().print(character, &template, autowrap);
    }

    fn hook(&mut self, _params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        self.xtgettcap_query = (!ignore && intermediates == b"+" && action == 'q').then(Vec::new);
    }

    fn put(&mut self, byte: u8) {
        if let Some(query) = &mut self.xtgettcap_query
            && query.len() < 4096
        {
            query.push(byte);
        }
    }

    fn unhook(&mut self) {
        let Some(query) = self.xtgettcap_query.take() else {
            return;
        };
        for capability in query.split(|byte| *byte == b';') {
            if capability.is_empty() {
                continue;
            }
            let mut reply = b"\x1bP0+r".to_vec();
            reply.extend_from_slice(capability);
            reply.extend_from_slice(b"\x1b\\");
            self.events.push_back(TerminalEvent::Reply(reply));
        }
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x07 => self.events.push_back(TerminalEvent::Bell),
            0x08 => self.active_mut().backspace(),
            0x09 => self.active_mut().tab(),
            0x0a..=0x0c => {
                let template = self.template.clone();
                self.active_mut().linefeed(&template);
            }
            0x0d => self.active_mut().carriage_return(),
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        self.osc(params);
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        if !ignore {
            self.csi(params, intermediates, action);
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
        if ignore || !intermediates.is_empty() {
            return;
        }
        match byte {
            b'D' => {
                let template = self.template.clone();
                self.active_mut().linefeed(&template);
            }
            b'E' => {
                let template = self.template.clone();
                self.active_mut().carriage_return();
                self.active_mut().linefeed(&template);
            }
            b'H' => self.active_mut().set_tab_stop(),
            b'M' => {
                let template = self.template.clone();
                self.active_mut().reverse_index(&template);
            }
            b'7' => self.active_mut().save_cursor(),
            b'8' => self.active_mut().restore_cursor(),
            b'=' => self.application_keypad = true,
            b'>' => self.application_keypad = false,
            b'c' => self.reset(),
            _ => {}
        }
    }
}

fn parameter_groups(params: &Params) -> Vec<Vec<u16>> {
    params.iter().map(<[u16]>::to_vec).collect()
}

fn parameter(groups: &[Vec<u16>], index: usize, default: u16) -> u16 {
    groups
        .get(index)
        .and_then(|group| group.first())
        .copied()
        .filter(|value| *value != 0)
        .unwrap_or(default)
}

fn first_or_zero(groups: &[Vec<u16>]) -> u16 {
    groups
        .first()
        .and_then(|group| group.first())
        .copied()
        .unwrap_or(0)
}

fn known_kitty_keyboard_flags(value: u16) -> KittyKeyboardFlags {
    let known = value & u16::from(KittyKeyboardFlags::all().bits());
    let bits = u8::try_from(known).expect("Kitty keyboard flags fit in u8 after masking");
    KittyKeyboardFlags::from_bits_retain(bits)
}

fn x_modifier_mask(mask: u16) -> crate::Modifiers {
    const SHIFT_MASK: u16 = 1;
    const LOCK_MASK: u16 = 2;
    const CONTROL_MASK: u16 = 4;
    const MOD1_MASK: u16 = 8;

    let mut modifiers = crate::Modifiers::empty();
    modifiers.set(crate::Modifiers::SHIFT, mask & SHIFT_MASK != 0);
    modifiers.set(crate::Modifiers::CAPS_LOCK, mask & LOCK_MASK != 0);
    modifiers.set(crate::Modifiers::CONTROL, mask & CONTROL_MASK != 0);
    modifiers.set(crate::Modifiers::ALT, mask & MOD1_MASK != 0);
    // Mod2 through Mod5 are assigned by the active X keymap, so they cannot be translated to
    // NumLock, Meta, Super, or Hyper reliably without keymap information.
    modifiers
}

fn extended_color(groups: &[Vec<u16>], index: usize) -> Option<(Color, usize)> {
    let group = groups.get(index)?;
    if group.len() > 1 {
        return match group.get(1).copied() {
            Some(5) => group
                .last()
                .copied()
                .map(|value| (Color::Indexed(to_u8(value)), 0)),
            Some(2) if group.len() >= 5 => {
                let values = &group[group.len() - 3..];
                Some((
                    Color::Rgb(to_u8(values[0]), to_u8(values[1]), to_u8(values[2])),
                    0,
                ))
            }
            _ => None,
        };
    }
    match parameter(groups, index + 1, 0) {
        5 => Some((Color::Indexed(to_u8(parameter(groups, index + 2, 0))), 2)),
        2 => Some((
            Color::Rgb(
                to_u8(parameter(groups, index + 2, 0)),
                to_u8(parameter(groups, index + 3, 0)),
                to_u8(parameter(groups, index + 4, 0)),
            ),
            4,
        )),
        _ => None,
    }
}

fn join_osc_limited(params: &[&[u8]], start: usize, limit: usize) -> Option<String> {
    let values = params.get(start..).unwrap_or_default();
    let separators = values.len().saturating_sub(1);
    let length = values
        .iter()
        .try_fold(separators, |length, value| length.checked_add(value.len()))?;
    if length > limit {
        return None;
    }
    let mut joined = String::with_capacity(length);
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            joined.push(';');
        }
        joined.push_str(std::str::from_utf8(value).ok()?);
    }
    Some(joined)
}

fn to_u8(value: u16) -> u8 {
    u8::try_from(value.min(u16::from(u8::MAX))).expect("value was clamped to u8")
}

const fn default_dynamic_color(target: DynamicColor) -> [u8; 3] {
    match target {
        DynamicColor::Foreground => DEFAULT_FOREGROUND_RGB,
        DynamicColor::Background => DEFAULT_BACKGROUND_RGB,
        DynamicColor::Cursor => DEFAULT_CURSOR_RGB,
    }
}

fn parse_dynamic_color(value: &str) -> Option<[u8; 3]> {
    if let Some(hex) = value.strip_prefix('#') {
        let width = hex.len().checked_div(3)?;
        if !(1..=4).contains(&width) || width * 3 != hex.len() {
            return None;
        }
        return Some([
            parse_hex_channel(&hex[..width])?,
            parse_hex_channel(&hex[width..width * 2])?,
            parse_hex_channel(&hex[width * 2..])?,
        ]);
    }

    let rgb = value.strip_prefix("rgb:")?;
    let mut channels = rgb.split('/');
    let color = [
        parse_hex_channel(channels.next()?)?,
        parse_hex_channel(channels.next()?)?,
        parse_hex_channel(channels.next()?)?,
    ];
    channels.next().is_none().then_some(color)
}

fn parse_hex_channel(value: &str) -> Option<u8> {
    if !(1..=4).contains(&value.len()) {
        return None;
    }
    let raw = u32::from_str_radix(value, 16).ok()?;
    let maximum = (1_u32 << (value.len() * 4)) - 1;
    u8::try_from((raw * 255 + maximum / 2) / maximum).ok()
}

const fn mode_status(enabled: bool) -> u8 {
    if enabled { 1 } else { 2 }
}
