import Foundation

// MARK: - Accounts and devices

public enum DeviceKind: String, Codable, Sendable {
    /// A computer running `termy-remote`, whose sessions others can open.
    case host
    /// A phone that opens sessions.
    case client
}

public struct RelayUser: Codable, Hashable, Sendable {
    public var id: UUID
    public var email: String
}

public struct RelayDevice: Codable, Hashable, Sendable, Identifiable {
    public var id: UUID
    public var name: String
    public var kind: DeviceKind
    /// Base64 X25519 public key.
    public var publicKey: String
    public var online: Bool
    public var createdAt: Date
    public var lastSeenAt: Date?

    public var publicKeyData: Data? { Data(base64Encoded: publicKey) }
    public var fingerprint: String { publicKeyData.map(Fingerprint.of) ?? "?" }
}

public struct DeviceRegistration: Codable, Sendable {
    public var name: String
    public var kind: DeviceKind
    public var publicKey: String

    public init(name: String, kind: DeviceKind, publicKey: Data) {
        self.name = name
        self.kind = kind
        self.publicKey = publicKey.base64EncodedString()
    }
}

public struct AuthResponse: Codable, Sendable {
    public var token: String
    public var user: RelayUser
    public var device: RelayDevice
}

public struct MeResponse: Codable, Sendable {
    public var user: RelayUser
    public var device: RelayDevice
}

public struct RelayAPIError: Error, LocalizedError, Sendable {
    public var status: Int
    public var message: String
    public var errorDescription: String? { message }
}

/// The relay's account API. Passwords go to the relay (over TLS) to sign in;
/// nothing a terminal shows does.
public struct RelayAPI: Sendable {
    public var baseURL: URL
    public var token: String?

    public init(baseURL: URL, token: String? = nil) {
        self.baseURL = baseURL
        self.token = token
    }

    /// Deployed on Unkey (project termy, app relay; also termy-relay-teamlasse.unkey.app).
    public static let defaultURL = URL(string: "https://relay.termy.sh")!

    private struct Credentials: Encodable {
        var email: String
        var password: String
        var device: DeviceRegistration
    }

    public func signUp(email: String, password: String, device: DeviceRegistration) async throws -> AuthResponse {
        try await request("POST", "v1/auth/signup", body: Credentials(email: email, password: password, device: device))
    }

    public func logIn(email: String, password: String, device: DeviceRegistration) async throws -> AuthResponse {
        try await request("POST", "v1/auth/login", body: Credentials(email: email, password: password, device: device))
    }

    /// Ends this device's session and removes the device.
    public func logOut() async throws {
        let _: Empty = try await request("POST", "v1/auth/logout")
    }

    public func me() async throws -> MeResponse {
        try await request("GET", "v1/me")
    }

    /// Deletes the account, its devices and sessions.
    public func deleteAccount() async throws {
        let _: Empty = try await request("DELETE", "v1/me")
    }

    public func devices() async throws -> [RelayDevice] {
        try await request("GET", "v1/devices")
    }

    public func removeDevice(_ id: UUID) async throws {
        let _: Empty = try await request("DELETE", "v1/devices/\(id.uuidString)")
    }

    public var relaySocketURL: URL {
        var components = URLComponents(url: baseURL.appendingPathComponent("v1/relay"), resolvingAgainstBaseURL: false)!
        components.scheme = components.scheme == "http" ? "ws" : "wss"
        return components.url!
    }

    struct Empty: Codable {}
    private struct ErrorBody: Decodable { var reason: String }

    static let decoder: JSONDecoder = {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .custom { decoder in
            let text = try decoder.singleValueContainer().decode(String.self)
            let plain = ISO8601DateFormatter()
            let fractional = ISO8601DateFormatter()
            fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
            guard let date = plain.date(from: text) ?? fractional.date(from: text) else {
                throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "bad date \(text)"))
            }
            return date
        }
        return decoder
    }()

    private func request<R: Decodable>(_ method: String, _ path: String) async throws -> R {
        try await request(method, path, body: Optional<Empty>.none)
    }

    private func request<B: Encodable, R: Decodable>(_ method: String, _ path: String, body: B?) async throws -> R {
        var request = URLRequest(url: baseURL.appendingPathComponent(path))
        request.httpMethod = method
        request.timeoutInterval = 20
        if let token { request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization") }
        if let body {
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = try JSONEncoder().encode(body)
        }
        let (data, response) = try await URLSession.shared.data(for: request)
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        guard (200 ..< 300).contains(status) else {
            let reason = (try? JSONDecoder().decode(ErrorBody.self, from: data))?.reason
            throw RelayAPIError(status: status, message: reason ?? HTTPURLResponse.localizedString(forStatusCode: status))
        }
        if R.self == Empty.self || data.isEmpty { return try Self.decoder.decode(R.self, from: Data("{}".utf8)) }
        return try Self.decoder.decode(R.self, from: data)
    }
}

