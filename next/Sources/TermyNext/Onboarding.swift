import AppKit
import TermyNextCore

/// The welcome tour: a glass card over the window, a page per idea, each
/// with a small animated Termy Next drawn in the theme's colors.
///
/// It shows once per `Onboarding.version`. Users who updated from a build
/// without it have seen version 0, so they get it on their next launch too;
/// raising the version shows a new tour to everyone once.
@MainActor
enum Onboarding {
    /// Raise to show the tour again to everyone, such as when it gains pages.
    static let version = 1

    private static let key = "onboardingVersionSeen"

    /// The tour version this user last finished or skipped; 0 for none.
    static var seenVersion: Int {
        get { UserDefaults.standard.integer(forKey: key) }
        set { UserDefaults.standard.set(newValue, forKey: key) }
    }

    static var isDue: Bool {
        seenVersion < version && ProcessInfo.processInfo.environment["TERMY_NEXT_ONBOARDING"] != "0"
    }

    /// Shows the tour in the front window when this user hasn't seen this version.
    static func presentIfDue() {
        guard isDue else { return }
        // After the window settles, so the tour doesn't fight the first layout.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) {
            guard isDue, let controller = AppDelegate.shared.keyController ?? AppDelegate.shared.controllersFrontToBack.first else { return }
            controller.presentOnboarding()
        }
    }

    static func finished() { seenVersion = max(seenVersion, version) }
}

// MARK: - Pages

/// One page of the tour.
@MainActor
struct OnboardingPage {
    enum Scene { case welcome, sessions, splits, palette, agents, shell, plugins, cli, done }

    /// A switch on the page that turns a feature on right there.
    struct Toggle {
        var title: String
        var get: @MainActor () -> Bool
        var set: @MainActor (Bool) -> Void
    }

    /// A choice between a few options, made right on the page.
    struct Choice {
        var title: String
        var options: [String]
        var get: @MainActor () -> Int
        var set: @MainActor (Int) -> Void
    }

    var scene: Scene
    var eyebrow: String
    var title: String
    var body: String
    var keys: [(keys: [String], label: String)] = []
    var toggle: Toggle?
    var choice: Choice?
    var link: (title: String, action: @MainActor () -> Void)?

    /// Keycaps for commands' shortcuts as they are now; a command without one is left out.
    private static func keys(_ entries: (Command, String)...) -> [(keys: [String], label: String)] {
        entries.compactMap { command, label in
            guard let shortcut = command.shortcutLabel else { return nil }
            let modifiers = shortcut.prefix { "⌃⌥⇧⌘".contains($0) }
            return (modifiers.map(String.init) + [String(shortcut.dropFirst(modifiers.count))], label)
        }
    }

    static var all: [OnboardingPage] {
        let prefs = Preferences.shared
        return [
            OnboardingPage(
                scene: .welcome, eyebrow: "Welcome",
                title: "Termy Next",
                body: "A session-first terminal. Programs run in Termy's session host, so quitting only detaches: everything is still running, in the same tabs and splits, when you come back."),
            OnboardingPage(
                scene: .sessions, eyebrow: "Sessions",
                title: "A window shows a session",
                body: "A session is a named group of tabs, one per window. Switch between them, start new ones, and close a window without ending anything: its session waits in the switcher.",
                keys: keys((.changeSession, "Change session"), (.newSession, "New session"), (.newTab, "New tab"))),
            OnboardingPage(
                scene: .splits, eyebrow: "Panes",
                title: "Split, float and zoom",
                body: "Every tab is a split layout of panes. Drag a pane's header to move it anywhere, float one above the rest, or zoom it to fill the tab.",
                keys: keys((.splitRight, "Split right"), (.splitDown, "Split down"), (.floatPane, "Float pane"), (.zoomPane, "Zoom pane"))),
            OnboardingPage(
                scene: .palette, eyebrow: "Command palette",
                title: "Everything is one search away",
                body: "Commands, tabs, sessions, tasks from your config and plugin actions, all in one list. If you forget a shortcut, it's next to the command here.",
                keys: keys((.commandPalette, "Command palette"), (.goToDirectory, "Go to directory"))),
            OnboardingPage(
                scene: .agents, eyebrow: "Agents",
                title: "See what your agents are doing",
                body: "Claude Code, Codex and other programs report their state, and tabs show it: a loader while working, a blue dot when one needs you, green when done. The Activity popup lists them all.",
                keys: keys((.toggleActivity, "Activity")),
                toggle: Toggle(title: "Notify me when a program needs me", get: { prefs.notificationsEnabled },
                               set: { prefs.notificationsEnabled = $0 })),
            OnboardingPage(
                scene: .shell, eyebrow: "Shell",
                title: "Completions under the prompt",
                body: "A small zsh plugin shows history and commands as you type, and \(Command.askForCommand.shortcutLabel ?? "Ask for a Command") turns a description into a command with Claude Code or Codex. It applies to panes you open after turning it on.",
                keys: [(["⌃", "N"], "Next completion")] + keys((.askForCommand, "Ask for a command")),
                toggle: Toggle(title: "Turn on the zsh plugin", get: { prefs.shellPlugins }, set: { prefs.shellPlugins = $0 })),
            OnboardingPage(
                scene: .plugins, eyebrow: "Plugins",
                title: "Bring your own commands",
                body: "Write actions and key bindings in Lua with termy.*. Plugins made for Rex, such as leaf, run as they are: Termy Next reads Rex's config too, and their actions land in the palette.",
                toggle: Toggle(title: "Load Rex plugins", get: { PluginHost.loadsRexConfig },
                               set: { UserDefaults.standard.set($0, forKey: "loadsRexPlugins"); PluginHost.shared.reload() }),
                link: ("Open Plugin Config", { AppDelegate.shared.openPluginConfig() })),
            OnboardingPage(
                scene: .cli, eyebrow: "Command line",
                title: "termy, from any shell",
                body: "Open splits and tabs, run commands, read a pane's screen and send it keys, from your scripts or an agent. Panes already have termy; install it to use it everywhere.",
                keys: [(["termy split -- npm test"], "Split beside you"), (["termy --help"], "Every command")],
                toggle: Toggle(title: "Install the termy command", get: { CLIInstall.isInstalled },
                               set: { on in
                                   if on { CLIInstall.install(attachedTo: NSApp.keyWindow, quiet: true) } else { CLIInstall.uninstall() }
                               })),
            OnboardingPage(
                scene: .done, eyebrow: "You're set",
                title: "Make it yours",
                body: "Pick a theme, or import the one Ghostty, Alacritty, kitty or Rex uses, and how the window looks. Fonts, tab placement and keyboard shortcuts are in Settings; the tour is in the Help menu.",
                choice: Choice(title: "Interface", options: ["Liquid", "Classic"],
                               get: { prefs.interfaceStyle == .classic ? 1 : 0 },
                               set: { prefs.interfaceStyle = $0 == 1 ? .classic : .liquid }),
                link: ("Open Settings", { AppDelegate.shared.showSettings() })),
        ]
    }
}

