#if os(macOS)
import AppKit
import Termy

extension TerminalView: NSMenuItemValidation {
    /// The right-click menu. A program that tracks the mouse gets right
    /// clicks instead, unless Shift or Option is held, as for selecting.
    public override func menu(for event: NSEvent) -> NSMenu? {
        window?.makeFirstResponder(self)
        let menu = NSMenu()
        switch linkAction(at: gridPosition(for: event)) {
        case .open(let url) where url.isFileURL:
            menu.addItem(menuItem("Open", #selector(openMenuLink(_:)), for: url))
            menu.addItem(menuItem("Show in Finder", #selector(revealMenuLink(_:)), for: url))
            menu.addItem(menuItem("Copy Path", #selector(copyMenuLink(_:)), for: url))
            menu.addItem(.separator())
        case .open(let url):
            menu.addItem(menuItem("Open Link", #selector(openMenuLink(_:)), for: url))
            menu.addItem(menuItem("Copy Link", #selector(copyMenuLink(_:)), for: url))
            menu.addItem(.separator())
        case .reveal(let url):
            menu.addItem(menuItem("Show in Finder", #selector(openMenuLink(_:)), for: url))
            menu.addItem(menuItem("Copy Path", #selector(copyMenuLink(_:)), for: url))
            menu.addItem(.separator())
        case nil:
            break
        }
        menu.addItem(menuItem("Copy", #selector(copy(_:)), key: "c"))
        menu.addItem(menuItem("Paste", #selector(paste(_:)), key: "v"))
        menu.addItem(menuItem("Select All", #selector(selectAll(_:)), key: "a"))
        delegate?.terminalView(self, willShowMenu: menu)
        return menu
    }

    public func validateMenuItem(_ item: NSMenuItem) -> Bool {
        switch item.action {
        case #selector(copy(_:)): selection != nil
        case #selector(paste(_:)): Self.pasteText(from: pasteboard).map { !$0.isEmpty } ?? false
        case #selector(selectAll(_:)): !session.frame.isEmpty
        default: true
        }
    }

    private func menuItem(_ title: String, _ action: Selector, key: String = "", for url: URL? = nil) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: action, keyEquivalent: key)
        item.target = self
        item.representedObject = url
        return item
    }

    @objc private func openMenuLink(_ sender: NSMenuItem) {
        guard let url = sender.representedObject as? URL else { return }
        if url.isFileURL, Self.isLaunchable(url) {
            NSWorkspace.shared.activateFileViewerSelecting([url])
        } else {
            NSWorkspace.shared.open(url)
        }
    }

    @objc private func revealMenuLink(_ sender: NSMenuItem) {
        guard let url = sender.representedObject as? URL else { return }
        NSWorkspace.shared.activateFileViewerSelecting([url])
    }

    @objc private func copyMenuLink(_ sender: NSMenuItem) {
        guard let url = sender.representedObject as? URL else { return }
        pasteboard.clearContents()
        pasteboard.setString(url.isFileURL ? url.path : url.absoluteString, forType: .string)
    }
}
#endif
