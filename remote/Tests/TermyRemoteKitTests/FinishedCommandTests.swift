import Foundation
import Testing

@testable import TermyRemoteKit

struct FinishedCommandTests {
    @Test func finishedCommandsAreFoundAndWorded() {
        let now = Date()
        func pane(_ id: String, _ title: String?, _ state: RemoteCommandState?) -> RemotePane {
            RemotePane(id: id, title: title, workingDirectory: nil, exited: false, command: state)
        }
        let old = [
            pane("long", "cargo build", .init(running: true, finished: 3, exitCode: 0, runningSince: now.addingTimeInterval(-192))),
            pane("quick", "ls", .init(running: true, finished: 0, exitCode: nil, runningSince: now.addingTimeInterval(-2))),
            pane("idle", "zsh", .init(running: false, finished: 1, exitCode: 0)),
        ]
        let new = [
            pane("long", "~/dev", .init(running: false, finished: 4, exitCode: 2)),
            pane("quick", "zsh", .init(running: false, finished: 1, exitCode: 0)),
            pane("idle", "zsh", .init(running: false, finished: 1, exitCode: 0)),
        ]
        let done = FinishedCommand.between(old, new, now: now, atLeast: 10)
        #expect(done.map(\.pane.id) == ["long"])
        #expect(done.first?.title == "cargo build")
        let text = done.first?.notification(on: "Studio")
        #expect(text?.title == "cargo build failed")
        #expect(text?.body == "Exit 2 after 3m 12s on Studio")
        #expect(FinishedCommand.between(new, new, now: now, atLeast: 10).isEmpty)
        #expect(DurationText.short(45) == "45s" && DurationText.short(3840) == "1h 4m")
    }
}
