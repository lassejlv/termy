import AppKit

/// The copied selection catches the light: a glint sweeps across the
/// highlighted cells while they briefly brighten, so a copy feels like it
/// landed. It is drawn in an overlay above the terminal, clipped to the
/// selection's shape, and removes itself when done.
@MainActor
enum CopyShine {
    static func play(over rects: [CGRect], of terminal: NSView) {
        guard !rects.isEmpty, let host = terminal.superview else { return }
        let overlay = ShineOverlay(frame: terminal.frame)
        host.addSubview(overlay, positioned: .above, relativeTo: terminal)
        guard let root = overlay.layer else { return overlay.removeFromSuperview() }

        // Clip everything to the selection's outline.
        let outline = CGMutablePath()
        for rect in rects { outline.addRoundedRect(in: rect.insetBy(dx: -0.5, dy: -0.5), cornerWidth: 2, cornerHeight: 2) }
        let mask = CAShapeLayer()
        mask.path = outline
        let content = CALayer()
        content.frame = root.bounds
        content.mask = mask
        root.addSublayer(content)

        let bounds = outline.boundingBoxOfPath
        let duration = Motion.reduced ? 0.3 : 0.7

        // The cells brighten, then settle.
        let flash = CALayer()
        flash.frame = bounds
        flash.backgroundColor = NSColor.white.withAlphaComponent(0.08).cgColor
        flash.compositingFilter = "plusL"
        flash.opacity = 0
        content.addSublayer(flash)
        let glow = CAKeyframeAnimation(keyPath: "opacity")
        glow.values = [0, 1, 0]
        glow.keyTimes = [0, 0.18, 1]
        glow.duration = duration
        flash.add(glow, forKey: "glow")

        if !Motion.reduced {
            // A slanted band of light crosses the selection.
            let bandWidth = max(36, min(110, bounds.width * 0.22))
            let band = CAGradientLayer()
            band.colors = [
                NSColor.white.withAlphaComponent(0).cgColor,
                NSColor.white.withAlphaComponent(0.12).cgColor,
                NSColor.white.withAlphaComponent(0.3).cgColor,
                NSColor.white.withAlphaComponent(0.12).cgColor,
                NSColor.white.withAlphaComponent(0).cgColor,
            ]
            band.locations = [0, 0.35, 0.5, 0.65, 1]
            band.startPoint = CGPoint(x: 0, y: 0.5)
            band.endPoint = CGPoint(x: 1, y: 0.5)
            band.compositingFilter = "plusL"
            // Tall enough to stay covered when slanted.
            let height = bounds.height + bandWidth * 2
            band.bounds = CGRect(x: 0, y: 0, width: bandWidth, height: height)
            band.setAffineTransform(CGAffineTransform(rotationAngle: .pi / 9))
            band.position = CGPoint(x: bounds.minX - bandWidth, y: bounds.midY)
            content.addSublayer(band)
            let sweep = CABasicAnimation(keyPath: "position.x")
            sweep.fromValue = bounds.minX - bandWidth
            sweep.toValue = bounds.maxX + bandWidth
            sweep.duration = duration
            sweep.timingFunction = CAMediaTimingFunction(controlPoints: 0.3, 0, 0.2, 1)
            band.add(sweep, forKey: "sweep")
            band.position.x = bounds.maxX + bandWidth
        }

        DispatchQueue.main.asyncAfter(deadline: .now() + duration + 0.05) {
            overlay.removeFromSuperview()
        }
    }
}

/// A click-through layer-hosting view matching the terminal's coordinates.
private final class ShineOverlay: NSView {
    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.masksToBounds = true
        autoresizingMask = [.width, .height]
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}
