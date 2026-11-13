import AppKit

/// The sidebar for Tab Style Left Side / Right Side: this Mac's sessions,
/// the current one expanded into its tabs.
@MainActor
final class SidebarView: GlassPanel {
    weak var navigator: TabNavigator?
    /// Switch to another session.
    var onSelectSession: ((TerminalSessionGroup) -> Void)?
    /// Space reserved at the top for the titlebar row (traffic lights and buttons).
    var topInset: CGFloat = 44

    private enum Row {
        case host
        case session(TerminalSessionGroup, expanded: Bool)
        case tab(TerminalTab, index: Int)
    }

    private var rows: [(Row, NSRect)] = []
    /// Slides under the selected tab row.
    private let pill = PassthroughView()
    private var pillGlass: GlassView?
    private var lastSelectedTab: ObjectIdentifier?
    private var hoverRow: Int?
    private var collapsed: Set<UUID> = []
    private var phase: CGFloat = 0
    private lazy var loaderSpots = LoaderSpots(host: self)
    private var drawnLoader = Preferences.shared.tabLoader
    private let footer = NSStackView()
    private lazy var addTabButton = IconButton(symbol: "plus.circle", pointSize: 12, help: Command.newTab.help()) { [weak self] in
        self?.navigator?.run(.newTab)
    }

    override init(frame: NSRect) {
        super.init(frame: frame)
        let buttons = [
            IconButton(symbol: "line.3.horizontal.decrease.circle", pointSize: 14, help: Command.changeSession.help("Filter Sessions")) { [weak self] in
                self?.navigator?.run(.changeSession)
            },
            IconButton(symbol: "rectangle.stack.badge.plus", pointSize: 14, help: Command.newSession.help()) { [weak self] in
                self?.navigator?.run(.newSession)
            },
            IconButton(symbol: "network.badge.shield.half.filled", pointSize: 14, help: "Add Remote Host…") { [weak self] in
                self?.navigator?.run(.addRemoteHost)
            },
        ]
        for (i, b) in buttons.enumerated() {
            addSubview(b)
            b.identifier = NSUserInterfaceItemIdentifier("footer\(i)")
        }
        addSubview(addTabButton)
        pill.wantsLayer = true
        pill.layer?.cornerRadius = 8
        pill.layer?.cornerCurve = .continuous
        addSubview(pill, positioned: .below, relativeTo: overlay)
    }

