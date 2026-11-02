import AppKit
import Termy
import TermyAppKit
import TermyNextCore

/// A pane's zsh plugin UI: the completion popup under the prompt and the
/// ⇧⌘E command box. Both float over the terminal at the cursor.
@MainActor
final class PaneShellUI: NSObject, NSTextFieldDelegate {
    private unowned let pane: PaneView
    private var completion: ShellCompletionView?
    private var ask: ShellAskView?
    private var run: ShellAgentRun?
    /// The popup stays away while the line is still this: after you pick an
    /// entry or press Esc.
    private var dismissedBuffer: String?
    /// A line the command box set and runs once zsh reports it back.
    private var pendingRun: String?
    /// The command the box last put on the line.
    private var generated: String?
    /// How many characters left of the cursor the popup's text starts under:
    /// the word being completed, or the whole line for history.
    private var alignCount = 0

    init(pane: PaneView) {
        self.pane = pane
    }

    private var connection: ShellConnection? { ShellPlugins.shared.connection(for: pane) }
    private var terminal: TerminalView { pane.terminal }
    var isAsking: Bool { ask != nil }

    // MARK: Line

    func lineChanged() {
        let line = connection?.line
        if let pendingRun, let line, line.buffer == pendingRun {
            self.pendingRun = nil
            dismissedBuffer = line.buffer
            try? terminal.session.terminal.write("\r")
        }
        if let dismissed = dismissedBuffer, line?.buffer != dismissed { dismissedBuffer = nil }
        guard let line, ask == nil, line.buffer != dismissedBuffer, pane.isFocused else { return hideCompletion() }
        let sections = rows(for: line)
        guard sections.contains(where: { !$0.isEmpty }) else { return hideCompletion() }
        if completion == nil {
            let view = ShellCompletionView()
            view.onPick = { [weak self] row in self?.pick(row) }
            pane.addSubview(view)
            completion = view
            view.update(sections: sections)
            position()
            Motion.appear(view, scale: 0.98, offsetY: -4, duration: 0.12)
        } else {
            completion?.update(sections: sections)
            position()
        }
        // The cursor moves once zsh's redraw reaches the terminal; follow it.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) { [weak self] in self?.position() }
    }

    /// The popup's sections: what the command's spec says can come next
    /// (with command completions on), then history, then command names.
    private func rows(for line: ShellLine) -> [[CompletionRow]] {
        let (history, commands) = line.suggestions
        let historyRows = history.map { entry in
            CompletionRow(title: entry, line: entry, typedCount: line.buffer.count,
                          icon: .logo(firstWord(entry), fallback: "clock.arrow.circlepath"))
        }
        let commandRows = commands.map { name in
            CompletionRow(title: name, line: name, typedCount: line.buffer.count, icon: .logo(name, fallback: "terminal"))
        }
        let specRows = self.specRows(for: line)
        guard !specRows.isEmpty else {
            alignCount = line.buffer.split(separator: "\n", omittingEmptySubsequences: false).last?.count ?? 0
            return [historyRows, commandRows]
        }
        // The spec leads; a little history stays under it.
        return [specRows, Array(historyRows.prefix(2))]
    }

    private func firstWord(_ text: String) -> String {
        String(text.split(whereSeparator: \.isWhitespace).first ?? "")
    }

    private func specRows(for line: ShellLine) -> [CompletionRow] {
        guard Preferences.shared.commandCompletions, line.cursor >= line.buffer.count,
              let command = ShellWords.split(line.buffer).first?.text, let spec = CommandSpecs.spec(for: command),
              let completion = spec.complete(line.buffer) else { return [] }
        alignCount = completion.word.count
        var rows: [CompletionRow] = []
        func add(_ item: SpecCompletion.Item, from generator: Generator?) {
            rows.append(CompletionRow(title: item.name, detail: item.description, line: completion.line(line.buffer, taking: item),
                                      typedCount: completion.word.count, icon: icon(for: item, generator: generator, command: completion.command),
                                      keepsOpen: true))
        }
        if let connection {
            for generator in completion.generators {
                let generated = GeneratorRunner.shared.items(
                    generator, word: completion.word, directory: pane.directory ?? NSHomeDirectory(),
                    searchPath: connection.path) { [weak self] in self?.lineChanged() }
                Generator.matching(generated, word: completion.word).forEach { add($0, from: generator) }
            }
        }
        completion.items.forEach { add($0, from: nil) }
        return rows
    }

