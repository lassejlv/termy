// Native macOS Settings window.
//
// Rust owns the settings model and every config write. This file only draws
// the model it is handed as JSON (`termy_native_settings_show` / `_update`)
// and reports edits back through the C callback as small JSON events:
//
//   {"kind":"set","key":"font_size","value":"14"}
//   {"kind":"reset","key":"red"}
//   {"kind":"open_section","section":"keybindings"}
//
// Every entry point runs on the main thread.

import AppKit
import SwiftUI

public typealias TermySettingsCallback = @convention(c) (UnsafePointer<CChar>) -> Void

// MARK: - Model

struct SettingsModel: Decodable {
    var pages: [SettingsPage]
}

struct SettingsPage: Decodable, Identifiable, Hashable {
    let id: String
    let title: String
    let symbol: String
    /// Drawn by Termy's own Settings window; selecting it opens that window.
    let external: Bool
    let groups: [SettingsGroup]
    let themes: ThemesPageModel?
}

struct ThemesPageModel: Decodable, Hashable {
    /// Whether the theme follows the system appearance (light and dark slots).
    let followsSystem: Bool
    /// The system appearance now: "light" or "dark".
    let appearance: String
    let theme: String
    let themeLight: String
    let themeDark: String
    let themes: [ThemeCardModel]
}

struct ThemeCardModel: Decodable, Hashable, Identifiable {
    let id: String
    let name: String
    let background: String
    let foreground: String
    /// Red, green, yellow, blue, magenta and cyan.
    let accents: [String]
    let dark: Bool
}

struct SettingsGroup: Decodable, Identifiable, Hashable {
    let id: String
    let title: String
    let rows: [SettingsRow]
}

struct SettingsChoice: Decodable, Hashable {
    let value: String
    let label: String
}

struct SettingsRow: Decodable, Identifiable, Hashable {
    let key: String
    let title: String
    let description: String
    let keywords: [String]
    /// toggle, picker, text, number, slider, color or link.
    let control: String
    let value: String
    let choices: [SettingsChoice]?
    let min: Double?
    let max: Double?
    let step: Double?
    /// Colors only: whether the value overrides the theme.
    let overridden: Bool?
    /// Links only: the text shown for the value, and the page it opens.
    let display: String?
    let target: String?
    let error: String?

    var id: String { key }

    func matches(_ query: String) -> Bool {
        let query = query.lowercased()
        return title.lowercased().contains(query)
            || description.lowercased().contains(query)
            || key.lowercased().contains(query)
            || keywords.contains { $0.lowercased().contains(query) }
    }
}

final class SettingsStore: ObservableObject {
    @Published var model: SettingsModel
    @Published var selection: String?
    /// The settings search; showing a page clears it.
    @Published var query = ""
    private let callback: TermySettingsCallback
    private var pendingColors: [String: DispatchWorkItem] = [:]

    init(model: SettingsModel, callback: @escaping TermySettingsCallback) {
        self.model = model
        self.callback = callback
        self.selection = model.pages.first { !$0.external }?.id
    }

    var localPages: [SettingsPage] { model.pages.filter { !$0.external } }
    var externalPages: [SettingsPage] { model.pages.filter { $0.external } }

    func select(_ id: String) {
        guard let page = model.pages.first(where: { $0.id == id }) else { return }
        if page.external {
            send(["kind": "open_section", "section": page.id])
        } else {
            query = ""
            selection = page.id
        }
    }

    func set(_ key: String, _ value: String) {
        send(["kind": "set", "key": key, "value": value])
    }

    /// The color panel reports every drag step; only the last one is written.
    func setColor(_ key: String, _ value: String) {
        pendingColors[key]?.cancel()
        let work = DispatchWorkItem { [weak self] in
            self?.pendingColors[key] = nil
            self?.set(key, value)
        }
        pendingColors[key] = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.15, execute: work)
    }

    /// Ask for the model again, after something Rust cannot see changed
    /// (the system appearance).
    func requestRefresh() {
        send(["kind": "refresh"])
    }

    func reset(_ key: String) {
        pendingColors.removeValue(forKey: key)?.cancel()
        send(["kind": "reset", "key": key])
    }

    private func send(_ event: [String: String]) {
        guard let data = try? JSONEncoder().encode(event),
            let json = String(data: data, encoding: .utf8)
        else { return }
        json.withCString { callback($0) }
    }
}

// MARK: - Views

@available(macOS 13.0, *)
struct SettingsRootView: View {
    @ObservedObject var store: SettingsStore

