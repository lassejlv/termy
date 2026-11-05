import Foundation

// MARK: - Devices and pairing

public enum DeviceKind: String, Codable, Sendable {
    /// A computer running a Termy remote host, whose sessions others can open.
    case host
    /// A phone that opens sessions.
    case client
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

public struct DeviceCredentials: Codable, Sendable {
    public var relayURL: URL
    public var token: String
    public var device: RelayDevice
    public init(relayURL: URL, token: String, device: RelayDevice) {
        self.relayURL = relayURL
        self.token = token
        self.device = device
    }
    public var api: RelayAPI { RelayAPI(baseURL: relayURL, token: token) }
}
public struct DeviceRegistrationResponse: Codable, Sendable {
    public var token: String
    public var device: RelayDevice
}
public struct PairingStatus: Codable, Sendable {
    public var id: UUID
    public var expiresAt: Date
    public var host: RelayDevice
    public var client: RelayDevice?
    public var initPacket: Data?
    public var proof: Data?
    public var responsePacket: Data?
}
public struct RelayAPIError: Error, LocalizedError, Sendable {
    public var status: Int
    public var message: String
    public var errorDescription: String? { message }
}
public struct RelayAPI: Sendable {
    public var baseURL: URL
    public var token: String?
    public static let defaultURL = URL(string: "https://relay.termy.sh")!
    public init(baseURL: URL, token: String? = nil) {
        self.baseURL = baseURL
        self.token = token
    }
    public func register(_ device: DeviceRegistration) async throws -> DeviceCredentials {
        let r: DeviceRegistrationResponse = try await request("POST", "v2/devices", body: device)
        return DeviceCredentials(relayURL: baseURL, token: r.token, device: r.device)
    }
    public func devices() async throws -> [RelayDevice] { try await request("GET", "v2/devices") }
    public func removeDevice(_ id: UUID) async throws {
        let _: Empty = try await request("DELETE", "v2/links/\(id)")
    }
    public func deleteRegistration() async throws { let _: Empty = try await request("DELETE", "v2/device") }
    public func createPairing() async throws -> PairingStatus { try await request("POST", "v2/pairings") }
    public func pairing(_ id: UUID) async throws -> PairingStatus { try await request("GET", "v2/pairings/\(id)") }
    public func cancelPairing(_ id: UUID) async throws {
        let _: Empty = try await request("DELETE", "v2/pairings/\(id)")
    }
    private struct Claim: Encodable {
        var initPacket: Data
        var proof: Data
    }
    public func claim(_ id: UUID, initPacket: Data, proof: Data) async throws {
        let _: Empty = try await request(
            "POST", "v2/pairings/\(id)/claim", body: Claim(initPacket: initPacket, proof: proof))
    }
    private struct Approval: Encodable {
        var clientId: UUID
        var initPacket: Data
        var responsePacket: Data
    }
    public func approve(_ id: UUID, client: UUID, initPacket: Data, response: Data) async throws {
        let _: Empty = try await request(
            "POST", "v2/pairings/\(id)/approve",
            body: Approval(clientId: client, initPacket: initPacket, responsePacket: response))
    }
    public var relaySocketURL: URL {
        var parts = URLComponents(url: baseURL.appendingPathComponent("v2/relay"), resolvingAgainstBaseURL: false)!
        parts.scheme = parts.scheme == "http" ? "ws" : "wss"
        return parts.url!
    }
    private struct Empty: Codable {}
    private struct ErrorBody: Decodable { var reason: String }
    static let decoder: JSONDecoder = {
        let d = JSONDecoder()
        d.dateDecodingStrategy = .iso8601
        return d
    }()
    private func request<R: Decodable>(_ method: String, _ path: String) async throws -> R {
        try await request(method, path, body: Optional<Empty>.none)
    }
    private func request<B: Encodable, R: Decodable>(_ method: String, _ path: String, body: B?) async throws -> R {
        var r = URLRequest(url: baseURL.appendingPathComponent(path))
        r.httpMethod = method
        r.timeoutInterval = 20
        if let token { r.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization") }
        if let body {
            r.setValue("application/json", forHTTPHeaderField: "Content-Type")
            r.httpBody = try JSONEncoder().encode(body)
        }
        let (data, response) = try await URLSession.shared.data(for: r)
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        guard (200..<300).contains(status) else {
            let reason = (try? JSONDecoder().decode(ErrorBody.self, from: data))?.reason
            throw RelayAPIError(
                status: status, message: reason ?? HTTPURLResponse.localizedString(forStatusCode: status))
        }
        return try Self.decoder.decode(R.self, from: data.isEmpty ? Data("{}".utf8) : data)
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
        let bytes = [UInt8](frame[frame.startIndex + 1..<frame.startIndex + 17])
        let uuid = UUID(
            uuid: (
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
    /// A paired device came online or went away.
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
        guard let body = try? JSONDecoder().decode(EventBody.self, from: Data(text.utf8)), let device = body.device
        else {
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
