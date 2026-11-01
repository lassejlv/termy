import AppKit
import Termy
import TermyNextCore

/// Termy's theme library in a searchable list: every built-in theme plus the
/// local files in `~/.config/termy-next/themes`, each with a color preview.
/// Picking one writes `theme = <id>` to Termy Next's config.
@MainActor
final class ThemePicker: NSView, NSTableViewDataSource, NSTableViewDelegate, NSSearchFieldDelegate {
    var onPick: ((TermyTheme) -> Void)?

    private let search = NSSearchField()
    private let table = NSTableView()
    private let scroll = NSScrollView()
    private let countLabel = NSTextField(labelWithString: "")
    private var all: [TermyTheme] = []
    private var shown: [TermyTheme] = []
    private var current: String?
    private var updatingSelection = false

    init(width: CGFloat, height: CGFloat) {
        super.init(frame: NSRect(x: 0, y: 0, width: width, height: height))
        translatesAutoresizingMaskIntoConstraints = false

        search.placeholderString = "Search themes"
        search.delegate = self
        search.sendsSearchStringImmediately = true
        search.target = self
        search.action = #selector(searchChanged)

        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("theme"))
        column.resizingMask = .autoresizingMask
        table.addTableColumn(column)
        table.headerView = nil
        table.rowHeight = 26
        table.style = .inset
        table.usesAlternatingRowBackgroundColors = false
        table.dataSource = self
        table.delegate = self
        table.allowsEmptySelection = true
        table.setAccessibilityLabel("Themes")

        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.borderType = .bezelBorder
        scroll.autohidesScrollers = true

        countLabel.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        countLabel.textColor = .tertiaryLabelColor

        for view in [search, scroll, countLabel] {
            view.translatesAutoresizingMaskIntoConstraints = false
            addSubview(view)
        }
        NSLayoutConstraint.activate([
            widthAnchor.constraint(equalToConstant: width),
            heightAnchor.constraint(equalToConstant: height),
            search.topAnchor.constraint(equalTo: topAnchor),
            search.leadingAnchor.constraint(equalTo: leadingAnchor),
            search.trailingAnchor.constraint(equalTo: trailingAnchor),
            scroll.topAnchor.constraint(equalTo: search.bottomAnchor, constant: 8),
            scroll.leadingAnchor.constraint(equalTo: leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: trailingAnchor),
            countLabel.topAnchor.constraint(equalTo: scroll.bottomAnchor, constant: 4),
            countLabel.leadingAnchor.constraint(equalTo: leadingAnchor),
            countLabel.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var firstBaselineOffsetFromTop: CGFloat { search.firstBaselineOffsetFromTop }

    /// Shows `themes` with `current` selected, keeping the search.
    func update(themes: [TermyTheme], current: String?) {
        let changed = themes.map(\.id) != all.map(\.id)
        all = themes
        self.current = current
        if changed { filter() } else { selectCurrent(scroll: false) }
    }

    @objc private func searchChanged() { filter() }

    private func filter() {
        let query = search.stringValue.trimmingCharacters(in: .whitespaces)
        shown = query.isEmpty ? all : FuzzyMatch.filter(all, query: query) { "\($0.name) \($0.id)" }
        table.reloadData()
        countLabel.stringValue = query.isEmpty ? "\(all.count) themes" : "\(shown.count) of \(all.count) themes"
        selectCurrent(scroll: true)
    }

    private func selectCurrent(scroll: Bool) {
        updatingSelection = true
        defer { updatingSelection = false }
        guard let current, let row = shown.firstIndex(where: { $0.id == current }) else {
            table.deselectAll(nil)
            return
        }
        table.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
        if scroll { table.scrollRowToVisible(row) }
    }

    // MARK: Table

    func numberOfRows(in tableView: NSTableView) -> Int { shown.count }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let id = NSUserInterfaceItemIdentifier("ThemeRow")
        let cell = tableView.makeView(withIdentifier: id, owner: self) as? ThemeRowView ?? ThemeRowView()
        cell.identifier = id
        let theme = shown[row]
        cell.configure(theme, isCurrent: theme.id == current)
        return cell
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        guard !updatingSelection, shown.indices.contains(table.selectedRow) else { return }
        let theme = shown[table.selectedRow]
        guard theme.isInstalled, theme.id != current else { return }
        current = theme.id
        onPick?(theme)
    }
}

/// One theme: a small preview of its background, text and accent colors,
/// then its name.
private final class ThemeRowView: NSTableCellView {
    private let preview = ThemePreview()
    private let name = NSTextField(labelWithString: "")

    init() {
        super.init(frame: .zero)
        name.lineBreakMode = .byTruncatingTail
        for view in [preview, name] as [NSView] {
            view.translatesAutoresizingMaskIntoConstraints = false
            addSubview(view)
        }
        textField = name
        NSLayoutConstraint.activate([
            preview.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 2),
            preview.centerYAnchor.constraint(equalTo: centerYAnchor),
            preview.widthAnchor.constraint(equalToConstant: 46),
            preview.heightAnchor.constraint(equalToConstant: 18),
            name.leadingAnchor.constraint(equalTo: preview.trailingAnchor, constant: 8),
            name.trailingAnchor.constraint(lessThanOrEqualTo: trailingAnchor, constant: -4),
            name.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    func configure(_ theme: TermyTheme, isCurrent: Bool) {
        name.stringValue = theme.isInstalled ? theme.name : "\(theme.name) (missing)"
        name.font = isCurrent ? .systemFont(ofSize: NSFont.systemFontSize, weight: .semibold) : .systemFont(ofSize: NSFont.systemFontSize)
        name.textColor = theme.isInstalled ? .labelColor : .tertiaryLabelColor
        preview.swatches = theme.swatches.map(NSColor.init)
        setAccessibilityLabel(theme.name)
    }
}

/// The theme's background with a line of "text" and four accent dots on it.
private final class ThemePreview: NSView {
    var swatches: [NSColor] = [] { didSet { needsDisplay = true } }

    override func draw(_ dirtyRect: NSRect) {
        let box = bounds.insetBy(dx: 0.5, dy: 0.5)
        let path = NSBezierPath(roundedRect: box, xRadius: 4, yRadius: 4)
        guard let background = swatches.first else {
            NSColor.quaternaryLabelColor.setFill()
            path.fill()
            return
        }
        background.setFill()
        path.fill()
        NSColor.separatorColor.setStroke()
        path.lineWidth = 1
        path.stroke()
        if swatches.count > 1 {
            swatches[1].setFill()
            NSBezierPath(roundedRect: NSRect(x: 5, y: bounds.midY - 1, width: 9, height: 2), xRadius: 1, yRadius: 1).fill()
        }
        for (index, color) in swatches.dropFirst(2).prefix(4).enumerated() {
            color.setFill()
            NSBezierPath(ovalIn: NSRect(x: 18 + CGFloat(index) * 6.5, y: bounds.midY - 2.5, width: 5, height: 5)).fill()
        }
    }
}
