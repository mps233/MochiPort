import Foundation

public struct HUDEvent: Equatable {
    public enum Kind: Equatable {
        case limitThreshold(ServiceID, Int)  // 70 또는 90 교차
        case depletionRisk(ServiceID)        // 리셋 전 소진 임박
        case windowReset(ServiceID)
        case burnSpike
        case briefing(BriefingEngine.Period)
        case comeback                         // 활동 공백 ≥3h 후 재시작
        case milestone                        // 오늘 누적 토큰 명예 임계 통과
        case record                           // 일일 토큰 신기록 경신
        case update                           // 새 앱 버전 출시 (EventEngine과 무관 — 타입만 공유)
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
    public var resetDropFloor: Double = 10  // 이 미만으로 떨어지면 리셋으로 간주
    public var resetDropFrom: Double = 30   // 직전 값이 이 이상이었을 때만
    /// depletionRisk 서비스별 쿨다운 (5h 등 시간 윈도우, 기본 30분 — 긴박하므로 짧게).
    public var depletionCooldown: TimeInterval = 30 * 60
    /// 주간 소진 임박 쿨다운 (기본 12시간 — 일 단위 예측이라 하루 최대 2번이면 충분).
    public var weeklyDepletionCooldown: TimeInterval = 12 * 3600
    /// REAL Mode — 켜면 이벤트 제목을 의인화 감성 멘트로 교체(부제 정보는 유지). 기본 off.
    public var realMode: Bool = false
    /// 이벤트별 사용자 커스텀 메시지 (kind.customKey → config). 호출자가 주입. 기본 없음.
    public var customMessages: [String: CustomMessageConfig] = [:]
    /// 주간 소진 임박 발화 기준 — 소진 예상이 "리셋까지 남은 기간 × 이 비율" 이전일 때만 발화한다.
    /// 절대 일수 대신 비율로 잡아, 리셋이 임박할수록 더 임박한 소진만 알린다
    /// (예: 0.6 → 리셋 6일이면 ~3.6일·리셋 2일이면 ~1.2일 이내 소진). 5h 윈도우엔 미적용.
    public var weeklyDepletionRatio: Double = 0.6
    /// 주간 소진 임박은 리셋까지 이 시간 이상 남았을 때만 발화 (당일/임박 시 무의미). 기본 1일.
    public var weeklyDepletionMinLead: TimeInterval = 24 * 3600

    private var lastPercent: [ServiceID: [LimitWindow.Kind: Double]] = [:]
    private var lastSpikeAt: Date = .distantPast
    private struct DepletionKey: Hashable { let service: ServiceID; let kind: LimitWindow.Kind }
    private var lastDepletionAt: [DepletionKey: Date] = [:]

    public init() {}

    /// - Parameters:
    ///   - burnRate: `UsageStore.tokensPerMinute(windowMinutes: 10)` 값 (현재 분당 토큰 소모율)
    ///   - baseline: `UsageStore.activeBaselineRate()` 값 (평소 활동 시의 기준 소모율)
    ///   - depletions: 서비스별 소진 예측 목록(윈도우당 0~2개). `willDepleteBeforeReset`인 것만 depletionRisk 발화 ((서비스, kind)별 30분 쿨다운). 기본 `[:]`.
    ///   - reportProvider: windowReset 발화 시 subtitle을 교체할 세션 요약 공급자. non-nil 문자열을 주면 그 값을 subtitle로 사용. 기본 nil.
    /// 우선순위: threshold > depletionRisk > reset > spike.
    /// 참고: 임계 근처 오실레이션(71→69→71 재발화)은 의도된 MVP 단순화로 미보호.
    public func evaluate(limits: [ServiceID: [LimitWindow]],
                         burnRate: Double, baseline: Double, now: Date,
                         depletions: [ServiceID: [Depletion]] = [:],
                         reportProvider: ((ServiceID) -> String?)? = nil) -> [HUDEvent] {
        var thresholdEvents: [HUDEvent] = []
        var resetEvents: [HUDEvent] = []

        for (service, windows) in limits {
            for window in windows {
                // previous 기본값 0: nil이면 0으로 간주하여 첫 관측값이 이미 임계값 이상이면 발화
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
                        break // 한 윈도우당 최고 임계값 하나만
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
        // 서비스는 결정적 순서로, 같은 서비스 안에서는 5h를 주간보다 먼저.
        for service in ServiceID.allCases {
            guard let list = depletions[service] else { continue }
            let ordered = list.filter { $0.willDepleteBeforeReset }
                // 주간은 비율 기반: 소진이 리셋 기간의 60% 이전 + 리셋까지 1일 이상 남았을 때만.
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
                // 주간은 일 단위 예측이라 길게(12h), 5h 등 시간 윈도우는 짧게(30m).
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

        // 우선순위: threshold > depletionRisk > reset > spike
        return thresholdEvents + depletionEvents + resetEvents + spikeEvents
    }

    private func kindOrder(_ kind: LimitWindow.Kind) -> Int {
        switch kind {
        case .session5h: return 0
        case .daily:     return 1
        case .weekly:    return 2
        }
    }

    /// 올림한 일수 (최소 1일). 주간 소진 "~N일" 표시용.
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
