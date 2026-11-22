import AppKit

/// The Dock icon Termy Next shows while it runs: the bundle's own (Glass), or
/// one of the alternates in `Resources/icon/alternates`, named by raw value
/// (bundled into `Contents/Resources/AppIcons` by scripts/bundle.sh). Finder
/// and a Dock that isn't running the app keep showing the bundle's icon,
/// since changing that would break the app's signature.
enum AppIconStyle: String, CaseIterable {
    // The bundle's icon, Resources/icon/TermyNext.icns.
    case glass
    // The mascot.
    case mono, candy
    case matcha = "mascot-matcha", clay = "mascot-clay", neon = "mascot-neon"
    case pixel = "mascot-pixel", holo = "mascot-holo"
    // The >_ prompt; classic was the bundle's icon before Glass.
    case classic
    case pearl = "classic-pearl", graphite = "classic-graphite", sunset = "classic-sunset"
    case phosphor = "classic-phosphor", amber = "classic-amber"

    static let standard = AppIconStyle.glass

    var title: String {
        switch self {
        case .glass: "Glass"
        case .mono: "Doodle"
        case .candy: "Candy"
        case .matcha: "Matcha"
        case .clay: "Clay"
        case .neon: "Neon"
        case .pixel: "Pixel"
        case .holo: "Holo"
        case .classic: "Classic"
        case .pearl: "Pearl"
        case .graphite: "Graphite"
        case .sunset: "Sunset"
        case .phosphor: "Phosphor"
        case .amber: "Amber"
        }
    }

    /// The source tree's icon folder, for `swift run`, which has no bundle.
    private static let sourceIcons = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .appendingPathComponent("Resources/icon")

    /// The icon's image; nil for the bundle's own.
    var image: NSImage? {
        guard self != .standard else { return nil }
        if let url = Bundle.main.url(forResource: rawValue, withExtension: "png", subdirectory: "AppIcons") {
            return NSImage(contentsOf: url)
        }
        return NSImage(contentsOf: Self.sourceIcons.appendingPathComponent("alternates/\(rawValue).png"))
    }

    /// The image to show in a picker: the alternate, or the bundle's icon.
    @MainActor var preview: NSImage {
        if let image { return image }
        // The bundle's icon as Finder shows it, which `apply()` doesn't change.
        if Bundle.main.bundleIdentifier != nil { return NSWorkspace.shared.icon(forFile: Bundle.main.bundlePath) }
        return NSImage(contentsOf: Self.sourceIcons.appendingPathComponent("termy-next-icon.png")) ?? NSImage()
    }

    /// Shows this icon in the Dock and app switcher; the standard one puts the bundle's back.
    @MainActor func apply() {
        NSApp.applicationIconImage = image
    }
}

/// The app icons in a grid; clicking one picks it.
@MainActor
final class AppIconPicker: NSView {
    var onPick: ((AppIconStyle) -> Void)?
    var selected: AppIconStyle = .standard {
        didSet { if selected != oldValue { updateSelection() } }
    }

    private static let tile: CGFloat = 52
    private static let spacing: CGFloat = 12
    private var cells: [(style: AppIconStyle, ring: NSView, label: NSTextField)] = []

    init(width: CGFloat) {
        super.init(frame: .zero)
        translatesAutoresizingMaskIntoConstraints = false
        let cellWidth = Self.tile + 8
        let columns = max(1, Int((width + Self.spacing) / (cellWidth + Self.spacing)))
        let grid = NSStackView()
        grid.orientation = .vertical
        grid.alignment = .leading
        grid.spacing = 10
        grid.translatesAutoresizingMaskIntoConstraints = false
        var row: NSStackView?
        for (index, style) in AppIconStyle.allCases.enumerated() {
            if index % columns == 0 {
                let next = NSStackView()
                next.orientation = .horizontal
                next.alignment = .top
                next.spacing = Self.spacing
                grid.addArrangedSubview(next)
                row = next
            }
            row?.addArrangedSubview(cell(style, index: index, width: cellWidth))
        }
        addSubview(grid)
        NSLayoutConstraint.activate([
            widthAnchor.constraint(equalToConstant: width),
            grid.leadingAnchor.constraint(equalTo: leadingAnchor),
            grid.topAnchor.constraint(equalTo: topAnchor, constant: 4),
            grid.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -4),
        ])
        setAccessibilityElement(true)
        setAccessibilityRole(.radioGroup)
        setAccessibilityLabel("App icon")
        updateSelection()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    private func cell(_ style: AppIconStyle, index: Int, width: CGFloat) -> NSView {
        let button = NSButton(image: style.preview, target: self, action: #selector(picked(_:)))
        button.isBordered = false
        button.imageScaling = .scaleProportionallyUpOrDown
        button.tag = index
        button.setAccessibilityLabel("\(style.title) app icon")
        button.translatesAutoresizingMaskIntoConstraints = false

        let ring = NSView()
        ring.wantsLayer = true
        ring.layer?.cornerRadius = 15
        ring.layer?.borderWidth = 2.5
        ring.translatesAutoresizingMaskIntoConstraints = false
        ring.addSubview(button)

        let label = NSTextField(labelWithString: style.title)
        label.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        label.alignment = .center
        label.lineBreakMode = .byTruncatingTail

        let column = NSStackView(views: [ring, label])
        column.orientation = .vertical
        column.spacing = 3
        NSLayoutConstraint.activate([
            column.widthAnchor.constraint(equalToConstant: width),
            ring.widthAnchor.constraint(equalToConstant: width),
            ring.heightAnchor.constraint(equalToConstant: width),
            button.widthAnchor.constraint(equalToConstant: Self.tile),
            button.heightAnchor.constraint(equalToConstant: Self.tile),
            button.centerXAnchor.constraint(equalTo: ring.centerXAnchor),
            button.centerYAnchor.constraint(equalTo: ring.centerYAnchor),
        ])
        cells.append((style, ring, label))
        return column
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        updateSelection()
    }

    private func updateSelection() {
        for cell in cells {
            let on = cell.style == selected
            effectiveAppearance.performAsCurrentDrawingAppearance {
                cell.ring.layer?.borderColor = on ? NSColor.controlAccentColor.cgColor : NSColor.clear.cgColor
            }
            cell.label.textColor = on ? .labelColor : .secondaryLabelColor
        }
    }

    @objc private func picked(_ sender: NSButton) {
        let style = AppIconStyle.allCases[sender.tag]
        selected = style
        onPick?(style)
    }
}
