import AppKit
import Termy

/// The Settings window (⌘,): a standard macOS preferences window with a
/// toolbar of panes. Every control writes straight to `Preferences`, and the
/// window follows changes made elsewhere, such as from the View menu.
@MainActor
final class SettingsWindowController: NSWindowController, NSToolbarDelegate {
    static let shared = SettingsWindowController()

    private enum Pane: String, CaseIterable {
        case general, appearance, sessions

        var title: String {
            switch self {
            case .general: "General"
            case .appearance: "Appearance"
            case .sessions: "Sessions"
            }
        }

        var symbol: String {
            switch self {
            case .general: "gearshape"
            case .appearance: "textformat.size"
            case .sessions: "rectangle.stack"
            }
        }

        var identifier: NSToolbarItem.Identifier { NSToolbarItem.Identifier(rawValue) }
    }

    private static let width: CGFloat = 540
    private var pane: Pane = .general
    private var paneViews: [Pane: NSView] = [:]

    // General
    private let interfaceControl = NSSegmentedControl(labels: ["Liquid", "Classic"], trackingMode: .selectOne, target: nil, action: nil)
    private let tabPopup = NSPopUpButton(frame: .zero, pullsDown: false)
    private let paneControl = NSSegmentedControl(labels: ["Comfortable", "Compact"], trackingMode: .selectOne, target: nil, action: nil)
    private let notifyCheckbox = NSButton(checkboxWithTitle: "Notify me about programs in panes", target: nil, action: nil)
    private let notifyBlocked = NSButton(checkboxWithTitle: "When one needs attention", target: nil, action: nil)
    private let notifyDone = NSButton(checkboxWithTitle: "When one finishes", target: nil, action: nil)
    private let notifyFailed = NSButton(checkboxWithTitle: "When one fails", target: nil, action: nil)
    private let notifySound = NSButton(checkboxWithTitle: "Play a sound", target: nil, action: nil)
    private let updateCheckbox = NSButton(checkboxWithTitle: "Check for updates automatically", target: nil, action: nil)
    private let updateDownload = NSButton(checkboxWithTitle: "Download and install them automatically", target: nil, action: nil)
    private let checkNowButton = NSButton(title: "Check Now", target: nil, action: nil)
    private let updateStatus = NSTextField(labelWithString: "")

    // Appearance
    private let fontPopup = NSPopUpButton(frame: .zero, pullsDown: false)
    private let sizeField = NSTextField()
    private let sizeStepper = NSStepper()
    private let themePicker = ThemePicker(width: 350, height: 250)
    private let configLabel = NSTextField(labelWithString: "")
    /// Every font family installed on this Mac, monospaced ones first;
    /// rebuilt when fonts are installed or removed.
    private var localFonts: (monospaced: [String], other: [String])?
    private var fontMenuKey: [String] = []

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

    // Sessions
    private let hostCheckbox = NSButton(checkboxWithTitle: "Keep programs running after Termy Next quits", target: nil, action: nil)
    private let hostStatus = NSTextField(labelWithString: "")
    private let hostFolderLabel = NSTextField(labelWithString: "")
    private let revealButton = NSButton(title: "Show in Finder", target: nil, action: nil)
    private let restartButton = NSButton(title: "Restart Session Host…", target: nil, action: nil)

