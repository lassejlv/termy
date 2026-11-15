---
title: Sessions
description: Close the window, keep the work. Programs run in a session host, so quitting only detaches.
---

## Sessions outlive the app

Termy Next runs its shells in a Termy session host, the multiplexer behind `termy mux`, in its own directory:

```txt
~/Library/Application Support/TermyNext/multiplexer
```

Quitting detaches. Every program keeps running, and the next launch reattaches to them in the same windows, sessions, tabs and splits. Updates install on quit too, so they don't interrupt anything.

## Windows, sessions and tabs

A session is a named group of tabs, such as `shy-orchard`. Each window shows one session.

- **⌘K** changes the window's session, or picks a detached one.
- **⇧⌘N** starts a new session.
- **⌘T** adds a tab, and each tab is a split layout of [panes](/docs/panes).

## What ends a program

| You close | What happens |
| --- | --- |
| A pane, tab or session | Its programs end. |
| A window | Its session detaches and keeps running. It stays in ⌘K. |
| The app | Every session detaches. The next launch reattaches. |

## Turn the host off

The session host can be turned off in Settings. New panes then run inside the app and end when it quits. You can also restart the host from Settings or the app menu.

> Panes running in the app aren't part of the host's layout, so `termy mux` and mobile access don't see them.
