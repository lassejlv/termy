import AppKit
import TermyNextCore

/// One row in a picker.
struct PickerItem {
    enum Accessory { case none, shortcut(String), chevron(String?), check }

    var title: String
    var subtitle: String?
    var symbol: String?
    var status: PaneActivity?
    var accessory: Accessory = .none
    /// Extra words the filter matches but the row doesn't show.
    var keywords: String = ""
    var run: () -> Void
}

/// The popover behind the command palette, session picker and text prompts:
/// a search field over a filtered list, keyboard driven, anchored under the
/// titlebar in the middle of the window.
@MainActor
final class PickerView: GlassPanel, NSTextFieldDelegate {
    enum Mode {
        /// Filtered list. `create` adds a row for text that matches nothing.
        case list(placeholder: String, items: () -> [PickerItem], create: ((String) -> PickerItem)?)
        /// Free text, such as a new name.
        case prompt(placeholder: String, initial: String, submit: (String) -> Void)
    }

    var onDismiss: (() -> Void)?
    var footer: String?

    private let field = NSTextField()
    private let searchIcon = NSImageView()
    private var mode: Mode
    private var allItems: [PickerItem] = []
    private var items: [PickerItem] = []
    private var selection = 0
    private var scroll = 0
    private var hoverRow: Int?
    private let rowHeight: CGFloat = 25
    private let maxRows = 14
    private let fieldHeight: CGFloat = 40

