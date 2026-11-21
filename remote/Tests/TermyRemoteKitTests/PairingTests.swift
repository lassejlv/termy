import Foundation
import Testing

@testable import TermyRemoteKit

@Suite struct PairingTests {
    let host = DeviceIdentity()
    let endpoints = [DirectEndpoint(host: "100.81.122.65", port: 47126)]

    private func code() throws -> PairingCode {
        try PairingCode(hostID: UUID(), hostName: "Studio", hostKey: host.publicKey, endpoints: endpoints)
    }

    /// The QR payload as JSON, changed, and put back in a pairing link.
    private func relink(_ code: PairingCode, _ change: (inout [String: Any]) -> Void) throws -> String {
        let raw = try #require(URLComponents(string: try code.text)?.queryItems?.first?.value)
        var b64 = raw.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
        b64 += String(repeating: "=", count: (4 - b64.count % 4) % 4)
        let data = try #require(Data(base64Encoded: b64))
        var json = try #require(try JSONSerialization.jsonObject(with: data) as? [String: Any])
        change(&json)
        return "termy://pair?data=" + (try JSONSerialization.data(withJSONObject: json)).base64EncodedString()
    }

    @Test func qrRoundTrip() throws {
        let code = try code()
        let scanned = try PairingCode.parse(code.text)
        #expect(scanned.version == PairingCode.currentVersion)
        #expect(scanned.endpoints == endpoints)
        #expect(scanned.hostID == code.hostID && scanned.hostKey == code.hostKey && scanned.secret == code.secret)
        #expect(scanned.hostName == "Studio")
    }

    @Test func codesFromOtherVersionsSayWhatToDo() throws {
        let code = try code()
        // A relay-era code, with or without its relay URL.
        #expect(throws: PairingError.relayCode) {
            try PairingCode.parse(try relink(code) { $0["version"] = 3; $0["relayURL"] = "https://relay.termy.sh" })
        }
        #expect(throws: PairingError.relayCode) { try PairingCode.parse(try relink(code) { $0["version"] = 3 }) }
        #expect(throws: PairingError.relayCode) {
            try PairingCode.parse(try relink(code) { $0["relayURL"] = "https://relay.termy.sh" })
        }
        #expect(throws: PairingError.outdatedCode) { try PairingCode.parse(try relink(code) { $0["version"] = 4 }) }
        #expect(throws: PairingError.newerCode) { try PairingCode.parse(try relink(code) { $0["version"] = 6 }) }
        for error in [PairingError.relayCode, .outdatedCode, .newerCode] {
            #expect(error.errorDescription?.contains("Update") == true)
        }
    }

    @Test func expiredDamagedAndUndialableCodesAreRefused() throws {
        let code = try code()
        #expect(throws: PairingError.expired) {
            try PairingCode.parse(code.text, now: code.expiresAt.addingTimeInterval(1))
        }
        #expect(throws: PairingError.invalidCode) { try PairingCode.parse("termy://pair?data=garbage") }
        #expect(throws: PairingError.invalidCode) {
            try PairingCode.parse(try relink(code) { $0["endpoints"] = [["host": "8.8.8.8", "port": 53]] })
        }
        // A Mac's address on its Wi-Fi isn't one a phone dials.
        #expect(throws: PairingError.invalidCode) {
            try PairingCode.parse(try relink(code) { $0["endpoints"] = [["host": "192.168.1.20", "port": 47126]] })
        }
        #expect(throws: PairingError.invalidCode) {
            try PairingCode.parse(try relink(code) { $0["endpoints"] = [] as [Any] })
        }
    }
}
