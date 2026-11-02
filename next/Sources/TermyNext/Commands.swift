import AppKit
import TermyNextCore

/// Every action the app offers. Menus, the command palette and shortcuts all
/// go through this list, so each one has a single title and key binding.
enum Command: String, CaseIterable {
    case settings, checkForUpdates, restartSessionHost, commandPalette, changeSession, renameSession, renameTab, addRemoteHost
    case newSession, newTab, goToDirectory, newWindow
    case previousTab, nextTab, moveTabLeft, moveTabRight, moveTabToNewWindow
    case closePane, closeTab, closeSession, closeWindow
    case splitRight, splitDown, splitLeft, splitUp, splitBestFit
    case floatPane, zoomPane, equalizePanes
    case focusLeft, focusRight, focusUp, focusDown, focusPreviousPane, focusNextPane
    case swapLeft, swapRight, swapUp, swapDown
    case toggleSidebar, toggleActivity
    case tabsAbove, tabsLeft, tabsRight, paneComfortable, paneCompact, interfaceLiquid, interfaceClassic
    case increaseFontSize, decreaseFontSize, resetFontSize
    case clearScrollback, insertPath, find, findNext, findPrevious
    case selectTab1, selectTab2, selectTab3, selectTab4, selectTab5, selectTab6, selectTab7, selectTab8, selectLastTab

    var title: String {
        switch self {
        case .settings: "Settings…"
        case .checkForUpdates: "Check for Updates…"
        case .restartSessionHost: "Restart Session Host…"
        case .commandPalette: "Command Palette…"
        case .changeSession: "Change Session…"
        case .renameSession: "Rename Session"
        case .renameTab: "Rename Tab"
        case .addRemoteHost: "Add Remote Host…"
        case .newSession: "New Session"
        case .newTab: "New Tab"
        case .goToDirectory: "Go to Directory…"
        case .newWindow: "New Window"
        case .previousTab: "Show Previous Tab"
        case .nextTab: "Show Next Tab"
        case .moveTabLeft: "Move Tab Left"
        case .moveTabRight: "Move Tab Right"
        case .moveTabToNewWindow: "Move Tab to New Window"
        case .closePane: "Close Pane"
        case .closeTab: "Close Tab"
        case .closeSession: "Close Session"
        case .closeWindow: "Close Window"
        case .splitRight: "Split Right"
        case .splitDown: "Split Down"
        case .splitLeft: "Split Left"
        case .splitUp: "Split Up"
        case .splitBestFit: "Split Best Fit"
        case .floatPane: "Float Pane"
        case .zoomPane: "Zoom Pane"
        case .equalizePanes: "Balance Panes"
        case .focusLeft: "Focus Left"
        case .focusRight: "Focus Right"
        case .focusUp: "Focus Up"
        case .focusDown: "Focus Down"
        case .focusPreviousPane: "Focus Previous Pane"
        case .focusNextPane: "Focus Next Pane"
        case .swapLeft: "Move Pane Left"
        case .swapRight: "Move Pane Right"
        case .swapUp: "Move Pane Up"
        case .swapDown: "Move Pane Down"
        case .toggleSidebar: "Toggle Sidebar"
        case .toggleActivity: "Toggle Activity"
        case .tabsAbove: "Tab Style: Above"
        case .tabsLeft: "Tab Style: Left Side"
        case .tabsRight: "Tab Style: Right Side"
        case .paneComfortable: "Pane Style: Comfortable"
        case .paneCompact: "Pane Style: Compact"
        case .interfaceLiquid: "Interface Style: Liquid"
        case .interfaceClassic: "Interface Style: Classic"
        case .increaseFontSize: "Increase Font Size"
        case .decreaseFontSize: "Decrease Font Size"
        case .resetFontSize: "Reset Font Size"
        case .clearScrollback: "Clear Scrollback"
        case .find: "Find…"
        case .findNext: "Find Next"
        case .findPrevious: "Find Previous"
        case .insertPath: "Insert Path…"
        case .selectTab1, .selectTab2, .selectTab3, .selectTab4, .selectTab5, .selectTab6, .selectTab7, .selectTab8:
            "Show Tab \(tabNumber!)"
        case .selectLastTab: "Show Last Tab"
        }
    }

    /// Short menu titles where the palette title carries a prefix.
    var menuTitle: String {
        switch self {
        case .tabsAbove: "Above"
        case .tabsLeft: "Left Side"
        case .tabsRight: "Right Side"
        case .paneComfortable: "Comfortable"
        case .paneCompact: "Compact"
        case .interfaceLiquid: "Liquid"
        case .interfaceClassic: "Classic"
        default: title
        }
    }

    var tabNumber: Int? {
        switch self {
        case .selectTab1: 1
        case .selectTab2: 2
        case .selectTab3: 3
        case .selectTab4: 4
        case .selectTab5: 5
        case .selectTab6: 6
        case .selectTab7: 7
        case .selectTab8: 8
        default: nil
        }
    }

    /// Commands that open a second step in the palette instead of running at once.
    var opensSubpicker: Bool {
        [.changeSession, .renameSession, .renameTab, .addRemoteHost, .goToDirectory, .insertPath].contains(self)
    }