    private func icon(for item: SpecCompletion.Item, generator: Generator?, command: String) -> CompletionRow.Icon {
        switch item.kind {
        case .subcommand: return .logo(command, fallback: "terminal")
        case .option: return .symbol("flag")
        case .argument:
            switch generator {
            case .gitBranches?, .gitRemoteBranches?: return .symbol("arrow.triangle.branch")
            case .gitTags?: return .symbol("tag")
            case .gitCommits?: return .symbol("smallcircle.filled.circle")
            case .gitStashes?: return .symbol("tray")
            case .gitRemotes?: return .symbol("network")
            case .gitChangedFiles?, .gitStagedFiles?: return .symbol("doc.badge.ellipsis")
            case .files?, .folders?: return .symbol(item.name.hasSuffix("/") ? "folder" : "doc")
            case .packageScripts?: return .symbol("play")
            case .justRecipes?: return .symbol("checklist")
            case .nodeBins?: return .symbol("shippingbox")
            case .sshHosts?: return .symbol("network")
            case .claudeSessions?: return .logo("claude", fallback: "bubble.left")
            case .codexSessions?: return .logo("codex", fallback: "bubble.left")
            case .brewInstalled?, .brewFormulae?, .brewCasks?, .brewOutdated?, .brewServices?, .brewTaps?: return .logo("brew", fallback: "shippingbox")
            case .cargoPackages?, .cargoBins?, .cargoExamples?: return .logo("cargo", fallback: "shippingbox")
            case .rustupToolchains?, .rustupTargets?, .rustupInstalledTargets?, .rustupComponents?: return .logo("rustup", fallback: "hammer")
            case nil: return .symbol("smallcircle.filled.circle")
            }
        }
    }

    private func hideCompletion() {
        guard let view = completion else { return }
        completion = nil
        Motion.disappear(view, duration: 0.1) { view.removeFromSuperview() }
    }

    private func pick(_ row: CompletionRow) {
        if !row.keepsOpen { dismissedBuffer = row.line }
        hideCompletion()
        connection?.setLine(row.line)
    }

    /// Keys for the popup: ⌃N / ⌃P move, and ↓ / ↑ too (with arrow keys on in
    /// Settings, else once a row is picked). Return and Tab take the row, Esc
    /// closes. Others go to the shell.
    func handleKey(_ event: NSEvent) -> Bool {
        guard let completion, let line = connection?.line else { return false }
        let flags = event.modifierFlags.intersection([.command, .shift, .option, .control])
        let key = event.charactersIgnoringModifiers?.lowercased()
        let arrows = Preferences.shared.completionArrowKeys
        switch (event.keyCode, flags) {
        case (_, .control) where key == "n": completion.move(1)
        case (_, .control) where key == "p": completion.move(-1)
        case (125, []) where arrows || completion.selected != nil: completion.move(1)
        case (126, []) where arrows || completion.selected != nil: completion.move(-1)
        case (36, []), (76, []), (48, []):
            guard let row = completion.selectedRow else { return false }
            pick(row)
        case (53, []):
            dismissedBuffer = line.buffer
            hideCompletion()
        default:
            return false
        }
        return true
    }

    // MARK: Command box

