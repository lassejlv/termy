import AppKit
import Termy
import TermyAppKit

/// Find in a pane (⌘F): a small bar in the pane's top-right corner, and a
/// spotlight over the terminal that dims everything except the matches, so
/// only the results stand out. Searches the pane's whole history; Return
/// walks up through older matches, ⇧Return back down.
@MainActor
final class PaneSearch: NSObject, NSTextFieldDelegate {
    let bar = SearchBar()
    let spotlight = SearchSpotlight()
    private unowned let terminal: TerminalView
    private(set) var matches: [SearchMatch] = []
    private var current: Int?
    private var options = SearchOptions()
    private var lastSearch = Date.distantPast
    private var lastGeometry: [CGRect] = []
    /// The terminal's content when it was last searched; re-running finds
    /// nothing new while it stays the same.
    private var searchedGeneration: UInt64?
    /// A search waiting for typing to pause.
    private var pendingSearch: DispatchWorkItem?
    /// How long typing pauses before the history is searched: each search
    /// walks the whole history, and a hosted pane's asks the session host.
    private static let typingDelay: TimeInterval = 0.12

    /// One search of the history, run off the main thread.
    private struct Request {
        var query: String
        var options: SearchOptions
        var keepingPlace: Bool
        /// Run again because output arrived while it ran the first time.
        var isRetry = false
    }

    /// A search is running; only one runs at a time.
    private var searching = false
    /// The latest search asked for while one ran; it replaces older ones.
    private var queued: Request?
    /// What `matches` were found for.
    private var matchesFor: (query: String, options: SearchOptions)?
    /// Steps (Return, ⇧Return, the arrows) taken while the matches for what
    /// is typed were still on their way; they're taken once those land.
    private var queuedSteps = 0

    init(terminal: TerminalView) {
        self.terminal = terminal
        super.init()
        bar.field.delegate = self
        bar.onStep = { [weak self] step in self?.step(step) }
        bar.onClose = { [weak self] in self?.onClose?() }
        bar.onToggleCase = { [weak self] in
            guard let self else { return }
            self.options.caseSensitive.toggle()
            self.bar.caseButton.isActive = self.options.caseSensitive
            self.search(keepingPlace: true)
        }
        bar.onToggleRegex = { [weak self] in
            guard let self else { return }
            self.options.regex.toggle()
            self.bar.regexButton.isActive = self.options.regex
            self.search(keepingPlace: true)
        }
    }

    var onClose: (() -> Void)?
    var query: String { bar.field.stringValue }

    func focusField(prefill: String? = nil) {
        if let prefill, !prefill.isEmpty, !prefill.contains("\n") {
            bar.field.stringValue = prefill
            search(keepingPlace: false)
        }
        bar.window?.makeFirstResponder(bar.field)
        bar.field.currentEditor()?.selectAll(nil)
    }

    // MARK: Searching

    func controlTextDidChange(_ notification: Notification) {
        pendingSearch?.cancel()
        let work = DispatchWorkItem { [weak self] in
            MainActor.assumeIsolated {
                guard let self else { return }
                self.pendingSearch = nil
                self.search(keepingPlace: false)
            }
        }
        pendingSearch = work
        DispatchQueue.main.asyncAfter(deadline: .now() + Self.typingDelay, execute: work)
    }

    /// Starts a search still waiting on typing now.
    private func flushPendingSearch() {
        guard pendingSearch != nil else { return }
        pendingSearch?.cancel()
        pendingSearch = nil
        search(keepingPlace: false)
    }

