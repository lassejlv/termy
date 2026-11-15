import AppKit
import Termy
import TermyAppKit
import TermyNextCore

/// What a pane's program reports through OSC 7501 (the Program Status
/// Protocol), for tab badges and the Activity popup. Nothing is guessed from
/// the screen or the process tree: no report means idle.
enum PaneActivity: Equatable {
    case idle, working, blocked, done, error

    init(_ state: ProgramState) {
        switch state {
        case .idle: self = .idle
        case .working: self = .working
        case .blocked: self = .blocked
        case .done: self = .done
        case .error: self = .error
        }
    }

    var label: String {
        switch self {
        case .idle: "Idle"
        case .working: "Working"
        case .blocked: "Needs attention"
        case .done: "Done"
        case .error: "Error"
        }
    }

    /// Higher wins when a tab or session summarizes its panes.
    var urgency: Int {
        switch self {
        case .blocked: 4
        case .error: 3
        case .done: 2
        case .working: 1
        case .idle: 0
        }
    }
}

@MainActor
protocol PaneDelegate: AnyObject {
    func paneDidBecomeFocused(_ pane: PaneView)
    func pane(_ pane: PaneView, requestSplit direction: Direction)
    func paneRequestFloat(_ pane: PaneView)
    func paneRequestZoom(_ pane: PaneView)
    func paneRequestClose(_ pane: PaneView)
    func paneDidExit(_ pane: PaneView)
    func paneDidChange(_ pane: PaneView)
    func pane(_ pane: PaneView, draggedHeaderBy delta: NSPoint)
}

/// A pane: a rounded card holding a header and a libtermy `TerminalView`.
@MainActor
final class PaneView: NSView, TerminalViewDelegate {
    /// Stable across relaunches for a hosted pane; plugins see it as `block:<id>`.
    let id: UUID
    let terminal: TerminalView
    weak var delegate: PaneDelegate?

    /// The pane in the session host that runs this pane's program.
    let paneID: String?
    /// Names an in-app pane to its zsh plugin (`TERMY_NEXT_PANE`).
    private let inAppShellToken: String?
    /// How this pane's zsh plugin names it: the host's pane ID, or ours.
    var shellToken: String? { paneID ?? inAppShellToken }
    /// The zsh plugin's completion popup and ⇧⌘E command box.
    private(set) lazy var shellUI = PaneShellUI(pane: self)
    /// The program the host started for this pane, usually the shell.
    private(set) var shellPID: pid_t?
    private var processName: String?
    private var processPath: String?
    private var reportedApp: String? {
        terminal.session.programStatus.mostUrgent?.app ?? terminal.session.programStatus.first?.app
    }
    /// The program in front: an OSC 7501 app name when one is reported, else the process.
    var foregroundProgram: String? { reportedApp ?? processName }
    /// The front process's executable, for its app icon; nil when OSC 7501 names the app.
    var foregroundExecutable: String? { reportedApp == nil ? processPath : nil }
    private(set) var directory: String?
    /// Whether the shell itself is in front, so typing `cd` lands at a prompt.
    private(set) var isShellInFront = true
    private(set) var label = PaneLabel(name: "shell", detail: nil)
    private(set) var activity: PaneActivity = .idle
    /// Set once the user has looked at a `done` report; the badge then clears.
    private var seenDone = false

    var isFocused = false { didSet { if oldValue != isFocused { focusChanged() } } }
    /// Faded when another pane in the tab has focus.
    var isDimmed = false { didSet { if oldValue != isDimmed { updateDim() } } }
    var isFloating = false { didSet { updateShape(); needsLayout = true } }
    var isZoomed = false { didSet { header.needsDisplay = true } }

    private let header = PaneHeaderView()
    /// Rounds the terminal's corners without clipping the card's shadow.
    private let clip = NSView()
    private var glass: GlassView?
    private var hovering = false { didSet { header.showsButtons = hovering || isFocused } }
    /// While set, the terminal keeps this size as the card animates around it,
    /// so the grid resizes once instead of on every frame.
    var terminalSizeOverride: NSSize?
    /// Closing panes animate out and are skipped by layout.
    var isClosing = false

    var compact: Bool { Preferences.shared.paneStyle == .compact && !isFloating }
    var headerHeight: CGFloat { compact ? 0 : 30 }

