import Foundation

public struct HUDEvent: Equatable {
    public enum Kind: Equatable {
        case limitThreshold(ServiceID, Int)  // 跨过 70 或 90
        case depletionRisk(ServiceID)        // 重置前即将耗尽
        case windowReset(ServiceID)
        case burnSpike
        case briefing(BriefingEngine.Period)
        case comeback                         // 活动空白 ≥3h 后重新开始
        case milestone                        // 今日累计 Token 越过荣誉阈值
        case record                           // 刷新单日 Token 纪录
        case update                           // 新版本发布（与 EventEngine 无关——仅共享类型）
    }
    public let kind: Kind
    public let title: String
    public let subtitle: String
    public let percent: Double?
    public init(kind: Kind, title: String, subtitle: String, percent: Double?) {
        self.kind = kind
        self.title = title
        self.subtitle = subtitle
        self.percent = percent
    }
}

/// 评估 UsageStore 快照并生成使用量通知事件，保存上次使用率和冷却状态。
@MainActor
public final class EventEngine {
    public var thresholds: [Int] = [70, 90]
    public var spikeMultiplier: Double = 3.0
    public var spikeCooldown: TimeInterval = 30 * 60
    public var resetDropFloor: Double = 10  // 降到该值以下即视为已重置
    public var resetDropFrom: Double = 30   // 仅当之前的值不低于该值时
    /// depletionRisk 的每服务冷却（5h 等小时窗口，默认 30 分钟——紧迫所以较短）。
    public var depletionCooldown: TimeInterval = 30 * 60
    /// 周窗口耗尽临近的冷却（默认 12 小时——按天预测，一天最多 2 次足够）。
    public var weeklyDepletionCooldown: TimeInterval = 12 * 3600
    /// REAL 模式——开启后事件标题替换为拟人化情感文案（副标题信息保留）。默认关。
    public var realMode: Bool = false
    /// 每种事件的自定义消息（kind.customKey → config）。由调用方注入。默认无。
    public var customMessages: [String: CustomMessageConfig] = [:]
    /// 周窗口耗尽临近的触发标准——仅当预计耗尽时间早于"距重置剩余时间 × 该比例"时触发。
    /// 用比例而非绝对天数，重置越临近只提示越紧迫的耗尽
    /// （例：0.6 → 重置 6 天时约 3.6 天内、重置 2 天时约 1.2 天内耗尽）。不适用于 5h 窗口。
    public var weeklyDepletionRatio: Double = 0.6
    /// 周窗口耗尽临近只在距重置不少于该时间时触发（当天/迫近时无意义）。默认 1 天。
    public var weeklyDepletionMinLead: TimeInterval = 24 * 3600

    private var lastPercent: [ServiceID: [LimitWindow.Kind: Double]] = [:]
    private var lastSpikeAt: Date = .distantPast
    private struct DepletionKey: Hashable { let service: ServiceID; let kind: LimitWindow.Kind }
    private var lastDepletionAt: [DepletionKey: Date] = [:]

    public init() {}

