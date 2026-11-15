import SwiftUI
import TermyRemoteKit

struct TerminalScreenView: View {
    let link: HostLink
    @State private var screen: TerminalScreen
    /// A second pane beside the first, on iPad and other wide windows.
    @State private var secondary: TerminalScreen?
    @State private var focusedSecondary = false
    @Environment(\.dismiss) private var dismiss
    @Environment(\.horizontalSizeClass) private var sizeClass
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var primaryView: TerminalUIView?
    @State private var secondaryView: TerminalUIView?
    @State private var showSearch = false
    @State private var query = ""
    @State private var mouseMode = false
    @State private var showConfig = false
    @State private var closeConfirmation = false
    /// Which way the next pane slides in.
    @State private var switchEdge = Edge.trailing
    @State private var snippets = SnippetStore.shared

    init(link: HostLink, screen: TerminalScreen) {
        self.link = link
        _screen = State(initialValue: screen)
    }

    private var split: Bool { sizeClass == .regular && secondary != nil }
    private var focused: TerminalScreen { split && focusedSecondary ? secondary ?? screen : screen }
    private var focusedView: TerminalUIView? { split && focusedSecondary ? secondaryView : primaryView }

    var body: some View {
        NavigationStack {
            HStack(spacing: 0) {
                terminal(screen, isSecondary: false)
                if split, let secondary {
                    Divider()
                    terminal(secondary, isSecondary: true)
                }
            }
            .background(Color(uiColor: Self.color(focused.configuration.background)))
            .safeAreaInset(edge: .top, spacing: 0) { paneStrip }
            .overlay(alignment: .top) { banner }
            .overlay(alignment: .bottomTrailing) {
                if focused.displayOffset > 0 {
                    Button {
                        link.send(.scrollToBottom(stream: focused.stream))
                    } label: {
                        Label("Live", systemImage: "arrow.down.to.line").font(.footnote.bold()).padding(10)
                    }.buttonStyle(.borderedProminent).padding()
                }
            }
            .navigationTitle(focused.title ?? "Terminal")
            .navigationBarTitleDisplayMode(.inline)
            .toolbarBackground(.visible, for: .navigationBar)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) { Button("Done") { dismiss() } }
                ToolbarItem(placement: .topBarTrailing) { menu }
            }
            .onChange(of: mouseMode) { _, value in
                primaryView?.mouseMode = value
                secondaryView?.mouseMode = value
            }
            .onChange(of: link.createdPaneID) { _, id in
                guard let id else { return }
                link.createdPaneID = nil
                switchTo(RemotePane(id: id, title: nil, workingDirectory: nil, exited: false), edge: .trailing)
            }
            .onChange(of: screen.live) { _, live in if live { primaryView?.resetFit() } }
            .onChange(of: secondary?.live) { _, live in if live == true { secondaryView?.resetFit() } }
            .onChange(of: sizeClass) { _, size in
                // A narrowed window keeps one pane.
                if size != .regular, let secondary { closeSecondary(secondary) }
            }
            .sheet(isPresented: $showSearch) { searchSheet }
            .sheet(isPresented: $showConfig) { configSheet }
            .confirmationDialog(
                "End this pane on your Mac?", isPresented: $closeConfirmation, titleVisibility: .visible
            ) {
                Button("End Pane", role: .destructive) {
                    link.send(.closePane(paneID: focused.paneID))
                    if split, let secondary {
                        if focusedSecondary { closeSecondary(secondary) } else { promoteSecondary(secondary) }
                    } else {
                        dismiss()
                    }
                }
            }
        }
        .onDisappear {
            link.close(screen)
            if let secondary { link.close(secondary) }
        }
        .preferredColorScheme(link.configuration.appearance == "light" ? .light : .dark)
    }

    // MARK: Panes

    private func terminal(_ screen: TerminalScreen, isSecondary: Bool) -> some View {
        ZStack {
            TerminalViewRepresentable(
                screen: screen, link: link, focusOnAppear: isSecondary == focusedSecondary || !split,
                onReady: { view in
                    DispatchQueue.main.async {
                        if isSecondary { secondaryView = view } else { primaryView = view }
                        view.mouseMode = mouseMode
                    }
                },
                onCommand: { handle($0) },
                onFocus: { if focusedSecondary != isSecondary { focusedSecondary = isSecondary } }
            )
            .id(screen.stream)
            .transition(reduceMotion ? .opacity : .push(from: switchEdge))
        }
        .clipped()
        .background(Color(uiColor: Self.color(screen.configuration.background)))
        .overlay {
            if split, focused === screen {
                Rectangle().strokeBorder(Color.accentColor.opacity(0.7), lineWidth: 2).allowsHitTesting(false)
            }
        }
    }

    /// The session's panes as chips; tap to switch, or swipe in from the screen's edges.
    @ViewBuilder private var paneStrip: some View {
        let panes = stripPanes
        if panes.count > 1 {
            ScrollViewReader { proxy in
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 6) {
                        ForEach(panes) { pane in
                            let current = pane.id == focused.paneID
                            let beside = split && pane.id == (focusedSecondary ? screen : secondary)?.paneID
                            Button {
                                switchTo(pane, edge: edge(toward: pane))
                            } label: {
                                Text(title(of: pane)).font(.caption.weight(current ? .semibold : .regular))
                                    .lineLimit(1).frame(maxWidth: 160)
                                    .padding(.horizontal, 10).padding(.vertical, 5)
                                    .background(
                                        current ? AnyShapeStyle(.tint.opacity(0.3)) : AnyShapeStyle(.quaternary),
                                        in: Capsule())
                                    .overlay { if beside { Capsule().strokeBorder(.tint.opacity(0.5)) } }
                            }
                            .buttonStyle(.plain)
                            .id(pane.id)
                            .accessibilityAddTraits(current ? .isSelected : [])
                        }
                    }
                    .padding(.horizontal, 12).padding(.vertical, 6)
                }
                .background(.bar)
                .onChange(of: focused.paneID) { _, id in
                    withAnimation(reduceMotion ? nil : .default) { proxy.scrollTo(id, anchor: .center) }
                }
            }
        }
    }

    /// The panes of the focused pane's session, or every pane when it has none.
    private var stripPanes: [RemotePane] {
        let all = link.orderedPanes
        guard let group = link.group(containing: focused.paneID) else { return all }
        let ids = Set(group.tabs.flatMap(\.paneIDs))
        return all.filter { ids.contains($0.id) }
    }

    private func title(of pane: RemotePane) -> String {
        if let title = pane.title, !title.isEmpty { return title }
        for group in link.sessions.groups {
            for (index, tab) in group.tabs.enumerated() where tab.paneIDs.contains(pane.id) {
                return tab.title ?? "Tab \(index + 1)"
            }
        }
        return "Shell"
    }

    private func edge(toward pane: RemotePane) -> Edge {
        let panes = link.orderedPanes
        guard let from = panes.firstIndex(where: { $0.id == focused.paneID }),
            let to = panes.firstIndex(where: { $0.id == pane.id })
        else { return .trailing }
        return to < from ? .leading : .trailing
    }

    private func step(by delta: Int) {
        let other = split ? (focusedSecondary ? screen : secondary)?.paneID : nil
        let panes = link.orderedPanes.filter { $0.id != other }
        guard panes.count > 1, let index = panes.firstIndex(where: { $0.id == focused.paneID }) else { return }
        let next = panes[(index + delta + panes.count) % panes.count]
        switchTo(next, edge: delta > 0 ? .trailing : .leading)
    }

    private func switchTo(_ pane: RemotePane, edge: Edge) {
        guard pane.id != focused.paneID else { return }
        // Tapping the pane shown beside this one just moves focus there.
        if split, let other = focusedSecondary ? screen : secondary, other.paneID == pane.id {
            focusedSecondary.toggle()
            _ = focusedView?.becomeFirstResponder()
            return
        }
        switchEdge = edge
        let old = focused
        let next = link.open(pane)
        withAnimation(reduceMotion ? nil : .snappy(duration: 0.28)) {
            if split && focusedSecondary {
                secondary = next
                secondaryView = nil
            } else {
                screen = next
                primaryView = nil
            }
        }
        link.close(old)
    }

    private func showBeside(_ pane: RemotePane) {
        if let secondary { link.close(secondary) }
        switchEdge = .trailing
        secondaryView = nil
        secondary = link.open(pane)
        focusedSecondary = true
    }

    private func closeSecondary(_ pane: TerminalScreen) {
        link.close(pane)
        secondary = nil
        secondaryView = nil
        focusedSecondary = false
        _ = primaryView?.becomeFirstResponder()
    }

    /// The left pane ended: the right one takes its place.
    private func promoteSecondary(_ pane: TerminalScreen) {
        link.close(screen)
        screen = pane
        primaryView = secondaryView
        secondary = nil
        secondaryView = nil
        focusedSecondary = false
    }

    private func handle(_ command: TerminalCommand) {
        switch command {
        case .find:
            showSearch = true
            _ = focusedView?.resignFirstResponder()
        case .newTab:
            if link.protocolVersion >= 2 { link.send(.createTab(inSessionContaining: focused.paneID)) }
        case .splitRight:
            if link.protocolVersion >= 2 { link.send(.splitPane(paneID: focused.paneID, horizontal: true)) }
        case .splitDown:
            if link.protocolVersion >= 2 { link.send(.splitPane(paneID: focused.paneID, horizontal: false)) }
        case .nextPane: step(by: 1)
        case .previousPane: step(by: -1)
        case .close: dismiss()
        }
    }

    // MARK: Menu

    private var menu: some View {
        Menu {
            Section("Switch Terminal") {
                ForEach(link.sessions.groups) { group in
                    Menu(group.name) {
                        ForEach(Array(group.tabs.enumerated()), id: \.offset) { index, tab in
                            ForEach(tab.paneIDs, id: \.self) { id in
                                if let pane = link.pane(id) {
                                    Button(pane.title ?? tab.title ?? "Tab \(index+1)") {
                                        switchTo(pane, edge: edge(toward: pane))
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if sizeClass == .regular {
                Section {
                    Menu("Show Pane Side by Side", systemImage: "rectangle.split.2x1") {
                        ForEach(link.orderedPanes.filter { $0.id != screen.paneID }) { pane in
                            Button(title(of: pane)) { showBeside(pane) }
                        }
                    }
                    if let secondary {
                        Button("Close Side Pane", systemImage: "xmark.rectangle") { closeSecondary(secondary) }
                    }
                }
            }
            if !snippets.snippets.isEmpty {
                Menu("Snippets", systemImage: "text.badge.plus") {
                    ForEach(snippets.snippets) { snippet in
                        Button(snippet.title, systemImage: snippet.runs ? "return" : "text.cursor") {
                            focusedView?.type(snippet)
                        }
                    }
                }
            }
            if link.protocolVersion >= 2 {
                Button("New Tab in This Session", systemImage: "plus") { handle(.newTab) }
                Button("Split on Mac Side by Side", systemImage: "rectangle.split.2x1") { handle(.splitRight) }
                Button("Split on Mac Above and Below", systemImage: "rectangle.split.1x2") { handle(.splitDown) }
                Button("Find in Scrollback", systemImage: "magnifyingglass") { handle(.find) }
            }
            Button(
                focused.fitted ? "Use Mac Grid Size" : "Fit to iPhone",
                systemImage: "arrow.up.left.and.arrow.down.right"
            ) { toggleFit() }
            Toggle("Mouse Mode", systemImage: "cursorarrow", isOn: $mouseMode)
            Button("Mac Configuration", systemImage: "slider.horizontal.3") { showConfig = true }
            Button("Paste", systemImage: "doc.on.clipboard") { focusedView?.paste() }
            Button("Show Keyboard", systemImage: "keyboard") { _ = focusedView?.becomeFirstResponder() }
            Button("Hide Keyboard", systemImage: "keyboard.chevron.compact.down") {
                _ = focusedView?.resignFirstResponder()
            }
            Button("End Pane", systemImage: "xmark.square", role: .destructive) { closeConfirmation = true }
        } label: {
            Image(systemName: "ellipsis.circle")
        }
        .accessibilityLabel("Terminal actions and tabs")
    }

    private var searchSheet: some View {
        NavigationStack {
            List {
                ForEach(Array(focused.searchMatches.enumerated()), id: \.offset) { _, match in
                    Button {
                        link.send(.revealSearch(stream: focused.stream, bufferRow: match.row))
                        showSearch = false
                    } label: {
                        VStack(alignment: .leading) {
                            Text("Line \(match.row+1)").font(.caption).foregroundStyle(.secondary)
                            Text(match.line).font(.system(.body, design: .monospaced)).lineLimit(3)
                        }
                    }
                }
                if !query.isEmpty, focused.searchMatches.isEmpty { Text("No matches").foregroundStyle(.secondary) }
            }
            .searchable(text: $query, prompt: "Search screen and history")
            .task(id: query) {
                do {
                    try await Task.sleep(for: .milliseconds(250))
                    try Task.checkCancellation()
                    link.send(.search(stream: focused.stream, query: query))
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
                    if let latency = focused.latency {
                        LabeledContent("Round Trip", value: "\(Int(latency * 1000)) ms")
                    }
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
        if focused.exited {
            Label("The program ended.", systemImage: "stop.circle").bannerStyle()
        } else if !focused.live {
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

    private func toggleFit() {
        if focused.fitted {
            link.send(.resize(stream: focused.stream, cols: nil, rows: nil))
            focused.fitted = false
        } else {
            focused.fitted = true
            focusedView?.resetFit()
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
