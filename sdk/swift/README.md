# Termy Swift SDK

Swift bindings for libtermy, the Termy terminal engine, built on its C ABI
([`crates/core/include/termy.h`](../../crates/core/include/termy.h)).

| Product | What it gives you |
| --- | --- |
| `Termy` | `Terminal`, `TerminalSession`, frames, input encoding, events, config, search, Kitty graphics and clipboard, tmux control mode. No UI dependency. |
| `TermyAppKit` | `TerminalView` (AppKit) and `TermyTerminal` (SwiftUI): Metal rendering, keyboard with IME, mouse reporting, selection, scrollback, OSC 8 links, Kitty graphics. |

Requires macOS 13+, Swift 6.2+ and a Rust toolchain to build libtermy.

## Build and test

libtermy is built by Cargo, not SwiftPM. Stage it in `lib/` first:

```sh
scripts/build-libtermy.sh          # release; pass `debug` for a debug build
swift test
swift run TermyHeadless            # parse sample output, no process
swift run TermyHeadless ls -la     # run a command in a PTY, print the screen
swift run TermyDemo                # a SwiftUI terminal window
```

From the repository root, `just test-swift-sdk` does the build and test steps.
Set `TERMY_LIBRARY_PATH` to link a libtermy from another directory.

## Use it in an app

The package links libtermy with unsafe linker flags, so SwiftPM accepts it only
as a root or local path dependency. SwiftPM names a path dependency after its
directory, so the package is `swift`:

```swift
.package(path: "../termy/sdk/swift"),
// ...
.product(name: "TermyAppKit", package: "swift"),
```

Bundle `libtermy_core.dylib` in `Contents/Frameworks`. Its install name is
`@rpath/libtermy_core.dylib`, so add `@executable_path/../Frameworks` to the
app's runpath search paths.

### SwiftUI

```swift
import SwiftUI
import TermyAppKit

@MainActor
final class Model: ObservableObject {
    let view = try! TerminalView()     // the user's Termy config and shell
    @Published var title = "Terminal"
}

struct ContentView: View {
    @StateObject private var model = Model()

    var body: some View {
        TermyTerminal(view: model.view)
            .onTitleChange { model.title = $0 ?? "Terminal" }
            .navigationTitle(model.title)
    }
}
```

Keep the `TerminalView` in a model, not in `body`: SwiftUI rebuilds the wrapper
often, and the view owns the shell.

### Headless

```swift
import Termy

@MainActor
func run() async throws {
    let session = try TerminalSession(
        size: TerminalSize(cols: 120, rows: 40),
        options: TerminalOptions(startupCommand: "make test")
    )
    defer { session.stop() }
    session.onEvent = { if case .title(let title) = $0 { print(title) } }

    while !session.hasExited { try await Task.sleep(for: .milliseconds(20)) }
    session.poll()
    for row in 0..<session.frame.rows { print(session.frame.text(row: row)) }
}
```

### Your own transport

A display terminal parses output but has no process (SSH, tmux, recordings).
Feed it output, send its input and protocol replies back to the program:

```swift
let session = try TerminalSession.display(size: TerminalSize(cols: 80, rows: 24))
session.terminal.onInput = { bytes in channel.write(bytes) }   // keys, mouse, pastes
session.onProtocolReply = { bytes in channel.write(bytes) }    // cursor reports, DA, ...
channel.onOutput = { bytes in try? session.feed(bytes) }
```

### Programs that outlive the app

`Multiplexer` connects to a Termy session host, the one behind `termy mux`,
which runs programs outside your process: they keep running when the app
quits, and the next launch attaches to them again. Its terminals are ordinary
`Terminal`s with a `paneID`; releasing one detaches.

```swift
let host = try Multiplexer.connect(root: myAppSupportDir, hostExecutable: Multiplexer.bundledHostExecutable!)
let terminal = try host.createTerminal(size: size, options: TerminalOptions(config: config))
save(terminal.paneID!)
let view = TerminalView(session: TerminalSession(terminal: terminal, size: size), renderConfig: renderConfig)

// After a relaunch:
let again = try host.attachTerminal(paneID: savedID)   // throws .notFound when it ended
```

`scripts/build-libtermy.sh` stages `termy-session-host` next to the library;
bundle it in `Contents/Helpers`. Calls block on the host (starting one takes
seconds), so connect off the main thread. Pass your own `root` to keep your
panes apart from Termy's; it must not be readable by other users.