// MARK: - Relay socket

/// Messages on the relay socket. Binary frames carry sealed packets:
/// one kind byte, the 16-byte peer device ID (the recipient when sent, the
/// sender when received), then the packet. Text frames are relay events.
public enum RelayWire {
    public static let dataKind: UInt8 = 1

    public static func encode(peer: UUID, packet: Data) -> Data {
        var out = Data([dataKind])
        withUnsafeBytes(of: peer.uuid) { out.append(contentsOf: $0) }
        out += packet
        return out
    }

    public static func decode(_ frame: Data) -> (peer: UUID, packet: Data)? {
        guard frame.count > 17, frame.first == dataKind else { return nil }
        let bytes = [UInt8](frame[frame.startIndex + 1 ..< frame.startIndex + 17])
        let uuid = UUID(uuid: (
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
        ))
        return (uuid, Data(frame[(frame.startIndex + 17)...]))
    }
}

public enum RelayEvent: Sendable {
    /// The relay accepted the socket for this device.
    case ready(deviceID: UUID)
    case packet(from: UUID, Data)
    /// A device of the same account came online or went away.
    case presence(deviceID: UUID, online: Bool)
    /// A packet couldn't be delivered because the device is offline.
    case undeliverable(deviceID: UUID)
    case closed(Error?)
}

/// One open relay socket. Read `events` until `.closed`, then make a new one.
public final class RelayConnection: NSObject, @unchecked Sendable, URLSessionWebSocketDelegate {
    public let events: AsyncStream<RelayEvent>
    private let continuation: AsyncStream<RelayEvent>.Continuation
    private var task: URLSessionWebSocketTask?
    private var session: URLSession?
    private var pingTimer: DispatchSourceTimer?
    private let lock = NSLock()
    private var finished = false

    public init(api: RelayAPI) {
        (events, continuation) = AsyncStream.makeStream(of: RelayEvent.self, bufferingPolicy: .unbounded)
        super.init()
        var request = URLRequest(url: api.relaySocketURL)
        if let token = api.token { request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization") }
        let session = URLSession(configuration: .default, delegate: self, delegateQueue: nil)
        let task = session.webSocketTask(with: request)
        task.maximumMessageSize = 16 * 1024 * 1024
        self.session = session
        self.task = task
        task.resume()
        receive()
        startPing()
    }

    public func send(_ packet: Data, to peer: UUID) {
        task?.send(.data(RelayWire.encode(peer: peer, packet: packet))) { [weak self] error in
            if let error { self?.finish(error) }
        }
    }

    public func close() {
        task?.cancel(with: .normalClosure, reason: nil)
        finish(nil)
    }

    private func receive() {
        task?.receive { [weak self] result in
            guard let self else { return }
            switch result {
            case .failure(let error):
                self.finish(error)
            case .success(.data(let data)):
                if let (peer, packet) = RelayWire.decode(data) { self.continuation.yield(.packet(from: peer, packet)) }
                self.receive()
            case .success(.string(let text)):
                if let event = Self.parseEvent(text) { self.continuation.yield(event) }
                self.receive()
            case .success:
                self.receive()
            }
        }
    }

    private struct EventBody: Decodable {
        var type: String
        var device: UUID?
        var online: Bool?
    }

    static func parseEvent(_ text: String) -> RelayEvent? {
        guard let body = try? JSONDecoder().decode(EventBody.self, from: Data(text.utf8)), let device = body.device else {
            return nil
        }
        switch body.type {
        case "ready": return .ready(deviceID: device)
        case "presence": return .presence(deviceID: device, online: body.online ?? false)
        case "undeliverable": return .undeliverable(deviceID: device)
        default: return nil
        }
    }

    private func startPing() {
        let timer = DispatchSource.makeTimerSource(queue: .global(qos: .utility))
        timer.schedule(deadline: .now() + 20, repeating: 20)
        timer.setEventHandler { [weak self] in
            guard let self else { return }
            self.task?.sendPing { [weak self] error in
                if let error { self?.finish(error) }
            }
        }
        timer.resume()
        pingTimer = timer
    }

    private func finish(_ error: Error?) {
        lock.lock()
        guard !finished else { return lock.unlock() }
        finished = true
        lock.unlock()
        pingTimer?.cancel()
        task?.cancel()
        session?.invalidateAndCancel()
        continuation.yield(.closed(error))
        continuation.finish()
    }

    public func urlSession(
        _ session: URLSession, webSocketTask: URLSessionWebSocketTask,
        didCloseWith closeCode: URLSessionWebSocketTask.CloseCode, reason: Data?
    ) {
        finish(nil)
    }

    public func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        finish(error)
    }
}
