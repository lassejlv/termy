import Foundation
import Testing
@testable import TermyNextCore

@Suite struct NextPathsTests {
    @Test func normalInstanceUsesUserDirectories() {
        let paths = NextPaths(
            environment: [:],
            homeDirectory: URL(fileURLWithPath: "/Users/me"),
            applicationSupportDirectory: URL(fileURLWithPath: "/Users/me/Library/Application Support")
        )
        #expect(paths.configurationDirectory.path == "/Users/me/.config/termy-next")
        #expect(paths.multiplexerDirectory.path == "/Users/me/Library/Application Support/TermyNext/multiplexer")
        #expect(paths.shellDirectory.path == "/Users/me/Library/Application Support/TermyNext/shell")
        #expect(paths.layoutFile.path == "/Users/me/Library/Application Support/TermyNext/layout.json")
    }

    @Test func isolatedInstanceKeepsConfigAndSupportUnderItsHome() {
        let paths = NextPaths(
            environment: ["TERMY_NEXT_HOME": "/tmp/isolated instance"],
            homeDirectory: URL(fileURLWithPath: "/Users/me"),
            applicationSupportDirectory: URL(fileURLWithPath: "/Users/me/Library/Application Support")
        )
        #expect(paths.configurationDirectory.path == "/tmp/isolated instance/config")
        #expect(paths.multiplexerDirectory.path == "/tmp/isolated instance/support/multiplexer")
        #expect(paths.shellDirectory.path == "/tmp/isolated instance/support/shell")
        #expect(paths.layoutFile.path == "/tmp/isolated instance/support/layout.json")
    }
}
