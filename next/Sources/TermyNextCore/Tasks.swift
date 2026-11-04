import Foundation

/// A task from the config (`task.<name>.command = …`): a shell command that
/// runs in a new tab. Same keys as Termy; see docs/configuration.md.
public struct TaskSpec: Equatable, Decodable, Sendable {
    public var name: String
    public var command: String
    /// Only offered in the session with this name. Termy shows such a task
    /// while that saved layout is active; Termy Next's sessions play that role.
    public var layout: String?
    public var workingDir: String?

    public init(name: String, command: String, layout: String? = nil, workingDir: String? = nil) {
        self.name = name
        self.command = command
        self.layout = layout
        self.workingDir = workingDir
    }

    enum CodingKeys: String, CodingKey {
        case name, command, layout
        case workingDir = "working_dir"
    }

    /// Tasks from libtermy's `termy_config_tasks_json`; nothing for bad JSON.
    public static func decodeList(_ json: Data) -> [TaskSpec] {
        (try? JSONDecoder().decode([TaskSpec].self, from: json)) ?? []
    }

    /// The tasks to offer in `session`: unscoped ones and those scoped to it.
    public static func visible(_ tasks: [TaskSpec], inSession session: String) -> [TaskSpec] {
        tasks.filter { task in
            guard let layout = task.layout else { return true }
            return layout.caseInsensitiveCompare(session) == .orderedSame
        }
    }

    /// The directory the task's tab starts in. `~` is the home folder and a
    /// relative path is taken from `base`, the focused pane's directory.
    /// Falls back to `base` when the directory doesn't exist.
    public func resolvedDirectory(base: String?, home: String = NSHomeDirectory(),
                                  isDirectory: (String) -> Bool = TaskSpec.directoryExists) -> String? {
        guard let raw = workingDir?.trimmingCharacters(in: .whitespaces), !raw.isEmpty else { return base }
        let path: String
        if raw == "~" {
            path = home
        } else if raw.hasPrefix("~/") {
            path = (home as NSString).appendingPathComponent(String(raw.dropFirst(2)))
        } else if raw.hasPrefix("/") {
            path = raw
        } else if let base {
            path = (base as NSString).appendingPathComponent(raw)
        } else {
            path = (home as NSString).appendingPathComponent(raw)
        }
        let standardized = (path as NSString).standardizingPath
        return isDirectory(standardized) ? standardized : base
    }

    /// The text typed into the new tab's shell: the command and Return.
    public var input: String? {
        let trimmed = command.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed + "\r"
    }

    public static func directoryExists(_ path: String) -> Bool {
        var isDir: ObjCBool = false
        return FileManager.default.fileExists(atPath: path, isDirectory: &isDir) && isDir.boolValue
    }
}
