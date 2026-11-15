import PhotosUI
import SwiftUI
import TermyRemoteKit
import UniformTypeIdentifiers

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
    /// Something worth a moment's banner: a long command ended, output was copied.
    private struct Notice: Equatable {
        var text: String
        var symbol: String
        var tint: Color
        let id = UUID()
    }
    @State private var notice: Notice?
    @State private var dictating = false
    @State private var choosingPhoto = false
    @State private var photo: PhotosPickerItem?

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
            .onChange(of: screen.finishedCommand) { _, command in announce(command, from: screen) }
            .onChange(of: secondary?.finishedCommand) { _, command in
                if let secondary { announce(command, from: secondary) }
            }
            .onChange(of: secondary?.live) { _, live in if live == true { secondaryView?.resetFit() } }
            .onChange(of: sizeClass) { _, size in
                // A narrowed window keeps one pane.
                if size != .regular, let secondary { closeSecondary(secondary) }
            }
            .sheet(isPresented: $showSearch) { searchSheet }
            .sheet(isPresented: $dictating) {
                DictationSheet { command, run in
                    focusedView?.type(Snippet(name: "", command: command, runs: run))
                }
            }
            .photosPicker(isPresented: $choosingPhoto, selection: $photo, matching: .images)
            .onChange(of: photo) { _, item in
                guard let item else { return }
                photo = nil
                let target = focused
                Task {
                    guard let data = try? await item.loadTransferable(type: Data.self) else {
                        return show(Notice(text: "Couldn’t read that photo", symbol: "exclamationmark.triangle", tint: .orange))
                    }
                    send(data: data, name: nil, to: target)
                }
            }
            .onDrop(of: [.image], isTargeted: nil) { providers in
                guard let provider = providers.first else { return false }
                let target = focused
                let name = provider.suggestedName
                _ = provider.loadDataRepresentation(for: .image) { data, _ in
                    guard let data else { return }
                    Task { @MainActor in send(data: data, name: name, to: target) }
                }
                return true
            }
            .onChange(of: screen.uploadedPath) { _, path in uploaded(path, on: screen) }
            .onChange(of: secondary?.uploadedPath) { _, path in if let secondary { uploaded(path, on: secondary) } }
            .userActivity(RemoteHandoff.activityType, isActive: focused.live && link.state == .connected) { activity in
                // Termy Next opens the same pane on the Mac.
                activity.title = focused.title ?? "Terminal"
                activity.addUserInfoEntries(from: RemoteHandoff.userInfo(host: link.device.id, pane: focused.paneID))
                activity.isEligibleForHandoff = true
            }
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
                onFocus: { if focusedSecondary != isSecondary { focusedSecondary = isSecondary } },
                onPasteImage: { image in send(image: image, to: screen) }
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
                                HStack(spacing: 5) {
                                    CommandDot(state: pane.command)
                                    Text(title(of: pane)).font(.caption.weight(current ? .semibold : .regular))
                                        .lineLimit(1)
                                }
                                .frame(maxWidth: 160)
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
        case .previousPrompt, .nextPrompt:
            guard link.followsPrompts else { return }
            link.send(.jumpToPrompt(stream: focused.stream, forward: command == .nextPrompt))
        case .copyOutput(let row): copyOutput(promptBufferRow: row)
        case .dictate:
            _ = focusedView?.resignFirstResponder()
            dictating = true
        case .sendPhoto:
            guard link.followsPrompts else {
                return show(Notice(text: "Update Termy Next on your Mac to send photos", symbol: "info.circle", tint: .secondary))
            }
            choosingPhoto = true
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
            Section {
                Button("Speak a Command…", systemImage: "mic") { handle(.dictate) }
                if link.followsPrompts {
                    Button("Send Photo…", systemImage: "photo") { handle(.sendPhoto) }
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
            if link.followsPrompts {
                Section {
                    Button("Previous Prompt", systemImage: "chevron.up.2") { handle(.previousPrompt) }
                    Button("Next Prompt", systemImage: "chevron.down.2") { handle(.nextPrompt) }
                    Button("Copy Last Output", systemImage: "text.below.photo") {
                        handle(.copyOutput(promptBufferRow: nil))
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
        if let notice {
            Label(notice.text, systemImage: notice.symbol).foregroundStyle(notice.tint).bannerStyle()
                .transition(.move(edge: .top).combined(with: .opacity))
                .onTapGesture { self.notice = nil }
                .accessibilityAddTraits(.isButton)
        } else if focused.exited {
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

    /// Tells about a long command that finished in a pane on screen, with a haptic.
    private func announce(_ command: TerminalScreen.FinishedCommand?, from pane: TerminalScreen) {
        guard let command else { return }
        pane.dismissFinishedCommand()
        var text = CommandFormat.finished(command)
        if split, let title = pane.title { text = "\(title): \(text)" }
        show(
            Notice(
                text: text, symbol: command.failed ? "xmark.circle.fill" : "checkmark.circle.fill",
                tint: command.failed ? .red : .green))
        UINotificationFeedbackGenerator().notificationOccurred(command.failed ? .error : .success)
    }

    private func show(_ new: Notice) {
        withAnimation(reduceMotion ? nil : .snappy) { notice = new }
        UIAccessibility.post(notification: .announcement, argument: new.text)
        Task {
            try? await Task.sleep(for: .seconds(4))
            if notice?.id == new.id { withAnimation(reduceMotion ? nil : .snappy) { notice = nil } }
        }
    }

    /// Sends a photo into a pane: the Mac saves it and types its path, ready for Claude Code and friends.
    private func send(data: Data, name: String?, to pane: TerminalScreen) {
        Task.detached(priority: .userInitiated) {
            let prepared = ImageUpload.prepare(data, name: name)
            await MainActor.run { upload(prepared, to: pane) }
        }
    }

    private func send(image: UIImage, to pane: TerminalScreen) {
        guard link.followsPrompts else {
            return show(Notice(text: "Update Termy Next on your Mac to paste images", symbol: "info.circle", tint: .secondary))
        }
        let image = Handoff(image)
        Task.detached(priority: .userInitiated) {
            let prepared = ImageUpload.prepare(image.value, name: "pasted")
            await MainActor.run { upload(prepared, to: pane) }
        }
    }

    private func upload(_ prepared: (data: Data, name: String)?, to pane: TerminalScreen) {
        guard let prepared else {
            return show(Notice(text: "That image is too large to send", symbol: "exclamationmark.triangle", tint: .orange))
        }
        guard link.upload(prepared.data, name: prepared.name, to: pane) else {
            return show(Notice(text: "Update Termy Next on your Mac to send photos", symbol: "info.circle", tint: .secondary))
        }
        show(Notice(text: "Sending to your Mac…", symbol: "arrow.up.circle", tint: .accentColor))
    }

    private func uploaded(_ path: String?, on pane: TerminalScreen) {
        guard let path else { return }
        pane.uploadedPath = nil
        UINotificationFeedbackGenerator().notificationOccurred(.success)
        show(Notice(text: "Saved as \((path as NSString).lastPathComponent)", symbol: "photo.badge.checkmark", tint: .accentColor))
    }

    /// Copies a command's output from the Mac's history, which may reach far past the screen.
    private func copyOutput(promptBufferRow: Int?) {
        guard link.followsPrompts else {
            return show(Notice(text: "Update Termy Next on your Mac to copy output", symbol: "info.circle", tint: .secondary))
        }
        link.requestOutput(of: focused, promptBufferRow: promptBufferRow) { output in
            guard let output, !output.text.isEmpty else {
                return show(Notice(text: "No command output to copy", symbol: "info.circle", tint: .secondary))
            }
            UIPasteboard.general.string = output.text
            UINotificationFeedbackGenerator().notificationOccurred(.success)
            let lines = output.text.split(separator: "\n", omittingEmptySubsequences: false).count
            var text = "Copied \(lines) line\(lines == 1 ? "" : "s")"
            if let command = output.command, !command.isEmpty { text += " from \(command.prefix(30))" }
            if output.truncated { text += " (the end)" }
            show(Notice(text: text, symbol: "doc.on.doc.fill", tint: .accentColor))
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
