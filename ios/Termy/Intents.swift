import AppIntents
import Foundation

/// A paired computer, for Shortcuts and Siri.
struct ComputerEntity: AppEntity {
    static let typeDisplayRepresentation: TypeDisplayRepresentation = "Computer"
    static let defaultQuery = ComputerQuery()

    let id: UUID
    let name: String

    var displayRepresentation: DisplayRepresentation {
        DisplayRepresentation(title: "\(name)", image: .init(systemName: "desktopcomputer"))
    }
}

struct ComputerQuery: EntityQuery {
    func entities(for identifiers: [UUID]) async throws -> [ComputerEntity] {
        await MainActor.run {
            AppModel.shared.hosts.filter { identifiers.contains($0.id) }.map {
                ComputerEntity(id: $0.id, name: $0.name)
            }
        }
    }

    func suggestedEntities() async throws -> [ComputerEntity] {
        await MainActor.run { AppModel.shared.hosts.map { ComputerEntity(id: $0.id, name: $0.name) } }
    }
}

struct OpenComputerIntent: AppIntent {
    static let title: LocalizedStringResource = "Open Computer"
    static let description = IntentDescription("Opens a paired computer's sessions in Termy.")
    static let openAppWhenRun = true

    @Parameter(title: "Computer") var computer: ComputerEntity

    static var parameterSummary: some ParameterSummary { Summary("Open \(\.$computer)") }

    @MainActor func perform() async throws -> some IntentResult {
        AppModel.shared.open(.host(computer.id, pane: nil))
        return .result()
    }
}

/// Types a command into a new tab on the Mac and runs it.
///
/// Only a Shortcut the user built can do this; `termy://` links never run anything.
struct RunCommandIntent: AppIntent {
    static let title: LocalizedStringResource = "Run Command on Computer"
    static let description = IntentDescription(
        "Opens a new tab in a Termy Next session on your Mac and runs a command in it.")
    static let openAppWhenRun = true

    @Parameter(title: "Command", inputOptions: String.IntentInputOptions(capitalizationType: .none, autocorrect: false))
    var command: String
    @Parameter(title: "Computer") var computer: ComputerEntity
    @Parameter(title: "Session", default: "Shortcuts") var session: String

    static var parameterSummary: some ParameterSummary {
        Summary("Run \(\.$command) on \(\.$computer)") { \.$session }
    }

    @MainActor func perform() async throws -> some IntentResult {
        let command = command.trimmingCharacters(in: .newlines)
        guard !command.isEmpty else { throw $command.needsValueError("What should run?") }
        let name = session.trimmingCharacters(in: .whitespacesAndNewlines)
        AppModel.shared.route = Route(
            host: computer.id, command: command, sessionName: name.isEmpty ? "Shortcuts" : name)
        return .result()
    }
}

struct TermyShortcuts: AppShortcutsProvider {
    static var appShortcuts: [AppShortcut] {
        AppShortcut(
            intent: OpenComputerIntent(),
            phrases: ["Open \(\.$computer) in \(.applicationName)", "Connect to \(\.$computer) with \(.applicationName)"],
            shortTitle: "Open Computer", systemImageName: "desktopcomputer")
        AppShortcut(
            intent: RunCommandIntent(), phrases: ["Run a command with \(.applicationName)"],
            shortTitle: "Run Command", systemImageName: "terminal")
    }
}
