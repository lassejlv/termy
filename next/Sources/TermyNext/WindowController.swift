import AppKit
import Termy
import TermyAppKit
import TermyNextCore

/// Tells the controller whenever keyboard focus moves, so the focused pane,
/// tab badges and pane fading follow the first responder.
@MainActor
final class TermyWindow: NSWindow {
    var onFirstResponderChange: (() -> Void)?

    override func makeFirstResponder(_ responder: NSResponder?) -> Bool {
        let result = super.makeFirstResponder(responder)
        onFirstResponderChange?()
        return result
    }
}

/// Root of the window's views; lays out chrome around the pane area.
@MainActor
final class RootView: NSView {
    var onLayout: (() -> Void)?
    override var isFlipped: Bool { true }
    override func layout() {
        super.layout()
        onLayout?()
    }
}

/// Swallows clicks outside an open picker and closes it.
@MainActor
final class ScrimView: NSView {
    var onClick: (() -> Void)?
    override func mouseDown(with event: NSEvent) { onClick?() }
    override var mouseDownCanMoveWindow: Bool { false }
}

/// One window showing one session: titlebar tabs or a sidebar, the pane
/// layout, the Activity panel and the pickers.
@MainActor
final class WindowController: NSWindowController, NSWindowDelegate, TabNavigator, PaneDelegate {
    private(set) var session: TerminalSessionGroup
    /// This window's ID in the session host's layout (`termy mux window <id>`).
    var hostWindowID = UUID().uuidString

    private let root = RootView()
    private let glass = NSVisualEffectView()
    private let tint = NSView()
    private let container = SplitContainerView()
    private let strip = TabStripView()
    private let sidebar = SidebarView()
    private var activity: ActivityView?
    private let scrim = ScrimView()
    private var picker: PickerView?
    private var sidebarHidden = false
    private var lastTabStyle = Preferences.shared.tabStyle

    private lazy var sessionButton = IconButton(symbol: "rectangle.stack", pointSize: 13, help: "Change Session (⌘K)") { [weak self] in
        self?.run(.changeSession)
    }
    private lazy var sidebarButton = IconButton(symbol: IconButton.sidebarGlyph, pointSize: 13, help: "Toggle Sidebar (⌘/)") { [weak self] in
        self?.run(.toggleSidebar)
    }
    private lazy var activityButton = IconButton(symbol: "eye", pointSize: 13, help: "Show Activity (⌘.)") { [weak self] in
        self?.run(.toggleActivity)
    }
    private lazy var paletteButton = IconButton(symbol: "command", pointSize: 13, help: "Command Palette (⇧⌘P)") { [weak self] in
        self?.run(.commandPalette)
    }
    private lazy var newTabButton = IconButton(symbol: "plus", pointSize: 14, help: "New Tab (⌘T)") { [weak self] in
        self?.run(.newTab)
    }
    private var chromeButtons: [IconButton] { [sessionButton, sidebarButton, activityButton, paletteButton, newTabButton] }

    let titlebarHeight: CGFloat = 44

