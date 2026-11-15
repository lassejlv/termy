import Foundation
import TermyRemoteKit

/// Mosh-style local echo: shows typed characters before the Mac echoes them.
///
/// Predictions are only shown once the program has echoed an earlier one in
/// the same "epoch", so password prompts and full-screen programs that don't
/// echo never get predicted text. Enter, escape, control keys and pastes end
/// the epoch; a wrong or late prediction drops them all.
struct EchoPredictor {
    struct Prediction: Equatable {
        var row: Int
        var col: Int
        var character: Character
        var sentAt: Date
    }

    /// Predictions the host hasn't confirmed yet, in typing order.
    private(set) var pending: [Prediction] = []
    /// Whether this epoch's echo was seen, so predictions may be shown.
    private(set) var confirmed = false
    /// How long the host may take before a prediction counts as wrong.
    var patience: TimeInterval = 1.5

    /// Records a key the phone is about to send.
    mutating func noteKey(_ key: RemoteKey, cursor: RemoteCursor, cols: Int, now: Date = Date()) {
        if !key.control, !key.alt, let text = key.text, text.count == 1, let character = text.first,
            Self.isPredictable(character)
        {
            let anchor = pending.last.map { ($0.row, $0.col + 1) } ?? (cursor.row, cursor.col)
            // Never guess a line wrap.
            guard anchor.1 < cols - 1 else {
                pending.removeAll()
                return
            }
            pending.append(Prediction(row: anchor.0, col: anchor.1, character: character, sentAt: now))
            return
        }
        switch key.key {
        case "backspace" where !key.control && !key.alt:
            if !pending.isEmpty { pending.removeLast() }
        case "left", "right", "up", "down", "home", "end":
            pending.removeAll()
        default:
            // Enter, escape, tab and control keys may change what the program echoes.
            reset()
        }
    }

    /// Text sent as a paste or anything else not typed key by key.
    mutating func reset() {
        pending.removeAll()
        confirmed = false
    }

    /// Checks the predictions against a frame the host sent.
    ///
    /// `character(row, col)` reads the screen after the frame was applied.
    mutating func reconcile(cursor: RemoteCursor, now: Date = Date(), character: (Int, Int) -> Character?) {
        while let first = pending.first {
            if character(first.row, first.col) == first.character, Self.hostPassed(first, cursor: cursor) {
                pending.removeFirst()
                confirmed = true
            } else if Self.hostPassed(first, cursor: cursor) || now.timeIntervalSince(first.sentAt) > patience {
                // The host moved on without drawing what we guessed.
                reset()
                return
            } else {
                return
            }
        }
    }

    /// Drops everything when the oldest prediction waited too long. True if it did.
    mutating func expire(now: Date = Date()) -> Bool {
        guard let first = pending.first, now.timeIntervalSince(first.sentAt) > patience else { return false }
        reset()
        return true
    }

    /// The predictions to draw: none until the epoch's echo was confirmed.
    var visible: [Prediction] { confirmed ? pending : [] }

    private static func hostPassed(_ prediction: Prediction, cursor: RemoteCursor) -> Bool {
        cursor.row > prediction.row || (cursor.row == prediction.row && cursor.col > prediction.col)
    }

    private static func isPredictable(_ character: Character) -> Bool {
        guard let scalar = character.unicodeScalars.first, character.unicodeScalars.count == 1 else { return false }
        // Printable, single-cell ASCII only; wide and combining characters are left to the host.
        return scalar.value >= 0x20 && scalar.value < 0x7f
    }
}
