import AppKit

/// The indicator a working tab shows. `dots` is Termy Next's own three-dot
/// loader and the default; the rest are native ports of the spinners from
/// loading.dev by Jakub Krehel and Paul Faivret (MIT License).
enum TabLoader: String, CaseIterable {
    case dots
    case arc, atom, blocks, bouncingDots = "bouncing-dots", cascade, circularDots = "circular-dots"
    case classic, classicV2 = "classic-v2", clock, comet, compass, dual, eclipse, flip, gather, leap
    case linearDots = "linear-dots", loading, morph, orbit, pulse, radar, ring, ripple, slide, snake
    case swirl, trace, wave

    var title: String {
        switch self {
        case .dots: "Dots"
        case .bouncingDots: "Bouncing Dots"
        case .circularDots: "Circular Dots"
        case .classicV2: "Classic v2"
        case .linearDots: "Linear Dots"
        default: rawValue.capitalized
        }
    }

    /// One loop, in seconds (loading.dev's defaults).
    var duration: Double {
        switch self {
        case .dots: 1
        case .arc: 0.8
        case .atom: 1
        case .blocks: 1.3
        case .bouncingDots: 0.5
        case .cascade: 1.5
        case .circularDots: 0.8
        case .classic: 1.2
        case .classicV2: 0.8
        case .clock: 1.2
        case .comet: 0.7
        case .compass: 0.5
        case .dual: 1
        case .eclipse: 1.2
        case .flip: 1.2
        case .gather: 1.6
        case .leap: 1.8
        case .linearDots: 0.9
        case .loading: 1
        case .morph: 1.2
        case .orbit: 0.75
        case .pulse: 1.2
        case .radar: 1.5
        case .ring: 0.8
        case .ripple: 1.2
        case .slide: 2.4
        case .snake: 1.4
        case .swirl: 1.2
        case .trace: 1.2
        case .wave: 0.9
        }
    }

    /// Width over height; the dot rows are wider than they are tall.
    var aspect: CGFloat {
        switch self {
        case .bouncingDots: 1.15
        case .linearDots: 0.9375
        default: 1
        }
    }
}

/// Draws loading.dev's spinners with Core Graphics, frame by frame, so they
/// can sit inside views that draw themselves (the tab strip, the sidebar).
@MainActor
enum LoaderDrawing {
    /// Draws `loader` as it looks `time` seconds into its loop, centered in
    /// `rect` at up to `size` points tall.
    static func draw(_ loader: TabLoader, in rect: NSRect, size: CGFloat, color: NSColor, time: Double) {
        guard let context = NSGraphicsContext.current?.cgContext else { return }
        let side = min(size, rect.height, rect.width / loader.aspect)
        guard side > 0 else { return }
        let width = side * loader.aspect
        let box = NSRect(x: rect.midX - width / 2, y: rect.midY - side / 2, width: width, height: side)
        context.saveGState()
        defer { context.restoreGState() }
        // Draw in a y-down unit space, like CSS and SVG.
        context.translateBy(x: box.minX, y: box.minY)
        if !(NSGraphicsContext.current?.isFlipped ?? false) {
            context.translateBy(x: 0, y: side)
            context.scaleBy(x: 1, y: -1)
        }
        context.scaleBy(x: side, y: side)
        let p = (time / loader.duration).truncatingRemainder(dividingBy: 1)
        let painter = Painter(context: context, color: color)
        painter.draw(loader, progress: p < 0 ? p + 1 : p, time: time)
    }
}

// MARK: - Timing

/// CSS's cubic-bezier timing functions.
private struct Bezier {
    let x1, y1, x2, y2: Double
    static let linear = Bezier(x1: 0, y1: 0, x2: 1, y2: 1)
    static let easeInOut = Bezier(x1: 0.42, y1: 0, x2: 0.58, y2: 1)
    static let easeOut = Bezier(x1: 0, y1: 0, x2: 0.58, y2: 1)

