import Foundation
import ServiceManagement

/// 로그인 시 자동 시작 (SMAppService.mainApp).
/// swift run 환경에서는 .app 번들이 아니므로 no-op (등록 시도가 무의미/오류).
enum LaunchAtLogin {
    /// .app 번들로 실행 중일 때만 동작. (swift run에서도 bundleIdentifier가 nil이 아닐 수 있어
    /// 확실한 가드로 번들 URL의 확장자가 .app인지 확인한다.)
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
