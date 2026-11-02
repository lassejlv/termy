// Termy Next's command specs: git, brew, cargo, rustup, rustc, bun, npm,
// pnpm, npx, bunx, just, ssh, claude, codex, and file commands (rm, mkdir,
// cd, zed…). Add a command by writing a spec and listing it in `all`.

public enum CommandSpecs {
    public static let all: [String: CommandSpec] = {
        var specs: [String: CommandSpec] = [:]
        for spec in [git, brew, cargo, rustup, rustc, bun, bunx, npm, pnpm, npx, just, ssh, claude, codex] + fileCommands {
            for name in spec.names { specs[name] = spec }
        }
        return specs
    }()

    public static func spec(for command: String) -> CommandSpec? { all[command] }

    // MARK: git

    typealias Spec = CommandSpec
    typealias Opt = CommandSpec.Option
    typealias Arg = CommandSpec.Argument

    static let git = Spec("git", "Distributed version control", options: [
        Opt("-C", "Run as if started in this folder", argument: .folder, persistent: true),
        Opt("--version", "Print the version"),
        Opt("--help", "Show help", persistent: true),
    ], subcommands: [
        Spec("status", "Show the working tree status", options: [
            Opt("-s|--short", "Short format"), Opt("-b|--branch", "Show the branch"),
            Opt("-u|--untracked-files", "Show untracked files"),
        ]),
        Spec("add", "Stage changes", options: [
            Opt("-A|--all", "Stage every change"), Opt("-p|--patch", "Pick hunks interactively"),
            Opt("-u|--update", "Stage changes to tracked files"), Opt("-n|--dry-run", "Show what would be staged"),
            Opt("-f|--force", "Stage ignored files too"),
        ], arguments: [.from(.gitChangedFiles, "file", variadic: true)]),
        Spec("restore", "Restore working tree files", options: [
            Opt("-S|--staged", "Unstage instead"), Opt("-W|--worktree", "Restore the working tree"),
            Opt("-s|--source", "Restore from this commit", argument: .from(.gitBranches)),
            Opt("-p|--patch", "Pick hunks interactively"),
        ], arguments: [.from(.gitChangedFiles, "file", variadic: true)]),
        Spec("commit", "Record staged changes", options: [
            Opt("-m|--message", "Commit message", argument: .text("message"), repeatable: true),
            Opt("-a|--all", "Stage tracked changes first"), Opt("--amend", "Replace the last commit"),
            Opt("--no-edit", "Keep the message when amending"), Opt("--no-verify", "Skip hooks"),
            Opt("-S|--gpg-sign", "Sign the commit"), Opt("--fixup", "Make a fixup commit", argument: .from(.gitCommits)),
            Opt("--allow-empty", "Allow a commit with no changes"), Opt("-v|--verbose", "Show the diff in the editor"),
        ]),
        Spec("push", "Update a remote", options: [
            Opt("-u|--set-upstream", "Track the remote branch"), Opt("-f|--force", "Overwrite the remote branch"),
            Opt("--force-with-lease", "Force only if the remote hasn't moved"), Opt("--tags", "Push tags too"),
            Opt("-d|--delete", "Delete the remote branch"), Opt("--no-verify", "Skip hooks"),
            Opt("-n|--dry-run", "Show what would be pushed"),
        ], arguments: [.from(.gitRemotes), .from(.gitBranches)]),
        Spec("pull", "Fetch and integrate a remote branch", options: [
            Opt("-r|--rebase", "Rebase instead of merging"), Opt("--no-rebase", "Merge"),
            Opt("--ff-only", "Only fast-forward"), Opt("--autostash", "Stash changes around it"),
        ], arguments: [.from(.gitRemotes), .from(.gitBranches)]),
        Spec("fetch", "Download from a remote", options: [
            Opt("--all", "Every remote"), Opt("-p|--prune", "Drop branches gone from the remote"),
            Opt("-t|--tags", "Fetch tags"), Opt("--depth", "Limit history", argument: .text("depth")),
        ], arguments: [.from(.gitRemotes)]),
        Spec("switch", "Switch branches", options: [
            Opt("-c|--create", "Create a branch", argument: .text("branch")),
            Opt("-C|--force-create", "Create or reset a branch", argument: .text("branch")),
            Opt("-d|--detach", "Detach at a commit"), Opt("-", "The previous branch"),
        ], arguments: [.from(.gitBranches)]),
        Spec("checkout", "Switch branches or restore files", options: [
            Opt("-b", "Create a branch", argument: .text("branch")), Opt("-B", "Create or reset a branch", argument: .text("branch")),
            Opt("-t|--track", "Track a remote branch", argument: .from(.gitRemoteBranches)),
            Opt("-f|--force", "Throw away local changes"), Opt("--", "Files follow"),
        ], arguments: [.from(.gitBranches)]),
        Spec("branch", "List, create or delete branches", options: [
            Opt("-a|--all", "List remote branches too"), Opt("-r|--remotes", "List remote branches"),
            Opt("-d|--delete", "Delete a merged branch", argument: .from(.gitBranches)),
            Opt("-D", "Delete a branch", argument: .from(.gitBranches)),
            Opt("-m|--move", "Rename a branch", argument: .from(.gitBranches)),
            Opt("-u|--set-upstream-to", "Set the upstream", argument: .from(.gitRemoteBranches)),
            Opt("-v|--verbose", "Show the last commit"), Opt("--show-current", "Print the current branch"),
        ], arguments: [.text("branch")]),
        Spec("merge", "Join branches", options: [
            Opt("--no-ff", "Always make a merge commit"), Opt("--ff-only", "Only fast-forward"),
            Opt("--squash", "Squash into one change"), Opt("--abort", "Stop a conflicted merge"),
            Opt("--continue", "Finish a merge"), Opt("-m", "Merge message", argument: .text("message")),
        ], arguments: [.from(.gitBranches)]),
        Spec("rebase", "Reapply commits on another base", options: [
            Opt("-i|--interactive", "Edit the list of commits"), Opt("--continue", "Continue after resolving"),
            Opt("--abort", "Go back to before the rebase"), Opt("--skip", "Skip this commit"),
            Opt("--onto", "New base", argument: .from(.gitBranches)), Opt("--autostash", "Stash changes around it"),
            Opt("--autosquash", "Apply fixup commits"),
        ], arguments: [.from(.gitBranches)]),
        Spec("reset", "Move HEAD", options: [
            Opt("--soft", "Keep changes staged"), Opt("--mixed", "Keep changes unstaged"),
            Opt("--hard", "Throw away changes"), Opt("-p|--patch", "Pick hunks interactively"),
        ], arguments: [.from(.gitCommits)]),
        Spec("log", "Show commits", options: [
            Opt("--oneline", "One line per commit"), Opt("--graph", "Draw the history"),
            Opt("-p|--patch", "Show each diff"), Opt("--stat", "Show changed files"),
            Opt("-n|--max-count", "Limit the count", argument: .text("count")), Opt("--all", "Every branch"),
            Opt("--author", "By this author", argument: .text("author")),
            Opt("--since", "Since a date", argument: .text("date")),
        ], arguments: [.from(.gitBranches)]),
        Spec("diff", "Show changes", options: [
            Opt("--staged|--cached", "Staged changes"), Opt("--stat", "Summary per file"),
            Opt("--name-only", "Just the file names"), Opt("-w|--ignore-all-space", "Ignore whitespace"),
        ], arguments: [.from(.gitChangedFiles, "file", variadic: true)]),
        Spec("show", "Show a commit", options: [Opt("--stat", "Summary per file")], arguments: [.from(.gitCommits)]),
        Spec("stash", "Put changes aside", options: [
            Opt("-u|--include-untracked", "Untracked files too"), Opt("-m|--message", "Stash message", argument: .text("message")),
        ], subcommands: [
            Spec("push", "Stash changes", options: [Opt("-m|--message", "Stash message", argument: .text("message"))]),
            Spec("pop", "Apply and drop a stash", arguments: [.from(.gitStashes)]),
            Spec("apply", "Apply a stash", arguments: [.from(.gitStashes)]),
            Spec("drop", "Delete a stash", arguments: [.from(.gitStashes)]),
            Spec("list", "List stashes"),
            Spec("show", "Show a stash", arguments: [.from(.gitStashes)]),
            Spec("clear", "Delete every stash"),
        ]),
        Spec("cherry-pick", "Apply commits", options: [
            Opt("--continue", "Continue after resolving"), Opt("--abort", "Give up"), Opt("-x", "Note the source commit"),
        ], arguments: [.from(.gitCommits, variadic: true)]),
        Spec("revert", "Undo commits with new ones", arguments: [.from(.gitCommits, variadic: true)]),
        Spec("tag", "List, create or delete tags", options: [
            Opt("-a|--annotate", "Annotated tag"), Opt("-m|--message", "Tag message", argument: .text("message")),
            Opt("-d|--delete", "Delete a tag", argument: .from(.gitTags)), Opt("-l|--list", "List tags"),
        ], arguments: [.text("tag")]),
        Spec("remote", "Manage remotes", options: [Opt("-v|--verbose", "Show URLs")], subcommands: [
            Spec("add", "Add a remote", arguments: [.text("name"), .text("url")]),
            Spec("remove|rm", "Remove a remote", arguments: [.from(.gitRemotes)]),
            Spec("rename", "Rename a remote", arguments: [.from(.gitRemotes), .text("name")]),
            Spec("set-url", "Change a remote's URL", arguments: [.from(.gitRemotes), .text("url")]),
            Spec("get-url", "Print a remote's URL", arguments: [.from(.gitRemotes)]),
        ]),
        Spec("clone", "Copy a repository", options: [
            Opt("--depth", "Limit history", argument: .text("depth")), Opt("-b|--branch", "Check out this branch", argument: .text("branch")),
            Opt("--recursive", "Submodules too"),
        ], arguments: [.text("repository"), .folder]),
        Spec("init", "Create a repository", options: [Opt("-b|--initial-branch", "First branch name", argument: .text("name"))],
             arguments: [.folder]),
        Spec("rm", "Remove files", options: [Opt("--cached", "Keep the file on disk"), Opt("-r", "Recursively")], arguments: [.files]),
        Spec("mv", "Move or rename a file", arguments: [.file, .file]),
        Spec("blame", "Show who changed each line", arguments: [.file]),
        Spec("worktree", "Manage working trees", subcommands: [
            Spec("add", "Add a working tree", options: [Opt("-b", "New branch", argument: .text("branch"))],
                 arguments: [.folder, .from(.gitBranches)]),
            Spec("list", "List working trees"),
            Spec("remove", "Remove a working tree", arguments: [.folder]),
            Spec("prune", "Drop stale working trees"),
        ]),
        Spec("bisect", "Find the commit that broke something", subcommands: [
            Spec("start", "Start"), Spec("good", "Mark good", arguments: [.from(.gitCommits)]),
            Spec("bad", "Mark bad", arguments: [.from(.gitCommits)]), Spec("reset", "Finish"),
        ]),
        Spec("reflog", "Show where HEAD has been"),
        Spec("clean", "Delete untracked files", options: [
            Opt("-n|--dry-run", "Show what would go"), Opt("-f|--force", "Delete"), Opt("-d", "Folders too"),
            Opt("-x", "Ignored files too"),
        ]),
        Spec("config", "Read and write settings", options: [
            Opt("--global", "For your user"), Opt("--local", "For this repository"), Opt("-l|--list", "List settings"),
            Opt("--unset", "Remove a setting", argument: .text("key")),
        ], arguments: [.text("key"), .text("value")]),
        Spec("submodule", "Manage submodules", subcommands: [
            Spec("update", "Check out submodules", options: [Opt("--init", "Initialize first"), Opt("--recursive", "Nested too")]),
            Spec("add", "Add a submodule", arguments: [.text("repository"), .folder]),
            Spec("status", "Show submodules"),
        ]),
    ])

