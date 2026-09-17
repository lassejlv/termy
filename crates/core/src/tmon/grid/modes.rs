use super::view::*;
use super::*;

impl Grid {
    pub(crate) fn reset(&mut self) {
        self.primary.reset();
        self.alternate = None;
        self.alternate_active = false;
        self.history.clear();
        self.history.shrink_to_fit();
        self.display_offset = 0;
        self.resize_anchor_suppressed_after_clear = false;
        self.cursor_visible = true;
        self.cursor_style = self.default_cursor_style;
        self.cursor_style_explicit = false;
        self.cursor_shape_status = cursor_shape_status(self.default_cursor_style);
        self.auto_wrap = true;
        self.insert_mode = false;
        self.origin_mode = false;
        self.line_feed_new_line = false;
        self.bracketed_paste = false;
        self.cursor_blinking = false;
        self.focus_reporting = false;
        self.alternate_scroll = true;
        self.urgency_hints = true;
        self.mouse_mode = MouseMode::default();
        self.keyboard_mode = KeyboardMode::default();
        self.kitty_keyboard_stack.clear();
        self.inactive_kitty_keyboard_stack.clear();
        self.tab_stops = default_tab_stops(self.cols());
        self.hyperlinks.clear();
        self.hyperlinks.shrink_to_fit();
        self.hyperlink_identities.clear();
        self.hyperlink_identities.shrink_to_fit();
        self.next_hyperlink_id = 1;
        self.next_hyperlink_prune_len = HYPERLINK_PRUNE_MIN_LEN;
        self.extras.clear();
        self.extras.shrink_to_fit();
        self.next_extra_id = 1;
        self.effects.clear();
        self.effects.push_back(GridEffect::Reset);
        self.damage.mark_full();
    }

    pub(crate) fn palette(&self) -> Palette {
        self.palette
    }

    pub(crate) fn set_indexed_color(&mut self, index: u8, color: Rgb) {
        let slot = &mut self.palette.indexed[usize::from(index)];
        if *slot != Some(color) {
            *slot = Some(color);
            self.palette.bump_revision();
            self.damage.mark_full();
        }
    }

    pub(crate) fn reset_indexed_color(&mut self, index: u8) {
        let slot = &mut self.palette.indexed[usize::from(index)];
        if slot.take().is_some() {
            self.palette.bump_revision();
            self.damage.mark_full();
        }
    }

    pub(crate) fn reset_indexed_colors(&mut self) {
        if self.palette.indexed.iter().any(Option::is_some) {
            self.palette.indexed.fill(None);
            self.palette.bump_revision();
            self.damage.mark_full();
        }
    }

    pub(crate) fn set_foreground_color(&mut self, color: Option<Rgb>) {
        if self.palette.foreground != color {
            self.palette.foreground = color;
            self.palette.bump_revision();
            self.damage.mark_full();
        }
    }

    pub(crate) fn set_background_color(&mut self, color: Option<Rgb>) {
        if self.palette.background != color {
            self.palette.background = color;
            self.palette.bump_revision();
            self.damage.mark_full();
        }
    }

    pub(crate) fn set_cursor_color(&mut self, color: Option<Rgb>) {
        if self.palette.cursor != color {
            self.palette.cursor = color;
            self.palette.bump_revision();
            self.damage.mark_full();
        }
    }

