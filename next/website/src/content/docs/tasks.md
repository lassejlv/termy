---
title: Tasks
description: Name the commands you run often, then start any of them in a new tab from the palette.
---

## Define tasks

Termy's tasks work here too. Add them to Termy Next's [config file](/docs/config):

```txt title="~/.config/termy-next/config.txt"
task.build.command = cargo build
task.build.working_dir = crates/cli
task.deploy.layout = shy-orchard
task.deploy.command = ./deploy.sh
```

| Key | Meaning |
| --- | --- |
| `task.<name>.command` | The command to type into the shell. Required. |
| `task.<name>.working_dir` | Where it runs. `~` is your home folder; a relative path starts from the focused pane's directory. |
| `task.<name>.layout` | Only offer the task in the session with this name. |

## Run a task

**Run Task…**, in the File menu and the command palette, lists your tasks, and the palette also matches them by name. A task opens a new tab and types its command into the shell, which stays open afterwards, so you can read the output and run it again.

> `task.<name>.keybind` is read but not used yet. Bind a key to a task with a [Lua plugin](/docs/plugins) instead.
