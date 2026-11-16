import Foundation
import Observation
import TermyRemoteKit
import UIKit
import UserNotifications

/// Notifies when a long command finishes in a pane you aren't looking at.
///
/// Off until turned on in Settings, which is when Termy asks for permission.
/// It works from the session lists the Mac sends while Termy is open; once
/// the relay can push, the same switch covers notifications in the background.
@MainActor @Observable
final class CommandNotifier: NSObject, UNUserNotificationCenterDelegate {
    static let shared = CommandNotifier()
    static let defaultsKey = "notifyCommands"
    /// Commands shorter than this end without a notification.
    nonisolated static let minimumDuration: TimeInterval = 10

    private(set) var enabled = UserDefaults.standard.bool(forKey: defaultsKey)
    /// Permission was refused in iOS Settings; the switch can't turn on.
    private(set) var denied = false

    override init() {
        super.init()
        UNUserNotificationCenter.current().delegate = self
    }

    /// Turns notifications on, asking for permission the first time; false when iOS refuses.
    @discardableResult
    func setEnabled(_ on: Bool) async -> Bool {
        guard on else {
            enabled = false
            UserDefaults.standard.set(false, forKey: Self.defaultsKey)
            await AppModel.shared.pushDisabled()
            return true
        }
        let center = UNUserNotificationCenter.current()
        let settings = await center.notificationSettings()
        var allowed = settings.authorizationStatus == .authorized || settings.authorizationStatus == .provisional
        if settings.authorizationStatus == .notDetermined {
            allowed = (try? await center.requestAuthorization(options: [.alert, .sound, .badge])) ?? false
        }
        denied = !allowed
        enabled = allowed
        UserDefaults.standard.set(allowed, forKey: Self.defaultsKey)
        // The token arrives in the app delegate, which hands it to the relays.
        if allowed { UIApplication.shared.registerForRemoteNotifications() }
        return allowed
    }

    /// Notices permission changed in iOS Settings while Termy was away.
    func refreshPermission() async {
        let status = await UNUserNotificationCenter.current().notificationSettings().authorizationStatus
        denied = status == .denied
        if enabled, status == .denied { await setEnabled(false) }
    }

    /// Commands that finished between `old` and `new` after running at least `minimumDuration`.
    nonisolated static func finished(old: [RemotePane], new: [RemotePane], now: Date = Date()) -> [FinishedCommand] {
        FinishedCommand.between(old, new, now: now, atLeast: minimumDuration)
    }

    /// Called with each new session list from a computer.
    func sessionsChanged(from old: RemoteSessionList, to new: RemoteSessionList, on host: RelayDevice, watching: Set<String>) {
        guard enabled else { return }
        let appActive = UIApplication.shared.applicationState == .active
        for done in Self.finished(old: old.panes, new: new.panes) {
            // The pane on screen already shows its own banner.
            if appActive, watching.contains(done.pane.id) { continue }
            post(done, host: host)
        }
    }

    private func post(_ done: FinishedCommand, host: RelayDevice) {
        let content = UNMutableNotificationContent()
        (content.title, content.body) = done.notification(on: host.name)
        content.sound = .default
        content.threadIdentifier = host.id.uuidString
        content.userInfo = ["url": TermyLink.open(host: host.id, pane: done.pane.id).absoluteString]
        UNUserNotificationCenter.current().add(
            UNNotificationRequest(identifier: "\(host.id)-\(done.pane.id)-\(done.state.finished)", content: content, trigger: nil))
    }

    // MARK: UNUserNotificationCenterDelegate

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter, willPresent notification: UNNotification
    ) async -> UNNotificationPresentationOptions {
        [.banner, .list, .sound]
    }

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse) async {
        guard let raw = response.notification.request.content.userInfo["url"] as? String, let url = URL(string: raw) else {
            return
        }
        await MainActor.run { AppModel.shared.open(url) }
    }
}
