import AppKit
import Termy
import TermyNextCore

/// The Settings window (⌘,), laid out like macOS System Settings: a sidebar
/// of colored pane icons with a search field over it, and on the right the
/// pane's title above grouped, rounded cards of rows, each with its label on
/// the left and its control on the right. Every control writes straight to
/// `Preferences`, and the window follows changes made elsewhere, such as from
/// the View menu. It wears the terminal theme like the main window.
@MainActor
final class SettingsWindowController: NSWindowController, NSSearchFieldDelegate, NSWindowDelegate {
    static let shared = SettingsWindowController()

    private enum Pane: String, CaseIterable {
        case general, appearance, theme, notifications, shell, sessions

        var title: String {
            switch self {
            case .general: "General"
            case .appearance: "Appearance"
            case .theme: "Theme & Font"
            case .notifications: "Notifications"
            case .shell: "Shell"
            case .sessions: "Sessions"
            }
        }

        var symbol: String {
            switch self {
            case .general: "gearshape.fill"
            case .appearance: "circle.lefthalf.filled"
            case .theme: "paintpalette.fill"
            case .notifications: "bell.badge.fill"
            case .shell: "terminal.fill"
            case .sessions: "rectangle.stack.fill"
            }
        }

        var color: NSColor {
            switch self {
            case .general: .systemGray
            case .appearance: .systemBlue
            case .theme: .systemPink
            case .notifications: .systemRed
            case .shell: .systemGreen
            case .sessions: .systemOrange
            }
        }

        /// The sidebar starts a new group at these panes, as System Settings does.
        var startsGroup: Bool { self == .appearance || self == .shell }
    }

    private static let size = NSSize(width: 780, height: 580)
    private static let sidebarWidth: CGFloat = 220
    private static let headerHeight: CGFloat = 52
    private static var contentWidth: CGFloat { size.width - sidebarWidth }
    /// Width of the cards: the content less its side margins.
    private static var groupWidth: CGFloat { contentWidth - 40 }

    private var pane: Pane = .general
    private var paneViews: [Pane: NSView] = [:]
    /// Every word a pane shows, for the sidebar search.
    private var paneText: [Pane: String] = [:]

    // Chrome
    private let root = NSView()
    private let glass = NSVisualEffectView()
    private let tint = NSView()
    private let contentBackground = NSView()
    private let divider = NSView()
    private let sidebar = SettingsSidebar()
    private let search = NSSearchField()
    private let titleLabel = NSTextField(labelWithString: "")
    private let scroll = NSScrollView()

    /// Text recolored with the theme. Row titles dim while their control is off.
    private var rowTitles: [(label: NSTextField, control: NSControl?)] = []
    private var headers: [NSTextField] = []
    private var notes: [NSTextField] = []
    private var groups: [NSView] = []
    private var separators: [NSView] = []

    // General
    private let updateCheckbox = NSSwitch()
    private let updateDownload = NSSwitch()
    private let checkNowButton = NSButton(title: "Check Now", target: nil, action: nil)
    private let updateStatus = NSTextField(labelWithString: "")
    private let appName = NSTextField(labelWithString: "Termy Next")
    private let configLabel = NSTextField(labelWithString: "")
    private let startPopup = NSPopUpButton(frame: .zero, pullsDown: false)
    private let inheritSwitch = NSSwitch()
    private let warnAlwaysSwitch = NSSwitch()
    private let warnRunningSwitch = NSSwitch()

    // Appearance
    private let interfaceControl = NSSegmentedControl(labels: ["Liquid", "Classic"], trackingMode: .selectOne, target: nil, action: nil)
    private let tabPopup = NSPopUpButton(frame: .zero, pullsDown: false)
    private let paneControl = NSSegmentedControl(labels: ["Comfortable", "Compact"], trackingMode: .selectOne, target: nil, action: nil)
    private let loaderPicker = LoaderPicker(width: SettingsWindowController.groupWidth - 24)
    private let copyShineSwitch = NSSwitch()
    private let reduceMotionSwitch = NSSwitch()

    // Theme & Font
    private let fontPopup = NSPopUpButton(frame: .zero, pullsDown: false)
    private let sizeField = NSTextField()
    private let sizeStepper = NSStepper()
    private let themePicker = ThemePicker(width: SettingsWindowController.groupWidth - 24, height: 300)
    /// Every font family installed on this Mac, monospaced ones first;
    /// rebuilt when fonts are installed or removed.
    private var localFonts: (monospaced: [String], other: [String])?
    private var fontMenuKey: [String] = []

    // Notifications
    private let notifyCheckbox = NSSwitch()
    private let notifyBlocked = NSSwitch()
    private let notifyDone = NSSwitch()
    private let notifyFailed = NSSwitch()
    private let notifySound = NSSwitch()

    // Shell
    private let shellCheckbox = NSSwitch()
    private let specCheckbox = NSSwitch()
    private let arrowsCheckbox = NSSwitch()
    private let agentPopup = NSPopUpButton(frame: .zero, pullsDown: false)
    private let sshColorCheckbox = NSSwitch()

    // Sessions
    private let hostCheckbox = NSSwitch()
    private let hostStatus = NSTextField(labelWithString: "")
    private let hostFolderLabel = NSTextField(labelWithString: "")
    private let revealButton = NSButton(title: "Show in Finder", target: nil, action: nil)
    private let restartButton = NSButton(title: "Restart…", target: nil, action: nil)

    private var switches: [NSSwitch] {
        [inheritSwitch, warnAlwaysSwitch, warnRunningSwitch, copyShineSwitch, reduceMotionSwitch, updateCheckbox, updateDownload, notifyCheckbox, notifyBlocked, notifyDone, notifyFailed, notifySound,
         shellCheckbox, specCheckbox, arrowsCheckbox, sshColorCheckbox, hostCheckbox]
    }

