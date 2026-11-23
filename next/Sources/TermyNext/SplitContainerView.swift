import AppKit
import TermyNextCore

/// Lays out one tab's panes as cards with gaps between them, handles divider
/// drags, stacks floating panes on top, and animates every layout change.
@MainActor
final class SplitContainerView: NSView {
    var tab: TerminalTab? {
        didSet {
            guard oldValue !== tab else { return }
            reloadPanes()
        }
    }

    private var dragging: SplitTree<UUID>.Divider?
    private var didDragDivider = false
    /// Monotonic counter so overlapping `animateLayout` completions never
    /// clobber a newer animation's `terminalSizeOverride` or visibility state.
    private var layoutAnimationGeneration: UInt64 = 0
    private var isAnimatingLayout = false
    private var lastAnimatedBounds: NSRect = .zero

    override var isFlipped: Bool { true }
    override var mouseDownCanMoveWindow: Bool { false }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    var gap: CGFloat { Preferences.shared.paneStyle == .compact ? 1 : 8 }

    /// Adds the tab's pane views and removes any that left it.
    func reloadPanes() {
        let wanted = Set(tab?.allPanes.map(\.id) ?? [])
        for case let pane as PaneView in subviews where !wanted.contains(pane.id) && !pane.isClosing {
            pane.removeFromSuperview()
        }
        let present = Set(subviews.compactMap { ($0 as? PaneView)?.id })
        for pane in tab?.allPanes ?? [] where !present.contains(pane.id) {
            addSubview(pane)
        }
        // Floating panes draw above the layout.
        for pane in tab?.allPanes ?? [] where pane.isFloating {
            addSubview(pane, positioned: .above, relativeTo: nil)
        }
        needsLayout = true
        needsDisplay = true
    }

    /// Where each pane should be and whether it shows, for the current bounds.
    private func targets(in area: NSRect) -> [(PaneView, NSRect, Bool)] {
        guard let tab else { return [] }
        if let zoomed = tab.zoomedID, tab.panes[zoomed] != nil {
            return tab.allPanes.map { pane in
                pane.id == zoomed ? (pane, area, true) : (pane, pane.frame, false)
            }
        }
        let frames = tab.tree.frames(in: area, gap: gap)
        return tab.allPanes.map { pane in
            if let frame = frames[pane.id] { return (pane, frame.integral, true) }
            if tab.isMinimized(pane.id) { return (pane, pane.frame, false) }
            let unit = tab.floating[pane.id] ?? CGRect(x: 0.2, y: 0.15, width: 0.6, height: 0.62)
            let frame = NSRect(x: area.minX + unit.minX * area.width, y: area.minY + unit.minY * area.height,
                               width: unit.width * area.width, height: unit.height * area.height).integral
            return (pane, frame, true)
        }
    }

    private func isDraggedSource(_ pane: PaneView) -> Bool {
        PaneDrag.current?.pane === pane && PaneDrag.current?.tab == nil
    }

    override func layout() {
        super.layout()
        guard let tab else { return }
        if isAnimatingLayout {
            // If bounds didn't change (e.g. a background workspace/process tick),
            // let the active layout animation keep running undisturbed.
            guard bounds != lastAnimatedBounds else { return }
            layoutAnimationGeneration &+= 1
            isAnimatingLayout = false
            for pane in tab.allPanes { pane.terminalSizeOverride = nil }
        }
        for (pane, frame, visible) in targets(in: bounds) {
            pane.isZoomed = tab.zoomedID == pane.id
            pane.isHidden = !visible
            if !isDraggedSource(pane) { pane.alphaValue = 1 }
            if visible { pane.frame = frame }
        }
        updateDimming()
        if dragging == nil { window?.invalidateCursorRects(for: self) }
        needsDisplay = true
    }

    func updateDimming() {
        guard let tab else { return }
        let multiple = tab.allPanes.count > 1 && tab.zoomedID == nil
        for pane in tab.allPanes {
            pane.isDimmed = multiple && pane.id != tab.focusedID
        }
    }

