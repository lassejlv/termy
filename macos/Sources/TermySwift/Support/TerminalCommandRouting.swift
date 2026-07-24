import AppKit
import Combine

enum TerminalHostCommand {
    case newTab
    case closePaneOrTab
    case splitPaneVertical
    case splitPaneHorizontal
    case closePane
    case focusPane(TerminalPaneDirection)
    case focusPaneNext
    case focusPanePrevious
    case resizePane(TerminalPaneDirection)
    case togglePaneZoom
    case increaseFontSize
    case decreaseFontSize
    case resetFontSize
    case copy
    case paste
    case openSearch
    case closeSearch
    case searchNext
    case searchPrevious
    case toggleSearchCaseSensitive
    case toggleSearchRegex
    case clearScrollback
    case sendInterrupt
    case toggleCommandPalette
}

/// The canonical keybind/command vocabulary, decoded from the action identifiers
/// the Rust `command_core` catalog emits in config keybinds. This is the single
/// source of truth for action identity (used by the keybind router and the
/// command palette); `TerminalHostCommand` remains the narrower set a focused
/// store can execute. `.unknown` preserves forward-compatibility with actions a
/// newer core defines.
enum TerminalKeybindAction: Equatable {
    case appInfo
    case restartApp
    case openConfig
    case prettifyConfig
    case toggleTabBarVisibility
    case moveTabLeft
    case moveTabRight
    case switchTabLeft
    case switchTabRight
    case newTab
    case closeTab
    case closePaneOrTab
    case closePane
    case minimizeWindow
    case quit
    case switchToTab(Int)
    case toggleCommandPalette
    case splitPaneVertical
    case splitPaneHorizontal
    case focusPaneNext
    case focusPanePrevious
    case focusPane(TerminalPaneDirection)
    case resizePane(TerminalPaneDirection)
    case togglePaneZoom
    case increaseFontSize
    case decreaseFontSize
    case resetFontSize
    case copy
    case paste
    case openSearch
    case closeSearch
    case searchNext
    case searchPrevious
    case toggleSearchCaseSensitive
    case toggleSearchRegex
    case clearScrollback
    case sendInterrupt
    case runTask
    case unknown(String)

    init(identifier: String) {
        switch identifier {
        case "app_info": self = .appInfo
        case "restart_app": self = .restartApp
        case "open_config": self = .openConfig
        case "prettify_config": self = .prettifyConfig
        case "toggle_tab_bar_visibility": self = .toggleTabBarVisibility
        case "move_tab_left": self = .moveTabLeft
        case "move_tab_right": self = .moveTabRight
        case "switch_tab_left": self = .switchTabLeft
        case "switch_tab_right", "cycle_tabs": self = .switchTabRight
        case "new_tab": self = .newTab
        case "close_tab": self = .closeTab
        case "close_pane_or_tab": self = .closePaneOrTab
        case "close_pane": self = .closePane
        case "minimize_window": self = .minimizeWindow
        case "quit": self = .quit
        case "toggle_command_palette": self = .toggleCommandPalette
        case "split_pane_vertical": self = .splitPaneVertical
        case "split_pane_horizontal": self = .splitPaneHorizontal
        case "focus_pane_next": self = .focusPaneNext
        case "focus_pane_previous": self = .focusPanePrevious
        case "focus_pane_left": self = .focusPane(.left)
        case "focus_pane_right": self = .focusPane(.right)
        case "focus_pane_up": self = .focusPane(.up)
        case "focus_pane_down": self = .focusPane(.down)
        case "resize_pane_left": self = .resizePane(.left)
        case "resize_pane_right": self = .resizePane(.right)
        case "resize_pane_up": self = .resizePane(.up)
        case "resize_pane_down": self = .resizePane(.down)
        case "toggle_pane_zoom": self = .togglePaneZoom
        case "increase_font_size": self = .increaseFontSize
        case "decrease_font_size": self = .decreaseFontSize
        case "reset_font_size": self = .resetFontSize
        case "copy": self = .copy
        case "paste": self = .paste
        case "open_search": self = .openSearch
        case "close_search": self = .closeSearch
        case "search_next": self = .searchNext
        case "search_previous": self = .searchPrevious
        case "toggle_search_case_sensitive": self = .toggleSearchCaseSensitive
        case "toggle_search_regex": self = .toggleSearchRegex
        case "clear_buffer": self = .clearScrollback
        case "send_interrupt": self = .sendInterrupt
        case "run_task": self = .runTask
        default:
            if identifier.hasPrefix("switch_to_tab_"),
               let number = Int(identifier.dropFirst("switch_to_tab_".count)), number >= 1 {
                self = .switchToTab(number)
            } else {
                self = .unknown(identifier)
            }
        }
    }

