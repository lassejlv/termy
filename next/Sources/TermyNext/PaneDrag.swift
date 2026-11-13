import AppKit
import TermyNextCore

/// Where a dragged pane lands.
enum PaneDropTarget {
    /// Next to another pane, on the side the pointer is nearest.
    case split(PaneView, Direction)
    /// In place of another pane in the same tab.
    case swap(PaneView)
    /// Into an existing tab, next to its focused pane.
    case tab(TerminalTab)
    /// A new tab of its own.
    case newTab
    /// A new window, outside every Termy Next window.
    case newWindow

    func same(as other: PaneDropTarget?) -> Bool {
        switch (self, other) {
        case (.split(let a, let x), .split(let b, let y)?): a === b && x == y
        case (.swap(let a), .swap(let b)?): a === b
        case (.tab(let a), .tab(let b)?): a === b
        case (.newTab, .newTab?), (.newWindow, .newWindow?): true
        default: false
        }
    }

    var hint: String {
        switch self {
        case .split: "Split"
        case .swap: "Swap"
        case .tab: "Move to Tab"
        case .newTab: "New Tab"
        case .newWindow: "New Window"
        }
    }
}

/// Dragging a pane by its header: the pane lifts into a card that follows
/// the pointer over every window, each window shows where it would land,
/// and dropping it moves the pane there, outside any window opens a new one.
/// Esc cancels.
@MainActor
final class PaneDrag {
    private(set) static var current: PaneDrag?

    let pane: PaneView
    /// Set when a whole tab of several panes is being dragged; `pane` is then
    /// its focused pane, shown on the card.
    let tab: TerminalTab?
    /// Runs when the drag ends without moving anything, such as to show
    /// again a tab that was pulled out of the tab bar.
    var onCancel: (() -> Void)?
    private let ghost: NSPanel
    private let card: PaneDragCard
    private let grab: NSPoint
    private var target: (controller: WindowController?, drop: PaneDropTarget)?
    private var springTimer: Timer?
    private var keyMonitor: Any?
    /// Termy Next windows front to back, read once: asking the window server
    /// for the order on every mouse event is what made the card lag.
    private var windows: [WindowController] = []
    private var lastEvaluated: (point: NSPoint, time: CFTimeInterval)?
    private var evaluationPending = false

    /// Starts dragging `pane`; `event` is the drag that crossed the threshold.
    static func begin(pane: PaneView, event: NSEvent) {
        guard current == nil, pane.window != nil else { return }
        current = PaneDrag(pane: pane, tab: nil, event: event)
    }

    /// Starts dragging a tab pulled out of the tab bar or sidebar. A tab with
    /// one pane drags just like that pane.
    static func begin(tab: TerminalTab, event: NSEvent) {
        guard current == nil, let pane = tab.focusedPane ?? tab.allPanes.first else { return }
        current = PaneDrag(pane: pane, tab: tab.allPanes.count > 1 ? tab : nil, event: event)
    }

