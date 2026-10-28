//! Clipboard requests a reply host answers later
//! ([`TerminalReplyHost::defer_clipboard_request`]). The program reads every
//! reply in the order of its requests, whenever the answers arrive: protocol
//! replies to output after a request whose answer is still owed (a `CSI c`
//! after an `OSC 52 ?`) queue behind it, as do requests answered at once.

use super::{BASE64, CustomBackend};
use crate::protocol::{KittyClipboardPending, TerminalClipboardReply, TerminalClipboardRequest};
use crate::{TerminalClipboardReadResult, TerminalClipboardWriteResult, TerminalReplyHost};
use base64::Engine as _;
use std::collections::VecDeque;

/// Requests a program can have waiting for the host. Beyond this, requests
/// are answered as busy, so a program flooding requests at a host that does
/// not answer cannot grow the queue.
const MAX_WAITING: usize = 32;
/// Everything queued behind waiting requests. Beyond this, a flood of
/// output while a request waits loses its replies instead of growing the
/// queue.
const MAX_QUEUED: usize = 1024;

/// What a deferred request's reply needs once the host answers.
pub(super) enum Continuation {
    /// An OSC 52 query; the reply repeats its selection parameter.
    Load {
        selection: String,
    },
    Kitty(KittyClipboardPending),
}

impl Continuation {
    /// The answer of a host that refused or never answered. An OSC 52 query
    /// gets an empty clipboard, as the desktop's own refusal sends, so the
    /// program stops waiting.
    fn denial(&self) -> TerminalClipboardReply {
        match self {
            Self::Load { .. } => TerminalClipboardReply::Load(Some(String::new())),
            Self::Kitty(pending) => pending.denial(),
        }
    }
}

/// One write to the program, in the order of the output that caused it.
pub(super) enum Slot {
    /// Waiting for the host's answer to request `id`.
    Waiting(u64, Continuation),
    Answered(Continuation, TerminalClipboardReply),
    /// Replies already known, or nothing for a request a reset cancelled.
    Ready(Vec<Vec<u8>>),
}

#[derive(Default)]
pub(super) struct DeferredClipboard {
    next_id: u64,
    waiting: usize,
    slots: VecDeque<Slot>,
}

impl DeferredClipboard {
    /// Queues a request for the host's answer, with the id to answer it by;
    /// hands the continuation back while too many requests are waiting.
    fn wait(&mut self, continuation: Continuation) -> Result<u64, Continuation> {
        if self.waiting >= MAX_WAITING {
            return Err(continuation);
        }
        self.next_id = self.next_id.wrapping_add(1);
        self.waiting += 1;
        self.slots
            .push_back(Slot::Waiting(self.next_id, continuation));
        Ok(self.next_id)
    }

    /// Records the answer to `id` (`None` denies it). Unknown, answered and
    /// cancelled ids are ignored.
    fn answer(&mut self, id: u64, reply: Option<TerminalClipboardReply>) {
        let Some(slot) = self
            .slots
            .iter_mut()
            .find(|slot| matches!(slot, Slot::Waiting(waiting, _) if *waiting == id))
        else {
            return;
        };
        let Slot::Waiting(_, continuation) = std::mem::replace(slot, Slot::Ready(Vec::new()))
        else {
            unreachable!("slot matched as waiting");
        };
        let reply = reply.unwrap_or_else(|| continuation.denial());
        *slot = Slot::Answered(continuation, reply);
        self.waiting -= 1;
    }

    /// Drops every waiting request without a reply.
    fn cancel(&mut self) {
        for slot in &mut self.slots {
            if matches!(slot, Slot::Waiting(..)) {
                *slot = Slot::Ready(Vec::new());
            }
        }
        self.waiting = 0;
    }

    /// The writes now due: everything before the first waiting request.
    fn take_due(&mut self) -> Vec<Slot> {
        let ready = self
            .slots
            .iter()
            .position(|slot| matches!(slot, Slot::Waiting(..)))
            .unwrap_or(self.slots.len());
        self.slots.drain(..ready).collect()
    }
}

fn busy(request: &TerminalClipboardRequest) -> TerminalClipboardReply {
    match request {
        TerminalClipboardRequest::Load(_) => TerminalClipboardReply::Load(Some(String::new())),
        TerminalClipboardRequest::Read(_) => {
            TerminalClipboardReply::Read(TerminalClipboardReadResult::Busy)
        }
        TerminalClipboardRequest::Write(_) => {
            TerminalClipboardReply::Write(TerminalClipboardWriteResult::Busy)
        }
    }
}