    // MARK: brew

    static let brew = Spec("brew", "Homebrew, the macOS package manager", options: [
        Opt("-v|--verbose", "More output", persistent: true), Opt("-d|--debug", "Debug output", persistent: true),
        Opt("--version", "Print the version"),
    ], subcommands: [
        Spec("install", "Install formulae or casks", options: [
            Opt("--cask", "Install casks"), Opt("--formula", "Install formulae"), Opt("-f|--force", "Install anyway"),
            Opt("--HEAD", "Install the development version"), Opt("-s|--build-from-source", "Build from source"),
        ], arguments: [.from(.brewFormulae, variadic: true)]),
        Spec("uninstall|remove|rm", "Uninstall formulae or casks", options: [
            Opt("--cask", "Casks"), Opt("--zap", "Remove all of a cask's files"), Opt("-f|--force", "Every version"),
        ], arguments: [.from(.brewInstalled, variadic: true)]),
        Spec("reinstall", "Reinstall formulae or casks", arguments: [.from(.brewInstalled, variadic: true)]),
        Spec("upgrade", "Upgrade outdated formulae and casks", options: [
            Opt("--cask", "Casks only"), Opt("--formula", "Formulae only"), Opt("-n|--dry-run", "Show what would upgrade"),
            Opt("--greedy", "Casks that update themselves too"),
        ], arguments: [.from(.brewOutdated, variadic: true)]),
        Spec("update", "Fetch the newest Homebrew and formulae"),
        Spec("outdated", "List outdated formulae and casks", options: [Opt("--cask", "Casks"), Opt("--greedy", "Self-updating casks too")]),
        Spec("list|ls", "List installed formulae and casks", options: [
            Opt("--cask", "Casks"), Opt("--formula", "Formulae"), Opt("-1", "One per line"), Opt("--versions", "With versions"),
        ], arguments: [.from(.brewInstalled)]),
        Spec("search", "Search formulae and casks", options: [Opt("--cask", "Casks"), Opt("--formula", "Formulae")],
             arguments: [.text("text")]),
        Spec("info", "Show a formula or cask", options: [Opt("--json", "As JSON"), Opt("--cask", "Casks")],
             arguments: [.from(.brewFormulae)]),
        Spec("home", "Open a formula's homepage", arguments: [.from(.brewFormulae)]),
        Spec("deps", "Show a formula's dependencies", options: [Opt("--tree", "As a tree"), Opt("--installed", "Installed only")],
             arguments: [.from(.brewInstalled)]),
        Spec("uses", "Show formulae that use one", options: [Opt("--installed", "Installed only")], arguments: [.from(.brewFormulae)]),
        Spec("leaves", "List formulae nothing depends on"),
        Spec("cleanup", "Remove old versions and caches", options: [Opt("-n|--dry-run", "Show what would go"), Opt("-s", "Downloads too")],
             arguments: [.from(.brewInstalled)]),
        Spec("autoremove", "Remove unused dependencies", options: [Opt("-n|--dry-run", "Show what would go")]),
        Spec("doctor", "Check for problems"),
        Spec("config", "Show the configuration"),
        Spec("pin", "Keep a formula from upgrading", arguments: [.from(.brewInstalled)]),
        Spec("unpin", "Let a formula upgrade again", arguments: [.from(.brewInstalled)]),
        Spec("link", "Link a formula into the prefix", options: [Opt("--overwrite", "Replace existing files")],
             arguments: [.from(.brewInstalled)]),
        Spec("unlink", "Unlink a formula", arguments: [.from(.brewInstalled)]),
        Spec("tap", "Add a tap, or list them", arguments: [.text("user/repo")]),
        Spec("untap", "Remove a tap", arguments: [.from(.brewTaps)]),
        Spec("services", "Manage background services", subcommands: [
            Spec("list", "List services"),
            Spec("start", "Start a service and run it at login", arguments: [.from(.brewServices)]),
            Spec("stop", "Stop a service", arguments: [.from(.brewServices)]),
            Spec("restart", "Restart a service", arguments: [.from(.brewServices)]),
            Spec("run", "Run a service without login start", arguments: [.from(.brewServices)]),
            Spec("info", "Show a service", arguments: [.from(.brewServices)]),
        ]),
        Spec("bundle", "Install from a Brewfile", subcommands: [
            Spec("dump", "Write a Brewfile", options: [Opt("--force", "Overwrite it")]),
            Spec("check", "Check everything is installed"),
            Spec("cleanup", "Uninstall what isn't in the Brewfile", options: [Opt("--force", "Uninstall")]),
        ]),
    ])

