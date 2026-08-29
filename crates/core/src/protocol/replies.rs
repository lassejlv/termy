use alacritty_terminal::{
    event::{Event as AlacEvent, WindowSize},
    term::{ClipboardType, color::Colors},
};

use crate::{
    protocol::{
        TerminalClipboardReadRequest, TerminalClipboardReadResult, TerminalClipboardWriteRequest,
        TerminalClipboardWriteResult, TerminalQueryColors,
    },
    runtime::TerminalSize,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalClipboardTarget {
    Clipboard,
    Selection,
}

impl TerminalClipboardTarget {
    fn from_alacritty(clipboard: ClipboardType) -> Self {
        match clipboard {
            ClipboardType::Clipboard => Self::Clipboard,
            ClipboardType::Selection => Self::Selection,
        }
    }
}

pub trait TerminalReplyHost {
    fn load_clipboard(&mut self, target: TerminalClipboardTarget) -> Option<String>;

    /// Receive protocol replies when the terminal has no owned PTY transport.
    fn protocol_reply(&mut self, _bytes: &[u8]) {}

    fn read_clipboard(
        &mut self,
        _request: TerminalClipboardReadRequest,
    ) -> TerminalClipboardReadResult {
        TerminalClipboardReadResult::Denied
    }

    fn write_clipboard(
        &mut self,
        _request: TerminalClipboardWriteRequest,
    ) -> TerminalClipboardWriteResult {
        TerminalClipboardWriteResult::Unsupported
    }
}

impl<F> TerminalReplyHost for F
where
    F: FnMut(TerminalClipboardTarget) -> Option<String>,
{
    fn load_clipboard(&mut self, target: TerminalClipboardTarget) -> Option<String> {
        self(target)
    }
}

pub(crate) fn reply_bytes_for_event(
    event: &AlacEvent,
    size: TerminalSize,
    live_colors: &Colors,
    query_colors: TerminalQueryColors,
    host: &mut impl TerminalReplyHost,
) -> Option<Vec<u8>> {
    match event {
        AlacEvent::PtyWrite(text) => Some(text.as_bytes().to_vec()),
        AlacEvent::ColorRequest(index, formatter) => query_colors
            .resolve_color(live_colors, *index)
            .map(|color| formatter(crate::backend::ansi_rgb(color)).into_bytes()),
        AlacEvent::TextAreaSizeRequest(formatter) => {
            Some(formatter(WindowSize::from(size)).into_bytes())
        }
        AlacEvent::ClipboardLoad(clipboard, formatter) => host
            .load_clipboard(TerminalClipboardTarget::from_alacritty(*clipboard))
            .map(|text| formatter(&text).into_bytes()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{TerminalClipboardTarget, reply_bytes_for_event};
    use crate::{protocol::TerminalQueryColors, runtime::TerminalSize};
    use alacritty_terminal::{
        event::Event as AlacEvent,
        term::{ClipboardType, color::Colors},
        vte::ansi::{NamedColor, Rgb as AnsiRgb},
    };
    use std::sync::Arc;

    fn test_terminal_size() -> TerminalSize {
        TerminalSize {
            cols: 32,
            rows: 4,
            cell_width: 9.0,
            cell_height: 18.0,
        }
    }

    #[test]
    fn replays_pty_write_events() {
        let response = reply_bytes_for_event(
            &AlacEvent::PtyWrite("\x1b[?6c".to_string()),
            test_terminal_size(),
            &Colors::default(),
            TerminalQueryColors::default(),
            &mut |_| None,
        );

        assert_eq!(response, Some(b"\x1b[?6c".to_vec()));
    }

    #[test]
    fn formats_text_area_size_queries() {
        let response = reply_bytes_for_event(
            &AlacEvent::TextAreaSizeRequest(Arc::new(|window_size| {
                format!("\x1b[4;{};{}t", window_size.num_lines, window_size.num_cols)
            })),
            test_terminal_size(),
            &Colors::default(),
            TerminalQueryColors::default(),
            &mut |_| None,
        );

        assert_eq!(response, Some(b"\x1b[4;4;32t".to_vec()));
    }

    #[test]
    fn formats_color_queries_from_live_or_fallback_colors() {
        let mut live = Colors::default();
        live[NamedColor::Foreground as usize] = Some(AnsiRgb {
            r: 0x12,
            g: 0x34,
            b: 0x56,
        });

        let response = reply_bytes_for_event(
            &AlacEvent::ColorRequest(
                NamedColor::Foreground as usize,
                Arc::new(|color| {
                    format!(
                        "\x1b]10;rgb:{:02x}/{:02x}/{:02x}\x1b\\",
                        color.r, color.g, color.b
                    )
                }),
            ),
            test_terminal_size(),
            &live,
            TerminalQueryColors::default(),
            &mut |_| None,
        );

        assert_eq!(response, Some(b"\x1b]10;rgb:12/34/56\x1b\\".to_vec()));

        let fallback_response = reply_bytes_for_event(
            &AlacEvent::ColorRequest(
                8,
                Arc::new(|color| {
                    format!(
                        "\x1b]4;8;rgb:{:02x}/{:02x}/{:02x}\x1b\\",
                        color.r, color.g, color.b
                    )
                }),
            ),
            test_terminal_size(),
            &Colors::default(),
            TerminalQueryColors::default(),
            &mut |_| None,
        );

        assert_eq!(
            fallback_response,
            Some(b"\x1b]4;8;rgb:7f/7f/7f\x1b\\".to_vec())
        );
    }

    #[test]
    fn formats_clipboard_load_queries() {
        let mut requested_target = None;
        let response = reply_bytes_for_event(
            &AlacEvent::ClipboardLoad(
                ClipboardType::Selection,
                Arc::new(|text| format!("\x1b]52;s;{text}\x1b\\")),
            ),
            test_terminal_size(),
            &Colors::default(),
            TerminalQueryColors::default(),
            &mut |target| {
                requested_target = Some(target);
                Some("payload".to_string())
            },
        );

        assert_eq!(requested_target, Some(TerminalClipboardTarget::Selection));
        assert_eq!(response, Some(b"\x1b]52;s;payload\x1b\\".to_vec()));
    }

    #[test]
    fn ignores_clipboard_load_without_host_data() {
        let response = reply_bytes_for_event(
            &AlacEvent::ClipboardLoad(
                ClipboardType::Clipboard,
                Arc::new(|text| format!("\x1b]52;c;{text}\x1b\\")),
            ),
            test_terminal_size(),
            &Colors::default(),
            TerminalQueryColors::default(),
            &mut |_| None,
        );

        assert_eq!(response, None);
    }
}
