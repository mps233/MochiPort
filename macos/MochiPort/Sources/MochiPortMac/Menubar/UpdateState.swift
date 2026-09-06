import Foundation
import Observation

/// 发现新版本的状态——仪表盘头部的更新徽标订阅它。
@MainActor
@Observable
final class UpdateState {
    /// nil 表示已是最新（隐藏徽标）。
    var available: ReleaseChecker.Release?
}
