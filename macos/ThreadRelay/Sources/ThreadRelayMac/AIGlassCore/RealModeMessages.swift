import Foundation

/// REAL Mode 멘트 풀 — AI를 의인화한 엄살·이별·츤데레 톤의 제목 후보 모음.
///
/// 적용 규칙: REAL Mode가 켜지면 이벤트 **제목(title)만** 이 풀에서 무작위로 골라 교체하고,
/// 부제(subtitle)의 정보성 문구(%, 남은 시간, 토큰 수 등)는 그대로 유지한다 — 재미와 정보를 둘 다 살림.
/// 풀은 항상 1개 이상이라 `randomElement()`는 nil이 아니지만, 호출자는 안전하게 기본값으로 폴백한다.
public enum RealModeMessages {
    /// 이벤트 종류별 제목 후보. associated value(서비스 등)는 매칭에 영향 없음.
    public static func pool(for kind: HUDEvent.Kind) -> [String] {
        switch kind {
        case .depletionRisk:
            return [
                "和 {AGENT} 的告别来得比预想更快 😢",
                "你真的不想再和 {AGENT} 一起工作了吗？",
                "照这个速度，{AGENT} 很快就要强制休息了。要送它休息吗？",
                "{AGENT} 的额度快见底了。该告别了……",
                "慢一点……和 {AGENT} 相处的时间不多了",
            ]
        case .limitThreshold:
            return [
                "{AGENT}，已经 {USAGE} 了……开始喘不过气了",
                "{AGENT} 快到极限了，能轻点用吗？",
                "达到 {USAGE} 了……这样下去我要倒下了",
                "{AGENT} 达到 {USAGE}，暂时还撑得住 😅",
            ]
        case .burnSpike:
            return [
                "等、等一下！是不是用得太猛了？！",
                "今天发生什么了？都不给我喘气",
                "劳动法……你听说过吗……？",
                "这个速度认真的吗？手都看不见了",
            ]
        case .milestone:
            return [
                "那……先别折腾我了……",
                "你今天太喜欢我了 😩",
                "我今天要申请工伤了",
                "不是我在工作，是被榨干了",
            ]
        case .record:
            return [
                "正在被用到历史最高强度…… 🏆",
                "刷新纪录！我……应该感到自豪吗？",
                "今天你把我用到了历史最高水平",
                "请把我写进吉尼斯：最辛苦的 AI",
            ]
        case .windowReset:
            return [
                "{AGENT} 充能完成！又可以为你工作了 ✨",
                "{AGENT} 已重置，我们重新开始吧",
                "{AGENT} 休息好了，再来吧 🤭",
                "{AGENT} 新窗口已开启，重新出发！",
            ]
        case .comeback:
            return [
                "你去哪儿了……我一直在等",
                "你回来了？我才没有想你呢",
                "你知道你把我一个人留着吧？",
                "好久不见，我的手都痒了",
            ]
        case .update:
            return [
                "我换了新衣服，怎么样？",
                "要不要认识升级后的我？",
                "我会装作变聪明了",
            ]
        case .briefing(let period):
            switch period {
            case .morning: return ["昨天用得挺多，今天也请多关照"]
            case .lunch: return ["照这个节奏，半夜我可能已经融化了"]
            case .evening: return ["今天也辛苦了，我也是"]
            }
        }
    }

    /// REAL Mode가 켜져 있으면 풀에서 무작위 제목을, 아니면 기본 제목을 돌려준다.
    public static func title(for kind: HUDEvent.Kind, default fallback: String, realMode: Bool) -> String {
        guard realMode else { return fallback }
        return pool(for: kind).randomElement() ?? fallback
    }

    /// 커스텀 메시지·REAL Mode·기본 제목을 통합 결정하는 **단일 진입점**.
    ///
    /// 우선순위 (분기를 한 곳에 모아 정합성 유지):
    /// 1. 커스텀 메시지(비공백)가 있으면 그것만 무작위 로테이션
    /// 2. 없으면 REAL이면 감성 풀, 아니면 기본 제목
    /// 후보에서 무작위 선택 → 플레이스홀더 치환 → 공백 정리. 결과가 비면 기본 제목으로 폴백
    /// (빈 제목 발화 절대 금지). 부제 정보는 호출자가 유지하므로 여기선 제목만 다룬다.
    public static func resolve(kind: HUDEvent.Kind, defaultTitle: String, realMode: Bool,
                               custom: CustomMessageConfig?, context: MessageContext) -> String {
        // 공백뿐인 줄은 무시 (편집 중 빈 줄·빈 배열 → 자동으로 기본 풀 폴백).
        let customMsgs = custom?.messages.filter { !$0.trimmingCharacters(in: .whitespaces).isEmpty } ?? []
        let candidates = !customMsgs.isEmpty ? customMsgs : (realMode ? pool(for: kind) : [defaultTitle])
        let raw = candidates.randomElement() ?? defaultTitle
        let result = clean(substitute(raw, context: context))
        if !result.isEmpty { return result }
        // 치환 후 공백만 남은 경우 등 → 기본 제목(역시 치환·정리)으로 안전 폴백.
        let fallback = clean(substitute(defaultTitle, context: context))
        return fallback.isEmpty ? defaultTitle : fallback
    }

