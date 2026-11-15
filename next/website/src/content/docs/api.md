---
title: termy.* API
description: Every function a Lua plugin can call. It's Rex's plugin API, under both the termy and rex names.
tags: ['Lua 5.4']
---

## Actions and keys

| Function | What it does |
| --- | --- |
| `action{…}`, `invoke(name, args)` | Palette actions. Unknown names run Termy Next's own: `pane.split`, `pane.zoom`, `client.tab.goto`, `client.mode.enter`, `termy.<command>`… |
| `bind(key, name or fn, args)` | Keys, sequences (`ctrl+b>c`), physical keys (`cmd+[KeyD]`) and modes (`resize/left`). A function that returns true takes the key; otherwise the pane gets it. |
| `unbind`, `bindings()` | With no action, `unbind` also takes a key from Termy Next's own shortcuts. |
| `mode(name, {exclusive})`, `modes()` | Key modes. |

## Sessions and panes

| Function | What it does |
| --- | --- |
| `session.*{…}`, `call(method, params)` | Every session method: windows, splits, floating layers (with splits, raise, lower, minimize), detached blocks (`new_block`, `detach_block`, `move_block`), owners, `resize_pane`, `view`, `target.resolve`… |
| `block.call(creator, method, args)` | `process` (with `last_exit`), `pwd`, `program_status`, `write`, `format` (screen and scrollback as plain, VT or HTML), `modes`, `input_mode`, `size`, `resize`, `set_theme`, `title`, `list_dir`, `clear`, `reset`, `blob_read`, `clipboard_read`, `download_begin`, `upload_begin`. |

Under a full-screen program, `modes` reports `screen` as `alternate`.

## Processes, events and timers

| Function | What it does |
| --- | --- |
| `exec(argv, opts)` | Run a program in the login shell's environment, with `env`, `cwd`, `timeout`, `stdin`, `detach`, and `stdout`, `stderr` and `on_exit` callbacks. |
| `exec.start(argv, opts)` | The same, returning a handle. |
| `on(event, [filter], fn)` | Rex's events: `session_*`, `window_*`, `active_window_changed`, `block_created`, `block_closed`, `block_event` with `program_status`, `title_changed`, `child_exited`, `selection_copied`, `keymap_changed`, `hosts_changed`, `client_*`. |
| `wait`, `after`, `every`, `sleep`, `stop` | Timers. |

## Other Macs and helpers

| Function | What it does |
| --- | --- |
| `server(label)`, `servers()`, `host{label, ssh}` | Other Macs running Termy Next. `rex.server("studio").session.list{}` runs there over ssh. |
| `json`, `layout`, `log` | JSON, layout helpers, and `plugins.log` in the support folder. |
| `client.queue`, `command(name)`, `commands()` | Termy Next's own commands. |

## Results and errors

Calls return their result, or `nil, err`:

```lua
local sessions, err = termy.session.list{}
if not sessions then termy.log("list failed: " .. err) end
```