    /// Starts a new pane: in the session host, or inside the app when the
    /// session host is turned off in Settings.
    /// `session` names the pane's session to plugins (REX_SESSION); `environment`
    /// is added on top of the pane's own, and `keepOpen` keeps the pane on
    /// screen once its program exits.
    convenience init(workingDirectory: String?, command: String? = nil, session: UUID? = nil,
                     environment extra: [String: String] = [:], keepOpen: Bool = false) throws {
        let app = AppConfig.shared
        let blockID = UUID()
        var options = TerminalOptions(config: app.termyConfig, workingDirectory: workingDirectory, startupCommand: command)
        options.environment["TERM_PROGRAM"] = "TermyNext"
        options.environment.merge(PluginEnvironment.pane(block: blockID, session: session)) { $1 }
        if Preferences.shared.usesSessionHost {
            options.environment.merge(ShellPlugins.shared.environment(token: nil)) { $1 }
            options.environment.merge(extra) { $1 }
            let terminal = try SessionHost.shared.requireMultiplexer().createTerminal(size: Self.initialSize, options: options)
            if let paneID = terminal.paneID { PluginIdentity.remember(block: blockID, hostPane: paneID) }
            self.init(id: blockID, hostTerminal: terminal, directory: workingDirectory)
        } else {
            let token = UUID().uuidString
            options.environment.merge(ShellPlugins.shared.environment(token: token)) { $1 }
            options.environment.merge(extra) { $1 }
            let terminal = try Terminal(size: Self.initialSize, options: options)
            self.init(id: blockID, hostTerminal: terminal, directory: workingDirectory, shellToken: token)
        }
        self.keepOpen = keepOpen
        runsCommand = command != nil
    }

    /// Kept on screen after its program exits (Rex's `keep_open`).
    var keepOpen = false
    /// Started for a command rather than the login shell.
    private(set) var runsCommand = false
    /// The program ended while the pane stays open.
    private(set) var hasExited = false
    /// How a command pane's program exited, as termy-run reports it.
    private(set) var exitCode: Int32?
    private let startedAt = Date()
    /// A name a plugin gave the pane (a Rex block's `label`), shown in its header.
    var customLabel: String? {
        didSet {
            guard oldValue != customLabel else { return }
            if let paneID { PluginIdentity.remember(label: customLabel, hostPane: paneID) }
            updateLabel()
        }
    }

    /// Whether the pane's program lives in the session host and outlives the app.
    var isHosted: Bool { paneID != nil }

    /// Attaches to a pane the session host already runs, such as one from before a relaunch.
    convenience init(attaching paneID: String) throws {
        let terminal = try SessionHost.shared.requireMultiplexer().attachTerminal(paneID: paneID)
        let blockID = PluginIdentity.blockID(forHostPane: paneID) ?? UUID()
        PluginIdentity.remember(block: blockID, hostPane: paneID)
        self.init(id: blockID, hostTerminal: terminal, directory: nil)
        customLabel = PluginIdentity.label(forHostPane: paneID)
    }

    /// A first grid size; the view resizes the pane to fit once laid out.
    private static var initialSize: TerminalSize {
        let render = AppConfig.shared.renderConfig
        return TerminalSize(cols: 80, rows: 24, cellWidth: render.cellWidth, cellHeight: render.cellHeight)
    }