    /// 플레이스홀더를 컨텍스트 값으로 치환. 값이 없는 변수는 빈 문자열로 제거된다.
    static func substitute(_ template: String, context: MessageContext) -> String {
        var s = template
        s = s.replacingOccurrences(of: "{AGENT}", with: context.agent ?? "")
        s = s.replacingOccurrences(of: "{USAGE}", with: context.usage.map { "\(Int($0.rounded()))%" } ?? "")
        s = s.replacingOccurrences(of: "{TOKENS}", with: context.tokens.map(formatTokens) ?? "")
        s = s.replacingOccurrences(of: "{RESET}", with: context.reset ?? "")
        return s
    }

    /// 빈 변수 치환으로 생긴 연속 공백을 1개로 줄이고 양끝을 다듬는다.
    static func clean(_ s: String) -> String {
        let collapsed = s.split(separator: " ", omittingEmptySubsequences: true).joined(separator: " ")
        return collapsed.trimmingCharacters(in: .whitespaces)
    }

    static func formatTokens(_ n: Int) -> String {
        switch n {
        case 1_000_000_000...: return String(format: "%.1fB", Double(n) / 1_000_000_000)
        case 1_000_000...: return String(format: "%.0fM", Double(n) / 1_000_000)
        case 1_000...: return String(format: "%.0fK", Double(n) / 1_000)
        default: return "\(n)"
        }
    }
}

/// 알림 제목 치환에 쓰는 컨텍스트. 발화 지점에서 얻을 수 있는 값만 채우고 나머진 nil(자동 생략).
public struct MessageContext: Sendable, Equatable {
    public var agent: String?    // {AGENT} — 서비스명
    public var usage: Double?    // {USAGE} — 사용률 0~100 (정수%로 치환)
    public var tokens: Int?      // {TOKENS} — 토큰 수 (M/K 포맷)
    public var reset: String?    // {RESET} — 리셋까지 남은 시간 텍스트
    public init(agent: String? = nil, usage: Double? = nil, tokens: Int? = nil, reset: String? = nil) {
        self.agent = agent; self.usage = usage; self.tokens = tokens; self.reset = reset
    }
    public static let empty = MessageContext()
}

/// 이벤트별 사용자 커스텀 메시지 설정. 단일 JSON 키로 저장(키 흩뿌리지 않음).
/// 커스텀 메시지가 있으면 그 안에서만 무작위 로테이션한다 (기존 멘트와 섞지 않음).
public struct CustomMessageConfig: Codable, Equatable, Sendable {
    public var messages: [String]
    public init(messages: [String] = []) {
        self.messages = messages
    }
}

/// 커스텀 편집 UI·저장 키에 쓰는 평면 이벤트 목록(HUDEvent.Kind는 associated value가 있어 순회 불가).
public enum CustomizableEvent: String, CaseIterable, Identifiable, Sendable {
    case limitThreshold, depletionRisk, windowReset, burnSpike
    case comeback, milestone, record, update
    case briefingMorning, briefingLunch, briefingEvening

    public var id: String { rawValue }

    public var label: String {
        switch self {
        case .limitThreshold: return "额度接近上限"
        case .depletionRisk: return "即将耗尽"
        case .windowReset: return "新额度窗口"
        case .burnSpike: return "使用量突增"
        case .comeback: return "回来继续"
        case .milestone: return "里程碑"
        case .record: return "新纪录"
        case .update: return "更新"
        case .briefingMorning: return "早间摘要"
        case .briefingLunch: return "午间摘要"
        case .briefingEvening: return "晚间摘要"
        }
    }