    private init() {
        let window = NSWindow(
            contentRect: NSRect(origin: .zero, size: Self.size),
            styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
            backing: .buffered, defer: true)
        window.isReleasedWhenClosed = false
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        window.isOpaque = false
        window.backgroundColor = .clear
        // An empty unified toolbar gives the tall title bar System Settings has,
        // with the traffic lights centered in it over the sidebar.
        window.toolbar = NSToolbar(identifier: "TermyNextSettings")
        window.toolbarStyle = .unified
        // Only the height changes, as in System Settings.
        window.contentMinSize = NSSize(width: Self.size.width, height: 420)
        window.contentMaxSize = NSSize(width: Self.size.width, height: 4000)
        super.init(window: window)
        window.delegate = self

        buildChrome()
        configureControls()
        for pane in Pane.allCases { paneViews[pane] = build(pane) }

        NotificationCenter.default.addObserver(self, selector: #selector(refresh), name: Preferences.didChange, object: nil)
        NotificationCenter.default.addObserver(self, selector: #selector(refresh), name: Workspace.didChange, object: nil)
        NotificationCenter.default.addObserver(self, selector: #selector(refresh), name: Updates.didChange, object: nil)
        NotificationCenter.default.addObserver(self, selector: #selector(fontsChanged), name: NSFont.fontSetChangedNotification, object: nil)
        updateSidebar()
        select(.general)
        window.center()
        window.setFrameAutosaveName("TermyNextSettings")
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    func show() {
        refresh()
        showWindow(nil)
        window?.makeKeyAndOrderFront(nil)
    }

    // MARK: Chrome

    private func buildChrome() {
        let size = Self.size
        let sidebarWidth = Self.sidebarWidth
        root.frame = NSRect(origin: .zero, size: size)
        root.wantsLayer = true
        glass.material = .sidebar
        glass.blendingMode = .behindWindow
        glass.state = .active
        tint.wantsLayer = true
        for view in [glass, tint] {
            view.frame = root.bounds
            view.autoresizingMask = [.width, .height]
            root.addSubview(view)
        }

        contentBackground.wantsLayer = true
        contentBackground.frame = NSRect(x: sidebarWidth, y: 0, width: size.width - sidebarWidth, height: size.height)
        contentBackground.autoresizingMask = [.width, .height]
        root.addSubview(contentBackground)
        divider.wantsLayer = true
        divider.frame = NSRect(x: sidebarWidth, y: 0, width: 1, height: size.height)
        divider.autoresizingMask = [.height]
        root.addSubview(divider)

        // Sidebar: search under the traffic lights, then the panes.
        search.placeholderString = "Search"
        search.delegate = self
        search.sendsSearchStringImmediately = true
        search.target = self
        search.action = #selector(searchChanged)
        search.frame = NSRect(x: 10, y: size.height - Self.headerHeight - 28, width: sidebarWidth - 20, height: 28)
        search.autoresizingMask = [.minYMargin]
        root.addSubview(search)
        sidebar.frame = NSRect(x: 0, y: 0, width: sidebarWidth, height: size.height - Self.headerHeight - 38)
        sidebar.autoresizingMask = [.height]
        sidebar.onSelect = { [weak self] id in
            guard let self, let pane = Pane(rawValue: id) else { return }
            self.select(pane)
        }
        root.addSubview(sidebar)

        // Content: the pane's title level with the traffic lights, its rows below.
        titleLabel.font = .systemFont(ofSize: 15, weight: .bold)
        titleLabel.frame = NSRect(x: sidebarWidth + 20, y: size.height - Self.headerHeight / 2 - 10,
                                  width: size.width - sidebarWidth - 40, height: 20)
        titleLabel.autoresizingMask = [.minYMargin, .width]
        root.addSubview(titleLabel)
        scroll.frame = NSRect(x: sidebarWidth + 1, y: 0, width: size.width - sidebarWidth - 1, height: size.height - Self.headerHeight)
        scroll.autoresizingMask = [.width, .height]
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.automaticallyAdjustsContentInsets = false
        root.addSubview(scroll)

        window?.contentView = root
    }

    /// Shows `pane` in the content area.
    private func select(_ pane: Pane) {
        self.pane = pane
        window?.title = pane.title
        titleLabel.stringValue = pane.title
        sidebar.selected = pane.rawValue
        guard let view = paneViews[pane] else { return }
        if scroll.documentView !== view {
            scroll.documentView = view
            let clip = scroll.contentView
            NSLayoutConstraint.activate([
                view.topAnchor.constraint(equalTo: clip.topAnchor),
                view.leadingAnchor.constraint(equalTo: clip.leadingAnchor),
                view.widthAnchor.constraint(equalTo: clip.widthAnchor),
            ])
            clip.scroll(to: .zero)
            scroll.reflectScrolledClipView(clip)
        }
        refresh()
    }

    // MARK: Search

    @objc private func searchChanged() { updateSidebar() }

    func controlTextDidChange(_ obj: Notification) { updateSidebar() }

    /// Lists the panes whose title or rows match the search, and shows the
    /// first match when the current pane isn't one.
    private func updateSidebar() {
        let query = search.stringValue.trimmingCharacters(in: .whitespaces).lowercased()
        let words = query.split(separator: " ").map(String.init)
        let panes = Pane.allCases.filter { pane in
            words.allSatisfy { (paneText[pane] ?? pane.title.lowercased()).contains($0) }
        }
        sidebar.items = panes.map {
            SettingsSidebar.Item(id: $0.rawValue, title: $0.title, symbol: $0.symbol, color: $0.color,
                                 startsGroup: query.isEmpty && $0.startsGroup)
        }
        if let first = panes.first, !panes.contains(pane) { select(first) }
    }

    // MARK: Theme

    /// Dresses the window in the terminal theme, the way the main window is.
    private func applyTheme() {
        let theme = AppConfig.shared.theme
        window?.appearance = theme.appearance
        glass.isHidden = !theme.isLiquid
        glass.appearance = theme.appearance
        tint.layer?.backgroundColor = (theme.isLiquid ? theme.backdropTint : theme.chrome).cgColor
        contentBackground.layer?.backgroundColor = (theme.isLiquid
            ? theme.terminalBackground.withAlphaComponent(theme.isDark ? 0.45 : 0.4)
            : theme.paneFill).cgColor
        divider.layer?.backgroundColor = theme.paneStroke.cgColor
        titleLabel.textColor = theme.textPrimary
        appName.textColor = theme.textPrimary
        sidebar.needsDisplay = true

        for (label, control) in rowTitles {
            label.textColor = (control?.isEnabled ?? true) ? theme.textPrimary : theme.textTertiary
        }
        for header in headers { header.textColor = theme.textPrimary }
        for note in notes { note.textColor = theme.textSecondary }
        for group in groups {
            group.layer?.backgroundColor = theme.foreground.withAlphaComponent(theme.isDark ? 0.05 : 0.035).cgColor
            group.layer?.borderColor = theme.paneStroke.cgColor
        }
        for line in separators { line.layer?.backgroundColor = theme.paneStroke.cgColor }
        for label in [updateStatus, configLabel, hostStatus, hostFolderLabel] { label.textColor = theme.textSecondary }
        themePicker.applyTheme(theme)
        loaderPicker.applyTheme(theme)
    }

    // MARK: Panes

    private func build(_ pane: Pane) -> NSView {
        let sections: [Section]
        switch pane {
        case .general:
            sections = [
                Section(rows: [.full(aboutView())]),
                Section(header: "New Windows and Tabs", rows: [
                    .control("Start in", startPopup),
                    .control("Open in the current pane's folder", inheritSwitch),
                ], footer: "New windows start in this folder (working_dir in the config). With the switch on, new tabs, splits and sessions open where the focused pane is; off, they open in this folder too."),
                Section(header: "Quitting", rows: [
                    .control("Always ask before quitting", warnAlwaysSwitch),
                    .control("Ask while programs are running", warnRunningSwitch),
                ], footer: "A program is running when something other than the shell is in front of a pane. With the session host on, quitting doesn't end programs: they keep running and come back on the next launch. Programs inside the app end when it quits."),
                Section(header: "Updates", rows: [
                    .control("Check for updates automatically", updateCheckbox),
                    .control("Download and install them automatically", updateDownload),
                ], footer: Updates.shared.isAvailable
                    ? "Updates are installed when you quit or relaunch. Programs in panes keep running through it."
                    : "This build doesn't update itself. Releases are on GitHub, tagged next-v*."),
                Section(header: "Config File", rows: [
                    .control("Config file", row([button("Reload", #selector(reloadConfig)), button("Open in Editor", #selector(openConfig))]),
                             subtitle: configLabel),
                    .control("Import from Termy", button("Import…", #selector(importConfig)),
                             subtitle: noteLabel("Copies ~/.config/termy/config.txt and its local themes")),
                ], footer: "Termy Next keeps its own config, separate from the Termy app's. Import copies a Termy config over it, along with its local themes; the old file is kept as config.txt.bak."),
            ]
        case .appearance:
            sections = [
                Section(rows: [
                    .control("Interface style", interfaceControl),
                    .control("Tabs", tabPopup),
                    .control("Pane spacing", paneControl),
                ], footer: "Liquid puts the window chrome and panes on Liquid Glass; Classic draws them solid. Compact drops the gaps and headers around panes."),
                Section(header: "Tab Loader", rows: [
                    .full(loaderPicker),
                    .control("Spinners from loading.dev", button("Visit Site", #selector(openLoadingDev)),
                             subtitle: noteLabel("By Jakub Krehel and Paul Faivret, MIT License")),
                ], footer: "Shown at the end of a tab, and in the sidebar, while its program is working. Dots is Termy Next's own."),
                Section(header: "Motion", rows: [
                    .control("Shine when copying", copyShineSwitch),
                    .control("Reduce motion", reduceMotionSwitch),
                ], footer: "Reduce motion turns off sliding, scaling and the copy glint; panes and panels change in place. It's always on while Reduce Motion is on in System Settings › Accessibility › Display."),
            ]
        case .theme:
            sections = [
                Section(header: "Font", rows: [
                    .control("Font", fontPopup),
                    .control("Size", row([sizeField, sizeStepper, NSTextField(labelWithString: "pt")], spacing: 6)),
                ], footer: "⌘+ and ⌘− zoom only the pane you're in; ⌘0 puts it back to this size."),
                Section(header: "Theme", rows: [.full(themePicker)],
                        footer: "Termy's theme library. Add your own as JSON files in ~/.config/termy-next/themes."),
            ]
        case .notifications:
            sections = [
                Section(rows: [.control("Notify me about programs in panes", notifyCheckbox)]),
                Section(header: "Notify Me When a Program", rows: [
                    .control("Needs attention", notifyBlocked),
                    .control("Finishes", notifyDone),
                    .control("Fails", notifyFailed),
                    .control("Play a sound", notifySound),
                ], footer: "Programs report their status with OSC 7501, as Claude Code and other agents do. You're notified only about panes you aren't looking at; clicking a notification takes you to the pane."),
            ]
        case .shell:
            sections = [
                Section(header: "zsh Plugin", rows: [
                    .control("Completions and the ⇧⌘E command box", shellCheckbox),
                    .control("Command completions", specCheckbox, subtitle: noteLabel("Experimental")),
                    .control("Pick completions with ↑ and ↓", arrowsCheckbox),
                    .control("Ask with", agentPopup),
                ], footer: "A small zsh plugin shows matching history and commands under the prompt: ⌃N and ⌃P pick one, Return takes it. ⇧⌘E describes a command to Claude Code (claude -p) or Codex (codex exec), with the sign-in they already have. Applies to panes opened after you turn it on. Command completions add subcommands, options, branches, formulae and scripts for git, brew, cargo, rustup, rustc and bun."),
                Section(header: "SSH", rows: [
                    .control("Send 24-bit color support to SSH hosts", sshColorCheckbox),
                ], footer: "ssh in zsh and Add Remote Host send COLORTERM, so Claude Code and other programs on the server draw true colors. The server has to accept it (AcceptEnv COLORTERM), as Debian and Ubuntu do. Copying over SSH works through OSC 52 from the pane you're in."),
            ]
        case .sessions:
            sections = [
                Section(rows: [
                    .control("Keep programs running after Termy Next quits", hostCheckbox),
                ], footer: "Shells run in a Termy session host, so they keep running when you quit and reattach when you open Termy Next again. Turned off, new panes run inside the app and end when it quits. Panes that are already open keep running where they are."),
                Section(header: "Session Host", rows: [
                    .control("Status", restartButton, subtitle: hostStatus),
                    .control("Host folder", revealButton, subtitle: hostFolderLabel),
                ]),
            ]
        }
        paneText[pane] = ([pane.title] + sections.flatMap(\.searchText)).joined(separator: " ").lowercased()
        return paneView(sections)
    }

    private struct Section {
        var header: String?
        var rows: [Row]
        var footer: String?

        var searchText: [String] {
            [header, footer].compactMap { $0 } + rows.compactMap {
                if case .control(let title, _, _) = $0 { return title }
                return nil
            }
        }
    }

    private enum Row {
        /// A title (and an optional line under it) on the left, a control on the right.
        case control(String, NSView, subtitle: NSTextField? = nil)
        /// A view across the whole card.
        case full(NSView)
    }

    /// The scrolling page for a pane: each section's header, its card of
    /// rows and its footer note, stacked down the content area.
    private func paneView(_ sections: [Section]) -> NSView {
        let page = FlippedView()
        page.translatesAutoresizingMaskIntoConstraints = false
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 0
        stack.edgeInsets = NSEdgeInsets(top: 4, left: 20, bottom: 24, right: 20)
        stack.translatesAutoresizingMaskIntoConstraints = false
        page.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: page.topAnchor),
            stack.leadingAnchor.constraint(equalTo: page.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: page.trailingAnchor),
            stack.bottomAnchor.constraint(equalTo: page.bottomAnchor),
        ])

        func add(_ view: NSView, after spacing: CGFloat) {
            stack.addArrangedSubview(view)
            view.widthAnchor.constraint(equalToConstant: Self.groupWidth).isActive = true
            stack.setCustomSpacing(spacing, after: view)
        }

        for (index, section) in sections.enumerated() {
            let last = index == sections.count - 1
            if let header = section.header {
                let label = NSTextField(labelWithString: header)
                label.font = .systemFont(ofSize: 13, weight: .bold)
                headers.append(label)
                add(inset(label, by: 6), after: 8)
            }
            add(group(section.rows), after: section.footer == nil ? (last ? 0 : 24) : 6)
            if let footer = section.footer {
                let note = noteLabel(footer, wraps: true)
                note.preferredMaxLayoutWidth = Self.groupWidth - 12
                add(inset(note, by: 6), after: last ? 0 : 24)
            }
        }
        return page
    }

    /// A rounded card of rows with hairlines between them.
    private func group(_ rows: [Row]) -> NSView {
        let card = NSView()
        card.wantsLayer = true
        card.layer?.cornerRadius = 10
        card.layer?.cornerCurve = .continuous
        card.layer?.borderWidth = 0.5
        groups.append(card)

        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 0
        stack.translatesAutoresizingMaskIntoConstraints = false
        card.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: card.topAnchor),
            stack.bottomAnchor.constraint(equalTo: card.bottomAnchor),
            stack.leadingAnchor.constraint(equalTo: card.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: card.trailingAnchor),
        ])
        for (index, row) in rows.enumerated() {
            if index > 0 {
                let line = NSView()
                line.wantsLayer = true
                separators.append(line)
                let holder = NSView()
                line.translatesAutoresizingMaskIntoConstraints = false
                holder.addSubview(line)
                NSLayoutConstraint.activate([
                    holder.heightAnchor.constraint(equalToConstant: 1),
                    line.heightAnchor.constraint(equalToConstant: 1),
                    line.centerYAnchor.constraint(equalTo: holder.centerYAnchor),
                    line.leadingAnchor.constraint(equalTo: holder.leadingAnchor, constant: 12),
                    line.trailingAnchor.constraint(equalTo: holder.trailingAnchor, constant: -12),
                ])
                stack.addArrangedSubview(holder)
                holder.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
            }
            let view = rowView(row)
            stack.addArrangedSubview(view)
            view.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        }
        return card
    }

    private func rowView(_ row: Row) -> NSView {
        let view = NSView()
        switch row {
        case .full(let content):
            content.translatesAutoresizingMaskIntoConstraints = false
            view.addSubview(content)
            NSLayoutConstraint.activate([
                content.topAnchor.constraint(equalTo: view.topAnchor, constant: 12),
                content.bottomAnchor.constraint(equalTo: view.bottomAnchor, constant: -12),
                content.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 12),
                content.trailingAnchor.constraint(lessThanOrEqualTo: view.trailingAnchor, constant: -12),
            ])
        case .control(let title, let control, let subtitle):
            let label = NSTextField(labelWithString: title)
            label.font = .systemFont(ofSize: 13)
            label.lineBreakMode = .byTruncatingTail
            label.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
            rowTitles.append((label, control as? NSControl))
            let text = NSStackView(views: [label] + (subtitle.map { [$0] } ?? []))
            text.orientation = .vertical
            text.alignment = .leading
            text.spacing = 2
            subtitle?.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
            for item in [text, control] {
                item.translatesAutoresizingMaskIntoConstraints = false
                view.addSubview(item)
            }
            control.setContentCompressionResistancePriority(.required, for: .horizontal)
            NSLayoutConstraint.activate([
                view.heightAnchor.constraint(greaterThanOrEqualToConstant: 40),
                text.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 12),
                text.centerYAnchor.constraint(equalTo: view.centerYAnchor),
                text.topAnchor.constraint(greaterThanOrEqualTo: view.topAnchor, constant: 9),
                text.trailingAnchor.constraint(lessThanOrEqualTo: control.leadingAnchor, constant: -16),
                control.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -12),
                control.centerYAnchor.constraint(equalTo: view.centerYAnchor),
                control.topAnchor.constraint(greaterThanOrEqualTo: view.topAnchor, constant: 7),
            ])
        }
        return view
    }

    /// The top of General: the app's icon, name and version, and Check Now.
    private func aboutView() -> NSView {
        let icon = NSImageView(image: NSApp.applicationIconImage ?? NSImage())
        icon.imageScaling = .scaleProportionallyUpOrDown
        icon.widthAnchor.constraint(equalToConstant: 48).isActive = true
        icon.heightAnchor.constraint(equalToConstant: 48).isActive = true
        appName.font = .systemFont(ofSize: 15, weight: .semibold)
        let text = NSStackView(views: [appName, updateStatus])
        text.orientation = .vertical
        text.alignment = .leading
        text.spacing = 2
        let spacer = NSView()
        spacer.setContentHuggingPriority(.init(1), for: .horizontal)
        let stack = NSStackView(views: [icon, text, spacer, checkNowButton])
        stack.orientation = .horizontal
        stack.alignment = .centerY
        stack.spacing = 12
        stack.widthAnchor.constraint(equalToConstant: Self.groupWidth - 24).isActive = true
        return stack
    }

    private func noteLabel(_ text: String, wraps: Bool = false) -> NSTextField {
        let label = wraps ? NSTextField(wrappingLabelWithString: text) : NSTextField(labelWithString: text)
        label.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        notes.append(label)
        return label
    }

    /// `view` set in from the card's edge, as System Settings sets its headers.
    private func inset(_ view: NSView, by amount: CGFloat) -> NSView {
        let holder = NSView()
        view.translatesAutoresizingMaskIntoConstraints = false
        holder.addSubview(view)
        NSLayoutConstraint.activate([
            view.topAnchor.constraint(equalTo: holder.topAnchor),
            view.bottomAnchor.constraint(equalTo: holder.bottomAnchor),
            view.leadingAnchor.constraint(equalTo: holder.leadingAnchor, constant: amount),
            view.trailingAnchor.constraint(equalTo: holder.trailingAnchor, constant: -amount),
        ])
        return holder
    }

    private func row(_ views: [NSView], spacing: CGFloat = 8) -> NSStackView {
        let stack = NSStackView(views: views)
        stack.orientation = .horizontal
        stack.spacing = spacing
        stack.alignment = .centerY
        return stack
    }

    private func button(_ title: String, _ action: Selector) -> NSButton {
        let button = NSButton(title: title, target: self, action: action)
        button.bezelStyle = .push
        button.controlSize = .regular
        return button
    }

    // MARK: Controls

    private func configureControls() {
        for toggle in switches {
            toggle.controlSize = .small
            toggle.target = self
        }
        interfaceControl.target = self
        interfaceControl.action = #selector(interfaceChanged)
        paneControl.target = self
        paneControl.action = #selector(paneStyleChanged)
        tabPopup.addItems(withTitles: ["Above", "Left Side", "Right Side"])
        tabPopup.target = self
        tabPopup.action = #selector(tabStyleChanged)
        loaderPicker.onPick = { loader in
            Preferences.shared.tabLoader = loader
        }
        copyShineSwitch.action = #selector(motionChanged)
        reduceMotionSwitch.action = #selector(motionChanged)

        shellCheckbox.action = #selector(shellPluginsChanged)
        arrowsCheckbox.action = #selector(arrowKeysChanged)
        specCheckbox.action = #selector(specCompletionsChanged)
        sshColorCheckbox.action = #selector(sshColorChanged)
        agentPopup.addItems(withTitles: ShellAgent.all.map(\.title))
        agentPopup.target = self
        agentPopup.action = #selector(shellAgentChanged)

        for toggle in [notifyCheckbox, notifyBlocked, notifyDone, notifyFailed, notifySound] {
            toggle.action = #selector(notificationsChanged(_:))
        }
        for toggle in [updateCheckbox, updateDownload] {
            toggle.action = #selector(updatesChanged(_:))
        }
        checkNowButton.bezelStyle = .push
        checkNowButton.target = self
        checkNowButton.action = #selector(checkForUpdates)
        updateStatus.font = .systemFont(ofSize: NSFont.smallSystemFontSize)

        let formatter = NumberFormatter()
        formatter.minimum = 6
        formatter.maximum = 72
        formatter.maximumFractionDigits = 1
        sizeField.formatter = formatter
        sizeField.alignment = .right
        sizeField.widthAnchor.constraint(equalToConstant: 52).isActive = true
        sizeField.target = self
        sizeField.action = #selector(sizeFieldChanged)
        sizeStepper.minValue = 6
        sizeStepper.maxValue = 72
        sizeStepper.increment = 1
        sizeStepper.valueWraps = false
        sizeStepper.target = self
        sizeStepper.action = #selector(sizeStepperChanged)

        fontPopup.target = self
        fontPopup.action = #selector(fontChanged)
        fontPopup.widthAnchor.constraint(lessThanOrEqualToConstant: 240).isActive = true
        themePicker.onPick = { theme in AppDelegate.shared.applySetting("theme", theme.id) }

        for label in [configLabel, hostFolderLabel, hostStatus] {
            label.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
            label.lineBreakMode = .byTruncatingMiddle
        }
        configLabel.isSelectable = true
        hostFolderLabel.isSelectable = true

        hostCheckbox.action = #selector(hostToggled)
        startPopup.target = self
        startPopup.action = #selector(startFolderChanged)
        startPopup.widthAnchor.constraint(lessThanOrEqualToConstant: 260).isActive = true
        inheritSwitch.action = #selector(inheritChanged)
        warnAlwaysSwitch.action = #selector(quitWarningChanged(_:))
        warnRunningSwitch.action = #selector(quitWarningChanged(_:))
        for button in [revealButton, restartButton] {
            button.bezelStyle = .push
            button.target = self
        }
        revealButton.action = #selector(revealHostFolder)
        restartButton.action = #selector(restartHost)
    }

    private func loadLocalFonts() -> (monospaced: [String], other: [String]) {
        if let localFonts { return localFonts }
        let manager = NSFontManager.shared
        let fixedPitch = Set(manager.availableFontNames(with: .fixedPitchFontMask) ?? [])
        // Many coding fonts don't set the fixed-pitch flag; their names say it.
        let codingName = try! NSRegularExpression(pattern: "mono|code|term|consol|courier|nerd font", options: .caseInsensitive)
        var monospaced: [String] = [], other: [String] = []
        for family in manager.availableFontFamilies where !family.hasPrefix(".") {
            let members = (manager.availableMembers(ofFontFamily: family) ?? []).compactMap { $0.first as? String }
            let range = NSRange(family.startIndex..., in: family)
            let proportional = family.localizedCaseInsensitiveContains("propo")
            if !proportional, members.contains(where: fixedPitch.contains) || codingName.firstMatch(in: family, range: range) != nil {
                monospaced.append(family)
            } else {
                other.append(family)
            }
        }
        let fonts = (monospaced, other)
        localFonts = fonts
        return fonts
    }

    @objc private func fontsChanged() {
        localFonts = nil
        refresh()
    }

    /// Puts every control in step with the current preferences and config.
    @objc private func refresh() {
        let prefs = Preferences.shared
        let app = AppConfig.shared
        interfaceControl.selectedSegment = prefs.interfaceStyle == .liquid ? 0 : 1
        tabPopup.selectItem(at: [TabStyle.above, .left, .right].firstIndex(of: prefs.tabStyle) ?? 0)
        paneControl.selectedSegment = prefs.paneStyle == .comfortable ? 0 : 1
        loaderPicker.selected = prefs.tabLoader
        copyShineSwitch.state = prefs.copyShine ? .on : .off
        reduceMotionSwitch.state = prefs.reduceMotion ? .on : .off
        notifyCheckbox.state = prefs.notificationsEnabled ? .on : .off
        notifyBlocked.state = prefs.notifyWhenBlocked ? .on : .off
        notifyDone.state = prefs.notifyWhenDone ? .on : .off
        notifyFailed.state = prefs.notifyWhenFailed ? .on : .off
        notifySound.state = prefs.notificationSound ? .on : .off
        for option in [notifyBlocked, notifyDone, notifyFailed, notifySound] { option.isEnabled = prefs.notificationsEnabled }
        shellCheckbox.state = prefs.shellPlugins ? .on : .off
        agentPopup.selectItem(at: ShellAgent.all.firstIndex { $0.id == prefs.shellAgent } ?? 0)
        agentPopup.isEnabled = prefs.shellPlugins
        specCheckbox.state = prefs.commandCompletions ? .on : .off
        specCheckbox.isEnabled = prefs.shellPlugins
        arrowsCheckbox.state = prefs.completionArrowKeys ? .on : .off
        arrowsCheckbox.isEnabled = prefs.shellPlugins
        sshColorCheckbox.state = prefs.sshSendsColor ? .on : .off
        refreshStartMenu()
        inheritSwitch.state = prefs.inheritsDirectory ? .on : .off
        warnAlwaysSwitch.state = app.warnsOnQuit ? .on : .off
        warnRunningSwitch.state = app.warnsOnQuitWithRunningProcess ? .on : .off
        // Always asking covers running programs too.
        warnRunningSwitch.isEnabled = !app.warnsOnQuit
        refreshUpdates()

        let render = app.renderConfig
        refreshFontMenu(current: render.fontFamily)
        sizeField.doubleValue = Double(app.baseFontSize)
        sizeStepper.doubleValue = Double(app.baseFontSize)
        if pane == .theme {
            themePicker.update(themes: app.themes, current: app.themeSetting ?? render.activeTheme)
        }
        configLabel.stringValue = (AppConfig.configURL.path as NSString).abbreviatingWithTildeInPath
        configLabel.toolTip = AppConfig.configURL.path

        hostCheckbox.state = prefs.usesSessionHost ? .on : .off
        hostFolderLabel.stringValue = (SessionHost.root.path as NSString).abbreviatingWithTildeInPath
        hostFolderLabel.toolTip = SessionHost.root.path
        restartButton.isEnabled = prefs.usesSessionHost && SessionHost.shared.multiplexer != nil && !SessionHost.shared.isRestarting
        if SessionHost.shared.isRestarting {
            hostStatus.stringValue = "Restarting…"
        } else if let multiplexer = SessionHost.shared.multiplexer {
            let running = ((try? multiplexer.panes()) ?? []).filter { !$0.exited }.count
            hostStatus.stringValue = "Running, \(running) \(running == 1 ? "pane" : "panes")"
        } else {
            hostStatus.stringValue = prefs.usesSessionHost ? "Not connected" : "Off"
        }
        applyTheme()
    }

    /// The font menu: monospaced families, then every other local family,
    /// each drawn in its own face.
    private func refreshFontMenu(current: String) {
        let fonts = loadLocalFonts()
        let missing = !current.isEmpty && !fonts.monospaced.contains(current) && !fonts.other.contains(current)
        let key = (missing ? [current] : []) + fonts.monospaced + ["|"] + fonts.other
        if key != fontMenuKey {
            fontMenuKey = key
            let menu = NSMenu()
            func add(_ family: String) {
                let item = NSMenuItem(title: family, action: nil, keyEquivalent: "")
                let face = NSFont(name: family, size: NSFont.systemFontSize)
                    ?? NSFontManager.shared.font(withFamily: family, traits: [], weight: 5, size: NSFont.systemFontSize)
                if let face { item.attributedTitle = NSAttributedString(string: family, attributes: [.font: face]) }
                menu.addItem(item)
            }
            if missing {
                add(current)
                menu.addItem(.separator())
            }
            menu.addItem(.sectionHeader(title: "Monospaced"))
            fonts.monospaced.forEach(add)
            if !fonts.other.isEmpty {
                menu.addItem(.separator())
                menu.addItem(.sectionHeader(title: "Other Fonts"))
                fonts.other.forEach(add)
            }
            fontPopup.menu = menu
        }
        fontPopup.selectItem(withTitle: current)
    }

    /// Home, the configured folder when there is one, and Other… to pick one.
    private func refreshStartMenu() {
        let menu = NSMenu()
        func item(_ title: String, tag: Int, symbol: String?) -> NSMenuItem {
            let item = NSMenuItem(title: title, action: nil, keyEquivalent: "")
            item.tag = tag
            if let symbol { item.image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil) }
            menu.addItem(item)
            return item
        }
        let home = item("Home Folder", tag: 0, symbol: "house")
        var selected = home
        if let custom = AppConfig.shared.workingDirSetting, !custom.isEmpty {
            let path = (custom as NSString).expandingTildeInPath
            selected = item(Naming.abbreviatingHome(path), tag: 1, symbol: "folder")
            selected.toolTip = path
        }
        menu.addItem(.separator())
        _ = item("Other…", tag: 2, symbol: nil)
        startPopup.menu = menu
        startPopup.select(selected)
    }