    var identifier: String {
        switch self {
        case .appInfo: return "app_info"
        case .restartApp: return "restart_app"
        case .openConfig: return "open_config"
        case .prettifyConfig: return "prettify_config"
        case .toggleTabBarVisibility: return "toggle_tab_bar_visibility"
        case .moveTabLeft: return "move_tab_left"
        case .moveTabRight: return "move_tab_right"
        case .switchTabLeft: return "switch_tab_left"
        case .switchTabRight: return "switch_tab_right"
        case .newTab: return "new_tab"
        case .closeTab: return "close_tab"
        case .closePaneOrTab: return "close_pane_or_tab"
        case .closePane: return "close_pane"
        case .minimizeWindow: return "minimize_window"
        case .quit: return "quit"
        case .switchToTab(let number): return "switch_to_tab_\(number)"
        case .toggleCommandPalette: return "toggle_command_palette"
        case .splitPaneVertical: return "split_pane_vertical"
        case .splitPaneHorizontal: return "split_pane_horizontal"
        case .focusPaneNext: return "focus_pane_next"
        case .focusPanePrevious: return "focus_pane_previous"
        case .focusPane(let direction): return "focus_pane_\(Self.suffix(direction))"
        case .resizePane(let direction): return "resize_pane_\(Self.suffix(direction))"
        case .togglePaneZoom: return "toggle_pane_zoom"
        case .increaseFontSize: return "increase_font_size"
        case .decreaseFontSize: return "decrease_font_size"
        case .resetFontSize: return "reset_font_size"
        case .copy: return "copy"
        case .paste: return "paste"
        case .openSearch: return "open_search"
        case .closeSearch: return "close_search"
        case .searchNext: return "search_next"
        case .searchPrevious: return "search_previous"
        case .toggleSearchCaseSensitive: return "toggle_search_case_sensitive"
        case .toggleSearchRegex: return "toggle_search_regex"
        case .clearScrollback: return "clear_buffer"
        case .sendInterrupt: return "send_interrupt"
        case .runTask: return "run_task"
        case .unknown(let identifier): return identifier
        }
    }

    private static func suffix(_ direction: TerminalPaneDirection) -> String {
        switch direction {
        case .left: return "left"
        case .right: return "right"
        case .up: return "up"
        case .down: return "down"
        }
    }
}

@MainActor
final class TerminalCommandRouter: ObservableObject {
    static let shared = TerminalCommandRouter()

    /// Whether any terminal store is reachable. Published so the app's menu
    /// commands re-evaluate when the first terminal window opens or the last
    /// one closes: terminal windows are AppKit-hosted, so SwiftUI sees neither
    /// them nor the `NSApp.keyWindow` lookup `focusedCommandSet()` performs.
    @Published private(set) var hasTerminalStore = false

    weak var activeStore: TerminalWorkspaceStore?
    private var storesByWindow: [ObjectIdentifier: WeakTerminalWorkspaceStore] = [:]

    func activate(_ store: TerminalWorkspaceStore) {
        activeStore = store
        refreshTerminalStoreAvailability()
    }

    func register(_ store: TerminalWorkspaceStore, for window: NSWindow) {
        storesByWindow[ObjectIdentifier(window)] = WeakTerminalWorkspaceStore(store)
        activate(store)
    }

    func unregister(window: NSWindow) {
        storesByWindow.removeValue(forKey: ObjectIdentifier(window))
        refreshTerminalStoreAvailability()
    }

    private var isTerminalStoreAvailable: Bool {
        activeStore != nil || storesByWindow.values.contains { $0.store != nil }
    }

    /// Deferred to the next main-actor turn: registration re-runs inside a
    /// SwiftUI update pass, where publishing a change is not allowed. The guard
    /// keeps the steady state (a re-register per view update) allocation-free.
    private func refreshTerminalStoreAvailability() {
        guard hasTerminalStore != isTerminalStoreAvailable else {
            return
        }
        Task { @MainActor in
            let available = self.isTerminalStoreAvailable
            guard self.hasTerminalStore != available else {
                return
            }
            self.hasTerminalStore = available
        }
    }

    /// The store hosted by a specific window, with no active-store fallback.
    /// Used to suspend/resume a window's panes on occlusion changes.
    func store(forWindow window: NSWindow) -> TerminalWorkspaceStore? {
        storesByWindow[ObjectIdentifier(window)]?.store
    }

    func closeFocusedPaneIfSplit(for event: NSEvent? = nil) -> Bool {
        store(for: event?.window ?? NSApp.keyWindow ?? NSApp.mainWindow)?
            .closeFocusedPaneIfSplit() ?? false
    }

    func splitFocused(_ axis: TerminalSplitAxis, for window: NSWindow? = nil) -> Bool {
        guard let store = store(for: window ?? NSApp.keyWindow ?? NSApp.mainWindow) else {
            return false
        }

        store.splitFocused(axis)
        return true
    }

    func focusedStore(for event: NSEvent? = nil) -> TerminalWorkspaceStore? {
        store(for: event?.window ?? NSApp.keyWindow ?? NSApp.mainWindow)
    }

