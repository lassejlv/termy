# Native macOS implementation status

Status date: 2026-07-10

The native SwiftUI/AppKit host is in **public beta**. This document records
the implementation milestones that are already complete. It is not the active
release checklist.

For production blockers, exit gates, and delivery order, use
[`road.md`](road.md).

## Completed foundation

- The terminal grid uses a retained AppKit renderer with damage-scoped frame
  updates, cached row render plans, display-synced presentation, and native
  render metrics.
- Keyboard encoding, mouse encoding, bracketed paste, search, selection,
  scrollback, hyperlinks, shell events, and configuration remain owned by
  `termy_core` and are consumed through `termy_ffi`.
- Native tabs, pane splits, pane focus, persistence, settings, configuration
  diagnostics, deeplinks, onboarding, theme installation, and the CLI install
  action are implemented.
- IME composition, inline marked text, VoiceOver value/selection/cursor
  reporting, command marks, search markers, bell feedback, and native context
  menus are implemented. Live interaction matrices remain release QA work.

## tmux control mode

The native GUI path is implemented. The live workspace now:

- launches `TmuxControlWorkspaceModel` when tmux is enabled;
- recursively renders horizontal and vertical control-mode layouts;
- reconciles pane output into display-only libtermy terminals;
- tracks and changes focused panes;
- routes bytes, encoded keys, mouse input, and paste to the target tmux pane;
- exposes search on the focused pane;
- wires split-right, split-down, close-pane, and focus-next commands.

`Support/TmuxIntegration.swift` remains intentionally available as the
shell-backed fallback when control mode cannot launch. Production work is now
real-GUI interaction coverage and failure-path testing, tracked by Task 5 in
[`road.md`](road.md), rather than missing GUI implementation.

## Performance status

The native benchmark now covers 10 deterministic scenarios with enforced
minimum sample counts, render-plan percentile budgets, launch/idle/resource
sampling, deliberate regression fixtures, and attached windowed xctrace
comparisons against the GPUI build. The implementation and local gates are
complete; Task 4 in [`road.md`](road.md) remains open only until the committed
candidate records a green, non-cancelled performance workflow run.

## Non-blocking backlog

These items are optional polish and do not block a native release:

- structured keybinding editor;
- broader command-palette coverage;
- theme-registry caching and richer offline behavior;
- richer updater progress UX;
- optional GPU renderer, only if measured AppKit performance misses the release
  budgets.