impl CustomBackend {
    /// Asks `host` for the answer to `request`, now or later, and replies
    /// once every earlier write is out.
    pub(super) fn ask_clipboard_host(
        &self,
        host: &mut impl TerminalReplyHost,
        request: TerminalClipboardRequest,
        continuation: Continuation,
    ) {
        // The request is queued before the host sees its id, so an answer
        // from another thread, even before `defer_clipboard_request`
        // returns, finds it.
        let waiting = self.deferred_clipboard().wait(continuation);
        match waiting {
            Err(continuation) => {
                self.write_ordered(Slot::Answered(continuation, busy(&request)));
            }
            Ok(id) => {
                // Without the queue's lock: the host may block on a prompt.
                if let Err(request) = host.defer_clipboard_request(id, request) {
                    let reply = request.execute(host);
                    self.complete_clipboard_request(id, Some(reply));
                }
            }
        }
    }

    /// Writes `slot` now when nothing waits ahead of it, else queues it.
    pub(super) fn write_ordered(&self, slot: Slot) {
        let mut queue = self.deferred_clipboard();
        if queue.slots.is_empty() {
            self.write_slots(vec![slot]);
        } else if queue.slots.len() >= MAX_QUEUED {
            log::warn!("terminal protocol replies dropped behind a clipboard request");
            self.release_ordered(1);
        } else {
            queue.slots.push_back(slot);
        }
    }

    pub(in crate::runtime) fn complete_clipboard_request(
        &self,
        id: u64,
        reply: Option<TerminalClipboardReply>,
    ) {
        let mut queue = self.deferred_clipboard();
        queue.answer(id, reply);
        let due = queue.take_due();
        // Written under the queue's lock, so a concurrent answer cannot
        // write a later reply first.
        self.write_slots(due);
    }

    /// Drops the waiting requests, after a reset, and writes what queued
    /// behind them.
    pub(super) fn cancel_clipboard_requests(&self) {
        let mut queue = self.deferred_clipboard();
        queue.cancel();
        let due = queue.take_due();
        self.write_slots(due);
    }

    fn deferred_clipboard(&self) -> std::sync::MutexGuard<'_, DeferredClipboard> {
        self.shared
            .deferred_clipboard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Writes `slots` in order. Each one stands for an ordered event that
    /// `drain_events` took; once written, later replies need not wait.
    fn write_slots(&self, slots: Vec<Slot>) {
        let count = slots.len();
        for slot in slots {
            match slot {
                Slot::Answered(continuation, reply) => {
                    self.reply_to_clipboard_request(continuation, reply);
                }
                Slot::Ready(replies) => {
                    for reply in replies {
                        self.send_reply(reply);
                    }
                }
                Slot::Waiting(..) => unreachable!("a waiting request is not due"),
            }
        }
        self.release_ordered(count);
    }

    fn release_ordered(&self, count: usize) {
        if count != 0 {
            let mut state = self.shared.state();
            state.ordered_in_flight = state.ordered_in_flight.saturating_sub(count);
        }
    }

