import SwiftUI
import TermyRemoteKit

struct SessionsView: View {
    @Environment(AppModel.self) private var model
    let link: HostLink
    @State private var openScreen: TerminalScreen?
    @State private var closing: RemotePane?
    @State private var newSession = false
    @State private var sessionName = ""
    @State private var renameTarget: (pane: String, session: Bool)?
    @State private var renamed = ""
    /// "host|pane" of the terminal open when the app was last used.
    @SceneStorage("openPane") private var openPane = ""

    var body: some View {
        List {
            switch link.state {
            case .needsTrust: TrustSection(link: link, changed: false)
            case .keyChanged: TrustSection(link: link, changed: true)
            case .offline:
                StatusRow(
                    symbol: "moon.zzz",
                    text: "\(link.device.name) is offline. Open Termy Next with Mobile access enabled.")
            case .failed(let message):
                StatusRow(symbol: "exclamationmark.triangle", text: message)
                Button("Try Again") { link.connect() }
            case .connecting, .idle:
                HStack {
                    ProgressView()
                    Text("Connecting securely…").foregroundStyle(.secondary)
                }
            case .connected: sessionSections
            }
            if let error = link.lastError { Text(error).font(.footnote).foregroundStyle(.red) }
            if link.state == .connected, link.protocolVersion < 2 {
                Text("Update Termy Next on your Mac for config sync and session controls.").font(.footnote)
                    .foregroundStyle(.secondary)
            }
        }
        .navigationTitle(link.hostName ?? link.device.name)
        .toolbar {
            if link.state == .connected {
                ToolbarItem(placement: .topBarTrailing) {
                    Menu {
                        Button("New Session", systemImage: "plus.rectangle.on.folder") {
                            sessionName = ""
                            newSession = true
                        }
                        ForEach(link.sessions.groups) { group in
                            if let anchor = group.tabs.first?.paneIDs.first {
                                Button("New Tab in \(group.name)") {
                                    link.send(.createTab(inSessionContaining: anchor))
                                }
                            }
                        }
                    } label: {
                        if link.creating { ProgressView() } else { Image(systemName: "plus") }
                    }
                    .accessibilityLabel("Create session or tab")
                    .disabled(link.creating || link.protocolVersion < 2)
                }
            }
        }
        .refreshable { link.send(.listSessions) }
        .onAppear {
            link.connect()
            followRoute()
        }
        .onChange(of: model.route) { _, _ in followRoute() }
        .onChange(of: link.state) { _, _ in followRoute() }
        .onChange(of: link.sessions) { _, _ in
            followRoute()
            restore()
        }
        .onChange(of: openScreen?.paneID) { _, pane in
            openPane = pane.map { "\(link.device.id.uuidString)|\($0)" } ?? ""
        }
        .onChange(of: link.createdPaneID) { _, id in
            guard let id, openScreen == nil else { return }
            link.createdPaneID = nil
            openScreen = link.open(RemotePane(id: id, title: nil, workingDirectory: nil, exited: false))
        }
        .fullScreenCover(item: $openScreen) { screen in TerminalScreenView(link: link, screen: screen) }
        .alert("New Session", isPresented: $newSession) {
            TextField("Session name", text: $sessionName)
            Button("Create") { link.send(.createSession(name: sessionName)) }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Creates a session in Termy Next on your Mac.")
        }
        .alert("Rename", isPresented: Binding(get: { renameTarget != nil }, set: { if !$0 { renameTarget = nil } })) {
            TextField("Name", text: $renamed)
            Button("Save") {
                if let target = renameTarget {
                    link.send(
                        target.session
                            ? .renameSession(containing: target.pane, name: renamed)
                            : .renameTab(containing: target.pane, title: renamed))
                }
                renameTarget = nil
            }
            Button("Cancel", role: .cancel) { renameTarget = nil }
        }
        .confirmationDialog(
            "End this pane?", isPresented: Binding(get: { closing != nil }, set: { if !$0 { closing = nil } }),
            titleVisibility: .visible, presenting: closing
        ) { pane in
            Button("End Pane", role: .destructive) { link.send(.closePane(paneID: pane.id)) }
        } message: { _ in
            Text("The program running in it stops on your Mac too.")
        }
    }

    /// Opens the pane, or runs the command, that a widget, quick action or Shortcut asked for.
    private func followRoute() {
        guard let route = model.route, route.host == link.device.id, link.state == .connected else { return }
        if let command = route.command {
            guard link.protocolVersion >= 2, !link.creating else { return }
            model.route = nil
            // An open terminal switches to the new tab itself; otherwise the list opens it.
            link.commandForNextCreation = command
            if let anchor = link.sessions.groups.first(where: { $0.name == route.sessionName })?.tabs.first?.paneIDs
                .first
            {
                link.send(.createTab(inSessionContaining: anchor))
            } else {
                link.send(.createSession(name: route.sessionName))
            }
        } else if let id = route.pane {
            // Wait for the session list; a pane that's gone just leaves the list showing.
            guard !link.sessions.panes.isEmpty else { return }
            model.route = nil
            if let pane = link.pane(id), !pane.exited, openScreen?.paneID != id {
                openScreen = nil
                openScreen = link.open(pane)
            }
        } else {
            model.route = nil
        }
    }