    private init(id: UUID, hostTerminal: Terminal, directory: String?, shellToken: String? = nil) {
        self.id = id
        let app = AppConfig.shared
        let session = TerminalSession(terminal: hostTerminal, size: Self.initialSize)
        terminal = TerminalView(session: session, renderConfig: app.terminalRenderConfig)
        terminal.optionAsAlt = (try? app.termyConfig.native().macosOptionAsAlt) ?? false
        paneID = hostTerminal.paneID
        inAppShellToken = shellToken
        // A pane inside the app knows the shell it started on its PTY; the
        // session host runs the program of a hosted one.
        shellPID = hostTerminal.paneID == nil ? hostTerminal.childPID : SessionHost.shared.childPID(for: hostTerminal.paneID)
        self.directory = directory
        super.init(frame: NSRect(x: 0, y: 0, width: 600, height: 400))
        wantsLayer = true
        layer?.masksToBounds = false
        terminal.delegate = self
        clip.wantsLayer = true
        clip.layer?.masksToBounds = true
        clip.layer?.cornerCurve = .continuous
        clip.addSubview(terminal)
        addSubview(clip)
        addSubview(header)
        header.pane = self
        updateShape()
        applyTerminalColors()
        appliedStyle = Self.currentStyle
        refreshProcess()
        ShellPlugins.shared.register(self)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    /// Detaches from a hosted pane, whose program keeps running in the host;
    /// ends an in-app pane's program.
    func stop() {
        shellUI.stopAgent()
        terminal.session.stop()
    }

    /// Ends the pane's program in the host, then detaches.
    func terminate() {
        SessionHost.shared.close(paneID: paneID)
        stop()
    }

    // MARK: Layout and drawing

    override var isFlipped: Bool { true }

    /// The terminal's frame inside a card of `size`.
    func terminalRect(forCard size: NSSize) -> NSRect {
        let inset: CGFloat = compact ? 0 : 1
        let top = compact ? 0 : headerHeight + inset
        let side: CGFloat = compact ? 0 : 2
        return NSRect(x: side, y: top, width: max(0, size.width - side * 2), height: max(0, size.height - top - (compact ? 0 : 4)))
    }

    override func layout() {
        super.layout()
        let inset: CGFloat = compact ? 0 : 1
        header.frame = NSRect(x: inset, y: inset, width: bounds.width - inset * 2, height: headerHeight)
        header.isHidden = compact
        glass?.frame = bounds
        clip.frame = terminalRect(forCard: bounds.size)
        let size = terminalSizeOverride ?? clip.bounds.size
        if terminal.frame.size != size || terminal.frame.origin != .zero {
            // Never animate the grid itself; only the card around it moves.
            Motion.instantly { terminal.frame = NSRect(origin: .zero, size: size) }
        }
        if let search {
            let barSize = NSSize(width: min(SearchBar.size.width, clip.frame.width - 16), height: SearchBar.size.height)
            search.bar.frame = NSRect(x: clip.frame.maxX - barSize.width - 8, y: clip.frame.minY + 8,
                                      width: barSize.width, height: barSize.height)
            search.spotlight.frame = terminal.frame
        }
        shellUI.position()
        updateShadowPath()
    }

    private var radius: CGFloat { compact ? 0 : (AppConfig.shared.theme.isLiquid ? 14 : 10) }

    private func updateShape() {
        guard let layer else { return }
        let theme = AppConfig.shared.theme
        let liquid = theme.isLiquid && !compact
        layer.cornerRadius = radius
        layer.cornerCurve = .continuous
        if liquid {
            if glass == nil {
                let view = GlassView(cornerRadius: radius)
                addSubview(view, positioned: .below, relativeTo: clip)
                glass = view
            }
            glass?.cornerRadius = radius
            glass?.tint = isFocused ? theme.focusedPaneGlassTint : theme.paneGlassTint
            layer.backgroundColor = NSColor.clear.cgColor
            layer.borderWidth = 0
        } else {
            glass?.removeFromSuperview()
            glass = nil
            layer.backgroundColor = theme.paneFill.cgColor
            layer.borderWidth = compact ? 0 : 0.7
            layer.borderColor = (isFocused && !compact ? theme.paneStroke.withAlphaComponent(0.16) : theme.paneStroke).cgColor
        }
        layer.shadowColor = NSColor.black.cgColor
        layer.shadowOpacity = isFloating ? 0.45 : 0
        layer.shadowRadius = 24
        layer.shadowOffset = CGSize(width: 0, height: -8)
        clip.layer?.cornerRadius = compact ? 0 : radius - 2
        updateShadowPath()
    }

    private func updateShadowPath() {
        layer?.shadowPath = isFloating ? CGPath(roundedRect: bounds, cornerWidth: radius, cornerHeight: radius, transform: nil) : nil
    }

    /// Points this pane's font is zoomed by from the config's size (⌘+ / ⌘−).
    /// Every pane zooms on its own.
    var zoom: Float = 0 {
        didSet {
            guard zoom != oldValue else { return }
            terminal.apply(AppConfig.shared.terminalRenderConfig(zoom: zoom))
        }
    }

    /// Zooms by `step` points; nil goes back to the config's size. Shows the
    /// new size in the pane for a moment.
    func changeZoom(by step: Float?) {
        let app = AppConfig.shared
        let target = step.map { app.fontSize(zoom: zoom + $0) - app.baseFontSize } ?? 0
        zoom = target
        showZoomBadge()
        SessionHost.shared.scheduleSave()
    }

    private var zoomBadge: NSView?

    private func showZoomBadge() {
        let size = AppConfig.shared.fontSize(zoom: zoom)
        let points = size.rounded() == size ? "\(Int(size)) pt" : String(format: "%.1f pt", size)
        showBadge(zoom == 0 ? "\(points) · default" : points)
    }

    /// A short note in the middle of the pane for a moment, such as the font
    /// size or "Copied".
    private func showBadge(_ text: String) {
        zoomBadge?.removeFromSuperview()
        let label = NSTextField(labelWithString: text)
        label.font = .monospacedDigitSystemFont(ofSize: 12, weight: .semibold)
        label.textColor = AppConfig.shared.theme.textPrimary
        label.sizeToFit()
        let badge = NSView(frame: NSRect(x: 0, y: 0, width: label.frame.width + 20, height: label.frame.height + 10))
        badge.wantsLayer = true
        badge.layer?.backgroundColor = AppConfig.shared.theme.popover.withAlphaComponent(0.92).cgColor
        badge.layer?.cornerRadius = badge.frame.height / 2
        badge.layer?.borderWidth = 1
        badge.layer?.borderColor = AppConfig.shared.theme.paneStroke.cgColor
        label.frame.origin = NSPoint(x: 10, y: 5)
        badge.addSubview(label)
        badge.frame.origin = NSPoint(x: (bounds.width - badge.frame.width) / 2, y: (bounds.height - badge.frame.height) / 2)
        badge.autoresizingMask = [.minXMargin, .maxXMargin, .minYMargin, .maxYMargin]
        addSubview(badge)
        zoomBadge = badge
        Motion.appear(badge, scale: 0.9, duration: 0.16)
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.9) { [weak self, weak badge] in
            guard let badge, self?.zoomBadge === badge else { return }
            Motion.disappear(badge) { badge.removeFromSuperview() }
        }
    }

