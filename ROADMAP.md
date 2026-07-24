# Termy roadmap

Ship a terminal people trust — then keep changing it cheap.

**Today:** `0.2.29` · 21 crates · ~127k lines Rust · ~1,400 tests


## North star

A fast native terminal on macOS, Linux, and Windows — tabs, splits, search, themes, tmux — without turning into a control panel.

**v1.0** means signed builds, solid docs, and the quality gates in [docs/engineering/](docs/engineering/).


## Already solid

Defend these; don’t reopen them casually.

- Tabs, splits, pinning, search, command palette
- Config + live reload, themes, deeplinks
- OSC escapes and Kitty graphics
- Optional tmux control-mode sessions
- Plugins, SSH hosts, auto-update
- CLI companion, website docs, AUR + AppImage
- Crate boundaries, CI tests/fmt, `just validate`

Native macOS (Swift) is a strong preview — production cutover lives in [macos/road.md](macos/road.md).


## Path to v1.0

### Trust

- Sign and notarize macOS releases (tooling exists; builds still ship unsigned)
- Sign Windows installers
- Keep crash logs and startup failures visible

### Platforms

- Finish Linux parity where it still feels thin (native menus, file drop)
- Linux ARM64 release artifacts
- Prefer measurable gates over packaging sprawl (Flatpak/Copr only if someone owns them)

### Product

- Multiple GPUI windows
- MRU tab switching (issue closed; behavior not in tree yet)
- Optional font ligatures
- Accessibility pass on the GPUI app


## After v1.0

Polish, not blockers.

- Migration guides (Ghostty, Alacritty, iTerm2, Windows Terminal)
- Broader image-protocol coverage if needed beyond Kitty
- Perf budgets that stay green without babysitting
- ADRs / CODEOWNERS when contributor volume warrants it


## Engineering

Pipeline trust (E0) is landed. Ongoing work — file budgets, `terminal_view` decomposition, tmux/FFI confidence, operability — lives here:

- [Engineering roadmap](docs/engineering/roadmap.md)
- [Quality scorecard](docs/engineering/quality-scorecard.md) (~9/12 · target 10/12 for v1.0)


## Related

- [CONTRIBUTING.md](CONTRIBUTING.md)
- [Project layout](docs/architecture/project-layout.md)
- [Native macOS production](macos/road.md)
