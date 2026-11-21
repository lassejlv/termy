import AppKit
import TermyRemoteKit

/// Handoff with the Termy iPhone app. The focused session-host pane is
/// offered to the phone, which opens it; a pane the phone shows opens here.
/// Both apps name the computer by the host ID it paired under, so Handoff needs
/// Mobile access on, and both apps signed by the same team.
@MainActor
enum PaneHandoff {
    private static var activity: NSUserActivity?

    static func paneFocused(_ pane: PaneView) {
        guard let paneID = pane.paneID, let host = MobileController.shared.deviceID else { return }
        let activity = Self.activity ?? NSUserActivity(activityType: RemoteHandoff.activityType)
        activity.title = pane.label.name
        activity.userInfo = RemoteHandoff.userInfo(host: host, pane: paneID)
        activity.isEligibleForHandoff = true
        Self.activity = activity
        activity.becomeCurrent()
    }

    /// Mobile access turned off: the phone can no longer reach this Mac.
    static func stop() {
        activity?.invalidate()
        activity = nil
    }

    /// Opens the pane a phone handed off, when it runs on this Mac.
    static func `continue`(_ userActivity: NSUserActivity, reveal: (UUID) -> Void) -> Bool {
        guard userActivity.activityType == RemoteHandoff.activityType,
            let target = RemoteHandoff.parse(userActivity.userInfo),
            target.host == MobileController.shared.deviceID
        else { return false }
        for session in Workspace.shared.sessions {
            for tab in session.tabs {
                if let pane = tab.panes.values.first(where: { $0.paneID == target.pane }) {
                    reveal(pane.id)
                    return true
                }
            }
        }
        return false
    }
}
