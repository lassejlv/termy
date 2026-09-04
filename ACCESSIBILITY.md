# Accessibility contract

Tmon 0.1 is an **accessibility preview**, not a VoiceOver-compatible terminal release. This is an
explicit support boundary, not an inference from the native macOS window chrome.

## Implemented baseline

- Every operation is keyboard reachable, including native tabs, search, copy/paste, font sizing,
  config access, and quitting.
- IME pre-edit text is visible at the terminal cursor or inside search, including UTF-8-safe
  selection/caret state; the macOS candidate window is anchored to the active insertion point.
- Startup, renderer, and multiplexer failures use native macOS alerts with a recovery action and do
  not rely solely on terminal output.
- Native AppKit windows and tab titles expose the active terminal title through standard window
  chrome.

## Not supported in 0.1

The Metal terminal canvas does not yet expose its rows, caret, selection, hyperlinks, search result,
or process-exit state as an `NSAccessibility` text tree. VoiceOver can navigate the native window
chrome and alerts, but Tmon does not claim that terminal contents or the custom search overlay are
readable or editable with VoiceOver. Full Keyboard Access is useful, but it is not a substitute for
that missing semantic tree.

Do not describe 0.1 as accessible or VoiceOver supported. A future supported tier must expose a
bounded visible-text model (never unrestricted scrollback by default), caret and selection ranges,
tab/search/status roles, actionable hyperlinks, and incremental change notifications. It must then
pass the packaged-app checks below with VoiceOver enabled.

## Release checks

- Launch the quarantined app using only the keyboard and reach every documented command.
- Use a Danish Option character and a CJK or Japanese input source; verify pre-edit, candidate
  placement, selection movement, commit, cancel, search composition, resize, and tab switching.
- With VoiceOver enabled, verify that the app name, window title, native tabs, and all failure alerts
  are announced; record the terminal canvas and search field as intentionally unsupported.
- Ensure release notes, the public compatibility matrix, and support replies repeat the preview
  limitation without implying content access.

The full semantic VoiceOver tree is a release-blocking requirement before Tmon claims an accessible
support tier. Until then, accessibility findings are accepted as production bugs only when they
regress the implemented keyboard, IME, native-window, or alert baseline above.
