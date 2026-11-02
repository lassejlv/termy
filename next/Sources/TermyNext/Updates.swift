import AppKit
import Sparkle

/// Automatic updates through Sparkle. Release builds carry the update feed
/// and its public key in Info.plist (scripts/release.sh); development builds,
/// `swift run` and local bundles have no feed, so they never check or update.
///
/// Sparkle checks in the background on its own schedule (every six hours),
/// shows its update window when a new version is out, and installs it on
/// quit or on "Install and Relaunch". Panes run in the session host, so they
/// keep running across the relaunch.
@MainActor
final class Updates: NSObject {
    static let shared = Updates()

    private var controller: SPUStandardUpdaterController?
    private var canCheckObservation: NSKeyValueObservation?

    /// This build can update itself.
    var isAvailable: Bool { controller != nil }

    /// Starts Sparkle when this build has an update feed.
    func start() {
        let info = Bundle.main.infoDictionary ?? [:]
        guard let feed = info["SUFeedURL"] as? String, !feed.isEmpty,
              let key = info["SUPublicEDKey"] as? String, !key.isEmpty
        else { return }
        let controller = SPUStandardUpdaterController(startingUpdater: true, updaterDelegate: nil, userDriverDelegate: nil)
        self.controller = controller
        // The menu item and Settings follow whether a check can start now.
        canCheckObservation = controller.updater.observe(\.canCheckForUpdates) { _, _ in
            DispatchQueue.main.async { NotificationCenter.default.post(name: Updates.didChange, object: nil) }
        }
    }

    static let didChange = Notification.Name("TermyNextUpdatesDidChange")

    /// A check can start now: this build updates itself and isn't already checking.
    var canCheck: Bool { controller?.updater.canCheckForUpdates ?? false }

    /// Checks now, with Sparkle's progress and result window.
    func checkForUpdates() {
        controller?.checkForUpdates(nil)
    }

    /// Checks for updates in the background on Sparkle's schedule.
    var checksAutomatically: Bool {
        get { controller?.updater.automaticallyChecksForUpdates ?? false }
        set {
            controller?.updater.automaticallyChecksForUpdates = newValue
            NotificationCenter.default.post(name: Self.didChange, object: nil)
        }
    }

    /// Downloads updates in the background and installs them on quit,
    /// without asking first.
    var downloadsAutomatically: Bool {
        get { controller?.updater.automaticallyDownloadsUpdates ?? false }
        set {
            controller?.updater.automaticallyDownloadsUpdates = newValue
            NotificationCenter.default.post(name: Self.didChange, object: nil)
        }
    }

    /// When Sparkle last looked for an update.
    var lastCheck: Date? { controller?.updater.lastUpdateCheckDate }

    /// This build's version, as the About window shows it.
    static var currentVersion: String {
        let info = Bundle.main.infoDictionary ?? [:]
        let version = info["CFBundleShortVersionString"] as? String ?? "development"
        let build = info["CFBundleVersion"] as? String
        return build.map { "\(version) (\($0))" } ?? version
    }
}
