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
