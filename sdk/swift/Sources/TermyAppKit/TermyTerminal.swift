#if os(macOS)
import SwiftUI
import Termy

/// A SwiftUI terminal backed by `TerminalView`.
///
/// ```swift
/// TermyTerminal(view: try TerminalView())
///     .onTitleChange { title = $0 ?? "Terminal" }
/// ```
///
/// Create the `TerminalView` once (for example in a `@State` model) and pass
/// it in; SwiftUI may rebuild this wrapper at any time, but the view and its
/// shell session must outlive those rebuilds.
public struct TermyTerminal: NSViewRepresentable {
    private let view: TerminalView
    private var onTitleChange: ((String?) -> Void)?
    private var onEvent: ((TerminalEvent) -> Void)?
    private var onExit: (() -> Void)?
    private var shouldOpenURL: ((URL) -> Bool)?
    private var shouldWriteClipboard: ((String) -> Bool)?

    public init(view: TerminalView) {
        self.view = view
    }

    public func onTitleChange(_ action: @escaping (String?) -> Void) -> TermyTerminal {
        var copy = self
        copy.onTitleChange = action
        return copy
    }

    public func onEvent(_ action: @escaping (TerminalEvent) -> Void) -> TermyTerminal {
        var copy = self
        copy.onEvent = action
        return copy
    }

    public func onExit(_ action: @escaping () -> Void) -> TermyTerminal {
        var copy = self
        copy.onExit = action
        return copy
    }

    /// Decides whether a Command-clicked link opens. Allowed schemes open by default.
    public func shouldOpenURL(_ decide: @escaping (URL) -> Bool) -> TermyTerminal {
        var copy = self
        copy.shouldOpenURL = decide
        return copy
    }

    /// Decides whether a program may replace the clipboard with OSC 52.
    /// Writes are denied by default; see `TerminalViewDelegate`.
    public func onClipboardWrite(_ decide: @escaping (String) -> Bool) -> TermyTerminal {
        var copy = self
        copy.shouldWriteClipboard = decide
        return copy
    }

    public func makeCoordinator() -> Coordinator {
        Coordinator()
    }

    public func makeNSView(context: Context) -> TerminalView {
        view.delegate = context.coordinator
        DispatchQueue.main.async { view.window?.makeFirstResponder(view) }
        return view
    }

    public func updateNSView(_ view: TerminalView, context: Context) {
        context.coordinator.onTitleChange = onTitleChange
        context.coordinator.onEvent = onEvent
        context.coordinator.onExit = onExit
        context.coordinator.shouldOpenURL = shouldOpenURL
        context.coordinator.shouldWriteClipboard = shouldWriteClipboard
        view.delegate = context.coordinator
    }

    @MainActor
    public final class Coordinator: TerminalViewDelegate {
        var onTitleChange: ((String?) -> Void)?
        var onEvent: ((TerminalEvent) -> Void)?
        var onExit: (() -> Void)?
        var shouldOpenURL: ((URL) -> Bool)?
        var shouldWriteClipboard: ((String) -> Bool)?

        public func terminalView(_ view: TerminalView, didChangeTitle title: String?) {
            onTitleChange?(title)
        }

        public func terminalView(_ view: TerminalView, didReceive event: TerminalEvent) {
            onEvent?(event)
        }

        public func terminalView(_ view: TerminalView, shouldOpen url: URL) -> Bool {
            shouldOpenURL?(url) ?? true
        }

        public func terminalView(_ view: TerminalView, shouldWriteClipboard text: String) -> Bool {
            shouldWriteClipboard?(text) ?? false
        }

        public func terminalViewDidExit(_ view: TerminalView) {
            onExit?()
        }
    }
}
#endif
