import Foundation

/// REAL 模式文案池——把 AI 拟人化（撒娇·告别·傲娇）的标题候选集合。
///
/// 应用规则：REAL 模式开启时，事件**仅标题（title）**从这个池中随机挑选替换，
/// 副标题（subtitle）的信息性文案（%、剩余时间、Token 数等）保持不变——趣味与信息兼得。
/// 池保证至少 1 条，`randomElement()` 不会为 nil，但调用方仍应安全地回退到默认值。
public enum RealModeMessages {
    /// 按事件类型的标题候选。关联值（服务等）不影响匹配。
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

    /// REAL 模式开启时从池中随机取标题，否则返回默认标题。
    public static func title(for kind: HUDEvent.Kind, default fallback: String, realMode: Bool) -> String {
        guard realMode else { return fallback }
        return pool(for: kind).randomElement() ?? fallback
    }

    /// 整合自定义消息、REAL 模式与默认标题的**单一入口**。
    ///
    /// 优先级（分支集中在一处以保持一致性）：
    /// 1. 有非空自定义消息时只在其中随机轮换
    /// 2. 没有时 REAL 用情感文案池，否则用默认标题
    /// 从候选中随机选择 → 占位符替换 → 清理空白。结果为空时回退默认标题
    /// （绝不发出空标题）。副标题信息由调用方维护，这里只处理标题。
    public static func resolve(kind: HUDEvent.Kind, defaultTitle: String, realMode: Bool,
                               custom: CustomMessageConfig?, context: MessageContext) -> String {
        // 忽略只有空白的行（编辑中的空行·空数组 → 自动回退默认池）。
        let customMsgs = custom?.messages.filter { !$0.trimmingCharacters(in: .whitespaces).isEmpty } ?? []
        let candidates = !customMsgs.isEmpty ? customMsgs : (realMode ? pool(for: kind) : [defaultTitle])
        let raw = candidates.randomElement() ?? defaultTitle
        let result = clean(substitute(raw, context: context))
        if !result.isEmpty { return result }
        // 替换后只剩空白等情况 → 安全回退到默认标题（同样替换·清理）。
        let fallback = clean(substitute(defaultTitle, context: context))
        return fallback.isEmpty ? defaultTitle : fallback
    }

    /// 用上下文值替换占位符。没有值的变量会被移除为空字符串。
    static func substitute(_ template: String, context: MessageContext) -> String {
        var s = template
        s = s.replacingOccurrences(of: "{AGENT}", with: context.agent ?? "")
        s = s.replacingOccurrences(of: "{USAGE}", with: context.usage.map { "\(Int($0.rounded()))%" } ?? "")
        s = s.replacingOccurrences(of: "{TOKENS}", with: context.tokens.map(formatTokens) ?? "")
        s = s.replacingOccurrences(of: "{RESET}", with: context.reset ?? "")
        return s
    }

    /// 把变量替换产生的连续空格压缩为一个并修剪首尾。
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

/// 用于通知标题替换的上下文。只填触发点能拿到的值，其余为 nil（自动省略）。
public struct MessageContext: Sendable, Equatable {
    public var agent: String?    // {AGENT}——服务名
    public var usage: Double?    // {USAGE}——使用率 0~100（按整数% 替换）
    public var tokens: Int?      // {TOKENS}——Token 数（M/K 格式）
    public var reset: String?    // {RESET}——距重置剩余时间的文本
    public init(agent: String? = nil, usage: Double? = nil, tokens: Int? = nil, reset: String? = nil) {
        self.agent = agent; self.usage = usage; self.tokens = tokens; self.reset = reset
    }
    public static let empty = MessageContext()
}

/// 每种事件的自定义消息设置。以单个 JSON 键存储（不散落多个键）。
/// 有自定义消息时只在其内部随机轮换（不与内置文案混合）。
public struct CustomMessageConfig: Codable, Equatable, Sendable {
    public var messages: [String]
    public init(messages: [String] = []) {
        self.messages = messages
    }
}

/// 自定义编辑 UI 与存储键使用的扁平事件列表（HUDEvent.Kind 带关联值，无法直接遍历）。
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

    /// 编辑器种子与预览用的默认标题。使用变量的事件直接暴露占位符
    /// （如 "{AGENT} 额度临近"），方便用户理解变量用法——实际触发时会被替换。
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
}

public extension HUDEvent.Kind {
    /// 查询·保存自定义消息用的稳定键（与 CustomizableEvent.rawValue 一致）。
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
