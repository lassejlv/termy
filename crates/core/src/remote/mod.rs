//! Renderer-neutral remote terminal boundary. The session host owns the engine;
//! clients cache only the viewport and send input or history queries to it.
mod backend;
pub(crate) mod serde_deadline;
pub(crate) mod serde_image;
pub(crate) mod serde_palette;

use crate::*;
pub(crate) use backend::RemoteBackend;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoteState {
    pub render: TerminalRenderRead,
    pub size: TerminalSize,
    pub child_pid: Option<u32>,
    pub cursor_position: (usize, usize),
    pub mouse_mode: TerminalMouseMode,
    pub keyboard_mode: TerminalKeyboardMode,
    pub bracketed_paste: bool,
    pub alternate_screen: bool,
    pub clipboard_paste_events: bool,
    pub graphics_revision: u64,
    #[serde(with = "serde_deadline")]
    pub graphics_deadline: Option<std::time::Instant>,
}

impl RemoteState {
    pub fn capture(terminal: &Terminal) -> Self {
        let (graphics_revision, graphics) = terminal.kitty_graphics_snapshot();
        // Capture screen identity under the same engine lock as its cells.
        // A PTY update may switch screens between independent reads.
        let (render, alternate_screen) = terminal.render_read_with_screen(true);
        Self {
            render,
            size: terminal.size(),
            child_pid: terminal.child_pid(),
            cursor_position: terminal.cursor_position(),
            mouse_mode: terminal.mouse_mode(),
            keyboard_mode: terminal.keyboard_mode(),
            bracketed_paste: terminal.bracketed_paste_mode(),
            alternate_screen,
            clipboard_paste_events: terminal.kitty_clipboard_paste_events_enabled(),
            graphics_revision,
            graphics_deadline: graphics
                .iter()
                .filter_map(|placement| placement.animation_deadline)
                .min(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RemoteCommand {
    Write(Vec<u8>),
    FeedOutput(Vec<u8>),
    HydrateOutput(Vec<u8>),
    Resize(TerminalSize),
    NudgeResize,
    Scroll(i32),
    ScrollToBottom,
    ClearScrollback,
    SetOptions(TerminalOptions),
    SetScrollback(usize),
    SetQueryColors(TerminalQueryColors),
    Snapshot,
    Lines {
        first: i32,
        last: i32,
    },
    Search {
        query: String,
        options: TermySearchOptions,
    },
    Hyperlink {
        row: usize,
        col: usize,
    },
    Link {
        row: usize,
        col: usize,
    },
    Graphics,
    ClipboardPaste {
        location: TerminalClipboardLocation,
        formats: Vec<String>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RemoteReply {
    Ok,
    Changed(bool),
    Frame(TermyFrame),
    Lines {
        bounds: (i32, i32, usize),
        cells: Vec<TerminalRenderCell>,
    },
    Search(Vec<TermySearchMatch>),
    Hyperlink(Option<DetectedLink>),
    Link(Option<DetectedViewportLink>),
    Graphics(u64, Vec<KittyGraphicsRenderPlacement>),
}

/// A clipboard operation is executed by the attached UI using its existing
/// permission policy. Detached sessions have no clipboard host.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RemoteHostRequest {
    Load(TerminalClipboardTarget),
    Read(TerminalClipboardReadRequest),
    Write(TerminalClipboardWriteRequest),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RemoteHostReply {
    Load(Option<String>),
    Read(TerminalClipboardReadResult),
    Write(TerminalClipboardWriteResult),
}

impl RemoteHostRequest {
    pub fn execute(self, host: &mut impl TerminalReplyHost) -> RemoteHostReply {
        match self {
            Self::Load(target) => RemoteHostReply::Load(host.load_clipboard(target)),
            Self::Read(request) => RemoteHostReply::Read(host.read_clipboard(request)),
            Self::Write(request) => RemoteHostReply::Write(host.write_clipboard(request)),
        }
    }
}

/// Implemented by the IPC client, keeping sockets, process discovery, and
/// session lifetime outside the terminal engine. Dropping a transport detaches;
/// destroying the remote process is an explicit session-host operation.
pub trait RemoteTransport: Send + Sync {
    fn state(&self) -> Arc<RemoteState>;
    fn request(&self, command: RemoteCommand) -> anyhow::Result<RemoteReply>;
    fn send(&self, command: RemoteCommand);
    fn take_events(&self) -> Vec<TerminalEvent>;
    fn take_host_requests(&self) -> Vec<(u64, RemoteHostRequest)>;
    fn reply_to_host(&self, id: u64, reply: RemoteHostReply);
    fn has_pending_events(&self) -> bool;
    fn set_wakeup_enabled(&self, enabled: bool);
}

/// Execute a client operation against the authoritative terminal. No output
/// transcript is needed: both screen buffers, parser state, and history stay
/// in this engine for the lifetime of its PTY.
pub fn execute(terminal: &mut Terminal, command: RemoteCommand) -> RemoteReply {
    use RemoteCommand as C;
    match command {
        C::Write(bytes) => terminal.write_owned(bytes),
        C::FeedOutput(bytes) => terminal.feed_output(&bytes),
        C::HydrateOutput(bytes) => terminal.hydrate_output(&bytes),
        C::Resize(size) => terminal.resize(size),
        C::NudgeResize => terminal.nudge_resize(),
        C::Scroll(lines) => return RemoteReply::Changed(terminal.scroll_display(lines)),
        C::ScrollToBottom => return RemoteReply::Changed(terminal.scroll_to_bottom()),
        C::ClearScrollback => return RemoteReply::Changed(terminal.clear_scrollback()),
        C::SetOptions(options) => terminal.set_term_options(options),
        C::SetScrollback(lines) => terminal.set_scrollback_history(lines),
        C::SetQueryColors(colors) => terminal.set_query_colors(colors),
        C::Snapshot => return RemoteReply::Frame(terminal.snapshot()),
        C::Lines { first, last } => {
            let mut cells = Vec::new();
            let bounds =
                terminal.visit_line_cells(first, last, |_, _, _, cell| cells.push(cell.clone()));
            return RemoteReply::Lines { bounds, cells };
        }
        C::Search { query, options } => {
            return RemoteReply::Search(terminal.search_with_options(&query, options));
        }
        C::Hyperlink { row, col } => {
            return RemoteReply::Hyperlink(terminal.hyperlink_at(row, col));
        }
        C::Link { row, col } => return RemoteReply::Link(terminal.link_at(row, col)),
        C::Graphics => {
            let (revision, placements) = terminal.kitty_graphics_snapshot();
            return RemoteReply::Graphics(revision, placements);
        }
        C::ClipboardPaste { location, formats } => {
            return RemoteReply::Changed(
                terminal.send_kitty_clipboard_paste_event(location, &formats),
            );
        }
    }
    RemoteReply::Ok
}
