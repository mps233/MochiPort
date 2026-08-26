import Foundation
import UserNotifications

/// 通知中心封装。没有 app bundle 时不调用系统通知（保护 swift run 场景）。
@MainActor
final class Notifier {
    /// 仅在 app bundle 中运行时可用。
    static var isAvailable: Bool {
        Bundle.main.bundleURL.pathExtension == "app"
    }

    private var didRequestAuthorization = false

    func notify(title: String, subtitle: String) {
        guard Notifier.isAvailable else { return }
        requestAuthorizationIfNeeded()

        let content = UNMutableNotificationContent()
        content.title = title
        if !subtitle.isEmpty { content.subtitle = subtitle }
        content.sound = .default

        let request = UNNotificationRequest(
            identifier: UUID().uuidString, content: content, trigger: nil)
        UNUserNotificationCenter.current().add(request)
    }

    private func requestAuthorizationIfNeeded() {
        guard !didRequestAuthorization else { return }
        didRequestAuthorization = true
        UNUserNotificationCenter.current().requestAuthorization(
            options: [.alert, .sound]) { granted, error in
            if let error { NSLog("[AIGlass] 请求通知权限失败：\(error)") }
            _ = granted
        }
    }
}
