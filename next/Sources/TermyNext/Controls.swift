import AppKit

/// A borderless SF Symbol button that brightens on hover, like the icons in
/// Rex's titlebar and pane headers.
@MainActor
final class IconButton: NSView {
    var action: (() -> Void)?
    var symbol: String { didSet { needsDisplay = true } }
    var pointSize: CGFloat
    var tint: NSColor = .secondaryLabelColor { didSet { needsDisplay = true } }
    var hoverTint: NSColor = .labelColor
    var hoverFill: NSColor = .clear
    var isActive = false { didSet { needsDisplay = true } }
    private var hovering = false { didSet { needsDisplay = true } }
    private var pressed = false { didSet { needsDisplay = true } }

    init(symbol: String, pointSize: CGFloat = 13, help: String? = nil, action: (() -> Void)? = nil) {
        self.symbol = symbol
        self.pointSize = pointSize
        self.action = action
        super.init(frame: NSRect(x: 0, y: 0, width: 24, height: 24))
        toolTip = help
        setAccessibilityRole(.button)
        setAccessibilityLabel(help)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

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
        if bounds.contains(convert(event.locationInWindow, from: nil)) { action?() }
    }
    override func accessibilityPerformPress() -> Bool { action?(); return true }
    override func isAccessibilityElement() -> Bool { true }
    override var mouseDownCanMoveWindow: Bool { false }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func draw(_ dirtyRect: NSRect) {
        if hovering || isActive {
            hoverFill.setFill()
            NSBezierPath(roundedRect: bounds.insetBy(dx: 1, dy: 1), xRadius: 6, yRadius: 6).fill()
        }
        let base = hovering || isActive ? hoverTint : tint
        let color = pressed ? base.withAlphaComponent(0.6) : base
        if symbol == Self.sidebarGlyph {
            drawSidebarGlyph(color)
            return
        }
        let config = NSImage.SymbolConfiguration(pointSize: pointSize, weight: .regular)
        guard let image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)?.withSymbolConfiguration(config) else { return }
        let tinted = image.tinted(color)
        let size = tinted.size
        // Whole-pixel origin: thin symbol strokes blur on half points.
        let origin = backingAligned(NSPoint(x: (bounds.width - size.width) / 2, y: (bounds.height - size.height) / 2))
        tinted.drawUpright(in: NSRect(origin: origin, size: size))
    }

    /// A sidebar icon drawn on the pixel grid: a window outline with a
    /// divider, like Rex's. The SF Symbol's inner lines smear at this size.
    static let sidebarGlyph = "termy.sidebar"

    private func drawSidebarGlyph(_ color: NSColor) {
        let line: CGFloat = 1.5
        let size = NSSize(width: 17, height: 14)
        let origin = backingAligned(NSPoint(x: (bounds.width - size.width) / 2, y: (bounds.height - size.height) / 2))
        let outline = NSRect(origin: origin, size: size).insetBy(dx: line / 2, dy: line / 2)
        color.setStroke()
        let path = NSBezierPath(roundedRect: outline, xRadius: 3.5, yRadius: 3.5)
        path.lineWidth = line
        path.stroke()
        color.setFill()
        NSRect(x: origin.x + 5.5, y: origin.y, width: line, height: size.height).fill()
    }

    private func backingAligned(_ point: NSPoint) -> NSPoint {
        let backing = convertToBacking(point)
        return convertFromBacking(NSPoint(x: backing.x.rounded(), y: backing.y.rounded()))
    }
}

extension NSImage {
    /// Draws centered on `point`-sized rect, upright in flipped views too.
    func drawUpright(in rect: NSRect) {
        draw(in: rect, from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
    }

    func drawUpright(at point: NSPoint) {
        drawUpright(in: NSRect(origin: point, size: size))
    }

    func tinted(_ color: NSColor) -> NSImage {
        let image = NSImage(size: size, flipped: false) { rect in
            self.draw(in: rect)
            color.set()
            rect.fill(using: .sourceAtop)
            return true
        }
        image.isTemplate = false
        return image
    }
}

/// The small icon tile shown before a pane or tab title, chosen from the
/// program in front.
@MainActor
enum ProgramIcon {
    static func symbol(for program: String?) -> String {
        let name = (program ?? "").lowercased()
        switch name {
        case "zsh", "bash", "fish", "sh", "nu", "elvish", "xonsh", "pwsh", "shell", "login": return "chevron.right"
        case "node", "bun", "deno", "npm", "pnpm", "yarn": return "hexagon"
        case "nvim", "vim", "vi", "hx", "helix", "nano", "emacs", "micro": return "chevron.left.forwardslash.chevron.right"
        case "git", "lazygit", "tig", "gitui": return "plus.forwardslash.minus"
        case "claude": return "sparkle"
        case "codex", "opencode", "pi", "aider", "gemini": return "brain"
        case "python", "python3", "ipython", "uv": return "p.circle"
        case "ssh", "mosh", "et": return "network"
        case "docker", "podman", "kubectl", "k9s": return "shippingbox"
        case "top", "htop", "btop", "btm": return "gauge.with.dots.needle.33percent"
        case "cargo", "rustc", "swift", "go", "make", "just", "xcodebuild": return "hammer"
        case "less", "more", "man", "bat": return "doc.text"
        default: return "terminal"
        }
    }

