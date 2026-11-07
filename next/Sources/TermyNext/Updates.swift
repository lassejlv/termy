import AppKit
import Sparkle

/// Automatic updates through Sparkle. Release builds carry the update feed
/// and its public key in Info.plist (scripts/bundle.sh); development builds,
/// `swift run` and local bundles have no feed, so they never check or update.
///
/// Sparkle checks every hour, downloads a new version in the background and
/// installs it on quit; "Restart to Update" installs it now. Panes run in the
/// session host, so they keep running across the relaunch.
///
/// Releases come out often, and Sparkle sticks with the version it found
/// first: a check started later shows that one again rather than reading the
/// feed. So the first launch of a new version checks right away, and a check
/// the user starts reads the feed again.
@MainActor
final class Updates: NSObject {
    static let shared = Updates()

    private var controller: SPUStandardUpdaterController?
    private var canCheckObservation: NSKeyValueObservation?
    /// Installs the downloaded update and relaunches, once Sparkle has one ready.
    private var installNow: (() -> Void)?

    /// The downloaded version that installs on quit, if any.
    private(set) var readyVersion: String?

    /// This build can update itself.
    var isAvailable: Bool { controller != nil }

    /// Starts Sparkle when this build has an update feed.
    func start() {
        let info = Bundle.main.infoDictionary ?? [:]
        guard let feed = info["SUFeedURL"] as? String, !feed.isEmpty,
              let key = info["SUPublicEDKey"] as? String, !key.isEmpty
        else { return }
        makeController()
        checkAfterUpdating()
    }

    private func makeController() {
        canCheckObservation = nil
        let controller = SPUStandardUpdaterController(startingUpdater: true, updaterDelegate: self, userDriverDelegate: nil)
        self.controller = controller
        // The menu item and Settings follow whether a check can start now.
        canCheckObservation = controller.updater.observe(\.canCheckForUpdates) { _, _ in
            DispatchQueue.main.async { NotificationCenter.default.post(name: Updates.didChange, object: nil) }
        }
    }

    /// The first launch of a new version looks for a newer one right away,
    /// instead of an hour later: the update that was installed may have been
    /// downloaded before the latest release came out.
    private func checkAfterUpdating() {
        let defaults = UserDefaults.standard
        let key = "TermyNextLastLaunchedBuild"
        let build = Bundle.main.infoDictionary?["CFBundleVersion"] as? String
        let previous = defaults.string(forKey: key)
        defaults.set(build, forKey: key)
        guard previous != nil, previous != build, checksAutomatically else { return }
        DispatchQueue.main.asyncAfter(deadline: .now() + 5) { [weak self] in
            guard let updater = self?.controller?.updater, updater.canCheckForUpdates else { return }
            updater.checkForUpdatesInBackground()
        }
    }

    static let didChange = Notification.Name("TermyNextUpdatesDidChange")

    /// A check can start now, or a downloaded update can be installed.
    var canCheck: Bool { installNow != nil || (controller?.updater.canCheckForUpdates ?? false) }

    /// What the menu item says: install the waiting update, or look for one.
    var menuTitle: String {
        readyVersion.map { "Restart to Update to \($0)" } ?? Command.checkForUpdates.title
    }

    /// Installs a downloaded update and relaunches, or checks now with
    /// Sparkle's progress and result window.
    func checkForUpdates() {
        if let installNow { return installNow() }
        // An updater that already found a version this run shows that one
        // again; a fresh one reads the feed for the latest.
        if controller?.updater.canCheckForUpdates == true { makeController() }
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

extension Updates: SPUUpdaterDelegate {
    /// A background download is ready. It installs when the app quits; the
    /// menu and Settings offer to install it now.
    func updater(_ updater: SPUUpdater, willInstallUpdateOnQuit item: SUAppcastItem, immediateInstallationBlock immediateInstallHandler: @escaping () -> Void) -> Bool {
        readyVersion = item.displayVersionString
        installNow = immediateInstallHandler
        NotificationCenter.default.post(name: Self.didChange, object: nil)
        return true
    }
}
