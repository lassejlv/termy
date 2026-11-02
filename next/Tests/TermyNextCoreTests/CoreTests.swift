import CoreGraphics
import Darwin
import Foundation
import Testing
@testable import TermyNextCore

@Suite struct SplitTreeTests {
    @Test func insertAndRemove() {
        var tree = SplitTree<Int>.leaf(1)
        tree = tree.inserting(2, at: 1, direction: .right)
        tree = tree.inserting(3, at: 2, direction: .down)
        tree = tree.inserting(0, at: 1, direction: .left)
        #expect(tree.leaves == [0, 1, 2, 3])
        #expect(tree.removing(2)?.leaves == [0, 1, 3])
        #expect(SplitTree<Int>.leaf(1).removing(1) == nil)
    }

    @Test func framesLeaveGaps() {
        let tree = SplitTree<Int>.leaf(1).inserting(2, at: 1, direction: .right)
        let frames = tree.frames(in: CGRect(x: 0, y: 0, width: 208, height: 100), gap: 8)
        #expect(frames[1] == CGRect(x: 0, y: 0, width: 100, height: 100))
        #expect(frames[2] == CGRect(x: 108, y: 0, width: 100, height: 100))
    }

    @Test func neighbors() {
        let tree = SplitTree<Int>.leaf(1)
            .inserting(2, at: 1, direction: .right)
            .inserting(3, at: 2, direction: .down)
        let frames = tree.frames(in: CGRect(x: 0, y: 0, width: 200, height: 200), gap: 0)
        #expect(SplitNavigation.neighbor(of: 1, direction: .right, frames: frames) == 2)
        #expect(SplitNavigation.neighbor(of: 3, direction: .up, frames: frames) == 2)
        #expect(SplitNavigation.neighbor(of: 3, direction: .left, frames: frames) == 1)
        #expect(SplitNavigation.neighbor(of: 1, direction: .left, frames: frames) == nil)
    }

    @Test func codableAndCompactMap() throws {
        let tree = SplitTree<String>.leaf("a").inserting("b", at: "a", direction: .right).inserting("c", at: "b", direction: .down)
        let data = try JSONEncoder().encode(tree)
        let decoded = try JSONDecoder().decode(SplitTree<String>.self, from: data)
        #expect(decoded == tree)
        #expect(tree.compactMap { $0 == "b" ? nil : $0.uppercased() }?.leaves == ["A", "C"])
        #expect(tree.compactMap { _ -> String? in nil } == nil)
    }

    @Test func insertsAWholeLayout() {
        let target = SplitTree<Int>.leaf(1).inserting(2, at: 1, direction: .right)
        let moved = SplitTree<Int>.leaf(10).inserting(11, at: 10, direction: .down)
        let merged = target.inserting(moved, at: 2, direction: .left)
        #expect(merged.leaves == [1, 10, 11, 2])
        // The moved layout keeps its own vertical split.
        guard case .split(_, _, _, let right) = merged, case .split(.horizontal, _, let block, _) = right,
              case .split(.vertical, _, _, _) = block else {
            Issue.record("unexpected shape \(merged)")
            return
        }
        #expect(SplitTree<Int>.leaf(5).inserting(moved, at: 99, direction: .up) == .leaf(5))
    }

    @Test func dividerRatio() {
        let tree = SplitTree<Int>.leaf(1).inserting(2, at: 1, direction: .down)
        let moved = tree.settingRatio(0.25, at: [])
        let frames = moved.frames(in: CGRect(x: 0, y: 0, width: 100, height: 100), gap: 0)
        #expect(frames[1]?.height == 25)
        #expect(moved.dividers(in: CGRect(x: 0, y: 0, width: 100, height: 100), gap: 0).count == 1)
    }
}

@Suite struct NamingTests {
    @Test func fuzzy() {
        #expect(FuzzyMatch.score("nt", in: "New Tab") != nil)
        #expect(FuzzyMatch.score("xyz", in: "New Tab") == nil)
        #expect(FuzzyMatch.score("sess", in: "Interface Style: Glass") == nil)
        #expect(FuzzyMatch.score("sess", in: "New Session") != nil)
        #expect(FuzzyMatch.score("zp", in: "Zoom Pane") != nil)
        let ranked = FuzzyMatch.filter(["Close Tab", "New Tab", "Move Tab to New Window"], query: "new tab") { $0 }
        #expect(ranked.first == "New Tab")
    }

