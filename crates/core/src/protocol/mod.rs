// Centralize PTY reply handling so protocol-specific branches stay out of runtime.rs.
mod kitty_clipboard;
mod kitty_clipboard_control;
mod query_colors;
mod replies;

pub use kitty_clipboard::{
    KittyClipboardHostState, KittyClipboardOsc, KittyClipboardOscTerminator,
    TerminalClipboardContent, TerminalClipboardLocation, TerminalClipboardReadRequest,
    TerminalClipboardReadResult, TerminalClipboardWriteRequest, TerminalClipboardWriteResult,
};
pub(crate) use kitty_clipboard::{KittyClipboardPending, KittyClipboardStep};
pub use kitty_clipboard_control::{
    KittyClipboardControl, KittyClipboardInput, KittyClipboardInterceptor,
};
pub use query_colors::TerminalQueryColors;
pub use replies::{
    TerminalClipboardReply, TerminalClipboardRequest, TerminalClipboardTarget, TerminalReplyHost,
};
