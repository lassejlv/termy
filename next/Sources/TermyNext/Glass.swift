import AppKit
import QuartzCore

/// Liquid Glass where the OS has it (`NSGlassEffectView`, macOS 26), a
/// blurred material before that. Clicks pass through to the views beneath.
@MainActor
final class GlassView: NSView {
    enum Style { case regular, clear }

    var cornerRadius: CGFloat { didSet { applyShape() } }
    /// Keeps the corners fully round at any height.
    var isCapsule = false { didSet { needsLayout = true } }
    var tint: NSColor? { didSet { applyTint() } }
    private let effect: NSView

    init(style: Style = .regular, cornerRadius: CGFloat = 12, tint: NSColor? = nil, interactive: Bool = false) {
        self.cornerRadius = cornerRadius
        self.tint = tint
        if #available(macOS 26.0, *) {
            let glass = StillGlassEffectView()
            glass.style = style == .clear ? .clear : .regular
            // Interactive glass is macOS 27 API. Set it by name so the app
            // still builds with the macOS 26 SDK that CI runners have.
            if glass.responds(to: NSSelectorFromString("setEffectIsInteractive:")) {
                glass.setValue(interactive, forKey: "effectIsInteractive")
            }
            effect = glass
        } else {
            let material = StillVisualEffectView()
            material.material = .hudWindow
            material.blendingMode = .withinWindow
            material.state = .active
            material.wantsLayer = true
            effect = material
        }
        super.init(frame: .zero)
        addSubview(effect)
        applyShape()
        applyTint()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override func hitTest(_ point: NSPoint) -> NSView? { nil }
    /// The app drags the window itself from empty titlebar; glass on a tab
    /// or button must never start a window drag.
    override var mouseDownCanMoveWindow: Bool { false }

    override func layout() {
        super.layout()
        effect.frame = bounds
        if isCapsule { cornerRadius = bounds.height / 2 }
    }

    private func applyShape() {
        if #available(macOS 26.0, *), let glass = effect as? NSGlassEffectView {
            glass.cornerRadius = cornerRadius
        } else {
            effect.layer?.cornerRadius = cornerRadius
            effect.layer?.cornerCurve = .continuous
            effect.layer?.masksToBounds = true
        }
    }

    private func applyTint() {
        if #available(macOS 26.0, *), let glass = effect as? NSGlassEffectView {
            glass.tintColor = tint
        } else {
            effect.layer?.backgroundColor = tint?.cgColor
        }
    }
}

/// A view that is only drawing: presses go to the view under it. A plain
/// see-through view in the titlebar would otherwise let AppKit drag the window.
@MainActor
final class PassthroughView: NSView {
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
    override var mouseDownCanMoveWindow: Bool { false }
}

/// Glass and material that never start a window drag (see `GlassView`).
@available(macOS 26.0, *)
private final class StillGlassEffectView: NSGlassEffectView {
    override var mouseDownCanMoveWindow: Bool { false }
}

private final class StillVisualEffectView: NSVisualEffectView {
    override var mouseDownCanMoveWindow: Bool { false }
}

/// Draws on behalf of its owner, above the owner's glass background.
@MainActor
final class DrawingView: NSView {
    weak var owner: GlassPanel?
    override var isFlipped: Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
    override var mouseDownCanMoveWindow: Bool { false }
    override func draw(_ dirtyRect: NSRect) { owner?.drawContent(dirtyRect) }
}

/// A view that can sit on Liquid Glass. Subclasses draw in `drawContent`,
/// which lands above the glass and below their subviews.
@MainActor
class GlassPanel: NSView {
    private(set) var glass: GlassView?
    let overlay = DrawingView()
    /// Where the glass sits inside the bounds.
    var glassRect: NSRect { bounds }

    override init(frame: NSRect) {
        super.init(frame: frame)
        overlay.owner = self
        addSubview(overlay)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }

    /// Turns the glass background on or off and restyles it.
    func setGlass(_ enabled: Bool, cornerRadius: CGFloat = 12, tint: NSColor? = nil, style: GlassView.Style = .regular, capsule: Bool = false) {
        if enabled {
            if glass == nil {
                let view = GlassView(style: style, cornerRadius: cornerRadius, tint: tint)
                // The background goes under everything else the panel holds.
                addSubview(view, positioned: .below, relativeTo: subviews.first)
                glass = view
            }
            glass?.cornerRadius = cornerRadius
            glass?.isCapsule = capsule
            glass?.tint = tint
        } else {
            glass?.removeFromSuperview()
            glass = nil
        }
        needsLayout = true
    }

