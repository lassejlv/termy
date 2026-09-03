# Tmon PTY

`tmon-pty` is Tmon's direct, macOS-only pseudo-terminal process layer. It replaces a generic PTY
adapter with one small safe API over the exact Darwin facilities Tmon needs:

- `forkpty` creates a session-leading child with a controlling terminal.
- Prepared `execve` arguments and environment keep the child-side post-fork path
  async-signal-safe.
- Close-on-exec `File` duplicates provide direct, non-dynamic read and write I/O.
- `TIOCSWINSZ`, process-group `SIGKILL`, and `waitpid` own resize and teardown behavior.
- `Child` terminates and reaps itself if a caller forgets to wait.

The unsafe syscall boundary is confined to `src/native.rs`. Terminal parsing, output buffering,
callbacks, and UI concerns do not live in this crate.

```rust
use std::io::{Read, Write};
use tmon_pty::{Command, PtySize, SpawnedPty};

let command = Command::new("sh")
    .args(["-c", "read name; printf 'hello %s' \"$name\""])
    .env("TERM", "xterm-256color")
    .current_dir("/tmp");
let size = PtySize {
    rows: 24,
    cols: 80,
    pixel_width: 720,
    pixel_height: 432,
};

let SpawnedPty { master, mut child } = tmon_pty::spawn(&command, size)?;
let mut reader = master.try_clone()?;
let mut writer = master.try_clone()?;
writer.write_all(b"Tmon\n")?;

let mut output = Vec::new();
reader.read_to_end(&mut output)?;
let status = child.wait()?;
# assert_eq!(status.exit_code(), 0);
# Ok::<(), std::io::Error>(())
```

Tmon normally uses `engine::pty::PtySession`, which keeps this process contract but adds one
coalescing reader thread, a bounded 512 KiB queue, callback wakeups, reusable drain storage, and
resize metrics. Embedders that already have an event loop can use `tmon-pty` directly.