    init(mode: Mode) {
        self.mode = mode
        super.init(frame: .zero)
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

        field.isBordered = false
        field.drawsBackground = false
        field.focusRingType = .none
        field.font = .systemFont(ofSize: 13)
        field.textColor = theme.textPrimary
        field.delegate = self
        field.cell?.usesSingleLineMode = true
        field.cell?.lineBreakMode = .byTruncatingHead
        addSubview(field)

        searchIcon.image = NSImage(systemSymbolName: "magnifyingglass", accessibilityDescription: nil)?
            .withSymbolConfiguration(.init(pointSize: 12, weight: .medium))
        searchIcon.contentTintColor = theme.textSecondary
        addSubview(searchIcon)

        switch mode {
        case .list(let placeholder, let items, _):
            field.placeholderAttributedString = placeholderText(placeholder)
            allItems = items()
        case .prompt(let placeholder, let initial, _):
            field.placeholderAttributedString = placeholderText(placeholder)
            field.stringValue = initial
            searchIcon.image = NSImage(systemSymbolName: "pencil", accessibilityDescription: nil)?
                .withSymbolConfiguration(.init(pointSize: 12, weight: .medium))
        }
        refilter()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var mouseDownCanMoveWindow: Bool { false }

    private func placeholderText(_ text: String) -> NSAttributedString {
        NSAttributedString(string: text, attributes: [
            .foregroundColor: AppConfig.shared.theme.textTertiary, .font: NSFont.systemFont(ofSize: 13),
        ])
    }

    func focusField() {
        window?.makeFirstResponder(field)
        field.currentEditor()?.selectAll(nil)
    }

    /// Height for the current rows, so the host can size the popover.
    var preferredHeight: CGFloat {
        let rows = min(items.count, maxRows)
        let list = rows > 0 ? CGFloat(rows) * rowHeight + 10 : 0
        return fieldHeight + list + (footer != nil && rows > 0 ? 30 : 0) + 6
    }

    override func layout() {
        super.layout()
        layer?.shadowPath = CGPath(roundedRect: bounds, cornerWidth: layer?.cornerRadius ?? 12, cornerHeight: layer?.cornerRadius ?? 12, transform: nil)
        let box = NSRect(x: 6, y: 6, width: bounds.width - 12, height: fieldHeight - 10)
        searchIcon.frame = NSRect(x: box.minX + 8, y: box.midY - 8, width: 16, height: 16)
        field.frame = NSRect(x: box.minX + 30, y: box.midY - 9, width: box.width - 38, height: 18)
    }

    override func drawContent(_ dirtyRect: NSRect) {
        let theme = AppConfig.shared.theme
        let box = NSRect(x: 6, y: 6, width: bounds.width - 12, height: fieldHeight - 10)
        theme.foreground.withAlphaComponent(0.07).setFill()
        NSBezierPath(roundedRect: box, xRadius: 8, yRadius: 8).fill()

        let visible = min(items.count, maxRows)
        for row in 0..<visible {
            let index = scroll + row
            guard items.indices.contains(index) else { break }
            drawRow(items[index], selected: index == selection, hovered: index == hoverRow, in: rowRect(row), theme: theme)
        }

        if let footer, visible > 0 {
            let y = bounds.height - 30
            theme.foreground.withAlphaComponent(0.08).setFill()
            NSRect(x: 0, y: y, width: bounds.width, height: 0.5).fill()
            drawFooter(footer, y: y, theme: theme)
        }
    }

    private func rowRect(_ row: Int) -> NSRect {
        NSRect(x: 6, y: fieldHeight + 4 + CGFloat(row) * rowHeight, width: bounds.width - 12, height: rowHeight)
    }

    private func drawRow(_ item: PickerItem, selected: Bool, hovered: Bool, in rect: NSRect, theme: Theme) {
        if selected {
            theme.selectionFill.setFill()
            NSBezierPath(roundedRect: rect, xRadius: 7, yRadius: 7).fill()
        } else if hovered {
            theme.hover.setFill()
            NSBezierPath(roundedRect: rect, xRadius: 7, yRadius: 7).fill()
        }
        let primary = selected ? theme.selectionText : theme.textPrimary
        let secondary = selected ? theme.selectionText.withAlphaComponent(0.7) : theme.textSecondary
        var x = rect.minX + 9
        if let symbol = item.symbol {
            if symbol.hasPrefix("program:") {
                ProgramIcon.draw(program: String(symbol.dropFirst(8)), in: NSRect(x: x, y: rect.midY - 8, width: 16, height: 16), theme: theme)
            } else if let image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)?
                .withSymbolConfiguration(.init(pointSize: 11, weight: .medium))?.tinted(secondary) {
                image.drawUpright(at: NSPoint(x: x + (16 - image.size.width) / 2, y: rect.midY - image.size.height / 2))
            }
            x += 24
        }

        var accessoryWidth: CGFloat = 0
        let accessoryFont = NSFont.systemFont(ofSize: 11.5)
        switch item.accessory {
        case .none: break
        case .shortcut(let text):
            let s = NSAttributedString(string: text, attributes: [.font: accessoryFont, .foregroundColor: secondary])
            accessoryWidth = s.size().width + 10
            s.draw(at: NSPoint(x: rect.maxX - accessoryWidth, y: rect.midY - s.size().height / 2))
        case .chevron(let shortcut):
            if let image = NSImage(systemSymbolName: "chevron.right", accessibilityDescription: nil)?
                .withSymbolConfiguration(.init(pointSize: 9, weight: .semibold))?.tinted(secondary) {
                image.drawUpright(at: NSPoint(x: rect.maxX - 16, y: rect.midY - image.size.height / 2))
            }
            accessoryWidth = 22
            if let shortcut {
                let s = NSAttributedString(string: shortcut, attributes: [.font: accessoryFont, .foregroundColor: secondary])
                accessoryWidth += s.size().width + 6
                s.draw(at: NSPoint(x: rect.maxX - accessoryWidth, y: rect.midY - s.size().height / 2))
            }
        case .check:
            if let image = NSImage(systemSymbolName: "checkmark", accessibilityDescription: nil)?
                .withSymbolConfiguration(.init(pointSize: 10, weight: .bold))?.tinted(primary) {
                image.drawUpright(at: NSPoint(x: rect.maxX - 20, y: rect.midY - image.size.height / 2))
            }
            accessoryWidth = 26
        }
        if let status = item.status, status.urgency >= PaneActivity.working.urgency {
            accessoryWidth += 14
            StatusIndicator.draw(status, in: NSRect(x: rect.maxX - accessoryWidth, y: rect.midY - 5, width: 10, height: 10), theme: theme, phase: 0)
        }

        let text = NSMutableAttributedString(string: item.title, attributes: [
            .font: NSFont.systemFont(ofSize: 13), .foregroundColor: primary,
        ])
        if let subtitle = item.subtitle {
            text.append(NSAttributedString(string: "  " + subtitle, attributes: [
                .font: NSFont.systemFont(ofSize: 12), .foregroundColor: secondary,
            ]))
        }
        let height = text.size().height
        text.draw(with: NSRect(x: x, y: rect.midY - height / 2, width: rect.maxX - accessoryWidth - x - 6, height: height),
                  options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
    }

    private func drawFooter(_ text: String, y: CGFloat, theme: Theme) {
        // "Action ↩" with the key in a small rounded box, right aligned.
        let label = NSAttributedString(string: text, attributes: [
            .font: NSFont.systemFont(ofSize: 11), .foregroundColor: theme.textSecondary,
        ])
        let key = NSAttributedString(string: "↩", attributes: [
            .font: NSFont.systemFont(ofSize: 10, weight: .medium), .foregroundColor: theme.textSecondary,
        ])
        let keyBox = NSRect(x: bounds.width - 12 - 20, y: y + 7, width: 20, height: 16)
        theme.foreground.withAlphaComponent(0.25).setStroke()
        let path = NSBezierPath(roundedRect: keyBox, xRadius: 4, yRadius: 4)
        path.lineWidth = 0.8
        path.stroke()
        key.draw(at: NSPoint(x: keyBox.midX - key.size().width / 2, y: keyBox.midY - key.size().height / 2))
        label.draw(at: NSPoint(x: keyBox.minX - 8 - label.size().width, y: keyBox.midY - label.size().height / 2))
    }

    // MARK: Filtering and selection

    private func refilter() {
        guard case .list(_, _, let create) = mode else {
            items = []
            needsDisplay = true
            return
        }
        let query = field.stringValue
        items = FuzzyMatch.filter(allItems, query: query) { "\($0.title) \($0.subtitle ?? "") \($0.keywords)" }
        let trimmed = query.trimmingCharacters(in: .whitespaces)
        if let create, !trimmed.isEmpty, !items.contains(where: { $0.title.caseInsensitiveCompare(trimmed) == .orderedSame }) {
            items.append(create(trimmed))
        }
        selection = 0
        scroll = 0
        needsDisplay = true
        superview?.needsLayout = true
    }

    private func move(_ delta: Int) {
        guard !items.isEmpty else { return }
        selection = (selection + delta + items.count) % items.count
        if selection < scroll { scroll = selection }
        if selection >= scroll + maxRows { scroll = selection - maxRows + 1 }
        needsDisplay = true
    }

    private func submit() {
        switch mode {
        case .prompt(_, _, let submit):
            let text = field.stringValue
            dismiss()
            submit(text)
        case .list:
            guard items.indices.contains(selection) else { return }
            let item = items[selection]
            dismiss()
            item.run()
        }
    }

    func dismiss() {
        guard !isDismissing else { return }
        isDismissing = true
        field.delegate = nil
        onDismiss?()
        Motion.disappear(self, scale: 0.97, duration: 0.14) { [weak self] in self?.removeFromSuperview() }
    }

    private var isDismissing = false

    func controlTextDidChange(_ obj: Notification) { refilter() }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        switch selector {
        case #selector(NSResponder.moveUp(_:)): move(-1)
        case #selector(NSResponder.moveDown(_:)): move(1)
        case #selector(NSResponder.insertNewline(_:)): submit()
        case #selector(NSResponder.cancelOperation(_:)): dismiss()
        case #selector(NSResponder.insertTab(_:)): move(1)
        case #selector(NSResponder.insertBacktab(_:)): move(-1)
        case #selector(NSResponder.scrollPageDown(_:)), #selector(NSResponder.pageDown(_:)): move(maxRows)
        case #selector(NSResponder.scrollPageUp(_:)), #selector(NSResponder.pageUp(_:)): move(-maxRows)
        default: return false
        }
        return true
    }

    // MARK: Mouse

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: bounds, options: [.mouseMoved, .mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self))
    }

    private func itemIndex(at point: NSPoint) -> Int? {
        for row in 0..<min(items.count, maxRows) where rowRect(row).contains(point) {
            return scroll + row
        }
        return nil
    }

    override func mouseMoved(with event: NSEvent) {
        let index = itemIndex(at: convert(event.locationInWindow, from: nil))
        if index != hoverRow { hoverRow = index; needsDisplay = true }
    }

    override func mouseExited(with event: NSEvent) { hoverRow = nil; needsDisplay = true }

    override func mouseDown(with event: NSEvent) {
        guard let index = itemIndex(at: convert(event.locationInWindow, from: nil)) else { return }
        selection = index
        submit()
    }

    override func scrollWheel(with event: NSEvent) {
        let steps = event.scrollingDeltaY > 0 ? -1 : event.scrollingDeltaY < 0 ? 1 : 0
        guard steps != 0 else { return }
        scroll = min(max(scroll + steps, 0), max(0, items.count - maxRows))
        needsDisplay = true
    }
}