    func focusedCommandSet(for window: NSWindow? = nil) -> TerminalCommandSet? {
        guard let store = store(for: window ?? NSApp.keyWindow ?? NSApp.mainWindow) else {
            return nil
        }
        return TerminalCommandSet(
            newTab: {
                NativeTabWindowManager.shared.openNativeTab()
            },
            closePaneOrTab: {
                if !store.closeFocusedPaneIfSplit() {
                    (window ?? NSApp.keyWindow)?.performClose(nil)
                }
            },
            splitRight: {
                store.splitFocused(.horizontal)
            },
            splitDown: {
                store.splitFocused(.vertical)
            },
            closePane: {
                store.closeFocusedPane()
            },
            focusPane: { direction in
                _ = store.focusPane(in: direction)
            },
            focusNextPane: store.focusNextPane,
            focusPreviousPane: store.focusPreviousPane,
            resizePane: { direction in
                _ = store.resizeFocusedPane(in: direction)
            },
            togglePaneZoom: store.toggleFocusedPaneZoom,
            increaseFontSize: {
                store.focusedTerminal?.increaseFontSize()
            },
            decreaseFontSize: {
                store.focusedTerminal?.decreaseFontSize()
            },
            resetFontSize: {
                store.focusedTerminal?.resetFontSize()
            },
            copy: {
                store.focusedTerminal?.copySelection() ?? false
            },
            paste: {
                guard let text = NSPasteboard.general.string(forType: .string) else {
                    return
                }
                store.focusedTerminal?.paste(text)
            },
            clearScrollback: {
                store.focusedTerminal?.clearScrollback()
            },
            showSearch: store.showSearch,
            hideSearch: store.hideSearch,
            searchNext: {
                store.focusedTerminal?.selectNextSearchMatch()
            },
            searchPrevious: {
                store.focusedTerminal?.selectPreviousSearchMatch()
            },
            toggleSearchCaseSensitive: {
                store.toggleSearchCaseSensitive()
            },
            toggleSearchRegex: {
                store.toggleSearchRegex()
            },
            sendInterrupt: {
                store.focusedTerminal?.sendControlC()
            },
            toggleCommandPalette: store.toggleCommandPalette
        )
    }

    func hasRunningTerminalProcess() -> Bool {
        cleanupReleasedStores()
        if activeStore?.hasRunningTerminalProcess == true {
            return true
        }
        return storesByWindow.values.contains { $0.store?.hasRunningTerminalProcess == true }
    }

    private func store(for window: NSWindow?) -> TerminalWorkspaceStore? {
        cleanupReleasedStores()
        guard let window else {
            return activeStore
        }
        return storesByWindow[ObjectIdentifier(window)]?.store ?? activeStore
    }

    private func cleanupReleasedStores() {
        storesByWindow = storesByWindow.filter { _, box in
            box.store != nil
        }
    }
}

private final class WeakTerminalWorkspaceStore {
    weak var store: TerminalWorkspaceStore?

    init(_ store: TerminalWorkspaceStore) {
        self.store = store
    }
}

struct TerminalCommandSet {
    var newTab: () -> Void = {}
    var closePaneOrTab: () -> Void = {}
    var splitRight: () -> Void
    var splitDown: () -> Void
    var closePane: () -> Void
    var focusPane: (TerminalPaneDirection) -> Void = { _ in }
    var focusNextPane: () -> Void
    var focusPreviousPane: () -> Void = {}
    var resizePane: (TerminalPaneDirection) -> Void = { _ in }
    var togglePaneZoom: () -> Void = {}
    var increaseFontSize: () -> Void = {}
    var decreaseFontSize: () -> Void = {}
    var resetFontSize: () -> Void = {}
    var copy: () -> Bool = { false }
    var paste: () -> Void = {}
    var clearScrollback: () -> Void = {}
    var showSearch: () -> Void
    var hideSearch: () -> Void
    var searchNext: () -> Void = {}
    var searchPrevious: () -> Void = {}
    var toggleSearchCaseSensitive: () -> Void = {}
    var toggleSearchRegex: () -> Void = {}
    var sendInterrupt: () -> Void
    var toggleCommandPalette: () -> Void = {}

    func execute(_ command: TerminalHostCommand) {
        switch command {
        case .newTab:
            newTab()
        case .closePaneOrTab:
            closePaneOrTab()
        case .splitPaneVertical:
            splitRight()
        case .splitPaneHorizontal:
            splitDown()
        case .closePane:
            closePane()
        case .focusPane(let direction):
            focusPane(direction)
        case .focusPaneNext:
            focusNextPane()
        case .focusPanePrevious:
            focusPreviousPane()
        case .resizePane(let direction):
            resizePane(direction)
        case .togglePaneZoom:
            togglePaneZoom()
        case .increaseFontSize:
            increaseFontSize()
        case .decreaseFontSize:
            decreaseFontSize()
        case .resetFontSize:
            resetFontSize()
        case .copy:
            _ = copy()
        case .paste:
            paste()
        case .openSearch:
            showSearch()
        case .closeSearch:
            hideSearch()
        case .searchNext:
            searchNext()
        case .searchPrevious:
            searchPrevious()
        case .toggleSearchCaseSensitive:
            toggleSearchCaseSensitive()
        case .toggleSearchRegex:
            toggleSearchRegex()
        case .clearScrollback:
            clearScrollback()
        case .sendInterrupt:
            sendInterrupt()
        case .toggleCommandPalette:
            toggleCommandPalette()
        }
    }
}