    func callAsFunction(_ x: Double) -> Double {
        if x <= 0 { return 0 }
        if x >= 1 { return 1 }
        func sample(_ a: Double, _ b: Double, _ t: Double) -> Double {
            let u = 1 - t
            return 3 * u * u * t * a + 3 * u * t * t * b + t * t * t
        }
        // x(t) rises monotonically, so bisection finds t.
        var low = 0.0, high = 1.0, t = x
        for _ in 0..<24 {
            let value = sample(x1, x2, t)
            if abs(value - x) < 1e-5 { break }
            if value < x { low = t } else { high = t }
            t = (low + high) / 2
        }
        return sample(y1, y2, t)
    }
}

/// The value at progress `p` through CSS keyframes, eased between each pair.
private func keyframes(_ p: Double, _ frames: [(Double, Double)], _ ease: Bezier = .easeInOut) -> Double {
    guard let first = frames.first else { return 0 }
    if p <= first.0 { return first.1 }
    for (a, b) in zip(frames, frames.dropFirst()) where p <= b.0 {
        let span = b.0 - a.0
        let local = span > 0 ? (p - a.0) / span : 1
        return a.1 + (b.1 - a.1) * ease(local)
    }
    return frames.last!.1
}

/// Progress of element `index` of `count`, each one step behind the last,
/// as loading.dev staggers them with negative delays.
private func lagged(_ p: Double, _ index: Int, _ count: Int) -> Double {
    let value = (p - Double(index) / Double(count)).truncatingRemainder(dividingBy: 1)
    return value < 0 ? value + 1 : value
}

// MARK: - Spinners

private struct Painter {
    let context: CGContext
    let color: NSColor

    private func fill(_ alpha: CGFloat = 1) { context.setFillColor(color.withAlphaComponent(color.alphaComponent * alpha).cgColor) }
    private func stroke(_ alpha: CGFloat = 1) { context.setStrokeColor(color.withAlphaComponent(color.alphaComponent * alpha).cgColor) }

    /// Runs `body` in a `view`-unit space (an SVG viewBox) over the unit box.
    private func viewBox(_ view: CGFloat, _ body: () -> Void) {
        context.saveGState()
        context.scaleBy(x: 1 / view, y: 1 / view)
        body()
        context.restoreGState()
    }

    private func circle(_ cx: CGFloat, _ cy: CGFloat, _ r: CGFloat, alpha: CGFloat = 1) {
        fill(alpha)
        context.fillEllipse(in: CGRect(x: cx - r, y: cy - r, width: r * 2, height: r * 2))
    }

    private func ring(_ cx: CGFloat, _ cy: CGFloat, _ r: CGFloat, width: CGFloat, alpha: CGFloat = 1) {
        stroke(alpha)
        context.setLineWidth(width)
        context.strokeEllipse(in: CGRect(x: cx - r, y: cy - r, width: r * 2, height: r * 2))
    }

    /// An arc starting at `start` radians (0 = 3 o'clock, growing clockwise)
    /// and `length` long along the circle.
    private func arc(_ cx: CGFloat, _ cy: CGFloat, _ r: CGFloat, start: CGFloat, length: CGFloat,
                     width: CGFloat, alpha: CGFloat = 1, cap: CGLineCap = .round) {
        guard length > 0 else { return }
        stroke(alpha)
        context.setLineWidth(width)
        context.setLineCap(cap)
        context.beginPath()
        context.addArc(center: CGPoint(x: cx, y: cy), radius: r, startAngle: start, endAngle: start + length / r, clockwise: false)
        context.strokePath()
    }

    private func roundedRect(_ rect: CGRect, radius: CGFloat, alpha: CGFloat = 1) {
        fill(alpha)
        let r = min(radius, rect.width / 2, rect.height / 2)
        context.addPath(CGPath(roundedRect: rect, cornerWidth: max(0, r), cornerHeight: max(0, r), transform: nil))
        context.fillPath()
    }