    func toggleAsk() {
        if ask != nil { return closeAsk() }
        guard connection?.line != nil else {
            NSSound.beep()
            return
        }
        hideCompletion()
        let view = ShellAskView()
        view.field.delegate = self
        view.onClose = { [weak self] in self?.closeAsk() }
        view.onPickAgent = { agent in Preferences.shared.shellAgent = agent.id }
        view.agent = currentAgent
        pane.addSubview(view)
        ask = view
        position()
        Motion.appear(view, scale: 0.98, offsetY: -4, duration: 0.14)
        pane.window?.makeFirstResponder(view.field)
    }

    private var currentAgent: ShellAgent {
        ShellAgent.with(id: Preferences.shared.shellAgent) ?? ShellAgent.all[0]
    }

    private func closeAsk() {
        run?.cancel()
        run = nil
        generated = nil
        guard let view = ask else { return }
        ask = nil
        Motion.disappear(view, duration: 0.12) { view.removeFromSuperview() }
        pane.focus()
    }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        guard let ask else { return false }
        switch selector {
        case #selector(NSResponder.insertNewline(_:)):
            let request = ask.field.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
            if !request.isEmpty {
                generate(request)
            } else if let generated, run == nil {
                if connection?.line?.buffer == generated {
                    try? terminal.session.terminal.write("\r")
                } else {
                    // zsh reports the line back first; lineChanged types Return then.
                    pendingRun = generated
                    connection?.setLine(generated)
                }
                closeAsk()
            }
            return true
        case #selector(NSResponder.cancelOperation(_:)):
            if run != nil {
                run?.cancel()
                run = nil
                ask.state = generated == nil ? .idle : .ready
            } else {
                closeAsk()
            }
            return true
        default:
            return false
        }
    }

    private func generate(_ request: String) {
        guard let ask, let connection else { return }
        run?.cancel()
        ask.state = .generating
        let agent = currentAgent
        // A follow-up changes what's on the line; a first request starts fresh.
        let refining = generated.flatMap { _ in connection.line?.buffer }.flatMap { $0.isEmpty ? nil : $0 }
        run = ShellAgentRun(agent: agent, request: request, refining: refining,
                            directory: pane.directory, searchPath: connection.path) { [weak self] result in
            guard let self, let ask = self.ask else { return }
            self.run = nil
            switch result {
            case .success(let command):
                self.generated = command
                self.dismissedBuffer = command
                self.connection?.setLine(command)
                ask.field.stringValue = ""
                ask.state = .ready
            case .failure(let error):
                ask.state = .failed(error.message)
            }
        }
    }

    // MARK: Layout

    func position() {
        guard completion != nil || ask != nil else { return }
        let area = pane.convert(terminal.bounds, from: terminal).insetBy(dx: 8, dy: 6)
        guard let cursorRect = terminal.cursorRect else { return }
        let cursor = pane.convert(cursorRect, from: terminal)
        if let ask {
            let width = min(area.width, 640)
            let height = ShellAskView.height
            var y = cursor.maxY + 6
            if y + height > area.maxY { y = max(area.minY, cursor.minY - height - 6) }
            let frame = NSRect(x: area.minX, y: y, width: width, height: height).integral
            if ask.frame != frame { ask.frame = frame }
        }
        if let completion {
            // Line the entries up under the start of what's typed.
            let typed = alignCount
            let size = completion.fittingSize(maxWidth: area.width)
            var x = cursor.minX - CGFloat(typed) * cursorRect.width - completion.textOffset
            x = max(area.minX, min(x, area.maxX - size.width))
            var y = cursor.maxY + 4
            if y + size.height > area.maxY { y = max(area.minY, cursor.minY - size.height - 4) }
            let frame = NSRect(origin: NSPoint(x: x, y: y), size: size).integral
            if completion.frame != frame { completion.frame = frame }
        }
    }

    func applyTheme() {
        completion?.needsDisplay = true
        ask?.applyTheme()
    }
}

// MARK: - Completion popup

