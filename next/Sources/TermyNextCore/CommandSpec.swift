import Foundation

// Command completion, Termy Next's own: a spec describes a command's
// subcommands, options and arguments (see CommandSpecs.swift), and
// `CommandSpec.complete` reads a command line against it to say what can
// come next. Arguments can draw on generators, which run a program in the
// pane's directory (`git branch`, `brew list`) or list files.

/// A command or subcommand.
public struct CommandSpec: Sendable {
    public var names: [String]
    public var description: String?
    public var subcommands: [CommandSpec]
    public var options: [Option]
    public var arguments: [Argument]

    /// `names` are "|"-separated aliases, such as "rm|remove".
    public init(_ names: String, _ description: String? = nil, options: [Option] = [], arguments: [Argument] = [],
                subcommands: [CommandSpec] = []) {
        self.names = names.split(separator: "|").map(String.init)
        self.description = description
        self.options = options
        self.arguments = arguments
        self.subcommands = subcommands
    }

    public struct Option: Sendable {
        public var names: [String]
        public var description: String?
        /// What follows the option, such as the message after `-m`.
        public var argument: Argument?
        /// Valid in every subcommand below the one that has it.
        public var isPersistent: Bool
        public var isRepeatable: Bool

        /// `names` are "|"-separated, such as "-m|--message".
        public init(_ names: String, _ description: String? = nil, argument: Argument? = nil,
                    persistent: Bool = false, repeatable: Bool = false) {
            self.names = names.split(separator: "|").map(String.init)
            self.description = description
            self.argument = argument
            self.isPersistent = persistent
            self.isRepeatable = repeatable
        }
    }

    public struct Argument: Sendable {
        public var name: String
        public var suggestions: [(name: String, description: String?)]
        /// Sources for more suggestions, in order: `bun dev` can be a script or a file.
        public var generators: [Generator]
        public var isVariadic: Bool

        public init(_ name: String, suggestions: [(name: String, description: String?)] = [], generators: [Generator] = [],
                    variadic: Bool = false) {
            self.name = name
            self.suggestions = suggestions
            self.generators = generators
            self.isVariadic = variadic
        }

        public init(_ name: String, generator: Generator, variadic: Bool = false) {
            self.init(name, generators: [generator], variadic: variadic)
        }

        /// Free text, such as a commit message: nothing to suggest.
        public static func text(_ name: String) -> Argument { Argument(name) }
        public static func from(_ generator: Generator, _ name: String? = nil, variadic: Bool = false) -> Argument {
            Argument(name ?? generator.rawValue, generator: generator, variadic: variadic)
        }
        public static func values(_ name: String, _ values: [String]) -> Argument {
            Argument(name, suggestions: values.map { ($0, nil) })
        }
        public static func values(_ name: String, described values: [(String, String)]) -> Argument {
            Argument(name, suggestions: values.map { ($0.0, $0.1) })
        }
        public static let files = Argument.from(.files, "path", variadic: true)
        public static let file = Argument.from(.files, "path")
        public static let folder = Argument.from(.folders, "folder")
    }
}

/// What may come at the end of a command line.
public struct SpecCompletion: Equatable, Sendable {
    public enum Kind: Equatable, Sendable { case subcommand, option, argument }

    public struct Item: Equatable, Sendable {
        public var name: String
        public var description: String?
        public var kind: Kind
        /// A space follows once it's taken; not after a folder ("src/").
        public var appendsSpace: Bool

        public init(name: String, description: String? = nil, kind: Kind, appendsSpace: Bool = true) {
            self.name = name
            self.description = description
            self.kind = kind
            self.appendsSpace = appendsSpace
        }
    }

    /// Character offset in the line where the word being typed starts.
    public var wordStart: Int
    /// The word being typed, which every item starts with.
    public var word: String
    /// The command the spec is for, such as "git".
    public var command: String
    public var items: [Item]
    /// Run for more items, matched against `word` with `Generator.matching`.
    public var generators: [Generator]
    public var generator: Generator? { generators.first }

    /// The line with `item` in place of the word being typed.
    public func line(_ buffer: String, taking item: Item) -> String {
        String(buffer.prefix(wordStart)) + item.name + (item.appendsSpace ? " " : "")
    }
}