    private init() {
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: Self.width, height: 300),
            styleMask: [.titled, .closable], backing: .buffered, defer: true)
        window.isReleasedWhenClosed = false
        window.toolbarStyle = .preference
        super.init(window: window)

        let toolbar = NSToolbar(identifier: "TermyNextSettings")
        toolbar.delegate = self
        toolbar.displayMode = .iconAndLabel
        toolbar.allowsUserCustomization = false
        window.toolbar = toolbar

        configureControls()
        NotificationCenter.default.addObserver(self, selector: #selector(refresh), name: Preferences.didChange, object: nil)
        NotificationCenter.default.addObserver(self, selector: #selector(refresh), name: Workspace.didChange, object: nil)
        NotificationCenter.default.addObserver(self, selector: #selector(refresh), name: Updates.didChange, object: nil)
        NotificationCenter.default.addObserver(self, selector: #selector(fontsChanged), name: NSFont.fontSetChangedNotification, object: nil)
        select(.general, animated: false)
        window.center()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    func show() {
        refresh()
        showWindow(nil)
        window?.makeKeyAndOrderFront(nil)
    }

    // MARK: Toolbar

    func toolbarAllowedItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] { Pane.allCases.map(\.identifier) }
    func toolbarDefaultItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] { Pane.allCases.map(\.identifier) }
    func toolbarSelectableItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] { Pane.allCases.map(\.identifier) }

    func toolbar(_ toolbar: NSToolbar, itemForItemIdentifier identifier: NSToolbarItem.Identifier,
                 willBeInsertedIntoToolbar flag: Bool) -> NSToolbarItem? {
        guard let pane = Pane(rawValue: identifier.rawValue) else { return nil }
        let item = NSToolbarItem(itemIdentifier: identifier)
        item.label = pane.title
        item.image = NSImage(systemSymbolName: pane.symbol, accessibilityDescription: pane.title)
        item.target = self
        item.action = #selector(toolbarItemClicked(_:))
        return item
    }

    @objc private func toolbarItemClicked(_ sender: NSToolbarItem) {
        if let pane = Pane(rawValue: sender.itemIdentifier.rawValue) { select(pane, animated: true) }
    }

    /// Shows `pane`, resizing the window around it from the top edge like
    /// every macOS settings window.
    private func select(_ pane: Pane, animated: Bool) {
        guard let window else { return }
        self.pane = pane
        window.toolbar?.selectedItemIdentifier = pane.identifier
        window.title = pane.title
        let view = paneViews[pane] ?? build(pane)
        paneViews[pane] = view
        refresh()

        view.layoutSubtreeIfNeeded()
        let content = NSSize(width: Self.width, height: view.fittingSize.height)
        var frame = window.frameRect(forContentRect: NSRect(origin: .zero, size: content))
        frame.origin = NSPoint(x: window.frame.minX, y: window.frame.maxY - frame.height)
        let reduceMotion = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        if animated, window.isVisible, !reduceMotion {
            window.contentView = NSView()
            window.setFrame(frame, display: true, animate: true)
        } else {
            window.setFrame(frame, display: true)
        }
        window.contentView = view
    }

    // MARK: Panes

    private func build(_ pane: Pane) -> NSView {
        switch pane {
        case .general:
            return form([
                .control("Interface style:", interfaceControl),
                .note("Liquid puts the window chrome and panes on Liquid Glass. Classic draws them solid."),
                .control("Tabs:", tabPopup),
                .control("Pane spacing:", paneControl),
                .note("Compact drops the gaps and headers around panes."),
                .separator,
                .control("Notifications:", notifyCheckbox),
                .control("", notifyBlocked),
                .control("", notifyDone),
                .control("", notifyFailed),
                .control("", notifySound),
                .note("Programs report their status with OSC 7501, as Claude Code and other agents do. You're notified only about panes you aren't looking at; clicking a notification takes you to the pane."),
                .separator,
                .control("Updates:", updateCheckbox),
                .control("", updateDownload),
                .control("", row([checkNowButton, updateStatus], spacing: 10)),
                .note(Updates.shared.isAvailable
                    ? "Updates are installed when you quit or relaunch. Programs in panes keep running through it."
                    : "This build doesn't update itself. Releases are on GitHub, tagged next-v*."),
            ])
        case .appearance:
            let size = row([sizeField, sizeStepper, NSTextField(labelWithString: "pt")], spacing: 6)
            let configButtons = row([
                button("Open in Editor", #selector(openConfig)),
                button("Import Termy Config…", #selector(importConfig)),
                button("Reload", #selector(reloadConfig)),
            ], spacing: 8)
            return form([
                .control("Font:", fontPopup),
                .control("Size:", size),
                .note("⌘+ and ⌘− zoom only the pane you're in; ⌘0 puts it back to this size."),
                .control("Theme:", themePicker),
                .note("Termy's theme library. Add your own as JSON files in ~/.config/termy-next/themes."),
                .separator,
                .control("Config file:", configLabel),
                .control("", configButtons),
                .note("Termy Next keeps its own config, separate from the Termy app's. Import copies a Termy config over it, along with its local themes; the old file is kept as config.txt.bak."),
            ])
        case .sessions:
            return form([
                .control("Session host:", hostCheckbox),
                .note("Shells run in a Termy session host, so they keep running when you quit and reattach when you open Termy Next again. Turned off, new panes run inside the app and end when it quits. Panes that are already open keep running where they are."),
                .control("Status:", hostStatus),
                .control("", restartButton),
                .control("Host folder:", hostFolderLabel),
                .control("", revealButton),
            ])
        }
    }

    private enum Row {
        case control(String, NSView)
        case note(String)
        case separator
    }

    /// A two-column settings form: right-aligned labels, controls on the left
    /// edge of the second column, as in macOS settings windows.
    private func form(_ rows: [Row]) -> NSView {
        let grid = NSGridView()
        grid.columnSpacing = 8
        grid.rowSpacing = 10
        grid.rowAlignment = .firstBaseline
        for row in rows {
            switch row {
            case .control(let title, let view):
                let label = NSTextField(labelWithString: title)
                label.alignment = .right
                grid.addRow(with: [title.isEmpty ? NSGridCell.emptyContentView : label, view])
            case .note(let text):
                let note = NSTextField(wrappingLabelWithString: text)
                note.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
                note.textColor = .secondaryLabelColor
                note.preferredMaxLayoutWidth = 340
                grid.addRow(with: [NSGridCell.emptyContentView, note])
                grid.row(at: grid.numberOfRows - 1).topPadding = -4
            case .separator:
                let line = NSBox()
                line.boxType = .separator
                grid.addRow(with: [line])
                let gridRow = grid.row(at: grid.numberOfRows - 1)
                gridRow.mergeCells(in: NSRange(location: 0, length: 2))
                gridRow.topPadding = 4
                gridRow.bottomPadding = 4
                gridRow.rowAlignment = .none
                gridRow.yPlacement = .center
            }
        }
        grid.column(at: 0).xPlacement = .trailing
        grid.column(at: 0).width = 130
        grid.column(at: 1).xPlacement = .leading

        let container = NSView()
        grid.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(grid)
        NSLayoutConstraint.activate([
            container.widthAnchor.constraint(equalToConstant: Self.width),
            grid.topAnchor.constraint(equalTo: container.topAnchor, constant: 24),
            grid.bottomAnchor.constraint(equalTo: container.bottomAnchor, constant: -24),
            grid.leadingAnchor.constraint(equalTo: container.leadingAnchor, constant: 24),
            grid.trailingAnchor.constraint(lessThanOrEqualTo: container.trailingAnchor, constant: -24),
        ])
        return container
    }

    private func row(_ views: [NSView], spacing: CGFloat) -> NSStackView {
        let stack = NSStackView(views: views)
        stack.orientation = .horizontal
        stack.spacing = spacing
        stack.alignment = .firstBaseline
        return stack
    }

    private func button(_ title: String, _ action: Selector) -> NSButton {
        let button = NSButton(title: title, target: self, action: action)
        button.bezelStyle = .push
        return button
    }

    // MARK: Controls

    private func configureControls() {
        interfaceControl.target = self
        interfaceControl.action = #selector(interfaceChanged)

        tabPopup.addItems(withTitles: ["Above", "Left Side", "Right Side"])
        tabPopup.target = self
        tabPopup.action = #selector(tabStyleChanged)

        for checkbox in [notifyCheckbox, notifyBlocked, notifyDone, notifyFailed, notifySound] {
            checkbox.target = self
            checkbox.action = #selector(notificationsChanged(_:))
        }
        paneControl.target = self
        paneControl.action = #selector(paneStyleChanged)

        for checkbox in [updateCheckbox, updateDownload] {
            checkbox.target = self
            checkbox.action = #selector(updatesChanged(_:))
        }
        checkNowButton.bezelStyle = .push
        checkNowButton.target = self
        checkNowButton.action = #selector(checkForUpdates)
        updateStatus.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        updateStatus.textColor = .secondaryLabelColor

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
        themePicker.onPick = { theme in AppDelegate.shared.applySetting("theme", theme.id) }
        configLabel.isSelectable = true
        configLabel.lineBreakMode = .byTruncatingMiddle
        configLabel.widthAnchor.constraint(lessThanOrEqualToConstant: 340).isActive = true
        configLabel.textColor = .secondaryLabelColor

        hostCheckbox.target = self
        hostCheckbox.action = #selector(hostToggled)
        hostStatus.textColor = .secondaryLabelColor
        hostFolderLabel.isSelectable = true
        hostFolderLabel.lineBreakMode = .byTruncatingMiddle
        hostFolderLabel.widthAnchor.constraint(lessThanOrEqualToConstant: 340).isActive = true
        hostFolderLabel.textColor = .secondaryLabelColor
        revealButton.bezelStyle = .push
        revealButton.target = self
        revealButton.action = #selector(revealHostFolder)
        restartButton.bezelStyle = .push
        restartButton.target = self
        restartButton.action = #selector(restartHost)
    }

    /// Puts every control in step with the current preferences and config.
    @objc private func refresh() {
        let prefs = Preferences.shared
        let app = AppConfig.shared
        interfaceControl.selectedSegment = prefs.interfaceStyle == .liquid ? 0 : 1
        tabPopup.selectItem(at: [TabStyle.above, .left, .right].firstIndex(of: prefs.tabStyle) ?? 0)
        paneControl.selectedSegment = prefs.paneStyle == .comfortable ? 0 : 1
        notifyCheckbox.state = prefs.notificationsEnabled ? .on : .off
        notifyBlocked.state = prefs.notifyWhenBlocked ? .on : .off
        notifyDone.state = prefs.notifyWhenDone ? .on : .off
        notifyFailed.state = prefs.notifyWhenFailed ? .on : .off
        notifySound.state = prefs.notificationSound ? .on : .off
        for option in [notifyBlocked, notifyDone, notifyFailed, notifySound] { option.isEnabled = prefs.notificationsEnabled }
        refreshUpdates()

        let render = app.renderConfig
        refreshFontMenu(current: render.fontFamily)
        sizeField.doubleValue = Double(app.baseFontSize)
        sizeStepper.doubleValue = Double(app.baseFontSize)
        if pane == .appearance || paneViews[.appearance] != nil {
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

    private static func points(_ size: Float) -> String {
        size.rounded() == size ? "\(Int(size)) pt" : String(format: "%.1f pt", size)
    }

    @objc private func interfaceChanged() {
        Preferences.shared.interfaceStyle = interfaceControl.selectedSegment == 0 ? .liquid : .classic
    }

    @objc private func tabStyleChanged() {
        Preferences.shared.tabStyle = [TabStyle.above, .left, .right][max(0, tabPopup.indexOfSelectedItem)]
    }

    @objc private func notificationsChanged(_ sender: NSButton) {
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

    @objc private func updatesChanged(_ sender: NSButton) {
        let on = sender.state == .on
        if sender === updateCheckbox {
            Updates.shared.checksAutomatically = on
        } else {
            Updates.shared.downloadsAutomatically = on
        }
        refreshUpdates()
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
