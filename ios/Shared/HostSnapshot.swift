import Foundation
import Security

/// What the widget shows: the paired computers as the app last saw them.
///
/// Readable names stay in Keychain, as the app's own pairing data does; the
/// snapshot sits in a Keychain group only the app and its widget share.
struct HostSnapshot: Codable, Equatable, Sendable {
    struct Host: Codable, Equatable, Identifiable, Sendable {
        var id: UUID
        var name: String
        var online: Bool
        /// Live panes, when the app was connected to it.
        var panes: Int?
        /// Panes running a command, from shell integration.
        var running: Int?
    }

    var hosts: [Host]
    var updatedAt: Date

    static let empty = HostSnapshot(hosts: [], updatedAt: .distantPast)

    var onlineCount: Int { hosts.filter(\.online).count }
}

/// Keychain items the app shares with its extensions (the widget and the
/// notification decrypter), in a group no other app can read.
enum SharedKeychain {
    private static let service = "sh.termy.shared"

    /// `$(AppIdentifierPrefix)sh.termy.shared`, from Info.plist; nil when unsigned.
    private static var accessGroup: String? {
        guard let group = Bundle.main.object(forInfoDictionaryKey: "TermyKeychainGroup") as? String,
            // Unsigned builds leave the team prefix out, and Keychain refuses such a group.
            !group.hasPrefix("$("), group != service
        else { return nil }
        return group
    }

    private static func query(_ account: String) -> [String: Any] {
        var query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        if let accessGroup { query[kSecAttrAccessGroup as String] = accessGroup }
        return query
    }

    static func load(_ account: String) -> Data? {
        var query = query(account)
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: AnyObject?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess else { return nil }
        return result as? Data
    }

    /// False when Keychain refused it.
    @discardableResult
    static func save(_ data: Data, account: String) -> Bool {
        let query = query(account)
        let status = SecItemUpdate(query as CFDictionary, [kSecValueData as String: data] as CFDictionary)
        if status == errSecItemNotFound {
            var item = query
            item[kSecValueData as String] = data
            // Extensions run while the phone is locked, after the first unlock.
            item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
            return SecItemAdd(item as CFDictionary, nil) == errSecSuccess
        }
        return status == errSecSuccess
    }

    static func delete(_ account: String) { SecItemDelete(query(account) as CFDictionary) }
}

enum HostSnapshotStore {
    private static let account = "host-snapshot"

    static func load() -> HostSnapshot {
        SharedKeychain.load(account).flatMap { try? JSONDecoder().decode(HostSnapshot.self, from: $0) } ?? .empty
    }

    /// Saves the snapshot. False when Keychain refused it.
    @discardableResult
    static func save(_ snapshot: HostSnapshot) -> Bool {
        guard let data = try? JSONEncoder().encode(snapshot) else { return false }
        return SharedKeychain.save(data, account: account)
    }

    static func clear() { SharedKeychain.delete(account) }
}

/// `termy://` links the widget, quick actions and Spotlight open.
///
/// Links only navigate. They never type or run anything: any app or web page
/// can open a URL, so commands come only from Shortcuts the user made.
enum TermyLink {
    static let scheme = "termy"

    static func open(host: UUID, pane: String? = nil) -> URL {
        var components = URLComponents()
        components.scheme = scheme
        components.host = "open"
        components.queryItems = [URLQueryItem(name: "host", value: host.uuidString)]
        if let pane { components.queryItems?.append(URLQueryItem(name: "pane", value: pane)) }
        return components.url!
    }

    static let pair = URL(string: "termy://pair")!

    enum Destination: Equatable {
        case host(UUID, pane: String?)
        case pair
    }

    static func parse(_ url: URL) -> Destination? {
        guard url.scheme == scheme, let components = URLComponents(url: url, resolvingAgainstBaseURL: false) else {
            return nil
        }
        switch components.host {
        case "pair": return .pair
        case "open":
            let items = components.queryItems ?? []
            guard let raw = items.first(where: { $0.name == "host" })?.value, let id = UUID(uuidString: raw) else {
                return nil
            }
            let pane = items.first(where: { $0.name == "pane" })?.value.flatMap { $0.count <= 128 ? $0 : nil }
            return .host(id, pane: pane)
        default: return nil
        }
    }
}