    @objc private func startFolderChanged() {
        switch startPopup.selectedTag() {
        case 0: AppDelegate.shared.applySetting("working_dir", nil)
        case 2: chooseStartFolder()
        default: break
        }
    }

    private func chooseStartFolder() {
        guard let window else { return }
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.canCreateDirectories = true
        panel.prompt = "Choose"
        panel.message = "New windows start in this folder."
        panel.directoryURL = URL(fileURLWithPath: AppConfig.shared.startDirectory)
        panel.beginSheetModal(for: window) { [weak self] response in
            if response == .OK, let url = panel.url {
                AppDelegate.shared.applySetting("working_dir", Naming.abbreviatingHome(url.path))
            }
            self?.refresh()
        }
    }

    @objc private func inheritChanged() { Preferences.shared.inheritsDirectory = inheritSwitch.state == .on }

    @objc private func quitWarningChanged(_ sender: NSSwitch) {
        let key = sender === warnAlwaysSwitch ? "warn_on_quit" : "warn_on_quit_with_running_process"
        AppDelegate.shared.applySetting(key, sender.state == .on ? "true" : "false")
    }

    @objc private func interfaceChanged() {
        Preferences.shared.interfaceStyle = interfaceControl.selectedSegment == 0 ? .liquid : .classic
    }