    @Test func labels() {
        let home = "/Users/me"
        #expect(PaneLabel.make(title: "me@mac: ~/x", process: "zsh", directory: "/Users/me/x", home: home)
            == PaneLabel(name: "zsh", detail: "~/x"))
        #expect(PaneLabel.make(title: "Snake demo", process: "node", directory: "/tmp", home: home)
            == PaneLabel(name: "Snake demo", detail: "/tmp"))
        #expect(PaneLabel.make(title: nil, process: nil, directory: nil, home: home) == PaneLabel(name: "shell", detail: nil))
    }

    @Test func names() {
        #expect(Naming.uniqued("a", taken: ["a", "a-2"]) == "a-3")
        #expect(Naming.sessionName(taken: []).contains("-"))
        #expect(Naming.pathFromWorkingDirectoryReport("file://host/Users/me") == "/Users/me")
    }

    @Test func shellProcess() {
        #expect(ShellProcess.name(of: getpid()) != nil)
        #expect(ShellProcess.workingDirectory(of: getpid()) != nil)
    }
}

@Suite struct ScriptNameTests {
    @Test func namesInterpretedTools() {
        #expect(ShellProcess.scriptName(arguments: ["/bin/bash", "-p", "/opt/homebrew/Library/Homebrew/brew.sh", "install", "jq"]) == "brew")
        #expect(ShellProcess.scriptName(arguments: ["/opt/homebrew/Library/Homebrew/vendor/portable-ruby/current/bin/ruby", "-W1",
                                                    "-I", "/opt/homebrew/Library/Homebrew", "--", "/opt/homebrew/Library/Homebrew/brew.rb", "up"]) == "brew")
        #expect(ShellProcess.scriptName(arguments: ["/usr/bin/git", "status"]) == nil)
        #expect(ShellProcess.scriptName(arguments: ["/bin/bash"]) == nil)
    }

    @Test func readsThisProcessArguments() {
        #expect(!ShellProcess.arguments(of: getpid()).isEmpty)
    }
}

@Suite struct ShellProtocolTests {
    @Test func parsesMessagesFromZsh() {
        #expect(ShellProtocol.parse("hello\u{1F}pane-1\u{1F}42\u{1F}/usr/bin:/bin") == .hello(pane: "pane-1", pid: 42, path: "/usr/bin:/bin"))
        #expect(ShellProtocol.parse("done") == .done)
        #expect(ShellProtocol.parse("nonsense") == nil)
        let line = ShellProtocol.parse("line\u{1F}2\u{1F}cl\u{1F}claude\u{1E}clear\u{1F}clang\u{1E}clang++")
        #expect(line == .line(ShellLine(buffer: "cl", cursor: 2, history: ["claude", "clear"], commands: ["clang", "clang++"])))
        let empty = ShellProtocol.parse("line\u{1F}0\u{1F}\u{1F}\u{1F}")
        #expect(empty == .line(ShellLine(buffer: "", cursor: 0)))
    }

    @Test func escapingRoundTrips() {
        let text = "print -r -- \"a\\b\"\necho second"
        #expect(ShellProtocol.escape(text) == "print -r -- \"a\\\\b\"\\necho second")
        #expect(ShellProtocol.unescape(ShellProtocol.escape(text)) == text)
        #expect(ShellProtocol.encode("set", "a\u{1F}b") == "set\u{1F}ab\n")
    }

    @Test func suggestionsSkipTheLineItself() {
        let line = ShellLine(buffer: "clear", cursor: 5, history: ["clear", "clear -x"], commands: ["clear"])
        #expect(line.suggestions.history == ["clear -x"])
        #expect(line.suggestions.commands.isEmpty)
        #expect(ShellLine(buffer: "cl", cursor: 1, history: ["clear"]).suggestions.history.isEmpty)
        #expect(ShellLine(buffer: "  ", cursor: 2, history: ["  ls"]).suggestions.history.isEmpty)
    }

    @Test func commandFromAgentReply() {
        #expect(ShellProtocol.command(fromAgentReply: "find . -name mod.rs\n") == "find . -name mod.rs")
        #expect(ShellProtocol.command(fromAgentReply: "```zsh\nls -la\n```") == "ls -la")
        #expect(ShellProtocol.command(fromAgentReply: "Here:\n```\n$ du -sh *\n```\nDone.") == "du -sh *")
        #expect(ShellProtocol.command(fromAgentReply: "Must output one command. Likely clear.\n\nman clear") == "man clear")
        #expect(ShellProtocol.command(fromAgentReply: "`git status`") == "git status")
        #expect(ShellProtocol.command(fromAgentReply: "for f in *; do\n  echo $f\ndone") == "for f in *; do\n  echo $f\ndone")
        #expect(ShellProtocol.command(fromAgentReply: "  \n") == nil)
    }
}

@Suite struct CommandSpecTests {
    private func names(_ line: String) -> [String] {
        let command = ShellWords.split(line).first?.text ?? ""
        return CommandSpecs.spec(for: command)?.complete(line)?.items.map(\.name) ?? []
    }

