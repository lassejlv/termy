import Foundation

/// The config and support files belonging to one Termy Next instance.
public struct NextPaths: Sendable {
    public let configurationDirectory: URL
    public let supportDirectory: URL

    public static var current: NextPaths {
        NextPaths(
            environment: ProcessInfo.processInfo.environment,
            homeDirectory: FileManager.default.homeDirectoryForCurrentUser,
            applicationSupportDirectory: FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        )
    }

    public init(environment: [String: String], homeDirectory: URL, applicationSupportDirectory: URL) {
        // TERMY_NEXT_HOME keeps a test instance's host and layout apart from yours.
        if let home = environment["TERMY_NEXT_HOME"] {
            let root = URL(fileURLWithPath: home)
            configurationDirectory = root.appendingPathComponent("config", isDirectory: true)
            supportDirectory = root.appendingPathComponent("support", isDirectory: true)
        } else {
            configurationDirectory = homeDirectory.appendingPathComponent(".config/termy-next", isDirectory: true)
            supportDirectory = applicationSupportDirectory.appendingPathComponent("TermyNext", isDirectory: true)
        }
    }

    public var multiplexerDirectory: URL { supportDirectory.appendingPathComponent("multiplexer", isDirectory: true) }
    public var shellDirectory: URL { supportDirectory.appendingPathComponent("shell", isDirectory: true) }
    public var layoutFile: URL { supportDirectory.appendingPathComponent("layout.json") }
}