// MARK: - The card

@MainActor
final class OnboardingView: GlassPanel {
    var onDismiss: (() -> Void)?

    private let pages = OnboardingPage.all
    private var index = 0
    private let scene = OnboardingSceneView()
    private let eyebrow = NSTextField(labelWithString: "")
    private let title = NSTextField(labelWithString: "")
    private let body = NSTextField(wrappingLabelWithString: "")
    private let keys = NSStackView()
    private let toggleRow = NSStackView()
    private let toggleSwitch = NSSwitch()
    private let toggleLabel = NSTextField(labelWithString: "")
    private let linkButton = OnboardingButton(title: "", style: .link)
    private let choiceRow = NSStackView()
    private let choiceLabel = NSTextField(labelWithString: "")
    private let choiceControl = NSSegmentedControl()
    private let content = OnboardingColumn()
    private let dots = OnboardingDots()
    private let skip = OnboardingButton(title: "Skip", style: .plain)
    private let back = OnboardingButton(title: "Back", style: .secondary)
    private let next = OnboardingButton(title: "Next", style: .primary)
    /// The theme library, over the scene on the last page.
    private let themeBox = NSView()
    private let themePicker = ThemePicker()

    static let preferredSize = NSSize(width: 780, height: 470)

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.cornerRadius = 18
        layer?.cornerCurve = .continuous
        layer?.masksToBounds = false
        layer?.shadowColor = NSColor.black.cgColor
        layer?.shadowOpacity = 0.35
        layer?.shadowRadius = 40
        layer?.shadowOffset = CGSize(width: 0, height: -14)

        addSubview(scene)
        addSubview(themeBox)
        addSubview(content)
        themeBox.isHidden = true
        themeBox.addSubview(themePicker)
        NSLayoutConstraint.activate([
            themePicker.topAnchor.constraint(equalTo: themeBox.topAnchor),
            themePicker.leadingAnchor.constraint(equalTo: themeBox.leadingAnchor),
            themePicker.trailingAnchor.constraint(equalTo: themeBox.trailingAnchor),
            themePicker.bottomAnchor.constraint(equalTo: themeBox.bottomAnchor),
        ])
        themePicker.onPick = { theme in AppDelegate.shared.useTheme(theme.id) }
        themePicker.onImport = { id in AppDelegate.shared.useTheme(id) }
        eyebrow.font = .systemFont(ofSize: 11.5, weight: .semibold)
        title.font = .systemFont(ofSize: 26, weight: .bold)
        title.lineBreakMode = .byWordWrapping
        title.maximumNumberOfLines = 2
        body.font = .systemFont(ofSize: 13.5)
        body.maximumNumberOfLines = 0
        keys.orientation = .vertical
        keys.alignment = .leading
        keys.spacing = 8
        toggleRow.orientation = .horizontal
        toggleRow.spacing = 10
        toggleRow.alignment = .centerY
        toggleSwitch.controlSize = .small
        toggleSwitch.target = self
        toggleSwitch.action = #selector(toggled)
        toggleLabel.font = .systemFont(ofSize: 13, weight: .medium)
        toggleRow.addArrangedSubview(toggleSwitch)
        toggleRow.addArrangedSubview(toggleLabel)
        choiceRow.orientation = .horizontal
        choiceRow.spacing = 12
        choiceRow.alignment = .centerY
        choiceLabel.font = .systemFont(ofSize: 13, weight: .medium)
        choiceControl.segmentStyle = .rounded
        choiceControl.trackingMode = .selectOne
        choiceControl.target = self
        choiceControl.action = #selector(chose)
        choiceRow.addArrangedSubview(choiceLabel)
        choiceRow.addArrangedSubview(choiceControl)
        for view in [eyebrow, title, body, keys, toggleRow, choiceRow, linkButton] { content.addSubview(view) }
        for view in [dots, skip, back, next] { addSubview(view) }
        skip.onClick = { [weak self] in self?.dismiss() }
        back.onClick = { [weak self] in self?.go(to: (self?.index ?? 0) - 1) }
        next.onClick = { [weak self] in self?.advance() }
        linkButton.onClick = { [weak self] in
            guard let self, let link = self.pages[self.index].link else { return }
            link.action()
        }
        dots.count = pages.count
        dots.onSelect = { [weak self] in self?.go(to: $0) }
        applyTheme()
        show(0, animated: false)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var acceptsFirstResponder: Bool { true }

    func applyTheme() {
        let theme = AppConfig.shared.theme
        if theme.isLiquid {
            setGlass(true, cornerRadius: 18, tint: theme.popover.withAlphaComponent(0.55))
            layer?.backgroundColor = NSColor.clear.cgColor
        } else {
            setGlass(false)
            layer?.backgroundColor = theme.popover.cgColor
        }
        layer?.borderWidth = 0.5
        layer?.borderColor = theme.paneStroke.cgColor
        eyebrow.textColor = theme.accent
        title.textColor = theme.textPrimary
        body.textColor = theme.textSecondary
        toggleLabel.textColor = theme.textPrimary
        choiceLabel.textColor = theme.textPrimary
        [dots, skip, back, next, linkButton].forEach { $0.needsDisplay = true }
        scene.applyTheme()
        themePicker.applyTheme(theme)
        refreshThemes()
        for case let row as OnboardingKeyRow in keys.arrangedSubviews { row.applyTheme() }
    }

