import Foundation
import ServiceManagement

/// 登录时自动启动（SMAppService.mainApp）。
/// swift run 环境下不是 .app 包，因此为 no-op（注册无意义且会报错）。
enum LaunchAtLogin {
    /// 仅在以 .app 包运行时生效。（swift run 下 bundleIdentifier 也可能非 nil，
    /// 所以用可靠的守卫：检查包 URL 的扩展名是否为 .app。）
    static var isAvailable: Bool {
        Bundle.main.bundleURL.pathExtension == "app"
    }

    static var isEnabled: Bool {
        guard isAvailable else { return false }
        return SMAppService.mainApp.status == .enabled
    }

    static func set(_ enabled: Bool) {
        guard isAvailable else {
            NSLog("[AIGlass] LaunchAtLogin：当前不是 .app，忽略登录启动设置（swift run 环境）。请先生成 app 包。")
            return
        }
        do {
            if enabled {
                if SMAppService.mainApp.status != .enabled {
                    try SMAppService.mainApp.register()
                }
            } else {
                if SMAppService.mainApp.status == .enabled {
                    try SMAppService.mainApp.unregister()
                }
            }
        } catch {
            NSLog("[AIGlass] LaunchAtLogin 切换失败：\(error)")
        }
    }
}