    pub(crate) fn set_mode(&mut self, private: bool, mode: u16, enabled: bool) {
        if !private {
            match mode {
                4 => self.insert_mode = enabled,
                20 => self.line_feed_new_line = enabled,
                _ => {}
            }
            return;
        }

        match mode {
            1 => self.keyboard_mode.application_cursor_keys = enabled,
            3 => self.column_mode_reset(),
            6 => {
                self.origin_mode = enabled;
                if enabled {
                    let screen = self.active_mut();
                    screen.cursor_row = screen.scroll_top;
                    screen.cursor_col = 0;
                    screen.wrap_pending = false;
                }
            }
            7 => self.auto_wrap = enabled,
            12 => {
                self.cursor_blinking = enabled;
                self.cursor_style_explicit = true;
            }
            25 => self.cursor_visible = enabled,
            47 | 1047 => self.set_alternate_screen(enabled, false),
            1048 => {
                if enabled {
                    self.save_cursor();
                } else {
                    self.restore_cursor();
                }
            }
            1049 => self.set_alternate_screen(enabled, true),
            1000 => {
                if enabled {
                    self.mouse_mode.report_drag = false;
                    self.mouse_mode.report_motion = false;
                }
                self.mouse_mode.report_click = enabled;
            }
            1002 => {
                if enabled {
                    self.mouse_mode.report_click = false;
                    self.mouse_mode.report_motion = false;
                }
                self.mouse_mode.report_drag = enabled;
            }
            1003 => {
                if enabled {
                    self.mouse_mode.report_click = false;
                    self.mouse_mode.report_drag = false;
                }
                self.mouse_mode.report_motion = enabled;
            }
            1004 => self.focus_reporting = enabled,
            1005 => {
                if enabled {
                    self.mouse_mode.sgr_encoding = false;
                }
                self.mouse_mode.utf8_encoding = enabled;
            }
            1006 => {
                if enabled {
                    self.mouse_mode.utf8_encoding = false;
                }
                self.mouse_mode.sgr_encoding = enabled;
            }
            1007 => self.alternate_scroll = enabled,
            1042 => self.urgency_hints = enabled,
            2004 => self.bracketed_paste = enabled,
            _ => {}
        }
        self.mouse_mode.enabled = self.mouse_mode.report_click
            || self.mouse_mode.report_drag
            || self.mouse_mode.report_motion;
    }

    pub(super) fn column_mode_reset(&mut self) {
        self.reset_scroll_region();
        let rows = self.rows();
        let cols = self.cols();
        self.effects.push_back(GridEffect::ClearViewport {
            alternate: self.alternate_active,
            history_size: self.history_size(),
            rows,
            cols,
        });
        let blank = self.pen().blank();
        self.active_mut().fill(blank);
        self.damage.mark_full();
    }

    pub(crate) fn set_keypad_application_mode(&mut self, enabled: bool) {
        self.keyboard_mode.application_keypad = enabled;
    }

    pub(crate) fn alignment_test(&mut self) {
        self.active_mut().fill(Cell {
            character: 'E',
            ..Cell::default()
        });
        self.damage.mark_full();
    }

    pub(crate) fn mode_state(&self, private: bool, mode: u16) -> u8 {
        let supported = if private {
            match mode {
                1 => Some(self.keyboard_mode.application_cursor_keys),
                3 => None,
                6 => Some(self.origin_mode),
                7 => Some(self.auto_wrap),
                12 => Some(self.cursor_blinking),
                25 => Some(self.cursor_visible),
                47 | 1047 | 1049 => Some(self.alternate_active),
                1000 => Some(self.mouse_mode.report_click),
                1002 => Some(self.mouse_mode.report_drag),
                1003 => Some(self.mouse_mode.report_motion),
                1004 => Some(self.focus_reporting),
                1005 => Some(self.mouse_mode.utf8_encoding),
                1006 => Some(self.mouse_mode.sgr_encoding),
                1007 => Some(self.alternate_scroll),
                1042 => Some(self.urgency_hints),
                2004 => Some(self.bracketed_paste),
                2026 => Some(false),
                _ => return 0,
            }
        } else {
            match mode {
                4 => Some(self.insert_mode),
                20 => Some(self.line_feed_new_line),
                _ => return 0,
            }
        };
        match supported {
            Some(true) => 1,
            Some(false) => 2,
            None => 0,
        }
    }

    pub(super) fn set_alternate_screen(&mut self, enabled: bool, save_cursor: bool) {
        if enabled == self.alternate_active {
            return;
        }
        if enabled {
            if save_cursor {
                self.primary.save_cursor();
            }
            let cursor_col = self.primary.cursor_col;
            let cursor_row = self.primary.cursor_row;
            let pen = self.primary.pen;
            let charsets = self.primary.charsets;
            let wrap_pending = self.primary.wrap_pending;
            let scroll_top = self.primary.scroll_top;
            let scroll_bottom = self.primary.scroll_bottom;
            let cols = self.primary.cols;
            let rows = self.primary.rows;
            let alternate = self
                .alternate
                .get_or_insert_with(|| Screen::new(cols, rows));
            alternate.fill(pen.blank());
            alternate.cursor_col = cursor_col.min(alternate.cols.saturating_sub(1));
            alternate.cursor_row = cursor_row.min(alternate.rows.saturating_sub(1));
            alternate.pen = pen;
            alternate.charsets = charsets;
            alternate.wrap_pending = wrap_pending;
            alternate.scroll_top = scroll_top.min(alternate.rows.saturating_sub(1));
            alternate.scroll_bottom = scroll_bottom.min(alternate.rows.saturating_sub(1));
            self.alternate_active = true;
            self.effects.push_back(GridEffect::EnteredAlternate);
        } else {
            self.alternate_active = false;
            self.effects.push_back(GridEffect::ExitedAlternate);
        }
        std::mem::swap(
            &mut self.kitty_keyboard_stack,
            &mut self.inactive_kitty_keyboard_stack,
        );
        let flags = self.kitty_keyboard_stack.last().copied().unwrap_or(0);
        self.set_kitty_keyboard_flags(flags, 1);
        self.damage.mark_full();
    }