    /// Whether `matches` are for what is typed and the options set now.
    private var matchesAreCurrent: Bool {
        matchesFor.map { $0.query == query && $0.options == options } ?? query.isEmpty
    }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        switch selector {
        case #selector(NSResponder.insertNewline(_:)):
            step(NSApp.currentEvent?.modifierFlags.contains(.shift) == true ? 1 : -1)
            return true
        case #selector(NSResponder.cancelOperation(_:)):
            onClose?()
            return true
        default:
            return false
        }
    }

    /// Runs the query over the history, off the main thread. A fresh query
    /// starts at the newest match; a re-run (new output, toggled option)
    /// stays on the same one.
    func search(keepingPlace: Bool) {
        pendingSearch?.cancel()
        pendingSearch = nil
        let request = Request(query: query, options: options, keepingPlace: keepingPlace)
        guard !request.query.isEmpty else {
            // Nothing to look for; a search still running is dropped when it lands.
            lastSearch = Date()
            searchedGeneration = terminal.session.contentGeneration
            return apply([], for: request)
        }
        if searching {
            queued = request
        } else {
            start(request)
        }
    }

    private func start(_ request: Request) {
        searching = true
        lastSearch = Date()
        let session = terminal.session
        let generation = session.contentGeneration
        searchedGeneration = generation
        Task { [weak self] in
            let found = (try? await session.search(request.query, options: request.options)) ?? []
            guard let self else { return }
            self.searching = false
            if let next = self.queued {
                // Asked for again meanwhile: the latest request wins.
                self.queued = nil
                return self.start(next)
            }
            // Typed or toggled since: the new query's own search follows.
            guard request.query == self.query, request.options == self.options else { return }
            if session.contentGeneration != generation, !request.isRetry {
                // Output arrived during the scan; look once more. Under
                // output that never stops, the second result is shown, and
                // the ticks keep catching up with it.
                var again = request
                again.isRetry = true
                return self.start(again)
            }
            self.apply(found, for: request)
        }
    }

    private func apply(_ found: [SearchMatch], for request: Request) {
        let keepingPlace = request.keepingPlace
        let previous = current.flatMap { matches.indices.contains($0) ? matches[$0] : nil }
        matches = found
        matchesFor = (request.query, request.options)
        if matches.isEmpty {
            current = nil
        } else if keepingPlace, let previous,
                  let same = matches.firstIndex(where: { $0.row == previous.row && $0.startCol == previous.startCol }) {
            current = same
        } else if keepingPlace, let previous {
            current = matches.lastIndex { $0.row <= previous.row } ?? matches.count - 1
        } else {
            current = matches.count - 1
        }
        if !keepingPlace, let current { terminal.reveal(matches[current]) }
        refresh(animated: !keepingPlace)
        let steps = queuedSteps
        queuedSteps = 0
        if steps != 0, !matches.isEmpty { move(by: steps) }
    }

    /// Moves to an older (-1) or newer (+1) match, wrapping around.
    func step(_ direction: Int) {
        // Return right after typing goes to the matches of what's typed:
        // the step waits for them and is taken when they land.
        guard matchesAreCurrent else {
            flushPendingSearch()
            if !searching { search(keepingPlace: false) }
            queuedSteps += direction
            return
        }
        guard !matches.isEmpty else { return search(keepingPlace: false) }
        move(by: direction)
    }

    private func move(by direction: Int) {
        let next = (((current ?? matches.count - 1) + direction) % matches.count + matches.count) % matches.count
        current = next
        terminal.reveal(matches[next])
        refresh(animated: true)
    }

    /// Called often: follows scrolling and output, and re-runs the search now
    /// and then so new output gets matched too.
    func tick() {
        // New output gets searched now and then; without any, the matches stand.
        if pendingSearch == nil, !searching, !query.isEmpty, Date().timeIntervalSince(lastSearch) > 0.6,
           terminal.session.contentGeneration != searchedGeneration {
            search(keepingPlace: true)
        } else {
            refresh(animated: false)
        }
    }

    private func refresh(animated: Bool) {
        let visible: [(CGRect, Bool)] = matches.indices.compactMap { index in
            terminal.rect(for: matches[index]).map { ($0, index == current) }
        }
        bar.update(count: matches.count, current: current, hasQuery: !query.isEmpty)
        let geometry = visible.map(\.0)
        guard animated || geometry != lastGeometry || spotlight.isActive != !matches.isEmpty else { return }
        lastGeometry = geometry
        spotlight.frame = terminal.frame
        spotlight.show(matches: visible, active: !matches.isEmpty, pulseCurrent: animated)
    }
}

