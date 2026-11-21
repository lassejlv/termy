import Foundation
import Observation
import TermyRemoteKit
import UIKit

/// What a `HostLink` needs from the app: trust decisions and the connection to the computer.
@MainActor
protocol HostTransport: AnyObject {
    var identity: DeviceIdentity { get }
    func verdict(for host: PairedDevice) -> TrustStore.Verdict
    /// Whether the phone has a connection open to the computer right now.
    func isConnected(to host: UUID) -> Bool
    func send(_ packet: Data, to host: UUID)
    /// A view wants the computer: open a connection to it.
    func dial(_ host: UUID)
    /// The computer said where it can be reached.
    func endpointsChanged(_ endpoints: [DirectEndpoint], for host: UUID)
    /// The computer didn't answer, or didn't prove its key, on the connection.
    func directFailed(for host: UUID)
    func configuration(for host: UUID) -> RemoteConfiguration?
    func saveConfiguration(_ configuration: RemoteConfiguration, for host: UUID)
    func sessionsChanged(for host: UUID)
}

/// The end-to-end encrypted channel to one computer, its session list, and
/// the screens open on it.
@MainActor
@Observable
final class HostLink {
    enum State: Equatable {
        case idle
        /// The host's key isn't pinned yet; the user compares fingerprints first.
        case needsTrust
        /// The host presents another key than the one pinned.
        case keyChanged
        case offline
        case connecting
        case connected
        case failed(String)
    }

    var device: PairedDevice
    private(set) var state: State = .idle
    private(set) var hostName: String?
    private(set) var protocolVersion = 1
    private(set) var configuration: RemoteConfiguration
    private(set) var creating = false
    @ObservationIgnored private var creationTimeout: Task<Void, Never>?
    private(set) var sessions = RemoteSessionList(groups: [], panes: [])
    private(set) var screens: [Int: TerminalScreen] = [:]
    var lastError: String?
    /// Set when the host made a pane at our request; the list opens it.
    var createdPaneID: String?

    /// Typed into the next pane the host creates for us, e.g. a Shortcut's command.
    @ObservationIgnored var commandForNextCreation: String?
    @ObservationIgnored private var pendingInputs: [String: String] = [:]
    /// Output requests waiting for the host, by stream; one at a time each.
    @ObservationIgnored private var outputRequests: [Int: (id: UUID, reply: (RemoteCommandOutput?) -> Void)] = [:]

    @ObservationIgnored private weak var model: (any HostTransport)?
    @ObservationIgnored private var handshake: SecureChannel.ClientHandshake?
    @ObservationIgnored private var channel: SecureChannel?
    @ObservationIgnored private var nextStream = 1
    /// Glyphs the host sent before, which later frames name by key only.
    /// Every frame goes through it, whichever screen it is for.
    @ObservationIgnored private let glyphs = GlyphCache()
    @ObservationIgnored private var timeout: Task<Void, Never>?
    /// Whether a view wants this link open.
    @ObservationIgnored private(set) var wanted = false

    init(device: PairedDevice, model: any HostTransport) {
        self.device = device
        self.model = model
        self.configuration = model.configuration(for: device.id) ?? RemoteConfiguration()
    }

    // MARK: Connecting

    func connect() {
        wanted = true
        guard let model else { return }
        switch model.verdict(for: device) {
        case .unknown:
            state = .needsTrust
            return
        case .changed:
            state = .keyChanged
            return
        case .trusted:
            break
        }
        model.dial(device.id)
        guard model.isConnected(to: device.id) else {
            if state != .connected { state = .connecting }
            return
        }
        guard state != .connected, handshake == nil, let hostKey = device.publicKeyData else { return }
        state = .connecting
        let handshake = SecureChannel.startClient(identity: model.identity, hostStaticKey: hostKey)
        self.handshake = handshake
        model.send(handshake.initPacket, to: device.id)
        timeout?.cancel()
        timeout = Task { [weak self] in
            // The Mac is a few milliseconds away.
            try? await Task.sleep(for: .seconds(4))
            guard let self, !Task.isCancelled, self.handshake != nil else { return }
            self.handshake = nil
            // Whatever answers at that address isn't serving us: dial again.
            self.model?.directFailed(for: self.device.id)
        }
    }

