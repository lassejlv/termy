# Raw PTY input fixtures

`raw-pty.json` is the byte-level input contract exercised by the app tests. Expected values are
lowercase hexadecimal so control bytes cannot be mistaken for display text. The matrix covers a
Danish Option-produced character, legacy xterm input, Kitty press/release reporting, bracketed
paste, focus transitions, and SGR mouse input.

Keyboard layout mapping remains an app responsibility; protocol negotiation and byte encoding
remain an engine responsibility. Adding support for an input path requires a fixture when its
host-visible bytes differ from an existing case.
