import Foundation
import Network

// How a phone reaches a Mac: over plain TCP to the Mac itself, on their
// tailnet (Tailscale). The packets are sealed `SecureChannel` packets, so the
// network in between is trusted with nothing: the Mac answers only a handshake
// naming a key pinned on it, and the phone only a Mac that proves the key it pinned.

// MARK: - Addresses

public struct IPv4: Hashable, Sendable {
    public let value: UInt32

    public init(value: UInt32) { self.value = value }

    /// A dotted quad and nothing else: no names to look up, no shorthand forms.
    public init?(_ text: String) {
        let parts = text.split(separator: ".", omittingEmptySubsequences: false)
        guard parts.count == 4 else { return nil }
        var value: UInt32 = 0
        for part in parts {
            guard (1...3).contains(part.count), part.allSatisfy({ $0.isASCII && $0.isNumber }), let octet = UInt8(part)
            else { return nil }
            value = value << 8 | UInt32(octet)
        }
        self.value = value
    }

    public var text: String { "\(value >> 24).\(value >> 16 & 255).\(value >> 8 & 255).\(value & 255)" }

    /// Tailscale gives its devices addresses in 100.64.0.0/10.
    public var isTailnet: Bool { value & 0xFFC0_0000 == 0x6440_0000 }
    public var isLoopback: Bool { value >> 24 == 127 }
}

/// One IPv4 address this device has.
public struct NetworkInterface: Hashable, Sendable {
    public var name: String
    public var address: IPv4

    public init(name: String, address: IPv4) {
        self.name = name
        self.address = address
    }

    /// Tailscale's tunnel, on a Mac and an iPhone alike.
    public var isTailnet: Bool { name.hasPrefix("utun") && address.isTailnet }

    /// The addresses of every interface that is up.
    public static func current() -> [NetworkInterface] {
        var list: UnsafeMutablePointer<ifaddrs>?
        guard getifaddrs(&list) == 0 else { return [] }
        defer { freeifaddrs(list) }
        var result: [NetworkInterface] = []
        var cursor = list
        while let entry = cursor {
            cursor = entry.pointee.ifa_next
            let flags = Int32(entry.pointee.ifa_flags)
            guard flags & IFF_UP != 0, flags & IFF_RUNNING != 0, let address = entry.pointee.ifa_addr,
                address.pointee.sa_family == sa_family_t(AF_INET)
            else { continue }
            let word = address.withMemoryRebound(to: sockaddr_in.self, capacity: 1) {
                UInt32(bigEndian: $0.pointee.sin_addr.s_addr)
            }
            result.append(NetworkInterface(name: String(cString: entry.pointee.ifa_name), address: IPv4(value: word)))
        }
        return result
    }

    /// Whether this device is on a tailnet right now.
    public static var onTailnet: Bool { current().contains(where: \.isTailnet) }
}

/// Where a Mac can be reached: its Tailscale address. Sent in its pairing QR code
/// and, after that, only inside `SecureChannel`.
public struct DirectEndpoint: Codable, Hashable, Sendable {
    /// An IPv4 address, never a name.
    public var host: String
    public var port: Int

    public init(host: String, port: Int) {
        self.host = host
        self.port = port
    }

    /// The port a Mac listens on unless another program has it.
    public static let defaultPort = 47126
    /// The most endpoints a Mac advertises and a phone keeps.
    public static let maxCount = 4

    public var address: IPv4? { IPv4(host) }

    /// False for anything a phone shouldn't dial: a name, an address off the tailnet, a bad port.
    public var isDialable: Bool { (1...65535).contains(port) && address?.isTailnet == true }

    /// What a Mac with `interfaces` tells its phones: its tailnet address.
    public static func advertised(by interfaces: [NetworkInterface], port: Int) -> [DirectEndpoint] {
        interfaces.filter(\.isTailnet).prefix(1).map { DirectEndpoint(host: $0.address.text, port: port) }
    }

    /// The endpoints worth dialing from a device with `interfaces`: the tailnet ones,
    /// when it is on a tailnet. Elsewhere the same address belongs to someone else's device.
    public static func reachable(_ endpoints: [DirectEndpoint], from interfaces: [NetworkInterface])
        -> [DirectEndpoint]
    {
        guard interfaces.contains(where: \.isTailnet) else { return [] }
        var result: [DirectEndpoint] = []
        for endpoint in endpoints.prefix(maxCount) where endpoint.isDialable && !result.contains(endpoint) {
            result.append(endpoint)
        }
        return result
    }
}

