import CoreGraphics
import Foundation
import Testing
@testable import TermyNextCore

@Suite struct KeyChordTests {
    @Test func parsesRexSpecs() {
        #expect(KeyChord("shift+cmd+a") == KeyChord(key: "a", modifiers: [.shift, .command]))
        #expect(KeyChord("ctrl+g") == KeyChord(key: "g", modifiers: [.control]))
        #expect(KeyChord("alt+Return") == KeyChord(key: "enter", modifiers: [.option]))
        #expect(KeyChord("cmd++") == KeyChord(key: "+", modifiers: [.command]))
        #expect(KeyChord("ctrl+A") == KeyChord(key: "a", modifiers: [.control, .shift]))
        #expect(KeyChord("F12") == KeyChord(key: "f12", modifiers: []))
        #expect(KeyChord("ctrl+hoem") == nil)
        #expect(KeyChord("hyper+a") == nil)
        #expect(KeyChord("") == nil)
    }

    @Test func printsInOneOrder() {
        #expect(KeyChord("cmd+shift+a")?.description == "shift+cmd+a")
        #expect(KeyChord("shift+cmd+a")?.symbolLabel == "⇧⌘A")
        #expect(KeyChord("ctrl+enter")?.symbolLabel == "⌃↩")
    }
}

@Suite struct PluginLayoutTests {
    @Test func readsLeafsLayout() throws {
        let json: [String: Any] = ["block": ["flavor": "com.superlogical.terminal.shell", "label": "leaf",
                                             "options": ["command": ["/opt/homebrew/bin/leaf", "open"], "cwd": "/tmp"]]]
        let layout = try PluginLayout(json: json)
        #expect(layout == .block(PluginBlockSpec(label: "leaf", command: ["/opt/homebrew/bin/leaf", "open"], cwd: "/tmp")))
    }

    @Test func splitsInLayoutOrder() throws {
        let json: [String: Any] = ["split": ["direction": "horizontal", "ratio": 0.25,
                                             "before": ["block": ["flavor": "com.superlogical.terminal.shell", "label": "a"]],
                                             "after": ["split": ["direction": "vertical",
                                                                 "before": ["block": ["label": "b"]],
                                                                 "after": ["block": ["label": "c"]]]]]]
        let layout = try PluginLayout(json: json)
        #expect(layout.blocks.map(\.label) == ["a", "b", "c"])
        let rects = layout.rects(in: CGRect(x: 0, y: 0, width: 1, height: 1))
        #expect(rects[0] == CGRect(x: 0, y: 0, width: 0.25, height: 1))
        #expect(rects[2] == CGRect(x: 0.25, y: 0.5, width: 0.75, height: 0.5))
        let tree = layout.tree(ids: [1, 2, 3])
        #expect(tree == .split(axis: .horizontal, ratio: 0.25, first: .leaf(1),
                               second: .split(axis: .vertical, ratio: 0.5, first: .leaf(2), second: .leaf(3))))
    }

    @Test func refusesWhatRexRefuses() {
        #expect(throws: PluginLayoutError.self) { try PluginLayout(json: ["block": ["flavor": "com.example.browser"]]) }
        #expect(throws: PluginLayoutError.self) { try PluginLayout(json: ["split": ["direction": "diagonal"]]) }
        #expect(throws: PluginLayoutError.self) { try PluginLayout(json: nil) }
    }

    @Test func splitIDsAndUnitRects() {
        let a = UUID(), b = UUID(), c = UUID()
        let tree = SplitTree<UUID>.split(axis: .horizontal, ratio: 0.5, first: .leaf(a),
                                         second: .split(axis: .vertical, ratio: 0.5, first: .leaf(b), second: .leaf(c)))
        #expect(tree.splitKeys() == [b, c])
        let rects = tree.unitRects()
        #expect(rects[a] == CGRect(x: 0, y: 0, width: 0.5, height: 1))
        #expect(rects[c] == CGRect(x: 0.5, y: 0.5, width: 0.5, height: 0.5))
        let replaced = tree.replacingSplit(c) { _ in .leaf(c) }
        #expect(replaced?.leaves == [a, c])
    }
}

