use termy_core::{Terminal, TerminalSize};

fn size(cols: u16, rows: u16) -> TerminalSize {
    TerminalSize {
        cols,
        rows,
        cell_width: 9.0,
        cell_height: 18.0,
    }
}

/// Frames only ever describe the viewport, so the point of `buffer_text` is
/// reaching rows that have already scrolled off it.
#[test]
fn buffer_text_includes_rows_scrolled_out_of_the_viewport() {
    let terminal = Terminal::new_display(size(20, 4), None);
    for line in 0..12 {
        terminal.feed_output(format!("line{line}\r\n").as_bytes());
    }

    let text = terminal.buffer_text(false);

    // Long gone from the viewport, still in the buffer.
    assert!(text.contains("line0"), "missing scrolled-off row: {text:?}");
    assert!(text.contains("line11"), "missing latest row: {text:?}");
}

#[test]
fn scrollback_only_omits_the_viewport() {
    let terminal = Terminal::new_display(size(20, 4), None);
    for line in 0..12 {
        terminal.feed_output(format!("line{line}\r\n").as_bytes());
    }

    let scrollback = terminal.buffer_text(true);

    assert!(scrollback.contains("line0"));
    // line11 is on screen, so it is not scrollback.
    assert!(
        !scrollback.contains("line11"),
        "viewport leaked into scrollback: {scrollback:?}"
    );
}

/// Empty scrollback is how a caller tells a shell sitting at its prompt from a
/// full-screen TUI, so it must not report the visible rows.
#[test]
fn scrollback_only_is_empty_before_anything_scrolls() {
    let terminal = Terminal::new_display(size(20, 8), None);
    terminal.feed_output(b"just one line\r\n");

    assert!(terminal.buffer_text(true).trim().is_empty());
    assert!(terminal.buffer_text(false).contains("just one line"));
}
