---
title: Install
description: Download the signed app for any Mac running macOS 14 or later, or build it from source.
tags: ['Apple Silicon and Intel']
---

## Download

Get the latest DMG from the [download section](/#download) or from [GitHub Releases](https://github.com/lassejlv/termy/releases), where Termy Next releases are tagged `next-v*`. Each release has:

- `TermyNext-<version>-macos-universal.dmg`, a universal app for Apple Silicon and Intel
- a zip of the same app
- SHA-256 checksums

Open the DMG and drag **Termy Next** to Applications. Release builds are signed with Termy's Developer ID and notarized, so they open without a warning.

> Termy Next releases are pre-releases on GitHub and never become the repository's "Latest" release, which stays Termy's. Look for the `next-v*` tag.

## Add the termy command

Panes already have `termy` on their PATH. To use it from every other shell, choose **Termy Next → Install termy Command…** or turn it on in the welcome tour. It writes `~/.local/bin/termy` and adds that folder to PATH in your shell's startup file. It asks before replacing another `termy`, such as Termy's. See [termy CLI](/docs/cli).

## Build from source

You need Xcode's command-line tools and Rust, since libtermy is built first. From the repository:

```sh
just next          # build libtermy, then swift run (debug)
just bundle-next   # next/build/Termy Next.app (release)
just test-next     # layout, matching and naming tests
```

Without `just`, run this from `next/`:

```sh
../sdk/swift/scripts/build-libtermy.sh && swift run TermyNext
```

Development builds and `scripts/bundle.sh` bundles don't update themselves. See [Updates](/docs/updates).
