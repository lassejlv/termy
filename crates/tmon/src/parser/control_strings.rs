use super::*;

impl Parser {
    pub(super) fn start_osc(&mut self) {
        self.state = State::Osc;
        self.osc.clear();
        self.osc_oversized = false;
    }

    pub(super) fn start_dcs(&mut self) {
        self.state = State::Dcs;
        self.dcs.clear();
        self.dcs_oversized = false;
        self.dcs_phase = DcsPhase::Entry;
    }

    pub(super) fn advance_dcs(&mut self, grid: &mut Grid, byte: u8, output: &mut ParseOutput) {
        if byte == 0x1b {
            self.finish_dcs(grid, output);
            self.state = State::Escape;
            return;
        }

        match self.dcs_phase {
            DcsPhase::Entry => match byte {
                0x20..=0x2f => {
                    self.push_dcs(byte);
                    self.dcs_phase = DcsPhase::Intermediate;
                }
                0x30..=0x3f => {
                    self.push_dcs(byte);
                    self.dcs_phase = DcsPhase::Param;
                }
                0x40..=0x7e => {
                    self.push_dcs(byte);
                    self.dcs_phase = DcsPhase::Passthrough;
                }
                _ => {}
            },
            DcsPhase::Param => match byte {
                0x20..=0x2f => {
                    self.push_dcs(byte);
                    self.dcs_phase = DcsPhase::Intermediate;
                }
                0x30..=0x3b => self.push_dcs(byte),
                0x3c..=0x3f => self.dcs_phase = DcsPhase::Ignore,
                0x40..=0x7e => {
                    self.push_dcs(byte);
                    self.dcs_phase = DcsPhase::Passthrough;
                }
                _ => {}
            },
            DcsPhase::Intermediate => match byte {
                0x20..=0x2f => self.push_dcs(byte),
                0x30..=0x3f => self.dcs_phase = DcsPhase::Ignore,
                0x40..=0x7e => {
                    self.push_dcs(byte);
                    self.dcs_phase = DcsPhase::Passthrough;
                }
                _ => {}
            },
            DcsPhase::Ignore => {}
            DcsPhase::Passthrough => match byte {
                0x20..=0x7e => self.push_dcs(byte),
                0x9c => {
                    self.finish_dcs(grid, output);
                    self.state = State::Ground;
                }
                _ => {}
            },
        }
    }

    pub(super) fn push_dcs(&mut self, byte: u8) {
        if self.dcs.len() < MAX_DCS_BYTES {
            self.dcs.push(byte);
        } else {
            self.dcs_oversized = true;
        }
    }

    pub(super) fn push_kitty(&mut self, byte: u8) {
        if self.kitty_oversized {
            return;
        }
        if !self.kitty_payload_started {
            if byte == b';' {
                self.kitty_payload_started = true;
            } else if self.kitty.len() >= MAX_CONTROL_BYTES {
                self.kitty_oversized = true;
                return;
            }
        }
        if self.kitty.len() < MAX_COMMAND_BYTES {
            self.kitty.push(byte);
        } else {
            self.kitty_oversized = true;
        }
    }

    pub(super) fn finish_kitty(&mut self, grid: &mut Grid, output: &mut ParseOutput) {
        // Apply preceding text scroll/clear effects before anchoring this image. Remaining
        // effects are drained once at the end of the PTY batch instead of once per byte.
        self.sync_grid_effects(grid);
        let command = GraphicsCommand::parse(
            std::mem::take(&mut self.kitty),
            std::mem::take(&mut self.kitty_oversized),
        );
        self.kitty_payload_started = false;
        let result = self.graphics.apply(command, grid, self.size);
        output.replies.extend(result.replies);
        if result.changed {
            self.graphics.bump_revision();
            grid.mark_full_damage();
        }
    }

