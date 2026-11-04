import AppKit

/// The Activity popup (⌘. or the eye button): every program reporting
/// status through OSC 7501, across all sessions, so a blocked agent or a
/// finished build is one click away. Idle reports stay out, as in Rex.
@MainActor
final class ActivityView: NSView {
    var onSelect: ((TerminalSessionGroup, TerminalTab, PaneView) -> Void)?
    var onDismiss: (() -> Void)?
    private var rows: [(TerminalSessionGroup, TerminalTab, PaneView, NSRect)] = []
    private var hoverRow: Int?
    private var phase: CGFloat = 0
    private lazy var loaderSpots = LoaderSpots(host: self)
    private var drawnLoader = Preferences.shared.tabLoader
    private var isDismissing = false

    static let width: CGFloat = 300
    private let headerHeight: CGFloat = 36
    private let rowHeight: CGFloat = 40
    private let maxRows = 10

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        let theme = AppConfig.shared.theme
        appearance = theme.appearance
        layer?.cornerRadius = 12
        layer?.cornerCurve = .continuous
        layer?.backgroundColor = theme.popover.withAlphaComponent(0.97).cgColor
        layer?.borderColor = theme.foreground.withAlphaComponent(0.12).cgColor
        layer?.borderWidth = 0.7
        layer?.shadowColor = NSColor.black.cgColor
        layer?.shadowOpacity = 0.4
        layer?.shadowRadius = 18
        layer?.shadowOffset = CGSize(width: 0, height: -8)
        reload()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }
    override var mouseDownCanMoveWindow: Bool { false }
    override var acceptsFirstResponder: Bool { true }

    /// Panes doing something, most urgent first.
    private var activePanes: [(TerminalSessionGroup, TerminalTab, PaneView)] {
        var result: [(TerminalSessionGroup, TerminalTab, PaneView)] = []
        for session in Workspace.shared.sessions {
            for tab in session.tabs {
                for pane in tab.allPanes where pane.activity != .idle {
                    result.append((session, tab, pane))
                }
            }
        }
        return result.sorted { $0.2.activity.urgency > $1.2.activity.urgency }
    }

    var preferredHeight: CGFloat {
        let count = min(rows.count, maxRows)
        return headerHeight + (count == 0 ? 44 : CGFloat(count) * rowHeight + 8)
    }

    func reload() {
        rows = activePanes.prefix(maxRows).enumerated().map { i, entry in
            (entry.0, entry.1, entry.2, NSRect(x: 6, y: headerHeight + CGFloat(i) * rowHeight, width: Self.width - 12, height: rowHeight))
        }
        needsDisplay = true
    }

    func advanceSpinner() {
        guard rows.contains(where: { $0.2.activity == .working }) else { return }
        phase = (phase - 24).truncatingRemainder(dividingBy: 360)
        if StatusIndicator.hostAnimatesWorking || drawnLoader != Preferences.shared.tabLoader { needsDisplay = true }
    }

    override func layout() {
        super.layout()
        layer?.shadowPath = CGPath(roundedRect: bounds, cornerWidth: 12, cornerHeight: 12, transform: nil)
    }

    override func draw(_ dirtyRect: NSRect) {
        loaderSpots.begin()
        defer { loaderSpots.end() }
        drawnLoader = Preferences.shared.tabLoader
        let theme = AppConfig.shared.theme
        NSAttributedString(string: "Activity", attributes: [
            .font: NSFont.systemFont(ofSize: 12, weight: .semibold), .foregroundColor: theme.textSecondary,
        ]).draw(at: NSPoint(x: 14, y: 11))

        if rows.isEmpty {
            NSAttributedString(string: "No processes are reporting activity", attributes: [
                .font: NSFont.systemFont(ofSize: 12.5), .foregroundColor: theme.textTertiary,
            ]).draw(at: NSPoint(x: 14, y: headerHeight + 8))
            return
        }

        for (i, (session, _, pane, frame)) in rows.enumerated() {
            if i == hoverRow {
                theme.hover.setFill()
                NSBezierPath(roundedRect: frame, xRadius: 7, yRadius: 7).fill()
            }
            let icon = NSRect(x: frame.minX + 8, y: frame.midY - 9, width: 18, height: 18)
            ProgramIcon.draw(program: pane.foregroundProgram, executable: pane.foregroundExecutable, in: icon, theme: theme)
            let textX = icon.maxX + 10
            let textWidth = frame.maxX - textX - 26
            let title = pane.status?.title ?? pane.status?.app ?? pane.label.name
            NSAttributedString(string: title, attributes: [
                .font: NSFont.systemFont(ofSize: 12.5, weight: .medium), .foregroundColor: theme.textPrimary,
            ]).draw(with: NSRect(x: textX, y: frame.minY + 4, width: textWidth, height: 16),
                    options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
            let detail = [pane.statusDetail ?? pane.activity.label, session.name].joined(separator: " · ")
            NSAttributedString(string: detail, attributes: [
                .font: NSFont.systemFont(ofSize: 11), .foregroundColor: color(for: pane.activity, theme: theme),
            ]).draw(with: NSRect(x: textX, y: frame.minY + 21, width: textWidth, height: 15),
                    options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
            StatusIndicator.draw(pane.activity, in: NSRect(x: frame.maxX - 20, y: frame.midY - 6, width: 12, height: 12), theme: theme, phase: phase, spots: loaderSpots)
        }
    }

    private func color(for activity: PaneActivity, theme: Theme) -> NSColor {
        switch activity {
        case .blocked: theme.blocked
        case .error: theme.error
        case .done: theme.done
        case .working: theme.working
        default: theme.textSecondary
        }
    }

    func dismiss() {
        guard !isDismissing else { return }
        isDismissing = true
        onDismiss?()
        Motion.disappear(self, scale: 0.97, duration: 0.14) { [weak self] in self?.removeFromSuperview() }
    }

    override func cancelOperation(_ sender: Any?) { dismiss() }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: bounds, options: [.mouseMoved, .mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self))
    }

    override func mouseMoved(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        let row = rows.firstIndex { $0.3.contains(point) }
        if row != hoverRow { hoverRow = row; needsDisplay = true }
    }

    override func mouseExited(with event: NSEvent) { hoverRow = nil; needsDisplay = true }

    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        guard let row = rows.first(where: { $0.3.contains(point) }) else { return }
        dismiss()
        onSelect?(row.0, row.1, row.2)
    }
}
