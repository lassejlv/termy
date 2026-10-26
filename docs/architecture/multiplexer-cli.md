# Multiplexer CLI

`termy-cli mux` controls the same persistent host used by desktop.
The installed launcher exposes this as `termy mux`. Commands are noninteractive
and return one JSON object with `schema_version: 2`, `ok`, and `result`.
Only `start` and `create` start a missing host. Other commands fail if it is
absent; `status` reports whether one runs without starting it.

```sh
termy mux start
termy mux status
termy mux create --cols 100 --rows 30 --working-directory path/to/project
termy mux create --window WINDOW_ID --workspace 0
termy mux list
termy mux list --verbose
termy mux send PANE_ID "make; printf '%s_%s\n' build done" --enter --wait build_done
termy mux wait PANE_ID build_done --after ANCHOR --timeout-ms 5000
printf 'literal input' | termy mux send PANE_ID --stdin
termy mux send PANE_ID -- -h
termy mux send PANE_ID 'make test' --enter
termy mux wait PANE_ID --command-done --timeout-ms 600000
termy mux capture PANE_ID
termy mux capture PANE_ID --start -500 --end -1
termy mux key PANE_ID ctrl+c
termy mux key PANE_ID up up enter
termy mux key PANE_ID c --control
termy mux resize PANE_ID 120 40
termy mux split PANE_ID --axis horizontal
termy mux layout
termy mux tab PANE_ID rename 'Build logs'
termy mux tab PANE_ID reset-title
termy mux tab PANE_ID pin
termy mux tab PANE_ID unpin
termy mux tab PANE_ID zoom
termy mux tab PANE_ID unzoom
termy mux tab PANE_ID focus OTHER_PANE_ID
termy mux tab PANE_ID resize-divider right 8
termy mux tab PANE_ID resize-divider right -8
termy mux workspace WINDOW_ID 0 rename 'Agent work'
termy mux workspace WINDOW_ID 0 pin
termy mux workspace WINDOW_ID 0 unpin
termy mux workspace WINDOW_ID 0 select-tab 1
termy mux workspace WINDOW_ID 0 move-tab 0 1
termy mux window WINDOW_ID create-workspace Review
termy mux window WINDOW_ID select-workspace 1
termy mux window WINDOW_ID move-workspace 1 0
termy mux window WINDOW_ID delete-empty-workspace 1
termy mux close PANE_ID
```

Pane and window IDs come from `list` and `layout`. Workspace indices are
zero-based positions in the returned layout. Commands never attach to a pane:
they read it with single requests, and `create` and `split` start panes
without attaching, so the CLI never receives a pane's clipboard requests or
changes what a desktop window shows. The terminal remains alive when a command
exits. `close` terminates one terminal. `shutdown` terminates all terminals and
stops the host.

## Errors and exit codes

A failure writes `{"schema_version":2,"ok":false,"error":{"code":...,"message":...}}`
to stderr. Command-line errors use the same shape, also a missing subcommand
(`termy mux`, `termy mux tab ID`); only `--help` and `--version` print plain
text. The exit status is 2 for `invalid_argument` and 1 for every other code;
tell failures apart by `code`:

| `code` | Meaning |
|---|---|
| `invalid_argument` | The command line is invalid: an option out of range, empty `wait` text, an unknown key name, input over 1 MiB, or a `--working-directory` that is not a directory. |
| `not_found` | No pane, window or workspace has that ID or index, or the pane was closed (also while `wait` ran). |
| `exited` | The pane's program has exited: `send` and `key` input would reach no one, and `wait` cannot succeed any more. A pane that was closed after its program exited, as a desktop window closes it, reports `exited` for 10 minutes (the host remembers the last 256 such panes), then `not_found`. |
| `input_dropped` | The pane's input backlog is full because its program stopped reading; the input was not sent. |
| `timeout` | `wait` gave up after `--timeout-ms`. |
| `output_lost` | `wait` gave up, and output it should have searched left the scrollback (`scrollback_history`) before it could read it, so the text may have been there. |
| `unsupported` | The pane's state does not allow the command, such as `resize` of a pane that shares its tab. |
| `host_unavailable` | No host runs in the session directory, or it cannot be reached. |
| `version_mismatch` | A host of another Termy version runs; `termy mux shutdown` stops it. |
| `failed` | Anything else, such as a layout edit the saved layout rejects. |