    pub(super) fn finish_dcs(&mut self, grid: &Grid, output: &mut ParseOutput) {
        if std::mem::take(&mut self.dcs_oversized) {
            self.dcs.clear();
            return;
        }
        let payload = self.dcs.as_slice();
        if let Some(request) = payload.strip_prefix(b"$q") {
            let response = match request {
                b"m" => Some(grid.sgr_status()),
                b"r" => {
                    let (top, bottom) = grid.scroll_region_status();
                    Some(format!("{top};{bottom}r"))
                }
                b" q" => Some(format!("{} q", grid.cursor_style_status())),
                b"\"p" => Some("65;1\"p".to_string()),
                b"\"q" => Some(format!("{}\"q", grid.character_protection_status())),
                _ => None,
            };
            output.replies.extend_from_slice(b"\x1bP");
            match response {
                Some(response) => {
                    output.replies.extend_from_slice(b"1$r");
                    output.replies.extend_from_slice(response.as_bytes());
                }
                None => output.replies.extend_from_slice(b"0$r"),
            }
            output.replies.extend_from_slice(b"\x1b\\");
        } else if let Some(requests) = payload.strip_prefix(b"+q") {
            for encoded_name in requests.split(|byte| *byte == b';') {
                if encoded_name.is_empty() {
                    continue;
                }
                let Some(name) = decode_hex_ascii(encoded_name) else {
                    continue;
                };
                let capability = match name.as_str() {
                    "TN" => Some(Some("xterm-256color")),
                    "Co" | "colors" => Some(Some("256")),
                    "RGB" => Some(Some("8/8/8")),
                    "Tc" | "Su" => Some(None),
                    _ => None,
                };
                output.replies.extend_from_slice(b"\x1bP");
                if let Some(value) = capability {
                    output.replies.extend_from_slice(b"1+r");
                    output.replies.extend_from_slice(encoded_name);
                    if let Some(value) = value {
                        output.replies.push(b'=');
                        push_hex_ascii(&mut output.replies, value.as_bytes());
                    }
                } else {
                    output.replies.extend_from_slice(b"0+r");
                    output.replies.extend_from_slice(encoded_name);
                }
                output.replies.extend_from_slice(b"\x1b\\");
            }
        }
    }

    pub(super) fn advance_osc(&mut self, grid: &mut Grid, byte: u8, output: &mut ParseOutput) {
        match byte {
            0x00..=0x06 | 0x08..=0x17 | 0x19 | 0x1c..=0x1f => {}
            0x07 => {
                self.finish_osc(grid, output, b"\x07");
                self.state = State::Ground;
            }
            0x1b => {
                self.finish_osc(grid, output, b"\x1b\\");
                self.state = State::Escape;
            }
            _ => self.push_osc(byte),
        }
    }

    pub(super) fn push_osc(&mut self, byte: u8) {
        if self.osc.len() < MAX_OSC_BYTES {
            self.osc.push(byte);
        } else {
            self.osc_oversized = true;
        }
    }