    private init(pane: PaneView, tab: TerminalTab?, event: NSEvent) {
        self.pane = pane
        self.tab = tab
        card = PaneDragCard(pane: pane, title: tab?.title, badge: tab.map { "\($0.allPanes.count) panes" })
        let size = card.frame.size
        // Hold the card where the header was grabbed, scaled to the card.
        if tab == nil, pane.window != nil {
            let inPane = pane.convert(event.locationInWindow, from: nil)
            grab = NSPoint(x: min(max(inPane.x / max(pane.bounds.width, 1), 0.1), 0.9) * size.width, y: 16)
        } else {
            grab = NSPoint(x: size.width / 2, y: 16)
        }
        ghost = NSPanel(contentRect: NSRect(origin: .zero, size: size), styleMask: [.borderless, .nonactivatingPanel],
                        backing: .buffered, defer: false)
        ghost.isOpaque = false
        ghost.backgroundColor = .clear
        ghost.hasShadow = true
        ghost.level = .floating
        ghost.ignoresMouseEvents = true
        ghost.isReleasedWhenClosed = false
        ghost.contentView = card
        ghost.animationBehavior = .none
        windows = AppDelegate.shared.controllersFrontToBack
        move(to: NSEvent.mouseLocation)
        ghost.orderFrontRegardless()
        Motion.appear(card, scale: 0.92, duration: 0.16)
        if tab == nil, pane.window != nil {
            NSAnimationContext.runAnimationGroup { context in
                context.duration = 0.15
                pane.animator().alphaValue = 0.35
            }
        }
        NSCursor.closedHand.push()
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
            guard event.keyCode == 53 else { return event }
            MainActor.assumeIsolated { PaneDrag.current?.cancel() }
            return nil
        }
    }

    private func move(to screenPoint: NSPoint) {
        ghost.setFrameOrigin(NSPoint(x: screenPoint.x - grab.x, y: screenPoint.y - ghost.frame.height + grab.y))
    }

    // MARK: Tracking

    /// Moves the card with the pointer straight away; working out the drop
    /// target waits for the next display frame, at most once per frame.
    func update(_ event: NSEvent? = nil) {
        move(to: NSEvent.mouseLocation)
        let now = CACurrentMediaTime()
        if let last = lastEvaluated, now - last.time < 1.0 / 60 {
            guard !evaluationPending else { return }
            evaluationPending = true
            DispatchQueue.main.asyncAfter(deadline: .now() + (1.0 / 60 - (now - last.time))) { [weak self] in
                guard let self, PaneDrag.current === self else { return }
                self.evaluationPending = false
                self.evaluateTarget()
            }
            return
        }
        evaluateTarget()
    }

    private func evaluateTarget() {
        let point = NSEvent.mouseLocation
        lastEvaluated = (point, CACurrentMediaTime())
        let found = target(at: point)
        guard !(found?.drop.same(as: target?.drop) ?? (target == nil)) || found?.controller !== target?.controller else { return }
        target?.controller?.showDropIndicator(nil)
        target = found
        found?.controller?.showDropIndicator(found?.drop)
        card.hint = found?.drop.hint
        scheduleSpringLoad()
    }

    /// Resting on a tab for a moment switches to it, so the pane can be
    /// placed among that tab's panes, even if the pointer stops moving.
    private func scheduleSpringLoad() {
        springTimer?.invalidate()
        springTimer = nil
        guard case .tab(let tab)? = target?.drop, let controller = target?.controller else { return }
        springTimer = Timer.scheduledTimer(withTimeInterval: 0.6, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self, PaneDrag.current === self, case .tab(let still)? = self.target?.drop, still === tab else { return }
                controller.showTabForDrop(tab)
                self.target = nil
                self.evaluateTarget()
            }
        }
    }

    /// The window under `point` and where the pane would land in it; a new
    /// window when the point is outside all of them.
    private func target(at point: NSPoint) -> (controller: WindowController?, drop: PaneDropTarget)? {
        windows.removeAll { $0.window?.isVisible != true }
        for controller in windows {
            guard let window = controller.window, window.isVisible, window.frame.contains(point) else { continue }
            return controller.dropTarget(atScreenPoint: point, dragging: pane, tab: tab).map { (controller, $0) }
        }
        let overOtherWindow = NSApp.windows.contains { $0.isVisible && $0 !== ghost && $0.frame.contains(point) && $0.level == .normal }
        return overOtherWindow ? nil : (nil, .newWindow)
    }

    // MARK: Ending

    func finish(_ event: NSEvent) {
        let landing = target
        end()
        guard let landing else {
            onCancel?()
            return restorePane(animated: true)
        }
        restorePane(animated: false)
        if case .newWindow = landing.drop {
            if let tab {
                AppDelegate.shared.moveTabToNewWindow(tab, at: NSEvent.mouseLocation)
            } else {
                AppDelegate.shared.movePaneToNewWindow(pane, at: NSEvent.mouseLocation)
            }
        } else if let controller = landing.controller {
            if let tab {
                controller.acceptTabDrop(tab, at: landing.drop)
            } else {
                controller.acceptDrop(pane, at: landing.drop)
            }
        }
    }

    func cancel() {
        end()
        onCancel?()
        restorePane(animated: true)
    }

    private func end() {
        springTimer?.invalidate()
        target?.controller?.showDropIndicator(nil)
        if let keyMonitor { NSEvent.removeMonitor(keyMonitor) }
        keyMonitor = nil
        NSCursor.pop()
        Self.current = nil
        let ghost = self.ghost
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.14
            ghost.animator().alphaValue = 0
        }, completionHandler: { ghost.orderOut(nil) })
    }

    private func restorePane(animated: Bool) {
        let pane = self.pane
        if animated {
            NSAnimationContext.runAnimationGroup { context in
                context.duration = 0.18
                pane.animator().alphaValue = 1
            }
        } else {
            pane.alphaValue = 1
        }
    }
}

/// The card that follows the pointer: the pane's title and a miniature of
/// its text, with a pill naming where it will land.
@MainActor
final class PaneDragCard: NSView {
    var hint: String? { didSet { if hint != oldValue { needsDisplay = true } } }
    private let title: String
    private let badge: String?
    private let program: String?
    private let executable: String?
    private let lines: [String]