    pub(crate) fn set_cursor_style(&mut self, parameter: u16) {
        let (style, shape_status, blinking, explicit) = match parameter {
            0 => (
                self.default_cursor_style,
                cursor_shape_status(self.default_cursor_style),
                false,
                false,
            ),
            1 | 2 => (CursorStyle::Block, 2, parameter == 1, true),
            3 | 4 => (CursorStyle::Line, 4, parameter == 3, true),
            5 | 6 => (CursorStyle::Line, 6, parameter == 5, true),
            _ => return,
        };
        self.cursor_style = style;
        self.cursor_shape_status = shape_status;
        self.cursor_blinking = blinking;
        self.cursor_style_explicit = explicit;
        self.damage.mark_full();
    }

    pub(crate) fn set_cursor_shape(&mut self, parameter: u16) {
        let (style, shape_status) = match parameter {
            0 => (CursorStyle::Block, 2),
            1 => (CursorStyle::Line, 6),
            2 => (CursorStyle::Line, 4),
            _ => return,
        };
        self.cursor_style = style;
        self.cursor_shape_status = shape_status;
        self.cursor_style_explicit = true;
        self.damage.mark_full();
    }

    pub(crate) fn cursor_style_status(&self) -> u16 {
        self.cursor_shape_status - u16::from(self.cursor_blinking)
    }

    pub(crate) fn set_character_protection(&mut self, parameter: u16) {
        self.pen_mut().protected = parameter == 1;
    }

    pub(crate) fn character_protection_status(&self) -> u16 {
        u16::from(self.pen().protected)
    }

    pub(crate) fn scroll_region_status(&self) -> (usize, usize) {
        let screen = self.active();
        (screen.scroll_top + 1, screen.scroll_bottom + 1)
    }

    pub(crate) fn scroll_region_covers_full_screen(&self) -> bool {
        let screen = self.active();
        screen.scroll_top == 0 && screen.scroll_bottom == screen.rows.saturating_sub(1)
    }

    pub(crate) fn sgr_status(&self) -> String {
        let mut codes = Vec::with_capacity(12);
        let pen = self.pen();
        let attributes = pen.attributes;
        if attributes.bold() {
            codes.push("1".to_string());
        }
        if attributes.dim() {
            codes.push("2".to_string());
        }
        if attributes.italic() {
            codes.push("3".to_string());
        }
        match attributes.underline_style() {
            UnderlineStyle::None => {}
            UnderlineStyle::Single => codes.push("4".to_string()),
            UnderlineStyle::Double => codes.push("4:2".to_string()),
            UnderlineStyle::Curly => codes.push("4:3".to_string()),
            UnderlineStyle::Dotted => codes.push("4:4".to_string()),
            UnderlineStyle::Dashed => codes.push("4:5".to_string()),
        }
        if attributes.inverse() {
            codes.push("7".to_string());
        }
        if attributes.hidden() {
            codes.push("8".to_string());
        }
        if attributes.strikethrough() {
            codes.push("9".to_string());
        }
        push_sgr_color(&mut codes, pen.foreground, true);
        push_sgr_color(&mut codes, pen.background, false);
        push_underline_sgr_color(&mut codes, pen.underline_color);
        if codes.is_empty() {
            codes.push("0".to_string());
        }
        format!("{}m", codes.join(";"))
    }