    // MARK: Rust

    private static let cargoCommon: [Opt] = [
        Opt("-p|--package", "This package", argument: .from(.cargoPackages)),
        Opt("--workspace", "Every package in the workspace"),
        Opt("-r|--release", "Optimized build"),
        Opt("--profile", "Build profile", argument: .values("profile", ["dev", "release", "test", "bench"])),
        Opt("-F|--features", "Turn on features", argument: .text("features")),
        Opt("--all-features", "Every feature"), Opt("--no-default-features", "No default features"),
        Opt("--target", "Target triple", argument: .from(.rustupInstalledTargets)),
        Opt("-j|--jobs", "Parallel jobs", argument: .text("count")),
        Opt("-q|--quiet", "Less output"), Opt("-v|--verbose", "More output", repeatable: true),
        Opt("--locked", "Require Cargo.lock to be up to date"), Opt("--offline", "No network"),
    ]
    private static let cargoTargets: [Opt] = [
        Opt("--bin", "This binary", argument: .from(.cargoBins)), Opt("--bins", "Every binary"),
        Opt("--example", "This example", argument: .from(.cargoExamples)), Opt("--examples", "Every example"),
        Opt("--lib", "The library"), Opt("--tests", "Every test"), Opt("--all-targets", "Every target"),
    ]