    init(pane: PaneView, title: String? = nil, badge: String? = nil) {
        self.title = title ?? pane.label.name
        self.badge = badge
        program = pane.foregroundProgram
        executable = pane.foregroundExecutable
        // A tab pulled out of the tab bar is hidden before its card is made, and
        // one pressed out of the background may not have caught up yet: hidden
        // panes take no frames, so take the latest now.
        pane.terminal.session.poll()
        let frame = pane.terminal.session.frame
        let rows = (0..<frame.rows).map { frame.text(row: $0).replacingOccurrences(of: "\\s+$", with: "", options: .regularExpression) }
        // The last screenful of text that has anything on it.
        let used = rows.lastIndex { !$0.isEmpty } ?? 0
        // A frame never filled (its terminal failed to update) has no rows.
        lines = rows.isEmpty ? [] : Array(rows[max(0, used - 13)...used])
        let aspect = pane.bounds.height / max(pane.bounds.width, 1)
        let width: CGFloat = 300
        super.init(frame: NSRect(x: 0, y: 0, width: width, height: min(230, max(150, width * aspect))))
        wantsLayer = true
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }

    override func draw(_ dirtyRect: NSRect) {
        let theme = AppConfig.shared.theme
        let card = bounds.insetBy(dx: 1, dy: 1)
        let shape = NSBezierPath(roundedRect: card, xRadius: 12, yRadius: 12)
        theme.paneFill.withAlphaComponent(0.96).setFill()
        shape.fill()
        theme.paneStroke.withAlphaComponent(0.6).setStroke()
        shape.lineWidth = 1
        shape.stroke()

        ProgramIcon.draw(program: program, executable: executable, in: NSRect(x: 12, y: 9, width: 16, height: 16), theme: theme)
        let titleText = NSAttributedString(string: title, attributes: [
            .font: NSFont.systemFont(ofSize: 12, weight: .semibold), .foregroundColor: theme.textPrimary,
        ])
        var titleWidth = bounds.width - 46
        if let badge {
            let text = NSAttributedString(string: badge, attributes: [
                .font: NSFont.systemFont(ofSize: 10, weight: .semibold), .foregroundColor: theme.textSecondary,
            ])
            let size = text.size()
            let pill = NSRect(x: bounds.width - size.width - 24, y: 8, width: size.width + 12, height: size.height + 4)
            theme.selected.setFill()
            NSBezierPath(roundedRect: pill, xRadius: pill.height / 2, yRadius: pill.height / 2).fill()
            text.draw(at: NSPoint(x: pill.minX + 6, y: pill.minY + 2))
            titleWidth = pill.minX - 40
        }
        titleText.draw(with: NSRect(x: 34, y: 10, width: titleWidth, height: 16),
                       options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
        theme.paneStroke.setFill()
        NSRect(x: 1, y: 33, width: bounds.width - 2, height: 1).fill()

        let mono = NSFont.monospacedSystemFont(ofSize: 8.5, weight: .regular)
        let attributes: [NSAttributedString.Key: Any] = [.font: mono, .foregroundColor: theme.textSecondary]
        var y: CGFloat = 40
        for line in lines where y < bounds.height - 14 {
            NSAttributedString(string: line, attributes: attributes)
                .draw(with: NSRect(x: 12, y: y, width: bounds.width - 24, height: 12), options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
            y += 11
        }

        if let hint {
            let text = NSAttributedString(string: hint, attributes: [
                .font: NSFont.systemFont(ofSize: 11, weight: .semibold), .foregroundColor: theme.selectionText,
            ])
            let size = text.size()
            let pill = NSRect(x: bounds.width - size.width - 30, y: bounds.height - size.height - 18,
                              width: size.width + 18, height: size.height + 8)
            theme.selectionFill.setFill()
            NSBezierPath(roundedRect: pill, xRadius: pill.height / 2, yRadius: pill.height / 2).fill()
            text.draw(at: NSPoint(x: pill.minX + 9, y: pill.minY + 4))
        }
    }
}

/// The highlight a window draws where a dragged pane would land.
@MainActor
final class DropIndicatorView: NSView {
    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.cornerRadius = 10
        layer?.cornerCurve = .continuous
        layer?.borderWidth = 1.5
        applyTheme()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    func applyTheme() {
        let theme = AppConfig.shared.theme
        layer?.backgroundColor = theme.selectionFill.withAlphaComponent(0.18).cgColor
        layer?.borderColor = theme.selectionFill.withAlphaComponent(0.8).cgColor
    }
}
