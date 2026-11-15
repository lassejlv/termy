---
title: termy CLI
description: Open splits and tabs, run commands, read a pane's screen and send it keys, from scripts or an agent.
tags: ['Since 0.3.0']
---

## In every pane

Panes have `termy` already. It's the same tool as `rex`, under Termy's name:

```sh
termy split -- npm test   # run tests in a split beside this pane
termy capture             # print this pane's screen
termy --help
```

To use it from other shells, see [Add the termy command](/docs/install#add-the-termy-command).

## Commands

| Command | What it does |
| --- | --- |
| `new`, `ls` | Start and list sessions. |
| `run`, `split` | Run a program in a tab or split. `--wait` exits with the program's status. |
| `do`, `send`, `send-key` | Run an action, type text, or press keys in a pane. |
| `capture` | Print a pane's screen. |
| `kill`, `focus`, `zoom` | End, focus or zoom a pane. |
| `move`, `swap`, `resize` | Rearrange panes. |
| `session`, `window`, `block`, `client` | Work with each part of the model. |
| `events`, `wait` | Follow events, or wait for one. |
| `api`, `actions`, `keymap`, `config` | Call the API, list actions and keys, read the config. |
| `server`, `hosts`, `home`, `whoami` | Other Macs and this one. |
| `terminfo`, `skill`, `completion` | Terminfo, an agent skill, and shell completions. |

## Another Mac

`-S HOST`, or `rex home HOST`, runs a command on another Mac's Termy Next over ssh.

## Environment

Every pane gets these variables:

| Variable | Value |
| --- | --- |
| `TERMY_SESSION`, `REX_SESSION` | The pane's session ID. |
| `TERMY_BLOCK`, `REX_BLOCK` | The pane's ID. |
| `REX_BIN_DIR` | A folder whose `rex` is answered by Termy Next. |
| `TERMY_MUX_DIR` | The session host, for [termy mux](/docs/termy-mux). |
