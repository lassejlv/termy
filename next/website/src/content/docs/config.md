---
title: Config file
description: A Termy-format config of its own, so Termy Next can differ from Termy where you want it to.
---

## Where it lives

Termy Next reads `~/.config/termy-next/config.txt`, separate from the Termy app's `~/.config/termy/config.txt`. It's created empty on first launch, so Termy's defaults apply. Local themes live in the `themes` folder beside it.

Change the common settings in **Settings (⌘,)**. **Open Config File** and **Reload Config** are in the app menu.

## Import your Termy config

Settings can import Termy's config. Termy Next's current config is kept as `config.txt.bak`, and the local themes the imported file uses are copied next to it.

## Common settings

The file uses the same keys as Termy's. Every key is in [Termy's configuration reference](https://github.com/lassejlv/termy/blob/main/docs/configuration.md).

```txt title="~/.config/termy-next/config.txt"
theme = termy
font_family = JetBrains Mono
font_size = 14
line_height = 1.4
padding_x = 12
padding_y = 8
shell = /bin/zsh
```

| Key | Default | Meaning |
| --- | --- | --- |
| `theme` | `termy` | The color scheme. |
| `font_family` | `Menlo` | The terminal font. |
| `font_size` | `14` | Font size in pixels. Each pane adds its own ⌘+ / ⌘− zoom. |
| `line_height` | `1.40` | Line height multiplier, from 0.8 to 2.5. |
| `padding_x`, `padding_y` | `12`, `8` | Padding around the terminal. |
| `shell` | unset | The program new panes start. |

## Tasks and plugins

[Tasks](/docs/tasks) also go in this file, as `task.<name>.*` keys. [Lua plugins](/docs/plugins) live in `init.lua` in the same folder.