    fn reply_to_clipboard_request(
        &self,
        continuation: Continuation,
        reply: TerminalClipboardReply,
    ) {
        match continuation {
            Continuation::Load { selection } => {
                if let TerminalClipboardReply::Load(Some(text)) = reply {
                    self.send_reply(
                        format!("\x1b]52;{selection};{}\x1b\\", BASE64.encode(text)).into_bytes(),
                    );
                }
            }
            Continuation::Kitty(pending) => {
                let replies = self
                    .shared
                    .clipboard
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .finish(pending, reply);
                for reply in replies {
                    self.send_reply(reply);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        Terminal, TerminalClipboardReply, TerminalClipboardRequest, TerminalClipboardTarget,
        TerminalReplyHost, TerminalSize,
    };
    use std::cell::RefCell;

    /// Defers every request, recording its id, and collects the replies a
    /// display terminal hands back.
    #[derive(Default)]
    struct DeferringHost {
        ids: Vec<u64>,
        replies: Vec<u8>,
    }

    impl TerminalReplyHost for DeferringHost {
        fn load_clipboard(&mut self, _: TerminalClipboardTarget) -> Option<String> {
            panic!("requests must be deferred")
        }
        fn protocol_reply(&mut self, bytes: &[u8]) {
            self.replies.extend_from_slice(bytes);
        }
        fn defer_clipboard_request(
            &mut self,
            id: u64,
            _: TerminalClipboardRequest,
        ) -> Result<(), TerminalClipboardRequest> {
            self.ids.push(id);
            Ok(())
        }
    }

    fn terminal() -> Terminal {
        Terminal::new_display(
            TerminalSize {
                cols: 20,
                rows: 4,
                cell_width: 9.0,
                cell_height: 18.0,
            },
            None,
        )
    }

    fn load(text: &str) -> TerminalClipboardReply {
        TerminalClipboardReply::Load(Some(text.into()))
    }

    fn drain(terminal: &Terminal, host: &mut DeferringHost) -> String {
        terminal.drain_events(host);
        String::from_utf8(std::mem::take(&mut host.replies)).unwrap()
    }

    /// Audit B7: `OSC 52 ?` then `CSI c` must get its replies in that order,
    /// although the clipboard answer comes later.
    #[test]
    fn replies_to_later_output_wait_for_a_deferred_clipboard_reply() {
        let terminal = terminal();
        let mut host = DeferringHost::default();
        terminal.feed_output(b"\x1b[c\x1b]52;c;?\x07\x1b[c\x1b]52;c;?\x07\x1b[5n");
        // The first device attributes reply answers output before the query.
        assert_eq!(drain(&terminal, &mut host), "\x1b[?62;22c");
        let [first, second] = host.ids[..] else {
            panic!("expected two deferred queries: {:?}", host.ids)
        };
        terminal.complete_clipboard_request(second, load("two"));
        assert_eq!(drain(&terminal, &mut host), "");
        terminal.complete_clipboard_request(first, load("one"));
        assert_eq!(
            drain(&terminal, &mut host),
            "\x1b]52;c;b25l\x1b\\\x1b[?62;22c\x1b]52;c;dHdv\x1b\\\x1b[0n"
        );
        // With nothing owed, replies go out at once again.
        terminal.feed_output(b"\x1b[c");
        assert_eq!(drain(&terminal, &mut host), "\x1b[?62;22c");
    }

    /// An embedder may answer from another thread before
    /// `defer_clipboard_request` even returns.
    #[test]
    fn an_answer_given_while_deferring_is_not_lost() {
        struct EagerHost<'a> {
            terminal: &'a Terminal,
            replies: RefCell<Vec<u8>>,
        }
        impl TerminalReplyHost for EagerHost<'_> {
            fn load_clipboard(&mut self, _: TerminalClipboardTarget) -> Option<String> {
                panic!("requests must be deferred")
            }
            fn protocol_reply(&mut self, bytes: &[u8]) {
                self.replies.borrow_mut().extend_from_slice(bytes);
            }
            fn defer_clipboard_request(
                &mut self,
                id: u64,
                _: TerminalClipboardRequest,
            ) -> Result<(), TerminalClipboardRequest> {
                self.terminal
                    .complete_clipboard_request(id, TerminalClipboardReply::Load(Some("x".into())));
                Ok(())
            }
        }
        let terminal = terminal();
        let mut host = EagerHost {
            terminal: &terminal,
            replies: RefCell::default(),
        };
        terminal.feed_output(b"\x1b]52;c;?\x07\x1b[5n");
        terminal.drain_events(&mut host);
        terminal.drain_events(&mut host);
        assert_eq!(
            String::from_utf8(host.replies.take()).unwrap(),
            "\x1b]52;c;eA==\x1b\\\x1b[0n"
        );
    }

    /// Audit B7: denied queries (timeout, no view, detach) get an empty
    /// clipboard so the program stops waiting.
    #[test]
    fn a_denied_query_gets_an_empty_clipboard() {
        let terminal = terminal();
        let mut host = DeferringHost::default();
        terminal.feed_output(b"\x1b]52;c;?\x07");
        drain(&terminal, &mut host);
        terminal.deny_clipboard_request(host.ids[0]);
        assert_eq!(drain(&terminal, &mut host), "\x1b]52;c;\x1b\\");
    }

    /// A reset (RIS) may start another program: an answer that arrives
    /// afterwards must not write the clipboard to it.
    #[test]
    fn a_reset_drops_the_waiting_requests() {
        let terminal = terminal();
        let mut host = DeferringHost::default();
        terminal.feed_output(b"\x1b]52;c;?\x07\x1bc\x1b[5n");
        assert_eq!(drain(&terminal, &mut host), "\x1b[0n");
        terminal.complete_clipboard_request(host.ids[0], load("secret"));
        assert_eq!(drain(&terminal, &mut host), "");
    }

    /// Beyond the waiting limit, requests are answered as busy, still in
    /// order behind the waiting ones.
    #[test]
    fn a_full_queue_answers_busy_in_order() {
        let terminal = terminal();
        let mut host = DeferringHost::default();
        let queries = b"\x1b]52;c;?\x07".repeat(super::MAX_WAITING + 1);
        terminal.feed_output(&queries);
        assert_eq!(drain(&terminal, &mut host), "");
        assert_eq!(host.ids.len(), super::MAX_WAITING);
        for id in host.ids.clone() {
            terminal.complete_clipboard_request(id, load("x"));
        }
        let mut expected = "\x1b]52;c;eA==\x1b\\".repeat(super::MAX_WAITING);
        expected.push_str("\x1b]52;c;\x1b\\");
        assert_eq!(drain(&terminal, &mut host), expected);
    }
}
