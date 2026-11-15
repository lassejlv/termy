---
title: Lua plugins
description: Add actions, keys and modes in a few lines of Lua. Termy Next speaks Rex's plugin API, so Rex plugins run unchanged.
tags: ['Lua 5.4', 'Since 0.3.0']
---

## Your first action

Termy Next loads `~/.config/termy-next/init.lua` at launch. Add an action and bind it to a key, then run **Reload Plugins** from the palette.

```lua title="~/.config/termy-next/init.lua"
termy.action{
  name  = "htop_beside",
  title = "htop Beside",
  run   = function(ctx)
    termy.session.new_split{
      anchor_id = ctx.block_id, direction = "horizontal",
      side = "after", focus = true,
      layout = termy.layout.block{
        label = "htop", options = { command = { "htop" } },
      },
    }
  end,
}
termy.bind("ctrl+shift+h", "htop_beside")
```

Actions show up in the command palette with their `description`, `category`, `keywords` and bound key. Required `args` are asked for before the action runs. **Open Plugin Config** in the palette opens the file, with a starting point written the first time.

## Where plugins load

Two files load in order. `rex` and `termy` are the same table, so either name works in either file.

1. `~/.config/termy-next/init.lua`: your config, written against `termy.*`.
2. `$REX_CONFIG`, or else `~/.config/rex/init.lua`: Rex's config, where Rex plugins such as leaf install themselves. Turn it off in **Settings → Plugins**.

> A file that fails to load leaves the last working one in place. Its error shows in **Settings → Plugins**, next to every action, its key and a Run button.

## The API at a glance

Everything lives on one table. Calls return a result, or `nil, err`. The [API reference](/docs/api) lists every function.

| Function | What it does |
| --- | --- |
| `action{…}`, `invoke()` | Palette actions. Unknown names run Termy Next's own, like `pane.split`. |
| `bind()`, `mode()` | Keys, sequences like `ctrl+b>c`, physical keys and modes. |
| `session.*{…}` | Windows, splits, floating layers, detached blocks and resizing. |
| `block.call()` | Read a pane's process, directory, screen, title or program status. |
| `exec()`, `exec.start()` | Run a program in your login shell's environment. |
| `on()`, `after()`, `every()` | Session, window and block events, plus timers. |
| `server(label)` | Drive Termy Next on another Mac over ssh. |

## Lua version

Lua is 5.4, with Rex's 5.1 functions added back: `setfenv`, `getfenv`, `module`, `unpack`, `loadstring`, `table.getn` and others. A key that can't be read is reported and skipped.

## Try leaf

[leaf](https://github.com/DeepanshuMishraa/leaf) is a Rex plugin that opens a document over the pane, reviews diffs, asks the agent pane about a selection with ⇧⌘A, and reads aloud. It runs here as is:

```sh
leaf rex install
```