    /// A rounded tick from `inner` to `outer` along `angle` (0 = 12 o'clock).
    private func tick(_ cx: CGFloat, _ cy: CGFloat, angle: CGFloat, inner: CGFloat, outer: CGFloat, width: CGFloat, alpha: CGFloat = 1) {
        let dx = sin(angle), dy = -cos(angle)
        stroke(alpha)
        context.setLineWidth(width)
        context.setLineCap(.round)
        context.beginPath()
        context.move(to: CGPoint(x: cx + dx * inner, y: cy + dy * inner))
        context.addLine(to: CGPoint(x: cx + dx * outer, y: cy + dy * outer))
        context.strokePath()
    }

    private func rotated(by angle: CGFloat, around center: CGPoint, _ body: () -> Void) {
        context.saveGState()
        context.translateBy(x: center.x, y: center.y)
        context.rotate(by: angle)
        context.translateBy(x: -center.x, y: -center.y)
        body()
        context.restoreGState()
    }

    private static let turn = CGFloat.pi * 2
    private static let top = -CGFloat.pi / 2

    func draw(_ loader: TabLoader, progress p: Double, time: Double) {
        let turn = Self.turn, top = Self.top
        let spin = CGFloat(p) * turn
        switch loader {
        case .dots:
            break

        case .arc:
            viewBox(24) { arc(12, 12, 10, start: spin, length: 18, width: 2.5) }

        case .ring:
            viewBox(24) {
                ring(12, 12, 10, width: 2.5, alpha: 0.2)
                arc(12, 12, 10, start: spin, length: 16, width: 2.5)
            }

        case .dual:
            viewBox(24) {
                arc(12, 12, 10, start: spin, length: 18, width: 2.5)
                arc(12, 12, 5.5, start: -spin, length: 10, width: 2.5)
            }

        case .snake:
            viewBox(24) {
                let length = keyframes(p, [(0, 1), (0.5, 45), (1, 45)])
                let offset = keyframes(p, [(0, 0), (0.5, 17), (1, 62)])
                arc(12, 12, 10, start: spin + CGFloat(offset) / 10, length: CGFloat(length), width: 2.5)
            }

        case .cascade:
            viewBox(24) {
                let ease = Bezier(x1: 0.68, y1: -0.75, x2: 0.265, y2: 1.75)
                for (index, r) in [10.5, 7, 3.5].enumerated() {
                    let angle = CGFloat(ease(lagged(p, index, 24))) * turn
                    arc(12, 12, r, start: angle, length: 2 * .pi * r / 4, width: 2)
                }
            }

        case .atom:
            let r = 0.5 - 0.0225
            ring(0.5, 0.5, 0.5 - 0.0275, width: 0.055)
            for index in 0..<3 {
                let a = CGFloat(lagged(p, index, 3)) * turn
                rotated(by: CGFloat(index) * .pi / 3, around: CGPoint(x: 0.5, y: 0.5)) {
                    let w = max(0.001, r * abs(cos(a)))
                    stroke()
                    context.setLineWidth(0.045)
                    context.strokeEllipse(in: CGRect(x: 0.5 - w, y: 0.5 - r, width: w * 2, height: r * 2))
                }
            }

        case .blocks:
            let cell = (1 - 0.2) / 3
            for row in 0..<3 {
                for col in 0..<3 {
                    let scale = keyframes(lagged(p, row + col, 5), [(0, 1), (0.35, 0), (0.7, 1), (1, 1)])
                    let size = cell * scale
                    let cx = CGFloat(col) * (cell + 0.1) + cell / 2, cy = CGFloat(row) * (cell + 0.1) + cell / 2
                    roundedRect(CGRect(x: cx - size / 2, y: cy - size / 2, width: size, height: size), radius: 0.0625 * scale)
                }
            }

        case .bouncingDots:
            // A 0.5 s bounce that alternates direction, so one loop is 1 s.
            for index in 0..<3 {
                let local = (time / 0.5 - Double(index) / 3 + 1).truncatingRemainder(dividingBy: 2)
                let forward = local < 1
                let f = forward ? local : local - 1
                let eased = Bezier.easeInOut(forward ? f : 1 - f)
                let dy = 0.25 * (0.28 - eased)
                circle(0.125 + CGFloat(index) * 0.45, 0.5 + dy, 0.125)
            }

        case .circularDots:
            let dots: [(CGFloat, CGFloat)] = [(8, 1.5), (12.5962, 3.4038), (14.5, 8), (12.5962, 12.5962),
                                              (8, 14.5), (3.4038, 12.5962), (1.5, 8), (3.4038, 3.4038)]
            viewBox(16) {
                for (index, dot) in dots.enumerated() {
                    circle(dot.0, dot.1, 1.5, alpha: 1 - 0.8 * lagged(p, index, 8))
                }
            }

        case .classic:
            for index in 0..<12 {
                let alpha = 1 - 0.85 * lagged(p, index, 12)
                rotated(by: CGFloat(index) * .pi / 6, around: CGPoint(x: 0.5, y: 0.5)) {
                    roundedRect(CGRect(x: 0.5 + 0.35 - 0.12, y: 0.46, width: 0.24, height: 0.08), radius: 0.04, alpha: alpha)
                }
            }

        case .classicV2:
            viewBox(16) {
                for index in 0..<8 {
                    tick(8, 8, angle: CGFloat(index) * .pi / 4, inner: 4.667, outer: 7.333, width: 1.333,
                         alpha: 1 - 0.6 * lagged(p, index, 8))
                }
            }

        case .compass:
            viewBox(16) {
                rotated(by: CGFloat(Bezier.easeInOut(p)) * .pi / 2, around: CGPoint(x: 8, y: 8)) {
                    for index in 0..<4 {
                        tick(8, 8, angle: CGFloat(index) * .pi / 2, inner: 4.667, outer: 7.333, width: 1.333)
                    }
                }
            }

        case .clock:
            viewBox(16) {
                circle(8, 8, 8, alpha: 0.1)
                fill()
                context.beginPath()
                context.move(to: CGPoint(x: 8, y: 8))
                context.addArc(center: CGPoint(x: 8, y: 8), radius: 6, startAngle: top + spin, endAngle: top + spin + 2.15, clockwise: false)
                context.closePath()
                context.fillPath()
            }

        case .comet:
            let r = 0.5 - 0.06
            let segments = 60
            for index in 0..<segments {
                let start = top + spin + CGFloat(index) / CGFloat(segments) * turn
                arc(0.5, 0.5, r, start: start, length: r * turn / CGFloat(segments) * 1.08, width: 0.12,
                    alpha: CGFloat(index + 1) / CGFloat(segments), cap: .butt)
            }
            let head = CGPoint(x: 0.5 + r * cos(top + spin), y: 0.5 + r * sin(top + spin))
            circle(head.x, head.y, 0.06)

        case .orbit:
            let r = 0.42 - 0.03125
            let segments = 40
            for index in 0..<segments {
                // From the bottom round the left to the top, fading in.
                let start = .pi / 2 + spin + CGFloat(index) / CGFloat(segments) * .pi
                arc(0.5, 0.5, r, start: start, length: r * .pi / CGFloat(segments) * 1.08, width: 0.0625,
                    alpha: CGFloat(index + 1) / CGFloat(segments), cap: .butt)
            }
            circle(0.5, 0.5, 0.125)

        case .radar:
            viewBox(16) {
                circle(8, 8, 8, alpha: 0.2)
                ring(8, 8, 5.5, width: 1, alpha: 0.2)
                context.saveGState()
                context.beginPath()
                context.move(to: CGPoint(x: 8, y: 8))
                context.addArc(center: CGPoint(x: 8, y: 8), radius: 8, startAngle: top + spin, endAngle: top + spin + 2.24, clockwise: false)
                context.closePath()
                context.clip()
                let colors = [color.cgColor, color.withAlphaComponent(0).cgColor] as CFArray
                if let gradient = CGGradient(colorsSpace: nil, colors: colors, locations: [0.3334, 1]) {
                    context.drawRadialGradient(gradient, startCenter: CGPoint(x: 8, y: 8), startRadius: 0,
                                               endCenter: CGPoint(x: 8, y: 8), endRadius: 8, options: [])
                }
                context.restoreGState()
                circle(8, 8, 2)
            }

        case .eclipse:
            var dots: [(x: CGFloat, scale: CGFloat, alpha: CGFloat)] = []
            for index in 0..<2 {
                let q = lagged(p, index, 2)
                let x = keyframes(q, [(0, 0.3), (0.5, -0.3), (1, 0.3)])
                let scale = keyframes(q, [(0, 1), (0.25, 1.3), (0.5, 1), (0.75, 0.7), (1, 1)])
                let alpha = keyframes(q, [(0, 0.75), (0.25, 1), (0.5, 0.75), (0.75, 0.5), (1, 0.75)])
                dots.append((CGFloat(x), CGFloat(scale), CGFloat(alpha)))
            }
            // The nearer (bigger) dot goes in front.
            for dot in dots.sorted(by: { $0.scale < $1.scale }) {
                circle(0.5 + dot.x, 0.5, 0.2 * dot.scale, alpha: dot.alpha)
            }

        case .flip:
            let x = keyframes(p, [(0, 0), (0.5, 1), (1, 1)])
            let y = keyframes(p, [(0, 0), (0.5, 0), (1, 1)])
            let height = 0.64 * abs(cos(CGFloat(x) * .pi)), width = 0.64 * abs(cos(CGFloat(y) * .pi))
            roundedRect(CGRect(x: 0.5 - width / 2, y: 0.5 - height / 2, width: width, height: height), radius: 0.06)

        case .gather:
            let block: CGFloat = 0.38
            let pull = CGFloat(keyframes(p, [(0, 0), (0.3, 1), (0.6, 1), (1, 0)])) * 0.08
            let angle = CGFloat(keyframes(p, [(0, 0), (0.3, 0), (0.6, 1), (1, 1)])) * .pi / 2
            rotated(by: angle, around: CGPoint(x: 0.5, y: 0.5)) {
                for (sx, sy) in [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] as [(CGFloat, CGFloat)] {
                    let cx = 0.5 - sx * 0.31 + sx * pull, cy = 0.5 - sy * 0.31 + sy * pull
                    roundedRect(CGRect(x: cx - block / 2, y: cy - block / 2, width: block, height: block), radius: 0.14)
                }
            }

        case .leap:
            let gap: CGFloat = 0.39
            for index in 0..<3 {
                let q = lagged(p, index, 3)
                let point: CGPoint
                if q < 1 / 3 {
                    let a = .pi + CGFloat(Bezier.easeInOut(q * 3)) * .pi
                    point = CGPoint(x: 0.5 + gap * cos(a), y: 0.5 + gap * sin(a))
                } else if q < 2 / 3 {
                    point = CGPoint(x: 0.89 - gap * CGFloat(Bezier.easeInOut(q * 3 - 1)), y: 0.5)
                } else {
                    point = CGPoint(x: 0.5 - gap * CGFloat(Bezier.easeInOut(q * 3 - 2)), y: 0.5)
                }
                circle(point.x, point.y, 0.11)
            }

        case .linearDots:
            for index in 0..<3 {
                let alpha = keyframes(lagged(p, index, 3), [(0, 1), (0.6667, 0.5), (1, 1)], .linear)
                circle(0.09375 + CGFloat(index) * 0.375, 0.5, 0.09375, alpha: alpha)
            }

        case .loading:
            let segments: [(CGFloat, CGFloat)] = [(12, 6), (10, 10), (6, 12), (2, 10), (0, 6), (2, 2), (6, 0), (10, 2)]
            viewBox(15) {
                for (index, segment) in segments.enumerated() {
                    fill(1 - 0.8 * lagged(p, index, 8))
                    for (dx, dy) in [(0, 0), (2, 0), (0, 2), (2, 2)] as [(CGFloat, CGFloat)] {
                        context.fill(CGRect(x: segment.0 + dx, y: segment.1 + dy, width: 1, height: 1))
                    }
                }
            }

        case .morph:
            let radius = keyframes(p, [(0, 0.08), (0.5, 0.32), (1, 0.08)])
            let angle = keyframes(p, [(0, 0), (0.5, .pi / 4), (1, .pi / 2)])
            rotated(by: angle, around: CGPoint(x: 0.5, y: 0.5)) {
                roundedRect(CGRect(x: 0.18, y: 0.18, width: 0.64, height: 0.64), radius: radius)
            }

        case .pulse:
            viewBox(16) {
                let eased = Bezier.easeOut(p)
                circle(8, 8, 8 * (0.25 + 0.75 * eased), alpha: 0.4 * (1 - eased))
                circle(8, 8, 2)
            }

        case .ripple:
            for index in 0..<3 {
                let eased = Bezier.easeOut(lagged(p, index, 3))
                let r = (0.5 - 0.04) * eased
                guard r > 0.001 else { continue }
                ring(0.5, 0.5, r, width: 0.08 * eased, alpha: 1 - eased)
            }

        case .slide:
            let far: CGFloat = 0.54
            let stops: [(Double, CGFloat, CGFloat)] = [(0, far, 0), (0.0833, far, far), (0.25, far, far), (0.3333, 0, far),
                                                        (0.5, 0, far), (0.5833, 0, 0), (0.75, 0, 0), (0.8333, far, 0), (1, far, 0)]
            for index in 0..<3 {
                let q = lagged(p, index, 3)
                let x = keyframes(q, stops.map { ($0.0, $0.1) }), y = keyframes(q, stops.map { ($0.0, $0.2) })
                circle(0.06 + 0.17 + x, 0.06 + 0.17 + y, 0.17)
            }

        case .swirl:
            let places: [Int?] = [0, 1, 2, 7, nil, 3, 6, 5, 4]
            let cell: CGFloat = 0.1875, gap: CGFloat = 0.15625, pad: CGFloat = 0.0625
            for (index, place) in places.enumerated() {
                guard let place else { continue }
                let x = pad + CGFloat(index % 3) * (cell + gap), y = pad + CGFloat(index / 3) * (cell + gap)
                roundedRect(CGRect(x: x, y: y, width: cell, height: cell), radius: 0.0625, alpha: 1 - 0.8 * lagged(p, place, 8))
            }

        case .trace:
            viewBox(20) {
                let rect = CGRect(x: 1.25, y: 1.25, width: 17.5, height: 17.5)
                let path = CGPath(roundedRect: rect, cornerWidth: 4, cornerHeight: 4, transform: nil)
                let perimeter = 4 * (17.5 - 8) + 2 * .pi * 4
                context.setLineWidth(2.5)
                stroke(0.2)
                context.addPath(path)
                context.strokePath()
                stroke()
                context.setLineCap(.round)
                context.setLineDash(phase: perimeter * (1 - CGFloat(p)), lengths: [16, perimeter - 16])
                context.addPath(path)
                context.strokePath()
            }

        case .wave:
            let width: CGFloat = 0.12
            for index in 0..<5 {
                let height = keyframes(lagged(p, index, 5), [(0, 0.3), (0.5, 1), (1, 0.3)])
                let x = CGFloat(index) * (1 - width) / 4
                roundedRect(CGRect(x: x, y: 0.5 - height / 2, width: width, height: height), radius: width / 2)
            }
        }
    }
}

