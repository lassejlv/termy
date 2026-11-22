import Foundation

/// A command that ran long and just finished, found by comparing two session lists.
public struct FinishedCommand: Hashable, Sendable {
    public var pane: RemotePane
    /// The pane's title while the command ran, which usually names it.
    public var title: String?
    public var state: RemoteCommandState
    public var duration: TimeInterval

    /// Commands that finished between `old` and `new` after running at least `minimum` seconds.
    public static func between(
        _ old: [RemotePane], _ new: [RemotePane], now: Date = Date(), atLeast minimum: TimeInterval
    ) -> [FinishedCommand] {
        let before = Dictionary(old.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        return new.compactMap { pane in
            guard let state = pane.command, !state.running, let was = before[pane.id], let running = was.command,
                running.running, state.finished > running.finished, let since = running.runningSince
            else { return nil }
            let duration = now.timeIntervalSince(since)
            guard duration >= minimum else { return nil }
            let title = [was.title, pane.title].compactMap { $0 }.first { !$0.isEmpty }
            return FinishedCommand(pane: pane, title: title, state: state, duration: duration)
        }
    }

    /// "cargo build finished" / "cargo build failed", and "Done in 3m 12s on Studio" / "Exit 2 after 12s on Studio".
    public func notification(on computer: String) -> (title: String, body: String) {
        let name = title ?? "A command"
        let time = DurationText.short(duration)
        if state.failed {
            return ("\(name) failed", "Exit \(state.exitCode ?? 0) after \(time) on \(computer)")
        }
        return ("\(name) finished", "Done in \(time) on \(computer)")
    }
}

public enum DurationText {
    /// "45s", "3m 12s", "1h 4m".
    public static func short(_ seconds: TimeInterval) -> String {
        let total = max(0, Int(seconds.rounded()))
        if total < 60 { return "\(total)s" }
        if total < 3600 { return "\(total / 60)m \(total % 60)s" }
        return "\(total / 3600)h \((total % 3600) / 60)m"
    }
}
