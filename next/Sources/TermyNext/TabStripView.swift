import AppKit

/// What the tab strip and sidebar need from the window controller.
@MainActor
protocol TabNavigator: AnyObject {
    var session: TerminalSessionGroup { get }
    func selectTab(at index: Int)
    func closeTab(at index: Int)
    func moveTab(from source: Int, to destination: Int)
    func run(_ command: Command)
    /// Shows a tab other than `tab`, preferring the one at `index`, so a tab
    /// being dragged out can be dropped into it.
    func showOtherTab(than tab: TerminalTab, preferring index: Int?)
}

/// Tabs in the titlebar (Tab Style: Above): a rounded track holding a capsule
/// per tab. The selected tab sits on a pill that slides between tabs; in
/// Liquid the pill is glass, as in Rex.
@MainActor
final class TabStripView: GlassPanel {
    weak var navigator: TabNavigator?
    private var hoverIndex: Int?
    private var hoverClose = false
    private var phase: CGFloat = 0
    private var dragIndex: Int?
    private var dragStart: NSPoint = .zero
    private var dragOffset: CGFloat = 0
    /// The tab that was showing before the press, to go back to when the
    /// pressed tab is pulled out to be dropped elsewhere.
    private var selectionBeforePress: Int?
    /// The pill under the selected tab: glass in Liquid, a filled capsule in Classic.
    private var pill: NSView?
    private var pillIsGlass = false
    private var lastSelected: Int?

    override var mouseDownCanMoveWindow: Bool { false }

    func advanceSpinner() {
        guard navigator?.session.tabs.contains(where: { $0.activity == .working }) == true else { return }
        phase = (phase - 24).truncatingRemainder(dividingBy: 360)
        needsDisplay = true
    }

    func computeFrames() -> [NSRect] {
        guard let tabs = navigator?.session.tabs, !tabs.isEmpty else { return [] }
        let height = bounds.height - 6
        let available = bounds.width - 6
        let preferred: CGFloat = 230
        let width = max(90, min(preferred, (available - CGFloat(tabs.count - 1) * 2) / CGFloat(tabs.count)))
        return tabs.indices.map { i in
            NSRect(x: 3 + CGFloat(i) * (width + 2), y: 3, width: width, height: height)
        }
    }

    /// The width the strip wants for its tabs, so the titlebar can size it.
    func preferredWidth(maximum: CGFloat) -> CGFloat {
        let count = CGFloat(navigator?.session.tabs.count ?? 1)
        return min(maximum, count * 232 + 6)
    }

    /// Restyles the track and pill for the interface style.
    func applyStyle() {
        let theme = AppConfig.shared.theme
        if pill == nil || pillIsGlass != theme.isLiquid {
            pill?.removeFromSuperview()
            let view: NSView
            if theme.isLiquid {
                view = GlassView(style: .regular, cornerRadius: 16, tint: theme.selectionGlassTint, interactive: true)
                (view as? GlassView)?.isCapsule = true
            } else {
                view = NSView()
                view.wantsLayer = true
                view.layer?.cornerCurve = .continuous
                view.layer?.shadowColor = NSColor.black.cgColor
                view.layer?.shadowOpacity = 0.25
                view.layer?.shadowRadius = 3.5
                view.layer?.shadowOffset = CGSize(width: 0, height: -1.5)
            }
            addSubview(view, positioned: .below, relativeTo: overlay)
            pill = view
            pillIsGlass = theme.isLiquid
            lastSelected = nil
        }
        if let glassPill = pill as? GlassView {
            glassPill.tint = theme.selectionGlassTint
        } else if let layer = pill?.layer {
            layer.backgroundColor = theme.terminalBackground.mixed(with: theme.foreground, by: theme.isDark ? 0.1 : 0).cgColor
            layer.borderColor = theme.foreground.withAlphaComponent(0.12).cgColor
            layer.borderWidth = 0.7
        }
        needsLayout = true
        needsDisplay = true
    }

    override func layout() {
        super.layout()
        positionPill(animated: lastSelected != nil && lastSelected != navigator?.session.selectedIndex)
    }

    /// Puts the pill under the selected tab, sliding when the selection moves.
    func positionPill(animated: Bool) {
        guard let navigator, let pill else { return }
        let frames = computeFrames()
        let index = navigator.session.selectedIndex
        guard frames.indices.contains(index) else { pill.isHidden = true; return }
        pill.isHidden = false
        var target = frames[index]
        if index == dragIndex { target.origin.x += dragOffset }
        pill.layer?.cornerRadius = target.height / 2
        if animated, !Motion.reduced, pill.frame != .zero {
            Motion.animate(0.36) {
                pill.animator().frame = target
                pill.layoutSubtreeIfNeeded()
            }
        } else {
            pill.frame = target
        }
        lastSelected = index
    }

