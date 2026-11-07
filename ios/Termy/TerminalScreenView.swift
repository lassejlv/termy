import SwiftUI
import TermyRemoteKit

struct TerminalScreenView: View {
    let link: HostLink
    @State private var screen: TerminalScreen
    @Environment(\.dismiss) private var dismiss
    @State private var terminalView: TerminalUIView?
    @State private var showSearch = false
    @State private var query = ""
    @State private var mouseMode = false
    @State private var showConfig = false
    @State private var closeConfirmation = false
    init(link: HostLink, screen: TerminalScreen) {
        self.link = link
        _screen = State(initialValue: screen)
    }
    var body: some View {
        NavigationStack {
            TerminalViewRepresentable(screen: screen, link: link) { view in
                DispatchQueue.main.async {
                    terminalView = view
                    view.mouseMode = mouseMode
                }
            }
            .id(screen.stream)
            .background(Color(uiColor: Self.color(screen.configuration.background)))
            .overlay(alignment: .top) { banner }
            .overlay(alignment: .bottomTrailing) {
                if screen.displayOffset > 0 {
                    Button {
                        link.send(.scrollToBottom(stream: screen.stream))
                    } label: {
                        Label("Live", systemImage: "arrow.down.to.line").font(.footnote.bold()).padding(10)
                    }.buttonStyle(.borderedProminent).padding()
                }
            }
            .navigationTitle(screen.title ?? "Terminal")
            .navigationBarTitleDisplayMode(.inline)
            .toolbarBackground(.visible, for: .navigationBar)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) { Button("Done") { dismiss() } }
                ToolbarItem(placement: .topBarTrailing) {
                    Menu {
                        Section("Switch Terminal") {
                            ForEach(link.sessions.groups) { group in
                                Menu(group.name) {
                                    ForEach(Array(group.tabs.enumerated()), id: \.offset) { index, tab in
                                        ForEach(tab.paneIDs, id: \.self) { id in
                                            if let pane = link.pane(id) {
                                                Button(pane.title ?? tab.title ?? "Tab \(index+1)") { switchTo(pane) }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        if link.protocolVersion >= 2 {
                            Button("New Tab in This Session", systemImage: "plus") {
                                link.send(.createTab(inSessionContaining: screen.paneID))
                            }
                            Button("Split Side by Side", systemImage: "rectangle.split.2x1") {
                                link.send(.splitPane(paneID: screen.paneID, horizontal: true))
                            }
                            Button("Split Above and Below", systemImage: "rectangle.split.1x2") {
                                link.send(.splitPane(paneID: screen.paneID, horizontal: false))
                            }
                            Button("Find in Scrollback", systemImage: "magnifyingglass") {
                                showSearch = true
                                _ = terminalView?.resignFirstResponder()
                            }
                        }
                        Button(
                            screen.fitted ? "Use Mac Grid Size" : "Fit to iPhone",
                            systemImage: "arrow.up.left.and.arrow.down.right"
                        ) { toggleFit() }
                        Toggle("Mouse Mode", systemImage: "cursorarrow", isOn: $mouseMode)
                        Button("Mac Configuration", systemImage: "slider.horizontal.3") { showConfig = true }
                        Button("Paste", systemImage: "doc.on.clipboard") { terminalView?.paste() }
                        Button("Show Keyboard", systemImage: "keyboard") { _ = terminalView?.becomeFirstResponder() }
                        Button("Hide Keyboard", systemImage: "keyboard.chevron.compact.down") {
                            _ = terminalView?.resignFirstResponder()
                        }
                        Button("End Pane", systemImage: "xmark.square", role: .destructive) { closeConfirmation = true }
                    } label: {
                        Image(systemName: "ellipsis.circle")
                    }
                    .accessibilityLabel("Terminal actions and tabs")
                }
            }
            .onChange(of: mouseMode) { _, value in terminalView?.mouseMode = value }
            .onChange(of: link.createdPaneID) { _, id in
                guard let id else { return }
                link.createdPaneID = nil
                switchTo(RemotePane(id: id, title: nil, workingDirectory: nil, exited: false))
            }
            .onChange(of: screen.live) { _, live in if live { terminalView?.resetFit() } }
            .sheet(isPresented: $showSearch) { searchSheet }
            .sheet(isPresented: $showConfig) { configSheet }
            .confirmationDialog(
                "End this pane on your Mac?", isPresented: $closeConfirmation, titleVisibility: .visible
            ) {
                Button("End Pane", role: .destructive) {
                    link.send(.closePane(paneID: screen.paneID))
                    dismiss()
                }
            }
        }
        .onDisappear { link.close(screen) }
        .preferredColorScheme(link.configuration.appearance == "light" ? .light : .dark)
    }
    private var searchSheet: some View {
        NavigationStack {
            List {
                ForEach(Array(screen.searchMatches.enumerated()), id: \.offset) { _, match in
                    Button {
                        link.send(.revealSearch(stream: screen.stream, bufferRow: match.row))
                        showSearch = false
                    } label: {
                        VStack(alignment: .leading) {
                            Text("Line \(match.row+1)").font(.caption).foregroundStyle(.secondary)
                            Text(match.line).font(.system(.body, design: .monospaced)).lineLimit(3)
                        }
                    }
                }
                if !query.isEmpty, screen.searchMatches.isEmpty { Text("No matches").foregroundStyle(.secondary) }
            }
            .searchable(text: $query, prompt: "Search screen and history")
            .task(id: query) {
                do {
                    try await Task.sleep(for: .milliseconds(250))
                    try Task.checkCancellation()
                    link.send(.search(stream: screen.stream, query: query))
                } catch {}
            }
            .navigationTitle("Find in Terminal")
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { showSearch = false } } }
        }
    }
    private var configSheet: some View {
        NavigationStack {
            Form {
                Section("Synced from \(link.hostName ?? link.device.name)") {
                    LabeledContent("Theme", value: link.configuration.theme)
                    LabeledContent("Font", value: link.configuration.fontFamily)
                    LabeledContent("Font Size", value: "\(Int(link.configuration.fontSize)) pt")
                    Text(
                        "Updates automatically over your encrypted connection. Unicode symbols are rendered on your Mac; unavailable iPhone fonts use a monospace fallback."
                    ).font(.footnote).foregroundStyle(.secondary)
                }
                Section("Mac config.txt") {
                    Text(link.configuration.contents.isEmpty ? "Using defaults" : link.configuration.contents)
                        .font(.system(.caption, design: .monospaced)).textSelection(.enabled)
                }
            }.navigationTitle("Mac Configuration")
                .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { showConfig = false } } }
        }
    }
    @ViewBuilder private var banner: some View {
        if screen.exited {
            Label("The program ended.", systemImage: "stop.circle").bannerStyle()
        } else if !screen.live {
            Label(
                link.state == .offline ? "\(link.device.name) is offline" : "Connecting…",
                systemImage: "wifi.exclamationmark"
            ).bannerStyle()
        } else if let error = link.lastError {
            Text(error).bannerStyle()
        } else if mouseMode {
            Text("Mouse mode · Tap to click, drag to select in the program").bannerStyle()
        }
    }
    private func switchTo(_ pane: RemotePane) {
        guard pane.id != screen.paneID else { return }
        link.close(screen)
        screen = link.open(pane)
        terminalView = nil
    }
    private func toggleFit() {
        if screen.fitted {
            link.send(.resize(stream: screen.stream, cols: nil, rows: nil))
            screen.fitted = false
        } else {
            screen.fitted = true
            terminalView?.resetFit()
        }
    }
    static func color(_ rgba: UInt32) -> UIColor {
        UIColor(
            red: CGFloat(rgba >> 24 & 255) / 255, green: CGFloat(rgba >> 16 & 255) / 255,
            blue: CGFloat(rgba >> 8 & 255) / 255, alpha: 1)
    }
}
extension View {
    fileprivate func bannerStyle() -> some View {
        font(.footnote.weight(.medium)).padding(.horizontal, 12).padding(.vertical, 6)
            .background(.ultraThinMaterial, in: Capsule()).padding(.top, 8)
    }
}
