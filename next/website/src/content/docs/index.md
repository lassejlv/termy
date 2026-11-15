---
title: Introduction
description: A session-first terminal for macOS. Quit it or update it and every program keeps running; the next launch puts each window, tab and split back.
tags: ['macOS 14+', 'Version 0.3.0']
---

Termy Next is a native macOS terminal written in Swift, modelled on Superlogical's Rex and built on libtermy, the engine behind Termy, through the Swift SDK. Its shells run in a Termy session host rather than in the app, so closing the app is never the end of your work.

## What you get

- **Sessions that outlive the app.** Quitting detaches. Builds, servers and agents keep running, and the next launch reattaches to them in the same windows, tabs and splits.
- **Agents you can see.** Claude Code, Codex and other programs report their state through OSC 7501. Tabs show who is working, who needs you, and who is done.
- **A shell that helps.** An optional zsh plugin draws completions under the prompt, and the command box turns a description into a command.
- **Your own commands.** Lua plugins add actions, keys and modes. Plugins written for Rex run unchanged.
- **Scriptable from anywhere.** The `termy` command opens splits and tabs, runs commands and reads a pane's screen, from scripts or from an agent.

## Where to start

1. [Install Termy Next](/docs/install) and take the [welcome tour](/docs/welcome-tour).
2. Learn how [sessions](/docs/sessions) and [panes](/docs/panes) fit together.
3. Turn on the [zsh plugins](/docs/zsh-plugins) and see what your [agents](/docs/agents) are doing.
4. Make it yours with [themes](/docs/themes), [shortcuts](/docs/shortcuts) and [Lua plugins](/docs/plugins).

> Termy Next reads Termy's config format and can import your Termy config, so your theme, font, padding and shell carry over.