    /// Moves every pane to its new place with motion: siblings slide, `appearing`
    /// grows in, panes leaving a zoom fade. Terminal grids resize once, up front.
    func animateLayout(appearing: PaneView? = nil, duration: TimeInterval = Motion.standard) {
        guard let tab, window != nil, !Motion.reduced else {
            layoutAnimationGeneration &+= 1
            isAnimatingLayout = false
            needsLayout = true
            layoutSubtreeIfNeeded()
            if let appearing { Motion.appear(appearing) }
            return
        }
        layoutAnimationGeneration &+= 1
        let generation = layoutAnimationGeneration
        isAnimatingLayout = true
        lastAnimatedBounds = bounds

        let plan = targets(in: bounds)
        for (pane, frame, visible) in plan where visible {
            pane.terminalSizeOverride = pane.terminalRect(forCard: frame.size).size
            if pane.isHidden {
                // Coming back from behind a zoomed pane: start invisible in place.
                pane.frame = frame
                pane.alphaValue = 0
                pane.isHidden = false
            }
        }
        if let appearing, let frame = plan.first(where: { $0.0 === appearing })?.1 {
            Motion.instantly {
                appearing.frame = frame
                appearing.layoutSubtreeIfNeeded()
            }
        }
        Motion.animate(duration, {
            for (pane, frame, visible) in plan {
                pane.isZoomed = tab.zoomedID == pane.id
                guard pane !== appearing else { continue }
                if visible {
                    pane.animator().frame = frame
                    if !isDraggedSource(pane) { pane.animator().alphaValue = 1 }
                    pane.layoutSubtreeIfNeeded()
                } else {
                    pane.animator().alphaValue = 0
                }
            }
        }, completion: { [weak self] in
            guard let self, self.layoutAnimationGeneration == generation else { return }
            self.isAnimatingLayout = false
            for (pane, _, visible) in plan {
                pane.terminalSizeOverride = nil
                pane.isHidden = !visible
                if !self.isDraggedSource(pane) { pane.alphaValue = 1 }
                pane.needsLayout = true
            }
            self.needsLayout = true
        })
        if let appearing { Motion.appear(appearing, scale: 0.92) }
        updateDimming()
        window?.invalidateCursorRects(for: self)
        needsDisplay = true
    }

    /// Animates `pane` out, then removes it.
    func animateRemoval(of pane: PaneView, completion: @escaping () -> Void) {
        pane.isClosing = true
        Motion.disappear(pane) {
            pane.removeFromSuperview()
            pane.layer?.removeAllAnimations()
            completion()
        }
    }

    /// Hairline dividers for the compact style.
    override func draw(_ dirtyRect: NSRect) {
        guard let tab, Preferences.shared.paneStyle == .compact, tab.zoomedID == nil else { return }
        let theme = AppConfig.shared.theme
        theme.paneStroke.withAlphaComponent(0.18).setFill()
        for divider in tab.tree.dividers(in: bounds, gap: gap) {
            let line = divider.axis == .horizontal
                ? NSRect(x: divider.frame.midX - 0.5, y: divider.frame.minY, width: 1, height: divider.frame.height)
                : NSRect(x: divider.frame.minX, y: divider.frame.midY - 0.5, width: divider.frame.width, height: 1)
            line.fill()
        }
    }

    var paneFrames: [UUID: CGRect] {
        guard let tab else { return [:] }
        return tab.tree.frames(in: bounds, gap: gap)
    }

    // MARK: Divider dragging

    private var dividers: [SplitTree<UUID>.Divider] {
        guard let tab, tab.zoomedID == nil else { return [] }
        return tab.tree.dividers(in: bounds, gap: gap)
    }

    /// How thick the area that grabs a divider is, whatever the gap: it
    /// reaches past the gap into the panes' edges, so the 1 pt compact
    /// hairline is as easy to grab as the comfortable gap.
    static let dividerHitThickness: CGFloat = 12