    static func color(for program: String?, theme: Theme) -> NSColor {
        switch symbol(for: program) {
        case "chevron.right": NSColor.systemGreen
        case "sparkle": NSColor.systemOrange
        case "hexagon": NSColor.systemGreen.mixed(with: .systemYellow, by: 0.3)
        case "plus.forwardslash.minus": NSColor.systemOrange.mixed(with: .systemRed, by: 0.3)
        default: theme.textPrimary
        }
    }

    /// Draws the program's icon into `rect`: its app's icon when it has one
    /// (Claude Code, Codex…), else a dark rounded tile with a glyph.
    @MainActor
    static func draw(program: String?, executable: String? = nil, in rect: NSRect, theme: Theme) {
        // Agents wear their own logo (svgl.app), in its light or dark version.
        if let logo = ProgramLogos.image(for: program, dark: theme.isDark) {
            ProgramLogos.draw(logo, in: rect)
            return
        }
        if let image = AppIcons.icon(program: program, executable: executable) {
            // App icons leave a margin around their shape; grow the rect so the shape fills it.
            let size = rect.width * 1.24
            image.drawUpright(in: NSRect(x: rect.midX - size / 2, y: rect.midY - size / 2, width: size, height: size))
            return
        }
        let tile = NSBezierPath(roundedRect: rect, xRadius: rect.width * 0.26, yRadius: rect.width * 0.26)
        (theme.isDark ? NSColor.black.withAlphaComponent(0.45) : NSColor.black.withAlphaComponent(0.75)).setFill()
        tile.fill()
        NSColor.white.withAlphaComponent(0.12).setStroke()
        tile.lineWidth = 0.5
        tile.stroke()
        let config = NSImage.SymbolConfiguration(pointSize: rect.height * 0.5, weight: .bold)
        guard let image = NSImage(systemSymbolName: symbol(for: program), accessibilityDescription: nil)?
            .withSymbolConfiguration(config)?.tinted(color(for: program, theme: theme)) else { return }
        let size = image.size
        image.drawUpright(in: NSRect(x: rect.midX - size.width / 2, y: rect.midY - size.height / 2, width: size.width, height: size.height))
    }
}

/// Icons of the apps behind terminal programs, taken from apps installed on
/// this Mac, so agents show their own logo the way Rex's tabs do.
@MainActor
enum AppIcons {
    /// CLI names and OSC 7501 app names mapped to the desktop app whose icon they wear.
    static let bundleIDs: [String: [String]] = [
        "claude": ["com.anthropic.claudefordesktop"],
        "codex": ["com.openai.codex", "com.openai.chat"],
        "cursor-agent": ["com.todesktop.230313mzl4w4u92"],
        "cursor": ["com.todesktop.230313mzl4w4u92"],
        "zed": ["dev.zed.Zed"],
        "code": ["com.microsoft.VSCode"],
        "opencode": ["ai.opencode.desktop"],
        "gemini": ["com.google.GeminiMacOS"],
        "copilot": ["com.github.CopilotForXcode"],
        "docker": ["com.docker.docker"],
    ]

    static let displayNames: [String: String] = [
        "claude": "Claude Code", "codex": "Codex", "cursor-agent": "Cursor Agent", "opencode": "OpenCode",
        "gemini": "Gemini CLI", "copilot": "GitHub Copilot", "aider": "Aider", "pi": "Pi", "amp": "Amp",
        "grok": "Grok", "qwen": "Qwen Code", "kimi": "Kimi", "vibe": "Mistral Vibe", "ollama": "Ollama",
        "kilo": "Kilo Code", "kilocode": "Kilo Code", "cursor": "Cursor", "deepseek": "DeepSeek", "windsurf": "Windsurf",
    ]

