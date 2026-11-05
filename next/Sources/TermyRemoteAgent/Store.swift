import Foundation
import TermyRemoteKit

/// What `termy-remote` keeps in `~/.config/termy/remote` (or
/// `$TERMY_REMOTE_HOME`): this computer's device key, the relay session, and
/// the phones it trusts. Every file is private to the user.
struct AgentStore {
    struct Credentials: Codable {
        var relayURL: URL
        var token: String
        var email: String
        var deviceID: UUID
    }

    let root: URL

    init() {
        if let home = ProcessInfo.processInfo.environment["TERMY_REMOTE_HOME"] {
            root = URL(fileURLWithPath: home, isDirectory: true)
        } else {
            let config = ProcessInfo.processInfo.environment["XDG_CONFIG_HOME"].map { URL(fileURLWithPath: $0) }
                ?? FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".config")
            root = config.appendingPathComponent("termy/remote", isDirectory: true)
        }
    }

    private var identityURL: URL { root.appendingPathComponent("identity.key") }
    private var credentialsURL: URL { root.appendingPathComponent("credentials.json") }
    var trustURL: URL { root.appendingPathComponent("trusted-phones.json") }

    private func ensureRoot() throws {
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: root.path)
    }

    private func writePrivate(_ data: Data, to url: URL) throws {
        try ensureRoot()
        try data.write(to: url, options: .atomic)
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
    }

    /// This computer's key, made on first use.
    func identity() throws -> DeviceIdentity {
        if let raw = try? Data(contentsOf: identityURL) { return try DeviceIdentity(rawRepresentation: raw) }
        let identity = DeviceIdentity()
        try writePrivate(identity.rawRepresentation, to: identityURL)
        return identity
    }

    func credentials() -> Credentials? {
        guard let data = try? Data(contentsOf: credentialsURL) else { return nil }
        return try? JSONDecoder().decode(Credentials.self, from: data)
    }

    func save(_ credentials: Credentials) throws {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        try writePrivate(try encoder.encode(credentials), to: credentialsURL)
    }

    func clearCredentials() {
        try? FileManager.default.removeItem(at: credentialsURL)
    }
}
