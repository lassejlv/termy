<p align="center">
  <img alt="Termy — The terminal, at full speed" src="./assets/termy-readme-hero.png" width="900" />
</p>

<p align="center">
  <a href="https://github.com/lassejlv/termy/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/lassejlv/termy?style=flat-square&color=7aa2f7&labelColor=16161e" /></a>
  <a href="https://github.com/lassejlv/termy/stargazers"><img alt="GitHub stars" src="https://img.shields.io/github/stars/lassejlv/termy?style=flat-square&color=9ece6a&labelColor=16161e" /></a>
  <a href="./LICENSE"><img alt="MIT license" src="https://img.shields.io/badge/license-MIT-c0caf5?style=flat-square&labelColor=16161e" /></a>
</p>

<p align="center">
  <a href="https://termy.sh/download"><strong>Download</strong></a> ·
  <a href="https://termy.sh/docs"><strong>Documentation</strong></a> ·
  <a href="https://github.com/lassejlv/termy/releases"><strong>Releases</strong></a> ·
  <a href="./CONTRIBUTING.md"><strong>Contribute</strong></a>
</p>

Termy is a fast, native terminal for macOS, Linux, and Windows. It combines GPU-accelerated rendering with the terminal workflows you use every day—tabs, splits, search, tasks, layouts, themes, and optional tmux sessions—without turning the interface into a control panel.

- Damage-scoped GPU rendering with dirty-span cell caching
- Tabs, splits, search, tasks, and reusable layouts
- Configurable keybindings, colors, themes, and terminal behavior
- Optional tmux control-mode sessions
- Native platform integration with a reusable headless runtime and FFI

## Install

Download the latest build from **[termy.sh/download](https://termy.sh/download)** or browse every artifact on **[GitHub Releases](https://github.com/lassejlv/termy/releases)**.

> [!IMPORTANT]
> macOS builds are not signed yet. After moving Termy to `/Applications`, run:
>
> ```bash
> sudo xattr -d com.apple.quarantine /Applications/Termy.app
> ```
>
> See [macOS troubleshooting](https://termy.sh/docs/getting-started/troubleshooting) if Gatekeeper still prevents Termy from opening.

### Build from source

Termy is a Rust workspace. Build and launch the desktop app with:

```bash
cargo run --release -p termy
```

See the [installation guide](https://termy.sh/docs/getting-started/installation) for platform-specific steps.

## What you can shape

Termy keeps its behavior in plain configuration rather than burying it in hidden application state.

| Surface | What you control |
| --- | --- |
| Appearance | Themes, colors, fonts, chrome contrast, and tab presentation |
| Input | Keybindings, terminal behavior, mouse reporting, and shortcuts |
| Workspace | Tabs, split panes, tasks, reusable layouts, and working directories |
| Sessions | Local shells and optional tmux-backed sessions |

Start with [Customize Termy](https://termy.sh/docs/customize) or use the complete [configuration reference](https://termy.sh/docs/reference/configuration-reference).

## Architecture

Termy is more than a window around a PTY. Its terminal emulation is powered by [Alacritty's terminal engine](https://github.com/alacritty/alacritty), wrapped in Termy's reusable runtime. The repository also contains a GPUI desktop application, CLI, native FFI, website, and release tooling.

```text
desktop / embedding hosts
        │
        ├── terminal UI and platform integration
        │
        ├── reusable command, search, theme, and release crates
        │
        └── terminal runtime, PTY, parser, and rendering snapshots
```

Read [Project Layout](./docs/architecture/project-layout.md) for ownership boundaries and [Release Packaging](./docs/architecture/release-packaging.md) for artifact flow.

## Sponsors

Termy is supported by companies and people who care about fast, native developer tools.

<p align="center">
  <a href="https://neon.tech">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="./assets/legends/neon-logo-dark-color.svg" />
      <source media="(prefers-color-scheme: light)" srcset="./assets/legends/neon-logo-light-color.svg" />
      <img alt="Neon" src="./assets/legends/neon-logo-light-color.svg" width="157" />
    </picture>
  </a>
  &nbsp;&nbsp;&nbsp;&nbsp;
  <a href="https://github.com/mezotv">
    <img alt="Dominik Koch" src="https://github.com/mezotv.png" width="64" />
  </a>
</p>

## Roadmap and contributing

- [Engineering quality roadmap](./docs/engineering/roadmap.md)
- [Contributor setup and validation](./CONTRIBUTING.md)

Contributions are welcome. Keep changes scoped, run the nearest validation command, and preserve the boundaries documented in the architecture guide.

<p align="center">
  <sub>MIT licensed · Built in Rust · <a href="https://termy.sh">termy.sh</a></sub>
</p>