    /// What the pane's look was last built from: the config, the interface
    /// and pane styles, and light or dark.
    private struct Style: Equatable {
        var config: ObjectIdentifier
        var interface: InterfaceStyle
        var pane: PaneStyle
        var appearance: Appearance
    }

    private var appliedStyle: Style?

    private static var currentStyle: Style {
        let app = AppConfig.shared
        return Style(config: ObjectIdentifier(app.termyConfig), interface: Preferences.shared.interfaceStyle,
                     pane: Preferences.shared.paneStyle, appearance: app.appearance)
    }

    /// Catches up on config and style changes made while no window showed
    /// the pane: a window restyles only the session it shows.
    func themeChangedIfStale() {
        if appliedStyle != Self.currentStyle { themeChanged() }
    }

    func themeChanged() {
        appliedStyle = Self.currentStyle
        terminal.apply(AppConfig.shared.terminalRenderConfig(zoom: zoom))
        applyTerminalColors()
        search?.bar.applyTheme()
        search?.spotlight.applyTheme()
        shellUI.applyTheme()
        updateShape()
        needsLayout = true
        header.needsDisplay = true
    }

    /// The config and light or dark the terminal's color answers come from.
    private var appliedColors: (config: ObjectIdentifier, appearance: Appearance)?

    /// Makes the terminal answer color queries (OSC 4/10/11, `CSI ? 996 n`)
    /// with the theme on screen, so programs that pick light or dark colors
    /// from them, like Claude Code, Codex and Gemini CLI, match it. Programs
    /// that turned on `?2031` are told when it changes.
    private func applyTerminalColors() {
        let app = AppConfig.shared
        let config = ObjectIdentifier(app.termyConfig)
        let appearance = app.appearance
        if let appliedColors, appliedColors.config == config, appliedColors.appearance == appearance { return }
        appliedColors = (config, appearance)
        try? terminal.session.terminal.applyColors(from: app.termyConfig, appearance: appearance)
    }

    private func focusChanged() {
        shellUI.lineChanged()
        header.showsButtons = hovering || isFocused
        header.needsDisplay = true
        if isFocused, activity == .done {
            seenDone = true
            recomputeActivity()
        }
        if isFocused {
            PaneNotifier.shared.paneWasSeen(self)
            PaneHandoff.paneFocused(self)
        }
        updateShape()
        updateDim()
    }