@Suite struct ShellQuoteTests {
    @Test func quotesForSh() {
        #expect(ShellQuote.quote("leaf") == "leaf")
        #expect(ShellQuote.quote("a b") == "'a b'")
        #expect(ShellQuote.quote("it's") == "'it'\\''s'")
        #expect(ShellQuote.quote("") == "''")
        #expect(ShellQuote.loginCommand(["leaf", "open", "my doc.md"], shell: "/bin/zsh")
            == "exec /bin/zsh -l -c 'exec leaf open '\\''my doc.md'\\'''")
    }

    @Test func pluginIDs() {
        let id = UUID()
        #expect(PluginID.uuid(PluginID.make("block", id), kind: "block") == id)
        #expect(PluginID.uuid(PluginID.make("block", id), kind: "session") == nil)
        #expect(PluginID.uuid(id.uuidString) == id)
        #expect(PluginFlavor.terminalMethod("com.superlogical.terminal.write") == "write")
        #expect(PluginFlavor.terminalMethod("sh.termy.terminal.process") == "process")
        #expect(PluginFlavor.terminalMethod("write") == nil)
    }
}

@Suite struct PluginCLITests {
    private func call(_ args: [String]) throws -> (String, [String: Any], PluginCLI.Render) {
        guard case .call(let method, let params, let render, _) = try PluginCLI.plan(args) else {
            Issue.record("not a call")
            return ("", [:], .none)
        }
        return (method, params.value, render)
    }

    @Test func leafsBlockCalls() throws {
        let (method, params, render) = try call(["block", "call", "-s", "session:s", "-b", "block:b", "process"])
        #expect(method == "block.method")
        #expect(params["session_id"] as? String == "session:s")
        #expect(params["block_id"] as? String == "block:b")
        #expect(params["method"] as? String == "process")
        #expect(render == .json)
    }

    @Test func leafsBlockList() throws {
        let (method, params, render) = try call(["block", "ls", "-s", "session:s", "--json", "--short=false"])
        #expect(method == "session.list_blocks")
        #expect(params["session_id"] as? String == "session:s")
        #expect(render == .json)
    }

    @Test func leafsSplit() throws {
        let (method, params, render) = try call([
            "split", "-s", "session:s", "-b", "block:b", "-c", "/tmp", "--split=right", "--focus=false",
            "--label", "claude", "--json", "--", "claude", "hello world",
        ])
        #expect(method == "session.new_split")
        #expect(params["anchor_id"] as? String == "block:b")
        #expect(params["direction"] as? String == "horizontal")
        #expect(params["side"] as? String == "after")
        #expect(params["focus"] as? Bool == false)
        let block = (params["layout"] as? [String: Any])?["block"] as? [String: Any]
        #expect(block?["label"] as? String == "claude")
        let options = block?["options"] as? [String: Any]
        #expect(options?["command"] as? [String] == ["claude", "hello world"])
        #expect(options?["cwd"] as? String == "/tmp")
        #expect(render == .json)
    }

    @Test func ratioIsTheNewPanesShare() throws {
        let (_, below, _) = try call(["split", "--split=below", "--ratio", "30"])
        #expect((below["ratio"] as? Double).map { abs($0 - 0.7) < 1e-9 } == true)
        let (_, above, _) = try call(["split", "--split=above", "--ratio", "30"])
        #expect((above["ratio"] as? Double).map { abs($0 - 0.3) < 1e-9 } == true)
    }

