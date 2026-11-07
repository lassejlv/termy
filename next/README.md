# Termy Next

A session-first macOS terminal in pure Swift, modelled on Superlogical's Rex
and built on libtermy through the [Swift SDK](../sdk/swift). It reads your
Termy config, so theme, font, padding and shell match Termy.

## Run

```sh
just next                 # build libtermy, then swift run (debug)
just bundle-next          # next/build/Termy Next.app (release)
just test-next            # layout, matching and naming tests
```

Without `just`: `../sdk/swift/scripts/build-libtermy.sh && swift run TermyNext`.

## Sessions outlive the app

Termy Next runs its shells in a Termy session host (the multiplexer behind
`termy mux`), in its own directory
`~/Library/Application Support/TermyNext/multiplexer`. Quitting detaches:
every program keeps running, and the next launch reattaches to them in the
same windows, sessions, tabs and splits. Closing a pane, tab or session ends
its programs; closing a window only detaches its session, which stays in ⌘K.
The session host can be turned off in Settings (new panes then run in the
app) and restarted from there or the app menu.

## termy mux

The host's shared layout is Termy Next's layout: each window is a host
window, each session a workspace, and tabs, splits, titles, focus and font
zoom match. Changes in the app are written to the host, and `termy mux`
changes show up live:

```sh
termy mux layout                         # what is on screen
termy mux create --window <id>           # a new tab in that window
termy mux split <pane> --axis vertical   # split a pane
termy mux tab <pane> rename build        # rename its tab
termy mux window <id> select-workspace 1 # switch the window's session
termy mux send <pane> "make" --enter     # type into a pane
termy mux close <pane>                   # end a pane
```

Inside a Termy Next pane `termy mux` finds the host through `TERMY_MUX_DIR`;
elsewhere pass `--session-dir ~/Library/Application\ Support/TermyNext/multiplexer`.
Floating panes are stored as splits and float again when Termy Next shows
them. Panes running in the app (session host off) are not in the layout.

## Model

| Rex | Termy Next | Notes |
| --- | --- | --- |
| Session | `TerminalSessionGroup` | Named group of tabs, such as `shy-orchard`. One per window; ⌘K switches. Closing a window detaches its session, which keeps running. |
| Window | `TerminalTab` | A tab: a split tree of panes, plus floating panes. |
| Block | `PaneView` | A rounded card with a header and a libtermy `TerminalView`. |

Pane headers and tabs name the program in front and its directory
(`zsh ~/Dev/termy`, `nvim ~/notes`). That comes from the kernel (the shell's
foreground process group) and from OSC 7. Agents wear their own logo and name
("Claude Code", "Codex"), using the icon of the matching app installed on this
Mac (Claude, ChatGPT/Codex, Cursor, Zed…); no logos ship with Termy Next. A tab
with several programs stacks up to three of their icons, the one reporting
status in front.