/// One entry in the popup: what it shows, and the line taking it gives.
struct CompletionRow: Equatable {
    enum Icon: Equatable {
        /// A program's logo, such as "git" (falls back to `symbol`).
        case logo(String, fallback: String)
        case symbol(String)
    }

    var title: String
    var detail: String?
    var line: String
    /// Leading characters of `title` already typed, drawn dimmer.
    var typedCount: Int
    var icon: Icon
    /// After taking it the popup stays, to offer what comes next (a
    /// subcommand's options, a folder's files).
    var keepsOpen = false
}

/// Sections of completions with dividers between them, in the terminal's font.
@MainActor
final class ShellCompletionView: NSView {
    static let textInset: CGFloat = 8
    static let maxRows = 8
    private static let iconSize: CGFloat = 14

    private var sections: [[CompletionRow]] = []
    private(set) var selected: Int?
    var onPick: ((CompletionRow) -> Void)?

    private var rows: [CompletionRow] { sections.flatMap { $0 } }
    var selectedRow: CompletionRow? { selected.map { rows[$0] } }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.cornerRadius = 7
        layer?.cornerCurve = .continuous
        layer?.borderWidth = 1
        layer?.shadowOpacity = 0.18
        layer?.shadowRadius = 8
        layer?.shadowOffset = CGSize(width: 0, height: -2)
        layer?.masksToBounds = false
        setAccessibilityRole(.list)
        setAccessibilityLabel("Completions")
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }

    /// The terminal's font, a point smaller, so the list reads as a hint.
    private var font: NSFont {
        let render = AppConfig.shared.renderConfig
        let size = max(9, CGFloat(render.fontSize) - 1)
        return NSFont(name: render.fontFamily, size: size) ?? .monospacedSystemFont(ofSize: size, weight: .regular)
    }

    private var detailFont: NSFont { .systemFont(ofSize: max(9, font.pointSize - 1.5)) }
    private var rowHeight: CGFloat { ceil(font.ascender - font.descender + font.leading) + 5 }
    private static let dividerHeight: CGFloat = 7
    /// Where a row's text starts: after the icon slot.
    private var textX: CGFloat { 3 + Self.textInset - 4 + Self.iconSize + 6 }

    /// Text drawn this far right of the popup's left edge lines up under the
    /// cursor's column, for placing the popup.
    var textOffset: CGFloat { textX }

    func update(sections: [[CompletionRow]]) {
        let previous = selectedRow?.line
        var remaining = Self.maxRows
        self.sections = sections.compactMap { section in
            let kept = Array(section.prefix(remaining))
            remaining -= kept.count
            return kept.isEmpty ? nil : kept
        }
        selected = previous.flatMap { line in rows.firstIndex { $0.line == line } }
        needsDisplay = true
    }

    func move(_ step: Int) {
        guard !rows.isEmpty else { return }
        selected = selected.map { ($0 + step + rows.count) % rows.count } ?? (step > 0 ? 0 : rows.count - 1)
        needsDisplay = true
    }

    func fittingSize(maxWidth: CGFloat) -> NSSize {
        let titleWidth = rows.map { (display($0.title) as NSString).size(withAttributes: [.font: font]).width }.max() ?? 0
        let detailWidth = rows.compactMap { $0.detail.map { ($0 as NSString).size(withAttributes: [.font: detailFont]).width } }.max() ?? 0
        let width = textX + ceil(titleWidth) + (detailWidth > 0 ? 18 + min(ceil(detailWidth), 260) : 0) + Self.textInset + 4
        let dividers = CGFloat(max(0, sections.count - 1)) * Self.dividerHeight
        return NSSize(width: min(maxWidth, 600, max(180, width)), height: CGFloat(rows.count) * rowHeight + dividers + 6)
    }

    /// One line per entry: a multi-line history entry shows its first line.
    private func display(_ text: String) -> String {
        let lines = text.split(separator: "\n", omittingEmptySubsequences: false)
        return lines.count > 1 ? lines[0] + " …" : text
    }

    private func rowRect(_ index: Int) -> NSRect {
        var y: CGFloat = 3
        var counted = 0
        for (sectionIndex, section) in sections.enumerated() {
            if index < counted + section.count {
                y += CGFloat(index - counted) * rowHeight
                break
            }
            counted += section.count
            y += CGFloat(section.count) * rowHeight
            if sectionIndex < sections.count - 1 { y += Self.dividerHeight }
        }
        return NSRect(x: 3, y: y, width: bounds.width - 6, height: rowHeight)
    }

    override func draw(_ dirtyRect: NSRect) {
        let theme = AppConfig.shared.theme
        layer?.backgroundColor = theme.popover.cgColor
        layer?.borderColor = theme.paneStroke.cgColor
        let font = self.font
        let paragraph = NSMutableParagraphStyle()
        paragraph.lineBreakMode = .byTruncatingTail
        let textHeight = ceil(font.ascender - font.descender)
        for (index, row) in rows.enumerated() {
            let rect = rowRect(index)
            if index == selected {
                theme.selected.setFill()
                NSBezierPath(roundedRect: rect, xRadius: 4, yRadius: 4).fill()
            }
            let iconRect = NSRect(x: rect.minX + Self.textInset - 4, y: rect.midY - Self.iconSize / 2, width: Self.iconSize, height: Self.iconSize)
            drawIcon(row.icon, in: iconRect, theme: theme)

            var detailWidth: CGFloat = 0
            if let detail = row.detail {
                let attributes: [NSAttributedString.Key: Any] = [.font: detailFont, .foregroundColor: theme.textTertiary, .paragraphStyle: paragraph]
                detailWidth = min(ceil((detail as NSString).size(withAttributes: attributes).width), max(0, rect.width * 0.45))
                let detailHeight = ceil(detailFont.ascender - detailFont.descender)
                let detailRect = NSRect(x: rect.maxX - Self.textInset - detailWidth, y: rect.midY - detailHeight / 2,
                                        width: detailWidth, height: detailHeight)
                (detail as NSString).draw(with: detailRect, options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine], attributes: attributes)
            }
            let shown = display(row.title)
            let attributed = NSMutableAttributedString(string: shown, attributes: [
                .font: font, .foregroundColor: theme.textPrimary, .paragraphStyle: paragraph,
            ])
            let typed = min(row.typedCount, (shown as NSString).length)
            if typed > 0 {
                attributed.addAttribute(.foregroundColor, value: theme.textSecondary, range: NSRange(location: 0, length: typed))
            }
            let x = rect.minX + textX - 3
            let textRect = NSRect(x: x, y: rect.minY + (rect.height - textHeight) / 2,
                                  width: max(0, rect.maxX - x - Self.textInset - (detailWidth > 0 ? detailWidth + 12 : 0)), height: textHeight)
            attributed.draw(with: textRect, options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
        }
        theme.paneStroke.setFill()
        var counted = 0
        for section in sections.dropLast() {
            counted += section.count
            let y = rowRect(counted).minY - Self.dividerHeight / 2
            NSRect(x: 8, y: floor(y), width: bounds.width - 16, height: 1).fill()
        }
    }

    private func drawIcon(_ icon: CompletionRow.Icon, in rect: NSRect, theme: Theme) {
        var symbol: String
        switch icon {
        case .logo(let program, let fallback):
            if let logo = ProgramLogos.image(for: program, dark: theme.isDark) {
                ProgramLogos.draw(logo, in: rect.insetBy(dx: 0.5, dy: 0.5))
                return
            }
            symbol = fallback
        case .symbol(let name):
            symbol = name
        }
        let config = NSImage.SymbolConfiguration(pointSize: rect.height - 3, weight: .medium)
        guard let image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)?.withSymbolConfiguration(config) else { return }
        let tinted = image.tinted(theme.textSecondary)
        let size = tinted.size
        tinted.draw(in: NSRect(x: rect.midX - size.width / 2, y: rect.midY - size.height / 2, width: size.width, height: size.height),
                    from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
    }

    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        guard let index = rows.indices.first(where: { rowRect($0).contains(point) }) else { return }
        selected = index
        needsDisplay = true
        onPick?(rows[index])
    }

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
}