    @Test func apiCallTakesJSON() throws {
        let (method, params, _) = try call(["api", "call", "-s", "session:s", "session.new_layer", #"{"window_id":"window:w","bounds":[0,0,1,1]}"#])
        #expect(method == "session.new_layer")
        #expect(params["session_id"] as? String == "session:s")
        #expect(params["window_id"] as? String == "window:w")
    }

    @Test func doTypesItsPairs() throws {
        let (method, params, _) = try call(["do", "dev_layout", "branch=main", "n=2", "on=true"])
        #expect(method == "client.action")
        let args = params["args"] as? [String: Any]
        #expect(args?["branch"] as? String == "main")
        #expect(args?["n"] as? Int == 2)
        #expect(args?["on"] as? Bool == true)
    }

    @Test func globalFlagsAreIgnored() throws {
        let (method, _, _) = try call(["--autostart=false", "-S", "host", "config", "reload"])
        #expect(method == "server.config_reload")
    }

    @Test func unknownCommandsFail() {
        #expect(throws: PluginCLI.Failure.self) { try PluginCLI.plan(["frobnicate"]) }
        #expect(throws: PluginCLI.Failure.self) { try PluginCLI.plan(["split", "--split=sideways"]) }
    }
}

@Suite struct KeySpecTests {
    @Test func sequencesModesAndPhysicalKeys() {
        let sequence = KeySpec("ctrl+b>c")
        #expect(sequence?.chords == [KeyChord(key: "b", modifiers: [.control]), KeyChord(key: "c", modifiers: [])])
        #expect(sequence?.mode == nil)
        let mode = KeySpec("resize/left")
        #expect(mode?.mode == "resize")
        #expect(mode?.keys == "left")
        #expect(KeySpec("cmd+[KeyD]")?.chords.first?.key == "[KeyD]")
        #expect(KeySpec("root/left") == nil)
        #expect(KeySpec("ctrl+b>hoem") == nil)
        #expect(KeySpec("resize/ctrl+b>c")?.description == "resize/ctrl+b>c")
    }
}

@Suite struct SplitResizeTests {
    @Test func resizesTheNearestSplitAlongAnAxis() {
        let a = UUID(), b = UUID(), c = UUID()
        let tree = SplitTree<UUID>.split(axis: .horizontal, ratio: 0.5, first: .leaf(a),
                                         second: .split(axis: .vertical, ratio: 0.5, first: .leaf(b), second: .leaf(c)))
        #expect(tree.resizing(c, axis: .vertical, share: 0.3) == .split(axis: .horizontal, ratio: 0.5, first: .leaf(a),
                                                                     second: .split(axis: .vertical, ratio: 0.7, first: .leaf(b), second: .leaf(c))))
        #expect(tree.resizing(a, axis: .horizontal, share: 0.25)?.unitRects()[a]?.width == 0.25)
        #expect(SplitTree<UUID>.leaf(a).resizing(a, axis: .horizontal, share: 0.3) == nil)
        #expect(tree.settingRatio(0.2, between: a, and: c, axis: .horizontal) != nil)
        #expect(tree.settingRatio(0.2, between: c, and: a, axis: .horizontal) == nil)
    }
}

@Suite struct PluginCLIMoreTests {
    @Test func globalsKeepTheRest() throws {
        let globals = try PluginCLI.globals(["-S", "studio", "--timeout", "5s", "block", "ls", "--json"])
        #expect(globals.server == "studio")
        #expect(globals.timeout == 5)
        #expect(globals.rest == ["block", "ls", "--json"])
    }

    @Test func newCommands() throws {
        guard case .stream(let method, let params, let json) = try PluginCLI.plan(["events", "--all", "--json"]) else {
            Issue.record("events isn't a stream")
            return
        }
        #expect(method == "events.subscribe")
        #expect(params["all"] as? Bool == true)
        #expect(json)
        guard case .call(let move, let moveParams, _, _) = try PluginCLI.plan(["move", "block:a", "--to", "block:b", "--split=below", "--ratio", "25"]) else {
            Issue.record("move isn't a call")
            return
        }
        #expect(move == "session.move_block")
        #expect(moveParams["side"] as? String == "after")
        #expect((moveParams["ratio"] as? Double).map { abs($0 - 0.75) < 1e-9 } == true)
        guard case .call(let hosts, let hostParams, _, _) = try PluginCLI.plan(["hosts", "add", "studio", "me@studio"]) else {
            Issue.record("hosts add isn't a call")
            return
        }
        #expect(hosts == "hosts.put")
        #expect((hostParams["host"] as? [String: Any])?["endpoints"] as? [String] == ["me@studio"])
        #expect(try PluginCLI.completion("zsh").contains("compdef _rex rex"))
        #expect(throws: PluginCLI.Failure.self) { try PluginCLI.plan(["resize", "block:a"]) }
    }
}
