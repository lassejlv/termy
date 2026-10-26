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
entire scrollback. Commands never attach to a pane: `capture`, `wait` and
`key` read one snapshot of it (`wait` polls), and `create` and `split` start
panes without attaching, so the CLI never receives a pane's clipboard
requests. The terminal remains alive when a command exits. `close` terminates one terminal. `shutdown` terminates
all terminals and stops the host.

Creation saves a tab in the selected workspace, or the first window's active
workspace by default. A fresh host creates its first window and workspace.
Splitting uses desktop's split tree and saves both pane IDs; horizontal means
left/right and vertical means top/bottom. Closing a split pane removes its
saved entry and expands its sibling using that same layout tree; a tab whose
tree cannot be inferred is re-tiled evenly instead. `close` updates the layout
before it ends the process, so a failed close never leaves a dead pane listed.
Tab and workspace selection is persisted in the shared layout.
Reordering preserves the active item. Workspace deletion requires an empty
workspace and preserves at least one workspace in the window.

Pass `--session-dir PATH` anywhere after `mux` to select an isolated host.
The default uses the desktop config directory's `multiplexer` subdirectory
(`%LOCALAPPDATA%\termy\multiplexer` on Windows, falling back to the old location
while an older host still runs there).
Input is bounded to 1 MiB; terminal sizes to 1–400 columns and 1–200 rows.
A desktop window that shows the pane normally wins a size conflict: after
`resize`, it sets the pane back to the size it draws on its next layout pass.
It does so at most 3 times within 10 seconds per pane. After that (for example
when two windows or two Termy processes show the same pane at different sizes),
it keeps the size the host has until its own layout changes.

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

Once a layout change is committed, follow-up terminal resizes are best effort.
`split`, `close` and `tab ... resize-divider` report their failures in a
`warnings` array in `result` and still succeed; do not retry them.

A host started by a Termy version with another session protocol cannot serve
this version's commands; they fail and name `termy mux shutdown`. Commands
never restart such a host or kill its sessions automatically. `shutdown` is the
exception that works with any host version: it authenticates with the frozen
handshake frames every host understands, stops the host and ends all of its
terminals. `shutdown --force` stops a host that no longer answers by signalling
the process ID it recorded in `endpoint.json`. Without a live host (no process
holds `host.lock`), commands report that no session host is running. The host
writes its log, including start-up errors and panics, to `host.log` in the
session directory (rotated to `host.log.1` past 1 MiB).

## Validation

```sh
cargo test -p termy_cli --test mux
cargo test -p termy_core --test ipc
cargo test -p termy_core
```