    private func updateDim() {
        Motion.animate(0.25) {
            terminal.animator().alphaValue = isDimmed ? 0.7 : 1
            header.animator().alphaValue = isDimmed ? 0.75 : 1
        }
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        removeOwnTrackingAreas()
        addTrackingArea(NSTrackingArea(rect: bounds, options: [.mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect], owner: self))
    }

    override func mouseEntered(with event: NSEvent) { hovering = true }
    override func mouseExited(with event: NSEvent) { hovering = false }

    func focus() {
        window?.makeFirstResponder(terminal)
    }

    // MARK: Find

    private var search: PaneSearch?
    var isSearching: Bool { search != nil }

    /// Opens the find bar, or focuses it, starting from the selected text.
    func showSearch() {
        if search == nil {
            let search = PaneSearch(terminal: terminal)
            search.onClose = { [weak self] in self?.closeSearch() }
            clip.addSubview(search.spotlight, positioned: .above, relativeTo: terminal)
            addSubview(search.bar)
            self.search = search
            needsLayout = true
            layoutSubtreeIfNeeded()
            Motion.appear(search.bar, scale: 0.96, offsetY: -6, duration: 0.18)
        }
        search?.focusField(prefill: terminal.selection?.text(in: terminal.session.frame))
    }

    func closeSearch() {
        guard let search else { return }
        self.search = nil
        search.spotlight.show(matches: [], active: false, pulseCurrent: false)
        Motion.disappear(search.bar) { search.bar.removeFromSuperview() }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) { search.spotlight.removeFromSuperview() }
        focus()
    }

    /// Next (-1, older) or previous (+1, newer) match; opens find when closed.
    func findStep(_ direction: Int) {
        if let search { search.step(direction) } else { showSearch() }
    }

    func searchTick() {
        search?.tick()
        shellUI.position()
    }

    /// The zsh plugin reported a new command line, or went away.
    func shellLineChanged() { shellUI.lineChanged() }

    // MARK: Process and status

    /// Re-reads the foreground program and directory from the kernel.
    func refreshProcess() {
        if shellPID == nil, isHosted { shellPID = SessionHost.shared.childPID(for: paneID) }
        if let shell = shellPID, ShellProcess.isAlive(shell) {
            let front = ShellProcess.foreground(of: shell)
            processName = ShellProcess.name(of: front) ?? ShellProcess.name(of: shell)
            processPath = front == shell ? nil : ShellProcess.executablePath(of: front)
            isShellInFront = front == shell
            // A tool that's a script names itself after the script, so `brew`
            // (bash running brew.sh, then ruby running brew.rb) gets its logo.
            if !isShellInFront, let script = ShellProcess.scriptName(arguments: ShellProcess.arguments(of: front)),
               ProgramLogos.aliases[AppIcons.normalized(script)] != nil {
                processName = script
                processPath = nil
            }
            if let cwd = ShellProcess.workingDirectory(of: front) ?? ShellProcess.workingDirectory(of: shell) {
                directory = cwd
            }
        }
        if let reported = terminal.session.workingDirectory.flatMap(Naming.pathFromWorkingDirectoryReport) {
            directory = reported
        }
        updateLabel()
        recomputeActivity()
    }

    private func updateLabel() {
        let shellName = shellPID.flatMap(ShellProcess.name(of:))
        let process = processName.map(AppIcons.displayName(for:)) ?? shellName
        // A program that names itself over OSC 7501 wins over whatever the shell put in the title.
        var new = reportedApp.map { PaneLabel(name: AppIcons.displayName(for: $0), detail: directory.map { Naming.abbreviatingHome($0) }) }
            ?? PaneLabel.make(title: terminal.session.title, process: process, directory: directory)
        if let customLabel, !customLabel.isEmpty { new = PaneLabel(name: customLabel, detail: new.detail) }
        if new != label {
            label = new
            header.needsDisplay = true
            delegate?.paneDidChange(self)
        }
    }

    /// The most urgent OSC 7501 record, which the badge and Activity popup describe.
    var status: ProgramStatusRecord? { terminal.session.programStatus.mostUrgent }

    /// What the record says in a few words: the message, the blocked kind, or progress.
    var statusDetail: String? {
        guard let status else { return nil }
        var parts: [String] = []
        switch (status.state, status.kind) {
        case (.blocked, .permission?): parts.append("Needs permission")
        case (.blocked, .question?): parts.append("Has a question")
        case (.blocked, .auth?): parts.append("Needs sign-in")
        case (.working, _): parts.append(status.progress.map { "Working \($0)%" } ?? "Working")
        default: parts.append(activity.label)
        }
        if let msg = status.msg, !msg.isEmpty { parts.append(msg) }
        return parts.joined(separator: " · ")
    }

    /// Who sent each OSC 7501 record, and when: the process in front when
    /// the record last changed, as Rex's `program_status` reports `owner`.
    private(set) var statusOwners: [String: (pid: pid_t, name: String, updated: Date)] = [:]
    private var lastRecords: [String: ProgramStatusRecord] = [:]

    private func noteStatusOwners(_ records: [ProgramStatusRecord]) {
        let front = foregroundPID
        let name = front.flatMap(ShellProcess.name(of:)) ?? ""
        var seen: [String: ProgramStatusRecord] = [:]
        for record in records {
            seen[record.id] = record
            if lastRecords[record.id] != record || statusOwners[record.id] == nil {
                statusOwners[record.id] = (front ?? 0, name, Date())
            }
        }
        statusOwners = statusOwners.filter { seen[$0.key] != nil }
        lastRecords = seen
    }

    /// The program the pane started (usually the shell), and the one in front.
    var childPID: pid_t? { shellPID }
    var foregroundPID: pid_t? {
        guard let shell = shellPID, ShellProcess.isAlive(shell) else { return nil }
        return ShellProcess.foreground(of: shell)
    }

    /// The state the program last reported, for notifying on changes.
    private var lastReported: PaneActivity = .idle
    private let attachedAt = Date()

    private func recomputeActivity() {
        let reported = status.map { PaneActivity($0.state) } ?? .idle
        if reported != lastReported {
            // A pane that just attached replays its status; that isn't news.
            if Date().timeIntervalSince(attachedAt) > 2 {
                PaneNotifier.shared.pane(self, changedFrom: lastReported, to: reported)
            }
            lastReported = reported
        }
        // "done" means the user hasn't seen the result yet.
        if reported != .done { seenDone = false } else if isFocused, window?.isKeyWindow == true { seenDone = true }
        let new: PaneActivity = reported == .done && seenDone ? .idle : reported
        if new != activity {
            activity = new
            header.needsDisplay = true
            delegate?.paneDidChange(self)
        }
    }

    // MARK: TerminalViewDelegate

    func terminalView(_ view: TerminalView, didChangeTitle title: String?) {
        updateLabel()
        PluginEvents.blockEvent(self, "title_changed", ["title": title as Any])
    }

    func terminalView(_ view: TerminalView, didReceive event: TerminalEvent) {
        switch event {
        case .programStatus(let records):
            noteStatusOwners(records)
            refreshProcess()
            PluginEvents.blockEvent(self, "program_status", ["records": PluginAPI.statusRecords(of: self)])
        case .shellCommandFinished(let code):
            if runsCommand { exitCode = code }
            refreshProcess()
        case .workingDirectory:
            refreshProcess()
        default:
            break
        }
    }

    func terminalView(_ view: TerminalView, shouldOpen url: URL) -> Bool { true }

    /// OSC 52 copies, as Claude Code and Neovim send over SSH: allowed from
    /// the pane you're working in, never from one in the background.
    func terminalView(_ view: TerminalView, shouldWriteClipboard text: String) -> Bool {
        // Plugins can read what a program copied (Rex's clipboard_read).
        PluginAPI.shared.recordClipboard(text, from: self)
        guard isFocused, window?.isKeyWindow == true, NSApp.isActive else { return false }
        showBadge("Copied")
        return true
    }

    /// A file, image or text dragged over the pane: the pane lights up and
    /// says what dropping does.
    func terminalView(_ view: TerminalView, isDropTarget: Bool) {
        if isDropTarget {
            guard dropHighlight == nil else { return }
            let highlight = DropHighlightView(frame: terminal.frame)
            highlight.autoresizingMask = [.width, .height]
            clip.addSubview(highlight, positioned: .above, relativeTo: terminal)
            dropHighlight = highlight
            Motion.appear(highlight, scale: 0.98, duration: 0.14)
        } else if let highlight = dropHighlight {
            dropHighlight = nil
            Motion.disappear(highlight, scale: 1, duration: 0.14) { highlight.removeFromSuperview() }
        }
    }

    private var dropHighlight: DropHighlightView?

    /// Pane items after the terminal's own Copy, Paste and Select All.
    /// Right-clicking focuses the pane first, so they act on this one.
    func terminalView(_ view: TerminalView, willShowMenu menu: NSMenu) {
        for group in [[Command.clearScrollback, .find], [.splitRight, .splitDown], [.closePane]] {
            menu.addItem(.separator())
            group.forEach { menu.addItem($0.menuItem()) }
        }
    }

    func terminalView(_ view: TerminalView, didCopy text: String) {
        guard Preferences.shared.copyShine else { return }
        CopyShine.play(over: view.selectionRects, of: view)
    }

    func terminalViewDidExit(_ view: TerminalView) {
        hasExited = true
        PluginEvents.blockEvent(self, "child_exited", ["exit_code": exitCode.map { Int($0) } ?? -1])
        // Rex's keep_open: the output stays to read; ⌘W closes the pane. A
        // command that fails within moments stays too, to show why.
        let failedFast = runsCommand && (exitCode ?? 0) != 0 && Date().timeIntervalSince(startedAt) < 5
        if keepOpen || failedFast {
            if failedFast, !keepOpen {
                let note = "\r\n\u{1b}[2m[exited with status \(exitCode ?? -1) · ⌘W closes the pane]\u{1b}[0m\r\n"
                try? terminal.session.feed(Data(note.utf8))
            }
            header.needsDisplay = true
            return
        }
        // The window that last showed the pane may be gone.
        if let delegate { delegate.paneDidExit(self) } else { AppDelegate.shared.paneExitedOffScreen(self) }
    }

    // Header callbacks

    fileprivate func headerRequest(_ action: PaneHeaderView.Action) {
        switch action {
        case .splitRight: delegate?.pane(self, requestSplit: .right)
        case .splitDown: delegate?.pane(self, requestSplit: .down)
        case .float: delegate?.paneRequestFloat(self)
        case .zoom: delegate?.paneRequestZoom(self)
        case .close: delegate?.paneRequestClose(self)
        case .focus: focus()
        case .drag(let delta): delegate?.pane(self, draggedHeaderBy: delta)
        }
    }
}

