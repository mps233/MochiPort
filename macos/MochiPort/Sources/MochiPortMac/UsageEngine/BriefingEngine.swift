import Foundation

/// 按时段生成每日一次的使用量摘要。发放状态由调用方保存。
@MainActor
public final class BriefingEngine {
    /// 简报时段。按小时（本地日历 hour）区间判断。
    public enum Period: String, CaseIterable, Sendable {
        case morning, lunch, evening

        /// 该 Period 的小时区间（半开 [lower, upper)）。
        var hourRange: Range<Int> {
            switch self {
            case .morning: return 8..<11
            case .lunch: return 12..<14
            case .evening: return 18..<22
            }
        }

        /// 给定 hour 所属的 Period（无则 nil）。
        static func forHour(_ hour: Int) -> Period? {
            allCases.first { $0.hourRange.contains(hour) }
        }
    }

    /// 简报正文所需的聚合数据。由调用方从 store/statsStore 组装。
    public struct BriefingData {
        public var yesterdayTokens: Int
        public var yesterdayCost: Double
        public var yesterdayTopProject: String?
        public var todayTokens: Int
        public var todayCost: Double
        public var todayTopService: (service: ServiceID, share: Double)?
        // 周报用（周一 morning 特别版）。默认 nil 时为普通 morning。
        public var lastWeekTokens: Int?
        public var lastWeekCost: Double?
        public var prevWeekTokens: Int?
        public var lastWeekTopProject: String?
        /// 连续使用天数（含今天，Token>0 无中断）。morning 副标题 N≥2 时追加 " · {N}天连续 🔥"。
        public var streakDays: Int
        public init(yesterdayTokens: Int = 0, yesterdayCost: Double = 0,
                    yesterdayTopProject: String? = nil,
                    todayTokens: Int = 0, todayCost: Double = 0,
                    todayTopService: (service: ServiceID, share: Double)? = nil,
                    lastWeekTokens: Int? = nil, lastWeekCost: Double? = nil,
                    prevWeekTokens: Int? = nil, lastWeekTopProject: String? = nil,
                    streakDays: Int = 0) {
            self.yesterdayTokens = yesterdayTokens
            self.yesterdayCost = yesterdayCost
            self.yesterdayTopProject = yesterdayTopProject
            self.todayTokens = todayTokens
            self.todayCost = todayCost
            self.todayTopService = todayTopService
            self.lastWeekTokens = lastWeekTokens
            self.lastWeekCost = lastWeekCost
            self.prevWeekTokens = prevWeekTokens
            self.lastWeekTopProject = lastWeekTopProject
            self.streakDays = streakDays
        }
    }

    /// 各 Period 上次触发时间。外部注入/持久化（UserDefaults 等）由调用方负责。
    public var lastFired: [Period: Date] = [:]
    /// 判断 Period 区间用的日历。测试注入 UTC。
    public var calendar: Calendar
    /// REAL 模式——开启后把普通简报标题替换为拟人化文案。周报保持信息量。默认关。
    public var realMode: Bool = false
    /// 每种事件的自定义消息（kind.customKey → config）。由调用方注入。默认无。
    public var customMessages: [String: CustomMessageConfig] = [:]

    public init(calendar: Calendar = .current) {
        self.calendar = calendar
    }

    /// 当前时间段今天尚未发放时生成通知事件并更新 lastFired。
    /// - 不在时段内、数据为 0（tokens/cost）、今天已触发、lunch 无法外推（不足 4h）时返回 nil。
    public func evaluate(now: Date, data: BriefingData) -> HUDEvent? {
        let hour = calendar.component(.hour, from: now)
        guard let period = Period.forHour(hour) else { return nil }

        // 是否今天已触发：lastFired 是同一天就跳过。
        if let last = lastFired[period], calendar.isDate(last, inSameDayAs: now) {
            return nil
        }

        guard let event = makeEvent(period: period, now: now, data: data) else { return nil }
        lastFired[period] = now
        return event
    }

