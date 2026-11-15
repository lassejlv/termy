import SwiftUI
import TermyRemoteKit

enum CommandFormat {
    /// "45s", "3m 12s", "1h 4m".
    static func duration(_ seconds: TimeInterval) -> String {
        let total = max(0, Int(seconds.rounded()))
        if total < 60 { return "\(total)s" }
        if total < 3600 { return "\(total / 60)m \(total % 60)s" }
        return "\(total / 3600)h \((total % 3600) / 60)m"
    }

    static func finished(_ command: TerminalScreen.FinishedCommand) -> String {
        let time = duration(command.duration)
        if let code = command.exitCode, code != 0 { return "Exit \(code) after \(time)" }
        return "Finished in \(time)"
    }
}

/// A pane's command, beside it in the session list: running with a timer, or the exit code it failed with.
struct CommandBadge: View {
    let state: RemoteCommandState?

    var body: some View {
        if let state, state.running {
            HStack(spacing: 4) {
                ProgressView().controlSize(.mini)
                if let since = state.runningSince {
                    Text(since, style: .timer).monospacedDigit()
                }
            }
            .font(.caption).foregroundStyle(.secondary)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(state.runningSince.map { "Running for \(CommandFormat.duration(-$0.timeIntervalSinceNow))" } ?? "Running")
        } else if let state, state.failed {
            Label("Exit \(state.exitCode ?? 0)", systemImage: "xmark.circle.fill")
                .labelStyle(.titleAndIcon).font(.caption.monospacedDigit()).foregroundStyle(.red)
                .accessibilityLabel("Last command failed with exit code \(state.exitCode ?? 0)")
        }
    }
}

/// A dot for the pane strip: blue while a command runs, red after one failed.
struct CommandDot: View {
    let state: RemoteCommandState?

    var body: some View {
        if let state, state.running || state.failed {
            Circle().fill(state.running ? Color.accentColor : .red).frame(width: 6, height: 6)
                .accessibilityLabel(state.running ? "Running" : "Failed")
        }
    }
}
