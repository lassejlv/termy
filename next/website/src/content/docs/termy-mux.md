---
title: termy mux
description: The session host's layout is Termy Next's layout. Change it from the command line and watch the app follow.
---

## One layout, two views

Each Termy Next window is a host window, each session a workspace, and tabs, splits, titles, focus and font zoom match. Changes in the app are written to the host, and `termy mux` changes show up live.

```sh
termy mux layout                         # what is on screen
termy mux create --window <id>           # a new tab in that window
termy mux split <pane> --axis vertical   # split a pane
termy mux tab <pane> rename build        # rename its tab
termy mux window <id> select-workspace 1 # switch the window's session
termy mux send <pane> "make" --enter     # type into a pane
termy mux close <pane>                   # end a pane
```

## Finding the host

Inside a Termy Next pane, `termy mux` finds the host through `TERMY_MUX_DIR`. Elsewhere, pass the directory:

```sh
termy mux layout --session-dir ~/Library/Application\ Support/TermyNext/multiplexer
```

## Limits

- Floating panes are stored as splits, and float again when Termy Next shows them.
- Panes running in the app, with the session host off, aren't in the layout.