    /// - Parameters:
    ///   - burnRate：`UsageStore.tokensPerMinute(windowMinutes: 10)` 值（当前每分钟 Token 消耗率）
    ///   - baseline：`UsageStore.activeBaselineRate()` 值（平时活动时的基准消耗率）
    ///   - depletions：各服务的耗尽预测列表（每窗口 0~2 个）。只对 `willDepleteBeforeReset` 的触发 depletionRisk（(服务, kind) 各 30 分钟冷却）。默认 `[:]`。
    ///   - reportProvider：windowReset 触发时替换 subtitle 的会话摘要提供者。给非 nil 字符串就用它作为 subtitle。默认 nil。
    /// 优先级：threshold > depletionRisk > reset > spike。
    /// 备注：临界值附近的振荡（71→69→71 再次触发）是 MVP 的有意简化，未做防护。
    public func evaluate(limits: [ServiceID: [LimitWindow]],
                         burnRate: Double, baseline: Double, now: Date,
                         depletions: [ServiceID: [Depletion]] = [:],
                         reportProvider: ((ServiceID) -> String?)? = nil) -> [HUDEvent] {
        var thresholdEvents: [HUDEvent] = []
        var resetEvents: [HUDEvent] = []

        for (service, windows) in limits {
            for window in windows {
                // previous 默认 0：nil 时按 0 处理，首个观测值已达阈值即触发
                let previous = lastPercent[service]?[window.kind] ?? 0
                defer { lastPercent[service, default: [:]][window.kind] = window.usedPercent }

                for threshold in thresholds.sorted(by: >) {
                    if previous < Double(threshold), window.usedPercent >= Double(threshold) {
                        let kind = HUDEvent.Kind.limitThreshold(service, threshold)
                        let ctx = MessageContext(agent: service.displayName, usage: window.usedPercent,
                                                 reset: window.resetsAt.map { Self.countdown(to: $0, from: now) })
                        thresholdEvents.append(HUDEvent(
                            kind: kind,
                            title: RealModeMessages.resolve(kind: kind, defaultTitle: "\(service.displayName) 额度接近上限",
                                                            realMode: realMode, custom: customMessages[kind.customKey], context: ctx),
                            subtitle: "\(window.kind.label) 窗口 \(Int(window.usedPercent))%"
                                + (window.resetsAt.map { " · \(Self.countdown(to: $0, from: now)) 后重置" } ?? ""),
                            percent: window.usedPercent))
                        break // 每个窗口只取最高阈值
                    }
                }
                if previous >= resetDropFrom, window.usedPercent < resetDropFloor {
                    let defaultSubtitle = "\(window.kind.label) 额度已重置"
                    let subtitle = reportProvider?(service) ?? defaultSubtitle
                    let resetKind = HUDEvent.Kind.windowReset(service)
                    resetEvents.append(HUDEvent(
                        kind: resetKind,
                        title: RealModeMessages.resolve(kind: resetKind, defaultTitle: "\(service.displayName) 新额度窗口",
                                                        realMode: realMode, custom: customMessages[resetKind.customKey],
                                                        context: MessageContext(agent: service.displayName)),
                        subtitle: subtitle,
                        percent: window.usedPercent))
                }
            }
        }

        var depletionEvents: [HUDEvent] = []
        // 服务按确定性顺序，同一服务内 5h 先于周。
        for service in ServiceID.allCases {
            guard let list = depletions[service] else { continue }
            let ordered = list.filter { $0.willDepleteBeforeReset }
        // 周窗口按比例：耗尽早于重置期 60% 且距重置 ≥1 天时才触发。
                .filter { dep in
                    guard dep.kind == .weekly else { return true }
                    guard let reset = dep.resetsAt else { return false }
                    let resetLead = reset.timeIntervalSince(now)
                    let depLead = dep.etaTo100.timeIntervalSince(now)
                    return resetLead >= weeklyDepletionMinLead && depLead <= resetLead * weeklyDepletionRatio
                }
                .sorted { kindOrder($0.kind) < kindOrder($1.kind) }
            for depletion in ordered {
                let key = DepletionKey(service: service, kind: depletion.kind)
                let last = lastDepletionAt[key] ?? .distantPast
        // 周窗口按天预测所以较长（12h），5h 等小时窗口较短（30m）。
                let cooldown = depletion.kind == .weekly ? weeklyDepletionCooldown : depletionCooldown
                guard now.timeIntervalSince(last) >= cooldown else { continue }
                lastDepletionAt[key] = now
                let subtitle: String
                switch depletion.kind {
                case .weekly:
                    if depletion.etaTo100.timeIntervalSince(now) <= 24 * 3600 {
                        subtitle = "照这个趋势，今天内会耗尽每周额度"
                    } else {
                        let days = Self.daysUntil(depletion.etaTo100, from: now)
                        subtitle = "照这个趋势，约 \(days) 天后耗尽每周额度"
                    }
                default:
                    subtitle = "照这个速度，\(Self.countdown(to: depletion.etaTo100, from: now)) 后耗尽 5h 额度"
                }
                let depKind = HUDEvent.Kind.depletionRisk(service)
                depletionEvents.append(HUDEvent(
                    kind: depKind,
                    title: RealModeMessages.resolve(kind: depKind, defaultTitle: "⚠️ \(service.displayName) 即将耗尽",
                                                    realMode: realMode, custom: customMessages[depKind.customKey],
                                                    context: MessageContext(agent: service.displayName)),
                    subtitle: subtitle,
                    percent: nil))
            }
        }

        var spikeEvents: [HUDEvent] = []
        if baseline > 0, burnRate > baseline * spikeMultiplier,
           now.timeIntervalSince(lastSpikeAt) >= spikeCooldown {
            lastSpikeAt = now
            let ratio = burnRate / baseline
            spikeEvents.append(HUDEvent(
                kind: .burnSpike,
                title: RealModeMessages.resolve(kind: .burnSpike, defaultTitle: "Token 使用量突增",
                                                realMode: realMode, custom: customMessages[HUDEvent.Kind.burnSpike.customKey],
                                                context: .empty),
                subtitle: String(format: "正在以平时 %.1f 倍的速度消耗", ratio),
                percent: nil))
        }

        // 优先级：threshold > depletionRisk > reset > spike
        return thresholdEvents + depletionEvents + resetEvents + spikeEvents
    }

    private func kindOrder(_ kind: LimitWindow.Kind) -> Int {
        switch kind {
        case .session5h: return 0
        case .daily:     return 1
        case .weekly:    return 2
        }
    }

    /// 向上取整的天数（最少 1 天）。周窗口耗尽 "~N天" 显示用。
    public static func daysUntil(_ date: Date, from now: Date) -> Int {
        let seconds = max(0, date.timeIntervalSince(now))
        return max(1, Int(ceil(seconds / (24 * 3600))))
    }

    public static func countdown(to date: Date, from now: Date) -> String {
        let seconds = max(0, Int(date.timeIntervalSince(now)))
        let totalHours = seconds / 3600
        let minutes = (seconds % 3600) / 60
        if totalHours >= 24 {
            let days = totalHours / 24
            let hours = totalHours % 24
            return "\(days)d \(hours)h \(minutes)m"
        }
        return totalHours > 0 ? "\(totalHours)h \(minutes)m" : "\(minutes)m"
    }
}