    @objc private func openLoadingDev() { NSWorkspace.shared.open(URL(string: "https://loading.dev")!) }

    @objc private func motionChanged() {
        Preferences.shared.copyShine = copyShineSwitch.state == .on
        Preferences.shared.reduceMotion = reduceMotionSwitch.state == .on
    }

    @objc private func tabStyleChanged() {
        Preferences.shared.tabStyle = [TabStyle.above, .left, .right][max(0, tabPopup.indexOfSelectedItem)]
    }

    @objc private func notificationsChanged(_ sender: NSSwitch) {
        let prefs = Preferences.shared
        let on = sender.state == .on
        switch sender {
        case notifyCheckbox: prefs.notificationsEnabled = on
        case notifyBlocked: prefs.notifyWhenBlocked = on
        case notifyDone: prefs.notifyWhenDone = on
        case notifyFailed: prefs.notifyWhenFailed = on
        default: prefs.notificationSound = on
        }
        PaneNotifier.shared.updateBadge()
        refresh()
    }

    private func refreshUpdates() {
        let updates = Updates.shared
        updateCheckbox.state = updates.checksAutomatically ? .on : .off
        updateDownload.state = updates.downloadsAutomatically ? .on : .off
        updateCheckbox.isEnabled = updates.isAvailable
        updateDownload.isEnabled = updates.isAvailable && updates.checksAutomatically
        checkNowButton.isEnabled = updates.canCheck
        var status = "Version \(Updates.currentVersion)"
        if let last = updates.lastCheck {
            let relative = RelativeDateTimeFormatter()
            relative.unitsStyle = .full
            status += " · checked \(relative.localizedString(for: last, relativeTo: Date()))"
        }
        updateStatus.stringValue = status
    }

