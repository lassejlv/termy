---
title: Rex compatibility
description: Rex's model, IDs, plugin API and command-line tool, answered by Termy Next. It never talks to Rex itself.
---

## The model

| Rex | Termy Next | Notes |
| --- | --- | --- |
| Session | Session | A named group of tabs, one per window. Closing a window detaches it. |
| Window | Tab | A split tree of panes, plus floating panes. |
| Block | Pane | A rounded card with a header and a terminal. |
| Floating layer | Floating panes | One or more floating panes that move and stack together. |

IDs keep Rex's form (`session:…`, `block:…`) and survive relaunches for panes in the session host. Block creators and flavors answer to Rex's names (`com.superlogical.terminal.shell`) as well as `sh.termy.terminal.shell`.

## Commands in panes

A block's command runs through `termy-run`, which keeps the program in front and reports how it exited. A command that fails within moments keeps its pane open so you can read why. Panes nobody can see are still polled, so programs there get answers to their terminal queries.

## Rex tools work here

Tools that drive Rex through its `rex` command work in Termy Next, such as [leaf](https://github.com/DeepanshuMishraa/leaf). A plugin that runs Rex's own `rex` by its full path, inside Rex.app or its server as rex-durable does, gets Termy Next's instead, so it drives Termy Next.

## Where Termy Next differs

- It has no network server of its own, so Rex's Tailscale methods answer `not_supported`. Other Macs are reached over ssh.
- `data.connect` streams a block's screen as JSON lines over Termy Next's socket (`data.open`), not Rex's WebSocket protocol.
