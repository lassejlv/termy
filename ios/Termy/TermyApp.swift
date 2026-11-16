import SwiftUI
import TermyRemoteKit
import UIKit

@main
struct TermyApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @State private var model = AppModel.shared
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            HostsView()
                .environment(model)
                .preferredColorScheme(.dark)
                .onOpenURL { model.open($0) }
                .onContinueUserActivity(RemoteHandoff.activityType) { model.continue($0) }
        }
        .onChange(of: scenePhase) { _, phase in
            switch phase {
            case .active:
                model.resume()
                Task { await CommandNotifier.shared.refreshPermission() }
            case .background: model.background()
            default: break
            }
        }
    }
}

/// Home Screen quick actions arrive through the scene delegate, which SwiftUI leaves to UIKit.
final class AppDelegate: NSObject, UIApplicationDelegate {
    func application(
        _ application: UIApplication, didFinishLaunchingWithOptions options: [UIApplication.LaunchOptionsKey: Any]? = nil
    ) -> Bool {
        // Before launch ends, so tapping a notification opens its pane even when Termy wasn't running.
        _ = CommandNotifier.shared
        // Tokens can change; asking again each launch keeps the relays' copy current.
        if CommandNotifier.shared.enabled { application.registerForRemoteNotifications() }
        return true
    }

    func application(_ application: UIApplication, didRegisterForRemoteNotificationsWithDeviceToken token: Data) {
        AppModel.shared.pushRegistered(token)
    }

    func application(_ application: UIApplication, didFailToRegisterForRemoteNotificationsWithError error: Error) {
        AppModel.shared.pushFailed(error)
    }

    func application(
        _ application: UIApplication, configurationForConnecting session: UISceneSession,
        options: UIScene.ConnectionOptions
    ) -> UISceneConfiguration {
        let configuration = UISceneConfiguration(name: nil, sessionRole: session.role)
        configuration.delegateClass = QuickActionSceneDelegate.self
        return configuration
    }
}

final class QuickActionSceneDelegate: NSObject, UIWindowSceneDelegate {
    func scene(
        _ scene: UIScene, willConnectTo session: UISceneSession, options connectionOptions: UIScene.ConnectionOptions
    ) {
        if let item = connectionOptions.shortcutItem { Self.perform(item) }
    }

    func windowScene(
        _ windowScene: UIWindowScene, performActionFor shortcutItem: UIApplicationShortcutItem,
        completionHandler: @escaping (Bool) -> Void
    ) {
        completionHandler(Self.perform(shortcutItem))
    }

    @discardableResult
    @MainActor private static func perform(_ item: UIApplicationShortcutItem) -> Bool {
        guard let raw = item.userInfo?["url"] as? String, let url = URL(string: raw),
            let destination = TermyLink.parse(url)
        else { return false }
        AppModel.shared.open(destination)
        return true
    }
}
