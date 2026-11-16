import CryptoKit
import Foundation
import TermyRemoteKit
import UserNotifications

/// Opens notifications a Mac sealed for this phone (`PushSeal`) and shows
/// what they say. The relay only ever sent a generic alert; without the key,
/// or if anything fails, that generic alert is shown instead.
final class NotificationService: UNNotificationServiceExtension {
    override func didReceive(
        _ request: UNNotificationRequest, withContentHandler contentHandler: @escaping (UNNotificationContent) -> Void
    ) {
        contentHandler(Self.open(request.content))
    }

    static func open(_ original: UNNotificationContent) -> UNNotificationContent {
        guard let content = original.mutableCopy() as? UNMutableNotificationContent,
            let termy = original.userInfo["termy"] as? [String: Any],
            let host = (termy["h"] as? String).flatMap(UUID.init(uuidString:)),
            let sealed = (termy["s"] as? String).flatMap({ Data(base64Encoded: $0) }),
            let key = PushKeyStore.load()[host],
            let payload = try? PushSeal.open(sealed, key: SymmetricKey(data: key))
        else { return original }
        content.title = payload.title
        content.body = payload.body
        content.threadIdentifier = host.uuidString
        content.userInfo["url"] = TermyLink.open(host: host, pane: payload.pane).absoluteString
        content.userInfo["termy"] = nil
        return content
    }
}