// MARK: - Live views

/// One spinner in a view of its own, redrawn on every display refresh (up to
/// 60 Hz) while it's on screen, so it turns smoothly without redrawing what's
/// around it. Holds a still frame with reduced motion.
@MainActor
final class LoaderView: NSView {
    var loader: TabLoader = .dots { didSet { if loader != oldValue { needsDisplay = true } } }
    var color: NSColor = .secondaryLabelColor { didSet { if color != oldValue { needsDisplay = true } } }
    /// The spinner's height; it's centered in the bounds.
    var size: CGFloat = 12 { didSet { if size != oldValue { needsDisplay = true } } }

    private var link: CADisplayLink?
    private var time = CACurrentMediaTime()
    private var occlusionObserver: NSObjectProtocol?

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layerContentsRedrawPolicy = .onSetNeedsDisplay
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }
    override var mouseDownCanMoveWindow: Bool { false }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if let occlusionObserver { NotificationCenter.default.removeObserver(occlusionObserver) }
        occlusionObserver = nil
        if let window {
            occlusionObserver = NotificationCenter.default.addObserver(
                forName: NSWindow.didChangeOcclusionStateNotification, object: window, queue: .main
            ) { [weak self] _ in MainActor.assumeIsolated { self?.updateLink() } }
        }
        updateLink()
    }

    /// Ticks only while the window shows; a display link holds its target,
    /// so it's dropped as soon as the view leaves the window.
    private func updateLink() {
        let visible = window?.occlusionState.contains(.visible) ?? false
        if visible, link == nil {
            let link = displayLink(target: self, selector: #selector(step(_:)))
            link.preferredFrameRateRange = CAFrameRateRange(minimum: 30, maximum: 60, preferred: 60)
            link.add(to: .main, forMode: .common)
            self.link = link
        } else if !visible, let link {
            link.invalidate()
            self.link = nil
        }
    }

    @objc private func step(_ link: CADisplayLink) {
        guard !Motion.reduced else { return }
        time = link.targetTimestamp
        needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        let time = Motion.reduced ? loader.duration * 0.3 : time
        if loader == .dots {
            let phase = CGFloat((-time * 360).truncatingRemainder(dividingBy: 360))
            StatusIndicator.drawDots(in: bounds, color: color, phase: Motion.reduced ? 0 : phase)
        } else {
            LoaderDrawing.draw(loader, in: bounds, size: size, color: color, time: time)
        }
    }
}