    override func layout() {
        super.layout()
        glass?.frame = glassRect
        overlay.frame = bounds
    }

    override var needsDisplay: Bool {
        get { super.needsDisplay }
        set {
            super.needsDisplay = newValue
            if newValue { overlay.needsDisplay = true }
        }
    }

    func drawContent(_ dirtyRect: NSRect) {}
}

/// Shared timing for Termy Next's motion: quick, soft-landing curves, off
/// when the user asks for reduced motion.
@MainActor
enum Motion {
    static var reduced: Bool { NSWorkspace.shared.accessibilityDisplayShouldReduceMotion }
    /// Fast start, long gentle settle, close to a critically damped spring.
    static let curve = CAMediaTimingFunction(controlPoints: 0.2, 0.9, 0.25, 1)
    nonisolated static let standard: TimeInterval = 0.32

    /// Runs `changes` animated; frames set through `animator()` and layouts
    /// performed inside animate together.
    static func animate(_ duration: TimeInterval = standard, _ changes: () -> Void, completion: (() -> Void)? = nil) {
        guard !reduced, duration > 0 else {
            changes()
            completion?()
            return
        }
        // Animation completions run on the main thread.
        nonisolated(unsafe) let done = completion
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = duration
            context.timingFunction = curve
            context.allowsImplicitAnimation = true
            changes()
        }, completionHandler: { MainActor.assumeIsolated { done?() } })
    }

    /// Applies `changes` with no animation, even inside an animation group.
    static func instantly(_ changes: () -> Void) {
        NSAnimationContext.beginGrouping()
        NSAnimationContext.current.duration = 0
        NSAnimationContext.current.allowsImplicitAnimation = false
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        changes()
        CATransaction.commit()
        NSAnimationContext.endGrouping()
    }

    /// Scales and fades a view in from its center.
    static func appear(_ view: NSView, scale: CGFloat = 0.94, offsetY: CGFloat = 0, duration: TimeInterval = standard) {
        guard !reduced else { return }
        view.wantsLayer = true
        guard let layer = view.layer else { return }
        let size = view.bounds.size
        var start = CATransform3DMakeTranslation(size.width * (1 - scale) / 2, size.height * (1 - scale) / 2 + offsetY, 0)
        start = CATransform3DScale(start, scale, scale, 1)
        let transform = CABasicAnimation(keyPath: "transform")
        transform.fromValue = NSValue(caTransform3D: start)
        transform.toValue = NSValue(caTransform3D: CATransform3DIdentity)
        let fade = CABasicAnimation(keyPath: "opacity")
        fade.fromValue = 0
        fade.toValue = 1
        let group = CAAnimationGroup()
        group.animations = [transform, fade]
        group.duration = duration
        group.timingFunction = curve
        group.fillMode = .backwards
        layer.add(group, forKey: "termyAppear")
    }

    /// Scales and fades a view out, then calls `completion`.
    static func disappear(_ view: NSView, scale: CGFloat = 0.96, duration: TimeInterval = 0.2, completion: @escaping () -> Void) {
        guard !reduced, let layer = view.layer, view.window != nil else {
            completion()
            return
        }
        let size = view.bounds.size
        var end = CATransform3DMakeTranslation(size.width * (1 - scale) / 2, size.height * (1 - scale) / 2, 0)
        end = CATransform3DScale(end, scale, scale, 1)
        CATransaction.begin()
        nonisolated(unsafe) let done = completion
        CATransaction.setCompletionBlock { MainActor.assumeIsolated { done() } }
        let transform = CABasicAnimation(keyPath: "transform")
        transform.toValue = NSValue(caTransform3D: end)
        let fade = CABasicAnimation(keyPath: "opacity")
        fade.toValue = 0
        let group = CAAnimationGroup()
        group.animations = [transform, fade]
        group.duration = duration
        group.timingFunction = CAMediaTimingFunction(name: .easeIn)
        group.fillMode = .forwards
        group.isRemovedOnCompletion = false
        layer.add(group, forKey: "termyDisappear")
        CATransaction.commit()
    }

    /// Crossfades whatever `view` shows next, such as a different tab.
    static func crossfade(_ view: NSView, duration: TimeInterval = 0.2) {
        guard !reduced, let layer = view.layer else { return }
        let transition = CATransition()
        transition.type = .fade
        transition.duration = duration
        transition.timingFunction = CAMediaTimingFunction(name: .easeOut)
        layer.add(transition, forKey: "termyCrossfade")
    }
}
