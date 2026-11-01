import Foundation

/// Subsequence matching for the palette and pickers: every query character
/// must appear in order. Matches at word starts and in runs score higher.
public enum FuzzyMatch {
    /// The score of `candidate` for `query`, or nil when it does not match.
    /// An empty query matches everything with score 0.
    public static func score(_ query: String, in candidate: String) -> Int? {
        let query = Array(query.lowercased().filter { !$0.isWhitespace })
        guard !query.isEmpty else { return 0 }
        let text = Array(candidate.lowercased())
        var score = 0
        var qi = 0
        var previousMatch = -2
        for (ti, ch) in text.enumerated() where qi < query.count {
            guard ch == query[qi] else { continue }
            var bonus = 1
            if ti == previousMatch + 1 { bonus += 4 }
            if ti == 0 || !(text[ti - 1].isLetter || text[ti - 1].isNumber) { bonus += 6 }
            score += bonus
            previousMatch = ti
            qi += 1
        }
        guard qi == query.count else { return nil }
        if text.starts(with: query) { score += 10 }
        let final = score - text.count / 8
        // Scattered single letters ("sess" in "Interface Style: Glass") aren't a match.
        return Double(final) >= Double(query.count) * 3.5 ? final : nil
    }

    /// `items` that match `query`, best first; ties keep their input order.
    public static func filter<T>(_ items: [T], query: String, text: (T) -> String) -> [T] {
        items.enumerated()
            .compactMap { index, item in score(query, in: text(item)).map { (index, item, $0) } }
            .sorted { $0.2 != $1.2 ? $0.2 > $1.2 : $0.0 < $1.0 }
            .map(\.1)
    }
}