    func disconnect() {
        wanted = false
        for screen in screens.values { send(.detach(stream: screen.stream)) }
        screens.removeAll()
        creating = false
        creationTimeout?.cancel()
        timeout?.cancel()
        channel = nil
        handshake = nil
        state = .idle
    }

    /// The phone's connection to the computer opened.
    func directReady() {
        guard wanted else { return }
        connect()
    }

    /// The connection closed. Dialing goes on while a view wants the computer.
    func directWentAway() {
        handshake = nil
        timeout?.cancel()
        if channel != nil {
            creating = false
            creationTimeout?.cancel()
            channel = nil
            for screen in screens.values { screen.live = false }
        }
        if state == .connected || state == .connecting { state = .offline }
        if wanted { connect() }
    }

    /// No connection to the computer opened. Dialing goes on.
    func unreachable() {
        guard wanted, channel == nil, handshake == nil, state == .connecting || state == .offline else { return }
        state = .failed("Can’t reach \(device.name). Join its network, or turn on Tailscale on both devices.")
    }

    // MARK: Packets

    func receive(_ packet: Data) {
        if let handshake {
            do {
                channel = try handshake.finish(packet)
                self.handshake = nil
                timeout?.cancel()
                state = .connected
                lastError = nil
                send(.listSessions)
                // Pick up where we were: streams survive a reconnect.
                for screen in screens.values {
                    screen.changedRows = nil  // Refit after the host restores the previous stream’s size.
                    send(.attach(stream: screen.stream, paneID: screen.paneID))
                }
            } catch {
                self.handshake = nil
                timeout?.cancel()
                // Another device has the address the Mac had: nothing was given away.
                model?.directFailed(for: device.id)
            }
            return
        }
        guard var channel else { return }
        do {
            let plaintext = try channel.open(packet: packet)
            self.channel = channel
            handle(try MessageCodec.decode(HostMessage.self, from: plaintext))
        } catch {
            lastError = "Dropped a damaged message."
        }
    }

    private func handle(_ message: HostMessage) {
        switch message {
        case .hello(let hostName, let version):
            self.hostName = hostName
            protocolVersion = version
            if version >= 2 { send(.clientHello(version: RemoteProtocol.version)) }
        case .sessions(let list):
            let old = sessions
            sessions = list
            CommandNotifier.shared.sessionsChanged(
                from: old, to: list, on: device, watching: Set(screens.values.map(\.paneID)))
            model?.sessionsChanged(for: device.id)
        case .frame(var frame):
            glyphs.resolve(&frame)
            guard let screen = screens[frame.stream] else { return }
            screen.apply(frame)
            if let input = screen.pendingInput, !screen.exited {
                screen.pendingInput = nil
                send(.paste(stream: screen.stream, text: input))
                send(.key(stream: screen.stream, key: RemoteKey(key: "enter")))
            }
        case .title(let stream, let title):
            screens[stream]?.title = title
        case .exited(let stream):
            screens[stream]?.exited = true
        case .bell:
            UINotificationFeedbackGenerator().notificationOccurred(.warning)
        case .configuration(let config):
            configuration = config
            model?.saveConfiguration(config, for: device.id)
            for screen in screens.values {
                screen.configuration = config
                screen.changedRows = nil
                screen.onChange?()
            }
        case .searchResults(let stream, let matches):
            screens[stream]?.searchMatches = matches
        case .image(let stream, let key, let png):
            screens[stream]?.store(image: png, key: key)
        case .shell(let stream, let event):
            screens[stream]?.apply(shell: event)
        case .uploaded(let stream, let path):
            screens[stream]?.uploadedPath = path
        case .endpoints(let endpoints):
            model?.endpointsChanged(endpoints, for: device.id)
        case .commandOutput(let stream, let output):
            outputRequests.removeValue(forKey: stream)?.reply(output)
        case .images(let list):
            screens[list.stream]?.apply(list)
        case .created(let paneID):
            creating = false
            creationTimeout?.cancel()
            if let command = commandForNextCreation {
                commandForNextCreation = nil
                pendingInputs[paneID] = command
            }
            createdPaneID = paneID
        case .error(let message):
            creating = false
            creationTimeout?.cancel()
            commandForNextCreation = nil
            lastError = message
        }
    }