// MARK: - Spotlight

/// Dims the terminal around the matches: a scrim with holes cut where each
/// visible match is, a soft highlight on every match and a stronger one on
/// the current match.
@MainActor
final class SearchSpotlight: NSView {
    private let scrim = CAShapeLayer()
    private let others = CAShapeLayer()
    private let focus = CAShapeLayer()
    private(set) var isActive = false

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.masksToBounds = true
        scrim.fillRule = .evenOdd
        for shape in [scrim, others, focus] {
            shape.opacity = 0
            layer?.addSublayer(shape)
        }
        applyTheme()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    func applyTheme() {
        let theme = AppConfig.shared.theme
        scrim.fillColor = theme.terminalBackground.mixed(with: .black, by: theme.isDark ? 0.35 : 0)
            .withAlphaComponent(theme.isDark ? 0.62 : 0.55).cgColor
        // The theme's cursor color when it has color; gray or white ones would
        // look like the dimmed text, so those fall back to a warm yellow.
        let cursor = theme.accent.usingColorSpace(.sRGB) ?? theme.accent
        let accent = cursor.saturationComponent < 0.3 ? NSColor.systemYellow : cursor
        others.fillColor = accent.withAlphaComponent(0.16).cgColor
        others.strokeColor = accent.withAlphaComponent(0.35).cgColor
        others.lineWidth = 1
        focus.fillColor = accent.withAlphaComponent(0.3).cgColor
        focus.strokeColor = accent.cgColor
        focus.lineWidth = 1.5
    }

    func show(matches: [(rect: CGRect, isCurrent: Bool)], active: Bool, pulseCurrent: Bool) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        for shape in [scrim, others, focus] { shape.frame = bounds }
        let holes = CGMutablePath()
        holes.addRect(bounds)
        let otherPath = CGMutablePath(), focusPath = CGMutablePath()
        for (rect, isCurrent) in matches {
            let box = rect.insetBy(dx: -1, dy: 0)
            holes.addRoundedRect(in: box, cornerWidth: 3, cornerHeight: 3)
            (isCurrent ? focusPath : otherPath).addRoundedRect(in: box.insetBy(dx: 0.5, dy: 0.5), cornerWidth: 3, cornerHeight: 3)
        }
        scrim.path = holes
        others.path = otherPath
        focus.path = focusPath
        CATransaction.commit()

        if active != isActive {
            isActive = active
            let target: Float = active ? 1 : 0
            for shape in [scrim, others, focus] {
                let fade = CABasicAnimation(keyPath: "opacity")
                fade.fromValue = shape.presentation()?.opacity ?? shape.opacity
                fade.toValue = target
                fade.duration = Motion.reduced ? 0 : 0.18
                shape.opacity = target
                shape.add(fade, forKey: "fade")
            }
        }
        if pulseCurrent, active, !Motion.reduced {
            // The current match blooms as you land on it.
            let bloom = CABasicAnimation(keyPath: "lineWidth")
            bloom.fromValue = 5
            bloom.toValue = 1.5
            bloom.duration = 0.3
            bloom.timingFunction = CAMediaTimingFunction(name: .easeOut)
            focus.add(bloom, forKey: "bloom")
        }
    }
}

// MARK: - Bar

/// The find bar: query field, result count, case and regex toggles, and
/// previous / next / close buttons.
@MainActor
final class SearchBar: NSView {
    let field = NSTextField()
    let caseButton = IconButton(symbol: "textformat", pointSize: 11, help: "Match Case")
    let regexButton = IconButton(symbol: "asterisk", pointSize: 11, help: "Regular Expression")
    private let count = NSTextField(labelWithString: "")
    private let icon = NSImageView()
    private lazy var upButton = IconButton(symbol: "chevron.up", pointSize: 11, help: "Previous Match (↩)") { [weak self] in self?.onStep?(-1) }
    private lazy var downButton = IconButton(symbol: "chevron.down", pointSize: 11, help: "Next Match (⇧↩)") { [weak self] in self?.onStep?(1) }
    private lazy var closeButton = IconButton(symbol: "xmark", pointSize: 10, help: "Close (Esc)") { [weak self] in self?.onClose?() }

