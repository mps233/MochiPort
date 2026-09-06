import Foundation

/// 今日累计 Token 越过荣誉阈值（100M、250M……）时只上报一次。
///
/// 内部持有状态（已上报的最高阈值及其基准日期）。日期变更时重置。
/// 一次跨过多个阈值时只返回其中**最高的一个**。
@MainActor
public final class MilestoneTracker {
    /// 荣誉阈值（升序）。累计 Token 达到该值即为里程碑。
    public static let thresholds: [Int] = [
        100_000_000, 250_000_000, 500_000_000,
        1_000_000_000, 2_000_000_000, 5_000_000_000,
    ]

    // 已上报的最高阈值（没有则为 0）。
    private var reported: Int = 0
    // 以 reported 为基准的 day 字符串（UTC）。日期变更时重置。
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

    /// 今日累计 Token 越过新阈值时返回该阈值（多个时取最高），否则 nil。
    /// - day 与上次上报日期不同时重置内部状态。
    public func check(todayTokens: Int, day: Date, calendar: Calendar = .current) -> Int? {
        let dayStr = Self.dayFormatter.string(from: day)
        if reportedDay != dayStr {
            reported = 0
            reportedDay = dayStr
        }
        // 阈值中 ≤ todayTokens 且大于已上报值的最高阈值。
        guard let crossed = Self.thresholds.last(where: { $0 <= todayTokens && $0 > reported }) else {
            return nil
        }
        reported = crossed
        return crossed
    }
}