    override func layout() {
        super.layout()
        let inset: CGFloat = 14
        let sceneWidth = floor(bounds.width * 0.5)
        scene.frame = NSRect(x: inset, y: inset, width: sceneWidth - inset, height: bounds.height - inset * 2)
        themeBox.frame = scene.frame.insetBy(dx: 16, dy: 18)
        let left = sceneWidth + 30
        let width = bounds.width - left - 32
        content.frame = NSRect(x: left, y: 34, width: width, height: bounds.height - 34 - 72)
        layoutContent(width: width)
        let barY = bounds.height - 56
        next.frame = NSRect(x: bounds.width - 32 - next.intrinsicWidth, y: barY, width: next.intrinsicWidth, height: 32)
        back.frame = NSRect(x: next.frame.minX - 10 - back.intrinsicWidth, y: barY, width: back.intrinsicWidth, height: 32)
        skip.frame = NSRect(x: left - 10, y: barY, width: skip.intrinsicWidth, height: 32)
        let dotsX = skip.isHidden ? left - 10 : skip.frame.maxX + 4
        let dotsEnd = back.isHidden ? next.frame.minX : back.frame.minX
        dots.frame = NSRect(x: dotsX, y: barY + 11, width: max(0, dotsEnd - dotsX - 12), height: 10)
    }

    override var isFlipped: Bool { true }

    private func layoutContent(width: CGFloat) {
        var y: CGFloat = 0
        func place(_ view: NSView, height: CGFloat, gap: CGFloat) {
            view.frame = NSRect(x: 0, y: y, width: width, height: height)
            y += height + gap
        }
        func height(_ field: NSTextField) -> CGFloat {
            field.preferredMaxLayoutWidth = width
            return ceil(field.cell?.cellSize(forBounds: NSRect(x: 0, y: 0, width: width, height: 1000)).height ?? 20)
        }
        place(eyebrow, height: 15, gap: 6)
        place(title, height: height(title), gap: 10)
        place(body, height: height(body), gap: 18)
        if !keys.arrangedSubviews.isEmpty {
            let keysHeight = CGFloat(keys.arrangedSubviews.count) * 24 + CGFloat(keys.arrangedSubviews.count - 1) * 8
            place(keys, height: keysHeight, gap: 18)
        }
        if !toggleRow.isHidden { place(toggleRow, height: 22, gap: 12) }
        if !choiceRow.isHidden { place(choiceRow, height: 24, gap: 12) }
        if !linkButton.isHidden {
            linkButton.frame = NSRect(x: -6, y: y, width: linkButton.intrinsicWidth, height: 26)
        }
    }

    // MARK: Pages

    private func show(_ newIndex: Int, animated: Bool) {
        index = max(0, min(newIndex, pages.count - 1))
        let page = pages[index]
        let apply = { [self] in
            eyebrow.stringValue = page.eyebrow.uppercased() + "  ·  \(index + 1) of \(pages.count)"
            title.stringValue = page.title
            body.stringValue = page.body
            keys.arrangedSubviews.forEach { $0.removeFromSuperview() }
            for key in page.keys { keys.addArrangedSubview(OnboardingKeyRow(keys: key.keys, label: key.label)) }
            toggleRow.isHidden = page.toggle == nil
            if let toggle = page.toggle {
                toggleLabel.stringValue = toggle.title
                toggleSwitch.state = toggle.get() ? .on : .off
            }
            choiceRow.isHidden = page.choice == nil
            if let choice = page.choice {
                choiceLabel.stringValue = choice.title
                choiceControl.segmentCount = choice.options.count
                for (i, option) in choice.options.enumerated() {
                    choiceControl.setLabel(option, forSegment: i)
                    choiceControl.setWidth(0, forSegment: i)
                }
                choiceControl.selectedSegment = choice.get()
                choiceControl.setAccessibilityLabel(choice.title)
            }
            linkButton.isHidden = page.link == nil
            linkButton.title = page.link.map { $0.title + " →" } ?? ""
            back.isHidden = index == 0
            // "Get Started" already dismisses, and its width needs the room.
            skip.isHidden = index == pages.count - 1
            next.title = index == pages.count - 1 ? "Get Started" : index == 0 ? "Take the Tour" : "Next"
            dots.selected = index
            themeBox.isHidden = page.scene != .done
            if page.scene == .done { refreshThemes() }
            needsLayout = true
            layoutSubtreeIfNeeded()
        }
        scene.show(page.scene, animated: animated)
        guard animated, !Motion.reduced else { return apply() }
        Motion.crossfade(content, duration: 0.22)
        apply()
        Motion.appear(content, scale: 0.985, offsetY: 6, duration: 0.3)
    }

    private func refreshThemes() {
        let app = AppConfig.shared
        let current = app.followsAppearance ? app.themeSetting(for: app.appearance) : app.themeSetting
        themePicker.update(themes: app.themes, current: current ?? app.renderConfig.activeTheme)
    }

    private func go(to newIndex: Int) {
        guard newIndex != index, pages.indices.contains(newIndex) else { return }
        show(newIndex, animated: true)
    }

    private func advance() {
        if index == pages.count - 1 { dismiss() } else { go(to: index + 1) }
    }

    @objc private func chose() {
        pages[index].choice?.set(choiceControl.selectedSegment)
    }

    @objc private func toggled() {
        guard let toggle = pages[index].toggle else { return }
        toggle.set(toggleSwitch.state == .on)
        // A switch that couldn't take effect (a cancelled install) flips back.
        toggleSwitch.state = toggle.get() ? .on : .off
    }

    private var isDismissing = false

    func dismiss() {
        guard !isDismissing else { return }
        isDismissing = true
        Onboarding.finished()
        scene.stop()
        Motion.disappear(self, scale: 0.97, duration: 0.2) { [weak self] in
            self?.removeFromSuperview()
            self?.onDismiss?()
        }
    }

    override func keyDown(with event: NSEvent) {
        switch event.keyCode {
        case 124, 36, 76, 49: advance()
        case 123: go(to: index - 1)
        case 53: dismiss()
        default: super.keyDown(with: event)
        }
    }
}

/// Lays its subviews out from the top.
@MainActor
private final class OnboardingColumn: NSView {
    override var isFlipped: Bool { true }
}

// MARK: - Controls

/// A rounded button drawn in the theme: filled for the main action.
@MainActor
final class OnboardingButton: NSView {
    enum Style { case primary, secondary, plain, link }

    var title: String { didSet { needsDisplay = true } }
    var onClick: (() -> Void)?
    private let style: Style
    private var hovering = false { didSet { needsDisplay = true } }
    private var pressed = false { didSet { needsDisplay = true } }

