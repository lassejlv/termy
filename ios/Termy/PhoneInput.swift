import AVFoundation
import Observation
import Speech
import SwiftUI
import TermyRemoteKit
import UIKit

/// Photos and screenshots made small enough for the relay.
enum ImageUpload {
    /// The bytes to send and a name: the original when it already fits, else a JPEG scaled down until it does.
    static func prepare(_ data: Data, name: String? = nil) -> (data: Data, name: String)? {
        let sniffed = RemoteUpload.sniffedExtension(data)
        if data.count <= RemoteUpload.maxBytes, sniffed == "png" || sniffed == "jpg" || sniffed == "gif" {
            return (data, name ?? "image.\(sniffed!)")
        }
        guard let image = UIImage(data: data) else { return nil }
        return prepare(image, name: name)
    }

    static func prepare(_ image: UIImage, name: String? = nil) -> (data: Data, name: String)? {
        let base = ((name ?? "image") as NSString).deletingPathExtension
        var side: CGFloat = 2560
        while side >= 320 {
            let scale = min(1, side / max(image.size.width, image.size.height, 1))
            let size = CGSize(width: image.size.width * scale, height: image.size.height * scale)
            let format = UIGraphicsImageRendererFormat()
            format.scale = 1
            let scaled = UIGraphicsImageRenderer(size: size, format: format).image { _ in
                image.draw(in: CGRect(origin: .zero, size: size))
            }
            if let jpeg = scaled.jpegData(compressionQuality: 0.8), jpeg.count <= RemoteUpload.maxBytes {
                return (jpeg, base + ".jpg")
            }
            side *= 0.75
        }
        return nil
    }
}

/// A spoken command as a shell would want it: symbols spoken as words become symbols.
///
/// "ls dash l" is `ls -l`, "commit dash dash amend" is `commit --amend`,
/// "cd tilde slash dev" is `cd ~/dev`, "readme dot md" is `readme.md` and
/// "ps pipe grep node" is `ps | grep node`.
enum SpokenCommand {
    private static let symbols: [String: String] = [
        "dash": "-", "slash": "/", "dot": ".", "tilde": "~", "underscore": "_", "pipe": "|", "ampersand": "&",
        "star": "*", "equals": "=",
    ]
    /// Symbols that join the word before them.
    private static let glueBefore: Set<String> = ["/", ".", "_", "="]
    /// Symbols that join the word after them.
    private static let glueAfter: Set<String> = ["-", "/", ".", "_", "~", "="]

    static func normalize(_ spoken: String) -> String {
        var text = spoken.trimmingCharacters(in: .whitespacesAndNewlines)
        while let last = text.last, ".!?".contains(last) { text.removeLast() }
        var tokens: [String] = []
        for word in text.split(separator: " ").map(String.init) {
            let symbol = symbols[word.lowercased()]
            if symbol == "-", tokens.last == "-" {
                tokens[tokens.count - 1] = "--"
            } else {
                tokens.append(symbol ?? word)
            }
        }
        var result = ""
        for token in tokens {
            let joins = result.isEmpty || glueBefore.contains(token) || glueAfter.contains(String(result.suffix(1)))
            result += joins ? token : " " + token
        }
        // Speech capitalizes the first word; commands are lowercase.
        if let first = result.first, first.isUppercase, result.dropFirst().first.map({ !$0.isUppercase }) ?? true {
            result = first.lowercased() + result.dropFirst()
        }
        return result
    }
}

/// Live, private speech-to-text: on the phone when it can, for a command the user checks before it's typed.
@MainActor @Observable
final class Dictation {
    private(set) var transcript = ""
    private(set) var listening = false
    private(set) var error: String?
    @ObservationIgnored private let engine = AVAudioEngine()
    @ObservationIgnored private var request: SFSpeechAudioBufferRecognitionRequest?
    @ObservationIgnored private var task: SFSpeechRecognitionTask?
    @ObservationIgnored private let recognizer = SFSpeechRecognizer()