/// The thin bar on top of a pane: program tile, title, directory, and the
/// split, float, zoom and close buttons.
@MainActor
final class PaneHeaderView: NSView {
    enum Action { case splitRight, splitDown, float, zoom, close, focus, drag(NSPoint) }

    weak var pane: PaneView?
    var showsButtons = false {
        didSet {
            guard oldValue != showsButtons else { return }
            updateButtons()
            Motion.animate(0.18) { for button in buttons { button.animator().alphaValue = showsButtons ? 1 : 0.3 } }
        }
    }
    private var buttons: [IconButton] = []
    private var dragOrigin: NSPoint?

    override init(frame: NSRect) {
        super.init(frame: frame)
        let specs: [(String, String, Action)] = [
            ("rectangle.split.2x1", Command.splitRight.help(), .splitRight),
            ("rectangle.split.1x2", Command.splitDown.help(), .splitDown),
            ("rectangle.on.rectangle", Command.floatPane.help(), .float),
            ("arrow.up.left.and.arrow.down.right", Command.zoomPane.help(), .zoom),
            ("xmark", Command.closePane.help(), .close),
        ]
        for (symbol, help, action) in specs {
            let button = IconButton(symbol: symbol, pointSize: 11.5, help: help) { [weak self] in
                self?.pane?.headerRequest(action)
            }
            button.alphaValue = 0.3
            buttons.append(button)
            addSubview(button)
        }
        updateButtons()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }
    override var mouseDownCanMoveWindow: Bool { false }