    /// A readable name for an agent ("Claude Code" for `claude`), or the program as is.
    static func displayName(for program: String) -> String {
        displayNames[normalized(program)] ?? program
    }

    private static var cache: [String: NSImage] = [:]
    private static var misses: Set<String> = []

    /// "claude-code" and "Claude" both mean claude.
    static func normalized(_ name: String) -> String {
        var name = name.lowercased()
        if name.hasSuffix("-code") { name = String(name.dropLast(5)) }
        return name
    }

    static func icon(program: String?, executable: String?) -> NSImage? {
        let key = "\(program.map(normalized) ?? "")|\(executable ?? "")"
        if let image = cache[key] { return image }
        if misses.contains(key) { return nil }
        let image = lookup(program: program, executable: executable)
        if let image { cache[key] = image } else { misses.insert(key) }
        return image
    }

    private static func lookup(program: String?, executable: String?) -> NSImage? {
        if let program, let ids = bundleIDs[normalized(program)] {
            for id in ids {
                if let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: id) {
                    return NSWorkspace.shared.icon(forFile: url.path)
                }
            }
        }
        // A program shipped inside an app bundle wears that app's icon, if it declares one.
        if let executable, let range = executable.range(of: ".app/Contents/MacOS/") {
            let bundlePath = String(executable[..<range.lowerBound]) + ".app"
            let info = Bundle(path: bundlePath)?.infoDictionary
            if bundlePath != Bundle.main.bundlePath, info?["CFBundleIconFile"] != nil || info?["CFBundleIconName"] != nil {
                return NSWorkspace.shared.icon(forFile: bundlePath)
            }
        }
        return nil
    }
}

/// The overlapping icons on a tab with several programs, front one first.
@MainActor
enum IconStack {
    static let offset: CGFloat = 9

    /// Width the stack takes for `count` icons of `size`.
    static func width(count: Int, size: CGFloat) -> CGFloat {
        size + CGFloat(max(count - 1, 0)) * offset
    }

    /// Draws back to front so the first icon sits on top, the others peeking out to its right.
    static func draw(_ icons: [(program: String?, executable: String?)], at origin: NSPoint, size: CGFloat, theme: Theme) {
        for (i, icon) in icons.enumerated().reversed() {
            let rect = NSRect(x: origin.x + CGFloat(i) * offset, y: origin.y, width: size, height: size)
            if i > 0 {
                // Dim the ones behind so the front icon reads first.
                NSGraphicsContext.saveGraphicsState()
                NSGraphicsContext.current?.cgContext.setAlpha(0.85)
                ProgramIcon.draw(program: icon.program, executable: icon.executable, in: rect, theme: theme)
                NSGraphicsContext.restoreGraphicsState()
            } else {
                ProgramIcon.draw(program: icon.program, executable: icon.executable, in: rect, theme: theme)
            }
        }
    }
}

/// Status marks: Rex's three-dot loader for working, a dot for the rest.
@MainActor
final class StatusIndicator {
    static func draw(_ state: PaneActivity, in rect: NSRect, theme: Theme, phase: CGFloat) {
        switch state {
        case .working:
            drawDots(in: rect, color: theme.textSecondary, phase: phase)
        case .blocked, .done, .error:
            let color = state == .blocked ? theme.blocked : state == .done ? theme.done : theme.error
            color.setFill()
            NSBezierPath(ovalIn: rect.insetBy(dx: rect.width * 0.2, dy: rect.height * 0.2)).fill()
        case .idle:
            break
        }
    }

    /// Three dots pulsing left to right, one wave per second. `rect` is the
    /// space for the whole row; dots are centered in it.
    static func drawDots(in rect: NSRect, color: NSColor, phase: CGFloat) {
        let size: CGFloat = 3.5
        let gap: CGFloat = 3
        let total = size * 3 + gap * 2
        let wave = -phase * .pi / 180
        for i in 0..<3 {
            let pulse = max(0, cos(wave - CGFloat(i) * 0.9))
            color.withAlphaComponent(0.25 + 0.75 * pulse * pulse).setFill()
            let x = rect.midX - total / 2 + CGFloat(i) * (size + gap)
            NSBezierPath(ovalIn: NSRect(x: x, y: rect.midY - size / 2, width: size, height: size)).fill()
        }
    }

    /// Width the three-dot loader needs.
    static let dotsWidth: CGFloat = 16.5
}
