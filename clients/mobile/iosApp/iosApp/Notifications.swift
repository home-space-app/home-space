import Foundation
import UIKit
import UserNotifications

/// Takes notification taps, which can arrive before the first scene does on a cold start.
final class AppDelegate: NSObject, UIApplicationDelegate, UNUserNotificationCenterDelegate {
    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
    ) -> Bool {
        let center = UNUserNotificationCenter.current()
        center.delegate = self
        let open = UNNotificationAction(
            identifier: IncomingNotices.openAction, title: "Open", options: [.foreground])
        center.setNotificationCategories([
            UNNotificationCategory(
                identifier: IncomingNotices.linkCategory, actions: [open], intentIdentifiers: [])
        ])
        return true
    }

    /// Nothing pops up over the list it would announce.
    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification
    ) async -> UNNotificationPresentationOptions {
        let quiet = await MainActor.run { AppRouter.shared.transfersVisible }
        return quiet ? [] : [.banner, .list, .sound]
    }

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse
    ) async {
        let transfer = response.notification.request.content.userInfo[IncomingNotices.transferKey] as? String
        let open = response.actionIdentifier == IncomingNotices.openAction
        await MainActor.run {
            AppRouter.shared.tab = .transfers
            if open, let transfer {
                AppRouter.shared.openTransfer = transfer
            }
        }
    }
}

/// Notifications about transfers sent to this phone, each once. There is no push: they come from
/// the lists the app loads while open and in background refresh.
enum IncomingNotices {
    static let linkCategory = "INCOMING_LINK"
    static let openAction = "OPEN"
    static let transferKey = "transfer"
    private static let notifiedKey = "transfers.notified"
    private static let thread = "transfers"

    private static var defaults: UserDefaults { SessionStore.defaults }

    /// Notifies the transfers waiting for this phone that haven't been, unless `quiet`; either
    /// way they count as seen. Withdraws notifications of those no longer waiting.
    @MainActor
    static func update(_ received: [TransferRow], quiet: Bool) async {
        let waiting = received.filter { $0.thisDevice && $0.status == .ready }
        let notified = Set(defaults.stringArray(forKey: notifiedKey) ?? [])
        let ids = Set(waiting.map(\.id))
        let gone = notified.subtracting(ids)
        let center = UNUserNotificationCenter.current()
        if !gone.isEmpty {
            center.removeDeliveredNotifications(withIdentifiers: gone.map(identifier))
        }
        defaults.set(Array(ids), forKey: notifiedKey)
        let fresh = waiting.filter { !notified.contains($0.id) }
        guard !quiet, !fresh.isEmpty else { return }
        let settings = await center.notificationSettings()
        guard settings.authorizationStatus == .authorized || settings.authorizationStatus == .provisional
        else { return }
        for row in fresh {
            let content = UNMutableNotificationContent()
            content.title = row.noticeTitle
            content.body = row.noticeDetail
            content.sound = .default
            content.threadIdentifier = thread
            content.userInfo = [transferKey: row.id]
            if row.canOpen {
                content.categoryIdentifier = linkCategory
            }
            try? await center.add(
                UNNotificationRequest(identifier: identifier(row.id), content: content, trigger: nil))
        }
    }

    /// Asks for permission the first time, when a transfer makes it clear why.
    static func askIfNeeded() async {
        let center = UNUserNotificationCenter.current()
        guard await center.notificationSettings().authorizationStatus == .notDetermined else { return }
        _ = try? await center.requestAuthorization(options: [.alert, .sound, .badge])
    }

    /// Forgets everything notified, for sign-out.
    static func forgetAll() {
        let ids = defaults.stringArray(forKey: notifiedKey) ?? []
        UNUserNotificationCenter.current().removeDeliveredNotifications(withIdentifiers: ids.map(identifier))
        defaults.removeObject(forKey: notifiedKey)
    }

    private static func identifier(_ transfer: String) -> String {
        "transfer.\(transfer)"
    }
}

/// Tells the user a send from this phone stopped while the app was away.
enum SendNotices {
    static func stopped(recipient: String) {
        guard UIApplication.shared.applicationState != .active else { return }
        let content = UNMutableNotificationContent()
        content.title = "Sending to \(recipient) stopped"
        content.body = "Open Home Space to send it again."
        content.threadIdentifier = "sends"
        UNUserNotificationCenter.current().add(
            UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil))
    }
}
