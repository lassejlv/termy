---
title: Updates
description: Release builds update themselves in the background and install the new version when you quit.
---

## How updates work

Termy Next uses [Sparkle](https://sparkle-project.org). It checks for a new version every hour, downloads it in the background, and installs it the next time you quit. Because your programs run in the session host, quitting to update doesn't stop them.

The first launch of a new version checks again right away, so an update downloaded before a newer release doesn't leave you a version behind.

## Update now

- **Termy Next → Restart to Update** shows the downloaded version's release notes with **Install and Relaunch**.
- **Termy Next → Check for Updates…**, also in the command palette, checks right away.

## Turn it off

**Settings → General → Updates** turns automatic checks and downloads on or off.

> Development builds and `scripts/bundle.sh` bundles have no update feed and never update.

## Release notes

The update window shows the release notes from the [changelog](https://github.com/lassejlv/termy/blob/main/next/CHANGELOG.md), including for updates that already downloaded in the background.
