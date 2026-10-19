use std::sync::{RwLock, RwLockReadGuard};
use termy_core::{
    DetectedLink, DetectedViewportLink, KittyGraphicsRenderPlacement, Terminal,
    TerminalClipboardLocation, TerminalClipboardReadRequest, TerminalClipboardReadResult,
    TerminalClipboardTarget, TerminalClipboardWriteRequest, TerminalClipboardWriteResult,
    TerminalCursorState, TerminalKeyboardMode, TerminalMouseMode, TerminalOptions, TerminalPalette,
    TerminalQueryColors, TerminalRenderRead, TerminalReplyHost, TerminalRuntimeConfig,
    TerminalSize,
};

/// Display terminal for a tmux pane. Core owns parsing, screen state and protocols
/// just as it does for a native pane; tmux supplies the transport.
pub struct PaneTerminal {
    terminal: RwLock<Terminal>,
}

impl PaneTerminal {
    pub fn new(size: TerminalSize, options: TerminalOptions) -> Self {
        let config = TerminalRuntimeConfig {
            scrollback_history: options.scrollback_history,
            default_cursor_style: options.default_cursor_style,
            ..TerminalRuntimeConfig::default()
        };
        Self {
            terminal: RwLock::new(Terminal::new_display(size, Some(&config))),
        }
    }

    pub fn read(&self) -> RwLockReadGuard<'_, Terminal> {
        self.terminal
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn engine_label(&self) -> &'static str {
        self.read().engine_label()
    }
    pub fn feed_output(&self, bytes: &[u8]) {
        self.read().feed_output(bytes);
    }
    pub fn resize(&self, size: TerminalSize) {
        self.terminal
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .resize(size);
    }
    pub fn size(&self) -> TerminalSize {
        self.read().size()
    }
    pub fn hyperlink_at(&self, row: usize, col: usize) -> Option<DetectedLink> {
        self.read().hyperlink_at(row, col)
    }
    pub fn link_at(&self, row: usize, col: usize) -> Option<DetectedViewportLink> {
        self.read().link_at(row, col)
    }
    pub fn render_read(&self, force_full: bool) -> TerminalRenderRead {
        self.read().render_read(force_full)
    }
    pub fn scroll_display(&self, delta: i32) -> bool {
        self.read().scroll_display(delta)
    }
    pub fn scroll_to_bottom(&self) -> bool {
        self.read().scroll_to_bottom()
    }
    pub fn scroll_state(&self) -> (usize, usize) {
        self.read().scroll_state()
    }
    pub fn cursor_state(&self) -> Option<TerminalCursorState> {
        self.read().cursor_state()
    }
    pub fn cursor_position(&self) -> (usize, usize) {
        self.read().cursor_position()
    }
    pub fn set_term_options(&self, options: TerminalOptions) {
        self.read().set_term_options(options);
    }
    pub fn set_query_colors(&self, colors: TerminalQueryColors) {
        self.terminal
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_query_colors(colors);
    }
    pub fn palette(&self) -> TerminalPalette {
        self.read().palette()
    }
    pub fn bracketed_paste_mode(&self) -> bool {
        self.read().bracketed_paste_mode()
    }
    pub fn mouse_mode(&self) -> TerminalMouseMode {
        self.read().mouse_mode()
    }
    pub fn keyboard_mode(&self) -> TerminalKeyboardMode {
        self.read().keyboard_mode()
    }
    pub fn alternate_screen_mode(&self) -> bool {
        self.read().alternate_screen_mode()
    }
    pub fn kitty_graphics_placements(&self) -> Vec<KittyGraphicsRenderPlacement> {
        self.read().kitty_graphics_placements()
    }
    pub fn kitty_graphics_revision(&self) -> u64 {
        self.read().kitty_graphics_revision()
    }
    pub fn kitty_graphics_snapshot(&self) -> (u64, Vec<KittyGraphicsRenderPlacement>) {
        self.read().kitty_graphics_snapshot()
    }
    pub fn kitty_clipboard_paste_events_enabled(&self) -> bool {
        self.read().kitty_clipboard_paste_events_enabled()
    }

    pub fn kitty_clipboard_paste_notification(
        &self,
        location: TerminalClipboardLocation,
        available_formats: &[String],
    ) -> Option<Vec<u8>> {
        self.read()
            .kitty_clipboard_paste_notification(location, available_formats)
    }

    pub fn drain_kitty_clipboard_events(&self, host: &mut impl TerminalReplyHost) -> Vec<Vec<u8>> {
        let mut collector = PaneReplyHost {
            host,
            replies: Vec::new(),
        };
        // Tmux drains only after an output notification. Finish the finite queue
        // while holding the write guard so a later batch cannot be stranded
        // waiting for output that may never arrive.
        let terminal = self
            .terminal
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while terminal.drain_events(&mut collector).1 {}
        collector.replies
    }
}