## Input

`send` sends its text literally, without shell escaping or interpretation,
and `--enter` appends Enter (`\r`). Text may begin with `-`; put `--` before
text that is also an option name (`send PANE_ID -- -h`). `--stdin` sends
standard input as bytes, which need not be UTF-8. Input is limited to 1 MiB.
`send` returns once the host queued the input for the program, not when the
program handled it; it fails with `exited` or `input_dropped` when the input
cannot reach the program. `send` and `key` return an `anchor`: the line the
cursor was on when the host queued the input, counted from the first line
that ever scrolled into history. `wait --after ANCHOR` searches the output from
that line on. `send --wait TEXT` sends, then waits like `wait --after` with its
own anchor (`--timeout-ms` applies), and returns that result as `wait`; a
failure there says the input was sent.

`key` sends one or more keys in a single write, in order. A key is a single
character, sent as itself (an uppercase letter is Shift and that letter), or
one of these names, in any case:

| Key | Names |
|---|---|
| Enter | `enter`, `return`, `ret`, `cr` |
| Tab | `tab` |
| Escape | `escape`, `esc` |
| Backspace | `backspace`, `bs`, `bksp` |
| Space | `space`, `spc` |
| Arrows | `up`, `down`, `left`, `right` |
| Home, End | `home`, `end` |
| Page Up | `pageup`, `pgup`, `page-up`, `page_up` |
| Page Down | `pagedown`, `pgdn`, `pgdown`, `page-down`, `page_down` |
| Insert | `insert`, `ins` |
| Delete | `delete`, `del` |
| F1–F12 | `f1` … `f12` |

Prefix a key with modifiers joined by `+`: `ctrl` (or `control`), `alt` (or
`meta`, `option`, `opt`) and `shift`, as in `ctrl+c`, `alt+left` or
`ctrl+alt+x`; `+` itself is `+` or `ctrl++`. `--control`, `--alt` and
`--shift` hold a modifier for every key. Keys are encoded as xterm on Linux
encodes them, on every platform, and in the pane's keyboard mode (legacy, or
the kitty keyboard protocol when the program enabled it): Alt sends Escape
before the key, as with Option as Alt on macOS, and modified arrows send
their modifier sequences (`ctrl+left` is `ESC [1;5D`) instead of the desktop's
prompt shortcuts. Shift capitalizes a letter; for a shifted symbol, send the
symbol itself.

## Reading output

`capture` and `wait` read the host's terminal, not a view of it: a desktop
window scrolled into history does not change what they see. Soft-wrapped rows
join into one line, so text wider than the pane is found and returned whole.

`capture` returns the screen: `text`, `cols`, `rows`, `cursor` (`row` 0 is the
top screen row), `alternate_screen`, `history_size`, and `start` and `end`, the
lines returned. `--start` and `--end` select other lines, numbered from the top
of the screen: 0 is its first row and -1 the newest history line, so
`--start -500 --end -1` returns the last 500 lines of scrollback.

`wait PANE_ID TEXT` waits until the output contains `TEXT`, polling every
100 ms. Without `--after`, it searches the screen as the wait starts and all
output after it, also lines that scrolled into history between two polls;
`--start -N` also searches the N history lines before. That includes text an
earlier command left on screen: a marker that repeats in a loop matches its
previous round at once, and output that scrolled away between `send` and
`wait` is missed. `--after ANCHOR` (or `send --wait`) avoids both. It returns
`line`, where the match starts (numbered like `--start`, from the current
screen), `text`, the lines that contain it, and `truncated`, true when some
output left the scrollback before it could be searched. It fails with
`timeout` after `--timeout-ms` (5000 by default, at most an hour), or
`output_lost` when output was lost too, and at once with `exited` when the
pane's program exits (after a last look at its output) or with `not_found`
when the pane is closed. When the pane is resized, lines reflow by an amount
no counter tracks, so the search starts again from the current screen and as
much history as output added since the wait began.