    pub(crate) fn set_hyperlink(&mut self, protocol_id: Option<&str>, target: Option<&str>) {
        let Some(target) = target.filter(|target| !target.is_empty()) else {
            self.pen_mut().hyperlink_id = None;
            return;
        };

        // Alacritty treats each OSC 8 link without an explicit `id` as a new
        // identity. Explicit IDs, however, reconnect equal `id + uri` pairs.
        // Index their combined identity so reopening a link does not scan all
        // retained links. The full scan is only a collision-safe fallback.
        let identity_hash =
            protocol_id.map(|protocol_id| self.hyperlink_identity_hash(protocol_id, target));
        if let (Some(protocol_id), Some(identity_hash)) = (protocol_id, identity_hash) {
            let indexed_id = self.hyperlink_identities.get(&identity_hash).copied();
            let id = match indexed_id {
                Some(id)
                    if self.hyperlinks.get(&id).is_some_and(|existing| {
                        existing.protocol_id.as_deref() == Some(protocol_id)
                            && existing.target == target
                    }) =>
                {
                    Some(id)
                }
                Some(_) => self.hyperlinks.iter().find_map(|(id, existing)| {
                    (existing.protocol_id.as_deref() == Some(protocol_id)
                        && existing.target == target)
                        .then_some(*id)
                }),
                None => None,
            };
            if let Some(id) = id {
                self.pen_mut().hyperlink_id = NonZeroU32::new(id);
                return;
            }
        }
        if self.hyperlinks.len() >= self.next_hyperlink_prune_len {
            self.prune_hyperlinks();
            // A screen can legitimately retain thousands of live links. Once
            // a full grid scan finds that many, amortize the next scan instead
            // of rescanning every time another OSC 8 link is opened.
            self.next_hyperlink_prune_len = self
                .hyperlinks
                .len()
                .saturating_add(HYPERLINK_PRUNE_INTERVAL)
                .max(HYPERLINK_PRUNE_MIN_LEN);
        }
        let id = self.next_available_hyperlink_id();
        self.hyperlinks.insert(
            id,
            HyperlinkEntry {
                protocol_id: protocol_id.map(str::to_owned),
                target: target.to_string(),
            },
        );
        if let Some(identity_hash) = identity_hash {
            self.hyperlink_identities.entry(identity_hash).or_insert(id);
        }
        self.pen_mut().hyperlink_id = NonZeroU32::new(id);
    }

    pub(super) fn next_available_hyperlink_id(&mut self) -> u32 {
        loop {
            let id = self.next_hyperlink_id.clamp(1, METADATA_VALUE_MASK);
            self.next_hyperlink_id = if id == METADATA_VALUE_MASK { 1 } else { id + 1 };
            if !self.hyperlinks.contains_key(&id) {
                return id;
            }
        }
    }

    pub(super) fn hyperlink_identity_hash(&self, protocol_id: &str, target: &str) -> u64 {
        self.hyperlinks.hasher().hash_one((protocol_id, target))
    }

    pub(super) fn prune_hyperlinks(&mut self) {
        let mut live = HashSet::new();
        let extras = &self.extras;
        live.extend(
            self.primary
                .cells
                .iter()
                .flatten()
                .filter_map(|cell| cell_hyperlink_id(extras, cell)),
        );
        if let Some(alternate) = &self.alternate {
            live.extend(
                alternate
                    .cells
                    .iter()
                    .flatten()
                    .filter_map(|cell| cell_hyperlink_id(extras, cell)),
            );
        }
        live.extend(self.history.iter().flat_map(|row| {
            row.iter()
                .filter_map(|cell| cell_hyperlink_id(extras, &cell))
        }));
        let mut retain_pen = |pen: Pen| {
            if let Some(active) = pen.hyperlink_id {
                live.insert(active);
            }
        };
        retain_pen(self.primary.pen);
        retain_pen(self.primary.saved_pen);
        if let Some(alternate) = &self.alternate {
            retain_pen(alternate.pen);
            retain_pen(alternate.saved_pen);
        }
        self.hyperlinks
            .retain(|id, _| NonZeroU32::new(*id).is_some_and(|id| live.contains(&id)));
        self.hyperlink_identities.clear();
        for (id, entry) in &self.hyperlinks {
            let Some(protocol_id) = entry.protocol_id.as_deref() else {
                continue;
            };
            self.hyperlink_identities
                .entry(
                    self.hyperlinks
                        .hasher()
                        .hash_one((protocol_id, entry.target.as_str())),
                )
                .or_insert(*id);
        }
    }