struct PaneReplyHost<'a, H> {
    host: &'a mut H,
    replies: Vec<Vec<u8>>,
}

impl<H: TerminalReplyHost> TerminalReplyHost for PaneReplyHost<'_, H> {
    fn load_clipboard(&mut self, target: TerminalClipboardTarget) -> Option<String> {
        self.host.load_clipboard(target)
    }
    fn read_clipboard(
        &mut self,
        request: TerminalClipboardReadRequest,
    ) -> TerminalClipboardReadResult {
        self.host.read_clipboard(request)
    }
    fn write_clipboard(
        &mut self,
        request: TerminalClipboardWriteRequest,
    ) -> TerminalClipboardWriteResult {
        self.host.write_clipboard(request)
    }
    fn protocol_reply(&mut self, bytes: &[u8]) {
        self.replies.push(bytes.to_vec());
    }
}

#[cfg(test)]
mod tests {
    use super::PaneTerminal;
    use termy_core::{TerminalClipboardLocation, TerminalOptions, TerminalSize};

    fn test_term_options(scrollback_history: usize) -> TerminalOptions {
        TerminalOptions {
            scrollback_history,
            ..TerminalOptions::default()
        }
    }

    fn visible_viewport_text(terminal: &PaneTerminal) -> String {
        let size = terminal.size();
        let cols = size.cols as usize;
        let rows = size.rows as usize;
        let mut grid = vec![vec![' '; cols]; rows];

        terminal
            .read()
            .visit_viewport_cells(|offset, line, col, cell| {
                let row = line + offset as i32;
                if let Ok(row) = usize::try_from(row) {
                    let c = cell.text.chars().next().unwrap_or(' ');
                    if row < rows && col < cols && c != '\0' && !c.is_control() {
                        grid[row][col] = c;
                    }
                }
            });

        grid.into_iter()
            .map(|row| {
                let line: String = row.into_iter().collect();
                line.trim_end().to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn feed_output_handles_tmux_prompt_repaint_without_prefix_duplication() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 120,
                rows: 10,
                ..TerminalSize::default()
            },
            test_term_options(2000),
        );

        terminal.feed_output(
            b"c\x08cd Desk\x08\x08\x08\x08\x08\x08\x08\x1b[32mc\x1b[32md\x1b[39m\x1b[5C\r\r\n",
        );
        terminal.feed_output(b"cd: no such file or directory: Desk\r\n");
        terminal.feed_output(b"c\x08\x1b[4mc\x1b[24m\r\r\n");
        terminal.feed_output(b"zsh: command not found: c\r\n");

        let visible = visible_viewport_text(&terminal);
        assert!(visible.contains("cd: no such file or directory: Desk"));
        assert!(visible.contains("zsh: command not found: c"));
        assert!(!visible.contains("cdcd:"));
        assert!(!visible.contains("czsh:"));
    }

