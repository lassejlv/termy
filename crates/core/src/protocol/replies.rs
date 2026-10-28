use crate::protocol::{
    TerminalClipboardReadRequest, TerminalClipboardReadResult, TerminalClipboardWriteRequest,
    TerminalClipboardWriteResult,
};

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalClipboardTarget {
    Clipboard,
    Selection,
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

    /// Offers a clipboard request to answer later instead of now. Returning
    /// `Ok` takes it: the terminal then waits for
    /// `Terminal::complete_clipboard_request` (or `deny_clipboard_request`)
    /// with this `id`, and writes the replies of deferred requests in the
    /// order the program made them. Returning the request answers it now
    /// through the methods above, which is the default.
    fn defer_clipboard_request(
        &mut self,
        _id: u64,
        request: TerminalClipboardRequest,
    ) -> Result<(), TerminalClipboardRequest> {
        Err(request)
    }
}

/// A clipboard operation a program asked for, for a [`TerminalReplyHost`]
/// that answers it later (see [`TerminalReplyHost::defer_clipboard_request`]).
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub enum TerminalClipboardRequest {
    Load(TerminalClipboardTarget),
    Read(TerminalClipboardReadRequest),
    Write(TerminalClipboardWriteRequest),
}

/// The answer to a [`TerminalClipboardRequest`] of the same kind.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum TerminalClipboardReply {
    Load(Option<String>),
    Read(TerminalClipboardReadResult),
    Write(TerminalClipboardWriteResult),
}

impl TerminalClipboardRequest {
    /// Answers the request now through `host`.
    pub fn execute(self, host: &mut impl TerminalReplyHost) -> TerminalClipboardReply {
        match self {
            Self::Load(target) => TerminalClipboardReply::Load(host.load_clipboard(target)),
            Self::Read(request) => TerminalClipboardReply::Read(host.read_clipboard(request)),
            Self::Write(request) => TerminalClipboardReply::Write(host.write_clipboard(request)),
        }
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

// The tests drive replies through `Terminal`, which needs `native`.
#[cfg(all(test, feature = "native"))]
mod tests {
    use super::{TerminalClipboardTarget, TerminalReplyHost};
    use crate::{Terminal, TerminalSize};

    #[derive(Default)]
    struct Host {
        requested: Option<TerminalClipboardTarget>,
        clipboard: Option<String>,
        replies: Vec<u8>,
    }

    impl TerminalReplyHost for Host {
        fn load_clipboard(&mut self, target: TerminalClipboardTarget) -> Option<String> {
            self.requested = Some(target);
            self.clipboard.clone()
        }
        fn protocol_reply(&mut self, bytes: &[u8]) {
            self.replies.extend_from_slice(bytes);
        }
    }

    fn feed(input: &[u8], host: &mut Host) {
        let terminal = Terminal::new_display(
            TerminalSize {
                cols: 32,
                rows: 4,
                cell_width: 9.0,
                cell_height: 18.0,
            },
            None,
        );
        terminal.feed_output(input);
        let (_, more) = terminal.drain_events(host);
        assert!(!more);
    }

    #[test]
    fn forwards_device_attribute_replies_to_the_external_transport() {
        let mut host = Host::default();
        feed(b"\x1b[c", &mut host);
        assert_eq!(host.replies, b"\x1b[?62;22c");
    }

    #[test]
    fn formats_text_area_size_queries() {
        let mut host = Host::default();
        feed(b"\x1b[18t\x1b[14t", &mut host);
        assert_eq!(host.replies, b"\x1b[8;4;32t\x1b[4;72;288t");
    }

    #[test]
    fn formats_color_queries_from_live_or_fallback_colors() {
        let mut host = Host::default();
        feed(b"\x1b]10;#123456\x07\x1b]10;?\x07\x1b]4;8;?\x07", &mut host);
        assert_eq!(
            host.replies,
            b"\x1b]10;rgb:1212/3434/5656\x07\x1b]4;8;rgb:7f7f/7f7f/7f7f\x07"
        );
    }

    #[test]
    fn formats_clipboard_load_queries() {
        let mut host = Host {
            clipboard: Some("payload".into()),
            ..Host::default()
        };
        feed(b"\x1b]52;s;?\x1b\\", &mut host);
        assert_eq!(host.requested, Some(TerminalClipboardTarget::Selection));
        assert_eq!(host.replies, b"\x1b]52;s;cGF5bG9hZA==\x1b\\");
    }

    #[test]
    fn ignores_clipboard_load_without_host_data() {
        let mut host = Host::default();
        feed(b"\x1b]52;c;?\x1b\\", &mut host);
        assert_eq!(host.requested, Some(TerminalClipboardTarget::Clipboard));
        assert!(host.replies.is_empty());
    }
}
