---
title: zsh plugins
description: Completions under the prompt, from your history and from the commands you use every day.
tags: ['Off by default']
---

## Turn them on

Turn on **Shell plugins** in **Settings → General**. Panes you open afterwards start zsh with a small plugin, loaded through a ZDOTDIR of Termy Next's that puts yours back first. It reports the command line over a Unix socket as you type, and the app draws native UI at the cursor.

The plugin does nothing when Termy Next isn't listening, and a closed socket can't end the shell.

## Completions

As you type, a list shows history entries and commands that start with what you typed.

| Keys | Action |
| --- | --- |
| ↓ / ↑ or ⌃N / ⌃P | Pick a completion |
| Return or Tab | Put it on the line |
| Esc | Hide the list |

With **Pick completions with ↑ and ↓** off, the arrows stay with the shell's history until ⌃N / ⌃P picks a row.

## Command completions

An experimental second checkbox under Shell plugins, off by default. The list then also offers what can come next for common tools: subcommands and options with a short description, and your project's own data.

- **Tools:** git, brew, cargo, rustup, rustc, bun, bunx, npm, pnpm, npx, just, ssh, claude, codex, and file commands such as rm, mkdir, cd, cp, mv, zed, code and open.
- **Project data:** branches, stashes, changed files, installed formulae, Cargo binaries, package.json scripts, node_modules tools, just recipes, ssh hosts, and Claude Code and Codex sessions to resume.

Generators run in the pane's directory and are cached for a few seconds.

## The command box

The plugin also powers the [command box](/docs/command-box), which writes a command from a description.