    func applyStyle() {
        let theme = AppConfig.shared.theme
        if theme.isLiquid {
            if pillGlass == nil {
                let glass = GlassView(style: .regular, cornerRadius: 9, tint: theme.selectionGlassTint, interactive: true)
                pill.addSubview(glass)
                pillGlass = glass
            }
            pillGlass?.tint = theme.selectionGlassTint
            pill.layer?.backgroundColor = NSColor.clear.cgColor
            pill.layer?.borderWidth = 0
        } else {
            pillGlass?.removeFromSuperview()
            pillGlass = nil
            pill.layer?.backgroundColor = theme.selected.cgColor
            pill.layer?.borderColor = theme.foreground.withAlphaComponent(0.1).cgColor
            pill.layer?.borderWidth = 0.7
        }
        needsLayout = true
        needsDisplay = true
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var mouseDownCanMoveWindow: Bool { false }

    func advanceSpinner() {
        guard Workspace.shared.sessions.contains(where: { $0.activity == .working }) else { return }
        phase = (phase - 24).truncatingRemainder(dividingBy: 360)
        if drawnLoader != Preferences.shared.tabLoader {
            needsDisplay = true
        } else if StatusIndicator.hostAnimatesWorking {
            // Only the dots move: redraw them, not the whole sidebar.
            for rect in workingDotsRects() { overlay.setNeedsDisplay(rect.insetBy(dx: -2, dy: -2)) }
        }
    }

    /// Where the three dots of each working row are drawn (see `drawContent`).
    private func workingDotsRects() -> [NSRect] {
        rows.compactMap { row, frame in
            switch row {
            case .tab(let tab, _) where tab.activity == .working:
                return NSRect(x: frame.maxX - 10 - StatusIndicator.dotsWidth, y: frame.minY, width: StatusIndicator.dotsWidth, height: frame.height)
            case .session(let session, _) where session !== navigator?.session && session.activity == .working:
                // The dots are centered on this spot and wider than it.
                let spot = NSRect(x: frame.maxX - 38, y: frame.midY - 5, width: 10, height: 10)
                return NSRect(x: spot.midX - StatusIndicator.dotsWidth / 2, y: spot.minY, width: StatusIndicator.dotsWidth, height: spot.height)
            default:
                return nil
            }
        }
    }

    override func layout() {
        super.layout()
        let theme = AppConfig.shared.theme
        let footerButtons = subviews.compactMap { $0 as? IconButton }.filter { $0.identifier?.rawValue.hasPrefix("footer") == true }
        for button in footerButtons {
            button.tint = theme.textSecondary
            button.hoverTint = theme.textPrimary
            button.hoverFill = theme.hover
        }
        if let first = footerButtons.first {
            first.frame = NSRect(x: 10, y: bounds.height - 34, width: 26, height: 26)
        }
        for (i, button) in footerButtons.dropFirst().reversed().enumerated() {
            button.frame = NSRect(x: bounds.width - 36 - CGFloat(i) * 28, y: bounds.height - 34, width: 26, height: 26)
        }
        addTabButton.tint = theme.textTertiary
        addTabButton.hoverTint = theme.textPrimary
        addTabButton.hoverFill = theme.hover
        rebuildRows()
    }

    private func rebuildRows() {
        rows = []
        guard let navigator else { return }
        var y = topInset + 2
        let rowHeight: CGFloat = 30
        let width = bounds.width - 16
        rows.append((.host, NSRect(x: 8, y: y, width: width, height: 24)))
        y += 26
        for session in Workspace.shared.sessions {
            let isCurrent = session === navigator.session
            let expanded = isCurrent && !collapsed.contains(session.id)
            rows.append((.session(session, expanded: expanded), NSRect(x: 8, y: y, width: width, height: 26)))
            y += 28
            if expanded {
                for (i, tab) in session.tabs.enumerated() {
                    rows.append((.tab(tab, index: i), NSRect(x: 8, y: y, width: width, height: rowHeight)))
                    y += rowHeight + 2
                }
                y += 6
            }
        }
        if let index = rows.firstIndex(where: { if case .session(let s, _) = $0.0 { s === navigator.session } else { false } }) {
            let frame = rows[index].1
            addTabButton.frame = NSRect(x: frame.maxX - 48, y: frame.midY - 11, width: 22, height: 22)
            addTabButton.isHidden = false
        } else {
            addTabButton.isHidden = true
        }
        positionPill()
        needsDisplay = true
    }

    private func positionPill() {
        let selected = rows.first { row in
            if case .tab(_, let i) = row.0 { return i == navigator?.session.selectedIndex }
            return false
        }
        guard let (row, frame) = selected, case .tab(let tab, _) = row else {
            pill.isHidden = true
            lastSelectedTab = nil
            return
        }
        let id = ObjectIdentifier(tab)
        let moved = lastSelectedTab != nil && lastSelectedTab != id && !pill.isHidden
        pill.isHidden = false
        if moved {
            Motion.animate(0.32) {
                pill.animator().frame = frame
                pillGlass?.animator().frame = NSRect(origin: .zero, size: frame.size)
            }
        } else {
            pill.frame = frame
            pillGlass?.frame = pill.bounds
        }
        lastSelectedTab = id
    }

    override func drawContent(_ dirtyRect: NSRect) {
        loaderSpots.begin()
        defer { loaderSpots.end() }
        drawnLoader = Preferences.shared.tabLoader
        let theme = AppConfig.shared.theme
        if rows.isEmpty { rebuildRows() }
        for (i, (row, frame)) in rows.enumerated() {
            // A spinner tick redraws only the dots; see `TabStripView.drawContent`.
            if StatusIndicator.hostAnimatesWorking, !frame.intersects(dirtyRect) { continue }
            switch row {
            case .host:
                let text = NSAttributedString(string: "This Mac", attributes: [
                    .font: NSFont.systemFont(ofSize: 11, weight: .semibold), .foregroundColor: theme.textTertiary,
                ])
                text.draw(at: NSPoint(x: frame.minX + 6, y: frame.midY - 7))
            case .session(let session, let expanded):
                if i == hoverRow { theme.hover.setFill(); NSBezierPath(roundedRect: frame, xRadius: 7, yRadius: 7).fill() }
                let current = session === navigator?.session
                let name = NSAttributedString(string: session.name, attributes: [
                    .font: NSFont.systemFont(ofSize: 12, weight: current ? .semibold : .medium),
                    .foregroundColor: current ? theme.textSecondary.withAlphaComponent(0.9) : theme.textSecondary,
                ])
                name.draw(with: NSRect(x: frame.minX + 8, y: frame.midY - 8, width: frame.width - 70, height: 16),
                          options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
                let chevron = SymbolImages.tinted(expanded ? "chevron.down" : "chevron.right", pointSize: 9, weight: .semibold,
                                                  color: theme.textTertiary)
                chevron?.drawUpright(at: NSPoint(x: frame.maxX - 18, y: frame.midY - 5))
                if !current, session.activity.urgency >= PaneActivity.working.urgency {
                    StatusIndicator.draw(session.activity, in: NSRect(x: frame.maxX - 38, y: frame.midY - 5, width: 10, height: 10), theme: theme, phase: phase, spots: loaderSpots)
                }
            case .tab(let tab, let index):
                let selected = index == navigator?.session.selectedIndex
                let path = NSBezierPath(roundedRect: frame, xRadius: 8, yRadius: 8)
                if !selected, i == hoverRow {
                    theme.hover.setFill()
                    path.fill()
                }
                let icon = NSRect(x: frame.minX + 8, y: frame.midY - 9, width: 18, height: 18)
                let stack = tab.icons
                IconStack.draw(stack, at: icon.origin, size: 18, theme: theme)
                let textX = icon.minX + IconStack.width(count: stack.count, size: 18) + 8
                let working = tab.activity == .working
                if tab.activity != .idle, !working {
                    StatusIndicator.draw(tab.activity, in: NSRect(x: icon.maxX - 6, y: icon.minY - 3, width: 9, height: 9), theme: theme, phase: phase)
                }
                if working {
                    StatusIndicator.drawWorking(in: NSRect(x: frame.maxX - 10 - StatusIndicator.dotsWidth, y: frame.minY, width: StatusIndicator.dotsWidth, height: frame.height),
                                             color: theme.textSecondary, phase: phase, spots: loaderSpots)
                }
                let text = NSMutableAttributedString(string: tab.title, attributes: [
                    .font: NSFont.systemFont(ofSize: 12.5, weight: .medium),
                    .foregroundColor: selected ? theme.textPrimary : theme.textPrimary.withAlphaComponent(0.8),
                ])
                if let detail = tab.detail {
                    text.append(NSAttributedString(string: " " + detail, attributes: [
                        .font: NSFont.systemFont(ofSize: 12.5), .foregroundColor: theme.textSecondary,
                    ]))
                }
                text.draw(with: NSRect(x: textX, y: frame.midY - 8, width: frame.maxX - textX - (working ? 14 + StatusIndicator.dotsWidth : 8), height: 16),
                          options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
            }
        }
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        removeOwnTrackingAreas()
        addTrackingArea(NSTrackingArea(rect: bounds, options: [.mouseMoved, .mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self))
    }

    private func rowIndex(at point: NSPoint) -> Int? {
        rows.firstIndex { $0.1.contains(point) }
    }

    // MARK: Pane drops

    /// The tab row under `point` (in this view) and its frame.
    func tab(at point: NSPoint) -> (tab: TerminalTab, frame: NSRect)? {
        guard let index = rowIndex(at: point), case .tab(let tab, _) = rows[index].0 else { return nil }
        return (tab, rows[index].1)
    }

    /// The frame of `tab`'s row, if the sidebar shows it.
    func rowFrame(for tab: TerminalTab) -> NSRect? {
        for (row, frame) in rows { if case .tab(let shown, _) = row, shown === tab { return frame } }
        return nil
    }

    /// Where a new tab would appear: below the last row.
    var newTabDropFrame: NSRect {
        let last = rows.last?.1 ?? NSRect(x: 8, y: 0, width: bounds.width - 16, height: 0)
        return NSRect(x: last.minX, y: last.maxY + 2, width: last.width, height: 30)
    }

    override func mouseMoved(with event: NSEvent) {
        let row = rowIndex(at: convert(event.locationInWindow, from: nil))
        if row != hoverRow { hoverRow = row; needsDisplay = true }
    }

    override func mouseExited(with event: NSEvent) {
        hoverRow = nil
        needsDisplay = true
    }

    /// A tab row being pressed, which lifts out when dragged.
    private var press: (tab: TerminalTab, point: NSPoint, selectionBefore: Int?)?

    override func mouseDragged(with event: NSEvent) {
        if let drag = PaneDrag.current { return drag.update(event) }
        guard let press else { return }
        let point = convert(event.locationInWindow, from: nil)
        guard hypot(point.x - press.point.x, point.y - press.point.y) > 6 else { return }
        self.press = nil
        navigator?.showOtherTab(than: press.tab, preferring: press.selectionBefore)
        let tab = press.tab
        PaneDrag.begin(tab: tab, event: event)
        PaneDrag.current?.onCancel = { [weak self] in
            guard let navigator = self?.navigator, let index = navigator.session.tabs.firstIndex(where: { $0 === tab }) else { return }
            navigator.selectTab(at: index)
        }
        PaneDrag.current?.update(event)
    }

    override func mouseUp(with event: NSEvent) {
        press = nil
        PaneDrag.current?.finish(event)
    }

    override func mouseDown(with event: NSEvent) {
        press = nil
        let point = convert(event.locationInWindow, from: nil)
        // The band above the rows is titlebar: drag the window, double-click to zoom.
        if point.y < topInset { return (window as? TermyWindow)?.titlebarMouseDown(event) ?? () }
        guard let index = rowIndex(at: point) else { return }
        switch rows[index].0 {
        case .host:
            break
        case .session(let session, _):
            if session === navigator?.session {
                if collapsed.contains(session.id) { collapsed.remove(session.id) } else { collapsed.insert(session.id) }
                rebuildRows()
            } else {
                onSelectSession?(session)
            }
        case .tab(let tab, let i):
            press = (tab, convert(event.locationInWindow, from: nil), navigator?.session.selectedIndex)
            navigator?.selectTab(at: i)
            if event.clickCount == 2 { navigator?.run(.renameTab) }
        }
    }

    override func menu(for event: NSEvent) -> NSMenu? {
        guard let index = rowIndex(at: convert(event.locationInWindow, from: nil)) else { return nil }
        let menu = NSMenu()
        switch rows[index].0 {
        case .tab(_, let i):
            navigator?.selectTab(at: i)
            [Command.renameTab, .newTab, .moveTabToNewWindow, .closeTab].forEach { menu.addItem($0.menuItem()) }
        case .session(let session, _):
            if session !== navigator?.session { onSelectSession?(session) }
            [Command.renameSession, .newSession, .closeSession].forEach { menu.addItem($0.menuItem()) }
        case .host:
            return nil
        }
        return menu
    }
}
