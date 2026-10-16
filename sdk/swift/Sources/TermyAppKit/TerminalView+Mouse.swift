#if os(macOS)
import AppKit
import Termy

extension TerminalView {
    private func gridPosition(for event: NSEvent) -> GridPosition {
        let frame = session.frame
        return renderer.position(at: convert(event.locationInWindow, from: nil), cols: frame.cols, rows: frame.rows)
    }

    private func reportingModifiers(_ event: NSEvent) -> KeyModifiers {
        KeyMapping.modifiers(event.modifierFlags).intersection([.control, .alt, .shift])
    }

    /// Option forces local selection even when the program tracks the mouse.
    private func forcesLocalSelection(_ event: NSEvent) -> Bool {
        event.modifierFlags.contains(.option)
    }

    // MARK: Buttons

    public override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        let position = gridPosition(for: event)
        if event.modifierFlags.contains(.command), openLink(at: position) { return }
        if press(.left, at: position, event: event) { return }
        let granularity: TerminalSelection.Granularity = switch event.clickCount {
        case 2: .word
        case 3...: .line
        default: .character
        }
        selection = granularity == .character ? nil : TerminalSelection(anchor: position, granularity: granularity)
        gesture.selectionAnchor = position
        gesture.granularity = granularity
    }

    public override func mouseDragged(with event: NSEvent) {
        let position = gridPosition(for: event)
        if drag(.left, at: position, event: event) { return }
        guard let anchor = gesture.selectionAnchor else { return }
        selection = TerminalSelection(anchor: anchor, head: position, granularity: gesture.granularity)
    }

    public override func mouseUp(with event: NSEvent) {
        if release(.left, at: gridPosition(for: event), event: event) { return }
        gesture.selectionAnchor = nil
        if renderConfig.copyOnSelect, selection != nil { copy(nil) }
    }

    public override func rightMouseDown(with event: NSEvent) {
        if !press(.right, at: gridPosition(for: event), event: event) { super.rightMouseDown(with: event) }
    }

    public override func rightMouseDragged(with event: NSEvent) {
        _ = drag(.right, at: gridPosition(for: event), event: event)
    }

    public override func rightMouseUp(with event: NSEvent) {
        if !release(.right, at: gridPosition(for: event), event: event) { super.rightMouseUp(with: event) }
    }

    public override func otherMouseDown(with event: NSEvent) {
        _ = press(.middle, at: gridPosition(for: event), event: event)
    }

    public override func otherMouseDragged(with event: NSEvent) {
        _ = drag(.middle, at: gridPosition(for: event), event: event)
    }

    public override func otherMouseUp(with event: NSEvent) {
        _ = release(.middle, at: gridPosition(for: event), event: event)
    }

    /// Reports a press to the program. Returns true when the program consumed it.
    private func press(_ button: MouseButton, at position: GridPosition, event: NSEvent) -> Bool {
        guard !forcesLocalSelection(event),
              (try? session.send(MouseInput(.press(button), at: position, modifiers: reportingModifiers(event)))) == true
        else { return false }
        gesture.reportingButton = button
        selection = nil
        return true
    }

    private func drag(_ button: MouseButton, at position: GridPosition, event: NSEvent) -> Bool {
        guard gesture.reportingButton == button else { return false }
        if position != gesture.lastDrag {
            gesture.lastDrag = position
            _ = try? session.send(MouseInput(.drag(button), at: position, modifiers: reportingModifiers(event)))
        }
        return true
    }

    private func release(_ button: MouseButton, at position: GridPosition, event: NSEvent) -> Bool {
        guard gesture.reportingButton == button else { return false }
        gesture.reportingButton = nil
        gesture.lastDrag = nil
        _ = try? session.send(MouseInput(.release(button), at: position, modifiers: reportingModifiers(event)))
        return true
    }

    // MARK: Motion and wheel

    public override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(
            rect: bounds, options: [.mouseMoved, .activeInKeyWindow, .inVisibleRect], owner: self))
    }

    public override func mouseMoved(with event: NSEvent) {
        let position = gridPosition(for: event)
        guard position != gesture.lastMove else { return }
        gesture.lastMove = position
        // Link text is chosen by the program; show where a link really goes.
        let target = linkTarget(at: position)?.absoluteString
        if toolTip != target { toolTip = target }
        _ = try? session.send(MouseInput(.move, at: position, modifiers: reportingModifiers(event)))
    }

    public override func scrollWheel(with event: NSEvent) {
        let position = gridPosition(for: event)
        let multiplier = CGFloat(max(renderConfig.mouseScrollMultiplier, 0.1))
        let delta = event.hasPreciseScrollingDeltas
            ? event.scrollingDeltaY / CGFloat(renderConfig.cellHeight)
            : event.scrollingDeltaY
        gesture.scrollAccumulator += delta * multiplier
        let lines = Int32(gesture.scrollAccumulator.rounded(.towardZero))
        guard lines != 0 else { return }
        gesture.scrollAccumulator -= CGFloat(lines)

        let input = MouseInput(lines > 0 ? .wheelUp : .wheelDown, at: position, modifiers: reportingModifiers(event))
        if (try? session.terminal.encode(input).isEmpty) == false {
            for _ in 0..<abs(lines) { _ = try? session.send(input) }
        } else {
            session.scroll(lines: lines)
        }
    }
}

/// Per-gesture mouse state for `TerminalView`.
struct MouseGestureState {
    var selectionAnchor: GridPosition?
    var granularity: TerminalSelection.Granularity = .character
    var reportingButton: MouseButton?
    var lastDrag: GridPosition?
    var lastMove: GridPosition?
    var scrollAccumulator: CGFloat = 0
}
#endif
