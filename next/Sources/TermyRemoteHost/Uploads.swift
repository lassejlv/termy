import Foundation
import TermyRemoteKit

/// Files phones send into panes, such as a photo for Claude Code to read.
///
/// They go in a folder only this user can read, under the temporary
/// directory, and are deleted after a week.
enum Uploads {
    static let keepFor: TimeInterval = 7 * 24 * 3600

    static var folder: URL {
        FileManager.default.temporaryDirectory.appendingPathComponent("TermyUploads", isDirectory: true)
    }

    /// Saves `data` and returns its path. The name is the phone's, made safe and dated.
    static func save(_ data: Data, name: String, in folder: URL = folder, now: Date = Date()) throws -> String {
        guard !data.isEmpty, data.count <= RemoteUpload.maxBytes else { throw CocoaError(.fileWriteFileExists) }
        let manager = FileManager.default
        try manager.createDirectory(at: folder, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        try manager.setAttributes([.posixPermissions: 0o700], ofItemAtPath: folder.path)
        prune(folder, now: now)
        let sniffed = RemoteUpload.sniffedExtension(data)
        var safe = RemoteUpload.fileName(name, fallbackExtension: sniffed ?? "bin")
        // A photo named .heic but sent as JPEG opens as what it is.
        if let sniffed, (safe as NSString).pathExtension.lowercased() != sniffed {
            safe = (safe as NSString).deletingPathExtension + "." + sniffed
        }
        let stamp = Self.stamp.string(from: now)
        var url = folder.appendingPathComponent("\(stamp)-\(safe)")
        var attempt = 1
        while manager.fileExists(atPath: url.path) {
            attempt += 1
            url = folder.appendingPathComponent("\(stamp)-\(attempt)-\(safe)")
        }
        guard manager.createFile(atPath: url.path, contents: data, attributes: [.posixPermissions: 0o600]) else {
            throw CocoaError(.fileWriteUnknown)
        }
        return url.path
    }

    static func prune(_ folder: URL, now: Date) {
        let manager = FileManager.default
        guard
            let files = try? manager.contentsOfDirectory(
                at: folder, includingPropertiesForKeys: [.contentModificationDateKey])
        else { return }
        for file in files {
            let modified = (try? file.resourceValues(forKeys: [.contentModificationDateKey]))?.contentModificationDate
            if let modified, now.timeIntervalSince(modified) > keepFor { try? manager.removeItem(at: file) }
        }
    }

    private static let stamp: DateFormatter = {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyyMMdd-HHmmss"
        return formatter
    }()
}
