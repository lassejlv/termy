# Multiplexer CLI

`termy-cli mux` controls the same persistent host used by desktop.
The installed launcher exposes this as `termy mux`. Commands are noninteractive
and return one JSON object with `schema_version: 1`, `ok`, and `result`.
Runtime failures write a JSON error to stderr and exit 1; argument errors exit 2.
Only `start` and `create` start a missing host. Other commands fail if it is absent.

```sh
termy mux start
termy mux create --cols 100 --rows 30 --working-directory /path/to/project
termy mux create --window WINDOW_ID --workspace 0
termy mux list
termy mux send PANE_ID "printf 'ready\\n'" --enter
printf 'literal input' | termy mux send PANE_ID --stdin
termy mux wait PANE_ID ready --timeout-ms 5000
termy mux capture PANE_ID
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
zero-based positions in the returned layout. `send` sends literal UTF-8;
`--enter` appends Enter. It waits for host acceptance, not command completion.
Use `wait` to check output. `capture` returns the visible viewport, not the
entire scrollback. A capture or input command detaches when it exits; the
terminal remains alive. `close` terminates one terminal. `shutdown` terminates
all terminals and stops the host.

Creation saves a tab in the selected workspace, or the first window's active
workspace by default. A fresh host creates its first window and workspace.
Splitting uses desktop's split tree and saves both pane IDs; horizontal means
left/right and vertical means top/bottom. Closing a split pane removes its
saved entry and expands its sibling using that same layout tree.
Tab and workspace selection is persisted in the shared layout.
Reordering preserves the active item. Workspace deletion requires an empty
workspace and preserves at least one workspace in the window.

Pass `--session-dir PATH` anywhere after `mux` to select an isolated host.
The default uses the desktop config directory's `multiplexer` subdirectory.
Input is bounded to 1 MiB; terminal sizes to 1–400 columns and 1–200 rows.

Workspace edits use conditional host writes. Independent edits preserve other
windows; a stale workspace snapshot fails instead of overwriting another client.
An older running host supports terminal commands but must be replaced after its
sessions are closed before it can support layout edits. Commands do not restart
an older host or kill its sessions automatically.

## Validation

```sh
cargo test -p termy_cli --test mux
cargo test -p termy_core --test ipc
cargo test -p termy_core
```