// MARK: - Command box

/// Describe a command, get it on the line: a field, the agent picker and a
/// hint for what Return and Esc do.
@MainActor
final class ShellAskView: NSView {
    static let height: CGFloat = 66

    enum State: Equatable {
        case idle, generating, ready
        case failed(String)
    }

    let field = NSTextField()
    private let status = NSTextField(labelWithString: "")
    /// Stands in for the command while the agent writes it.
    private let skeleton = ShimmerView()
    private let agentButton = NSButton(title: "", target: nil, action: nil)
    private lazy var closeButton = IconButton(symbol: "xmark", pointSize: 10, help: "Close (Esc)") { [weak self] in self?.onClose?() }

    var onClose: (() -> Void)?
    var onPickAgent: ((ShellAgent) -> Void)?
    var agent: ShellAgent = ShellAgent.all[0] { didSet { applyTheme() } }
    var state: State = .idle { didSet { applyTheme() } }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.cornerRadius = 9
        layer?.cornerCurve = .continuous
        layer?.borderWidth = 1
        layer?.shadowOpacity = 0.3
        layer?.shadowRadius = 12
        layer?.shadowOffset = CGSize(width: 0, height: -4)
        layer?.masksToBounds = false

        field.isBordered = false
        field.drawsBackground = false
        field.focusRingType = .none
        field.cell?.isScrollable = true
        field.cell?.wraps = false
        field.lineBreakMode = .byClipping
        field.setAccessibilityLabel("Describe a command")
        status.lineBreakMode = .byTruncatingTail
        skeleton.isHidden = true
        agentButton.isBordered = false
        agentButton.target = self
        agentButton.action = #selector(showAgents)
        agentButton.setAccessibilityLabel("Agent")
        for view in [field, skeleton, status, agentButton, closeButton] as [NSView] { addSubview(view) }
        applyTheme()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }

    func applyTheme() {
        let theme = AppConfig.shared.theme
        layer?.backgroundColor = theme.popover.cgColor
        layer?.borderColor = theme.paneStroke.cgColor
        appearance = theme.appearance
        let render = AppConfig.shared.renderConfig
        let mono = NSFont(name: render.fontFamily, size: 13) ?? .monospacedSystemFont(ofSize: 13, weight: .regular)
        field.font = mono
        field.textColor = theme.textPrimary
        field.placeholderAttributedString = NSAttributedString(string: state == .ready ? "Refine it…" : "Describe a command…", attributes: [
            .foregroundColor: theme.textTertiary, .font: mono,
        ])
        let small = NSFont.systemFont(ofSize: 11)
        let hint = { (key: String, label: String) -> NSAttributedString in
            let text = NSMutableAttributedString(string: key, attributes: [.font: NSFont.systemFont(ofSize: 11, weight: .semibold), .foregroundColor: theme.textSecondary])
            text.append(NSAttributedString(string: " \(label)", attributes: [.font: small, .foregroundColor: theme.textTertiary]))
            return text
        }
        let line = NSMutableAttributedString()
        switch state {
        case .idle:
            line.append(hint("↩", "generate"))
            line.append(NSAttributedString(string: "    "))
            line.append(hint("esc", "dismiss"))
        case .generating:
            line.append(NSAttributedString(string: "Generating…", attributes: [.font: small, .foregroundColor: theme.textSecondary]))
            line.append(NSAttributedString(string: "    "))
            line.append(hint("esc", "stop"))
        case .ready:
            line.append(hint("↩", "run"))
            line.append(NSAttributedString(string: "    "))
            line.append(hint("esc", "edit"))
        case .failed(let message):
            line.append(NSAttributedString(string: message, attributes: [.font: small, .foregroundColor: theme.error.withAlphaComponent(0.9)]))
        }
        let generating = state == .generating
        skeleton.tint = theme.foreground
        skeleton.isHidden = !generating
        skeleton.isAnimating = generating
        field.alphaValue = generating ? 0 : 1
        field.isEditable = !generating
        status.attributedStringValue = line
        agentButton.attributedTitle = NSAttributedString(string: "\(agent.title) ⌄", attributes: [
            .font: small, .foregroundColor: theme.textTertiary,
        ])
        closeButton.tint = theme.textSecondary
        closeButton.hoverTint = theme.textPrimary
        closeButton.hoverFill = theme.hover
        needsLayout = true
    }

    @objc private func showAgents() {
        let menu = NSMenu()
        for agent in ShellAgent.all {
            let item = NSMenuItem(title: agent.title, action: #selector(pickAgent(_:)), keyEquivalent: "")
            item.target = self
            item.representedObject = agent.id
            item.state = agent == self.agent ? .on : .off
            menu.addItem(item)
        }
        menu.popUp(positioning: nil, at: NSPoint(x: 0, y: agentButton.bounds.maxY + 4), in: agentButton)
    }

    @objc private func pickAgent(_ item: NSMenuItem) {
        guard let id = item.representedObject as? String, let agent = ShellAgent.with(id: id) else { return }
        self.agent = agent
        onPickAgent?(agent)
        window?.makeFirstResponder(field)
    }

    override func layout() {
        super.layout()
        let w = bounds.width
        closeButton.frame = NSRect(x: w - 30, y: 7, width: 22, height: 22)
        field.frame = NSRect(x: 12, y: 10, width: w - 48, height: 18)
        agentButton.sizeToFit()
        let agentWidth = agentButton.frame.width
        agentButton.frame = NSRect(x: w - agentWidth - 12, y: 40, width: agentWidth, height: 18)
        let statusX: CGFloat = 12
        skeleton.frame = NSRect(x: 12, y: 12, width: min(w - 60, max(120, (w - 60) * 0.62)), height: 14)
        status.frame = NSRect(x: statusX, y: 41, width: max(40, agentButton.frame.minX - statusX - 8), height: 15)
    }
}

