import CoreGraphics
import Foundation

public enum SplitAxis: Hashable, Sendable {
    /// Children sit side by side.
    case horizontal
    /// Children are stacked top to bottom.
    case vertical
}

public enum Direction: CaseIterable, Hashable, Sendable {
    case left, right, up, down

    public var axis: SplitAxis { self == .left || self == .right ? .horizontal : .vertical }
    /// Whether the new or target side comes after the anchor.
    public var isTrailing: Bool { self == .right || self == .down }
}

/// A binary split layout over pane identifiers. Value type, so every edit
/// returns a new tree and undo or diffing stays trivial.
public indirect enum SplitTree<ID: Hashable & Sendable>: Hashable, Sendable {
    case leaf(ID)
    case split(axis: SplitAxis, ratio: CGFloat, first: SplitTree, second: SplitTree)

    public var leaves: [ID] {
        switch self {
        case .leaf(let id): [id]
        case .split(_, _, let first, let second): first.leaves + second.leaves
        }
    }

    public func contains(_ id: ID) -> Bool { leaves.contains(id) }

    /// Puts `new` next to `anchor` on the side `direction` names.
    public func inserting(_ new: ID, at anchor: ID, direction: Direction, ratio: CGFloat = 0.5) -> SplitTree {
        switch self {
        case .leaf(let id) where id == anchor:
            let pair: (SplitTree, SplitTree) = direction.isTrailing ? (.leaf(id), .leaf(new)) : (.leaf(new), .leaf(id))
            return .split(axis: direction.axis, ratio: ratio, first: pair.0, second: pair.1)
        case .leaf:
            return self
        case .split(let axis, let r, let first, let second):
            return .split(
                axis: axis, ratio: r,
                first: first.inserting(new, at: anchor, direction: direction, ratio: ratio),
                second: second.inserting(new, at: anchor, direction: direction, ratio: ratio))
        }
    }

    /// Removes `id`; its sibling takes the freed space. Nil when the tree was only `id`.
    public func removing(_ id: ID) -> SplitTree? {
        switch self {
        case .leaf(let leaf):
            return leaf == id ? nil : self
        case .split(let axis, let ratio, let first, let second):
            let a = first.removing(id)
            let b = second.removing(id)
            switch (a, b) {
            case (nil, let rest?), (let rest?, nil): return rest
            case (let a?, let b?): return .split(axis: axis, ratio: ratio, first: a, second: b)
            case (nil, nil): return nil
            }
        }
    }

    /// Swaps the positions of two leaves.
    public func swapping(_ a: ID, _ b: ID) -> SplitTree {
        switch self {
        case .leaf(let id): id == a ? .leaf(b) : id == b ? .leaf(a) : self
        case .split(let axis, let ratio, let first, let second):
            .split(axis: axis, ratio: ratio, first: first.swapping(a, b), second: second.swapping(a, b))
        }
    }

    /// Every split back to an even ratio.
    public func equalized() -> SplitTree {
        switch self {
        case .leaf: self
        case .split(let axis, _, let first, let second):
            .split(axis: axis, ratio: 0.5, first: first.equalized(), second: second.equalized())
        }
    }

    // MARK: - Geometry

    /// The frame of every leaf inside `rect`, leaving `gap` between siblings.
    public func frames(in rect: CGRect, gap: CGFloat) -> [ID: CGRect] {
        var result: [ID: CGRect] = [:]
        layout(in: rect, gap: gap) { id, frame in result[id] = frame }
        return result
    }

    /// Every divider: its hit rect, the split's path from the root, and the rect the split spans.
    public func dividers(in rect: CGRect, gap: CGFloat) -> [Divider] {
        var result: [Divider] = []
        collectDividers(in: rect, gap: gap, path: [], into: &result)
        return result
    }

    public struct Divider: Hashable, Sendable {
        public var path: [Bool]
        public var axis: SplitAxis
        public var frame: CGRect
        public var container: CGRect
    }

    /// Sets the ratio of the split at `path` (false = first child, true = second).
    public func settingRatio(_ ratio: CGFloat, at path: [Bool]) -> SplitTree {
        guard case .split(let axis, let r, let first, let second) = self else { return self }
        guard let step = path.first else {
            return .split(axis: axis, ratio: min(max(ratio, 0.08), 0.92), first: first, second: second)
        }
        let rest = Array(path.dropFirst())
        return step
            ? .split(axis: axis, ratio: r, first: first, second: second.settingRatio(ratio, at: rest))
            : .split(axis: axis, ratio: r, first: first.settingRatio(ratio, at: rest), second: second)
    }

    private func layout(in rect: CGRect, gap: CGFloat, visit: (ID, CGRect) -> Void) {
        switch self {
        case .leaf(let id):
            visit(id, rect)
        case .split(let axis, let ratio, let first, let second):
            let (a, b) = Self.divide(rect, axis: axis, ratio: ratio, gap: gap)
            first.layout(in: a, gap: gap, visit: visit)
            second.layout(in: b, gap: gap, visit: visit)
        }
    }

    private func collectDividers(in rect: CGRect, gap: CGFloat, path: [Bool], into result: inout [Divider]) {
        guard case .split(let axis, let ratio, let first, let second) = self else { return }
        let (a, b) = Self.divide(rect, axis: axis, ratio: ratio, gap: gap)
        let grab = max(gap, 6)
        let frame = switch axis {
        case .horizontal: CGRect(x: a.maxX + (gap - grab) / 2, y: rect.minY, width: grab, height: rect.height)
        case .vertical: CGRect(x: rect.minX, y: a.maxY + (gap - grab) / 2, width: rect.width, height: grab)
        }
        result.append(Divider(path: path, axis: axis, frame: frame, container: rect))
        first.collectDividers(in: a, gap: gap, path: path + [false], into: &result)
        second.collectDividers(in: b, gap: gap, path: path + [true], into: &result)
    }

    static func divide(_ rect: CGRect, axis: SplitAxis, ratio: CGFloat, gap: CGFloat) -> (CGRect, CGRect) {
        switch axis {
        case .horizontal:
            let usable = max(rect.width - gap, 0)
            let w = (usable * ratio).rounded()
            return (CGRect(x: rect.minX, y: rect.minY, width: w, height: rect.height),
                    CGRect(x: rect.minX + w + gap, y: rect.minY, width: usable - w, height: rect.height))
        case .vertical:
            let usable = max(rect.height - gap, 0)
            let h = (usable * ratio).rounded()
            return (CGRect(x: rect.minX, y: rect.minY, width: rect.width, height: h),
                    CGRect(x: rect.minX, y: rect.minY + h + gap, width: rect.width, height: usable - h))
        }
    }
}

