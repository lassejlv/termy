---
title: Command box
description: Describe a command and Claude Code or Codex writes it onto the shell line, with the sign-in they already have.
tags: ['⇧⌘E']
---

## Ask for a command

With the [zsh plugins](/docs/zsh-plugins) on, press **⇧⌘E** in a pane and describe what you want, such as "find the five largest files here". Claude Code (`claude -p`) or Codex (`codex exec`) writes the command onto the line.

1. **Return** runs it.
2. **A follow-up** refines it, such as "only Swift files".
3. **Esc** leaves it on the line for you to edit.

## No keys of our own

The command box uses the sign-in Claude Code or Codex already has on your Mac. Termy Next needs no API keys, and you pick the agent in the box.

> The command box needs the zsh plugin, so it works in panes opened after you turned Shell plugins on.
