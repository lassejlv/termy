import Foundation
import Observation
import TermyRemoteKit
import UIKit

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

    var device: RelayDevice
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

    @ObservationIgnored private weak var model: AppModel?
    @ObservationIgnored private var handshake: SecureChannel.ClientHandshake?
    @ObservationIgnored private var channel: SecureChannel?
    @ObservationIgnored private var nextStream = 1
    @ObservationIgnored private var timeout: Task<Void, Never>?
    /// Whether a view wants this link open.
    @ObservationIgnored private var wanted = false

    init(device: RelayDevice, model: AppModel) {
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
        guard model.isConnected(to: device.id) else {
            state = .connecting
            return
        }
        guard state != .connected, handshake == nil else { return }
        guard let hostKey = device.publicKeyData else { return }
        state = .connecting
        let handshake = SecureChannel.startClient(identity: model.identity, hostStaticKey: hostKey)
        self.handshake = handshake
        model.send(handshake.initPacket, to: device.id)
        timeout?.cancel()
        timeout = Task { [weak self] in
            try? await Task.sleep(nanoseconds: 10_000_000_000)
            guard let self, !Task.isCancelled, self.handshake != nil else { return }
            self.handshake = nil
            self.state =
                self.device.online
                ? .failed("\(self.device.name) didn't answer. Is Termy Next running with Mobile access enabled?")
                : .offline
        }
    }

    func disconnect() {
        wanted = false
        for screen in screens.values { send(.detach(stream: screen.stream)) }
        screens.removeAll()
        creating = false
        creationTimeout?.cancel()
        channel = nil
        handshake = nil
        state = .idle
    }

    func relayReady() {
        if wanted { connect() }
    }

    func relayWentAway() {
        creating = false
        creationTimeout?.cancel()
        channel = nil
        handshake = nil
        timeout?.cancel()
        if state == .connected || state == .connecting { state = .offline }
        for screen in screens.values { screen.live = false }
    }

    func presenceChanged(online: Bool) {
        device.online = online
        if online {
            if wanted, state != .connected { connect() }
        } else {
            channel = nil
            handshake = nil
            if state == .connected || state == .connecting { state = .offline }
            for screen in screens.values { screen.live = false }
        }
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
                state = .failed("\(device.name) couldn't prove it holds the key you trusted.")
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
        case .sessions(let list):
            sessions = list
        case .frame(let frame):
            screens[frame.stream]?.apply(frame)
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
        case .created(let paneID):
            creating = false
            creationTimeout?.cancel()
            createdPaneID = paneID
        case .error(let message):
            creating = false
            creationTimeout?.cancel()
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
}