    init(title: String, style: Style) {
        self.title = title
        self.style = style
        super.init(frame: .zero)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }

    /// A press in a window that isn't key still counts, as for NSButton.
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    // A button to VoiceOver and other assistive apps.
    override func isAccessibilityElement() -> Bool { !isHidden }
    override func accessibilityRole() -> NSAccessibility.Role? { style == .link ? .link : .button }
    override func accessibilityLabel() -> String? { title }
    override func accessibilityPerformPress() -> Bool {
        onClick?()
        return true
    }

    private var font: NSFont { .systemFont(ofSize: 13, weight: style == .primary ? .semibold : .medium) }

    var intrinsicWidth: CGFloat {
        ceil((title as NSString).size(withAttributes: [.font: font]).width) + (style == .primary ? 36 : 26)
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: bounds, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self))
    }

    override func mouseEntered(with event: NSEvent) { hovering = true }
    override func mouseExited(with event: NSEvent) { hovering = false }
    override func mouseDown(with event: NSEvent) { pressed = true }
    override func mouseUp(with event: NSEvent) {
        pressed = false
        if bounds.contains(convert(event.locationInWindow, from: nil)) { onClick?() }
    }
    override func resetCursorRects() { addCursorRect(bounds, cursor: .pointingHand) }

    override func draw(_ dirtyRect: NSRect) {
        let theme = AppConfig.shared.theme
        let rect = bounds.insetBy(dx: 0.5, dy: 0.5)
        let path = NSBezierPath(roundedRect: rect, xRadius: rect.height / 2, yRadius: rect.height / 2)
        let text: NSColor
        switch style {
        case .primary:
            theme.accent.blended(withFraction: pressed ? 0.2 : hovering ? 0.08 : 0, of: .black)?.setFill()
            path.fill()
            text = theme.selectionText
        case .secondary:
            theme.foreground.withAlphaComponent(pressed ? 0.16 : hovering ? 0.12 : 0.08).setFill()
            path.fill()
            text = theme.textPrimary
        case .plain:
            if hovering { theme.hover.setFill(); path.fill() }
            text = hovering ? theme.textPrimary : theme.textSecondary
        case .link:
            if hovering { theme.hover.setFill(); path.fill() }
            text = theme.accent
        }
        let attributes: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: text]
        let size = (title as NSString).size(withAttributes: attributes)
        (title as NSString).draw(at: NSPoint(x: (bounds.width - size.width) / 2, y: (bounds.height - size.height) / 2), withAttributes: attributes)
    }
}

/// Page dots; the current one stretches into a pill.
@MainActor
final class OnboardingDots: NSView {
    var count = 0 { didSet { needsDisplay = true } }
    var selected = 0 {
        didSet {
            guard oldValue != selected else { return }
            if !Motion.reduced { Motion.crossfade(self, duration: 0.18) }
            needsDisplay = true
        }
    }
    var onSelect: ((Int) -> Void)?

    override var isFlipped: Bool { true }

    private func frames() -> [NSRect] {
        var x = (bounds.width - CGFloat(count - 1) * 12 - 22) / 2
        return (0..<count).map { i in
            let width: CGFloat = i == selected ? 22 : 6
            defer { x += width + 6 }
            return NSRect(x: x, y: (bounds.height - 6) / 2, width: width, height: 6)
        }
    }

    override func draw(_ dirtyRect: NSRect) {
        let theme = AppConfig.shared.theme
        for (i, rect) in frames().enumerated() {
            (i == selected ? theme.accent : theme.foreground.withAlphaComponent(0.2)).setFill()
            NSBezierPath(roundedRect: rect, xRadius: 3, yRadius: 3).fill()
        }
    }

    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        if let i = frames().firstIndex(where: { $0.insetBy(dx: -4, dy: -8).contains(point) }) { onSelect?(i) }
    }
}

/// A shortcut drawn as keycaps, with what it does.
@MainActor
final class OnboardingKeyRow: NSView {
    private let keys: [String]
    private let label: String

    init(keys: [String], label: String) {
        self.keys = keys
        self.label = label
        super.init(frame: .zero)
        translatesAutoresizingMaskIntoConstraints = false
        heightAnchor.constraint(equalToConstant: 24).isActive = true
        widthAnchor.constraint(equalToConstant: 300).isActive = true
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }

    func applyTheme() { needsDisplay = true }

    override func draw(_ dirtyRect: NSRect) {
        let theme = AppConfig.shared.theme
        let font = NSFont.systemFont(ofSize: 12, weight: .semibold)
        var x: CGFloat = 0
        for key in keys {
            let attributes: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: theme.textPrimary]
            let size = (key as NSString).size(withAttributes: attributes)
            let cap = NSRect(x: x, y: 1, width: max(24, size.width + 14), height: 22)
            let shadow = NSBezierPath(roundedRect: cap.offsetBy(dx: 0, dy: 1.5), xRadius: 6, yRadius: 6)
            theme.foreground.withAlphaComponent(0.1).setFill()
            shadow.fill()
            let face = NSBezierPath(roundedRect: cap, xRadius: 6, yRadius: 6)
            theme.popover.mixed(with: theme.foreground, by: 0.08).setFill()
            face.fill()
            theme.paneStroke.setStroke()
            face.lineWidth = 0.5
            face.stroke()
            (key as NSString).draw(at: NSPoint(x: cap.midX - size.width / 2, y: cap.midY - size.height / 2), withAttributes: attributes)
            x = cap.maxX + 4
        }
        let attributes: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 13), .foregroundColor: theme.textSecondary]
        let size = (label as NSString).size(withAttributes: attributes)
        (label as NSString).draw(at: NSPoint(x: x + 8, y: (bounds.height - size.height) / 2 + 1), withAttributes: attributes)
    }
}

// MARK: - Scenes

/// A miniature Termy Next, drawn with layers in the theme's colors, that acts
/// out each page's idea on a loop.
@MainActor
final class OnboardingSceneView: NSView {
    private let stage = CALayer()
    private var timer: Timer?
    private var step = 0
    private var kind: OnboardingPage.Scene = .welcome
    private var parts: [String: CALayer] = [:]

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.cornerRadius = 12
        layer?.cornerCurve = .continuous
        layer?.masksToBounds = true
        stage.isGeometryFlipped = true
        layer?.addSublayer(stage)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    deinit { MainActor.assumeIsolated { timer?.invalidate() } }

