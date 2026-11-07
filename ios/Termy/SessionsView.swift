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
        .onAppear { link.connect() }
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
                            Label(
                                tab.title ?? tab.paneIDs.compactMap { link.pane($0)?.title }.first
                                    ?? "Tab \(index + 1)", systemImage: "terminal"
                            )
                            .lineLimit(1).foregroundStyle(.primary)
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
            VStack(alignment: .leading, spacing: 2) {
                Text(pane.title?.isEmpty == false ? pane.title! : fallback).foregroundStyle(
                    pane.exited ? .secondary : .primary
                ).lineLimit(1)
                if let directory = pane.workingDirectory {
                    Text(Self.abbreviate(directory)).font(.caption.monospaced()).foregroundStyle(.secondary).lineLimit(
                        1
                    ).truncationMode(.head)
                }
            }
        }
        .swipeActions { Button("End", role: .destructive) { closing = pane } }
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
    let link: HostLink
    let changed: Bool
    var body: some View {
        Section {
            Label(
                changed ? "This computer’s key changed" : "Pair this computer again",
                systemImage: "exclamationmark.shield"
            ).foregroundStyle(.orange)
            Text("Remove this computer in Settings, then scan a new QR code from Termy Next → Settings → Mobile.")
                .foregroundStyle(.secondary)
        }
    }
}