    var body: some View {
        NavigationSplitView {
            List(selection: Binding(get: { store.selection }, set: { id in
                if let id { store.select(id) }
            })) {
                Section {
                    ForEach(store.localPages) { page in
                        Label(page.title, systemImage: page.symbol).tag(page.id)
                    }
                }
                // Buttons, not selectable rows: these open Termy's own
                // Settings window and never become the shown page.
                Section("More") {
                    ForEach(store.externalPages) { page in
                        Button {
                            store.select(page.id)
                        } label: {
                            Label {
                                HStack {
                                    Text(page.title)
                                    Spacer()
                                    Image(systemName: "arrow.up.forward.app")
                                        .foregroundStyle(.tertiary)
                                }
                            } icon: {
                                Image(systemName: page.symbol)
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
            .navigationSplitViewColumnWidth(min: 180, ideal: 200, max: 260)
        } detail: {
            detail
        }
        .searchable(text: $store.query, placement: .sidebar, prompt: "Search")
    }

    @ViewBuilder
    private var detail: some View {
        let trimmed = store.query.trimmingCharacters(in: .whitespaces)
        if !trimmed.isEmpty {
            searchResults(trimmed)
        } else if let page = store.localPages.first(where: { $0.id == store.selection }),
            let themes = page.themes
        {
            ThemesView(page: themes, store: store)
        } else if let page = store.localPages.first(where: { $0.id == store.selection }) {
            Form {
                ForEach(page.groups) { group in
                    Section(group.title) {
                        ForEach(group.rows) { row in
                            SettingsRowView(row: row, store: store)
                        }
                    }
                }
            }
            .formStyle(.grouped)
            .navigationTitle(page.title)
        } else {
            Text("Select a section").foregroundStyle(.secondary)
        }
    }

    @ViewBuilder
    private func searchResults(_ query: String) -> some View {
        let pages = store.localPages.compactMap { page -> (SettingsPage, [SettingsRow])? in
            let rows = page.groups.flatMap(\.rows).filter { $0.matches(query) }
            return rows.isEmpty ? nil : (page, rows)
        }
        if pages.isEmpty {
            Text("No settings match “\(query)”").foregroundStyle(.secondary)
        } else {
            Form {
                ForEach(pages, id: \.0.id) { page, rows in
                    Section(page.title) {
                        ForEach(rows) { row in
                            SettingsRowView(row: row, store: store)
                        }
                    }
                }
            }
            .formStyle(.grouped)
            .navigationTitle("Search")
        }
    }
}

@available(macOS 13.0, *)
struct RowLabel: View {
    let row: SettingsRow

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(row.title)
            if !row.description.isEmpty {
                Text(row.description)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let error = row.error {
                Text(error)
                    .font(.caption)
                    .foregroundStyle(.red)
            }
        }
    }
}

@available(macOS 13.0, *)
struct SettingsRowView: View {
    let row: SettingsRow
    let store: SettingsStore

    var body: some View {
        control
            .contextMenu {
                Button("Reset to Default") { store.reset(row.key) }
            }
    }

    @ViewBuilder
    private var control: some View {
        switch row.control {
        case "toggle":
            Toggle(isOn: Binding(
                get: { row.value == "true" },
                set: { store.set(row.key, $0 ? "true" : "false") }
            )) {
                RowLabel(row: row)
            }
            .toggleStyle(.switch)
        case "picker":
            Picker(selection: Binding(
                get: { row.value },
                set: { store.set(row.key, $0) }
            )) {
                ForEach(row.choices ?? [], id: \.value) { choice in
                    Text(choice.label).tag(choice.value)
                }
            } label: {
                RowLabel(row: row)
            }
        case "slider":
            SliderRow(row: row, store: store)
        case "color":
            ColorRow(row: row, store: store)
        case "link":
            LabeledContent {
                HStack {
                    Text(row.display ?? row.value)
                        .foregroundStyle(.secondary)
                    Button("Browse…") {
                        if let target = row.target { store.select(target) }
                    }
                }
            } label: {
                RowLabel(row: row)
            }
        default:
            TextRow(row: row, store: store)
        }
    }
}

/// Writes once the drag ends; the config reloads on every write.
@available(macOS 13.0, *)
struct SliderRow: View {
    let row: SettingsRow
    let store: SettingsStore
    @State private var value: Double = 0
    @State private var editing = false

    var body: some View {
        LabeledContent {
            HStack {
                Slider(
                    value: $value,
                    in: (row.min ?? 0)...(row.max ?? 1),
                    step: row.step ?? 0.01
                ) { editing in
                    self.editing = editing
                    if !editing { commit() }
                }
                .frame(maxWidth: 200)
                Text(formatted)
                    .monospacedDigit()
                    .foregroundStyle(.secondary)
                    .frame(width: 44, alignment: .trailing)
            }
        } label: {
            RowLabel(row: row)
        }
        .onAppear { value = Double(row.value) ?? 0 }
        .onChange(of: row.value) { newValue in
            if !editing { value = Double(newValue) ?? value }
        }
        // Keyboard and VoiceOver changes come without a drag to end.
        .onChange(of: value) { _ in
            if !editing { commit() }
        }
    }

    private var formatted: String {
        String(format: (row.step ?? 0.01) < 0.1 ? "%.2f" : "%.1f", value)
    }

    private func commit() {
        let text = String(format: "%.3f", value)
        if Double(text) != Double(row.value) {
            store.set(row.key, text)
        }
    }
}

/// Writes on Return or when the field loses focus.
@available(macOS 13.0, *)
struct TextRow: View {
    let row: SettingsRow
    let store: SettingsStore
    @State private var text = ""
    @FocusState private var focused: Bool

    var body: some View {
        LabeledContent {
            TextField(row.title, text: $text)
                .labelsHidden()
                .textFieldStyle(.roundedBorder)
                .multilineTextAlignment(row.control == "number" ? .trailing : .leading)
                .frame(width: row.control == "number" ? 90 : 220)
                .focused($focused)
                .onSubmit(commit)
        } label: {
            RowLabel(row: row)
        }
        .onAppear { text = row.value }
        // The value only changes after an edit lands (ours or elsewhere);
        // show its canonical spelling.
        .onChange(of: row.value) { newValue in text = newValue }
        .onChange(of: focused) { isFocused in
            if !isFocused { commit() }
        }
    }

    private var isUnchanged: Bool {
        if row.control == "number", let typed = Double(text), let current = Double(row.value) {
            return typed == current
        }
        return text == row.value
    }

    private func commit() {
        if isUnchanged {
            // Show "14.5" again after "14.50", and retry an edit that failed
            // even when it now matches the saved value.
            if row.error != nil {
                store.set(row.key, row.value)
            }
            text = row.value
            return
        }
        store.set(row.key, text)
    }
}

@available(macOS 13.0, *)
struct ColorRow: View {
    let row: SettingsRow
    let store: SettingsStore

    var body: some View {
        LabeledContent {
            HStack(spacing: 8) {
                if row.overridden == true {
                    Button("Reset") { store.reset(row.key) }
                        .buttonStyle(.link)
                        .help("Use the theme's color")
                }
                ColorPicker(
                    row.title,
                    selection: Binding(
                        get: { Color(hex: row.value) },
                        set: { store.setColor(row.key, $0.hexString) }
                    ),
                    supportsOpacity: false
                )
                .labelsHidden()
            }
        } label: {
            RowLabel(row: row)
        }
    }
}

// MARK: - Themes

@available(macOS 13.0, *)
struct ThemesView: View {
    let page: ThemesPageModel
    let store: SettingsStore
    @State private var query = ""
    @State private var tone = ThemeTone.all
    /// The slot a click sets while the theme follows the system appearance;
    /// `nil` until chosen, which means the current appearance.
    @State private var slot: ThemeSlot?

    enum ThemeTone: String, CaseIterable, Identifiable {
        case all = "All"
        case dark = "Dark"
        case light = "Light"
        var id: Self { self }
    }

    enum ThemeSlot: String, CaseIterable, Identifiable {
        case light = "Light"
        case dark = "Dark"
        var id: Self { self }
    }

    private var activeSlot: ThemeSlot {
        slot ?? (page.appearance == "light" ? .light : .dark)
    }

    private var visibleThemes: [ThemeCardModel] {
        let query = query.trimmingCharacters(in: .whitespaces).lowercased()
        return page.themes.filter { theme in
            let toneMatches: Bool
            switch tone {
            case .all: toneMatches = true
            case .dark: toneMatches = theme.dark
            case .light: toneMatches = !theme.dark
            }
            return toneMatches
                && (query.isEmpty
                    || theme.name.lowercased().contains(query)
                    || theme.id.contains(query))
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 12) {
                NativeSearchField(text: $query, prompt: "Search \(page.themes.count) themes")
                    .frame(minWidth: 160, maxWidth: 260)
                Picker("Show", selection: $tone) {
                    ForEach(ThemeTone.allCases) { Text($0.rawValue).tag($0) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .fixedSize()
                Spacer(minLength: 0)
                if page.followsSystem {
                    Picker("Apply to", selection: Binding(
                        get: { activeSlot },
                        set: { slot = $0 }
                    )) {
                        ForEach(ThemeSlot.allCases) { Text($0.rawValue).tag($0) }
                    }
                    .pickerStyle(.segmented)
                    .fixedSize()
                    .help("Which appearance a click sets the theme for")
                }
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 12)
            Divider()
            if visibleThemes.isEmpty {
                Spacer()
                Text("No themes match").foregroundStyle(.secondary)
                Spacer()
            } else {
                ScrollView {
                    LazyVGrid(
                        columns: [GridItem(.adaptive(minimum: 160, maximum: 240), spacing: 14)],
                        spacing: 16
                    ) {
                        ForEach(visibleThemes) { theme in
                            Button {
                                apply(theme)
                            } label: {
                                ThemeCardView(
                                    theme: theme,
                                    selected: isSelected(theme),
                                    badge: badge(theme)
                                )
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    .padding(20)
                }
            }
        }
        .navigationTitle("Themes")
    }

    private func isSelected(_ theme: ThemeCardModel) -> Bool {
        guard page.followsSystem else { return theme.id == page.theme }
        return theme.id == (activeSlot == .light ? page.themeLight : page.themeDark)
    }

    /// While following the system, which slots use this theme.
    private func badge(_ theme: ThemeCardModel) -> String? {
        guard page.followsSystem else { return nil }
        switch (theme.id == page.themeLight, theme.id == page.themeDark) {
        case (true, true): return "Light & Dark"
        case (true, false): return "Light"
        case (false, true): return "Dark"
        case (false, false): return nil
        }
    }

    private func apply(_ theme: ThemeCardModel) {
        let key: String
        if page.followsSystem {
            key = activeSlot == .light ? "theme_light" : "theme_dark"
        } else {
            key = "theme"
        }
        store.set(key, theme.id)
    }
}

@available(macOS 13.0, *)
struct ThemeCardView: View {
    let theme: ThemeCardModel
    let selected: Bool
    let badge: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            VStack(alignment: .leading, spacing: 8) {
                HStack(spacing: 0) {
                    Text("~ ").foregroundColor(Color(hex: theme.accents[3]))
                    Text("echo hello").foregroundColor(Color(hex: theme.foreground))
                }
                .font(.system(size: 11, design: .monospaced))
                HStack(spacing: 5) {
                    ForEach(Array(theme.accents.enumerated()), id: \.offset) { accent in
                        Circle()
                            .fill(Color(hex: accent.element))
                            .frame(width: 10, height: 10)
                    }
                }
            }
            .frame(maxWidth: .infinity, minHeight: 56, alignment: .topLeading)
            .padding(10)
            .background(Color(hex: theme.background))
            .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .strokeBorder(
                        selected ? Color.accentColor : Color.primary.opacity(0.12),
                        lineWidth: selected ? 2.5 : 1
                    )
            )
            HStack(spacing: 4) {
                Text(theme.name)
                    .font(.callout)
                    .lineLimit(1)
                    .truncationMode(.tail)
                Spacer(minLength: 0)
                if let badge {
                    Text(badge)
                        .font(.caption2)
                        .padding(.horizontal, 5)
                        .padding(.vertical, 1)
                        .background(.quaternary, in: Capsule())
                }
                if selected {
                    Image(systemName: "checkmark.circle.fill")
                        .foregroundStyle(Color.accentColor)
                }
            }
        }
        .contentShape(Rectangle())
        .help(theme.name)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(theme.name)
        .accessibilityAddTraits(selected ? [.isButton, .isSelected] : .isButton)
    }
}

/// AppKit's search field, for a search box inside a page.
struct NativeSearchField: NSViewRepresentable {
    @Binding var text: String
    let prompt: String

    func makeNSView(context: Context) -> NSSearchField {
        let field = NSSearchField()
        field.placeholderString = prompt
        field.delegate = context.coordinator
        return field
    }

    func updateNSView(_ field: NSSearchField, context: Context) {
        field.placeholderString = prompt
        if field.stringValue != text {
            field.stringValue = text
        }
    }

    func makeCoordinator() -> Coordinator {
        Coordinator(text: $text)
    }

    final class Coordinator: NSObject, NSSearchFieldDelegate {
        private var text: Binding<String>

        init(text: Binding<String>) {
            self.text = text
        }

        func controlTextDidChange(_ notification: Notification) {
            guard let field = notification.object as? NSSearchField else { return }
            text.wrappedValue = field.stringValue
        }
    }
}

extension Color {
    init(hex: String) {
        let digits = hex.trimmingCharacters(in: CharacterSet(charactersIn: "#"))
        let value = UInt32(digits, radix: 16) ?? 0
        self.init(
            .sRGB,
            red: Double((value >> 16) & 0xFF) / 255,
            green: Double((value >> 8) & 0xFF) / 255,
            blue: Double(value & 0xFF) / 255
        )
    }

    var hexString: String {
        let color = NSColor(self).usingColorSpace(.sRGB) ?? .black
        let channel = { (value: CGFloat) in Int((min(max(value, 0), 1) * 255).rounded()) }
        return String(
            format: "#%02x%02x%02x",
            channel(color.redComponent),
            channel(color.greenComponent),
            channel(color.blueComponent)
        )
    }
}

// MARK: - Window

/// Closes itself on Cmd-W. The main menu binds Cmd-W to Termy's "Close Pane
/// or Tab", which would act on a terminal window instead.
final class SettingsNSWindow: NSWindow {
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        let modifiers = event.modifierFlags.intersection([.command, .shift, .option, .control])
        if modifiers == .command, event.charactersIgnoringModifiers?.lowercased() == "w" {
            performClose(nil)
            return true
        }
        return super.performKeyEquivalent(with: event)
    }
}

@available(macOS 13.0, *)
final class SettingsWindowController: NSObject, NSWindowDelegate {
    let store: SettingsStore
    let window: NSWindow
    private var appearanceObservation: NSKeyValueObservation?

    init(model: SettingsModel, callback: @escaping TermySettingsCallback) {
        store = SettingsStore(model: model, callback: callback)
        window = SettingsNSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 780, height: 580),
            styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        super.init()
        window.title = "Settings"
        window.isReleasedWhenClosed = false
        window.minSize = NSSize(width: 640, height: 420)
        window.toolbar = NSToolbar(identifier: "TermySettings")
        window.toolbarStyle = .unified
        window.contentViewController = NSHostingController(rootView: SettingsRootView(store: store))
        window.delegate = self
        if !window.setFrameUsingName("TermySettings") {
            window.center()
        }
        window.setFrameAutosaveName("TermySettings")
        // Colors and the Themes page depend on the system appearance.
        appearanceObservation = window.observe(\.effectiveAppearance) { [weak self] _, _ in
            DispatchQueue.main.async { self?.store.requestRefresh() }
        }
    }

    /// Commit a field still being edited: closing does not end editing.
    func windowWillClose(_ notification: Notification) {
        window.makeFirstResponder(nil)
    }
}

private var activeController: AnyObject?

private func decodeModel(_ json: UnsafePointer<CChar>) -> SettingsModel? {
    let data = Data(String(cString: json).utf8)
    let decoder = JSONDecoder()
    decoder.keyDecodingStrategy = .convertFromSnakeCase
    do {
        return try decoder.decode(SettingsModel.self, from: data)
    } catch {
        NSLog("Termy: invalid native settings model: \(error)")
        return nil
    }
}

// MARK: - C entry points

@_cdecl("termy_native_settings_supported")
public func termyNativeSettingsSupported() -> Bool {
    if #available(macOS 13.0, *) {
        return true
    }
    return false
}

/// Show (or raise) the window with `model`, on `section` when it is not null.
@_cdecl("termy_native_settings_show")
public func termyNativeSettingsShow(
    _ model: UnsafePointer<CChar>,
    _ section: UnsafePointer<CChar>?,
    _ callback: @escaping TermySettingsCallback
) {
    guard #available(macOS 13.0, *), let model = decodeModel(model) else { return }
    let controller: SettingsWindowController
    if let existing = activeController as? SettingsWindowController {
        controller = existing
        controller.store.model = model
    } else {
        controller = SettingsWindowController(model: model, callback: callback)
        activeController = controller
    }
    if let section {
        controller.store.select(String(cString: section))
    }
    controller.window.makeKeyAndOrderFront(nil)
    NSApp.activate(ignoringOtherApps: true)
}

/// Replace the shown model, after a config change.
@_cdecl("termy_native_settings_update")
public func termyNativeSettingsUpdate(_ model: UnsafePointer<CChar>) {
    guard #available(macOS 13.0, *),
        let controller = activeController as? SettingsWindowController,
        let model = decodeModel(model)
    else { return }
    controller.store.model = model
}

@_cdecl("termy_native_settings_is_visible")
public func termyNativeSettingsIsVisible() -> Bool {
    guard #available(macOS 13.0, *),
        let controller = activeController as? SettingsWindowController
    else { return false }
    // A minimized window still shows its model when restored.
    return controller.window.isVisible || controller.window.isMiniaturized
}

@_cdecl("termy_native_settings_close")
public func termyNativeSettingsClose() {
    guard #available(macOS 13.0, *),
        let controller = activeController as? SettingsWindowController
    else { return }
    controller.window.close()
}