    func start() async {
        error = nil
        let speech = await withCheckedContinuation { continuation in
            SFSpeechRecognizer.requestAuthorization { continuation.resume(returning: $0) }
        }
        guard speech == .authorized else { return fail("Allow Speech Recognition in Settings to speak commands.") }
        guard await AVAudioApplication.requestRecordPermission() else {
            return fail("Allow the microphone in Settings to speak commands.")
        }
        guard let recognizer, recognizer.isAvailable else { return fail("Speech recognition isn’t available right now.") }
        do {
            let session = AVAudioSession.sharedInstance()
            try session.setCategory(.record, mode: .measurement, options: .duckOthers)
            try session.setActive(true, options: .notifyOthersOnDeactivation)
            let request = SFSpeechAudioBufferRecognitionRequest()
            request.shouldReportPartialResults = true
            request.requiresOnDeviceRecognition = recognizer.supportsOnDeviceRecognition
            request.taskHint = .dictation
            request.contextualStrings = [
                "git", "cd", "ls", "sudo", "npm", "cargo", "brew", "ssh", "grep", "claude", "docker", "make", "pnpm",
            ]
            self.request = request
            let input = engine.inputNode
            input.installTap(onBus: 0, bufferSize: 1024, format: input.outputFormat(forBus: 0)) { buffer, _ in
                request.append(buffer)
            }
            engine.prepare()
            try engine.start()
            listening = true
            task = recognizer.recognitionTask(with: request) { [weak self] result, error in
                let text = result?.bestTranscription.formattedString
                let final = result?.isFinal ?? false
                Task { @MainActor in
                    guard let self else { return }
                    if let text { self.transcript = SpokenCommand.normalize(text) }
                    if final || error != nil { self.stop() }
                }
            }
        } catch {
            stop()
            fail("Couldn’t start listening.")
        }
    }

    func stop() {
        guard listening || task != nil else { return }
        engine.stop()
        engine.inputNode.removeTap(onBus: 0)
        request?.endAudio()
        task?.cancel()
        request = nil
        task = nil
        listening = false
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    }

    private func fail(_ message: String) {
        error = message
        listening = false
    }
}

/// Speak a command, check and edit it, then type or run it in the terminal.
struct DictationSheet: View {
    let onType: (String, Bool) -> Void
    @State private var dictation = Dictation()
    @State private var command = ""
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("Command", text: $command, axis: .vertical)
                        .font(.body.monospaced()).textInputAutocapitalization(.never).autocorrectionDisabled()
                        .lineLimit(1...6)
                } footer: {
                    if let error = dictation.error {
                        Text(error).foregroundStyle(.red)
                    } else {
                        Text("Say symbols as words: “dash”, “slash”, “dot”, “pipe”. Nothing is sent until you choose Type or Run.")
                    }
                }
                Section {
                    Button(dictation.listening ? "Stop Listening" : "Listen Again", systemImage: dictation.listening ? "stop.circle.fill" : "mic.fill") {
                        if dictation.listening { dictation.stop() } else { Task { await dictation.start() } }
                    }
                }
            }
            .navigationTitle(dictation.listening ? "Listening…" : "Speak a Command").navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItemGroup(placement: .bottomBar) {
                    Button("Type") { finish(run: false) }.disabled(command.isEmpty)
                    Spacer()
                    Button("Run", systemImage: "return") { finish(run: true) }
                        .buttonStyle(.borderedProminent).disabled(command.isEmpty)
                }
            }
            .onChange(of: dictation.transcript) { _, text in command = text }
            .task { await dictation.start() }
            .onDisappear { dictation.stop() }
        }
        .presentationDetents([.medium])
    }

    private func finish(run: Bool) {
        dictation.stop()
        onType(command, run)
        dismiss()
    }
}

/// Moves a value made on one thread to another, for UIKit types that are safe to hand over but not marked so.
struct Handoff<Value>: @unchecked Sendable {
    let value: Value
    init(_ value: Value) { self.value = value }
}