    static let cargo = Spec("cargo", "Rust's package manager", options: [
        Opt("--version|-V", "Print the version"), Opt("--list", "List commands"),
        Opt("-C", "Run in this folder", argument: .folder, persistent: true),
        Opt("--color", "When to use color", argument: .values("when", ["auto", "always", "never"]), persistent: true),
    ], subcommands: [
        Spec("build|b", "Compile the package", options: cargoCommon + cargoTargets),
        Spec("run|r", "Run a binary or example", options: cargoCommon + [
            Opt("--bin", "This binary", argument: .from(.cargoBins)), Opt("--example", "This example", argument: .from(.cargoExamples)),
        ]),
        Spec("check|c", "Check for errors without building", options: cargoCommon + cargoTargets),
        Spec("test|t", "Run tests", options: cargoCommon + cargoTargets + [
            Opt("--doc", "Only doc tests"), Opt("--no-run", "Build but don't run"), Opt("--no-fail-fast", "Keep going after a failure"),
        ], arguments: [.text("filter")]),
        Spec("bench", "Run benchmarks", options: cargoCommon + [Opt("--bench", "This benchmark", argument: .text("name"))]),
        Spec("clippy", "Lint the package", options: cargoCommon + cargoTargets + [Opt("--fix", "Apply suggestions")]),
        Spec("fmt", "Format the code", options: [Opt("--check", "Only check"), Opt("--all", "Every package")]),
        Spec("doc|d", "Build documentation", options: cargoCommon + [Opt("--open", "Open it"), Opt("--no-deps", "Skip dependencies")]),
        Spec("new", "Create a package", options: [
            Opt("--bin", "A binary"), Opt("--lib", "A library"), Opt("--name", "Package name", argument: .text("name")),
            Opt("--edition", "Rust edition", argument: .values("edition", ["2015", "2018", "2021", "2024"])),
        ], arguments: [.text("path")]),
        Spec("init", "Create a package in this folder", options: [Opt("--bin", "A binary"), Opt("--lib", "A library")],
             arguments: [.folder]),
        Spec("add", "Add dependencies", options: [
            Opt("--dev", "As a dev dependency"), Opt("--build", "As a build dependency"),
            Opt("-F|--features", "Turn on features", argument: .text("features")), Opt("--optional", "As optional"),
            Opt("-p|--package", "To this package", argument: .from(.cargoPackages)),
        ], arguments: [Arg("crate", variadic: true)]),
        Spec("remove|rm", "Remove dependencies", options: [Opt("--dev", "Dev dependency"), Opt("-p|--package", "From this package", argument: .from(.cargoPackages))],
             arguments: [Arg("crate", variadic: true)]),
        Spec("update", "Update Cargo.lock", options: [Opt("-p|--package", "Only this package", argument: .from(.cargoPackages)),
                                                      Opt("--dry-run", "Show what would change")]),
        Spec("install", "Install a binary crate", options: [
            Opt("--path", "From a local folder", argument: .folder), Opt("--git", "From a git repository", argument: .text("url")),
            Opt("--locked", "Use its Cargo.lock"), Opt("-f|--force", "Overwrite"),
        ], arguments: [.text("crate")]),
        Spec("uninstall", "Remove an installed binary", arguments: [.text("crate")]),
        Spec("clean", "Remove the target folder", options: [Opt("-p|--package", "Only this package", argument: .from(.cargoPackages)),
                                                            Opt("-r|--release", "Release artifacts only")]),
        Spec("tree", "Show the dependency tree", options: [Opt("-i|--invert", "Who depends on a package", argument: .text("package")),
                                                           Opt("-d|--duplicates", "Packages in several versions"),
                                                           Opt("--depth", "Limit the depth", argument: .text("depth"))]),
        Spec("search", "Search crates.io", arguments: [.text("query")]),
        Spec("publish", "Upload the package to crates.io", options: [Opt("--dry-run", "Check without uploading")]),
        Spec("fix", "Apply compiler suggestions", options: cargoCommon + [Opt("--allow-dirty", "With uncommitted changes")]),
        Spec("metadata", "Print package metadata as JSON", options: [Opt("--no-deps", "Without dependencies")]),
    ])

    static let rustup = Spec("rustup", "The Rust toolchain installer", subcommands: [
        Spec("update", "Update toolchains and rustup", arguments: [.from(.rustupToolchains)]),
        Spec("default", "Set the default toolchain", arguments: [.values("toolchain", ["stable", "beta", "nightly"])]),
        Spec("show", "Show the active toolchain", subcommands: [Spec("active-toolchain", "The active toolchain")]),
        Spec("toolchain", "Manage toolchains", subcommands: [
            Spec("list", "List installed toolchains"),
            Spec("install", "Install a toolchain", arguments: [.values("toolchain", ["stable", "beta", "nightly"])]),
            Spec("uninstall", "Uninstall a toolchain", arguments: [.from(.rustupToolchains)]),
        ]),
        Spec("target", "Manage compile targets", subcommands: [
            Spec("list", "List targets", options: [Opt("--installed", "Installed only")]),
            Spec("add", "Install a target", arguments: [.from(.rustupTargets, variadic: true)]),
            Spec("remove", "Remove a target", arguments: [.from(.rustupInstalledTargets, variadic: true)]),
        ]),
        Spec("component", "Manage components", subcommands: [
            Spec("list", "List components", options: [Opt("--installed", "Installed only")]),
            Spec("add", "Install a component", arguments: [.from(.rustupComponents, variadic: true)]),
            Spec("remove", "Remove a component", arguments: [.from(.rustupComponents, variadic: true)]),
        ]),
        Spec("override", "Pick a toolchain for a folder", subcommands: [
            Spec("set", "Use a toolchain here", arguments: [.from(.rustupToolchains)]),
            Spec("unset", "Go back to the default"), Spec("list", "List overrides"),
        ]),
        Spec("run", "Run a command with a toolchain", arguments: [.from(.rustupToolchains), .text("command")]),
        Spec("which", "Show which binary runs", arguments: [.text("command")]),
        Spec("doc", "Open the local documentation", options: [Opt("--std", "The standard library"), Opt("--book", "The Book")]),
        Spec("self", "Manage rustup itself", subcommands: [Spec("update", "Update rustup"), Spec("uninstall", "Uninstall Rust")]),
    ])