    func stop() {
        timer?.invalidate()
        timer = nil
    }

    override func layout() {
        super.layout()
        if stage.frame != bounds {
            stage.frame = bounds
            build()
        }
    }

    override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        build()
    }

    func applyTheme() { build() }

    func show(_ scene: OnboardingPage.Scene, animated: Bool) {
        kind = scene
        step = 0
        if animated, !Motion.reduced {
            let transition = CATransition()
            transition.type = .push
            transition.subtype = .fromRight
            transition.duration = 0.35
            transition.timingFunction = Motion.curve
            stage.add(transition, forKey: "page")
        }
        build()
        timer?.invalidate()
        guard !Motion.reduced else { return }
        timer = Timer.scheduledTimer(withTimeInterval: interval, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.tick() }
        }
    }

    private var interval: TimeInterval {
        switch kind {
        case .welcome, .shell: 0.55
        case .agents: 0.4
        case .palette: 0.9
        default: 1.5
        }
    }

    private func tick() {
        step += 1
        CATransaction.begin()
        CATransaction.setAnimationDuration(0.55)
        CATransaction.setAnimationTimingFunction(Motion.curve)
        apply(step)
        CATransaction.commit()
    }

    // MARK: Drawing

    private var theme: Theme { AppConfig.shared.theme }
    private var scale: CGFloat { window?.backingScaleFactor ?? 2 }

    private func box(_ name: String, _ rect: CGRect, fill: NSColor, radius: CGFloat = 6, stroke: NSColor? = nil, in parent: CALayer? = nil) -> CALayer {
        let layer = CALayer()
        layer.frame = rect
        layer.backgroundColor = fill.cgColor
        layer.cornerRadius = radius
        layer.cornerCurve = .continuous
        if let stroke {
            layer.borderColor = stroke.cgColor
            layer.borderWidth = 0.5
        }
        (parent ?? stage).addSublayer(layer)
        parts[name] = layer
        return layer
    }

    private func text(_ name: String, _ string: String, at point: CGPoint, size: CGFloat = 10, color: NSColor, weight: NSFont.Weight = .regular,
                      mono: Bool = false, in parent: CALayer? = nil) -> CATextLayer {
        let layer = CATextLayer()
        let font = mono ? NSFont.monospacedSystemFont(ofSize: size, weight: weight) : NSFont.systemFont(ofSize: size, weight: weight)
        layer.string = NSAttributedString(string: string, attributes: [.font: font, .foregroundColor: color])
        let width = ceil((string as NSString).size(withAttributes: [.font: font]).width) + 2
        layer.frame = CGRect(x: point.x, y: point.y, width: width, height: size * 1.4)
        layer.contentsScale = scale
        (parent ?? stage).addSublayer(layer)
        parts[name] = layer
        return layer
    }

    /// Faint lines of "output" in a pane.
    private func lines(_ name: String, in pane: CALayer, count: Int, top: CGFloat = 26, accent: Int? = nil, width paneWidth: CGFloat? = nil) {
        for i in 0..<count {
            let width = (paneWidth ?? pane.bounds.width) * [0.62, 0.44, 0.7, 0.36, 0.55, 0.48, 0.66][i % 7]
            let color = i == accent ? theme.accent.withAlphaComponent(0.8) : theme.foreground.withAlphaComponent(i == 0 ? 0.5 : 0.22)
            _ = box("\(name).line\(i)", CGRect(x: 10, y: top + CGFloat(i) * 11, width: max(12, width - 20), height: 4.5), fill: color, radius: 2.25, in: pane)
        }
    }

    /// A pane card with a header strip.
    @discardableResult
    private func pane(_ name: String, _ rect: CGRect, title: String, focused: Bool = false, in parent: CALayer? = nil) -> CALayer {
        let fill = theme.terminalBackground.mixed(with: theme.foreground, by: focused ? 0.06 : 0.03)
        let card = box(name, rect, fill: fill, radius: 7, stroke: focused ? theme.accent.withAlphaComponent(0.55) : theme.paneStroke, in: parent)
        card.masksToBounds = true
        _ = box("\(name).icon", CGRect(x: 8, y: 6, width: 9, height: 9), fill: theme.accent.withAlphaComponent(0.75), radius: 2.5, in: card)
        _ = text("\(name).title", title, at: CGPoint(x: 21, y: 4), size: 8.5, color: theme.textSecondary, weight: .semibold, in: card)
        return card
    }

    /// The window: a backdrop, traffic lights, tab pills.
    private func window(tabs: [String], selected: Int = 0) -> CGRect {
        let backdrop = theme.terminalBackground.mixed(with: theme.isDark ? .black : .white, by: theme.isDark ? 0.35 : 0.4)
        stage.backgroundColor = backdrop.cgColor
        let frame = bounds.insetBy(dx: 26, dy: 34)
        let windowLayer = box("window", frame, fill: theme.chrome, radius: 12, stroke: theme.paneStroke)
        windowLayer.shadowColor = NSColor.black.cgColor
        windowLayer.shadowOpacity = theme.isDark ? 0.5 : 0.18
        windowLayer.shadowRadius = 18
        windowLayer.shadowOffset = CGSize(width: 0, height: 8)
        for (i, color) in [NSColor.systemRed, .systemYellow, .systemGreen].enumerated() {
            _ = box("light\(i)", CGRect(x: frame.minX + 12 + CGFloat(i) * 13, y: frame.minY + 11, width: 8, height: 8),
                    fill: color.withAlphaComponent(0.9), radius: 4)
        }
        var x = frame.minX + 62
        for (i, tab) in tabs.enumerated() {
            let width: CGFloat = 70
            if i == selected {
                _ = box("tab\(i)", CGRect(x: x, y: frame.minY + 6, width: width, height: 18), fill: theme.selected, radius: 9)
            }
            _ = text("tab\(i).label", tab, at: CGPoint(x: x + 10, y: frame.minY + 9), size: 8.5,
                     color: i == selected ? theme.textPrimary : theme.textTertiary, weight: .semibold)
            x += width + 4
        }
        return CGRect(x: frame.minX + 8, y: frame.minY + 30, width: frame.width - 16, height: frame.height - 38)
    }

    private func build() {
        stage.sublayers?.forEach { $0.removeFromSuperlayer() }
        parts = [:]
        guard bounds.width > 40 else { return }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        switch kind {
        case .welcome: buildWelcome()
        case .sessions: buildSessions()
        case .splits: buildSplits()
        case .palette: buildPalette()
        case .agents: buildAgents()
        case .shell: buildShell()
        case .plugins: buildPlugins()
        case .cli: buildCLI()
        case .done: buildDone()
        }
        apply(Motion.reduced ? 3 : 0)
        CATransaction.commit()
    }

    /// Moves the scene's parts to where `step` puts them.
    private func apply(_ step: Int) {
        switch kind {
        case .welcome:
            // The prompt types, then the line runs and starts over.
            let typed = parts["typed"]
            let full: CGFloat = 120
            let width = full * CGFloat(min(step % 9, 6)) / 6
            typed?.frame.size.width = width
            parts["cursor"]?.frame.origin.x = (typed?.frame.minX ?? 0) + width + 2
            parts["cursor"]?.opacity = step % 2 == 0 ? 1 : 0.2
            parts["output"]?.opacity = step % 9 >= 7 ? 1 : 0
        case .sessions:
            // The front session card goes to the back.
            let cards = ["card0", "card1", "card2"]
            for (i, name) in cards.enumerated() {
                guard let card = parts[name] else { continue }
                let depth = (i - step % 3 + 3) % 3
                card.frame.origin = CGPoint(x: bounds.midX - card.frame.width / 2 + CGFloat(depth) * 14,
                                            y: bounds.midY - card.frame.height / 2 - CGFloat(depth) * 14 + 8)
                card.opacity = Float(1 - Double(depth) * 0.28)
                card.zPosition = CGFloat(3 - depth)
            }
        case .splits:
            guard let area = parts["area"]?.frame else { return }
            let phase = step % 5
            let gap: CGFloat = 6
            var a = area, b = area, c = area
            switch phase {
            case 0:
                b = CGRect(x: area.maxX, y: area.minY, width: 0, height: area.height)
                c = CGRect(x: area.maxX, y: area.maxY, width: 0, height: 0)
            case 1:
                a.size.width = (area.width - gap) / 2
                b = CGRect(x: a.maxX + gap, y: area.minY, width: a.width, height: area.height)
                c = CGRect(x: area.maxX, y: area.maxY, width: 0, height: 0)
            case 2:
                a.size.width = (area.width - gap) / 2
                b = CGRect(x: a.maxX + gap, y: area.minY, width: a.width, height: (area.height - gap) / 2)
                c = CGRect(x: b.minX, y: b.maxY + gap, width: b.width, height: b.height)
            case 3:
                // c floats over the others.
                a.size.width = (area.width - gap) / 2
                b = CGRect(x: a.maxX + gap, y: area.minY, width: a.width, height: area.height)
                c = CGRect(x: area.midX - area.width * 0.3, y: area.midY - area.height * 0.25, width: area.width * 0.6, height: area.height * 0.5)
            default:
                // a zooms to fill.
                b = CGRect(x: area.maxX, y: area.minY, width: 0, height: area.height)
                c = CGRect(x: area.maxX, y: area.maxY, width: 0, height: 0)
            }
            parts["paneA"]?.frame = a
            parts["paneB"]?.frame = b
            parts["paneC"]?.frame = c
            parts["paneB"]?.opacity = b.width < 1 ? 0 : 1
            parts["paneC"]?.opacity = c.width < 1 ? 0 : 1
            // The floating pane stands out with the accent edge.
            parts["paneC"]?.borderColor = (phase == 3 ? theme.accent : theme.paneStroke).cgColor
            parts["paneC"]?.borderWidth = phase == 3 ? 1.5 : 0.5
            parts["paneC"]?.zPosition = 2
        case .palette:
            let row = step % 4
            if let first = parts["row0"] {
                parts["highlight"]?.frame.origin.y = first.frame.minY + CGFloat(row) * 22 - 4
            }
        case .agents:
            for i in 0..<3 {
                parts["dot\(i)"]?.opacity = (step % 3 == i) ? 1 : 0.3
            }
            parts["blocked"]?.transform = step % 6 < 3 ? CATransform3DMakeScale(1.25, 1.25, 1) : CATransform3DIdentity
        case .shell:
            let typed = parts["typed"]
            let width = 64 * CGFloat(min(step % 10, 4)) / 4
            typed?.frame.size.width = width
            parts["cursor"]?.frame.origin.x = (typed?.frame.minX ?? 0) + width + 2
            parts["popup"]?.opacity = step % 10 >= 4 ? 1 : 0
            if let first = parts["completion0"] {
                parts["completionHighlight"]?.frame.origin.y = first.frame.minY + CGFloat(max(0, (step % 10) - 5) % 3) * 16 - 3
            }
        case .plugins:
            parts["code"]?.opacity = 1
            parts["action"]?.opacity = step % 4 >= 1 ? 1 : 0
            parts["leaf"]?.opacity = step % 4 >= 2 ? 1 : 0
            parts["leaf"]?.transform = step % 4 >= 2 ? CATransform3DIdentity : CATransform3DMakeTranslation(0, 10, 0)
        case .cli:
            // Types the command, runs it, and the split opens beside it.
            let phase = step % 8
            let command = "termy split -- npm test"
            let shown = phase >= 3 ? command.count : command.count * phase / 3
            if let layer = parts["cmd"] as? CATextLayer {
                layer.string = NSAttributedString(string: String(command.prefix(shown)), attributes: [
                    .font: NSFont.monospacedSystemFont(ofSize: 8.5, weight: .medium), .foregroundColor: theme.foreground.withAlphaComponent(0.85),
                ])
            }
            let split = phase >= 3
            parts["cliOut"]?.opacity = split ? 1 : 0
            guard let area = parts["area"]?.frame else { return }
            let gap: CGFloat = 6
            var a = area, b = CGRect(x: area.maxX, y: area.minY, width: 0, height: area.height)
            if split {
                a.size.width = (area.width - gap) / 2
                b = CGRect(x: a.maxX + gap, y: area.minY, width: a.width, height: area.height)
            }
            parts["cliA"]?.frame = a
            parts["cliB"]?.frame = b
            parts["cliB"]?.opacity = split ? 1 : 0
        case .done:
            break
        }
    }

    // MARK: Scenes

    private func buildWelcome() {
        let area = window(tabs: ["zsh  ~/Dev"])
        let card = pane("main", area, title: "zsh  ~/Dev/termy", focused: true)
        let mono = NSFont.monospacedSystemFont(ofSize: 10, weight: .medium)
        _ = mono
        _ = text("prompt", "➜", at: CGPoint(x: 10, y: 28), size: 10, color: theme.done, weight: .bold, mono: true, in: card)
        let typed = box("typed", CGRect(x: 24, y: 31, width: 0, height: 6), fill: theme.foreground.withAlphaComponent(0.55), radius: 3, in: card)
        typed.anchorPoint = .zero
        typed.frame = CGRect(x: 24, y: 31, width: 0, height: 6)
        _ = box("cursor", CGRect(x: 26, y: 28, width: 6, height: 12), fill: theme.accent, radius: 1.5, in: card)
        let output = box("output", CGRect(x: 0, y: 48, width: card.bounds.width, height: 80), fill: .clear, in: card)
        lines("out", in: output, count: 5, top: 4, accent: 2)
        // The app's mark, large, in the corner.
        let mark = text("mark", "termy next", at: CGPoint(x: area.minX + 12, y: area.maxY - 36), size: 22,
                        color: theme.foreground.withAlphaComponent(0.12), weight: .heavy)
        mark.zPosition = -1
    }

    private func buildSessions() {
        stage.backgroundColor = theme.terminalBackground.mixed(with: theme.isDark ? .black : .white, by: 0.35).cgColor
        let names = ["shy-orchard", "fix-login", "release-notes"]
        let size = CGSize(width: bounds.width * 0.62, height: bounds.height * 0.52)
        for (i, name) in names.enumerated() {
            let card = box("card\(i)", CGRect(origin: .zero, size: size), fill: theme.chrome, radius: 12, stroke: theme.paneStroke)
            card.shadowColor = NSColor.black.cgColor
            card.shadowOpacity = 0.3
            card.shadowRadius = 12
            card.shadowOffset = CGSize(width: 0, height: 6)
            _ = text("card\(i).name", name, at: CGPoint(x: 14, y: 10), size: 11, color: theme.textPrimary, weight: .bold, in: card)
            _ = text("card\(i).meta", "\(3 - i) tabs", at: CGPoint(x: 14, y: 26), size: 9, color: theme.textTertiary, in: card)
            let inner = CGRect(x: 10, y: 44, width: size.width - 20, height: size.height - 54)
            let half = (inner.width - 6) / 2
            pane("card\(i).a", CGRect(x: inner.minX, y: inner.minY, width: i == 1 ? inner.width : half, height: inner.height), title: ["nvim", "claude", "zsh"][i], in: card)
            if i != 1 { pane("card\(i).b", CGRect(x: inner.minX + half + 6, y: inner.minY, width: half, height: inner.height), title: "zsh", in: card) }
            for part in ["card\(i).a", "card\(i).b"] { if let p = parts[part] { lines(part, in: p, count: 4) } }
        }
        _ = text("hint", "⌘K", at: CGPoint(x: bounds.maxX - 46, y: bounds.maxY - 34), size: 14, color: theme.accent, weight: .bold)
    }

    private func buildSplits() {
        let area = window(tabs: ["nvim", "server"])
        _ = box("area", area, fill: .clear)
        for (name, title, focused) in [("paneA", "nvim  ~/app", true), ("paneB", "npm run dev", false), ("paneC", "claude", false)] {
            let card = pane(name, area, title: title, focused: focused)
            // Sized for the narrowest the pane gets, so they fit once it's split.
            lines(name, in: card, count: 6, accent: name == "paneC" ? 1 : nil, width: area.width / 2)
        }
    }

    private func buildPalette() {
        let area = window(tabs: ["zsh", "logs", "claude"])
        let back = pane("back", area, title: "zsh  ~/Dev/termy")
        lines("back", in: back, count: 8)
        back.opacity = 0.55
        let width = area.width * 0.78
        let palette = box("palette", CGRect(x: area.midX - width / 2, y: area.minY + 14, width: width, height: 132),
                          fill: theme.popover, radius: 10, stroke: theme.paneStroke)
        palette.shadowColor = NSColor.black.cgColor
        palette.shadowOpacity = 0.4
        palette.shadowRadius = 16
        palette.shadowOffset = CGSize(width: 0, height: 8)
        _ = text("query", "Search commands, sessions, and tabs…", at: CGPoint(x: 12, y: 9), size: 9.5, color: theme.textTertiary, in: palette)
        _ = box("rule", CGRect(x: 0, y: 28, width: width, height: 0.5), fill: theme.paneStroke, radius: 0, in: palette)
        _ = box("highlight", CGRect(x: 6, y: 34, width: width - 12, height: 20), fill: theme.selectionFill.withAlphaComponent(0.5), radius: 6, in: palette)
        let rows = [("Split Right", "⌘D"), ("Leaf: Open Document…", ""), ("Run Task…", ""), ("shy-orchard", "session")]
        for (i, row) in rows.enumerated() {
            _ = text("row\(i)", row.0, at: CGPoint(x: 14, y: 38 + CGFloat(i) * 22), size: 10, color: theme.textPrimary, weight: .medium, in: palette)
            _ = text("row\(i).key", row.1, at: CGPoint(x: width - 54, y: 38 + CGFloat(i) * 22), size: 9, color: theme.textTertiary, in: palette)
        }
    }

    private func buildAgents() {
        let area = window(tabs: ["claude", "codex", "tests"], selected: 1)
        // Each tab's state: working (dots), blocked (blue), done (green).
        let frame = parts["window"]?.frame ?? area
        let tabY = frame.minY + 10
        for i in 0..<3 {
            _ = box("dot\(i)", CGRect(x: frame.minX + 62 + 48 + CGFloat(i) * 4, y: tabY + 4, width: 3, height: 3), fill: theme.working, radius: 1.5)
        }
        let blocked = box("blocked", CGRect(x: frame.minX + 62 + 74 + 50, y: tabY + 2, width: 7, height: 7), fill: theme.blocked, radius: 3.5)
        blocked.anchorPoint = CGPoint(x: 0.5, y: 0.5)
        _ = box("done", CGRect(x: frame.minX + 62 + 148 + 44, y: tabY + 2, width: 7, height: 7), fill: theme.done, radius: 3.5)
        let main = pane("main", area, title: "Codex  ~/Dev/app", focused: true)
        lines("main", in: main, count: 7, accent: 3)
        let width: CGFloat = 170
        let popup = box("activity", CGRect(x: area.maxX - width - 8, y: area.minY + 8, width: width, height: 96),
                        fill: theme.popover, radius: 10, stroke: theme.paneStroke)
        popup.shadowColor = NSColor.black.cgColor
        popup.shadowOpacity = 0.35
        popup.shadowRadius = 12
        let rows: [(String, String, NSColor)] = [("Claude Code", "Working · Running tests", theme.working),
                                                 ("Codex", "Needs permission", theme.blocked), ("tests", "Done", theme.done)]
        for (i, row) in rows.enumerated() {
            _ = box("activity.dot\(i)", CGRect(x: 12, y: 14 + CGFloat(i) * 27, width: 7, height: 7), fill: row.2, radius: 3.5, in: popup)
            _ = text("activity.name\(i)", row.0, at: CGPoint(x: 26, y: 8 + CGFloat(i) * 27), size: 9.5, color: theme.textPrimary, weight: .semibold, in: popup)
            _ = text("activity.msg\(i)", row.1, at: CGPoint(x: 26, y: 20 + CGFloat(i) * 27), size: 8.5, color: theme.textTertiary, in: popup)
        }
    }

    private func buildShell() {
        let area = window(tabs: ["zsh"])
        let card = pane("main", area, title: "zsh  ~/Dev/termy", focused: true)
        lines("history", in: card, count: 3)
        _ = text("prompt", "➜", at: CGPoint(x: 10, y: 62), size: 10, color: theme.done, weight: .bold, mono: true, in: card)
        _ = text("cmd", "git ch", at: CGPoint(x: 24, y: 62), size: 10, color: theme.textPrimary, weight: .medium, mono: true, in: card).opacity = 0
        let typed = box("typed", CGRect(x: 24, y: 65, width: 0, height: 6), fill: theme.foreground.withAlphaComponent(0.55), radius: 3, in: card)
        typed.anchorPoint = .zero
        typed.frame = CGRect(x: 24, y: 65, width: 0, height: 6)
        _ = box("cursor", CGRect(x: 26, y: 62, width: 6, height: 12), fill: theme.accent, radius: 1.5, in: card)
        let popup = box("popup", CGRect(x: 22, y: 82, width: 190, height: 56), fill: theme.popover, radius: 8, stroke: theme.paneStroke, in: card)
        popup.shadowColor = NSColor.black.cgColor
        popup.shadowOpacity = 0.3
        popup.shadowRadius = 10
        _ = box("completionHighlight", CGRect(x: 4, y: 5, width: 182, height: 15), fill: theme.selectionFill.withAlphaComponent(0.5), radius: 4, in: popup)
        for (i, item) in ["checkout main", "cherry-pick", "checkout -b feature"].enumerated() {
            _ = text("completion\(i)", "git " + item, at: CGPoint(x: 10, y: 7 + CGFloat(i) * 16), size: 9, color: theme.textPrimary, mono: true, in: popup)
        }
    }

    private func buildPlugins() {
        let area = window(tabs: ["init.lua", "leaf"])
        let half = (area.width - 6) / 2
        let editor = pane("editor", CGRect(x: area.minX, y: area.minY, width: half, height: area.height), title: "init.lua", focused: true)
        let code = box("code", editor.bounds, fill: .clear, in: editor)
        let tokens: [[(String, NSColor)]] = [
            [("termy", theme.accent), (".action{", theme.textPrimary)],
            [("  name = ", theme.textSecondary), ("\"standup\"", theme.done)],
            [("  title = ", theme.textSecondary), ("\"Standup\"", theme.done)],
            [("  run = ", theme.textSecondary), ("function", theme.blocked), ("(ctx)", theme.textPrimary)],
            [("    termy", theme.accent), (".session.new_split{…}", theme.textPrimary)],
            [("  end,", theme.textSecondary)],
            [("}", theme.textPrimary)],
            [("termy", theme.accent), (".bind(", theme.textPrimary), ("\"ctrl+s\"", theme.done), (")", theme.textPrimary)],
        ]
        for (row, parts) in tokens.enumerated() {
            var x: CGFloat = 10
            for (i, token) in parts.enumerated() {
                let layer = text("code\(row).\(i)", token.0, at: CGPoint(x: x, y: 26 + CGFloat(row) * 13), size: 8.5, color: token.1, mono: true, in: code)
                x = layer.frame.maxX - 2
            }
        }
        let reader = pane("leaf", CGRect(x: area.minX + half + 6, y: area.minY, width: half, height: area.height), title: "leaf  notes.md")
        lines("leaf", in: reader, count: 8, top: 30, accent: 0)
        let width = half * 0.92
        let action = box("action", CGRect(x: area.midX - width / 2, y: area.maxY - 34, width: width, height: 24),
                         fill: theme.popover, radius: 7, stroke: theme.accent.withAlphaComponent(0.6))
        action.shadowColor = NSColor.black.cgColor
        action.shadowOpacity = 0.35
        action.shadowRadius = 10
        _ = text("action.label", "Leaf: Open Document…", at: CGPoint(x: 10, y: 6), size: 9.5, color: theme.textPrimary, weight: .semibold, in: action)
    }

    private func buildCLI() {
        let area = window(tabs: ["zsh", "npm test"])
        _ = box("area", area, fill: .clear)
        let shell = pane("cliA", area, title: "zsh  ~/app", focused: true)
        _ = text("cliPrompt", "➜", at: CGPoint(x: 10, y: 26), size: 10, color: theme.done, weight: .bold, mono: true, in: shell)
        _ = text("cmd", "", at: CGPoint(x: 24, y: 27), size: 9, color: theme.foreground, weight: .medium, mono: true, in: shell)
        parts["cmd"]?.frame.size.width = 200
        let out = text("cliOut", "block:4f2c", at: CGPoint(x: 10, y: 44), size: 8.5, color: theme.textSecondary, mono: true, in: shell)
        out.opacity = 0
        let tests = pane("cliB", CGRect(x: area.maxX, y: area.minY, width: 0, height: area.height), title: "npm test")
        lines("cliB", in: tests, count: 6, accent: 4, width: area.width / 2)
    }

    /// A backdrop for the theme list the card puts over it.
    private func buildDone() {
        stage.backgroundColor = theme.terminalBackground.mixed(with: theme.isDark ? .black : .white, by: 0.35).cgColor
    }
}