    /// The area that grabs `divider`: its gap, widened across its axis.
    private func hitRect(_ divider: SplitTree<UUID>.Divider) -> NSRect {
        let grow = max(0, (Self.dividerHitThickness - (divider.axis == .horizontal ? divider.frame.width : divider.frame.height)) / 2)
        return divider.axis == .horizontal ? divider.frame.insetBy(dx: -grow, dy: 0) : divider.frame.insetBy(dx: 0, dy: -grow)
    }

    /// The divider under `point`; the nearest one where grab areas overlap.
    private func divider(at point: NSPoint) -> SplitTree<UUID>.Divider? {
        dividers.filter { hitRect($0).contains(point) }.min { a, b in
            func distance(_ d: SplitTree<UUID>.Divider) -> CGFloat {
                d.axis == .horizontal ? abs(point.x - d.frame.midX) : abs(point.y - d.frame.midY)
            }
            return distance(a) < distance(b)
        }
    }

    override func resetCursorRects() {
        for divider in dividers {
            addCursorRect(hitRect(divider), cursor: divider.axis == .horizontal ? .resizeLeftRight : .resizeUpDown)
        }
    }

    override func hitTest(_ point: NSPoint) -> NSView? {
        let local = convert(point, from: superview)
        // Floating panes sit above the layout and keep their own clicks.
        if let tab, tab.allPanes.contains(where: { $0.isFloating && !$0.isHidden && $0.frame.contains(local) }) {
            return super.hitTest(point)
        }
        if divider(at: local) != nil { return self }
        return super.hitTest(point)
    }

    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        dragging = divider(at: point)
        didDragDivider = false
        if event.clickCount == 2, let divider = dragging, let tab {
            // End the drag immediately so tiny pointer movement before mouseUp
            // doesn't override the equalized 0.5 ratio.
            dragging = nil
            tab.tree = tab.tree.settingRatio(0.5, at: divider.path)
            animateLayout()
            Workspace.shared.changed()
        }
    }

    override func mouseDragged(with event: NSEvent) {
        guard let divider = dragging, let tab else { return }
        didDragDivider = true
        let point = convert(event.locationInWindow, from: nil)
        let c = divider.container
        let ratio = divider.axis == .horizontal
            ? (point.x - c.minX - gap / 2) / max(c.width - gap, 1)
            : (point.y - c.minY - gap / 2) / max(c.height - gap, 1)
        tab.tree = tab.tree.settingRatio(ratio, at: divider.path)
        needsLayout = true
        layoutSubtreeIfNeeded()
    }

    override func mouseUp(with event: NSEvent) {
        let moved = didDragDivider
        dragging = nil
        didDragDivider = false
        if moved {
            window?.invalidateCursorRects(for: self)
            Workspace.shared.changed()
        }
    }

    /// Brings a floating pane (and its layer) above other floating panes.
    func raiseFloating(_ pane: PaneView) {
        guard let tab, pane.isFloating, let layer = tab.layerID(of: pane.id) else { return }
        tab.raiseLayer(layer)
        for id in tab.layerPanes(layer) {
            if let member = tab.panes[id], member.superview === self {
                addSubview(member, positioned: .above, relativeTo: nil)
            }
        }
    }

    /// Moves a floating pane (and its layer) by a drag on its header without
    /// re-laying out all tiled panes or invalidating divider cursor rects.
    func moveFloating(_ pane: PaneView, by delta: NSPoint) {
        guard let tab, let layer = tab.layerID(of: pane.id), bounds.width > 0, bounds.height > 0 else { return }
        // A pane in a layer with others carries them along. Window
        // coordinates grow upward; this view is flipped.
        tab.moveLayer(containing: pane.id, dx: delta.x / bounds.width, dy: -delta.y / bounds.height)
        let area = bounds
        for id in tab.layerPanes(layer) {
            guard let member = tab.panes[id], let unit = tab.floating[id], !tab.isMinimized(id) else { continue }
            let frame = NSRect(x: area.minX + unit.minX * area.width, y: area.minY + unit.minY * area.height,
                               width: unit.width * area.width, height: unit.height * area.height).integral
            Motion.instantly { member.frame = frame }
        }
    }
}
