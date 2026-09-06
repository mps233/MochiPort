import Foundation

/// 按时段生成每日一次的使用量摘要。发放状态由调用方保存。
@MainActor
public final class BriefingEngine {
    /// 브리핑 시간대. 시각(로컬 캘린더 hour) 구간으로 판정.
    public enum Period: String, CaseIterable, Sendable {
        case morning, lunch, evening

        /// 이 Period의 hour 구간 (반열림 [lower, upper)).
        var hourRange: Range<Int> {
            switch self {
            case .morning: return 8..<11
            case .lunch: return 12..<14
            case .evening: return 18..<22
            }
        }

        /// 주어진 hour가 속한 Period (없으면 nil).
        static func forHour(_ hour: Int) -> Period? {
            allCases.first { $0.hourRange.contains(hour) }
        }
    }

    /// 브리핑 본문에 필요한 집계 데이터. 호출자가 store/statsStore에서 구성한다.
    public struct BriefingData {
        public var yesterdayTokens: Int
        public var yesterdayCost: Double
        public var yesterdayTopProject: String?
        public var todayTokens: Int
        public var todayCost: Double
        public var todayTopService: (service: ServiceID, share: Double)?
        // 주간 리포트용 (월요일 morning 특별판). 기본 nil이면 일반 morning.
        public var lastWeekTokens: Int?
        public var lastWeekCost: Double?
        public var prevWeekTokens: Int?
        public var lastWeekTopProject: String?
        /// 연속 사용일수 (오늘 포함, 끊김 없는 토큰>0). morning 부제에 N≥2일 때 " · {N}일 연속 🔥".
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

    /// Period별 마지막 발화 시각. 외부 주입/저장(UserDefaults 등)은 호출자 책임.
    public var lastFired: [Period: Date] = [:]
    /// Period 구간 판정에 쓰는 캘린더. 테스트는 UTC 주입.
    public var calendar: Calendar
    /// REAL Mode — 켜면 일반 브리핑 제목을 의인화 멘트로 교체. 주간 리포트는 정보 유지. 기본 off.
    public var realMode: Bool = false
    /// 이벤트별 사용자 커스텀 메시지 (kind.customKey → config). 호출자가 주입. 기본 없음.
    public var customMessages: [String: CustomMessageConfig] = [:]

    public init(calendar: Calendar = .current) {
        self.calendar = calendar
    }

    /// 当前时间段今天尚未发放时生成通知事件并更新 lastFired。
    /// - 시간대 밖, 데이터 0(tokens/cost), 오늘 이미 발화, lunch 외삽 불가(경과<4h) 시 nil.
    public func evaluate(now: Date, data: BriefingData) -> HUDEvent? {
        let hour = calendar.component(.hour, from: now)
        guard let period = Period.forHour(hour) else { return nil }

        // 오늘 이미 발화했는지: lastFired가 같은 날이면 스킵.
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
            // 월요일(weekday == 2) morning이고 지난주 데이터가 있으면 주간 리포트 특별판.
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
            // 자정~now 경과 비율로 외삽. 경과 < 4h면 발화 안 함.
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

    /// 월요일 morning 주간 리포트 특별판. kind는 .briefing(.morning) 유지(하루 1회 발화 공유).
    private func makeWeeklyReport(now: Date, data: BriefingData, lastWeekTokens: Int) -> HUDEvent {
        var subtitle = "上周 \(Self.formatTokens(lastWeekTokens))"
        if let cost = data.lastWeekCost {
            subtitle += " (~\(Self.formatCost(cost)))"
        }
        if let project = data.lastWeekTopProject {
            subtitle += " · 主要项目 \(project)"
        }
        // 전주 대비 % 는 prevWeekTokens > 0 일 때만.
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
