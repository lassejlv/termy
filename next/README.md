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
