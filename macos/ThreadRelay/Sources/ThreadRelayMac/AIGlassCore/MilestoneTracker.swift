import Foundation

/// 오늘 누적 토큰이 명예 임계(100M, 250M, …)를 넘을 때 한 번씩만 보고한다.
///
/// 상태(이미 보고한 최고 임계 + 그 기준 날짜)를 내부 보유한다. 날짜가 바뀌면 리셋.
/// 한 번에 여러 임계를 건너뛰면 그 중 **최고 임계 하나만** 반환한다.
@MainActor
public final class MilestoneTracker {
    /// 명예 임계 (오름차순). 토큰 누적이 이 값 이상이면 마일스톤.
    public static let thresholds: [Int] = [
        100_000_000, 250_000_000, 500_000_000,
        1_000_000_000, 2_000_000_000, 5_000_000_000,
    ]

    // 이미 보고한 최고 임계 (없으면 0).
    private var reported: Int = 0
    // reported를 기준한 day 문자열 (UTC). 날짜 바뀌면 리셋.
    private var reportedDay: String?

    private static let dayFormatter: DateFormatter = {
        let f = DateFormatter()
        f.calendar = Calendar(identifier: .gregorian)
        f.timeZone = TimeZone(identifier: "UTC")!
        f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = "yyyy-MM-dd"
        return f
    }()

    public init() {}

    /// 오늘 누적 토큰이 새 임계를 넘었으면 그 임계(여럿이면 최고)를, 아니면 nil.
    /// - day가 직전 보고 날짜와 다르면 내부 상태를 리셋한다.
    public func check(todayTokens: Int, day: Date, calendar: Calendar = .current) -> Int? {
        let dayStr = Self.dayFormatter.string(from: day)
        if reportedDay != dayStr {
            reported = 0
            reportedDay = dayStr
        }
        // todayTokens 이하의 임계 중 이미 보고한 것보다 큰 최고 임계.
        guard let crossed = Self.thresholds.last(where: { $0 <= todayTokens && $0 > reported }) else {
            return nil
        }
        reported = crossed
        return crossed
    }
}