    #[test]
    fn clamps_zero_size_on_new_and_resize() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 0,
                rows: 0,
                ..TerminalSize::default()
            },
            test_term_options(2000),
        );
        assert_eq!(terminal.size().cols, 1);
        assert_eq!(terminal.size().rows, 1);
        assert_eq!(terminal.size().cols.saturating_sub(1), 0);
        assert_eq!(terminal.size().rows.saturating_sub(1), 0);

        terminal.resize(TerminalSize {
            cols: 0,
            rows: 0,
            ..TerminalSize::default()
        });
        assert_eq!(terminal.size().cols, 1);
        assert_eq!(terminal.size().rows, 1);
        assert_eq!(terminal.size().cols.saturating_sub(1), 0);
        assert_eq!(terminal.size().rows.saturating_sub(1), 0);
    }

    #[test]
    fn resize_keeps_grid_and_reported_size_synchronized() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 4,
                rows: 3,
                ..TerminalSize::default()
            },
            test_term_options(2000),
        );

        terminal.resize(TerminalSize {
            cols: 9,
            rows: 7,
            ..TerminalSize::default()
        });

        let size = terminal.size();
        assert_eq!(size.cols, 9);
        assert_eq!(size.rows, 7);
    }

    #[test]
    fn mouse_mode_detects_click_and_sgr_flags_from_output_stream() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 4,
                rows: 3,
                ..TerminalSize::default()
            },
            test_term_options(2000),
        );

        terminal.feed_output(b"\x1b[?1000h\x1b[?1006h");
        let mode = terminal.mouse_mode();
        assert!(mode.enabled);
        assert!(mode.report_click);
        assert!(mode.sgr_encoding);
        assert!(!mode.report_drag);
        assert!(!mode.report_motion);
    }

    #[test]
    fn mouse_mode_detects_drag_and_motion_flags_from_output_stream() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 4,
                rows: 3,
                ..TerminalSize::default()
            },
            test_term_options(2000),
        );

        terminal.feed_output(b"\x1b[?1002h");
        let drag_mode = terminal.mouse_mode();
        assert!(drag_mode.enabled);
        assert!(drag_mode.report_drag);
        assert!(!drag_mode.report_motion);

        terminal.feed_output(b"\x1b[?1003h");
        let motion_mode = terminal.mouse_mode();
        assert!(motion_mode.enabled);
        assert!(motion_mode.report_motion);
    }

    #[test]
    fn tmux_link_lookup_is_owned_by_pane_terminal() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 12,
                rows: 3,
                ..TerminalSize::default()
            },
            test_term_options(2000),
        );
        terminal.feed_output(
            b"\x1b]8;id=docs;https://example.com/docs\x1b\\docs\x1b]8;;\x1b\\ https://x.io",
        );

        let hyperlink = terminal.hyperlink_at(0, 1).expect("OSC 8 link");
        assert_eq!((hyperlink.start_col, hyperlink.end_col), (0, 3));
        assert_eq!(hyperlink.target, "https://example.com/docs");

        let detected = terminal.link_at(1, 3).expect("wrapped text link");
        assert_eq!(detected.target, "https://x.io");
    }

    #[test]
    fn feed_output_intercepts_kitty_graphics_for_tmux_panes() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 20,
                rows: 10,
                cell_width: 10.0,
                cell_height: 20.0,
            },
            test_term_options(2000),
        );

        terminal.feed_output(b"\x1b_Ga=T,f=32,s=1,v=1,i=91,c=2,r=2;AQID/w==\x1b\\");

        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].image_id, 91);
        assert_eq!(placements[0].display_cols, Some(2));
        // As in kitty: just past the image, on its last row.
        assert_eq!(terminal.cursor_position(), (2, 1));
    }

    #[test]
    fn direct_kitty_image_does_not_churn_revision_for_unrelated_text() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 20,
                rows: 10,
                cell_width: 10.0,
                cell_height: 20.0,
            },
            test_term_options(2000),
        );
        terminal.feed_output(b"\x1b_Ga=T,f=32,s=1,v=1,i=91,c=2,r=2,C=1;AQID/w==\x1b\\");
        let revision = terminal.kitty_graphics_revision();

        terminal.feed_output(b"ordinary text");

        assert_eq!(terminal.kitty_graphics_revision(), revision);
    }

    #[test]
    fn tmux_kitty_relative_image_tracks_and_clears_with_unicode_placeholder() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 20,
                rows: 10,
                cell_width: 10.0,
                cell_height: 20.0,
            },
            test_term_options(2000),
        );
        terminal.feed_output(
            b"\x1b_Ga=t,f=32,s=1,v=1,i=41,q=1;AQID/w==\x1b\\\
              \x1b_Ga=t,f=32,s=1,v=1,i=42,q=1;AQID/w==\x1b\\\
              \x1b_Ga=p,U=1,i=41,p=7,c=1,r=1,C=1,q=1;\x1b\\\
              \x1b_Ga=p,i=42,p=8,P=41,Q=7,H=3,V=-2,c=2,r=2,C=1,q=1;\x1b\\\
              \x1b[7;5H\x1b[38;5;41;58;5;7m",
        );
        terminal.feed_output("\u{10eeee}\u{0305}\u{0305}".as_bytes());
        terminal.feed_output(b"\x1b[0m");

        let child = terminal
            .kitty_graphics_placements()
            .into_iter()
            .find(|placement| placement.image_id == 42)
            .expect("relative child should follow the placeholder");
        assert_eq!((child.viewport_row, child.col), (4, 7));

        let revision = terminal.kitty_graphics_revision();
        terminal.feed_output(b"\x1b[7;5Hx");
        assert!(terminal.kitty_graphics_placements().is_empty());
        assert!(terminal.kitty_graphics_revision() > revision);
    }

    #[test]
    fn clear_screen_removes_kitty_graphics_from_tmux_panes() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 20,
                rows: 10,
                cell_width: 10.0,
                cell_height: 20.0,
            },
            test_term_options(2000),
        );
        terminal.feed_output(b"\x1b_Ga=T,f=32,s=1,v=1,i=96,c=2,r=2,C=1;AQID/w==\x1b\\");
        assert_eq!(terminal.kitty_graphics_placements().len(), 1);

        terminal.feed_output(b"\x1b[H\x1b[2J");

        assert!(terminal.kitty_graphics_placements().is_empty());
    }

    #[test]
    fn kitty_cursor_advance_scrolls_tmux_pane_at_bottom() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 20,
                rows: 4,
                cell_width: 10.0,
                cell_height: 20.0,
            },
            test_term_options(2000),
        );

        terminal.feed_output(b"\x1b[4;1H\x1b_Ga=T,f=32,s=1,v=1,i=92,c=2,r=3;AQID/w==\x1b\\");

        // The cursor stays on the image's last row, so 3 rows scroll 2.
        assert_eq!(terminal.scroll_state(), (0, 2));
        assert_eq!(terminal.cursor_position(), (2, 3));
        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].viewport_row, 1);
    }

    #[test]
    fn kitty_cursor_advance_tracks_tmux_alternate_screen_scroll() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 20,
                rows: 4,
                cell_width: 10.0,
                cell_height: 20.0,
            },
            test_term_options(2000),
        );

        terminal
            .feed_output(b"\x1b[?1049h\x1b[4;1H\x1b_Ga=T,f=32,s=1,v=1,i=93,c=2,r=3;AQID/w==\x1b\\");

        assert!(terminal.alternate_screen_mode());
        assert_eq!(terminal.scroll_state(), (0, 0));
        assert_eq!(terminal.cursor_position(), (2, 3));
        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].viewport_row, 1);
    }

    #[test]
    fn ordinary_newlines_shift_tmux_alternate_screen_kitty_placement() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 20,
                rows: 4,
                cell_width: 10.0,
                cell_height: 20.0,
            },
            test_term_options(2000),
        );
        terminal.feed_output(
            b"\x1b[?1049h\x1b[2;1H\x1b_Ga=T,f=32,s=1,v=1,i=95,c=2,r=1,C=1;AQID/w==\x1b\\",
        );

        terminal.feed_output(b"\x1b[4;1H\n");

        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].viewport_row, 0);
    }

    #[test]
    fn kitty_cursor_advance_does_not_scroll_tmux_partial_region() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 20,
                rows: 4,
                cell_width: 10.0,
                cell_height: 20.0,
            },
            test_term_options(2000),
        );

        terminal.feed_output(
            b"\x1b[2;3r\x1b[3;1H\x1b_Ga=T,f=32,s=1,v=1,i=94,c=2,r=3;AQID/w==\x1b\\\x1b[r",
        );

        assert_eq!(terminal.scroll_state(), (0, 0));
        assert_eq!(terminal.cursor_position(), (0, 0));
        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].image_id, 94);
        assert_eq!(placements[0].viewport_row, 2);
    }

    #[test]
    fn tmux_panes_route_kitty_clipboard_packets_and_paste_mode() {
        let terminal = PaneTerminal::new(TerminalSize::default(), test_term_options(100));
        terminal.feed_output(
            b"\x1b[?5522h\x1b[?5522$p\x1b]5522;type=write:id=tmux\x1b\\\
              \x1b]5522;type=wdata\x1b\\",
        );

        assert!(terminal.kitty_clipboard_paste_events_enabled());
        let replies = terminal.drain_kitty_clipboard_events(&mut |_| None);
        assert_eq!(
            replies.concat(),
            b"\x1b[?5522;1$y\x1b]5522;type=write:status=ENOSYS:id=tmux\x1b\\"
        );

        let notification = terminal
            .kitty_clipboard_paste_notification(
                TerminalClipboardLocation::Clipboard,
                &["text/plain".to_string(), "image/png".to_string()],
            )
            .unwrap();
        assert!(String::from_utf8_lossy(&notification).contains("status=DATA:mime=Lg=="));
    }
    #[test]
    fn clipboard_drains_more_than_one_core_event_batch() {
        struct Host {
            reads: usize,
        }
        impl termy_core::TerminalReplyHost for Host {
            fn load_clipboard(&mut self, _: termy_core::TerminalClipboardTarget) -> Option<String> {
                None
            }
            fn read_clipboard(
                &mut self,
                _: termy_core::TerminalClipboardReadRequest,
            ) -> termy_core::TerminalClipboardReadResult {
                self.reads += 1;
                termy_core::TerminalClipboardReadResult::Denied
            }
        }
        let terminal = PaneTerminal::new(TerminalSize::default(), TerminalOptions::default());
        let request = b"\x1b]5522;type=read:id=batch;Lg==\x1b\\";
        terminal.feed_output(&request.repeat(4097));
        let mut host = Host { reads: 0 };
        let replies = terminal.drain_kitty_clipboard_events(&mut host).concat();
        assert_eq!(host.reads, 4097);
        assert_eq!(
            String::from_utf8(replies)
                .unwrap()
                .matches("id=batch")
                .count(),
            4097
        );
        assert!(!terminal.read().has_pending_events());
    }
    #[test]
    fn clipboard_paste_notification_retains_its_single_use_grant() {
        struct Host {
            permissions: Vec<bool>,
        }
        impl termy_core::TerminalReplyHost for Host {
            fn load_clipboard(&mut self, _: termy_core::TerminalClipboardTarget) -> Option<String> {
                None
            }
            fn read_clipboard(
                &mut self,
                request: termy_core::TerminalClipboardReadRequest,
            ) -> termy_core::TerminalClipboardReadResult {
                self.permissions.push(request.permission_granted);
                termy_core::TerminalClipboardReadResult::Denied
            }
        }
        let terminal = PaneTerminal::new(TerminalSize::default(), TerminalOptions::default());
        terminal.feed_output(b"\x1b[?5522h");
        terminal.drain_kitty_clipboard_events(&mut |_| None);
        let notification = String::from_utf8(
            terminal
                .kitty_clipboard_paste_notification(
                    TerminalClipboardLocation::Clipboard,
                    &["text/plain".to_string()],
                )
                .expect("paste events enabled"),
        )
        .unwrap();
        let encoded_password = notification
            .split("pw=")
            .nth(1)
            .unwrap()
            .split([':', '\x1b'])
            .next()
            .unwrap();
        let request = format!(
            "\x1b]5522;type=read:pw={encoded_password}:name=UGFzdGUgZXZlbnQ=;dGV4dC9wbGFpbg==\x1b\\"
        );
        terminal.feed_output(request.repeat(2).as_bytes());
        let mut host = Host {
            permissions: Vec::new(),
        };
        terminal.drain_kitty_clipboard_events(&mut host);
        assert_eq!(host.permissions, vec![true, false]);
    }
}