// MARK: - Wire

/// Packets on a direct connection: a 4-byte big-endian length, then the packet.
public enum DirectWire {
    /// The largest packet. A larger one closes the connection, and with it every session on it.
    public static let maxPacketBytes = 4 * 1024 * 1024

    public static func frame(_ packet: Data) -> Data {
        var out = Data(capacity: packet.count + 4)
        withUnsafeBytes(of: UInt32(packet.count).bigEndian) { out.append(contentsOf: $0) }
        out += packet
        return out
    }

    /// Cuts the byte stream back into packets.
    public struct Reader: Sendable {
        private var buffer = Data()

        public init() {}

        /// The packets completed by `data`. Throws on a length no sender uses;
        /// the stream can't be read past it.
        public mutating func append(_ data: Data) throws -> [Data] {
            buffer.append(data)
            var packets: [Data] = []
            var start = buffer.startIndex
            while buffer.endIndex - start >= 4 {
                let length = buffer[start..<start + 4].reduce(0) { $0 << 8 | Int($1) }
                guard length > 0, length <= DirectWire.maxPacketBytes else { throw ChannelError.malformed }
                guard buffer.endIndex - start >= 4 + length else { break }
                packets.append(Data(buffer[start + 4..<start + 4 + length]))
                start += 4 + length
            }
            // A packet still arriving stays where it is; only what was read is dropped.
            if start != buffer.startIndex { buffer = Data(buffer[start...]) }
            return packets
        }
    }
}

// MARK: - Connection

public enum DirectEvent: Sendable {
    case ready
    case packet(Data)
    case closed(Error?)
}

public enum DirectError: Error, Sendable {
    case timedOut
}

/// One TCP connection between a phone and a Mac. Read `events` until `.closed`.
public final class DirectConnection: @unchecked Sendable {
    public let events: AsyncStream<DirectEvent>
    /// The other end's address.
    public let remote: IPv4?
    private let continuation: AsyncStream<DirectEvent>.Continuation
    private let connection: NWConnection
    private let queue = DispatchQueue(label: "sh.termy.direct")
    private let lock = NSLock()
    private var finished = false
    private var queuedBytes = 0
    /// Only touched on `queue`.
    private var reader = DirectWire.Reader()

    /// Dials a Mac. Gives up after `timeout` seconds without a connection:
    /// with the Mac off the tailnet nothing answers, and nothing says so either.
    public convenience init?(to endpoint: DirectEndpoint, timeout: TimeInterval = 3) {
        guard endpoint.isDialable, let port = UInt16(exactly: endpoint.port).flatMap(NWEndpoint.Port.init(rawValue:))
        else { return nil }
        self.init(
            NWConnection(host: NWEndpoint.Host(endpoint.host), port: port, using: Self.parameters()),
            remote: endpoint.address, timeout: timeout)
    }

    /// For a listener's and a test's connections.
    init(_ connection: NWConnection, remote: IPv4?, timeout: TimeInterval?) {
        (events, continuation) = AsyncStream.makeStream(of: DirectEvent.self, bufferingPolicy: .unbounded)
        self.connection = connection
        self.remote = remote
        connection.stateUpdateHandler = { [weak self] state in
            guard let self else { return }
            switch state {
            case .ready:
                self.continuation.yield(.ready)
                self.receive()
            case .failed(let error): self.finish(error)
            // No route yet: an app that was just reopened dials before its tunnel is back, and the connection goes on by itself once there is one, until
            // the timeout. A refused port stays refused.
            case .waiting(let error):
                if case .posix(.ECONNREFUSED) = error { self.finish(error) }
            case .cancelled: self.finish(nil)
            default: break
            }
        }
        connection.start(queue: queue)
        if let timeout {
            queue.asyncAfter(deadline: .now() + timeout) { [weak self] in
                guard let self, self.connection.state != .ready else { return }
                self.finish(DirectError.timedOut)
            }
        }
    }

    deinit { connection.cancel() }

