import AppKit

/// The tab loaders as a grid of live previews; clicking one picks it. It
/// shows two rows at first, the picked loader always among them, and
/// "Show All" opens the rest.
@MainActor
final class LoaderPicker: NSView {
    var onPick: ((TabLoader) -> Void)?
    var selected: TabLoader = .dots {
        didSet { if selected != oldValue { reloadCells() } }
    }

    private static let columns = 5
    private static let collapsedRows = 2
    private static let cellHeight: CGFloat = 64
    private static let inset: CGFloat = 6
    private static let footerHeight: CGFloat = 34

    private var expanded = false
    private var shown: [TabLoader] = []
    private var previews: [LoaderView] = []
    private var hovered: TabLoader?
    private var heightConstraint: NSLayoutConstraint!
    private let toggle = NSButton(title: "", target: nil, action: nil)
    private var colors = (text: NSColor.labelColor, secondary: NSColor.secondaryLabelColor,
                          selection: NSColor.controlAccentColor, selectedText: NSColor.white, hover: NSColor.quaternaryLabelColor)

    init(width: CGFloat) {
        super.init(frame: NSRect(x: 0, y: 0, width: width, height: 0))
        translatesAutoresizingMaskIntoConstraints = false
        heightConstraint = heightAnchor.constraint(equalToConstant: 0)
        NSLayoutConstraint.activate([widthAnchor.constraint(equalToConstant: width), heightConstraint])

        toggle.bezelStyle = .push
        toggle.controlSize = .small
        toggle.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        toggle.target = self
        toggle.action = #selector(toggleExpanded)
        toggle.translatesAutoresizingMaskIntoConstraints = false
        addSubview(toggle)
        NSLayoutConstraint.activate([
            toggle.centerXAnchor.constraint(equalTo: centerXAnchor),
            toggle.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -4),
        ])

        setAccessibilityElement(true)
        setAccessibilityRole(.radioGroup)
        setAccessibilityLabel("Tab loader")
        reloadCells()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }
    override var mouseDownCanMoveWindow: Bool { false }

    func applyTheme(_ theme: Theme) {
        colors = (theme.textPrimary, theme.textSecondary, theme.selectionFill, theme.selectionText, theme.hover)
        updatePreviewColors()
        needsDisplay = true
    }

    // MARK: Cells

    /// The first two rows, with the picked loader swapped into the last
    /// spot when it's further down; or every loader once expanded.
    private var visibleLoaders: [TabLoader] {
        let all = TabLoader.allCases
        guard !expanded else { return Array(all) }
        var loaders = Array(all.prefix(Self.columns * Self.collapsedRows))
        if !loaders.contains(selected) { loaders[loaders.count - 1] = selected }
        return loaders
    }

    private func reloadCells() {
        shown = visibleLoaders
        while previews.count > shown.count { previews.removeLast().removeFromSuperview() }
        while previews.count < shown.count {
            let preview = LoaderView(frame: .zero)
            preview.size = 20
            addSubview(preview, positioned: .below, relativeTo: toggle)
            previews.append(preview)
        }
        let rows = (shown.count + Self.columns - 1) / Self.columns
        heightConstraint.constant = CGFloat(rows) * Self.cellHeight + Self.inset + Self.footerHeight
        toggle.title = expanded ? "Show Less" : "Show All \(TabLoader.allCases.count)"
        needsLayout = true
        needsDisplay = true
        updatePreviewColors()
    }

    private func updatePreviewColors() {
        for (preview, loader) in zip(previews, shown) {
            preview.loader = loader
            preview.color = loader == selected ? colors.selectedText : colors.text
        }
    }

    override func layout() {
        super.layout()
        for (index, preview) in previews.enumerated() {
            let cell = cellRect(index).insetBy(dx: 3, dy: 3)
            preview.frame = NSRect(x: cell.minX, y: cell.minY + 8, width: cell.width, height: 26)
        }
    }

    @objc private func toggleExpanded() {
        expanded.toggle()
        hovered = nil
        reloadCells()
    }

    private func cellRect(_ index: Int) -> NSRect {
        let width = (bounds.width - Self.inset * 2) / CGFloat(Self.columns)
        return NSRect(x: Self.inset + CGFloat(index % Self.columns) * width,
                      y: Self.inset + CGFloat(index / Self.columns) * Self.cellHeight,
                      width: width, height: Self.cellHeight)
    }

    private func loader(at point: NSPoint) -> TabLoader? {
        shown.indices.first { cellRect($0).contains(point) }.map { shown[$0] }
    }

    // MARK: Drawing

    override func draw(_ dirtyRect: NSRect) {
        let style = NSMutableParagraphStyle()
        style.alignment = .center
        style.lineBreakMode = .byTruncatingTail
        for (index, loader) in shown.enumerated() {
            let cell = cellRect(index).insetBy(dx: 3, dy: 3)
            guard cell.intersects(dirtyRect) else { continue }
            let isSelected = loader == selected
            if isSelected || loader == hovered {
                (isSelected ? colors.selection : colors.hover).setFill()
                NSBezierPath(roundedRect: cell, xRadius: 8, yRadius: 8).fill()
            }
            let title = loader == .dots ? "Dots (Default)" : loader.title
            (title as NSString).draw(
                with: NSRect(x: cell.minX + 2, y: cell.maxY - 20, width: cell.width - 4, height: 14),
                options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine],
                attributes: [.font: NSFont.systemFont(ofSize: 10.5, weight: isSelected ? .semibold : .regular),
                             .foregroundColor: isSelected ? colors.selectedText : colors.secondary,
                             .paragraphStyle: style])
        }
    }

    // MARK: Mouse

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: bounds, options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect],
                                       owner: self))
    }

    override func mouseMoved(with event: NSEvent) {
        let next = loader(at: convert(event.locationInWindow, from: nil))
        if next != hovered { hovered = next; needsDisplay = true }
    }

    override func mouseExited(with event: NSEvent) {
        hovered = nil
        needsDisplay = true
    }

    override func mouseDown(with event: NSEvent) {
        guard let loader = loader(at: convert(event.locationInWindow, from: nil)) else { return }
        selected = loader
        onPick?(loader)
    }

    override func accessibilityValue() -> Any? { selected.title }
}
