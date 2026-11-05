import Foundation
import Security

/// Both clients keep private identity and relay credentials in the device's Keychain.
/// No account, password, or shared filesystem credentials are required.
public struct DeviceStore: Sendable {
    private let service: String
    public init(service: String) { self.service = service }
    public func load<T: Decodable>(_ type: T.Type, key: String) throws -> T? {
        guard let data = try read(key) else { return nil }
        return try JSONDecoder().decode(type, from: data)
    }
    public func save<T: Encodable>(_ value: T, key: String) throws {
        try write(JSONEncoder().encode(value), key: key)
    }
    public func identity() throws -> DeviceIdentity {
        if let raw = try read("identity") { return try DeviceIdentity(rawRepresentation: raw) }
        let identity = DeviceIdentity()
        try write(identity.rawRepresentation, key: "identity")
        return identity
    }
    private func read(_ key: String) throws -> Data? {
        var q = query(key)
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: AnyObject?
        let status = SecItemCopyMatching(q as CFDictionary, &result)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess, let data = result as? Data else { throw PairingError.storage }
        return data
    }
    private func query(_ key: String) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service,
            kSecAttrAccount as String: key,
        ]
    }
    private func write(_ data: Data, key: String) throws {
        let q = query(key)
        let status = SecItemUpdate(q as CFDictionary, [kSecValueData as String: data] as CFDictionary)
        if status == errSecItemNotFound {
            var item = q
            item[kSecValueData as String] = data
            item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
            guard SecItemAdd(item as CFDictionary, nil) == errSecSuccess else { throw PairingError.storage }
            return
        }
        guard status == errSecSuccess else { throw PairingError.storage }
    }
}