    /// Hidden from the palette: they only make sense as shortcuts.
    var showsInPalette: Bool { tabNumber == nil && self != .selectLastTab && self != .commandPalette }

    // Rex's default bindings (see the Rex Beta menus).
    var shortcut: (key: String, modifiers: NSEvent.ModifierFlags)? {
        let up = String(UnicodeScalar(NSUpArrowFunctionKey)!)
        let down = String(UnicodeScalar(NSDownArrowFunctionKey)!)
        let left = String(UnicodeScalar(NSLeftArrowFunctionKey)!)
        let right = String(UnicodeScalar(NSRightArrowFunctionKey)!)
        let pageUp = String(UnicodeScalar(NSPageUpFunctionKey)!)
        let pageDown = String(UnicodeScalar(NSPageDownFunctionKey)!)
        switch self {
        case .commandPalette: return ("p", [.command, .shift])
        case .changeSession: return ("k", .command)
        case .newSession: return ("n", [.command, .shift])
        case .newTab: return ("t", .command)
        case .goToDirectory: return ("g", [.command, .shift])
        case .newWindow: return ("n", .command)
        case .previousTab: return ("\t", [.control, .shift])
        case .nextTab: return ("\t", .control)
        case .moveTabLeft: return (pageUp, [.control, .shift])
        case .moveTabRight: return (pageDown, [.control, .shift])
        case .closePane: return ("w", .command)
        case .closeTab: return ("w", [.command, .option])
        case .closeWindow: return ("w", [.command, .shift])
        case .splitRight: return ("d", .command)
        case .splitDown: return ("d", [.command, .shift])
        case .splitLeft: return (left, [.command, .shift])
        case .splitUp: return (up, [.command, .shift])
        case .splitBestFit: return ("\r", .command)
        case .floatPane: return ("\r", [.command, .option])
        case .zoomPane: return ("\r", [.command, .shift])
        case .focusLeft: return (left, [.command, .option])
        case .focusRight: return (right, [.command, .option])
        case .focusUp: return (up, [.command, .option])
        case .focusDown: return (down, [.command, .option])
        case .focusPreviousPane: return ("[", .command)
        case .focusNextPane: return ("]", .command)
        case .swapLeft: return (left, [.command, .option, .shift])
        case .swapRight: return (right, [.command, .option, .shift])
        case .swapUp: return (up, [.command, .option, .shift])
        case .swapDown: return (down, [.command, .option, .shift])
        case .settings: return (",", .command)
        case .toggleSidebar: return ("/", .command)
        case .toggleActivity: return (".", .command)
        case .increaseFontSize: return ("+", .command)
        case .decreaseFontSize: return ("-", .command)
        case .resetFontSize: return ("0", .command)
        case .clearScrollback: return ("k", [.command, .option])
        case .find: return ("f", .command)
        case .findNext: return ("g", .command)
        case .insertPath: return ("i", [.command, .control])
        case .equalizePanes: return ("=", [.command, .control])
        case .selectLastTab: return ("9", .command)
        default:
            if let n = tabNumber { return (String(n), .command) }
            return nil
        }
    }

    /// The shortcut drawn the way macOS menus show it, e.g. "⇧⌘P".
    var shortcutLabel: String? {
        guard let (key, modifiers) = shortcut else { return nil }
        var s = ""
        if modifiers.contains(.control) { s += "⌃" }
        if modifiers.contains(.option) { s += "⌥" }
        if modifiers.contains(.shift) { s += "⇧" }
        if modifiers.contains(.command) { s += "⌘" }
        let names: [String: String] = [
            "\t": "⇥", "\r": "↩",
            String(UnicodeScalar(NSUpArrowFunctionKey)!): "↑",
            String(UnicodeScalar(NSDownArrowFunctionKey)!): "↓",
            String(UnicodeScalar(NSLeftArrowFunctionKey)!): "←",
            String(UnicodeScalar(NSRightArrowFunctionKey)!): "→",
            String(UnicodeScalar(NSPageUpFunctionKey)!): "⇞",
            String(UnicodeScalar(NSPageDownFunctionKey)!): "⇟",
        ]
        return s + (names[key] ?? key.uppercased())
    }

    func menuItem(title: String? = nil) -> NSMenuItem {
        let item = NSMenuItem(title: title ?? menuTitle, action: #selector(AppDelegate.runCommand(_:)), keyEquivalent: "")
        item.representedObject = rawValue
        if let (key, modifiers) = shortcut {
            item.keyEquivalent = key
            item.keyEquivalentModifierMask = modifiers
        }
        return item
    }

    /// The command bound to `event`, if any.
    static func matching(_ event: NSEvent) -> Command? {
        let flags = event.modifierFlags.intersection([.command, .shift, .option, .control])
        guard let chars = event.charactersIgnoringModifiers?.lowercased() else { return nil }
        for command in allCases {
            guard let (key, modifiers) = command.shortcut else { continue }
            if modifiers == flags, key.lowercased() == chars { return command }
            // ⌘+ arrives as "=" without shift on US layouts; ⇧⌘↩ etc. keep their key.
            if command == .increaseFontSize, flags.subtracting(.shift) == .command, chars == "=" { return command }
            // Shifted tab arrives as backtab.
            if key == "\t", chars == "\u{19}", modifiers == flags { return command }
        }
        return nil
    }
}