/// A skeleton bar with a sheen sweeping across it while something loads.
/// Holds still when Reduce Motion is on.
@MainActor
final class ShimmerView: NSView {
    private let sheen = CAGradientLayer()

    var tint: NSColor = .labelColor { didSet { applyColors() } }
    var isAnimating = false {
        didSet {
            guard isAnimating != oldValue else { return }
            isAnimating ? start() : sheen.removeAllAnimations()
        }
    }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.cornerCurve = .continuous
        layer?.masksToBounds = true
        sheen.startPoint = CGPoint(x: 0, y: 0.5)
        sheen.endPoint = CGPoint(x: 1, y: 0.5)
        sheen.locations = [0, 0.15, 0.3]
        layer?.addSublayer(sheen)
        applyColors()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override func layout() {
        super.layout()
        layer?.cornerRadius = bounds.height / 2
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        sheen.frame = bounds
        CATransaction.commit()
    }

    private func applyColors() {
        layer?.backgroundColor = tint.withAlphaComponent(0.08).cgColor
        sheen.colors = [NSColor.clear.cgColor, tint.withAlphaComponent(0.16).cgColor, NSColor.clear.cgColor]
    }

    private func start() {
        guard !Motion.reduced else { return }
        let sweep = CABasicAnimation(keyPath: "locations")
        sweep.fromValue = [-0.3, -0.15, 0]
        sweep.toValue = [1, 1.15, 1.3]
        sweep.duration = 1.1
        sweep.repeatCount = .infinity
        sweep.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
        sheen.add(sweep, forKey: "sweep")
    }
}
