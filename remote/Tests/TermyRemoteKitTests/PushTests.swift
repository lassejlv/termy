import CryptoKit
import Foundation
import Testing

@testable import TermyRemoteKit

struct PushTests {
    @Test func bothEndsDeriveTheSameKeyAndOnlyTheyCanOpen() throws {
        let mac = DeviceIdentity()
        let phone = DeviceIdentity()
        let host = UUID()
        let client = UUID()
        let macKey = try PushSeal.key(identity: mac, peerPublicKey: phone.publicKey, host: host, client: client)
        let phoneKey = try PushSeal.key(identity: phone, peerPublicKey: mac.publicKey, host: host, client: client)
        #expect(macKey == phoneKey)

        let payload = PushPayload(
            title: "cargo build finished", body: "Done in 3m 12s on Studio", pane: "p1",
            sentAt: Date(timeIntervalSince1970: 1_800_000_000))
        let sealed = try PushSeal.seal(payload, key: macKey)
        #expect(sealed.count <= PushSeal.maxSealedBytes)
        #expect(try PushSeal.open(sealed, key: phoneKey) == payload)

        // Another phone, or the same pair for another computer, can't.
        let other = try PushSeal.key(identity: DeviceIdentity(), peerPublicKey: mac.publicKey, host: host, client: client)
        #expect(throws: (any Error).self) { try PushSeal.open(sealed, key: other) }
        let otherHost = try PushSeal.key(identity: phone, peerPublicKey: mac.publicKey, host: UUID(), client: client)
        #expect(throws: (any Error).self) { try PushSeal.open(sealed, key: otherHost) }
        var tampered = sealed
        tampered[tampered.count - 1] ^= 1
        #expect(throws: (any Error).self) { try PushSeal.open(tampered, key: phoneKey) }
    }

    @Test func lengthsArePaddedAndLongTextIsCut() throws {
        let key = SymmetricKey(size: .bits256)
        let short = try PushSeal.seal(PushPayload(title: "a", body: "b", pane: nil), key: key)
        let longer = try PushSeal.seal(PushPayload(title: "ab", body: "bc", pane: nil), key: key)
        #expect(short.count == longer.count)
        let huge = PushPayload(title: String(repeating: "t", count: 500), body: String(repeating: "界", count: 500), pane: String(repeating: "p", count: 500))
        let sealed = try PushSeal.seal(huge, key: key)
        #expect(sealed.count <= PushSeal.maxSealedBytes)
        let opened = try PushSeal.open(sealed, key: key)
        #expect(opened.title.count == 80 && opened.body.count == 180 && opened.pane?.count == 128)
    }

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
