import AppKit
import Termy
import UserNotifications

/// Desktop notifications from OSC 7501 program status: when a program in a
/// pane says it needs attention (blocked), finished (done) or failed (error)
/// while you aren't looking at that pane. Clicking one brings the pane to
/// the front. Each pane has at most one notification, replaced as its status
/// changes and removed once you look at the pane. The Dock icon counts the
/// panes that need attention.
@MainActor
final class PaneNotifier: NSObject, UNUserNotificationCenterDelegate {
    static let shared = PaneNotifier()

    private var center: UNUserNotificationCenter? {
        // Notification Center needs an app bundle; `swift run` has none.
        Bundle.main.bundleIdentifier == nil ? nil : UNUserNotificationCenter.current()
    }
    private var asked = false

    func start() {
        center?.delegate = self
    }

    /// A pane's program reported `state` instead of `previous`.
    func pane(_ pane: PaneView, changedFrom previous: PaneActivity, to state: PaneActivity) {
        updateBadge()
        let prefs = Preferences.shared
        guard prefs.notificationsEnabled, previous != state else { return }
        let wanted = switch state {
        case .blocked: prefs.notifyWhenBlocked
        case .done: prefs.notifyWhenDone
        case .error: prefs.notifyWhenFailed
        case .idle, .working: false
        }
        guard wanted, !isWatching(pane) else { return }
        post(for: pane, state: state)
    }

    /// The pane is on screen in the frontmost window of the active app, so
    /// a notification would only interrupt.
    private func isWatching(_ pane: PaneView) -> Bool {
        guard NSApp.isActive, let window = pane.window, window.isKeyWindow, window.isVisible else { return false }
        return !pane.isHiddenOrHasHiddenAncestor && pane.isFocused
    }

    private func post(for pane: PaneView, state: PaneActivity) {
        guard let center else { return }
        if !asked {
            asked = true
            center.requestAuthorization(options: [.alert, .sound, .badge]) { _, _ in }
        }
        let status = pane.status
        let content = UNMutableNotificationContent()
        let program = status?.app.map(AppIcons.displayName(for:)) ?? pane.foregroundProgram.map(AppIcons.displayName(for:))
        content.title = [program, status?.title].compactMap { $0 }.first ?? pane.label.name
        var context: [String] = []
        if let (session, tab) = location(of: pane) {
            context.append(session.name)
            if session.tabs.count > 1 { context.append(tab.title) }
        }
        if let directory = pane.label.detail { context.append(directory) }
        content.subtitle = context.joined(separator: " · ")
        content.body = status?.msg?.trimmingCharacters(in: .whitespacesAndNewlines).nonEmpty
            ?? pane.statusDetail
            ?? state.label
        if Preferences.shared.notificationSound { content.sound = .default }
        content.threadIdentifier = "pane-\(pane.id.uuidString)"
        content.userInfo = ["pane": pane.id.uuidString]
        if state == .blocked { content.interruptionLevel = .timeSensitive }
        if let attachment = iconAttachment(for: pane) { content.attachments = [attachment] }
        // One per pane: a newer status replaces the older notification.
        let request = UNNotificationRequest(identifier: "pane-\(pane.id.uuidString)", content: content, trigger: nil)
        center.add(request)
        if state == .blocked, !NSApp.isActive { NSApp.requestUserAttention(.informationalRequest) }
    }

    /// The pane's program icon (an agent's logo, say) as an attachment.
    private func iconAttachment(for pane: PaneView) -> UNNotificationAttachment? {
        let program = pane.status?.app ?? pane.foregroundProgram
        let dark = NSApp.effectiveAppearance.isDarkAqua
        guard let image = ProgramLogos.image(for: program, dark: dark).map(Self.rasterized)
                ?? AppIcons.icon(program: program, executable: pane.foregroundExecutable),
              let tiff = image.tiffRepresentation, let png = NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:])
        else { return nil }
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("termy-next-icon-\(UUID().uuidString).png")
        guard (try? png.write(to: url)) != nil else { return nil }
        return try? UNNotificationAttachment(identifier: "icon", url: url)
    }

    /// An SVG logo drawn into a 256-point bitmap, for a notification attachment.
    private static func rasterized(_ logo: NSImage) -> NSImage {
        NSImage(size: NSSize(width: 256, height: 256), flipped: false) { rect in
            ProgramLogos.draw(logo, in: rect.insetBy(dx: 24, dy: 24))
            return true
        }
    }

    /// Removes the pane's notification once its result has been seen.
    func paneWasSeen(_ pane: PaneView) {
        center?.removeDeliveredNotifications(withIdentifiers: ["pane-\(pane.id.uuidString)"])
        updateBadge()
    }

    /// The Dock icon shows how many panes need attention.
    func updateBadge() {
        let waiting = Workspace.shared.sessions.flatMap(\.tabs).flatMap(\.allPanes).filter { $0.activity == .blocked }.count
        NSApp.dockTile.badgeLabel = Preferences.shared.notificationsEnabled && waiting > 0 ? "\(waiting)" : nil
    }

    private func location(of pane: PaneView) -> (TerminalSessionGroup, TerminalTab)? {
        for session in Workspace.shared.sessions {
            if let tab = session.tabs.first(where: { $0.panes[pane.id] != nil }) { return (session, tab) }
        }
        return nil
    }

    // MARK: UNUserNotificationCenterDelegate

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification,
                                            withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void) {
        // Shown even while Termy Next is in front: a pane you aren't looking at wants you.
        completionHandler([.banner, .sound, .list])
    }

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
                                            withCompletionHandler completionHandler: @escaping () -> Void) {
        let id = (response.notification.request.content.userInfo["pane"] as? String).flatMap(UUID.init(uuidString:))
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                if let id { AppDelegate.shared.revealPane(id) }
                completionHandler()
            }
        }
    }
}

private extension String {
    var nonEmpty: String? { isEmpty ? nil : self }
}