    var onStep: ((Int) -> Void)?
    var onClose: (() -> Void)?
    var onToggleCase: (() -> Void)?
    var onToggleRegex: (() -> Void)?

    static let size = NSSize(width: 330, height: 34)

    override init(frame: NSRect) {
        super.init(frame: NSRect(origin: .zero, size: Self.size))
        wantsLayer = true
        layer?.cornerRadius = 9
        layer?.cornerCurve = .continuous
        layer?.borderWidth = 1
        shadow = NSShadow()
        layer?.shadowOpacity = 0.3
        layer?.shadowRadius = 12
        layer?.shadowOffset = CGSize(width: 0, height: -4)

        field.isBordered = false
        field.drawsBackground = false
        field.focusRingType = .none
        field.font = .systemFont(ofSize: 13)
        field.cell?.isScrollable = true
        field.cell?.wraps = false
        field.lineBreakMode = .byClipping
        field.setAccessibilityLabel("Find")
        count.font = .monospacedDigitSystemFont(ofSize: 11, weight: .medium)
        count.alignment = .right
        icon.image = NSImage(systemSymbolName: "magnifyingglass", accessibilityDescription: nil)?
            .withSymbolConfiguration(.init(pointSize: 12, weight: .medium))
        caseButton.action = { [weak self] in self?.onToggleCase?() }
        regexButton.action = { [weak self] in self?.onToggleRegex?() }
        for view in [icon, field, count, caseButton, regexButton, upButton, downButton, closeButton] as [NSView] { addSubview(view) }
        applyTheme()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }

    func applyTheme() {
        let theme = AppConfig.shared.theme
        layer?.backgroundColor = theme.popover.cgColor
        layer?.borderColor = theme.paneStroke.cgColor
        field.textColor = theme.textPrimary
        field.placeholderAttributedString = NSAttributedString(string: "Find", attributes: [
            .foregroundColor: theme.textTertiary, .font: NSFont.systemFont(ofSize: 13),
        ])
        icon.contentTintColor = theme.textSecondary
        for button in [caseButton, regexButton, upButton, downButton, closeButton] {
            button.tint = theme.textSecondary
            button.hoverTint = theme.textPrimary
            button.hoverFill = theme.hover
        }
        appearance = theme.appearance
    }

    func update(count matches: Int, current: Int?, hasQuery: Bool) {
        let theme = AppConfig.shared.theme
        if !hasQuery {
            count.stringValue = ""
        } else if matches == 0 {
            count.stringValue = "No results"
            count.textColor = theme.error.withAlphaComponent(0.85)
        } else {
            count.stringValue = "\((current ?? 0) + 1) of \(matches)"
            count.textColor = theme.textSecondary
        }
        upButton.alphaValue = matches > 0 ? 1 : 0.4
        downButton.alphaValue = matches > 0 ? 1 : 0.4
    }

    override func layout() {
        super.layout()
        let h = bounds.height
        icon.frame = NSRect(x: 10, y: (h - 16) / 2, width: 16, height: 16)
        var x = bounds.width - 6
        for button in [closeButton, downButton, upButton, regexButton, caseButton] {
            x -= 22
            button.frame = NSRect(x: x, y: (h - 22) / 2, width: 22, height: 22)
            if button === upButton { x -= 4 }
        }
        let countWidth: CGFloat = 70
        count.frame = NSRect(x: x - countWidth - 4, y: (h - 14) / 2, width: countWidth, height: 14)
        let fieldX: CGFloat = 32
        field.frame = NSRect(x: fieldX, y: (h - 18) / 2, width: max(40, count.frame.minX - fieldX - 4), height: 18)
    }
}
