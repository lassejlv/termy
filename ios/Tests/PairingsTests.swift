import TermyRemoteKit
import XCTest

@testable import Termy

final class PairingsTests: XCTestCase {
    /// What a build with a relay saved, in its shape.
    private struct OldProfile: Encodable {
        struct Credentials: Encodable {
            var relayURL: URL
            var token: String
            var device: PairedDevice
        }
        var credentials: Credentials
        var pins: [UUID: Data]
        var hosts: [PairedDevice]
        var endpoints: [UUID: [DirectEndpoint]]?
    }

    func testRelayEraPairingsKeepDirectMacsAndListTheOthersToPairAgain() throws {
        let key = Data(repeating: 7, count: 32)
        let phone = PairedDevice(id: UUID(), name: "iPhone", kind: .client, publicKey: key)
        let studio = PairedDevice(id: UUID(), name: "Studio", kind: .host, publicKey: key)
        var laptop = PairedDevice(id: UUID(), name: "Laptop", kind: .host, publicKey: key)
        laptop.online = true
        let endpoints = [DirectEndpoint(host: "100.81.122.65", port: 47126)]
        let old: [String: OldProfile] = [
            "direct": OldProfile(
                credentials: .init(relayURL: URL(string: "termy-direct://local")!, token: "", device: phone),
                pins: [studio.id: key], hosts: [studio], endpoints: [studio.id: endpoints]),
            "https://relay.termy.sh": OldProfile(
                credentials: .init(relayURL: URL(string: "https://relay.termy.sh")!, token: "secret", device: phone),
                pins: [laptop.id: key], hosts: [laptop], endpoints: nil),
        ]
        let profiles = try JSONDecoder().decode(
            [String: Pairings.LegacyProfile].self, from: JSONEncoder().encode(old))
        let pairings = Pairings.migrating(profiles)
        XCTAssertEqual(pairings.clientID, phone.id, "The Macs paired directly know the phone by this ID")
        XCTAssertEqual(pairings.pins, [studio.id: key])
        XCTAssertEqual(pairings.hosts.map(\.id), [studio.id])
        XCTAssertEqual(pairings.endpoints[studio.id], endpoints)
        XCTAssertEqual(pairings.relayPaired.map(\.name), ["Laptop"])
        XCTAssertEqual(pairings.relayPaired.first?.online, false)
    }
}
