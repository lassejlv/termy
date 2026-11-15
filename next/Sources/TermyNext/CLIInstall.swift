import AppKit
import TermyNextCore

/// Installs `termy`, Termy Next's command-line tool, at `~/.local/bin/termy`
/// (where Termy's own Install CLI puts it) and puts that folder on the
/// shell's PATH. Panes have `termy` without it; this is for every other shell.
@MainActor
enum CLIInstall {
    private static let fm = FileManager.default
    private static var home: URL { fm.homeDirectoryForCurrentUser }
    static var directory: URL { home.appendingPathComponent(".local/bin", isDirectory: true) }
    static var target: URL { directory.appendingPathComponent("termy") }

    /// The script, pointing at this copy of the app.
    static var script: String {
        """
        #!/bin/sh
        # Termy Next's command-line tool, installed from Termy Next (Install CLI).
        # `termy --help` lists the commands.
        exec \(ShellQuote.quote(PluginPaths.executable)) --termy-cli "$@"

        """
    }

    /// The script is there and points at this app.
    static var isInstalled: Bool { (try? String(contentsOf: target, encoding: .utf8)) == script }

    /// Something else lives at the target, such as Termy's own CLI.
    private static var hasOtherFile: Bool {
        (try? fm.destinationOfSymbolicLink(atPath: target.path)) != nil
            || (fm.fileExists(atPath: target.path) && !isOurs)
    }

    private static var isOurs: Bool {
        (try? String(contentsOf: target, encoding: .utf8))?.contains("Termy Next's command-line tool") == true
    }

    /// Installs, asking first before replacing someone else's `termy`.
    /// With `quiet`, success shows no alert (the caller shows it instead).
    @discardableResult
    static func install(attachedTo window: NSWindow?, quiet: Bool = false) -> Bool {
        if hasOtherFile {
            let alert = NSAlert()
            alert.messageText = "Replace the existing termy command?"
            let existing = (try? fm.destinationOfSymbolicLink(atPath: target.path)) ?? target.path
            alert.informativeText = "\((target.path as NSString).abbreviatingWithTildeInPath) is already there (\((existing as NSString).abbreviatingWithTildeInPath)). Termy Next's replaces it."
            alert.addButton(withTitle: "Replace")
            alert.addButton(withTitle: "Cancel")
            guard alert.runModal() == .alertFirstButtonReturn else { return false }
        }
        do {
            try fm.createDirectory(at: directory, withIntermediateDirectories: true)
            if (try? fm.destinationOfSymbolicLink(atPath: target.path)) != nil || fm.fileExists(atPath: target.path) {
                try fm.removeItem(at: target)
            }
            try script.write(to: target, atomically: true, encoding: .utf8)
            try fm.setAttributes([.posixPermissions: 0o755], ofItemAtPath: target.path)
            let profile = try addToPath()
            guard !quiet else { return true }
            let alert = NSAlert()
            alert.messageText = "termy is installed"
            var info = "Try `termy --help` in a new shell. It's at \((target.path as NSString).abbreviatingWithTildeInPath)."
            if let profile {
                info += " \((directory.path as NSString).abbreviatingWithTildeInPath) was added to your PATH in \((profile.path as NSString).abbreviatingWithTildeInPath)."
            }
            alert.informativeText = info
            if let window { alert.beginSheetModal(for: window) } else { alert.runModal() }
            return true
        } catch {
            let alert = NSAlert(error: error)
            alert.messageText = "Couldn't install termy"
            if let window { alert.beginSheetModal(for: window) } else { alert.runModal() }
            return false
        }
    }

    /// Removes the script when it's Termy Next's.
    static func uninstall() {
        guard isOurs else { return }
        try? fm.removeItem(at: target)
    }

    // MARK: PATH

    private enum Shell { case zsh, bash, fish }

    private static var shell: Shell {
        switch (ProcessInfo.processInfo.environment["SHELL"].map { ($0 as NSString).lastPathComponent }) {
        case "bash": .bash
        case "fish": .fish
        default: .zsh
        }
    }

    /// The startup file the shell reads that the PATH block goes in.
    private static var profile: URL {
        switch shell {
        case .zsh:
            let zdotdir = ProcessInfo.processInfo.environment["ZDOTDIR"].map { URL(fileURLWithPath: $0) } ?? home
            return zdotdir.appendingPathComponent(".zshrc")
        case .bash:
            // Bash reads only the first of these that exists.
            let files = [".bash_profile", ".bash_login", ".profile"].map { home.appendingPathComponent($0) }
            return files.first { fm.fileExists(atPath: $0.path) } ?? files[0]
        case .fish:
            return home.appendingPathComponent(".config/fish/config.fish")
        }
    }

    /// Adds `~/.local/bin` to PATH in the shell's startup file, with the same
    /// marker Termy's Install CLI uses so the two never add it twice. Returns
    /// the file when it changed it.
    private static func addToPath() throws -> URL? {
        let file = profile
        let existing = (try? String(contentsOf: file, encoding: .utf8)) ?? ""
        let start = "# >>> termy cli path >>>"
        if existing.contains(start) || existing.contains(".local/bin") { return nil }
        let quoted = ShellQuote.quote(directory.path)
        let block: String
        switch shell {
        case .zsh, .bash:
            block = """
            \(start)
            # Added by Termy Next Install CLI
            TERMY_CLI_PATH=\(quoted)
            case ":$PATH:" in
              *":$TERMY_CLI_PATH:"*) ;;
              *) export PATH="$TERMY_CLI_PATH:$PATH" ;;
            esac
            # <<< termy cli path <<<
            """
        case .fish:
            block = """
            \(start)
            # Added by Termy Next Install CLI
            set -l termy_cli_path "\(directory.path)"
            if not contains -- $termy_cli_path $PATH
                set -gx PATH $termy_cli_path $PATH
            end
            # <<< termy cli path <<<
            """
        }
        try fm.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
        let separator = existing.isEmpty || existing.hasSuffix("\n\n") ? "" : existing.hasSuffix("\n") ? "\n" : "\n\n"
        let updated = existing + separator + block + "\n"
        // Append to the file itself, so a symlinked dotfile stays a symlink.
        if fm.fileExists(atPath: file.path), let handle = try? FileHandle(forWritingTo: file) {
            defer { try? handle.close() }
            try handle.seekToEnd()
            try handle.write(contentsOf: Data((separator + block + "\n").utf8))
        } else {
            try updated.write(to: file, atomically: true, encoding: .utf8)
        }
        return file
    }
}