    @Test func subcommandsByPrefix() {
        #expect(names("git ch").contains("checkout"))
        #expect(names("git ch").contains("cherry-pick"))
        #expect(!names("git ch").contains("commit"))
        #expect(names("cargo b").contains("build"))
        #expect(names("brew up") == ["upgrade", "update"])
        #expect(CommandSpecs.spec(for: "git")?.complete("git") == nil)
    }

    @Test func optionsAfterADash() {
        #expect(names("git commit --am") == ["--amend"])
        #expect(names("git commit -").contains("-m"))
        // Used once already, so not again.
        #expect(!names("git commit --amend --").contains("--amend"))
        // Persistent options reach subcommands; cargo -C does.
        #expect(names("cargo build -C ").isEmpty)
    }

    @Test func argumentsAndGenerators() {
        let spec = CommandSpecs.spec(for: "git")!
        #expect(spec.complete("git checkout ")?.generator == .gitBranches)
        #expect(spec.complete("git checkout -b ")?.generator == nil)
        #expect(spec.complete("git push origin ")?.generator == .gitBranches)
        #expect(spec.complete("git stash pop ")?.generator == .gitStashes)
        #expect(CommandSpecs.spec(for: "bun")?.complete("bun run ")?.generator == .packageScripts)
        #expect(CommandSpecs.spec(for: "brew")?.complete("brew uninstall ")?.generator == .brewInstalled)
        #expect(CommandSpecs.spec(for: "cargo")?.complete("cargo run --bin ")?.generator == .cargoBins)
        // Only the last command of a pipeline counts.
        #expect(names("ls | git sta") == ["status", "stash"])
    }

    @Test func takingAnItemReplacesTheWord() {
        let line = "git chec"
        let completion = CommandSpecs.spec(for: "git")!.complete(line)!
        let item = completion.items.first { $0.name == "checkout" }!
        #expect(completion.line(line, taking: item) == "git checkout ")
    }

    @Test func shellWords() {
        #expect(ShellWords.split("git commit -m 'a b' ").map(\.text) == ["git", "commit", "-m", "a b", ""])
        #expect(ShellWords.split("cd x && cargo b").map(\.text) == ["cargo", "b"])
        #expect(ShellWords.split("cd x && cargo b").last?.start == 14)
    }

    @Test func generatorOutput() {
        #expect(Generator.gitBranches.items(from: "main\nfeature/x\n").map(\.name) == ["main", "feature/x"])
        #expect(Generator.gitRemoteBranches.items(from: "origin\norigin/main\n").map(\.name) == ["origin/main"])
        #expect(Generator.gitChangedFiles.items(from: " M src/a.rs\n?? new.txt\nR  old -> moved\n").map(\.name) == ["src/a.rs", "new.txt", "moved"])
        #expect(Generator.gitCommits.items(from: "abc1234 Fix it\n").first?.description == "Fix it")
        #expect(Generator.rustupToolchains.items(from: "stable-aarch64-apple-darwin (default)\n").first?.description == "default")
        #expect(Generator.brewServices.items(from: "Name Status User File\npostgresql@16 started me ~/x\n").map(\.name) == ["postgresql@16"])
        #expect(Generator.packageScripts("{\"scripts\":{\"dev\":\"vite\",\"build\":\"tsc\"}}").map(\.name) == ["build", "dev"])
        let metadata = #"{"packages":[{"name":"app","version":"0.1.0","targets":[{"name":"app","kind":["bin"]},{"name":"demo","kind":["example"]}]}]}"#
        #expect(Generator.cargoBins.items(from: metadata).map(\.name) == ["app"])
        #expect(Generator.cargoExamples.items(from: metadata).map(\.name) == ["demo"])
        #expect(Generator.cargoPackages.items(from: metadata).map(\.name) == ["app"])
    }

    @Test func paths() throws {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("spec-paths-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: folder.appendingPathComponent("src"), withIntermediateDirectories: true)
        FileManager.default.createFile(atPath: folder.appendingPathComponent("src/main.rs").path, contents: Data())
        FileManager.default.createFile(atPath: folder.appendingPathComponent("README.md").path, contents: Data())
        defer { try? FileManager.default.removeItem(at: folder) }
        let top = Generator.paths(for: "", in: folder.path, foldersOnly: false)
        #expect(top.map(\.name) == ["README.md", "src/"])
        #expect(top.last?.appendsSpace == false)
        #expect(Generator.paths(for: "src/m", in: folder.path, foldersOnly: false).map(\.name) == ["src/main.rs"])
        #expect(Generator.paths(for: "", in: folder.path, foldersOnly: true).map(\.name) == ["src/"])
    }
}

@Suite struct MoreSpecTests {
    private func completion(_ line: String) -> SpecCompletion? {
        CommandSpecs.spec(for: ShellWords.split(line).first?.text ?? "")?.complete(line)
    }