    @objc private func updatesChanged(_ sender: NSSwitch) {
        let on = sender.state == .on
        if sender === updateCheckbox {
            Updates.shared.checksAutomatically = on
        } else {
            Updates.shared.downloadsAutomatically = on
        }
        refresh()
    }

    @objc private func checkForUpdates() { Updates.shared.checkForUpdates() }

    @objc private func paneStyleChanged() {
        Preferences.shared.paneStyle = paneControl.selectedSegment == 0 ? .comfortable : .compact
    }

    @objc private func sizeFieldChanged() { AppDelegate.shared.setFontSize(sizeField.floatValue) }
    @objc private func sizeStepperChanged() { AppDelegate.shared.setFontSize(Float(sizeStepper.doubleValue)) }
    @objc private func fontChanged() {
        guard let family = fontPopup.titleOfSelectedItem else { return }
        AppDelegate.shared.applySetting("font_family", family)
    }
    @objc private func openConfig() { AppDelegate.shared.openConfig() }
    @objc private func importConfig() { AppDelegate.shared.importTermyConfig(attachedTo: window) }
    @objc private func reloadConfig() { AppDelegate.shared.reloadConfig() }

    @objc private func shellPluginsChanged() { Preferences.shared.shellPlugins = shellCheckbox.state == .on }
    @objc private func arrowKeysChanged() { Preferences.shared.completionArrowKeys = arrowsCheckbox.state == .on }
    @objc private func specCompletionsChanged() { Preferences.shared.commandCompletions = specCheckbox.state == .on }
    @objc private func sshColorChanged() { Preferences.shared.sshSendsColor = sshColorCheckbox.state == .on }