The search starts on the line the input was typed on, so it also sees the
command line the shell echoed. Split the marker so that only the program's
output contains it:

```sh
termy mux send PANE_ID "make; printf '%s_%s\n' build done" --enter --wait build_done
```

The echo shows `printf '%s_%s\n' build done`; only the output reads
`build_done`. Waiting for `ready` after `send PANE_ID "echo ready"` would match
the echo before the command ran.

`wait PANE_ID --command-done` waits until the command that the pane's latest
input started has finished, and returns its `exit_code`. It needs shell
integration: a shell that reports each command's start and exit code with
OSC 133, such as one that sources Termy's integration script
(`assets/shell/termy.*`) while `tab_title_shell_integration` is on (the
default). Without it, the wait ends in `timeout` and says so. The host decides
which finish (`D`) belongs to the input, in order with the input: only one
that follows a command start (`C`) after the input. A command that was
running when the input arrived finishes first and does not count, nor does
the `D` fish prints at its first prompt. A shell that never reports `C`
(PowerShell) finishes the command with the first `D` after a prompt that
showed when, or after, the input arrived; there, input typed while a command
runs is taken for a new command. Input sent later replaces the one waited for,
so wait before sending more.

`list` reports each pane's `id`, `child_pid`, `title`, `working_directory`
and `exited`. `list --verbose` adds `cols`, `rows` and `shell`: `null` until
the pane reported shell integration, then `running` (a command runs),
`commands_started`, `commands_finished` and the last `exit_code`.

## Creating, splitting and resizing

`create` starts the shell and settings of the user's Termy config, as a new
desktop tab would: `shell`, `term`, `colorterm`, `term_program`,
`scrollback_history`, `cursor_style` and shell integration. `--shell` replaces
the shell. Invalid config lines are reported in `warnings`, and the rest
applies. The CLI cannot read the system appearance, so a theme that follows it
answers terminal colour queries as in dark mode. The pane starts in
`--working-directory`, resolved against the command's directory (`~` against
its home), or in the command's directory; symbolic links are kept, as in a
desktop tab, and the host uses the path as given. A path that is not a
directory fails with `invalid_argument` before anything starts. Each pane starts from the environment of the command that created it,
not that of the process that started the host. `list` shows the directory a
pane started in, then the one its shell last reported (OSC 7). Panes see
`TERMY_MUX_DIR` and their own ID in `TERMY_MUX_PANE`. Terminal sizes are
1–400 columns and 1–200 rows.

Creation saves a tab in the selected workspace: `--window` alone selects that
window's active workspace, and without `--window` it goes to the first
window's active workspace. A fresh host creates its first window and
workspace.

`split` starts the new pane with the settings and shell the source pane was
started with, in the source pane's current directory, with the environment of
the `split` command. Splitting uses desktop's split tree and saves both pane
IDs; horizontal means left/right and vertical means top/bottom, and each half
gets half of the pane. Closing a split pane removes its saved entry and
expands its sibling using that same layout tree; a tab whose tree cannot be
inferred is re-tiled evenly instead. `close` updates the layout before it ends
the process, so a failed close never leaves a dead pane listed. It fails with
`not_found` for an ID that neither the host nor the saved layout knows.
Tab and workspace selection is persisted in the shared layout.
Reordering preserves the active item. Workspace deletion requires an empty
workspace and preserves at least one workspace in the window.

`resize` resizes a pane that is alone in its tab and saves the new size in
the layout, so later splits start from it. It keeps the cell metrics the
desktop measured for the pane, which programs see as its pixel size. A pane
that shares its tab follows the tab's dividers, so resizing it alone fails
with `unsupported`; use `tab PANE_ID resize-divider`. A desktop window that
shows the pane normally wins a size conflict: after `resize`, it sets the pane
back to the size it draws on its next layout pass. It does so at most 3 times
within 10 seconds per pane. After that (for example when two windows or two
Termy processes show the same pane at different sizes), it keeps the size the
host has until its own layout changes.