    @Test func newSpecsPointAtTheirSources() {
        #expect(completion("claude --resume ")?.generators == [.claudeSessions])
        #expect(completion("claude --model ")?.items.map(\.name).contains("opus") == true)
        #expect(completion("codex resume ")?.generators == [.codexSessions])
        #expect(completion("codex -s ")?.items.map(\.name) == ["read-only", "workspace-write", "danger-full-access"])
        #expect(completion("npm run ")?.generators == [.packageScripts])
        #expect(completion("pnpm d")?.generators == [.packageScripts])
        #expect(completion("bun d")?.generators == [.packageScripts, .files])
        #expect(completion("npx ")?.generators == [.nodeBins])
        #expect(completion("bunx ")?.generators == [.nodeBins])
        #expect(completion("just ")?.generators == [.justRecipes])
        #expect(completion("ssh ")?.generators == [.sshHosts])
        #expect(completion("ssh -J ")?.generators == [.sshHosts])
        #expect(completion("rm -rf ")?.generators == [.files])
        #expect(completion("cd ")?.generators == [.folders])
        #expect(completion("zed ")?.generators == [.files])
        #expect(completion("claude mcp a")?.items.map(\.name) == ["add"])
    }

    @Test func justRecipes() {
        let output = """
            Available recipes:
                default                            # Show available recipes
                run-cli *args
                benchmark mib="32" # Measure it
            """
        let items = Generator.justRecipes.items(from: output)
        #expect(items.map(\.name) == ["default", "run-cli", "benchmark"])
        #expect(items.first?.description == "Show available recipes")
        #expect(items.last?.description == "Measure it")
    }

    @Test func sshHosts() {
        let config = """
            Host box prod
              HostName 10.0.0.2
            Host *.internal
              User me
            Host hetzner
              HostName example.com
            """
        let known = "github.com,140.82.1.1 ssh-ed25519 AAA\n[git.example.com]:2222 ssh-rsa BBB\n|1|hashed= ssh-rsa CCC\n"
        let items = Generator.sshHosts(config: config, knownHosts: known)
        #expect(items.map(\.name) == ["box", "prod", "hetzner", "github.com", "140.82.1.1", "git.example.com"])
        #expect(items.first?.description == "10.0.0.2")
    }

    @Test func localSources() throws {
        let fm = FileManager.default
        let home = fm.temporaryDirectory.appendingPathComponent("spec-home-\(UUID().uuidString)")
        defer { try? fm.removeItem(at: home) }
        // node_modules/.bin, found from a subfolder.
        let project = home.appendingPathComponent("my.app")
        try fm.createDirectory(at: project.appendingPathComponent("node_modules/.bin"), withIntermediateDirectories: true)
        try fm.createDirectory(at: project.appendingPathComponent("src"), withIntermediateDirectories: true)
        for tool in ["vite", "tsc"] { fm.createFile(atPath: project.appendingPathComponent("node_modules/.bin/\(tool)").path, contents: Data()) }
        #expect(Generator.nodeBins(from: project.appendingPathComponent("src").path).map(\.name) == ["tsc", "vite"])

        // Claude Code: ~/.claude/projects/<folder with "-">/<id>.jsonl
        let mangled = String(project.path.map { $0.isLetter || $0.isNumber ? $0 : "-" })
        let claude = home.appendingPathComponent(".claude/projects/\(mangled)")
        try fm.createDirectory(at: claude, withIntermediateDirectories: true)
        let lines = [
            #"{"type":"mode"}"#,
            #"{"type":"user","message":{"role":"user","content":"Fix the login bug\nand more"}}"#,
        ].joined(separator: "\n")
        try lines.write(to: claude.appendingPathComponent("abc-123.jsonl"), atomically: true, encoding: .utf8)
        let sessions = Generator.claudeSessions(for: project.path, home: home.path)
        #expect(sessions.map(\.name) == ["abc-123"])
        #expect(sessions.first?.description == "Fix the login bug")

        // Codex: ~/.codex/sessions/YYYY/MM/DD/rollout-…-<id>.jsonl
        let day = home.appendingPathComponent(".codex/sessions/2026/11/02")
        try fm.createDirectory(at: day, withIntermediateDirectories: true)
        let rollout = [
            #"{"type":"session_meta","payload":{"id":"01a1","cwd":"\#(project.path)"}}"#,
            #"{"type":"event_msg","payload":{"type":"user_message","message":"Add dark mode"}}"#,
        ].joined(separator: "\n")
        try rollout.write(to: day.appendingPathComponent("rollout-2026-11-02T11-44-54-01a1.jsonl"), atomically: true, encoding: .utf8)
        let codex = Generator.codexSessions(for: project.path, home: home.path)
        #expect(codex.map(\.name) == ["01a1"])
        #expect(codex.first?.description == "Add dark mode")
    }
}