    static let rustc = Spec("rustc", "The Rust compiler", options: [
        Opt("-o", "Output file", argument: .file),
        Opt("-O", "Optimize"),
        Opt("--edition", "Rust edition", argument: .values("edition", ["2015", "2018", "2021", "2024"])),
        Opt("--crate-type", "Kind of crate", argument: .values("type", ["bin", "lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"])),
        Opt("--crate-name", "Crate name", argument: .text("name")),
        Opt("--emit", "What to write", argument: .values("kind", ["asm", "llvm-bc", "llvm-ir", "obj", "metadata", "link", "dep-info", "mir"])),
        Opt("--target", "Target triple", argument: .from(.rustupInstalledTargets)),
        Opt("-C|--codegen", "Code generation option", argument: .text("option"), repeatable: true),
        Opt("-g", "Debug info"), Opt("--test", "Build a test harness"),
        Opt("--explain", "Explain an error code", argument: .text("code")),
        Opt("--version|-V", "Print the version"),
    ], arguments: [.file])

    // MARK: bun

    static let bun = Spec("bun", "JavaScript runtime and toolkit", options: [
        Opt("--version|-v", "Print the version"), Opt("--revision", "Print the version and commit"),
        Opt("--watch", "Restart on changes"), Opt("--hot", "Reload on changes"),
        Opt("--cwd", "Run in this folder", argument: .folder),
    ], arguments: [Arg("script", generators: [.packageScripts, .files])], subcommands: [
        Spec("run", "Run a script or file", options: [
            Opt("--watch", "Restart on changes"), Opt("--hot", "Reload on changes"), Opt("--bun", "Use bun instead of node"),
            Opt("--filter", "In these workspace packages", argument: .text("pattern")),
        ], arguments: [Arg("script", generators: [.packageScripts, .files]), .files]),
        Spec("install|i", "Install dependencies", options: [
            Opt("-p|--production", "Without dev dependencies"), Opt("--frozen-lockfile", "Fail if the lockfile would change"),
            Opt("-g|--global", "Globally"), Opt("--force", "Reinstall everything"),
        ]),
        Spec("add|a", "Add dependencies", options: [
            Opt("-d|--dev", "As a dev dependency"), Opt("--optional", "As optional"), Opt("--peer", "As a peer dependency"),
            Opt("-E|--exact", "Pin the exact version"), Opt("-g|--global", "Globally"),
        ], arguments: [Arg("package", variadic: true)]),
        Spec("remove|rm", "Remove dependencies", options: [Opt("-g|--global", "Globally")], arguments: [Arg("package", variadic: true)]),
        Spec("update", "Update dependencies", options: [Opt("--latest", "Past the version range"), Opt("-i|--interactive", "Pick which")],
             arguments: [Arg("package", variadic: true)]),
        Spec("outdated", "List outdated dependencies"),
        Spec("x", "Run a package's binary", options: [Opt("--bun", "Use bun instead of node")], arguments: [.text("package")]),
        Spec("test", "Run tests", options: [
            Opt("--watch", "Rerun on changes"), Opt("--coverage", "Report coverage"), Opt("-t|--test-name-pattern", "Only matching tests", argument: .text("pattern")),
            Opt("--bail", "Stop after a failure"), Opt("--timeout", "Per-test timeout in ms", argument: .text("ms")),
            Opt("-u|--update-snapshots", "Update snapshots"),
        ], arguments: [.files]),
        Spec("build", "Bundle files", options: [
            Opt("--outdir", "Output folder", argument: .folder), Opt("--outfile", "Output file", argument: .file),
            Opt("--target", "Where it runs", argument: .values("target", ["browser", "bun", "node"])),
            Opt("--minify", "Minify"), Opt("--sourcemap", "Write source maps"), Opt("--compile", "Make an executable"),
            Opt("--watch", "Rebuild on changes"),
        ], arguments: [.files]),
        Spec("init", "Start a project", options: [Opt("-y|--yes", "Accept the defaults")]),
        Spec("create|c", "Create a project from a template", arguments: [.text("template"), .folder]),
        Spec("upgrade", "Upgrade bun", options: [Opt("--canary", "The canary build")]),
        Spec("pm", "Package manager tools", subcommands: [
            Spec("ls", "List installed packages", options: [Opt("--all", "Dependencies too")]),
            Spec("cache", "Show the cache folder", subcommands: [Spec("rm", "Clear the cache")]),
            Spec("bin", "Show the bin folder", options: [Opt("-g", "The global one")]),
            Spec("trust", "Run a dependency's lifecycle scripts", arguments: [Arg("package", variadic: true)]),
        ]),
        Spec("link", "Link a local package", arguments: [.text("package")]),
        Spec("unlink", "Unlink a local package"),
        Spec("publish", "Publish to npm", options: [Opt("--dry-run", "Check without publishing")]),
        Spec("repl", "Start a REPL"),
    ])

    static let bunx = Spec("bunx", "Run a package's binary with bun", options: [Opt("--bun", "Use bun instead of node")],
                           arguments: [.from(.nodeBins, "package")])

    // MARK: npm, pnpm, npx