    @objc private func shellAgentChanged() {
        Preferences.shared.shellAgent = ShellAgent.all[max(0, agentPopup.indexOfSelectedItem)].id
    }

    @objc private func hostToggled() {
        let on = hostCheckbox.state == .on
        hostCheckbox.isEnabled = false
        if on { hostStatus.stringValue = "Starting…" }
        AppDelegate.shared.setUsesSessionHost(on) { [weak self] error in
            guard let self else { return }
            self.hostCheckbox.isEnabled = true
            self.refresh()
            if let error, let window = self.window {
                let alert = NSAlert()
                alert.messageText = "Termy Next can't start its session host"
                alert.informativeText = error.localizedDescription
                alert.beginSheetModal(for: window)
            }
        }
    }

    @objc private func restartHost() { AppDelegate.shared.restartSessionHost() }

    @objc private func revealHostFolder() {
        NSWorkspace.shared.activateFileViewerSelecting([SessionHost.root])
    }
}

/// A view whose origin is its top-left, so pages scroll from the top.
private final class FlippedView: NSView {
    override var isFlipped: Bool { true }
}

/// The Settings sidebar: a row per pane with a colored icon tile and its
/// name, in groups, the selected row filled with the theme's accent.
@MainActor
final class SettingsSidebar: NSView {
    struct Item {
        let id: String
        let title: String
        let symbol: String
        let color: NSColor
        let startsGroup: Bool
    }

