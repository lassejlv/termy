# Terminal stream regression corpus

`streams.json` contains exact UTF-8/control-byte chunks for minimized reproductions derived from
the user-provided Claude Code, OpenCode, and Ghostty comparisons. They are not claimed to be
byte-for-byte recordings from those upstream applications: the original reports were screenshots,
so the smallest equivalent control streams are preserved here and labeled honestly.

The engine fixture test asserts terminal cell and cursor positions. Renderer tests additionally
assert monochrome presentation for the bare record symbol, fixed-column geometry through every
OpenCode spinner frame, and the Tokyo Night block-cursor color/opacity. Reference screenshots are
kept under `reference/` as visual provenance; a future raw-session recorder may replace the
minimized streams without changing their observable contract.