    private static let npmInstallOptions: [Opt] = [
        Opt("-D|--save-dev", "As a dev dependency"), Opt("-E|--save-exact", "Pin the exact version"),
        Opt("-O|--save-optional", "As optional"), Opt("-g|--global", "Globally"), Opt("--no-save", "Don't touch package.json"),
    ]

    static let npm = Spec("npm", "Node's package manager", options: [Opt("-v|--version", "Print the version")], subcommands: [
        Spec("install|i|add", "Install dependencies", options: npmInstallOptions + [Opt("--legacy-peer-deps", "Ignore peer dependency conflicts")],
             arguments: [Arg("package", variadic: true)]),
        Spec("ci", "Clean install from the lockfile"),
        Spec("uninstall|remove|rm|un", "Remove dependencies", options: [Opt("-g|--global", "Globally")], arguments: [Arg("package", variadic: true)]),
        Spec("update|up", "Update dependencies", arguments: [Arg("package", variadic: true)]),
        Spec("run|run-script", "Run a package.json script", options: [Opt("--if-present", "Only if it exists"), Opt("-w|--workspace", "In this workspace", argument: .text("name"))],
             arguments: [.from(.packageScripts)]),
        Spec("test|t", "Run the test script"),
        Spec("start", "Run the start script"),
        Spec("exec|x", "Run a package's binary", arguments: [.from(.nodeBins, "package")]),
        Spec("outdated", "List outdated dependencies"),
        Spec("ls|list", "List installed packages", options: [Opt("--depth", "How deep", argument: .text("depth")), Opt("-g|--global", "Globally")]),
        Spec("init", "Create a package.json", options: [Opt("-y|--yes", "Accept the defaults")]),
        Spec("publish", "Publish the package", options: [Opt("--dry-run", "Check without publishing"), Opt("--tag", "Dist tag", argument: .text("tag")),
                                                         Opt("--access", "Access", argument: .values("access", ["public", "restricted"]))]),
        Spec("version", "Bump the version", arguments: [.values("version", ["patch", "minor", "major", "prerelease"])]),
        Spec("audit", "Check for vulnerabilities", subcommands: [Spec("fix", "Fix them")]),
        Spec("link", "Link a local package"),
        Spec("pack", "Make a tarball"),
        Spec("view|info", "Show a package's registry info", arguments: [.text("package")]),
        Spec("login", "Sign in to the registry"), Spec("whoami", "Show who you're signed in as"),
        Spec("cache", "Manage the cache", subcommands: [Spec("clean", "Clear it", options: [Opt("--force", "Really")]), Spec("verify", "Check it")]),
    ])

    static let pnpm = Spec("pnpm", "Fast, disk-efficient package manager", options: [
        Opt("-v|--version", "Print the version"),
        Opt("-F|--filter", "In these workspace packages", argument: .text("pattern"), persistent: true),
        Opt("-r|--recursive", "In every workspace package", persistent: true),
        Opt("-C|--dir", "Run in this folder", argument: .folder, persistent: true),
    ], arguments: [Arg("script", generators: [.packageScripts])], subcommands: [
        Spec("install|i", "Install dependencies", options: [Opt("--frozen-lockfile", "Fail if the lockfile would change"), Opt("-P|--prod", "Without dev dependencies")]),
        Spec("add", "Add dependencies", options: [Opt("-D|--save-dev", "As a dev dependency"), Opt("-E|--save-exact", "Pin the exact version"),
                                                  Opt("-g|--global", "Globally"), Opt("-w|--workspace-root", "To the workspace root")],
             arguments: [Arg("package", variadic: true)]),
        Spec("remove|rm|uninstall", "Remove dependencies", arguments: [Arg("package", variadic: true)]),
        Spec("update|up", "Update dependencies", options: [Opt("-L|--latest", "Past the version range"), Opt("-i|--interactive", "Pick which")],
             arguments: [Arg("package", variadic: true)]),
        Spec("run", "Run a package.json script", arguments: [.from(.packageScripts)]),
        Spec("exec", "Run a package's binary", arguments: [.from(.nodeBins, "package")]),
        Spec("dlx", "Fetch and run a package", arguments: [.text("package")]),
        Spec("test|t", "Run the test script"), Spec("start", "Run the start script"),
        Spec("outdated", "List outdated dependencies"), Spec("why", "Show why a package is installed", arguments: [.text("package")]),
        Spec("list|ls", "List installed packages", options: [Opt("--depth", "How deep", argument: .text("depth"))]),
        Spec("init", "Create a package.json"), Spec("publish", "Publish the package"),
        Spec("store", "Manage the store", subcommands: [Spec("prune", "Remove unused packages"), Spec("path", "Print the store folder")]),
    ])

    static let npx = Spec("npx", "Run a package's binary", options: [Opt("-y|--yes", "Install without asking"), Opt("-p|--package", "From this package", argument: .text("package"))],
                          arguments: [.from(.nodeBins, "package")])

    // MARK: just

    static let just = Spec("just", "Run project recipes", options: [
        Opt("-l|--list", "List recipes"), Opt("--summary", "Recipe names only"), Opt("--choose", "Pick a recipe with a fuzzy finder"),
        Opt("-n|--dry-run", "Print the commands without running them"), Opt("-s|--show", "Print a recipe", argument: .from(.justRecipes)),
        Opt("-f|--justfile", "Use this justfile", argument: .file), Opt("-d|--working-directory", "Run in this folder", argument: .folder),
        Opt("--set", "Override a variable", argument: .text("variable")), Opt("--evaluate", "Print the variables"),
        Opt("-e|--edit", "Open the justfile in $EDITOR"), Opt("--fmt", "Format the justfile"), Opt("--init", "Create a justfile"),
        Opt("-v|--verbose", "More output", repeatable: true), Opt("-q|--quiet", "Less output"),
    ], arguments: [.from(.justRecipes, variadic: true)])