/// Live spinners for a view that draws itself: during its draw the view says
/// where spinners go, and right after, matching `LoaderView`s are laid over
/// those spots (and stale ones removed). Views can't be added mid-draw, so
/// the update waits for the end of the run-loop turn.
@MainActor
final class LoaderSpots {
    private weak var host: NSView?
    private var views: [LoaderView] = []
    private var spots: [(rect: NSRect, color: NSColor)] = []
    private var scheduled = false

    init(host: NSView) { self.host = host }

    /// Call at the start of the host's draw.
    func begin() { spots = [] }

    func place(_ rect: NSRect, color: NSColor) {
        spots.append((rect.integral, color))
        schedule()
    }

    /// Call at the end of the host's draw, so spots that went away are removed.
    func end() {
        if spots.count != views.count { schedule() }
    }

    private func schedule() {
        guard !scheduled else { return }
        scheduled = true
        RunLoop.main.perform { [weak self] in MainActor.assumeIsolated { self?.apply() } }
    }

    private func apply() {
        scheduled = false
        guard let host else { return }
        let loader = Preferences.shared.tabLoader
        while views.count > spots.count { views.removeLast().removeFromSuperview() }
        for (index, spot) in spots.enumerated() {
            let view: LoaderView
            if index < views.count {
                view = views[index]
            } else {
                view = LoaderView(frame: spot.rect)
                host.addSubview(view)
                views.append(view)
            }
            if view.frame != spot.rect { view.frame = spot.rect }
            view.loader = loader
            view.color = spot.color
            view.size = min(12, spot.rect.height)
        }
    }
}
