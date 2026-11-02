import AppKit
import UniformTypeIdentifiers
import Termy

/// Dropping onto a terminal types what was dropped, the way pasting does:
/// files as shell-quoted paths, images without a file (from a browser or a
/// screenshot thumbnail) saved as PNG files first, then their paths, and
/// text or links as they are. A space follows paths, so several drops line
/// up as arguments.
extension TerminalView {
    static let droppableTypes: [NSPasteboard.PasteboardType] =
        [.fileURL, .URL, .png, .tiff, .string] + NSFilePromiseReceiver.readableDraggedTypes.map { NSPasteboard.PasteboardType($0) }

    /// Where dropped images without a file are saved.
    public static var droppedFilesDirectory: URL {
        FileManager.default.temporaryDirectory.appendingPathComponent("termy-drops", isDirectory: true)
    }

    public override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation {
        guard Self.canDrop(sender.draggingPasteboard) else { return [] }
        delegate?.terminalView(self, isDropTarget: true)
        return .copy
    }

    public override func draggingUpdated(_ sender: NSDraggingInfo) -> NSDragOperation {
        Self.canDrop(sender.draggingPasteboard) ? .copy : []
    }

    public override func draggingExited(_ sender: NSDraggingInfo?) {
        delegate?.terminalView(self, isDropTarget: false)
    }

    public override func draggingEnded(_ sender: NSDraggingInfo) {
        delegate?.terminalView(self, isDropTarget: false)
    }

    public override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        delegate?.terminalView(self, isDropTarget: false)
        let pasteboard = sender.draggingPasteboard
        window?.makeFirstResponder(self)

        // Files that don't exist yet (Photos, Mail, screenshot thumbnails)
        // are written to the drop folder first.
        let promises = pasteboard.readObjects(forClasses: [NSFilePromiseReceiver.self]) as? [NSFilePromiseReceiver] ?? []
        if !promises.isEmpty, pasteboard.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true])?.isEmpty ?? true {
            receive(promises)
            return true
        }
        guard let text = Self.dropText(from: pasteboard, savingImagesIn: Self.droppedFilesDirectory) else { return false }
        try? session.paste(text)
        return true
    }

    private func receive(_ promises: [NSFilePromiseReceiver]) {
        let directory = Self.droppedFilesDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let queue = OperationQueue()
        let group = DispatchGroup()
        let lock = NSLock()
        var paths: [(Int, String)] = []
        for (index, promise) in promises.enumerated() {
            group.enter()
            promise.receivePromisedFiles(atDestination: directory, options: [:], operationQueue: queue) { url, error in
                if error == nil {
                    lock.lock()
                    paths.append((index, url.path))
                    lock.unlock()
                }
                group.leave()
            }
        }
        group.notify(queue: .main) { [weak self] in
            let text = Self.pathsText(paths.sorted { $0.0 < $1.0 }.map(\.1))
            guard let self, !text.isEmpty else { return }
            try? self.session.paste(text)
        }
    }

    static func canDrop(_ pasteboard: NSPasteboard) -> Bool {
        pasteboard.availableType(from: droppableTypes) != nil
    }

    /// What a drop types: file paths, the path an image was saved to, a
    /// link, or text. Nil when the pasteboard holds none of these.
    static func dropText(from pasteboard: NSPasteboard, savingImagesIn directory: URL) -> String? {
        let files = pasteboard.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL] ?? []
        if !files.isEmpty { return pathsText(files.map(\.path)) }

        if let image = imageData(from: pasteboard) {
            let name = "Dropped Image \(Self.timestamp()).png"
            try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            let url = directory.appendingPathComponent(name)
            guard (try? image.write(to: url)) != nil else { return nil }
            return pathsText([url.path])
        }

        let urls = pasteboard.readObjects(forClasses: [NSURL.self]) as? [URL] ?? []
        if !urls.isEmpty { return urls.map(\.absoluteString).joined(separator: " ") }
        return pasteboard.string(forType: .string)
    }

    static func pathsText(_ paths: [String]) -> String {
        paths.isEmpty ? "" : paths.map(shellQuoted).joined(separator: " ") + " "
    }

    /// The pasteboard's image as PNG data, converting TIFF.
    private static func imageData(from pasteboard: NSPasteboard) -> Data? {
        if let png = pasteboard.data(forType: .png) { return png }
        guard let tiff = pasteboard.data(forType: .tiff), let bitmap = NSBitmapImageRep(data: tiff) else { return nil }
        return bitmap.representation(using: .png, properties: [:])
    }

    private static func timestamp() -> String {
        let formatter = DateFormatter()
        formatter.dateFormat = "yyyy-MM-dd 'at' HH.mm.ss"
        return formatter.string(from: Date())
    }
}