    override func layout() {
        super.layout()
        var x = bounds.width - 8
        for button in buttons.reversed() {
            x -= 22
            button.frame = NSRect(x: x, y: (bounds.height - 22) / 2, width: 22, height: 22)
            x -= 1
        }
    }

    private func updateButtons() {
        let theme = AppConfig.shared.theme
        for button in buttons {
            button.tint = theme.textSecondary
            button.hoverTint = theme.textPrimary
            button.hoverFill = theme.hover
        }
        if let zoom = buttons.dropLast().last {
            zoom.symbol = pane?.isZoomed == true ? "arrow.down.right.and.arrow.up.left" : "arrow.up.left.and.arrow.down.right"
        }
    }

    override func draw(_ dirtyRect: NSRect) {
        guard let pane else { return }
        updateButtons()
        let theme = AppConfig.shared.theme
        let tile = NSRect(x: 10, y: (bounds.height - 16) / 2, width: 16, height: 16)
        ProgramIcon.draw(program: pane.foregroundProgram, executable: pane.foregroundExecutable, in: tile, theme: theme)

        let maxX = (buttons.first?.frame.minX ?? bounds.width) - 8
        let titleFont = NSFont.systemFont(ofSize: 12.5, weight: .semibold)
        let detailFont = NSFont.systemFont(ofSize: 12.5, weight: .regular)
        let focusedAlpha: CGFloat = pane.isFocused ? 1 : 0.8
        let text = NSMutableAttributedString(string: pane.label.name, attributes: [
            .font: titleFont, .foregroundColor: theme.textPrimary.withAlphaComponent(focusedAlpha),
        ])
        if let detail = pane.label.detail {
            text.append(NSAttributedString(string: "  " + detail, attributes: [
                .font: detailFont, .foregroundColor: theme.textSecondary,
            ]))
        }
        let style = NSMutableParagraphStyle()
        style.lineBreakMode = .byTruncatingTail
        text.addAttribute(.paragraphStyle, value: style, range: NSRange(location: 0, length: text.length))
        let x = tile.maxX + 8
        let height = text.size().height
        text.draw(with: NSRect(x: x, y: (bounds.height - height) / 2, width: max(0, maxX - x), height: height),
                  options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
    }

    override func mouseDown(with event: NSEvent) {
        dragOrigin = event.locationInWindow
        pane?.headerRequest(.focus)
        if event.clickCount == 2 { pane?.headerRequest(.zoom) }
    }

    override func mouseDragged(with event: NSEvent) {
        if let drag = PaneDrag.current { return drag.update(event) }
        guard let origin = dragOrigin, let pane else { return }
        let now = event.locationInWindow
        if pane.isFloating {
            pane.headerRequest(.drag(NSPoint(x: now.x - origin.x, y: now.y - origin.y)))
            dragOrigin = now
        } else if hypot(now.x - origin.x, now.y - origin.y) > 5 {
            // A tiled pane lifts out and can go anywhere.
            PaneDrag.begin(pane: pane, event: event)
            PaneDrag.current?.update(event)
        }
    }

    override func mouseUp(with event: NSEvent) {
        dragOrigin = nil
        PaneDrag.current?.finish(event)
    }
}

/// The pane's look while something is dragged over it: a tinted edge and a
/// label saying the drop types a path.
private final class DropHighlightView: NSView {
    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    override func draw(_ dirtyRect: NSRect) {
        let theme = AppConfig.shared.theme
        theme.selectionFill.withAlphaComponent(0.08).setFill()
        bounds.fill()
        let edge = NSBezierPath(roundedRect: bounds.insetBy(dx: 3, dy: 3), xRadius: 8, yRadius: 8)
        edge.lineWidth = 2
        let dash: [CGFloat] = [6, 4]
        edge.setLineDash(dash, count: dash.count, phase: 0)
        theme.selectionFill.withAlphaComponent(0.8).setStroke()
        edge.stroke()

        let text = NSAttributedString(string: "Drop to insert the path", attributes: [
            .font: NSFont.systemFont(ofSize: 12, weight: .semibold), .foregroundColor: theme.selectionText,
        ])
        let size = text.size()
        let pill = NSRect(x: (bounds.width - size.width) / 2 - 12, y: (bounds.height - size.height) / 2 - 6,
                          width: size.width + 24, height: size.height + 12)
        theme.selectionFill.setFill()
        NSBezierPath(roundedRect: pill, xRadius: pill.height / 2, yRadius: pill.height / 2).fill()
        text.draw(at: NSPoint(x: pill.minX + 12, y: pill.minY + 6))
    }
}