    // MARK: ssh

    static let ssh = Spec("ssh", "Log in to a remote machine", options: [
        Opt("-p", "Port", argument: .text("port")), Opt("-l", "User name", argument: .text("user")),
        Opt("-i", "Identity (private key) file", argument: .file), Opt("-J", "Jump through this host", argument: .from(.sshHosts)),
        Opt("-L", "Forward a local port", argument: .text("port:host:hostport")), Opt("-R", "Forward a remote port", argument: .text("port:host:hostport")),
        Opt("-D", "SOCKS proxy on a local port", argument: .text("port")),
        Opt("-o", "Set an option", argument: .values("option", ["ServerAliveInterval=", "StrictHostKeyChecking=", "SendEnv=", "SetEnv=", "ProxyJump=", "ForwardAgent="])),
        Opt("-F", "Use this config file", argument: .file),
        Opt("-A", "Forward the agent"), Opt("-N", "No command, just forwarding"), Opt("-t", "Force a terminal"), Opt("-T", "No terminal"),
        Opt("-C", "Compress"), Opt("-q", "Quiet"), Opt("-v", "Verbose", repeatable: true), Opt("-4", "IPv4 only"), Opt("-6", "IPv6 only"),
    ], arguments: [.from(.sshHosts), Arg("command")])

    // MARK: Claude Code and Codex

    static let claude = Spec("claude", "Claude Code", options: [
        Opt("-c|--continue", "Continue the last conversation here"),
        Opt("-r|--resume", "Resume a conversation", argument: .from(.claudeSessions)),
        Opt("-p|--print", "Print the answer and exit"),
        Opt("--model", "Model", argument: .values("model", described: [("opus", "Most capable"), ("sonnet", "Fast and capable"), ("haiku", "Fastest"), ("fable", "Fable")])),
        Opt("--effort", "Effort level", argument: .values("level", ["low", "medium", "high", "xhigh", "max"])),
        Opt("--permission-mode", "Permission mode", argument: .values("mode", described: [
            ("acceptEdits", "Edit without asking"), ("auto", "Decide what to ask about"), ("plan", "Plan only"),
            ("manual", "Ask for everything"), ("dontAsk", "Don't ask; deny what isn't allowed"), ("bypassPermissions", "Never ask"),
        ])),
        Opt("--dangerously-skip-permissions", "Never ask for permission"),
        Opt("--allow-dangerously-skip-permissions", "Allow skipping permissions as an option"),
        Opt("--output-format", "With --print", argument: .values("format", ["text", "json", "stream-json"])),
        Opt("--add-dir", "Give access to another folder", argument: .folder, repeatable: true),
        Opt("--agent", "Agent for the session", argument: .text("agent")),
        Opt("-w|--worktree", "Work in a git worktree", argument: .text("name")),
        Opt("-n|--name", "Name the session", argument: .text("name")),
        Opt("--session-id", "Use this session id", argument: .text("uuid")),
        Opt("--fork-session", "Resume into a new session"),
        Opt("--system-prompt", "Replace the system prompt", argument: .text("prompt")),
        Opt("--append-system-prompt", "Add to the system prompt", argument: .text("prompt")),
        Opt("--allowedTools|--allowed-tools", "Tools allowed without asking", argument: .text("tools")),
        Opt("--disallowedTools|--disallowed-tools", "Tools not allowed", argument: .text("tools")),
        Opt("--mcp-config", "Load MCP servers from a file", argument: .file),
        Opt("--strict-mcp-config", "Only those MCP servers"),
        Opt("--settings", "Settings file or JSON", argument: .file),
        Opt("--plugin-dir", "Load plugins from a folder", argument: .folder),
        Opt("--bg|--background", "Start in the background"), Opt("--tmux", "Open in tmux"), Opt("--chrome", "Use Claude in Chrome"),
        Opt("--ide", "Connect to your IDE"), Opt("--verbose", "More output"), Opt("-d|--debug", "Debug mode"),
        Opt("--bare", "Minimal mode"), Opt("--no-session-persistence", "Don't save the session"),
        Opt("-v|--version", "Print the version"), Opt("-h|--help", "Show help"),
    ], arguments: [.text("prompt")], subcommands: [
        Spec("agents", "List background sessions"),
        Spec("attach", "Attach to a background session", arguments: [.from(.claudeSessions)]),
        Spec("logs", "Show a background session's output", arguments: [.from(.claudeSessions)]),
        Spec("rm", "Remove a background session", arguments: [.from(.claudeSessions)]),
        Spec("auth", "Manage sign-in", subcommands: [Spec("login", "Sign in"), Spec("logout", "Sign out"), Spec("status", "Show who's signed in")]),
        Spec("mcp", "Manage MCP servers", subcommands: [
            Spec("list", "List servers"), Spec("get", "Show a server", arguments: [.text("name")]),
            Spec("add", "Add a server", options: [
                Opt("-s|--scope", "Where to save it", argument: .values("scope", ["local", "project", "user"])),
                Opt("-t|--transport", "Transport", argument: .values("transport", ["stdio", "http", "sse"])),
                Opt("-e|--env", "Environment variable", argument: .text("KEY=value"), repeatable: true),
            ], arguments: [.text("name"), .text("command or url")]),
            Spec("remove", "Remove a server", arguments: [.text("name")]),
        ]),
        Spec("doctor", "Check the installation"), Spec("install", "Install a version", arguments: [.values("version", ["stable", "latest"])]),
        Spec("setup-token", "Make a long-lived token"),
    ])