    init(session: TerminalSessionGroup) {
        self.session = session
        let window = TermyWindow(
            contentRect: NSRect(x: 0, y: 0, width: 1100, height: 720),
            styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
            backing: .buffered, defer: false)
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        window.isMovableByWindowBackground = false
        window.minSize = NSSize(width: 480, height: 300)
        window.tabbingMode = .disallowed
        window.isOpaque = false
        window.backgroundColor = .clear
        super.init(window: window)
        window.delegate = self
        window.onFirstResponderChange = { [weak self] in self?.syncFocus() }
        buildViews()
        window.contentView = root
        window.setFrameAutosaveName("TermyNextMain")
        if window.frame.origin == .zero { window.center() }
        showSession(session)
        NotificationCenter.default.addObserver(self, selector: #selector(preferencesChanged), name: Preferences.didChange, object: nil)
        NotificationCenter.default.addObserver(self, selector: #selector(workspaceChanged), name: Workspace.didChange, object: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    // MARK: Views

    private func buildViews() {
        root.wantsLayer = true
        root.onLayout = { [weak self] in self?.layoutRoot() }
        glass.material = .hudWindow
        glass.blendingMode = .behindWindow
        glass.state = .active
        tint.wantsLayer = true
        root.addSubview(glass)
        root.addSubview(tint)
        root.addSubview(container)
        root.addSubview(sidebar)
        root.addSubview(strip)
        chromeButtons.forEach(root.addSubview)
        strip.navigator = self
        sidebar.navigator = self
        sidebar.onSelectSession = { [weak self] in self?.switchTo($0) }
        scrim.onClick = { [weak self] in
            self?.picker?.dismiss()
            self?.activity?.dismiss()
        }
        applyTheme()
    }

    private func applyTheme() {
        let theme = AppConfig.shared.theme
        let liquid = theme.isLiquid
        window?.appearance = theme.appearance
        glass.isHidden = !liquid
        glass.appearance = theme.appearance
        tint.layer?.backgroundColor = (liquid ? theme.backdropTint : theme.chrome).cgColor
        root.layer?.cornerRadius = 12
        for button in chromeButtons {
            button.tint = theme.textSecondary
            button.hoverTint = theme.textPrimary
            button.hoverFill = theme.hover
        }
        strip.applyStyle()
        sidebar.applyStyle()
        for tab in session.tabs { tab.allPanes.forEach { $0.themeChanged() } }
        root.needsLayout = true
        root.needsDisplay = true
    }

    /// Lays out the chrome; with `animated`, panels slide and fade and panes glide.
    private func layoutRoot(animated: Bool = false) {
        let bounds = root.bounds
        glass.frame = bounds
        tint.frame = bounds
        let style = Preferences.shared.tabStyle
        let showsSidebar = style != .above && !sidebarHidden
        let inFullScreen = window?.styleMask.contains(.fullScreen) == true
        let lightsWidth: CGFloat = inFullScreen ? 8 : 78
        let outer: CGFloat = Preferences.shared.paneStyle == .compact ? 0 : 8
        let sidebarWidth = Preferences.shared.sidebarWidth

        var content = bounds
        var buttonsRight = bounds.width - 10

        strip.isHidden = style != .above
        sessionButton.isHidden = style != .above
        sidebarButton.isHidden = style == .above

        var sidebarFrame = NSRect.zero
        switch style {
        case .above:
            content = NSRect(x: outer, y: titlebarHeight, width: bounds.width - outer * 2, height: bounds.height - titlebarHeight - outer)
            sessionButton.frame = NSRect(x: lightsWidth + 4, y: (titlebarHeight - 30) / 2, width: 30, height: 30)
            let stripX = sessionButton.frame.maxX + 10
            let maxWidth = bounds.width - stripX - 120
            let width = strip.preferredWidth(maximum: maxWidth)
            strip.frame = NSRect(x: stripX, y: (titlebarHeight - 36) / 2, width: max(0, width), height: 36)
        case .left, .right:
            if showsSidebar {
                let x = style == .left ? 0 : bounds.width - sidebarWidth
                sidebarFrame = NSRect(x: x, y: 0, width: sidebarWidth, height: bounds.height)
                sidebar.topInset = titlebarHeight
                let contentX = style == .left ? sidebarWidth : outer
                content = NSRect(x: contentX, y: outer, width: bounds.width - sidebarWidth - outer, height: bounds.height - outer * 2)
                if style == .right {
                    buttonsRight = bounds.width - 14
                    content.origin.y = titlebarHeight
                    content.size.height = bounds.height - titlebarHeight - outer
                } else {
                    buttonsRight = sidebarWidth - 12
                }
            } else {
                content = NSRect(x: outer, y: titlebarHeight, width: bounds.width - outer * 2, height: bounds.height - titlebarHeight - outer)
            }
            let toggleX = style == .left || !showsSidebar ? lightsWidth + 4 : bounds.width - sidebarWidth + 12
            sidebarButton.frame = NSRect(x: toggleX, y: (titlebarHeight - 28) / 2, width: 28, height: 28)
        }
        place(sidebar, frame: showsSidebar ? sidebarFrame : sidebarFrame.isEmpty ? sidebar.frame : sidebarFrame,
              visible: showsSidebar, slide: style == .right ? 24 : -24, animated: animated)

        for (i, button) in [newTabButton, paletteButton, activityButton].enumerated() {
            button.frame = NSRect(x: buttonsRight - 30 - CGFloat(i) * 30, y: (titlebarHeight - 30) / 2, width: 30, height: 30)
        }
        activityButton.isActive = activity != nil
        container.frame = content

        scrim.frame = bounds
        if let activity {
            // Hangs under the eye button, kept inside the window.
            let width = ActivityView.width
            let x = min(max(8, activityButton.frame.midX - width / 2), bounds.width - width - 8)
            activity.frame = NSRect(x: x, y: activityButton.frame.maxY + 6, width: width, height: activity.preferredHeight)
        }
        if let picker {
            let width: CGFloat = 340
            let height = min(picker.preferredHeight, bounds.height - 80)
            picker.frame = NSRect(x: (bounds.width - width) / 2, y: titlebarHeight + 4, width: width, height: height)
        }
        positionTrafficLights()
        // AppKit lays the titlebar out after us on resize and focus changes.
        DispatchQueue.main.async { [weak self] in self?.positionTrafficLights() }
        strip.needsLayout = true
        strip.needsDisplay = true
        sidebar.needsLayout = true
    }

    /// Shows or hides a side panel, sliding it in from `slide` points away.
    private func place(_ view: NSView, frame: NSRect, visible: Bool, slide: CGFloat, animated: Bool) {
        if visible {
            if view.isHidden {
                view.isHidden = false
                if animated {
                    Motion.instantly {
                        view.frame = frame.offsetBy(dx: slide, dy: 0)
                        view.alphaValue = 0
                    }
                }
            }
            if animated {
                view.animator().frame = frame
                view.animator().alphaValue = 1
            } else {
                view.frame = frame
                view.alphaValue = 1
            }
        } else if !view.isHidden {
            if animated, view.alphaValue > 0 {
                view.animator().frame = view.frame.offsetBy(dx: slide, dy: 0)
                view.animator().alphaValue = 0
                hidingViews.insert(ObjectIdentifier(view))
            } else if !hidingViews.contains(ObjectIdentifier(view)) {
                view.isHidden = true
            }
        }
    }

    private var hidingViews: Set<ObjectIdentifier> = []

    /// Animates the chrome to its new layout after a style or panel change.
    private func animateRoot() {
        Motion.animate(0.38, {
            layoutRoot(animated: true)
            root.layoutSubtreeIfNeeded()
        }, completion: { [weak self] in
            guard let self else { return }
            for view in [self.sidebar as NSView] where self.hidingViews.contains(ObjectIdentifier(view)) {
                view.isHidden = true
                view.alphaValue = 1
            }
            self.hidingViews.removeAll()
            self.root.needsLayout = true
        })
    }

    /// Centers the traffic lights in the taller titlebar row.
    private func positionTrafficLights() {
        guard let window, !window.styleMask.contains(.fullScreen),
              let titlebar = window.standardWindowButton(.closeButton)?.superview else { return }
        // The system titlebar stays 28pt tall; the buttons hang just past its
        // bottom edge so their centers line up with our 44pt row.
        let types: [NSWindow.ButtonType] = [.closeButton, .miniaturizeButton, .zoomButton]
        for (i, type) in types.enumerated() {
            guard let button = window.standardWindowButton(type) else { continue }
            let y = titlebar.isFlipped
                ? titlebarHeight / 2 - button.frame.height / 2
                : titlebar.bounds.height - titlebarHeight / 2 - button.frame.height / 2
            let origin = NSPoint(x: 16 + CGFloat(i) * 20, y: y)
            if button.frame.origin != origin { button.setFrameOrigin(origin) }
        }
    }

    // MARK: Session and tabs

    private func showSession(_ session: TerminalSessionGroup) {
        let changed = self.session !== session
        self.session = session
        session.hostWindowID = hostWindowID
        session.lastActive = Date()
        strip.navigator = self
        if changed {
            Motion.crossfade(sidebar, duration: 0.25)
            Motion.crossfade(strip, duration: 0.25)
        }
        showSelectedTab(transition: changed)
        updateTitle()
        root.needsLayout = true
    }

    private func showSelectedTab(transition: Bool = true) {
        guard let tab = session.selectedTab else { return }
        if transition, container.tab !== tab, container.tab != nil { Motion.crossfade(container, duration: 0.22) }
        container.tab = tab
        strip.applyStyle()
        container.reloadPanes()
        for pane in tab.allPanes { pane.delegate = self }
        root.needsLayout = true
        root.layoutSubtreeIfNeeded()
        tab.focusedPane?.focus()
        syncFocus()
        strip.needsDisplay = true
        sidebar.needsLayout = true
    }

    private var currentTab: TerminalTab? { session.selectedTab }
    private var focusedPane: PaneView? { currentTab?.focusedPane }

    func selectTab(at index: Int) {
        guard session.tabs.indices.contains(index) else { return }
        session.selectedIndex = index
        showSelectedTab()
        updateTitle()
    }

    func closeTab(at index: Int) {
        guard session.tabs.indices.contains(index) else { return }
        close(tab: session.tabs[index])
    }

    func moveTab(from source: Int, to destination: Int) {
        session.move(from: source, to: destination)
        strip.needsDisplay = true
        sidebar.needsLayout = true
    }

    private func newTab(directory: String? = nil, command: String? = nil) {
        do {
            let pane = try PaneView(workingDirectory: directory ?? focusedPane?.directory, command: command)
            session.append(TerminalTab(pane: pane))
            Workspace.shared.noteDirectory(pane.directory)
            showSelectedTab()
            Workspace.shared.changed()
        } catch {
            showError(error)
        }
    }

    private func close(tab: TerminalTab) {
        tab.allPanes.forEach { $0.terminate(); $0.removeFromSuperview() }
        session.remove(tab)
        if session.tabs.isEmpty {
            closeCurrentSession()
        } else {
            showSelectedTab()
            Workspace.shared.changed()
        }
    }

    private func closeCurrentSession() {
        let closing = session
        Workspace.shared.remove(closing)
        let shown = Set(AppDelegate.shared.controllers.map { ObjectIdentifier($0.session) })
        if let next = Workspace.shared.sessionsByRecency.first(where: { !shown.contains(ObjectIdentifier($0)) }) {
            showSession(next)
        } else {
            window?.close()
        }
    }

    /// Shows the session the host's layout puts in this window, with its
    /// tabs and splits as they now are.
    func showFromHost(_ target: TerminalSessionGroup) {
        if target !== session {
            showSession(target)
        } else {
            showSelectedTab(transition: false)
        }
        for tab in session.tabs { tab.allPanes.forEach { $0.delegate = self } }
        container.reloadPanes()
        container.animateLayout()
        strip.needsDisplay = true
        sidebar.needsLayout = true
        updateTitle()
    }

    func switchTo(_ target: TerminalSessionGroup) {
        guard target !== session else { return }
        if let other = AppDelegate.shared.controllers.first(where: { $0 !== self && $0.session === target }) {
            other.window?.makeKeyAndOrderFront(nil)
            return
        }
        session.lastActive = Date()
        showSession(target)
        Workspace.shared.changed()
    }

    func reveal(session target: TerminalSessionGroup, tab: TerminalTab, pane: PaneView) {
        if target !== session { switchTo(target) }
        guard let owner = AppDelegate.shared.controllers.first(where: { $0.session === target }) else { return }
        if let index = target.tabs.firstIndex(where: { $0 === tab }) { owner.selectTab(at: index) }
        tab.setFocus(pane.id)
        pane.focus()
    }

    private func newSession(named name: String? = nil) {
        do {
            let created = try Workspace.shared.createSession(name: name, directory: focusedPane?.directory)
            switchTo(created)
        } catch {
            showError(error)
        }
    }

    // MARK: Focus

    /// Marks the pane that owns the first responder as focused.
    func syncFocus() {
        guard let tab = currentTab else { return }
        if let terminal = window?.firstResponder as? TerminalView,
           let pane = tab.allPanes.first(where: { $0.terminal === terminal }) {
            tab.setFocus(pane.id)
        }
        let keyWindow = window?.isKeyWindow == true
        for pane in tab.allPanes {
            pane.isFocused = keyWindow && pane.id == tab.focusedID && picker == nil
        }
        container.updateDimming()
        updateTitle()
        strip.needsDisplay = true
        sidebar.needsDisplay = true
    }

    private func focus(_ id: UUID) {
        guard let tab = currentTab, let pane = tab.panes[id] else { return }
        tab.setFocus(id)
        pane.focus()
    }

    private func updateTitle() {
        guard let tab = currentTab else { return }
        window?.title = "\(tab.title) — \(session.name)"
    }

    // MARK: Panes

    private func split(_ direction: Direction, from anchor: PaneView? = nil) {
        guard let tab = currentTab, let anchor = anchor ?? focusedPane else { return }
        do {
            let pane = try PaneView(workingDirectory: anchor.directory)
            pane.zoom = anchor.zoom  // A split keeps the zoom of the pane it came from.
            pane.delegate = self
            if tab.floating[anchor.id] != nil { tab.toggleFloating(anchor.id) }
            tab.insert(pane, nextTo: anchor.id, direction: direction)
            container.reloadPanes()
            container.animateLayout(appearing: pane)
            pane.focus()
            Workspace.shared.changed()
        } catch {
            showError(error)
        }
    }

    private func close(pane: PaneView) {
        guard let tab = currentTab ?? session.tabs.first(where: { $0.panes[pane.id] != nil }) else { return }
        if tab.allPanes.count <= 1 {
            close(tab: tab)
            return
        }
        tab.remove(pane.id)
        container.animateRemoval(of: pane) { pane.terminate() }
        container.animateLayout()
        tab.focusedPane?.focus()
        Workspace.shared.changed()
    }

    private func moveFocus(_ direction: Direction) {
        guard let tab = currentTab else { return }
        if let target = SplitNavigation.neighbor(of: tab.focusedID, direction: direction, frames: container.paneFrames) {
            focus(target)
        }
    }

    private func swap(_ direction: Direction) {
        guard let tab = currentTab,
              let target = SplitNavigation.neighbor(of: tab.focusedID, direction: direction, frames: container.paneFrames) else { return }
        tab.tree = tab.tree.swapping(tab.focusedID, target)
        container.animateLayout()
    }

    private func cycleFocus(_ step: Int) {
        guard let tab = currentTab else { return }
        let order = tab.allPanes.map(\.id)
        guard let index = order.firstIndex(of: tab.focusedID), order.count > 1 else { return }
        focus(order[(index + step + order.count) % order.count])
    }

    // PaneDelegate

    func paneDidBecomeFocused(_ pane: PaneView) { syncFocus() }
    func pane(_ pane: PaneView, requestSplit direction: Direction) { split(direction, from: pane) }
    func paneRequestClose(_ pane: PaneView) { close(pane: pane) }
    func paneDidExit(_ pane: PaneView) {
        // A restarting host ends every program; those panes get replaced.
        guard !SessionHost.shared.isRestarting else { return }
        // Find the tab even when the pane's tab isn't showing.
        if let tab = session.tabs.first(where: { $0.panes[pane.id] != nil }) {
            if tab.allPanes.count <= 1 {
                close(tab: tab)
            } else {
                tab.remove(pane.id)
                if tab === currentTab {
                    container.animateRemoval(of: pane) { pane.terminate() }
                    container.animateLayout()
                    tab.focusedPane?.focus()
                } else {
                    pane.terminate()
                    pane.removeFromSuperview()
                }
                Workspace.shared.changed()
            }
        }
    }

    func paneRequestFloat(_ pane: PaneView) {
        guard let tab = currentTab else { return }
        tab.toggleFloating(pane.id)
        container.reloadPanes()
        container.animateLayout(duration: 0.4)
        pane.focus()
    }

    func paneRequestZoom(_ pane: PaneView) {
        guard let tab = currentTab else { return }
        tab.zoomedID = tab.zoomedID == pane.id ? nil : pane.id
        tab.setFocus(pane.id)
        container.animateLayout(duration: 0.36)
        pane.focus()
    }

    func paneDidChange(_ pane: PaneView) {
        strip.needsDisplay = true
        sidebar.needsDisplay = true
        refreshActivity()
        if pane.id == currentTab?.focusedID { updateTitle() }
    }

    func pane(_ pane: PaneView, draggedHeaderBy delta: NSPoint) {
        if pane.isFloating { container.moveFloating(pane, by: delta) }
    }

    // MARK: Dragging panes

    private var dropIndicator: DropIndicatorView?

    /// Where a pane dragged to `point` (screen coordinates) would land in
    /// this window: a tab or new tab in the tab bar or sidebar, or beside or
    /// in place of a pane. Nil over chrome, or over the dragged pane itself.
    func dropTarget(atScreenPoint point: NSPoint, dragging pane: PaneView, tab dragged: TerminalTab? = nil) -> PaneDropTarget? {
        guard let window else { return nil }
        let inWindow = window.convertPoint(fromScreen: point)
        if let dragged { return dropTarget(at: inWindow, draggingTab: dragged) }

        if !strip.isHidden, strip.superview != nil {
            let p = strip.convert(inWindow, from: nil)
            if strip.bounds.contains(p) {
                if let index = strip.tabIndex(at: p) { return dropOnTab(session.tabs[index], dragging: pane) }
                return dropOnNewTab(dragging: pane)
            }
        }
        if !sidebar.isHidden, sidebar.alphaValue > 0, sidebar.frame.width > 0 {
            let p = sidebar.convert(inWindow, from: nil)
            if sidebar.bounds.contains(p) {
                if let hit = sidebar.tab(at: p) { return dropOnTab(hit.tab, dragging: pane) }
                return dropOnNewTab(dragging: pane)
            }
        }

        let p = container.convert(inWindow, from: nil)
        guard container.bounds.contains(p), let tab = currentTab else { return nil }
        let candidates = tab.allPanes.filter { !$0.isFloating && $0.superview === container && !$0.isClosing }
        guard let over = candidates.first(where: { $0.frame.contains(p) }) else { return nil }
        if over === pane { return nil }
        let frame = over.frame
        let x = (p.x - frame.minX) / max(frame.width, 1), y = (p.y - frame.minY) / max(frame.height, 1)
        let edges: [(CGFloat, Direction)] = [(x, .left), (1 - x, .right), (y, .up), (1 - y, .down)]
        let nearest = edges.min { $0.0 < $1.0 }!
        let sameTab = tab.panes[pane.id] != nil
        if sameTab, nearest.0 > 0.3 { return .swap(over) }
        return .split(over, nearest.1)
    }

    /// Where a whole dragged tab would land: merged beside a pane or into a
    /// tab, or as a tab of this window.
    private func dropTarget(at inWindow: NSPoint, draggingTab dragged: TerminalTab) -> PaneDropTarget? {
        let ownTab = session.tabs.contains { $0 === dragged }
        if !strip.isHidden, strip.superview != nil {
            let p = strip.convert(inWindow, from: nil)
            if strip.bounds.contains(p) {
                if let index = strip.tabIndex(at: p) { return session.tabs[index] === dragged ? nil : .tab(session.tabs[index]) }
                return ownTab ? nil : .newTab
            }
        }
        if !sidebar.isHidden, sidebar.alphaValue > 0, sidebar.frame.width > 0 {
            let p = sidebar.convert(inWindow, from: nil)
            if sidebar.bounds.contains(p) {
                if let hit = sidebar.tab(at: p) { return hit.tab === dragged ? nil : .tab(hit.tab) }
                return ownTab ? nil : .newTab
            }
        }
        let p = container.convert(inWindow, from: nil)
        guard container.bounds.contains(p), let tab = currentTab, tab !== dragged else { return nil }
        let candidates = tab.allPanes.filter { !$0.isFloating && $0.superview === container && !$0.isClosing }
        guard let over = candidates.first(where: { $0.frame.contains(p) }) else { return nil }
        let frame = over.frame
        let x = (p.x - frame.minX) / max(frame.width, 1), y = (p.y - frame.minY) / max(frame.height, 1)
        let edges: [(CGFloat, Direction)] = [(x, .left), (1 - x, .right), (y, .up), (1 - y, .down)]
        return .split(over, edges.min { $0.0 < $1.0 }!.1)
    }

    /// Shows another tab while `tab` is pulled out of the tab bar.
    func showOtherTab(than tab: TerminalTab, preferring index: Int?) {
        let tabs = session.tabs
        guard tabs.count > 1, let own = tabs.firstIndex(where: { $0 === tab }) else { return }
        let target = index.flatMap { tabs.indices.contains($0) && $0 != own ? $0 : nil } ?? (own > 0 ? own - 1 : own + 1)
        selectTab(at: target)
    }

    /// Takes `tab` out of this window without ending its programs; an
    /// emptied window closes.
    func detachTabForDrop(_ tab: TerminalTab) {
        session.remove(tab)
        tab.allPanes.forEach { $0.removeFromSuperview() }
        if session.tabs.isEmpty {
            Workspace.shared.remove(session)
            window?.close()
            return
        }
        showSelectedTab(transition: false)
        strip.needsDisplay = true
        sidebar.needsLayout = true
    }

    /// Puts a dragged tab where it was dropped: its panes merged beside a
    /// pane or into a tab, or the tab itself added to this window.
    func acceptTabDrop(_ tab: TerminalTab, at target: PaneDropTarget) {
        let destination: TerminalTab
        switch target {
        case .split(let over, let direction):
            guard let dest = session.tabs.first(where: { $0.panes[over.id] != nil }), dest !== tab else { return }
            AppDelegate.shared.detachTab(tab)
            dest.merge(tab, nextTo: over.id, direction: direction)
            destination = dest
        case .swap(let over):
            guard let dest = session.tabs.first(where: { $0.panes[over.id] != nil }), dest !== tab else { return }
            AppDelegate.shared.detachTab(tab)
            dest.merge(tab, nextTo: over.id, direction: SplitNavigation.bestFitDirection(for: over.frame))
            destination = dest
        case .tab(let dest):
            guard dest !== tab, session.tabs.contains(where: { $0 === dest }) else { return }
            AppDelegate.shared.detachTab(tab)
            let anchor = dest.focusedPane ?? dest.allPanes.first
            let direction = anchor.map { $0.frame.isEmpty ? Direction.right : SplitNavigation.bestFitDirection(for: $0.frame) } ?? .right
            dest.merge(tab, nextTo: anchor?.id ?? dest.focusedID, direction: direction)
            destination = dest
        case .newTab:
            guard !session.tabs.contains(where: { $0 === tab }) else { return }
            AppDelegate.shared.detachTab(tab)
            session.append(tab)
            destination = tab
        case .newWindow:
            return
        }
        for pane in destination.allPanes { pane.delegate = self }
        select(destination)
        guard let focused = destination.focusedPane else { return }
        finishDrop(focused)
    }

    private func dropOnNewTab(dragging pane: PaneView) -> PaneDropTarget? {
        let source = session.tabs.first { $0.panes[pane.id] != nil }
        return source?.allPanes.count == 1 ? nil : .newTab
    }

    private func dropOnTab(_ tab: TerminalTab, dragging pane: PaneView) -> PaneDropTarget? {
        // Its own tab, when it is the only pane there, would change nothing.
        if tab.panes[pane.id] != nil, tab.allPanes.count == 1 { return nil }
        return .tab(tab)
    }

    /// Shows where a pane would land; nil hides the highlight.
    func showDropIndicator(_ target: PaneDropTarget?) {
        guard let target, let frame = dropFrame(for: target) else {
            if let indicator = dropIndicator {
                dropIndicator = nil
                Motion.disappear(indicator, scale: 0.98, duration: 0.12) { indicator.removeFromSuperview() }
            }
            return
        }
        if let indicator = dropIndicator {
            Motion.animate(0.16) { indicator.animator().frame = frame }
        } else {
            let indicator = DropIndicatorView(frame: frame)
            root.addSubview(indicator, positioned: .above, relativeTo: nil)
            dropIndicator = indicator
            Motion.appear(indicator, scale: 0.97, duration: 0.14)
        }
    }

    private func dropFrame(for target: PaneDropTarget) -> NSRect? {
        switch target {
        case .split(let over, let direction):
            var f = over.frame
            switch direction {
            case .left: f.size.width /= 2
            case .right: f.origin.x += f.width / 2; f.size.width /= 2
            case .up: f.size.height /= 2
            case .down: f.origin.y += f.height / 2; f.size.height /= 2
            }
            return root.convert(f.insetBy(dx: 3, dy: 3), from: container)
        case .swap(let over):
            return root.convert(over.frame.insetBy(dx: 6, dy: 6), from: container)
        case .tab(let tab):
            if !strip.isHidden, let index = session.tabs.firstIndex(where: { $0 === tab }), let f = strip.tabFrame(at: index) {
                return root.convert(f, from: strip)
            }
            if !sidebar.isHidden, let row = sidebar.rowFrame(for: tab) { return root.convert(row, from: sidebar) }
            return nil
        case .newTab:
            if !strip.isHidden { return root.convert(strip.newTabDropFrame, from: strip) }
            return root.convert(sidebar.newTabDropFrame, from: sidebar)
        case .newWindow:
            return nil
        }
    }

    /// Switches to `tab` while a pane hovers it during a drag.
    func showTabForDrop(_ tab: TerminalTab) {
        guard let index = session.tabs.firstIndex(where: { $0 === tab }), tab !== currentTab else { return }
        selectTab(at: index)
    }

    /// Takes `pane` out of its tab without ending its program. An emptied
    /// tab goes away; an emptied window closes.
    func detachForDrop(_ pane: PaneView) {
        guard let tab = session.tabs.first(where: { $0.panes[pane.id] != nil }) else { return }
        let wasShowing = tab === currentTab
        tab.remove(pane.id)
        pane.removeFromSuperview()
        if tab.isEmpty {
            session.remove(tab)
            if session.tabs.isEmpty {
                Workspace.shared.remove(session)
                window?.close()
                return
            }
            showSelectedTab(transition: false)
        } else if wasShowing {
            container.reloadPanes()
            container.animateLayout()
        }
        strip.needsDisplay = true
        sidebar.needsLayout = true
    }

    /// Puts a dragged pane where it was dropped in this window.
    func acceptDrop(_ pane: PaneView, at target: PaneDropTarget) {
        // Swapping within a tab keeps both panes where they are in the tree.
        if case .swap(let other) = target, let tab = currentTab, tab.panes[pane.id] != nil, tab.panes[other.id] != nil {
            tab.tree = tab.tree.swapping(pane.id, other.id)
            tab.setFocus(pane.id)
            container.animateLayout()
            pane.focus()
            Workspace.shared.changed()
            return
        }
        if case .tab(let tab) = target, tab.panes[pane.id] != nil, tab.allPanes.count == 1 { return }
        // A tab's only pane dropped on "new tab" in its own window stays put.
        if case .newTab = target, let source = session.tabs.first(where: { $0.panes[pane.id] != nil }), source.allPanes.count == 1 { return }
        AppDelegate.shared.detachPane(pane)
        pane.delegate = self
        pane.isFloating = false
        switch target {
        case .split(let over, let direction):
            guard let tab = session.tabs.first(where: { $0.panes[over.id] != nil }) else { return newTab(with: pane) }
            tab.insert(pane, nextTo: over.id, direction: direction)
            select(tab)
        case .swap(let over):
            guard let tab = session.tabs.first(where: { $0.panes[over.id] != nil }) else { return newTab(with: pane) }
            tab.insert(pane, nextTo: over.id, direction: SplitNavigation.bestFitDirection(for: over.frame))
            select(tab)
        case .tab(let tab):
            guard session.tabs.contains(where: { $0 === tab }) else { return newTab(with: pane) }
            let anchor = tab.focusedPane ?? tab.allPanes.first
            let direction = anchor.map { $0.frame.isEmpty ? Direction.right : SplitNavigation.bestFitDirection(for: $0.frame) } ?? .right
            if let anchor { tab.insert(pane, nextTo: anchor.id, direction: direction) } else { return newTab(with: pane) }
            select(tab)
        case .newTab, .newWindow:
            return newTab(with: pane)
        }
        finishDrop(pane)
    }

    private func newTab(with pane: PaneView) {
        let tab = TerminalTab(pane: pane)
        session.append(tab)
        select(tab)
        finishDrop(pane)
    }

    private func select(_ tab: TerminalTab) {
        if let index = session.tabs.firstIndex(where: { $0 === tab }) { session.selectedIndex = index }
    }

    private func finishDrop(_ pane: PaneView) {
        showSelectedTab(transition: false)
        container.animateLayout(appearing: pane)
        window?.makeKeyAndOrderFront(nil)
        pane.focus()
        Workspace.shared.changed()
    }

    // MARK: Ticks

    /// Whether the focused pane has its find bar open.
    var isSearching: Bool { focusedPane?.isSearching ?? false }

    func tick() {
        currentTab?.allPanes.forEach { $0.searchTick() }
        strip.advanceSpinner()
        sidebar.advanceSpinner()
        activity?.advanceSpinner()
    }

    // MARK: Notifications

    // MARK: Session host restart

    /// Everything in the window crashes to the floor: each pane drops to the
    /// bottom with gravity, bounces, tips over, then the pile fades out.
    func crashDown(completion: @escaping () -> Void) {
        let panes = container.subviews.compactMap { $0 as? PaneView }
        guard !Motion.reduced, window?.isVisible == true, !panes.isEmpty else {
            Motion.crossfade(container, duration: 0.25)
            completion()
            return
        }
        let floor = container.bounds.height
        let fall: TimeInterval = 1.05
        var longest: TimeInterval = 0
        for pane in panes {
            guard let layer = pane.layer else { continue }
            pane.isClosing = true
            let frame = pane.frame
            let drop = max(0, floor - frame.maxY)
            let delay = Double.random(in: 0...0.16) + Double(frame.minX / max(container.bounds.width, 1)) * 0.08
            let tilt = CGFloat.random(in: 4...11) * (Bool.random() ? 1 : -1) * .pi / 180
            let bounce = min(30, 10 + drop * 0.08)
            // Pivot on the bottom middle, where the pane hits the floor.
            func transform(drop y: CGFloat, angle: CGFloat) -> NSValue {
                let pivotX = frame.width / 2, pivotY = frame.height
                var t = CATransform3DMakeTranslation(-pivotX, -pivotY, 0)
                t = CATransform3DConcat(t, CATransform3DMakeRotation(angle, 0, 0, 1))
                t = CATransform3DConcat(t, CATransform3DMakeTranslation(pivotX, pivotY + y, 0))
                return NSValue(caTransform3D: t)
            }
            let animation = CAKeyframeAnimation(keyPath: "transform")
            animation.values = [
                transform(drop: 0, angle: 0),
                transform(drop: -6, angle: -tilt * 0.15),
                transform(drop: drop, angle: tilt * 0.35),
                transform(drop: drop - bounce, angle: tilt * 0.8),
                transform(drop: drop, angle: tilt),
                transform(drop: drop - bounce * 0.3, angle: tilt * 1.08),
                transform(drop: drop, angle: tilt),
            ]
            animation.keyTimes = [0, 0.1, 0.5, 0.63, 0.76, 0.86, 1]
            let easeIn = CAMediaTimingFunction(name: .easeIn), easeOut = CAMediaTimingFunction(name: .easeOut)
            animation.timingFunctions = [easeOut, easeIn, easeOut, easeIn, easeOut, easeIn]
            animation.duration = fall
            animation.beginTime = CACurrentMediaTime() + delay
            animation.fillMode = .both
            animation.isRemovedOnCompletion = false
            layer.add(animation, forKey: "termyCrash")
            longest = max(longest, delay + fall)
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + longest + 0.35) {
            NSAnimationContext.runAnimationGroup({ context in
                context.duration = 0.3
                for pane in panes { pane.animator().alphaValue = 0 }
            }, completionHandler: { MainActor.assumeIsolated { completion() } })
        }
    }

    /// After a restart: shows the session's fresh panes rising into place.
    func showRestartedPanes() {
        guard let tab = session.selectedTab else { return }
        for case let pane as PaneView in container.subviews where tab.panes[pane.id] == nil {
            pane.layer?.removeAllAnimations()
            pane.removeFromSuperview()
        }
        showSelectedTab(transition: false)
        for (index, pane) in tab.allPanes.enumerated() {
            pane.alphaValue = 1
            DispatchQueue.main.asyncAfter(deadline: .now() + Double(index) * 0.05) {
                Motion.appear(pane, scale: 0.9, offsetY: -24, duration: 0.42)
            }
        }
        strip.needsDisplay = true
        sidebar.needsLayout = true
    }

    @objc private func preferencesChanged() {
        // Picking a side tab style (in Settings, say) shows the sidebar again.
        let style = Preferences.shared.tabStyle
        if style != lastTabStyle, style != .above { sidebarHidden = false }
        lastTabStyle = style
        Motion.crossfade(root, duration: 0.25)
        applyTheme()
        container.reloadPanes()
        animateRoot()
    }

    @objc private func workspaceChanged() {
        strip.needsDisplay = true
        sidebar.needsLayout = true
        refreshActivity()
        root.needsLayout = true
        updateTitle()
    }

    // MARK: NSWindowDelegate

    func windowDidBecomeKey(_ notification: Notification) {
        session.lastActive = Date()
        syncFocus()
        DispatchQueue.main.async { [weak self] in self?.positionTrafficLights() }
    }

    func windowDidResignKey(_ notification: Notification) {
        syncFocus()
        DispatchQueue.main.async { [weak self] in self?.positionTrafficLights() }
    }
    func windowDidResize(_ notification: Notification) { positionTrafficLights() }
    func windowDidEnterFullScreen(_ notification: Notification) { root.needsLayout = true }
    func windowDidExitFullScreen(_ notification: Notification) { root.needsLayout = true }

    func windowWillClose(_ notification: Notification) {
        // Closing a window detaches its session; it keeps running and stays in ⌘K.
        session.lastActive = Date()
        AppDelegate.shared.windowClosed(self)
    }

    private func showError(_ error: Error) {
        let alert = NSAlert(error: error)
        if let window { alert.beginSheetModal(for: window) } else { alert.runModal() }
    }

    // MARK: Commands

    func run(_ command: Command) {
        let prefs = Preferences.shared
        switch command {
        case .settings: AppDelegate.shared.showSettings()
        case .find: focusedPane?.showSearch()
        case .findNext: focusedPane?.findStep(-1)
        case .findPrevious: focusedPane?.findStep(1)
        case .restartSessionHost: AppDelegate.shared.restartSessionHost()
        case .commandPalette: showPalette()
        case .changeSession: showSessionPicker()
        case .renameSession:
            showPrompt(placeholder: "Session name", initial: session.name) { [weak self] name in
                guard let self, !name.trimmingCharacters(in: .whitespaces).isEmpty else { return }
                self.session.name = name.trimmingCharacters(in: .whitespaces)
                Workspace.shared.changed()
            }
        case .renameTab:
            guard let tab = currentTab else { return }
            showPrompt(placeholder: "Tab name (empty to follow the program)", initial: tab.customName ?? tab.title) { [weak self] name in
                let trimmed = name.trimmingCharacters(in: .whitespaces)
                tab.customName = trimmed.isEmpty ? nil : trimmed
                self?.strip.needsDisplay = true
                Workspace.shared.changed()
            }
        case .addRemoteHost: showRemoteHostPicker()
        case .newSession: newSession()
        case .newTab: newTab()
        case .goToDirectory: showDirectoryPicker(insert: false)
        case .insertPath: showDirectoryPicker(insert: true)
        case .newWindow: AppDelegate.shared.newWindow(directory: focusedPane?.directory)
        case .previousTab: selectTab(at: (session.selectedIndex - 1 + session.tabs.count) % max(session.tabs.count, 1))
        case .nextTab: selectTab(at: (session.selectedIndex + 1) % max(session.tabs.count, 1))
        case .moveTabLeft: if let tab = currentTab { session.move(tab, by: -1); strip.needsDisplay = true; sidebar.needsLayout = true }
        case .moveTabRight: if let tab = currentTab { session.move(tab, by: 1); strip.needsDisplay = true; sidebar.needsLayout = true }
        case .moveTabToNewWindow: moveTabToNewWindow()
        case .closePane: if let pane = focusedPane { close(pane: pane) }
        case .closeTab: if let tab = currentTab { close(tab: tab) }
        case .closeSession:
            session.tabs.forEach { $0.allPanes.forEach { $0.removeFromSuperview() } }
            closeCurrentSession()
        case .closeWindow: window?.performClose(nil)
        case .splitRight: split(.right)
        case .splitDown: split(.down)
        case .splitLeft: split(.left)
        case .splitUp: split(.up)
        case .splitBestFit:
            if let pane = focusedPane { split(SplitNavigation.bestFitDirection(for: pane.frame)) }
        case .floatPane: if let pane = focusedPane { paneRequestFloat(pane) }
        case .zoomPane: if let pane = focusedPane { paneRequestZoom(pane) }
        case .equalizePanes:
            if let tab = currentTab { tab.tree = tab.tree.equalized(); container.animateLayout() }
        case .focusLeft: moveFocus(.left)
        case .focusRight: moveFocus(.right)
        case .focusUp: moveFocus(.up)
        case .focusDown: moveFocus(.down)
        case .focusPreviousPane: cycleFocus(-1)
        case .focusNextPane: cycleFocus(1)
        case .swapLeft: swap(.left)
        case .swapRight: swap(.right)
        case .swapUp: swap(.up)
        case .swapDown: swap(.down)
        case .toggleSidebar:
            if prefs.tabStyle == .above {
                prefs.tabStyle = .left
                sidebarHidden = false
            } else {
                sidebarHidden.toggle()
                animateRoot()
            }
        case .toggleActivity: toggleActivity()
        case .tabsAbove: prefs.tabStyle = .above
        case .tabsLeft: sidebarHidden = false; prefs.tabStyle = .left
        case .tabsRight: sidebarHidden = false; prefs.tabStyle = .right
        case .paneComfortable: prefs.paneStyle = .comfortable
        case .paneCompact: prefs.paneStyle = .compact
        case .interfaceLiquid: prefs.interfaceStyle = .liquid
        case .interfaceClassic: prefs.interfaceStyle = .classic
        case .increaseFontSize: focusedPane?.changeZoom(by: 1)
        case .decreaseFontSize: focusedPane?.changeZoom(by: -1)
        case .resetFontSize: focusedPane?.changeZoom(by: nil)
        case .clearScrollback: focusedPane?.terminal.clearScrollback(nil)
        case .selectLastTab: selectTab(at: session.tabs.count - 1)
        default:
            if let n = command.tabNumber { selectTab(at: n - 1) }
        }
    }

    private func moveTabToNewWindow() {
        guard let tab = currentTab, session.tabs.count > 1 else { return }
        session.remove(tab)
        tab.allPanes.forEach { $0.removeFromSuperview() }
        showSelectedTab()
        AppDelegate.shared.openWindow(withTab: tab, name: "\(session.name)-\(tab.title)")
    }

    // MARK: Activity

    var isActivityOpen: Bool { activity != nil }

    /// Opens or closes the Activity popup under the eye button.
    private func toggleActivity() {
        if let activity {
            activity.dismiss()
            return
        }
        picker?.dismiss()
        let view = ActivityView()
        view.onSelect = { [weak self] session, tab, pane in self?.reveal(session: session, tab: tab, pane: pane) }
        view.onDismiss = { [weak self, weak view] in
            guard let self, self.activity === view else { return }
            self.activity = nil
            self.scrim.removeFromSuperview()
            self.activityButton.isActive = false
            self.focusedPane?.focus()
        }
        activity = view
        root.addSubview(scrim)
        root.addSubview(view)
        layoutRoot()
        Motion.appear(view, scale: 0.95, offsetY: -6, duration: 0.22)
        window?.makeFirstResponder(view)
    }

    private func refreshActivity() {
        guard let activity else { return }
        activity.reload()
        root.needsLayout = true
    }

    // MARK: Pickers

    private func present(_ view: PickerView) {
        picker?.dismiss()
        activity?.dismiss()
        picker = view
        view.onDismiss = { [weak self, weak view] in
            guard let self, self.picker === view else { return }
            self.picker = nil
            self.scrim.removeFromSuperview()
            self.focusedPane?.focus()
            self.syncFocus()
        }
        root.addSubview(scrim)
        root.addSubview(view)
        layoutRoot()
        Motion.appear(view, scale: 0.96, offsetY: -6, duration: 0.22)
        view.focusField()
        syncFocus()
    }

    private func showPrompt(placeholder: String, initial: String, submit: @escaping (String) -> Void) {
        present(PickerView(mode: .prompt(placeholder: placeholder, initial: initial, submit: submit)))
    }

    private func showPalette() {
        let view = PickerView(mode: .list(placeholder: "Search commands, sessions, and tabs…", items: { [weak self] in
            self?.paletteItems() ?? []
        }, create: nil))
        view.footer = "Action"
        present(view)
    }

    private func paletteItems() -> [PickerItem] {
        var items: [PickerItem] = Command.allCases.filter(\.showsInPalette).map { command in
            PickerItem(
                title: command.title,
                accessory: command.opensSubpicker ? .chevron(command.shortcutLabel) : command.shortcutLabel.map { .shortcut($0) } ?? .none,
                run: { [weak self] in self?.run(command) })
        }
        for (i, tab) in session.tabs.enumerated() where i != session.selectedIndex {
            items.append(PickerItem(title: tab.title, subtitle: tab.detail, symbol: "program:\(tab.program ?? "")",
                                    status: tab.activity, keywords: "tab", run: { [weak self] in self?.selectTab(at: i) }))
        }
        for other in Workspace.shared.sessionsByRecency where other !== session {
            items.append(PickerItem(title: other.name, subtitle: "\(other.tabs.count) tab\(other.tabs.count == 1 ? "" : "s")",
                                    symbol: "square.stack", status: other.activity, keywords: "session",
                                    run: { [weak self] in self?.switchTo(other) }))
        }
        return items
    }

    private func showSessionPicker() {
        let view = PickerView(mode: .list(placeholder: "Filter or create…", items: { [weak self] in
            guard let self else { return [] }
            var items = Workspace.shared.sessionsByRecency.map { s in
                PickerItem(title: s.name, subtitle: s === self.session ? nil : "\(s.tabs.count) tab\(s.tabs.count == 1 ? "" : "s")",
                           status: s.activity, accessory: s === self.session ? .check : .none,
                           run: { [weak self] in self?.switchTo(s) })
            }
            items.append(PickerItem(title: "New Session", accessory: Command.newSession.shortcutLabel.map { .shortcut($0) } ?? .none,
                                    run: { [weak self] in self?.newSession() }))
            return items
        }, create: { [weak self] name in
            PickerItem(title: "Create “\(name)”", symbol: "plus", run: { self?.newSession(named: name) })
        }))
        present(view)
    }

    private func showRemoteHostPicker() {
        let view = PickerView(mode: .list(placeholder: "Host, user@host or SSH alias…", items: { [weak self] in
            SSHConfig.hosts().map { host in
                PickerItem(title: host, symbol: "network", run: { self?.newTab(command: "ssh \(host)") })
            }
        }, create: { [weak self] host in
            PickerItem(title: "Connect to \(host)", symbol: "network", run: { self?.newTab(command: "ssh \(host)") })
        }))
        present(view)
    }

    private func showDirectoryPicker(insert: Bool) {
        let base = focusedPane?.directory ?? NSHomeDirectory()
        let view = PickerView(mode: .list(placeholder: insert ? "Insert a path…" : "Go to directory…", items: { [weak self] in
            self?.directoryItems(base: base, includeFiles: insert, insert: insert) ?? []
        }, create: { [weak self] typed in
            let path = (typed as NSString).expandingTildeInPath
            let absolute = path.hasPrefix("/") ? path : (base as NSString).appendingPathComponent(path)
            return PickerItem(title: Naming.abbreviatingHome(absolute), symbol: insert ? "text.insert" : "arrow.right.circle",
                              run: { self?.useDirectory(absolute, insert: insert) })
        }))
        present(view)
    }

    private func directoryItems(base: String, includeFiles: Bool, insert: Bool) -> [PickerItem] {
        var seen = Set<String>()
        var items: [PickerItem] = []
        func add(_ path: String, symbol: String) {
            guard seen.insert(path).inserted else { return }
            items.append(PickerItem(title: Naming.abbreviatingHome(path), symbol: symbol,
                                    run: { [weak self] in self?.useDirectory(path, insert: insert) }))
        }
        let fm = FileManager.default
        if let children = try? fm.contentsOfDirectory(atPath: base) {
            for name in children.sorted() where !name.hasPrefix(".") {
                let path = (base as NSString).appendingPathComponent(name)
                var isDir: ObjCBool = false
                fm.fileExists(atPath: path, isDirectory: &isDir)
                if isDir.boolValue { add(path, symbol: "folder") } else if includeFiles { add(path, symbol: "doc") }
            }
        }
        add((base as NSString).deletingLastPathComponent, symbol: "arrow.up.circle")
        for dir in Workspace.shared.recentDirectories { add(dir, symbol: "clock") }
        for s in Workspace.shared.sessions { for t in s.tabs { for p in t.allPanes { if let d = p.directory { add(d, symbol: "clock") } } } }
        add(NSHomeDirectory(), symbol: "house")
        return items
    }

    private func useDirectory(_ path: String, insert: Bool) {
        let quoted = "'" + path.replacingOccurrences(of: "'", with: "'\\''") + "'"
        guard let pane = focusedPane else { return }
        if insert {
            try? pane.terminal.session.paste(quoted + " ")
            return
        }
        Workspace.shared.noteDirectory(path)
        if pane.isShellInFront {
            try? pane.terminal.session.terminal.write("cd \(quoted)\r")
        } else {
            newTab(directory: path)
        }
    }
}

/// Host aliases from ~/.ssh/config for Add Remote Host.
enum SSHConfig {
    static func hosts() -> [String] {
        let path = (NSHomeDirectory() as NSString).appendingPathComponent(".ssh/config")
        guard let text = try? String(contentsOfFile: path, encoding: .utf8) else { return [] }
        var hosts: [String] = []
        for line in text.split(whereSeparator: \.isNewline) {
            let parts = line.trimmingCharacters(in: .whitespaces).split(whereSeparator: \.isWhitespace)
            guard parts.count >= 2, parts[0].lowercased() == "host" else { continue }
            hosts += parts.dropFirst().map(String.init).filter { !$0.contains("*") && !$0.contains("?") }
        }
        return hosts
    }
}
