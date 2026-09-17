# Contributing to Termy

Thanks for contributing.

This guide focuses on the current Termy workflow so you can get a change from clone to PR without guessing.

## Before you start

- Check open issues and PRs for overlap before starting a larger change.
- Keep changes scoped. Small, reviewable PRs move faster here.
- If you touch generated docs, regenerate them instead of editing generated files by hand.

## Project layout

- `crates/desktop_app/`: main desktop app built with Rust + GPUI
- `crates/`: workspace crates for config, command catalog, terminal runtime, search, terminal UI, packaging helpers, CLI, FFI, and embedding surfaces
- `docs/`: repository docs used by contributors
- `website/`: website and public docs
- `assets/`: app icons, shell completions, UI icons, and media used by the app and website
- `scripts/`: local and CI packaging entrypoints
- `.github/`: issue templates, PR template, and CI workflows

See [Project Layout](docs/architecture/project-layout.md) for ownership boundaries between the app, reusable crates, embedding surfaces, and docs. See [Release Packaging](docs/architecture/release-packaging.md) before changing packaging scripts or release workflow artifacts.

## Local setup

Requirements:

- Rust stable toolchain
- Platform build dependencies for GPUI and native packaging where relevant

Optional tools:

- `just` for the repo command shortcuts
- `cargo-watch` if you want the `just dev` loop
- `tmux >= 3.3` if you want to run tmux integration tests

Clone and verify the workspace:

```sh
cargo check --workspace
```

## Common commands

Run the app:

```sh
cargo run -p termy
```

Useful `just` commands:

```sh
just
just check
just dev
just test-tmux-integration
just check-boundaries
```

## Docs and generated files

These files are generated. Do not edit them manually:

- `docs/keybindings.md`
- `docs/configuration.md`

Regenerate them with:

```sh
just generate-keybindings-doc
just generate-config-doc
```

Or verify they are up to date with:

```sh
just check-keybindings-doc
just check-config-doc
```

## Testing and validation

Use the smallest validation pass that proves your change. See [docs/engineering/testing.md](docs/engineering/testing.md) for the full test pyramid.

Common options:

```sh
cargo check -p termy
cargo test -p termy_core
just test-workspace          # all workspace tests (release)
just check-boundaries
just validate                # check + fmt + tests + boundaries + clippy (before large PRs)
```

If your change touches tmux behavior, also run:

```sh
just test-tmux-integration
```

Roadmaps:

- Engineering quality (CI, modularity, scorecard): [docs/engineering/roadmap.md](docs/engineering/roadmap.md)

## Config and command changes

If you change config keys:

- update the config schema in `crates/core/src/config_core`
- keep parsing, defaults, and rendering in sync
- regenerate config docs

If you change commands or keybind-facing actions:

- update the command catalog in `crates/core/src/command_core`
- wire the action through the app in `crates/desktop_app/src/`
- regenerate keybinding docs if defaults or public command names changed

## Documentation changes

Pick the right place:

- contributor-facing repo docs: `docs/`
- public user docs: `website/content/`
- quick project entry points: `README.md`

When behavior changes, update docs in the same PR.

## Pull requests

Before opening a PR:

- make sure the branch is up to date enough to review cleanly
- run the relevant checks for your change
- fill out `.github/PULL_REQUEST_TEMPLATE.md`
- link the issue with `Closes #<number>` when appropriate

Good PRs usually include:

- what changed
- why it changed
- any user-visible behavior changes
- screenshots or video for UI work
- notes about tests or intentionally skipped validation

## Style expectations

- Prefer small, explicit code paths over clever abstractions.
- Preserve existing project structure and naming where possible.
- Keep docs and behavior aligned.
- Do not add `#[allow(clippy::cognitive_complexity)]` unless the same line links the tracking issue.
- Avoid unrelated drive-by changes in the same PR.

## Questions

If the right direction is unclear, open an issue first or start with a smaller draft PR that frames the change.