    static let codex = Spec("codex", "OpenAI Codex", options: [
        Opt("-m|--model", "Model", argument: .text("model"), persistent: true),
        Opt("-s|--sandbox", "Sandbox", argument: .values("mode", described: [
            ("read-only", "Read only"), ("workspace-write", "Write in the workspace"), ("danger-full-access", "No sandbox"),
        ]), persistent: true),
        Opt("-a|--ask-for-approval", "When to ask", argument: .values("policy", described: [
            ("untrusted", "For untrusted commands"), ("on-request", "When the model asks"), ("never", "Never"),
        ]), persistent: true),
        Opt("-C|--cd", "Work in this folder", argument: .folder, persistent: true),
        Opt("-c|--config", "Override a setting", argument: .text("key=value"), persistent: true, repeatable: true),
        Opt("-p|--profile", "Config profile", argument: .text("profile"), persistent: true),
        Opt("-i|--image", "Attach images", argument: .files, persistent: true),
        Opt("--add-dir", "Give access to another folder", argument: .folder, persistent: true, repeatable: true),
        Opt("--search", "Allow web search", persistent: true), Opt("--oss", "Use a local model", persistent: true),
        Opt("--worktree", "Work in a git worktree"), Opt("--dangerously-bypass-approvals-and-sandbox", "Never ask, no sandbox"),
        Opt("-V|--version", "Print the version"), Opt("-h|--help", "Show help"),
    ], arguments: [.text("prompt")], subcommands: [
        Spec("exec|e", "Run without the interface", options: [
            Opt("--json", "Print events as JSON"), Opt("-o|--output-last-message", "Write the last message to a file", argument: .file),
            Opt("--skip-git-repo-check", "Run outside a git repository"), Opt("--ephemeral", "Don't save the session"),
        ], arguments: [.text("prompt")]),
        Spec("resume", "Resume a session", options: [Opt("--last", "The most recent one"), Opt("--all", "From every folder")],
             arguments: [.from(.codexSessions)]),
        Spec("fork", "Fork a session", options: [Opt("--last", "The most recent one")], arguments: [.from(.codexSessions)]),
        Spec("archive", "Archive a session", arguments: [.from(.codexSessions)]),
        Spec("unarchive", "Unarchive a session", arguments: [.from(.codexSessions)]),
        Spec("delete", "Delete a session", arguments: [.from(.codexSessions)]),
        Spec("review", "Review code without the interface"),
        Spec("apply|a", "Apply the last diff with git apply"),
        Spec("login", "Sign in", subcommands: [Spec("status", "Show who's signed in")]), Spec("logout", "Sign out"),
        Spec("mcp", "Manage MCP servers", subcommands: [
            Spec("list", "List servers"), Spec("get", "Show a server", arguments: [.text("name")]),
            Spec("add", "Add a server", arguments: [.text("name"), .text("command")]), Spec("remove", "Remove a server", arguments: [.text("name")]),
        ]),
        Spec("update", "Update Codex"), Spec("doctor", "Check the installation"), Spec("app", "Open the desktop app"),
        Spec("cloud", "Browse Codex Cloud tasks"), Spec("features", "Show feature flags"),
        Spec("completion", "Print shell completions", arguments: [.values("shell", ["zsh", "bash", "fish"])]),
    ])

    // MARK: Files

    static let fileCommands: [Spec] = [
        Spec("rm", "Remove files", options: [
            Opt("-r|-R", "Folders and their contents"), Opt("-f", "Don't ask, ignore missing files"), Opt("-i", "Ask for each"),
            Opt("-v", "List what's removed"), Opt("-d", "Empty folders too"),
        ], arguments: [.files]),
        Spec("rmdir", "Remove empty folders", options: [Opt("-p", "Parents too")], arguments: [Arg("folder", generator: .folders, variadic: true)]),
        Spec("mkdir", "Make folders", options: [Opt("-p", "Parents too, no error if it exists"), Opt("-v", "List what's made")],
             arguments: [Arg("folder", generator: .folders, variadic: true)]),
        Spec("cd", "Change folder", arguments: [.folder]),
        Spec("cp", "Copy files", options: [Opt("-r|-R", "Folders too"), Opt("-i", "Ask before overwriting"), Opt("-n", "Never overwrite"),
                                           Opt("-v", "List what's copied"), Opt("-p", "Keep dates and permissions")], arguments: [.files]),
        Spec("mv", "Move or rename files", options: [Opt("-i", "Ask before overwriting"), Opt("-n", "Never overwrite"), Opt("-f", "Don't ask"),
                                                     Opt("-v", "List what's moved")], arguments: [.files]),
        Spec("zed", "Open in Zed", options: [Opt("-n|--new", "In a new window"), Opt("-a|--add", "Add to the current window"),
                                             Opt("-w|--wait", "Wait until closed"), Opt("--diff", "Compare two files", argument: .file)],
             arguments: [.files]),
        Spec("code", "Open in VS Code", options: [Opt("-n|--new-window", "In a new window"), Opt("-r|--reuse-window", "In the last window"),
                                                  Opt("-w|--wait", "Wait until closed"), Opt("-d|--diff", "Compare two files", argument: .file),
                                                  Opt("-g|--goto", "Open at file:line", argument: .file)],
             arguments: [.files]),
        Spec("open", "Open with the default app", options: [Opt("-a", "With this app", argument: .text("app")), Opt("-R", "Show in Finder"),
                                                            Opt("-e", "In TextEdit"), Opt("-t", "In the default text editor"), Opt("-n", "A new instance")],
             arguments: [.files]),
        Spec("cat", "Print files", options: [Opt("-n", "Number lines")], arguments: [.files]),
        Spec("less", "Page through a file", arguments: [.files]),
        Spec("bat", "Print files with syntax colors", arguments: [.files]),
        Spec("touch", "Create files or update their date", arguments: [.files]),
        Spec("head", "First lines of a file", options: [Opt("-n", "How many lines", argument: .text("count"))], arguments: [.files]),
        Spec("tail", "Last lines of a file", options: [Opt("-n", "How many lines", argument: .text("count")), Opt("-f", "Follow new lines")],
             arguments: [.files]),
    ]
}