    override func drawContent(_ dirtyRect: NSRect) {
        guard let navigator else { return }
        let theme = AppConfig.shared.theme
        let tabs = navigator.session.tabs
        let frames = computeFrames()

        if tabs.count > 1 {
            let track = NSBezierPath(roundedRect: bounds, xRadius: bounds.height / 2, yRadius: bounds.height / 2)
            theme.foreground.withAlphaComponent(theme.isDark ? 0.05 : 0.06).setFill()
            track.fill()
        }

        for (i, tab) in tabs.enumerated() {
            var frame = frames[i]
            if i == dragIndex { frame.origin.x += dragOffset }
            let selected = i == navigator.session.selectedIndex
            let radius = frame.height / 2
            if !selected, i == hoverIndex {
                theme.hover.setFill()
                NSBezierPath(roundedRect: frame, xRadius: radius, yRadius: radius).fill()
            } else if !selected, i > 0, i - 1 != navigator.session.selectedIndex, i - 1 != hoverIndex {
                theme.foreground.withAlphaComponent(0.12).setFill()
                NSRect(x: frame.minX - 1.5, y: frame.midY - 7, width: 1, height: 14).fill()
            }
            drawContent(tab, index: i, in: frame, selected: selected, theme: theme)
        }
    }

    private func drawContent(_ tab: TerminalTab, index: Int, in frame: NSRect, selected: Bool, theme: Theme) {
        let iconSize: CGFloat = 18
        let icon = NSRect(x: frame.minX + 8, y: frame.midY - iconSize / 2, width: iconSize, height: iconSize)
        let stack = tab.icons
        IconStack.draw(stack, at: icon.origin, size: iconSize, theme: theme)
        if tab.activity != .idle, tab.activity != .working {
            let badge = NSRect(x: icon.maxX - 7, y: icon.minY - 2, width: 9, height: 9)
            theme.terminalBackground.setFill()
            NSBezierPath(ovalIn: badge.insetBy(dx: -1.5, dy: -1.5)).fill()
            StatusIndicator.draw(tab.activity, in: badge.insetBy(dx: -2, dy: -2), theme: theme, phase: phase)
        }

        let showsClose = index == hoverIndex && (navigator?.session.tabs.count ?? 0) > 1
        // Working tabs end in Rex's three-dot loader; hovering swaps it for the close button.
        let showsDots = tab.activity == .working && !showsClose
        let textMaxX = frame.maxX - (showsClose ? 26 : showsDots ? StatusIndicator.dotsWidth + 18 : 12)
        let text = NSMutableAttributedString(string: tab.title, attributes: [
            .font: NSFont.systemFont(ofSize: 13, weight: .semibold),
            .foregroundColor: selected ? theme.textPrimary : theme.textPrimary.withAlphaComponent(0.75),
        ])
        if let detail = tab.detail {
            text.append(NSAttributedString(string: " " + detail, attributes: [
                .font: NSFont.systemFont(ofSize: 13), .foregroundColor: theme.textSecondary,
            ]))
        }
        let x = icon.minX + IconStack.width(count: stack.count, size: iconSize) + 8
        let rect = NSRect(x: x, y: frame.midY - 8.5, width: max(0, textMaxX - x), height: 17)
        drawFaded(text, in: rect)
        if showsDots {
            StatusIndicator.drawDots(in: NSRect(x: frame.maxX - 12 - StatusIndicator.dotsWidth, y: frame.minY, width: StatusIndicator.dotsWidth, height: frame.height),
                                     color: theme.textSecondary, phase: phase)
        }

        if showsClose {
            let close = closeRect(for: frame)
            if hoverClose {
                theme.hover.setFill()
                NSBezierPath(ovalIn: close.insetBy(dx: -3, dy: -3)).fill()
            }
            let config = NSImage.SymbolConfiguration(pointSize: 9, weight: .bold)
            if let image = NSImage(systemSymbolName: "xmark", accessibilityDescription: "Close Tab")?
                .withSymbolConfiguration(config)?.tinted(theme.textSecondary) {
                image.drawUpright(in: NSRect(x: close.midX - image.size.width / 2, y: close.midY - image.size.height / 2,
                                             width: image.size.width, height: image.size.height))
            }
        }
    }

    /// Draws text clipped to `rect`, fading out the last few points instead of an ellipsis.
    private func drawFaded(_ text: NSAttributedString, in rect: NSRect) {
        guard rect.width > 4 else { return }
        let needed = text.size().width
        NSGraphicsContext.saveGraphicsState()
        NSBezierPath(rect: rect).addClip()
        if needed > rect.width, let context = NSGraphicsContext.current?.cgContext {
            context.beginTransparencyLayer(auxiliaryInfo: nil)
            text.draw(at: NSPoint(x: rect.minX, y: rect.minY))
            let fade: CGFloat = 22
            let gradient = NSGradient(colors: [NSColor.black.withAlphaComponent(0), .black])
            context.setBlendMode(.destinationOut)
            gradient?.draw(in: NSRect(x: rect.maxX - fade, y: rect.minY, width: fade, height: rect.height), angle: 0)
            context.endTransparencyLayer()
        } else {
            text.draw(at: NSPoint(x: rect.minX, y: rect.minY))
        }
        NSGraphicsContext.restoreGraphicsState()
    }