    pub(crate) fn set_kitty_keyboard_flags(&mut self, flags: u16, mode: u16) {
        let apply = |current: &mut bool, bit: u16| match mode {
            2 => *current |= flags & bit != 0,
            3 => *current &= flags & bit == 0,
            _ => *current = flags & bit != 0,
        };
        apply(&mut self.keyboard_mode.disambiguate_escape_codes, 1 << 0);
        apply(&mut self.keyboard_mode.report_event_types, 1 << 1);
        apply(&mut self.keyboard_mode.report_alternate_keys, 1 << 2);
        apply(&mut self.keyboard_mode.report_all_keys_as_esc, 1 << 3);
        apply(&mut self.keyboard_mode.report_associated_text, 1 << 4);
    }

    pub(crate) fn kitty_keyboard_report_flags(&self) -> u16 {
        self.kitty_keyboard_stack.last().copied().unwrap_or(0)
    }

    pub(crate) fn push_kitty_keyboard_flags(&mut self, flags: u16) {
        if self.kitty_keyboard_stack.len() >= KITTY_KEYBOARD_STACK_MAX_DEPTH {
            self.kitty_keyboard_stack.remove(0);
        }
        self.kitty_keyboard_stack.push(flags & 0x1f);
        self.set_kitty_keyboard_flags(flags, 1);
    }

    pub(crate) fn pop_kitty_keyboard_flags(&mut self, count: usize) {
        let count = count.max(1).min(self.kitty_keyboard_stack.len());
        for _ in 0..count {
            self.kitty_keyboard_stack.pop();
        }
        let flags = self.kitty_keyboard_stack.last().copied().unwrap_or(0);
        self.set_kitty_keyboard_flags(flags, 1);
    }

    #[cfg(test)]
    pub(crate) fn sgr(&mut self, params: &[u16]) {
        self.sgr_with_underline_styles(params, &[]);
    }

    pub(crate) fn sgr_with_underline_styles(
        &mut self,
        params: &[u16],
        underline_styles: &[Option<UnderlineStyle>],
    ) {
        if params.is_empty() {
            let protected = self.pen().protected;
            let hyperlink_id = self.pen().hyperlink_id;
            *self.pen_mut() = Pen {
                protected,
                hyperlink_id,
                ..Pen::default()
            };
            return;
        }

        let pen = self.pen_mut();
        let mut index = 0;
        while index < params.len() {
            let code = params[index];
            match code {
                0 => {
                    let protected = pen.protected;
                    let hyperlink_id = pen.hyperlink_id;
                    *pen = Pen {
                        protected,
                        hyperlink_id,
                        ..Pen::default()
                    };
                }
                1 => pen.attributes.set_bold(true),
                2 => pen.attributes.set_dim(true),
                3 => pen.attributes.set_italic(true),
                4 => pen.attributes.set_underline_style(
                    underline_styles
                        .get(index)
                        .copied()
                        .flatten()
                        .unwrap_or(UnderlineStyle::Single),
                ),
                7 => pen.attributes.set_inverse(true),
                8 => pen.attributes.set_hidden(true),
                9 => pen.attributes.set_strikethrough(true),
                21 => pen.attributes.set_bold(false),
                22 => {
                    pen.attributes.set_bold(false);
                    pen.attributes.set_dim(false);
                }
                23 => pen.attributes.set_italic(false),
                24 => pen.attributes.set_underline(false),
                27 => pen.attributes.set_inverse(false),
                28 => pen.attributes.set_hidden(false),
                29 => pen.attributes.set_strikethrough(false),
                30..=37 => pen.foreground = Color::Indexed((code - 30) as u8),
                38 => {
                    if let Some(color) = extended_color(params, &mut index) {
                        pen.foreground = color;
                    }
                }
                39 => pen.foreground = Color::Default,
                40..=47 => pen.background = Color::Indexed((code - 40) as u8),
                48 => {
                    if let Some(color) = extended_color(params, &mut index) {
                        pen.background = color;
                    }
                }
                49 => pen.background = Color::Default,
                58 => {
                    if let Some(color) = extended_color(params, &mut index) {
                        pen.underline_color = Some(color);
                    }
                }
                59 => pen.underline_color = None,
                90..=97 => pen.foreground = Color::Indexed((code - 90 + 8) as u8),
                100..=107 => pen.background = Color::Indexed((code - 100 + 8) as u8),
                _ => {}
            }
            index += 1;
        }
    }
}