    func send(_ message: ClientMessage) {
        guard var channel, let model else { return }
        switch message {
        case .createPane, .createSession, .createTab, .splitPane:
            guard !creating else { return }
            creating = true
            lastError = nil
            creationTimeout?.cancel()
            creationTimeout = Task { [weak self] in
                try? await Task.sleep(for: .seconds(15))
                guard !Task.isCancelled else { return }
                self?.creating = false
                self?.commandForNextCreation = nil
                self?.lastError = "The Mac hasn’t answered the creation request. Refresh before trying again."
            }
        default: break
        }
        do {
            let packet = try channel.seal(try MessageCodec.encode(message))
            self.channel = channel
            model.send(packet, to: device.id)
        } catch {
            lastError = "Couldn't send: \(error.localizedDescription)"
        }
    }

    // MARK: Screens

    func open(_ pane: RemotePane) -> TerminalScreen {
        if let existing = screens.values.first(where: { $0.paneID == pane.id }) { return existing }
        let stream = nextStream
        nextStream += 1
        let screen = TerminalScreen(stream: stream, paneID: pane.id, title: pane.title)
        screen.configuration = configuration
        screen.pendingInput = pendingInputs.removeValue(forKey: pane.id)
        screens[stream] = screen
        send(.attach(stream: stream, paneID: pane.id))
        return screen
    }

    func close(_ screen: TerminalScreen) {
        if screen.fitted { send(.resize(stream: screen.stream, cols: nil, rows: nil)) }
        send(.detach(stream: screen.stream))
        screens[screen.stream] = nil
    }

    func pane(_ id: String) -> RemotePane? {
        sessions.panes.first { $0.id == id }
    }

    /// Whether the host knows prompts: jumping between them and reading a command's output.
    var followsPrompts: Bool { state == .connected && protocolVersion >= 5 }

    /// Asks for a command's output: the one whose prompt starts at `promptBufferRow`, else the last.
    /// `reply` gets nil when there's none, the host doesn't answer within 10 s, or a newer request replaces it.
    func requestOutput(of screen: TerminalScreen, promptBufferRow: Int? = nil, reply: @escaping (RemoteCommandOutput?) -> Void) {
        guard followsPrompts else { return reply(nil) }
        let id = UUID()
        outputRequests.removeValue(forKey: screen.stream)?.reply(nil)
        outputRequests[screen.stream] = (id, reply)
        send(.commandOutput(stream: screen.stream, promptBufferRow: promptBufferRow))
        Task { [weak self] in
            try? await Task.sleep(for: .seconds(10))
            guard let self, self.outputRequests[screen.stream]?.id == id else { return }
            self.outputRequests.removeValue(forKey: screen.stream)?.reply(nil)
        }
    }

    /// Sends a file into a pane: the Mac saves it and types its path, as dropping a file there would.
    func upload(_ data: Data, name: String, to screen: TerminalScreen) -> Bool {
        guard followsPrompts, data.count <= RemoteUpload.maxBytes, !data.isEmpty else { return false }
        send(.upload(stream: screen.stream, name: name, data: data))
        return true
    }

    /// Every live pane in session, tab and split order: what swiping steps through.
    var orderedPanes: [RemotePane] {
        var seen = Set<String>()
        var result: [RemotePane] = []
        for group in sessions.groups {
            for tab in group.tabs {
                for id in tab.paneIDs where !seen.contains(id) {
                    if let pane = pane(id), !pane.exited {
                        seen.insert(id)
                        result.append(pane)
                    }
                }
            }
        }
        for pane in sessions.panes where !seen.contains(pane.id) && !pane.exited { result.append(pane) }
        return result
    }

    /// The session holding a pane.
    func group(containing paneID: String) -> RemoteSessionGroup? {
        sessions.groups.first { $0.tabs.contains { $0.paneIDs.contains(paneID) } }
    }
}
