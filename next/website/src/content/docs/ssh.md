---
title: SSH
description: Full color on remote machines, copying through OSC 52, and local selection inside remote programs.
---

## 24-bit color over SSH

`ssh` typed in zsh and **Add Remote Host** send `COLORTERM` along, so Claude Code and other programs on the server draw 24-bit color instead of guessing 256 or 16 colors from `TERM`. It's on by default, in **Settings → General → SSH**.

The server must accept the variable. Debian and Ubuntu ship with this in `sshd_config`:

```txt title="/etc/ssh/sshd_config"
AcceptEnv COLORTERM
```

## Copying

Copying over SSH goes through OSC 52, which Termy Next accepts from the pane you're working in.

## Selecting text

Shift- or Option-drag selects text locally inside programs that track the mouse, such as vim or tmux on the server.
