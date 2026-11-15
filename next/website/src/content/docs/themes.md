---
title: Themes and fonts
description: Search Termy's theme library, or bring the theme you already use in Ghostty, Alacritty, kitty or Rex.
tags: ['Since 0.3.0']
---

## Pick a theme

**Settings → Theme & Font** searches Termy's theme library and sets the terminal font. The last page of the [welcome tour](/docs/welcome-tour) has the same list.

## Import a theme

Choose **Import…** to bring in the theme another terminal uses, or any of their theme files:

| From | What's read |
| --- | --- |
| Ghostty | `theme` and its colors |
| Alacritty | `[colors]` and its `import`s |
| kitty | `kitty.conf` and its `include`s |
| Rex | Its custom themes |

The theme is saved as a JSON file in `~/.config/termy-next/themes`, and Termy Next switches to it.

## Light and dark in terminal programs

Programs that pick light or dark colors from the terminal, such as Claude Code's `auto` theme, Codex and Gemini CLI, read the theme on screen. Color queries (OSC 4/10/11, `CSI ? 996 n`) answer with it, and change with it when the theme or the Mac's appearance changes.

Programs that turn on DEC mode 2031 are told right away (`CSI ? 997 ; 1 n` for dark, `2` for light), so they switch without a restart. Programs that only ask at startup pick up a change on their next run.

## In the config file

The theme and font are ordinary keys in the [config file](/docs/config):

```txt title="~/.config/termy-next/config.txt"
theme = tokyo-night
font_family = JetBrains Mono
font_size = 14
line_height = 1.4
```
