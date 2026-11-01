import Foundation

public enum Naming {
    static let adjectives = [
        "amber", "brisk", "calm", "dusty", "eager", "faint", "gentle", "hollow", "icy", "jolly",
        "keen", "lucid", "mellow", "nimble", "quiet", "rapid", "shy", "tidy", "vivid", "witty",
    ]
    static let nouns = [
        "orchard", "harbor", "meadow", "comet", "summit", "canyon", "lagoon", "forest", "glacier", "prairie",
        "delta", "island", "beacon", "ridge", "valley", "willow", "falcon", "pebble", "river", "tundra",
    ]

    /// A two-word session name such as "shy-orchard" that is not in `taken`.
    public static func sessionName(taken: Set<String>, random: () -> Int = { Int.random(in: 0..<Int.max) }) -> String {
        for _ in 0..<64 {
            let name = "\(adjectives[random() % adjectives.count])-\(nouns[random() % nouns.count])"
            if !taken.contains(name) { return name }
        }
        return uniqued("session", taken: taken)
    }

    /// `base`, or `base-2`, `base-3`... whichever is free.
    public static func uniqued(_ base: String, taken: Set<String>) -> String {
        guard taken.contains(base) else { return base }
        var n = 2
        while taken.contains("\(base)-\(n)") { n += 1 }
        return "\(base)-\(n)"
    }

    /// `path` with the home directory written as `~`.
    public static func abbreviatingHome(_ path: String, home: String = NSHomeDirectory()) -> String {
        guard !home.isEmpty else { return path }
        if path == home { return "~" }
        if path.hasPrefix(home + "/") { return "~" + path.dropFirst(home.count) }
        return path
    }

    /// The path from an OSC 7 report (`file://host/path`) or a plain path.
    public static func pathFromWorkingDirectoryReport(_ report: String) -> String? {
        if report.hasPrefix("/") { return report }
        guard let url = URL(string: report), url.isFileURL else { return nil }
        let path = url.path
        return path.isEmpty ? nil : path
    }
}

/// What a pane header and tab show: the program in front and where it runs.
public struct PaneLabel: Hashable, Sendable {
    public var name: String
    public var detail: String?

    public init(name: String, detail: String?) {
        self.name = name
        self.detail = detail
    }

    /// Prefers a title the program set; otherwise the foreground process and its directory.
    ///
    /// Shell prompts often set titles like `user@host: ~/dir` or just the
    /// directory. Those say nothing the process and cwd don't, so they fall
    /// back to the process view.
    public static func make(title: String?, process: String?, directory: String?, home: String = NSHomeDirectory()) -> PaneLabel {
        let dir = directory.map { Naming.abbreviatingHome($0, home: home) }
        let fallback = PaneLabel(name: process ?? "shell", detail: dir)
        guard let raw = title?.trimmingCharacters(in: .whitespaces), !raw.isEmpty else { return fallback }
        if raw == dir || raw == directory || isPromptTitle(raw) { return fallback }
        if let process, raw == process { return fallback }
        return PaneLabel(name: raw, detail: dir)
    }

    static func isPromptTitle(_ title: String) -> Bool {
        // user@host: path, user@host:path, or a bare path.
        if title.hasPrefix("~") || title.hasPrefix("/") { return true }
        guard let at = title.firstIndex(of: "@"), let colon = title.firstIndex(of: ":"), at < colon else { return false }
        return !title[..<at].contains(" ")
    }
}