    private func closeRect(for frame: NSRect) -> NSRect {
        NSRect(x: frame.maxX - 20, y: frame.midY - 6, width: 12, height: 12)
    }

    // MARK: Mouse

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: bounds, options: [.mouseMoved, .mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self))
    }

    private func index(at point: NSPoint) -> Int? {
        computeFrames().firstIndex { $0.contains(point) }
    }

    // MARK: Pane drops

    /// The tab under `point` (in this view), for dropping a pane into it.
    func tabIndex(at point: NSPoint) -> Int? { index(at: point) }

    func tabFrame(at index: Int) -> NSRect? {
        let frames = computeFrames()
        return frames.indices.contains(index) ? frames[index] : nil
    }

    /// Where a new tab would appear: just after the last one.
    var newTabDropFrame: NSRect {
        let last = computeFrames().last ?? NSRect(x: 0, y: 3, width: 0, height: bounds.height - 6)
        return NSRect(x: min(last.maxX + 2, bounds.width - 60), y: last.minY, width: 60, height: last.height)
    }

    override func mouseMoved(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        let index = index(at: point)
        let overClose = index.map { closeRect(for: computeFrames()[$0]).insetBy(dx: -3, dy: -3).contains(point) } ?? false
        if index != hoverIndex || overClose != hoverClose {
            hoverIndex = index
            hoverClose = overClose
            needsDisplay = true
        }
    }

    override func mouseExited(with event: NSEvent) {
        hoverIndex = nil
        hoverClose = false
        needsDisplay = true
    }

    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        guard let index = index(at: point) else {
            if event.clickCount == 2 { navigator?.run(.newTab) }
            return
        }
        if hoverClose, (navigator?.session.tabs.count ?? 0) > 1 {
            navigator?.closeTab(at: index)
            hoverClose = false
            return
        }
        if event.clickCount == 2 {
            navigator?.selectTab(at: index)
            navigator?.run(.renameTab)
            return
        }
        selectionBeforePress = navigator?.session.selectedIndex
        navigator?.selectTab(at: index)
        dragIndex = index
        dragStart = point
        dragOffset = 0
    }

    override func mouseDragged(with event: NSEvent) {
        if let drag = PaneDrag.current { return drag.update(event) }
        guard let from = dragIndex else { return }
        let point = convert(event.locationInWindow, from: nil)
        // Pulled away from the tab bar: the tab lifts out, to be split into
        // another tab's panes, merged into a tab, or opened in a window.
        if !bounds.insetBy(dx: -30, dy: -12).contains(point), let tabs = navigator?.session.tabs, tabs.indices.contains(from) {
            let tab = tabs[from]
            dragIndex = nil
            dragOffset = 0
            positionPill(animated: true)
            navigator?.showOtherTab(than: tab, preferring: selectionBeforePress)
            PaneDrag.begin(tab: tab, event: event)
            PaneDrag.current?.onCancel = { [weak self] in
                guard let navigator = self?.navigator, let index = navigator.session.tabs.firstIndex(where: { $0 === tab }) else { return }
                navigator.selectTab(at: index)
            }
            PaneDrag.current?.update(event)
            return
        }
        dragOffset = point.x - dragStart.x
        positionPill(animated: false)
        needsDisplay = true
    }

    override func mouseUp(with event: NSEvent) {
        if let drag = PaneDrag.current {
            drag.finish(event)
            return
        }
        defer {
            dragIndex = nil
            dragOffset = 0
            positionPill(animated: true)
            needsDisplay = true
        }
        guard let from = dragIndex, abs(dragOffset) > 8 else { return }
        let frames = computeFrames()
        let center = frames[from].midX + dragOffset
        let to = frames.firstIndex { center < $0.maxX } ?? frames.count - 1
        if to != from { navigator?.moveTab(from: from, to: to) }
    }

    override func otherMouseUp(with event: NSEvent) {
        // Middle-click closes a tab.
        if let index = index(at: convert(event.locationInWindow, from: nil)) { navigator?.closeTab(at: index) }
    }

    override func menu(for event: NSEvent) -> NSMenu? {
        guard let index = index(at: convert(event.locationInWindow, from: nil)) else { return nil }
        navigator?.selectTab(at: index)
        let menu = NSMenu()
        for command in [Command.renameTab, .newTab, .moveTabToNewWindow, .closeTab] {
            menu.addItem(command.menuItem())
        }
        return menu
    }
}