// MARK: - Mapping and persistence

extension SplitTree {
    /// The same layout over other identifiers; leaves `transform` drops are
    /// removed as if closed. Nil when every leaf was dropped.
    public func compactMap<T: Hashable & Sendable>(_ transform: (ID) -> T?) -> SplitTree<T>? {
        switch self {
        case .leaf(let id):
            return transform(id).map { .leaf($0) }
        case .split(let axis, let ratio, let first, let second):
            let a = first.compactMap(transform)
            let b = second.compactMap(transform)
            switch (a, b) {
            case (let a?, let b?): return .split(axis: axis, ratio: ratio, first: a, second: b)
            case (let only?, nil), (nil, let only?): return only
            case (nil, nil): return nil
            }
        }
    }
}

extension SplitAxis: Codable {}

extension SplitTree: Codable where ID: Codable {
    private enum CodingKeys: String, CodingKey { case leaf, axis, ratio, first, second }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        if let id = try container.decodeIfPresent(ID.self, forKey: .leaf) {
            self = .leaf(id)
        } else {
            self = .split(
                axis: try container.decode(SplitAxis.self, forKey: .axis),
                ratio: try container.decode(CGFloat.self, forKey: .ratio),
                first: try container.decode(SplitTree.self, forKey: .first),
                second: try container.decode(SplitTree.self, forKey: .second))
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .leaf(let id):
            try container.encode(id, forKey: .leaf)
        case .split(let axis, let ratio, let first, let second):
            try container.encode(axis, forKey: .axis)
            try container.encode(ratio, forKey: .ratio)
            try container.encode(first, forKey: .first)
            try container.encode(second, forKey: .second)
        }
    }
}

// MARK: - Navigation

public enum SplitNavigation {
    /// The pane whose frame is the nearest neighbour of `from` in `direction`
    /// (frames in a top-left origin space), preferring the largest overlap.
    public static func neighbor<ID: Hashable>(of from: ID, direction: Direction, frames: [ID: CGRect]) -> ID? {
        guard let origin = frames[from] else { return nil }
        let candidates = frames.filter { id, frame in
            guard id != from else { return false }
            switch direction {
            case .left: return frame.maxX <= origin.minX + 1 && overlap(frame.minY...frame.maxY, origin.minY...origin.maxY) > 0
            case .right: return frame.minX >= origin.maxX - 1 && overlap(frame.minY...frame.maxY, origin.minY...origin.maxY) > 0
            case .up: return frame.maxY <= origin.minY + 1 && overlap(frame.minX...frame.maxX, origin.minX...origin.maxX) > 0
            case .down: return frame.minY >= origin.maxY - 1 && overlap(frame.minX...frame.maxX, origin.minX...origin.maxX) > 0
            }
        }
        return candidates.min { a, b in
            let sa = score(a.value, origin, direction), sb = score(b.value, origin, direction)
            if sa != sb { return sa < sb }
            // Equal candidates: the topmost, then leftmost, so the answer never depends on dictionary order.
            return (a.value.minY, a.value.minX) < (b.value.minY, b.value.minX)
        }?.key
    }

    private static func score(_ frame: CGRect, _ origin: CGRect, _ direction: Direction) -> CGFloat {
        let distance: CGFloat
        let shared: CGFloat
        switch direction {
        case .left: distance = origin.minX - frame.maxX; shared = overlap(frame.minY...frame.maxY, origin.minY...origin.maxY)
        case .right: distance = frame.minX - origin.maxX; shared = overlap(frame.minY...frame.maxY, origin.minY...origin.maxY)
        case .up: distance = origin.minY - frame.maxY; shared = overlap(frame.minX...frame.maxX, origin.minX...origin.maxX)
        case .down: distance = frame.minY - origin.maxY; shared = overlap(frame.minX...frame.maxX, origin.minX...origin.maxX)
        }
        return distance * 1000 - shared
    }

    private static func overlap(_ a: ClosedRange<CGFloat>, _ b: ClosedRange<CGFloat>) -> CGFloat {
        max(0, min(a.upperBound, b.upperBound) - max(a.lowerBound, b.lowerBound))
    }

    /// "Best fit": split along the longer side so new panes stay close to square.
    public static func bestFitDirection(for frame: CGRect) -> Direction {
        frame.width >= frame.height * 1.25 ? .right : .down
    }
}