extension CommandSpec {
    /// Reads `line` against this spec (whose command is the first word of
    /// the line's last command) and says what can come next. Nil while the
    /// command itself is still being typed.
    public func complete(_ line: String) -> SpecCompletion? {
        let words = ShellWords.split(line)
        guard words.count >= 2 else { return nil }
        let current = words[words.count - 1]
        var node = self
        var persistent: [Option] = []
        var used: Set<String> = []
        var pending: Argument?
        var positional = 0
        var sawPositional = false

        for word in words.dropFirst().dropLast() {
            if pending != nil {
                pending = nil
                continue
            }
            let text = word.text
            if text == "--" { continue }
            if text.hasPrefix("-"), text.count > 1 {
                let name = String(text.split(separator: "=", maxSplits: 1).first ?? "")
                if let option = (node.options + persistent).first(where: { $0.names.contains(name) }) {
                    used.formUnion(option.names)
                    if !text.contains("=") { pending = option.argument }
                }
                continue
            }
            if !sawPositional, let sub = node.subcommands.first(where: { $0.names.contains(text) }) {
                persistent += node.options.filter(\.isPersistent)
                node = sub
                used = []
                continue
            }
            sawPositional = true
            if node.arguments.indices.contains(positional), !node.arguments[positional].isVariadic { positional += 1 }
        }

        let typed = current.text
        var items: [SpecCompletion.Item] = []
        var generators: [Generator] = []
        func add(_ argument: Argument) {
            for suggestion in argument.suggestions where suggestion.name.hasPrefix(typed) && suggestion.name != typed {
                items.append(.init(name: suggestion.name, description: suggestion.description, kind: .argument))
            }
            generators += argument.generators.filter { !generators.contains($0) }
        }
        func matching(_ names: [String]) -> String? {
            names.first { $0.hasPrefix(typed) && $0 != typed }
        }

        if let pending {
            add(pending)
        } else if typed.hasPrefix("-") {
            for option in node.options + persistent {
                if !option.isRepeatable, !used.isDisjoint(with: option.names) { continue }
                // "--message" for "--", "-m" for "-".
                let names = typed.hasPrefix("--") ? option.names : option.names.sorted { $0.count < $1.count }
                guard let name = matching(names) else { continue }
                items.append(.init(name: name, description: option.description, kind: .option))
            }
        } else {
            if node.arguments.indices.contains(positional) {
                add(node.arguments[positional])
            } else if let last = node.arguments.last, last.isVariadic {
                add(last)
            }
            if !sawPositional {
                for sub in node.subcommands {
                    guard let name = matching(sub.names) else { continue }
                    items.append(.init(name: name, description: sub.description, kind: .subcommand))
                }
            }
        }
        return SpecCompletion(wordStart: current.start, word: typed, command: words[0].text, items: items, generators: generators)
    }
}

/// Splits a command line into words the way a shell would, enough for
/// completion: quotes group, a backslash escapes, and only the last command
/// after `|`, `;` or `&&` counts. A line ending in a space ends with an
/// empty word.
public enum ShellWords {
    public struct Word: Equatable, Sendable {
        public var text: String
        /// Character offset in the line.
        public var start: Int
    }

    public static func split(_ line: String) -> [Word] {
        let characters = Array(line)
        var segmentStart = 0
        var quote: Character?
        var escaped = false
        for (index, character) in characters.enumerated() {
            if escaped { escaped = false; continue }
            if character == "\\", quote != "'" { escaped = true; continue }
            if let q = quote {
                if character == q { quote = nil }
            } else if character == "'" || character == "\"" {
                quote = character
            } else if character == ";" || character == "|" || character == "&" {
                segmentStart = index + 1
            }
        }

        var words: [Word] = []
        var text = ""
        var start: Int?
        quote = nil
        escaped = false
        for index in segmentStart..<characters.count {
            let character = characters[index]
            if escaped {
                text.append(character)
                escaped = false
            } else if character == "\\", quote != "'" {
                escaped = true
                if start == nil { start = index }
            } else if let q = quote {
                if character == q { quote = nil } else { text.append(character) }
            } else if character == "'" || character == "\"" {
                quote = character
                if start == nil { start = index }
            } else if character.isWhitespace {
                if let s = start {
                    words.append(Word(text: text, start: s))
                    text = ""
                    start = nil
                }
            } else {
                if start == nil { start = index }
                text.append(character)
            }
        }
        words.append(start.map { Word(text: text, start: $0) } ?? Word(text: "", start: characters.count))
        return words
    }
}