Activity comes only from
[OSC 7501](https://mitchellh.com/writing/program-status-osc7501), the
Program Status Protocol, which libtermy parses; nothing is guessed from the
screen or the process tree.

- **Working:** the tab ends in Rex's three-dot loader and shows the
  program's reported `title`. Hovering the tab swaps the dots for its close
  button.
- **Blocked, done, error:** a blue, green or red dot on the tab icon. `done`
  clears once you've looked at the pane.
- **Activity popup (⌘. or the eye button):** every reporting program across
  sessions, with its kind ("Needs permission") and `msg`. Click a row to jump
  there.

## zsh plugins

Off by default; turn on **Shell plugins** in Settings → General. New panes
then start zsh with a small plugin (`Sources/TermyNext/ShellScripts.swift`),
loaded through a ZDOTDIR of ours that puts yours back first. It reports the
command line over a Unix socket as you type, and the app draws native UI at
the cursor:

- **Completions:** history entries and commands that start with what you
  typed. ↓ / ↑ or ⌃N / ⌃P pick one, Return or Tab puts it on the line, Esc
  hides the list. With **Pick completions with ↑ and ↓** off, the arrows stay
  with the shell's history until ⌃N / ⌃P picks a row.
- **Command completions (experimental, off by default):** a second checkbox
  under Shell plugins. The list then also offers what can come next for
  git, brew, cargo, rustup, rustc, bun, bunx, npm, pnpm, npx, just, ssh,
  claude, codex and file commands (rm, mkdir, cd, cp, mv, zed, code, open…):
  subcommands and options with a short description, and the project's own
  branches, stashes, changed files, installed formulae, Cargo binaries,
  package.json scripts, node_modules tools, just recipes, ssh hosts, and
  Claude Code and Codex sessions to resume. The specs are
  Termy Next's own (`Sources/TermyNextCore/CommandSpecs.swift`); generators
  run in the pane's directory and are cached for a few seconds.
- **Command box (⇧⌘E):** describe a command and Claude Code (`claude -p`) or
  Codex (`codex exec`) writes it onto the line, with the sign-in they already
  have; no API keys of our own. Return runs it, a follow-up refines it, Esc
  leaves it on the line to edit.

The plugin does nothing when Termy Next isn't listening, and a closed socket
can't end the shell. The protocol is in `ShellProtocol.swift`.

## Tasks

Termy's tasks work here too. Define them in Termy Next's config file:

```txt
task.build.command = cargo build
task.build.working_dir = crates/cli
task.deploy.layout = shy-orchard
task.deploy.command = ./deploy.sh
```

Run Task… (File menu or the command palette) lists them, and the palette
also matches them by name. A task opens a new tab and types its command
into the shell, which stays open afterwards. `working_dir` may use `~`; a
relative path starts from the focused pane's directory. `layout` limits a
task to the session with that name. `task.<name>.keybind` is not used yet.

## SSH

`ssh` typed in zsh and Add Remote Host send `COLORTERM` along (Settings →
General → SSH, on by default), so Claude Code and other programs on the server
draw 24-bit color instead of guessing 256 or 16 colors from `TERM`. The server
must accept it (`AcceptEnv COLORTERM`, as Debian and Ubuntu ship). Copying
over SSH goes through OSC 52, which Termy Next accepts from the pane you're
working in. Shift- or Option-drag selects text locally inside programs that
track the mouse.

## Shortcuts

These are Rex's defaults.

| Action | Keys |
| --- | --- |
| Command palette | ⇧⌘P |
| Change session | ⌘K |
| New session | ⇧⌘N |
| New tab | ⌘T |
| New window | ⌘N |
| Go to directory | ⇧⌘G |
| Split right / down | ⌘D / ⇧⌘D |
| Split left / up | ⇧⌘← / ⇧⌘↑ |
| Split best fit | ⌘↩ |
| Float pane | ⌥⌘↩ |
| Zoom pane | ⇧⌘↩ |
| Focus pane | ⌥⌘ + arrows |
| Previous / next pane | ⌘[ / ⌘] |
| Move pane | ⌥⇧⌘ + arrows |
| Next / previous tab | ⌃⇥ / ⌃⇧⇥ |
| Show tab 1–8 / last tab | ⌘1…⌘8 / ⌘9 |
| Move tab | ⌃⇧⇞ / ⌃⇧⇟ |
| Close pane / tab / window | ⌘W / ⌥⌘W / ⇧⌘W |
| Toggle sidebar | ⌘/ |
| Toggle activity | ⌘. |
| Font size | ⌘+ / ⌘− / ⌘0 |
| Clear scrollback | ⌥⌘K |
| Insert path | ⌃⌘I |

⌘← / ⌘→ / ⌘⌫ move to the start or end of the shell line, or delete it.

View › Customize View sets Tab Style (Above, Left Side, Right Side), Pane
Style (Comfortable, Compact) and Interface Style:

- **Liquid** (default) uses Liquid Glass (`NSGlassEffectView`, macOS 26+)
  where Rex does: the panes and the
  selected-tab pill, over a blurred window backdrop. Titlebar buttons, the
  tab track, the sidebar and pickers stay flat. Before macOS 26 it falls back
  to a blurred material.
- **Classic** draws solid chrome from the terminal theme.

## Motion

Splits grow in while their siblings slide over, closed panes shrink away, zoom
and float glide, the selected-tab pill slides between tabs, switching tabs or
sessions crossfades, the sidebar slides in and out, and pickers and the
Activity popup pop in. Terminal grids resize once at the start of a move instead of
on every frame. Reduce Motion in System Settings turns all of it off.

## Layout

| Path | Role |
| --- | --- |
| `Sources/TermyNextCore` | Split tree and navigation, fuzzy matching, names and pane labels, process lookup. No AppKit or libtermy. |
| `Sources/TermyNext/WindowController.swift` | One window: chrome layout, sessions, tabs, panes, commands, pickers. |
| `Sources/TermyNext/PaneView.swift` | Pane card, header and activity state. |
| `Sources/TermyNext/SplitContainerView.swift` | Pane frames, divider drags and floating panes. |
| `Sources/TermyNext/TabStripView.swift`, `SidebarView.swift` | Titlebar tabs and the sidebar. |
| `Sources/TermyNext/PickerView.swift` | The palette, session picker and prompts. |
| `Sources/TermyNext/Glass.swift` | `GlassView` (Liquid Glass with fallback), `GlassPanel`, and `Motion` timing and transitions. |
| `Sources/TermyNext/Commands.swift` | Every command with its title and shortcut; menus and the palette are built from it. |

## Releasing

Termy Next releases on its own tags, `next-v*`, separate from Termy's.
Push a tag, or run the **Release Termy Next** workflow with a version:

```sh
git tag next-v0.1.0 && git push origin next-v0.1.0
```

`.github/workflows/release-next.yml` builds a universal app (Apple Silicon
and Intel), signs it with Termy's Developer ID, notarizes and staples it, and
publishes a GitHub release with `TermyNext-<version>-macos-universal.dmg`, a
zip of the app and checksums. Releases are pre-releases unless the workflow
run says otherwise, and never become the repository's "Latest", which stays
Termy's. Termy's own release workflows skip `next-*` tags, and this one
never runs for Termy's.

### Updates

Release builds update themselves with [Sparkle](https://sparkle-project.org).
They read `appcast-universal.xml` from the `next-appcast` release, which each
release run replaces with a feed that lists the version it just published.
The DMG is EdDSA-signed with the same `SPARKLE_ED_PRIVATE_KEY` secret as
Termy's releases. Sparkle checks every hour, downloads new versions in the
background and installs them on quit; **Termy Next → Restart to Update**
installs one now. The first launch of a new version checks again right away,
so an update downloaded before a newer release doesn't leave you a version
behind. **Termy Next → Check for Updates…** (also in the command palette)
reads the feed right away, and **Settings → General → Updates** turns
automatic checks and downloads on or off.
Development builds and `scripts/bundle.sh` bundles have no feed and never
update.

To try the release build locally, signed ad hoc and without notarization:

```sh
TERMY_SIGN_IDENTITY=- next/scripts/release.sh --version 0.1.0 --native
```


## Mobile access

Open **Settings → Mobile**, enable mobile access, and choose **Pair iPhone…**.
Scan the QR in the Termy iPhone app, confirm the computer/relay there, and
approve the phone on your Mac. No account or terminal command is needed.
Paired phones can browse, view, create, control and end this Mac’s sessions.
Remove a phone’s access in the same settings pane.

Termy Next must be running, with its session host enabled. Closing all windows
keeps mobile access available; quitting or disabling it stops remote access
without ending hosted programs. Your Mac must be awake and reachable.

The default relay is `https://relay.termy.sh`. To use your own VPS, deploy the
[relay stack](../crates/relay/README.md#self-host-on-a-vps), enter its HTTPS URL
in **Mobile → Relay Server**, click **Apply**, and pair again. The QR configures
the phone’s relay automatically. Sessions remain end-to-end encrypted.

The reusable host is the `TermyRemoteHost` library product; pairing and
transport live in `../remote`. The old `termy remote` executable flow is retired.