    private func makeEvent(period: Period, now: Date, data: BriefingData) -> HUDEvent? {
        switch period {
        case .morning:
            // 周一（weekday == 2）的 morning 且有上周数据时，生成周报特别版。
            if calendar.component(.weekday, from: now) == 2,
               let lastWeekTokens = data.lastWeekTokens, lastWeekTokens > 0 {
                return makeWeeklyReport(now: now, data: data, lastWeekTokens: lastWeekTokens)
            }
            guard data.yesterdayTokens > 0 || data.yesterdayCost > 0 else { return nil }
            var subtitle = "昨日：\(Self.formatTokens(data.yesterdayTokens)) tokens · 约 \(Self.formatCost(data.yesterdayCost))"
            if let project = data.yesterdayTopProject {
                subtitle += " · 主要项目 \(project)"
            }
            if data.streakDays >= 2 {
                subtitle += " · 连续使用 \(data.streakDays) 天 🔥"
            }
            return HUDEvent(kind: .briefing(.morning),
                            title: RealModeMessages.resolve(kind: .briefing(.morning), defaultTitle: "昨日使用摘要",
                                                            realMode: realMode, custom: customMessages[HUDEvent.Kind.briefing(.morning).customKey],
                                                            context: MessageContext(tokens: data.yesterdayTokens)),
                            subtitle: subtitle, percent: nil)

        case .lunch:
            guard data.todayTokens > 0 || data.todayCost > 0 else { return nil }
        // 按午夜到当前时间的比例外推。不足 4h 不触发。
            let midnight = calendar.startOfDay(for: now)
            let elapsed = now.timeIntervalSince(midnight)
            guard elapsed >= 4 * 3600 else { return nil }
            let dayFraction = elapsed / (24 * 3600)
            let projectedTokens = Int(Double(data.todayTokens) / dayFraction)
            let projectedCost = data.todayCost / dayFraction
            let subtitle = "照这个进度，午夜前约 \(Self.formatTokens(projectedTokens))（约 \(Self.formatCost(projectedCost))）"
            return HUDEvent(kind: .briefing(.lunch),
                            title: RealModeMessages.resolve(kind: .briefing(.lunch), defaultTitle: "今日进度",
                                                            realMode: realMode, custom: customMessages[HUDEvent.Kind.briefing(.lunch).customKey],
                                                            context: MessageContext(tokens: data.todayTokens)),
                            subtitle: subtitle, percent: nil)

        case .evening:
            guard data.todayTokens > 0 || data.todayCost > 0 else { return nil }
            var subtitle = "今日：\(Self.formatTokens(data.todayTokens)) tokens · 约 \(Self.formatCost(data.todayCost))"
            if let top = data.todayTopService {
                subtitle += " · \(top.service.displayName) 占比 \(Int((top.share * 100).rounded()))%"
            }
            return HUDEvent(kind: .briefing(.evening),
                            title: RealModeMessages.resolve(kind: .briefing(.evening), defaultTitle: "今日使用总结",
                                                            realMode: realMode, custom: customMessages[HUDEvent.Kind.briefing(.evening).customKey],
                                                            context: MessageContext(tokens: data.todayTokens)),
                            subtitle: subtitle, percent: nil)
        }
    }

    /// 周一 morning 的周报特别版。kind 保持 .briefing(.morning)（共享一天一次的触发）。
    private func makeWeeklyReport(now: Date, data: BriefingData, lastWeekTokens: Int) -> HUDEvent {
        var subtitle = "上周 \(Self.formatTokens(lastWeekTokens))"
        if let cost = data.lastWeekCost {
            subtitle += " (~\(Self.formatCost(cost)))"
        }
        if let project = data.lastWeekTopProject {
            subtitle += " · 主要项目 \(project)"
        }
        // 对比上周的 % 只在 prevWeekTokens > 0 时。
        if let prev = data.prevWeekTokens, prev > 0 {
            let delta = Double(lastWeekTokens - prev) / Double(prev) * 100
            let sign = delta >= 0 ? "+" : ""
            subtitle += " · 较前一周 \(sign)\(Int(delta.rounded()))%"
        }
        return HUDEvent(kind: .briefing(.morning), title: "每周报告 📊",
                        subtitle: subtitle, percent: nil)
    }

    static func formatTokens(_ n: Int) -> String {
        switch n {
        case 1_000_000...: return String(format: "%.1fM", Double(n) / 1_000_000)
        case 1_000...: return String(format: "%.1fK", Double(n) / 1_000)
        default: return "\(n)"
        }
    }

    static func formatCost(_ cost: Double) -> String {
        String(format: "$%.2f", cost)
    }
}