    /// Reopens the terminal that was open when the app was last used.
    private func restore() {
        guard openScreen == nil, model.route == nil, !openPane.isEmpty else { return }
        let parts = openPane.split(separator: "|", maxSplits: 1).map(String.init)
        guard parts.count == 2, parts[0] == link.device.id.uuidString else { return }
        openPane = ""
        if let pane = link.pane(parts[1]), !pane.exited { openScreen = link.open(pane) }
    }

    @ViewBuilder private var sessionSections: some View {
        let grouped = Set(link.sessions.groups.flatMap { $0.tabs.flatMap(\.paneIDs) })
        ForEach(link.sessions.groups) { group in
            Section {
                ForEach(Array(group.tabs.enumerated()), id: \.offset) { index, tab in
                    DisclosureGroup {
                        ForEach(tab.paneIDs, id: \.self) { id in
                            if let pane = link.pane(id) { paneRow(pane, fallback: "Pane") }
                        }
                        if let anchor = tab.paneIDs.first, link.protocolVersion >= 2 {
                            Button("Rename Tab") {
                                renamed = tab.title ?? ""
                                renameTarget = (anchor, false)
                            }
                        }
                    } label: {
                        Button {
                            if let id = tab.activePaneID ?? tab.paneIDs.first, let pane = link.pane(id) {
                                openScreen = link.open(pane)
                            }
                        } label: {
                            HStack {
                                Label(
                                    tab.title ?? tab.paneIDs.compactMap { link.pane($0)?.title }.first
                                        ?? "Tab \(index + 1)", systemImage: "terminal"
                                )
                                .lineLimit(1).foregroundStyle(.primary)
                                Spacer(minLength: 4)
                                CommandBadge(state: commandState(of: tab))
                            }
                        }
                    }
                }
            } header: {
                HStack {
                    Text(group.name)
                    Spacer()
                    if let anchor = group.tabs.first?.paneIDs.first, link.protocolVersion >= 2 {
                        Menu {
                            Button("New Tab") { link.send(.createTab(inSessionContaining: anchor)) }
                            Button("Rename Session") {
                                renamed = group.name
                                renameTarget = (anchor, true)
                            }
                        } label: {
                            Image(systemName: "ellipsis.circle").accessibilityLabel("Session actions")
                        }
                        .disabled(link.creating)
                    }
                }
            }
        }
        let loose = link.sessions.panes.filter { !grouped.contains($0.id) && !$0.exited }
        if !loose.isEmpty {
            Section("Other Panes") { ForEach(loose) { paneRow($0, fallback: "Shell") } }
        }
        if link.sessions.panes.isEmpty {
            ContentUnavailableView(
                "No Sessions", systemImage: "terminal", description: Text("Tap + to create a session on your Mac."))
        }
    }

    private func paneRow(_ pane: RemotePane, fallback: String) -> some View {
        Button {
            openScreen = link.open(pane)
        } label: {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text(pane.title?.isEmpty == false ? pane.title! : fallback).foregroundStyle(
                        pane.exited ? .secondary : .primary
                    ).lineLimit(1)
                    if let directory = pane.workingDirectory {
                        Text(Self.abbreviate(directory)).font(.caption.monospaced()).foregroundStyle(.secondary)
                            .lineLimit(1).truncationMode(.head)
                    }
                }
                Spacer(minLength: 4)
                CommandBadge(state: pane.command)
            }
        }
        .swipeActions { Button("End", role: .destructive) { closing = pane } }
    }
    /// What a tab's row shows: a command running in any of its panes, else its active pane's failure.
    private func commandState(of tab: RemoteSessionGroup.Tab) -> RemoteCommandState? {
        let states = tab.paneIDs.compactMap { link.pane($0)?.command }
        if let running = states.first(where: \.running) { return running }
        return (tab.activePaneID ?? tab.paneIDs.first).flatMap { link.pane($0)?.command }
    }

    static func abbreviate(_ path: String) -> String {
        let path = path.hasPrefix("file://") ? (URL(string: path)?.path ?? path) : path
        if let range = path.range(of: #"^/Users/[^/]+"#, options: .regularExpression) {
            return "~" + path[range.upperBound...]
        }
        return path
    }
}

struct StatusRow: View {
    let symbol: String
    let text: String
    var body: some View { Label(text, systemImage: symbol).foregroundStyle(.secondary) }
}
struct TrustSection: View {
    @Environment(AppModel.self) private var model
    let link: HostLink
    let changed: Bool
    @State private var confirming = false
    @State private var error: String?
    var body: some View {
        Section {
            Label(
                changed ? "This computer’s key changed" : "Pair this computer again",
                systemImage: "exclamationmark.shield"
            ).foregroundStyle(.orange)
            Text(
                changed
                    ? "Termy can’t confirm this is the Mac you paired. If you reinstalled Termy Next or reset its key, pair again. Otherwise, don’t connect."
                    : "Scan a new QR code from Termy Next → Settings → Mobile."
            )
            .foregroundStyle(.secondary)
            Button("Remove and Pair Again", systemImage: "qrcode.viewfinder") { confirming = true }
            if let error { Text(error).foregroundStyle(.red) }
        }
        .confirmationDialog(
            "Remove \(link.device.name) and pair it again?", isPresented: $confirming, titleVisibility: .visible
        ) {
            Button("Remove and Pair Again", role: .destructive) {
                Task {
                    do { try await model.repair(link.device) } catch { self.error = error.localizedDescription }
                }
            }
        } message: {
            Text("You’ll scan a new QR code from Termy Next → Settings → Mobile on that Mac.")
        }
    }
}