    var items: [Item] = [] { didSet { needsDisplay = true } }
    var selected: String? { didSet { needsDisplay = true } }
    var onSelect: ((String) -> Void)?

    private let rowHeight: CGFloat = 30
    private let groupGap: CGFloat = 14

    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }
    override var mouseDownCanMoveWindow: Bool { false }

    private var rows: [(item: Item, rect: NSRect)] {
        var y: CGFloat = 4
        return items.enumerated().map { index, item in
            if index > 0, item.startsGroup { y += groupGap }
            defer { y += rowHeight }
            return (item, NSRect(x: 10, y: y, width: bounds.width - 20, height: rowHeight))
        }
    }

    override func draw(_ dirtyRect: NSRect) {
        let theme = AppConfig.shared.theme
        if items.isEmpty {
            let text = NSAttributedString(string: "No Results", attributes: [
                .font: NSFont.systemFont(ofSize: 13), .foregroundColor: theme.textTertiary,
            ])
            text.draw(at: NSPoint(x: (bounds.width - text.size().width) / 2, y: 16))
            return
        }
        for (item, rect) in rows {
            let isSelected = item.id == selected
            if isSelected {
                theme.selectionFill.setFill()
                NSBezierPath(roundedRect: rect, xRadius: 7, yRadius: 7).fill()
            }
            // The icon: a white symbol on a colored rounded square.
            let tile = NSRect(x: rect.minX + 6, y: rect.midY - 11, width: 22, height: 22)
            item.color.setFill()
            NSBezierPath(roundedRect: tile, xRadius: 6, yRadius: 6).fill()
            let config = NSImage.SymbolConfiguration(pointSize: 11.5, weight: .semibold)
                .applying(NSImage.SymbolConfiguration(paletteColors: [.white]))
            if let symbol = NSImage(systemSymbolName: item.symbol, accessibilityDescription: nil)?.withSymbolConfiguration(config) {
                let size = symbol.size
                symbol.draw(in: NSRect(x: tile.midX - size.width / 2, y: tile.midY - size.height / 2,
                                       width: size.width, height: size.height))
            }
            let title = NSAttributedString(string: item.title, attributes: [
                .font: NSFont.systemFont(ofSize: 13),
                .foregroundColor: isSelected ? theme.selectionText : theme.textPrimary,
            ])
            let size = title.size()
            title.draw(at: NSPoint(x: tile.maxX + 9, y: rect.midY - size.height / 2))
        }
    }

    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        guard let hit = rows.first(where: { $0.rect.contains(point) }) else { return }
        window?.makeFirstResponder(self)
        choose(hit.item.id)
    }

    override func keyDown(with event: NSEvent) {
        let ids = items.map(\.id)
        guard !ids.isEmpty else { return super.keyDown(with: event) }
        let index = selected.flatMap { ids.firstIndex(of: $0) }
        switch Int(event.keyCode) {
        case 125: // down
            choose(ids[min((index ?? -1) + 1, ids.count - 1)])
        case 126: // up
            choose(ids[max((index ?? ids.count) - 1, 0)])
        default:
            super.keyDown(with: event)
        }
    }

    private func choose(_ id: String) {
        guard id != selected else { return }
        selected = id
        onSelect?(id)
    }

    override func isAccessibilityElement() -> Bool { true }
    override func accessibilityRole() -> NSAccessibility.Role? { .list }
    override func accessibilityLabel() -> String? { "Settings" }
}
