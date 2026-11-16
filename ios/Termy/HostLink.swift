import Foundation
import Observation
import TermyRemoteKit
import UIKit

/// How packets reach a computer: through the relay, or over a connection of the phone's
/// own to it on the same network or tailnet.
enum HostPath: Equatable {
    case relay, direct
}

/// What a `HostLink` needs from the app: trust decisions and the ways to the computer.
@MainActor
protocol HostTransport: AnyObject {
    var identity: DeviceIdentity { get }
    func verdict(for host: RelayDevice) -> TrustStore.Verdict
    /// The best way to the computer right now: direct when there is one, else the relay.
    func path(to host: UUID) -> HostPath?
    func send(_ packet: Data, to host: UUID, via path: HostPath)
    /// A view wants the computer: open a connection of the phone's own to it, where one can be.
    func dial(_ host: UUID)
    /// The computer said where it can be reached without the relay.
    func endpointsChanged(_ endpoints: [DirectEndpoint], for host: UUID)
    /// The computer didn't answer, or didn't prove its key, on the direct path.
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

    /// Typed into the next pane the host creates for us, e.g. a Shortcut's command.
    @ObservationIgnored var commandForNextCreation: String?
    @ObservationIgnored private var pendingInputs: [String: String] = [:]
    /// Output requests waiting for the host, by stream; one at a time each.
    @ObservationIgnored private var outputRequests: [Int: (id: UUID, reply: (RemoteCommandOutput?) -> Void)] = [:]

    @ObservationIgnored private weak var model: (any HostTransport)?
    @ObservationIgnored private var handshake: SecureChannel.ClientHandshake?
    @ObservationIgnored private var handshakePath: HostPath?
    @ObservationIgnored private var channel: SecureChannel?
    /// The way the channel goes; nil while there is none.
    private(set) var path: HostPath?
    @ObservationIgnored private var nextStream = 1
    /// Glyphs the host sent before, which later frames name by key only.
    /// Every frame goes through it, whichever screen it is for.
    @ObservationIgnored private let glyphs = GlyphCache()
    @ObservationIgnored private var timeout: Task<Void, Never>?
    /// Whether a view wants this link open.
    @ObservationIgnored private(set) var wanted = false

    init(device: RelayDevice, model: any HostTransport) {
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
        guard let best = model.path(to: device.id) else {
            if state != .connected { state = .connecting }
            return
        }
        // A channel through the relay moves to the direct path when one opens. The relay's
        // keeps working until the new one is up, so nothing typed meanwhile is lost.
        if state == .connected, !(best == .direct && path == .relay) { return }
        guard handshake == nil, let hostKey = device.publicKeyData else { return }
        if state != .connected { state = .connecting }
        let handshake = SecureChannel.startClient(identity: model.identity, hostStaticKey: hostKey)
        self.handshake = handshake
        handshakePath = best
        model.send(handshake.initPacket, to: device.id, via: best)
        timeout?.cancel()
        timeout = Task { [weak self] in
            // The Mac is a few milliseconds away on the direct path; the relay may be slow.
            try? await Task.sleep(for: .seconds(best == .direct ? 4 : 10))
            guard let self, !Task.isCancelled, self.handshake != nil else { return }
            self.handshake = nil
            self.handshakePath = nil
            if best == .direct {
                // Whatever answers at that address isn't serving us: back to the relay.
                self.model?.directFailed(for: self.device.id)
                return
            }
            guard self.state != .connected else { return }
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
        timeout?.cancel()
        channel = nil
        path = nil
        handshake = nil
        handshakePath = nil
        state = .idle
    }

    func relayReady() {
        if wanted { connect() }
    }

    func relayWentAway() {
        went(away: .relay)
    }

    /// The phone's own connection to the computer opened: use it, also for a channel
    /// the relay carries or a handshake under way there.
    func directReady() {
        guard wanted else { return }
        if handshakePath == .relay {
            handshake = nil
            handshakePath = nil
            timeout?.cancel()
        }
        connect()
    }

    /// The direct connection closed. The relay takes over when it is there.
    func directWentAway() {
        went(away: .direct)
        if wanted { connect() }
    }

    /// No way to the computer opened and no relay will find one. Dialing goes on.
    func unreachable() {
        guard wanted, channel == nil, handshake == nil, state == .connecting || state == .offline else { return }
        state = .failed(
            "Can’t reach \(device.name). It uses no relay: join its network, or turn on Tailscale on both devices.")
    }

    private func went(away gone: HostPath) {
        if handshakePath == gone {
            handshake = nil
            handshakePath = nil
            timeout?.cancel()
        }
        if path == gone {
            creating = false
            creationTimeout?.cancel()
            channel = nil
            path = nil
            for screen in screens.values { screen.live = false }
        }
        // The other path may still carry the channel, or a handshake that will make one.
        guard channel == nil else { return }
        guard handshake == nil else {
            if state == .connected { state = .connecting }
            return
        }
        if state == .connected || state == .connecting { state = .offline }
    }

    /// What the relay sees of the computer. A direct channel doesn't depend on it.
    func presenceChanged(online: Bool) {
        device.online = online
        if online {
            if wanted, state != .connected { connect() }
        } else {
            went(away: .relay)
        }
    }

    // MARK: Packets

    func receive(_ packet: Data, via: HostPath = .relay) {
        if let handshake, via == handshakePath {
            do {
                channel = try handshake.finish(packet)
                path = via
                self.handshake = nil
                handshakePath = nil
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
                handshakePath = nil
                timeout?.cancel()
                // Another device has the address the Mac had: nothing was given away, use the relay.
                if via == .direct {
                    model?.directFailed(for: device.id)
                } else {
                    state = .failed("\(device.name) couldn't prove it holds the key you trusted.")
                }
            }
            return
        }
        // Packets still on their way along a path the channel left are not for it.
        guard via == path, var channel else { return }
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
        guard var channel, let path, let model else { return }
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
            model.send(packet, to: device.id, via: path)
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