    /// IPv4 TCP that sends each keystroke at once and notices a peer that went away.
    static func parameters() -> NWParameters {
        let tcp = NWProtocolTCP.Options()
        tcp.noDelay = true
        tcp.enableKeepalive = true
        tcp.keepaliveIdle = 20
        tcp.keepaliveInterval = 5
        tcp.keepaliveCount = 3
        let parameters = NWParameters(tls: nil, tcp: tcp)
        if let ip = parameters.defaultProtocolStack.internetProtocol as? NWProtocolIP.Options { ip.version = .v4 }
        return parameters
    }

    /// Bytes handed to `send` that the socket hasn't taken yet. Network.framework
    /// queues sends without limit, so a sender on a slow link checks this and holds
    /// back what it can send later in one piece (screen updates).
    public var pendingSendBytes: Int { lock.withLock { queuedBytes } }

    public func send(_ packet: Data) {
        guard !packet.isEmpty, packet.count <= DirectWire.maxPacketBytes else { return }
        let frame = DirectWire.frame(packet)
        let size = frame.count
        lock.withLock { queuedBytes += size }
        connection.send(
            content: frame,
            completion: .contentProcessed { [weak self] error in
                guard let self else { return }
                self.lock.withLock { self.queuedBytes -= size }
                if let error { self.finish(error) }
            })
    }

    public func close() { finish(nil) }

    private func receive() {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 256 * 1024) {
            [weak self] data, _, complete, error in
            guard let self else { return }
            if let data, !data.isEmpty {
                do {
                    for packet in try self.reader.append(data) { self.continuation.yield(.packet(packet)) }
                } catch {
                    return self.finish(error)
                }
            }
            if let error { return self.finish(error) }
            if complete { return self.finish(nil) }
            self.receive()
        }
    }

    private func finish(_ error: Error?) {
        lock.lock()
        guard !finished else { return lock.unlock() }
        finished = true
        lock.unlock()
        connection.stateUpdateHandler = nil
        connection.cancel()
        continuation.yield(.closed(error))
        continuation.finish()
    }
}

// MARK: - Listener

/// A Mac's listening port. Read `events` until `.closed`, then make a new one.
public final class DirectListener: @unchecked Sendable {
    public enum Event: Sendable {
        case ready(port: Int)
        case connection(DirectConnection)
        case closed(Error?)
    }

    public let events: AsyncStream<Event>
    private let continuation: AsyncStream<Event>.Continuation
    private let listener: NWListener
    private let lock = NSLock()
    private var finished = false

    /// Listens on `port`, or on any free one when nil.
    public init(port: Int?) throws {
        (events, continuation) = AsyncStream.makeStream(of: Event.self, bufferingPolicy: .unbounded)
        let parameters = DirectConnection.parameters()
        // Restarting the host must get the same port back, which phones remember.
        parameters.allowLocalEndpointReuse = true
        listener = try NWListener(
            using: parameters, on: port.flatMap { UInt16(exactly: $0) }.flatMap(NWEndpoint.Port.init(rawValue:)) ?? .any)
        listener.stateUpdateHandler = { [weak self] state in
            guard let self else { return }
            switch state {
            case .ready: self.continuation.yield(.ready(port: Int(self.listener.port?.rawValue ?? 0)))
            case .failed(let error): self.finish(error)
            // Keep waiting when interfaces come and go instead of churning listeners.
            case .waiting: break
            case .cancelled: self.finish(nil)
            default: break
            }
        }
        listener.newConnectionHandler = { [weak self] connection in
            // Phones come over the tailnet. Nothing else, on the local network or the
            // internet, is answered on this port.
            guard let self, let remote = Self.address(of: connection.endpoint),
                remote.isTailnet || remote.isLoopback
            else { return connection.cancel() }
            self.continuation.yield(.connection(DirectConnection(connection, remote: remote, timeout: nil)))
        }
        listener.start(queue: .global(qos: .userInitiated))
    }

    deinit { listener.cancel() }

    public func close() { finish(nil) }

    static func address(of endpoint: NWEndpoint) -> IPv4? {
        guard case .hostPort(.ipv4(let address), _) = endpoint, address.rawValue.count == 4 else { return nil }
        return IPv4(value: address.rawValue.reduce(0) { $0 << 8 | UInt32($1) })
    }

    private func finish(_ error: Error?) {
        lock.lock()
        guard !finished else { return lock.unlock() }
        finished = true
        lock.unlock()
        listener.stateUpdateHandler = nil
        listener.newConnectionHandler = nil
        listener.cancel()
        continuation.yield(.closed(error))
        continuation.finish()
    }
}