## Session directory and layout

Pass `--session-dir PATH` anywhere after `mux` to select an isolated host.
Inside a multiplexer pane the default is the pane's own host, from
`TERMY_MUX_DIR`; elsewhere it is the desktop config directory's `multiplexer`
subdirectory (`%LOCALAPPDATA%\termy\multiplexer` on Windows, falling back to
the old location while an older host still runs there). The directory holds
the host's access token, so it must be private: on Unix yours and mode 0700;
on Windows owned by you (or SYSTEM or the administrators), and Termy replaces
its permissions with ones granting access only to you and SYSTEM. Use a
dedicated directory; one owned by another account is refused.

The host owns the layout. It keeps a revision with it, rejects layouts this
version cannot read, and removes a pane from it when the pane is closed, or
when its process exits while no window shows it. Every edit reads the layout,
changes it and writes it back only if the revision is unchanged, retrying a
bounded number of times, so independent edits never overwrite each other.
`create` and `split` check their target before starting a process. Once the
process runs, they add it to the layout as it is at that moment: concurrent
edits by the desktop or other commands cause a retry, never a failure that
kills the new pane. `tab`, `workspace` and `window` edits fail instead when
their snapshot is stale.

Open desktop windows apply layout changes at once: new tabs and split panes
attach, closed panes and tabs disappear, split trees, sizes, renames, pins,
zoom and selections follow, and new or renamed workspaces show up. The window
re-fits the affected terminals to its own size afterwards. When the desktop
changed the same window concurrently, its edits are rebased onto the host's by
pane ID: both sides' new tabs are kept, the host's tab and workspace order
wins, and if both split the same tab, the desktop's new pane moves to a tab of
its own beside it.

When the desktop starts, it closes terminals whose process already exited
and removes them from the layout, so reattaching never replays their exit.
This trades away their final output: `capture` a script's pane before
launching Termy if you still need what it printed. A saved window left with no
tab at all is not restored.

Once a layout change is committed, follow-up steps are best effort. `split`,
`close`, `resize` and `tab ... resize-divider` report their failures in a
`warnings` array in `result` and still succeed; do not retry them. `create`
also warns when the Termy config cannot be read and the defaults apply.

A host started by a Termy version with another session protocol cannot serve
this version's commands; they fail with `version_mismatch` and name `termy mux
shutdown`. Commands never restart such a host or kill its sessions
automatically. `shutdown` is the exception that works with any host version:
it authenticates with the frozen handshake frames every host understands,
stops the host and ends all of its terminals. `shutdown --force` stops a host
that no longer answers by signalling the process ID it recorded in
`endpoint.json`. Without a live host (no process holds `host.lock`), commands
report that no session host is running. The host writes its log, including
start-up errors and panics, to `host.log` in the session directory (rotated to
`host.log.1` past 1 MiB).

## Not supported yet

- `split --size`, `--percent` and `--before`: the saved split tree always
  halves a pane and puts the new one after it; other ratios or orders need
  changes to the split tree code.
- `create --background`: placing a tab without selecting it needs another
  placement mode in the shared layout code.

## Changes in schema version 2

- Every error has `error.code`, and command-line errors are JSON too (exit 2).
- `capture` reads the host's lines: it adds `start`, `end`, `alternate_screen`
  and `history_size`, `cursor` is relative to the screen, and
  `display_offset` is gone (no view's scroll position matters).
- `wait` returns `line`, `text` (the matching lines, not the screen) and
  `truncated`, instead of the capture of the screen.
- `send` and `key` return `anchor`; `key` also `keys` and `bytes`.
- `list` keeps its fields; `list --verbose` adds `cols`, `rows` and `shell`.

## Validation

```sh
cargo test -p termy_cli --test mux
cargo test -p termy_cli --bin termy-cli mux
cargo test -p termy_core --test ipc
cargo test -p termy_core
```
