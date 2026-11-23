import AppKit
import Foundation
import Termy
import TermyNextCore

// `bin/rex` (in every pane's PATH) runs this binary as Rex's command-line
// tool; it talks to the running app and exits, without starting one.
let arguments = CommandLine.arguments
// `termy` (Install CLI) is the same tool under Termy's name.
if arguments.count > 1, arguments[1] == "--rex-cli" || arguments[1] == "--termy-cli" {
    if arguments[1] == "--termy-cli" { PluginCLI.program = "termy" }
    exit(PluginCLIRunner.main(Array(arguments.dropFirst(2))))
}
if let name = arguments.first.map({ ($0 as NSString).lastPathComponent }), name == "rex" || name == "termy" {
    PluginCLI.program = name
    exit(PluginCLIRunner.main(Array(arguments.dropFirst())))
}

// Fail at launch, not on the first terminal, when libtermy does not match.
TermyABI.requireCompatibleLibrary()

let app = NSApplication.shared
let delegate = MainActor.assumeIsolated { AppDelegate() }
app.delegate = delegate
app.run()