### Sharing the layout with `termy mux`

The host keeps a window, workspace, tab and split layout that `termy mux`
reads and edits. An app that shows the host's panes can follow it:

```swift
var layout = try mux.layout()               // windows → workspaces → tabs → split trees of pane IDs
let watch = try mux.watchLayout { revision in /* read and apply mux.layout() */ }
layout.windows[0].workspaces[0].tabs[0].title = "build"
let (written, revision) = try mux.setLayout(layout)  // false when another client wrote first
```

### A config of your own

`TermySettings.set(_:to:)` edits the Termy app's own config. An app that
keeps a separate config passes its path instead, and lists the theme library
from that config, which includes local themes in the `themes` folder beside it:

```swift
let path = "/Users/me/.config/my-app/config.txt"
try TermySettings.set("theme", to: "catppuccin-mocha", inConfigAt: path)
let config = try TermyConfig(path: path)
for theme in try config.themes() {
    print(theme.name, theme.swatches)   // background, foreground, then accents
}
```

## Untrusted output

Anything a program prints can reach `TerminalView`, including `cat` of a
downloaded file or output from a remote host, so the view's defaults are
conservative:

- **Clipboard writes (OSC 52)** are denied unless the delegate's
  `terminalView(_:shouldWriteClipboard:)` returns true (`.onClipboardWrite` in
  SwiftUI). The `.clipboardStore` event still reaches `didReceive`.
- **Links (OSC 8)** show text the program chooses, so the target appears as a
  tooltip on hover. Command-click opens only `http`, `https` and `mailto` by
  default (`allowedLinkSchemes`), after `terminalView(_:shouldOpen:)`
  (`.shouldOpenURL` in SwiftUI). If you allow `file`, apps, packages, scripts
  and executables are revealed in Finder instead of opened.
- **Pasted files** are typed as shell-quoted paths.

## Rendering

`TerminalView` renders with Metal. Each cell becomes instanced quads: solid
backgrounds and decorations, and glyphs from a texture atlas that CoreText
rasterizes on first use. Quads are cached per row and rebuilt only for rows
that changed, so cursor blinks and selection drags reuse the whole grid.
Box drawing, blocks and Braille use libtermy's glyph geometry on both paths.

Without a Metal device the view falls back to CoreGraphics. Pass
`backend: .coreGraphics` to force it, and read `renderingBackend` to see which
one is active.

| File | Role |
| --- | --- |
| `Metal/MetalTerminalRenderer.swift` | Row cache, frame assembly, draw order, frame pacing |
| `Metal/GlyphAtlas.swift` | Shelf-packed BGRA atlas and CoreText rasterization |
| `Metal/MetalShaders.swift` | The quad instance layout and the shader source |
| `TerminalFonts.swift` | Font styles and fallback lookup shared by both renderers |
| `TerminalRenderer.swift` | The CoreGraphics fallback, also used for `cacheDisplay` |

A full 200×60 frame at 2× takes about 1.2 ms of main-thread CPU on Apple
silicon when every row is rebuilt, and 0.1 ms when none changed, against
5.6 ms for CoreGraphics. The
[Swift Metal renderer guide](https://termy.sh/docs/developer/libtermy/swift-rendering)
covers the pipeline in detail.

## Threading

`Terminal` is not thread-safe; confine it to one thread. `TerminalSession` and
`TerminalView` run on the main actor. A PTY terminal starts one background
thread that waits for output and hops to the main actor to poll. `stop()`, or
releasing the session, wakes and joins it before the handle is freed.

## Layout

| Path | Contents |
| --- | --- |
| `Sources/CTermy` | Module map over the repository's `termy.h`, so bindings can't drift from the ABI. |
| `Sources/Termy` | The Swift API. Every C allocation is copied and freed inside the call that returns it. |
| `Sources/TermyAppKit` | AppKit/SwiftUI views and the CoreGraphics fallback renderer. |
| `Sources/TermyAppKit/Metal` | The Metal renderer, glyph atlas and shaders. |
| `Examples` | `TermyDemo` (SwiftUI app) and `TermyHeadless` (CLI). |
| `Tests` | Swift Testing suites, including a real PTY round trip. |

See the [libtermy guide](https://termy.sh/docs/developer/libtermy) for the
engine's rendering, input and event contracts.
