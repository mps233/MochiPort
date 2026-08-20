import Foundation
import Observation

/// 새 버전 발견 상태 — 대시보드 헤더의 업데이트 배지가 구독한다.
@MainActor
@Observable
final class UpdateState {
    /// nil이면 최신 (배지 숨김).
    var available: AIGlassUpdateChecker.Release?
}
