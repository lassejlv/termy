---
title: Panes and tabs
description: Every tab is a split layout of panes. Split, float, zoom and drag them where you want.
---

## Split, float and zoom

| Action | Keys |
| --- | --- |
| Split right / down | ⌘D / ⇧⌘D |
| Split left / up | ⇧⌘← / ⇧⌘↑ |
| Split best fit | ⌘↩ |
| Float pane | ⌥⌘↩ |
| Zoom pane | ⇧⌘↩ |
| Focus pane | ⌥⌘ + arrows |
| Move pane | ⌥⇧⌘ + arrows |

Drag a pane's header to move it anywhere in the tab. A floating pane sits above the others; zooming fills the tab with one pane until you zoom out.

## What a pane shows

Pane headers and tabs name the program in front and its directory, such as `zsh ~/Dev/termy` or `nvim ~/notes`. That comes from the kernel, through the shell's foreground process group, and from OSC 7.

Agents wear their own logo and name, like "Claude Code" or "Codex", using the icon of the matching app installed on your Mac. A tab with several programs stacks up to three of their icons, with the one reporting status in front. See [Agents and activity](/docs/agents).

## Customize the view

**View → Customize View** sets:

- **Tab Style:** Above, Left Side or Right Side.
- **Pane Style:** Comfortable or Compact.
- **Interface Style:** Liquid uses Liquid Glass on macOS 26 and later for the panes and the selected tab, over a blurred window backdrop, and a blurred material before that. Classic draws solid chrome from the terminal theme.

## Motion

Splits grow in while their siblings slide over, closed panes shrink away, and zoom and float glide. Terminal grids resize once at the start of a move instead of on every frame. **Reduce Motion** in System Settings turns all of it off.
