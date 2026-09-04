use engine::{Terminal, TerminalConfig};

fn main() -> anyhow::Result<()> {
    let mut terminal = Terminal::new(TerminalConfig {
        columns: 8,
        rows: 2,
        scrollback_limit: 3,
    });
    terminal.feed(b"alpha\r\n\x1b[31mred\x1b[0m\x1b[?2004h");
    terminal.set_pixel_size(80, 40);
    let snapshot = terminal.snapshot()?;
    for byte in snapshot {
        print!("{byte:02x}");
    }
    println!();
    Ok(())
}