    pub(super) fn finish_osc(
        &mut self,
        grid: &mut Grid,
        output: &mut ParseOutput,
        terminator: &[u8],
    ) {
        if std::mem::take(&mut self.osc_oversized) {
            self.osc.clear();
            return;
        }

        let mut raw_fields = self.osc.split(|byte| *byte == b';').take(MAX_OSC_PARAMS);
        let Some(raw_command) = raw_fields.next() else {
            return;
        };
        match raw_command {
            b"0" | b"2" => {
                let Some(first_value) = raw_fields.next() else {
                    return;
                };
                let mut title = String::new();
                let mut has_value = false;
                for value in std::iter::once(first_value).chain(raw_fields) {
                    let Ok(value) = std::str::from_utf8(value) else {
                        continue;
                    };
                    if has_value {
                        title.push(';');
                    }
                    title.push_str(value);
                    has_value = true;
                }
                let title = title.trim().to_owned();
                self.title = Some(Arc::from(title.as_str()));
                output.events.push(ParsedEvent::Title(title));
                return;
            }
            b"8" => {
                let parameters = raw_fields.next().unwrap_or_default();
                let Some(first_target) = raw_fields.next() else {
                    return;
                };
                let protocol_id = parameters
                    .split(|byte| *byte == b':')
                    .find_map(|parameter| parameter.strip_prefix(b"id="))
                    .and_then(|value| std::str::from_utf8(value).ok());
                let mut target = std::str::from_utf8(first_target)
                    .unwrap_or_default()
                    .to_owned();
                for value in raw_fields {
                    target.push(';');
                    target.push_str(std::str::from_utf8(value).unwrap_or_default());
                }
                grid.set_hyperlink(protocol_id, (!target.is_empty()).then_some(target.as_str()));
                return;
            }
            _ => {}
        }

        // Termy's native OSC interceptor only recognizes its custom events
        // when the complete payload is valid UTF-8. Invalid payloads are
        // forwarded to vte, where these commands are unhandled.
        let custom_osc = matches!(raw_command, b"7" | b"9" | b"133");
        if custom_osc && std::str::from_utf8(&self.osc).is_err() {
            return;
        }

        let payload = String::from_utf8_lossy(&self.osc);
        let field_limit = if custom_osc {
            usize::MAX
        } else {
            MAX_OSC_PARAMS
        };
        let mut fields = payload.split(';').take(field_limit);
        let Some(command) = fields.next() else {
            return;
        };

        match command {
            "7" => {
                let Some(first_value) = fields.next() else {
                    return;
                };
                let value = std::iter::once(first_value)
                    .chain(fields)
                    .collect::<Vec<_>>()
                    .join(";");
                output
                    .events
                    .push(ParsedEvent::WorkingDirectory(osc7_path(&value)));
            }
            "4" => {
                let values = fields.collect::<Vec<_>>();
                if values.len() % 2 == 0 {
                    for pair in values.as_chunks::<2>().0 {
                        let Some(index) = pair[0].parse::<u8>().ok() else {
                            continue;
                        };
                        if pair[1] == "?" {
                            let color = grid
                                .palette()
                                .indexed(index)
                                .unwrap_or_else(|| self.query_colors.indexed(index));
                            push_color_reply(output, &format!("4;{index}"), color, terminator);
                        } else if let Some(color) = parse_x_color(pair[1]) {
                            grid.set_indexed_color(index, color);
                        }
                    }
                }
            }
            "10" | "11" | "12" => {
                let Some(base) = command.parse::<u8>().ok() else {
                    return;
                };
                for (offset, value) in fields.enumerate() {
                    let code = base.saturating_add(offset as u8);
                    if code > 12 {
                        break;
                    }
                    if value == "?" {
                        let palette = grid.palette();
                        let color = match code {
                            10 => {
                                Some(palette.foreground().unwrap_or(self.query_colors.foreground))
                            }
                            11 => {
                                Some(palette.background().unwrap_or(self.query_colors.background))
                            }
                            12 => palette.cursor(),
                            _ => None,
                        };
                        if let Some(color) = color {
                            push_color_reply(output, &code.to_string(), color, terminator);
                        }
                    } else if let Some(color) = parse_x_color(value) {
                        match code {
                            10 => grid.set_foreground_color(Some(color)),
                            11 => grid.set_background_color(Some(color)),
                            12 => grid.set_cursor_color(Some(color)),
                            _ => {}
                        }
                    }
                }
            }
            "9" => {
                let subtype = fields.next();
                match subtype {
                    Some("4") => {
                        let Some(state) = fields.next().and_then(|value| value.parse().ok()) else {
                            return;
                        };
                        let progress = fields
                            .next()
                            .and_then(|value| value.parse().ok())
                            .unwrap_or(0);
                        output
                            .events
                            .push(ParsedEvent::Progress(progress_state(state, progress)));
                    }
                    Some("9") => {
                        let path = fields.collect::<Vec<_>>().join(";");
                        let path = path.trim().trim_matches('"');
                        if !path.is_empty() {
                            output
                                .events
                                .push(ParsedEvent::WorkingDirectory(path.to_string()));
                        }
                    }
                    _ => {}
                }
            }
            "52" => {
                let Some(selector_field) = fields.next() else {
                    return;
                };
                let selector = selector_field.as_bytes().first().unwrap_or(&b'c');
                let target = match selector {
                    b'c' => ClipboardTarget::Clipboard,
                    b'p' | b's' => ClipboardTarget::Selection,
                    _ => return,
                };
                if let Some(encoded) = fields.next() {
                    if encoded == "?" && self.osc52.allows_paste() {
                        output
                            .events
                            .push(ParsedEvent::ClipboardLoad(ClipboardRequest {
                                target,
                                selector: *selector,
                                bell_terminated: terminator == b"\x07",
                            }));
                    } else if encoded != "?"
                        && self.osc52.allows_copy()
                        && let Some(bytes) = decode_base64(encoded.as_bytes())
                        && let Ok(text) = String::from_utf8(bytes)
                    {
                        output.events.push(ParsedEvent::ClipboardStore(text));
                    }
                }
            }
            "50" => {
                let Some(shape) = fields
                    .next()
                    .and_then(|value| value.strip_prefix("CursorShape="))
                    .and_then(|value| value.as_bytes().first())
                    .and_then(|value| match value {
                        b'0'..=b'2' => Some(u16::from(value - b'0')),
                        _ => None,
                    })
                else {
                    return;
                };
                grid.set_cursor_shape(shape);
            }
            "133" => match fields.next().and_then(|value| value.as_bytes().first()) {
                Some(b'A') => output.events.push(ParsedEvent::ShellPromptStart),
                Some(b'B') => output.events.push(ParsedEvent::ShellCommandStart),
                Some(b'C') => output.events.push(ParsedEvent::ShellCommandExecuting),
                Some(b'D') => {
                    let exit_code = fields.collect::<Vec<_>>().join(";").parse().ok();
                    output
                        .events
                        .push(ParsedEvent::ShellCommandFinished(exit_code));
                }
                _ => {}
            },
            "104" => {
                let values = fields.collect::<Vec<_>>();
                if values.is_empty() || values.first().is_some_and(|value| value.is_empty()) {
                    grid.reset_indexed_colors();
                } else {
                    for value in values {
                        if let Ok(index) = value.parse::<u8>() {
                            grid.reset_indexed_color(index);
                        }
                    }
                }
            }
            "110" => grid.set_foreground_color(None),
            "111" => grid.set_background_color(None),
            "112" => grid.set_cursor_color(None),
            _ => {}
        }
    }
}