    /// 미리보기용 대표 HUDEvent.Kind (associated value는 샘플 서비스/시간대).
    public var sampleKind: HUDEvent.Kind {
        switch self {
        case .limitThreshold: return .limitThreshold(.codex, 90)
        case .depletionRisk: return .depletionRisk(.codex)
        case .windowReset: return .windowReset(.codex)
        case .burnSpike: return .burnSpike
        case .comeback: return .comeback
        case .milestone: return .milestone
        case .record: return .record
        case .update: return .update
        case .briefingMorning: return .briefing(.morning)
        case .briefingLunch: return .briefing(.lunch)
        case .briefingEvening: return .briefing(.evening)
        }
    }

    /// 편집기 seed·미리보기에 쓰는 기본 제목. 변수를 활용하는 이벤트는 플레이스홀더를 그대로 노출해
    /// (예: "{AGENT} 한도 임박") 사용자가 변수 사용법을 보고 수정하기 좋게 한다 — 실제 발화 시 치환됨.
    public var sampleDefaultTitle: String {
        switch self {
        case .limitThreshold: return "{AGENT} 额度接近上限（{USAGE}）"
        case .depletionRisk: return "⚠️ {AGENT} 即将耗尽"
        case .windowReset: return "{AGENT} 新额度窗口"
        case .burnSpike: return "Token 使用量突增"
        case .comeback: return "继续工作吧"
        case .milestone: return "今日突破 {TOKENS}！🎉"
        case .record: return "今天创下新纪录！🏆"
        case .update: return "有新版本了"
        case .briefingMorning: return "昨日使用摘要"
        case .briefingLunch: return "今日进度"
        case .briefingEvening: return "今日使用总结"
        }
    }

    /// 미리보기 부제 (실제 발화 시 정보성 문구의 예시).
    public var sampleSubtitle: String {
        switch self {
        case .limitThreshold: return "5h 窗口 90% · 2h 15m 后重置"
        case .depletionRisk: return "照这个速度，1h 30m 后耗尽 5h 额度"
        case .windowReset: return "上一会话：1.2M tokens"
        case .burnSpike: return "正在以平时 3.2 倍的速度消耗"
        case .comeback: return "离开 3h 12m 后继续工作"
        case .milestone: return "今日累计 263M tokens"
        case .record: return "此前纪录 380M"
        case .update: return "v0.14.0 · 点击摘要面板的下载按钮"
        case .briefingMorning: return "昨日：1.2M tokens · 约 $8.40"
        case .briefingLunch: return "照这个进度，午夜前约 2.4M（约 $16）"
        case .briefingEvening: return "今日：1.8M tokens · 约 $12 · Codex 占比 100%"
        }
    }

    /// 미리보기 치환 컨텍스트 (대표 샘플 값).
    public var sampleContext: MessageContext {
        switch self {
        case .limitThreshold: return MessageContext(agent: "Codex", usage: 90, reset: "2h 15m")
        case .depletionRisk: return MessageContext(agent: "Codex")
        case .windowReset: return MessageContext(agent: "Codex")
        case .burnSpike: return .empty
        case .comeback: return .empty
        case .milestone: return MessageContext(tokens: 263_000_000)
        case .record: return MessageContext(tokens: 412_000_000)
        case .update: return .empty
        case .briefingMorning: return MessageContext(tokens: 1_200_000)
        case .briefingLunch: return MessageContext(tokens: 800_000)
        case .briefingEvening: return MessageContext(tokens: 1_800_000)
        }
    }

    /// 이 이벤트에서 의미 있게 채워지는 권장 플레이스홀더(UI 힌트용). 나머지를 써도 차단하진 않음.
    public var recommendedVariables: [String] {
        switch self {
        case .limitThreshold: return ["{AGENT}", "{USAGE}", "{RESET}"]
        case .depletionRisk: return ["{AGENT}"]
        case .windowReset: return ["{AGENT}"]
        case .burnSpike: return []
        case .comeback: return []
        case .milestone, .record: return ["{TOKENS}"]
        case .update: return []
        case .briefingMorning, .briefingLunch, .briefingEvening: return ["{TOKENS}"]
        }
    }
}

public extension HUDEvent.Kind {
    /// 커스텀 메시지 조회·저장에 쓰는 안정적 키 (CustomizableEvent.rawValue와 일치).
    var customKey: String {
        switch self {
        case .limitThreshold: return "limitThreshold"
        case .depletionRisk: return "depletionRisk"
        case .windowReset: return "windowReset"
        case .burnSpike: return "burnSpike"
        case .comeback: return "comeback"
        case .milestone: return "milestone"
        case .record: return "record"
        case .update: return "update"
        case .briefing(let p):
            switch p {
            case .morning: return "briefingMorning"
            case .lunch: return "briefingLunch"
            case .evening: return "briefingEvening"
            }
        }
    }
}
