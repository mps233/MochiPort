import Foundation
import Observation

/// 菜单栏显示模式。无自动轮换——固定显示（避免闪烁，由用户决定）。
/// 旧版 todayAndBurn/serviceRotation 存储值因 raw 不匹配会回退到默认值（todayTokens）。
enum MenubarMode: String, CaseIterable, Identifiable {
    /// 今日累计 Token "✦ 612M"（默认）。
    case todayTokens
    /// 消耗速度 "✦ 38K/m"。
    case burnRate
    /// 已开启代理中的最高使用率 "✦ N%"。
    case maxPercent
    /// 只显示 "✦"（按风险着色）。
    case iconOnly

    var id: String { rawValue }
    var label: String {
        switch self {
        case .todayTokens: return "今日请求 Token 总量"
        case .burnRate: return "请求 Token 速度（Token/分钟）"
        case .maxPercent: return "最高使用率"
        case .iconOnly: return "仅显示图标"
        }
    }
}

/// 可同时在菜单栏显示的条目（多选）。按固定顺序渲染。
enum MenubarItem: String, CaseIterable, Identifiable {
    // 声明顺序 = 菜单栏渲染顺序 = 设置选项顺序。
    case usagePercent   // 使用率
    case resetCountdown // 距重置剩余时间（轮换服务）"2h 15m"
    case todayTokens    // 今日累计 Token "612M"
    case burnRate       // 消耗速度 "38K/m"

    var id: String { rawValue }
    var label: String {
        switch self {
        case .usagePercent: return "使用率"
        case .resetCountdown: return "距离重置"
        case .todayTokens: return "今日请求 Token 总量"
        case .burnRate: return "请求 Token 速度（Token/分钟）"
        }
    }

    /// 将给定集合按 allCases 固定顺序排序的数组（渲染顺序）。
    static func ordered(_ items: Set<MenubarItem>) -> [MenubarItem] {
        allCases.filter { items.contains($0) }
    }
}

/// 用户设置。UserDefaults 持久化，键名空间 `mochiport.*`；
/// 旧版 `aiglass.*` 键在首次读取前由 `migrateLegacyKeys()` 一次性搬迁。
/// 非核心逻辑，省略单元测试（人工验证）。
@MainActor
@Observable
final class AppSettings {
    private let defaults = UserDefaults.standard

    private enum Key {
        static let warnThreshold = "mochiport.warnThreshold"
        static let critThreshold = "mochiport.critThreshold"
        static let notificationsEnabled = "mochiport.notificationsEnabled"
        static let launchAtLogin = "mochiport.launchAtLogin"
        static let menubarMode = "mochiport.menubarMode"
        static let menubarItems = "mochiport.menubarItems"
        static let funMilestone = "mochiport.funMilestone"
        static let funRecord = "mochiport.funRecord"
        static let funStreak = "mochiport.funStreak"
        static let funWeeklyReport = "mochiport.funWeeklyReport"
        static let funSoundEnabled = "mochiport.funSoundEnabled"
        static let onboardingCompleted = "mochiport.onboardingCompleted"
        static let notifyLimitThreshold = "mochiport.notifyLimitThreshold"
        static let notifyDepletion = "mochiport.notifyDepletion"
        static let notifyWindowReset = "mochiport.notifyWindowReset"
        static let notifyBurnSpike = "mochiport.notifyBurnSpike"
        static let notifyComeback = "mochiport.notifyComeback"
        static let notifyBriefing = "mochiport.notifyBriefing"
        static let notifyUpdate = "mochiport.notifyUpdate"
        static let realMode = "mochiport.realMode"
        static let customMessages = "mochiport.customMessages"

        /// 旧 `aiglass.*` 键一次性搬迁到 `mochiport.*`：新键缺失且旧键存在时复制。
        /// 旧键保留不删，便于降级时回读。
        static func migrateLegacyKeys() {
            let defaults = UserDefaults.standard
            let keys = [
                warnThreshold, critThreshold, notificationsEnabled, launchAtLogin,
                menubarMode, menubarItems, funMilestone, funRecord, funStreak,
                funWeeklyReport, funSoundEnabled, onboardingCompleted,
                notifyLimitThreshold, notifyDepletion, notifyWindowReset,
                notifyBurnSpike, notifyComeback, notifyBriefing, notifyUpdate,
                realMode, customMessages,
            ]
            for key in keys {
                let legacy = key.replacingOccurrences(of: "mochiport.", with: "aiglass.")
                if defaults.object(forKey: key) == nil,
                   let value = defaults.object(forKey: legacy) {
                    defaults.set(value, forKey: key)
                }
            }
        }
    }

    var warnThreshold: Double {
        didSet { defaults.set(warnThreshold, forKey: Key.warnThreshold) }
    }
    var critThreshold: Double {
        didSet { defaults.set(critThreshold, forKey: Key.critThreshold) }
    }
    /// 是否同时发送到系统通知中心（macOS 横幅）。默认关。
    var notificationsEnabled: Bool {
        didSet { defaults.set(notificationsEnabled, forKey: Key.notificationsEnabled) }
    }
    /// 用量通知——接近上限（跨过 70/90%）。默认开。
    var notifyLimitThreshold: Bool {
        didSet { defaults.set(notifyLimitThreshold, forKey: Key.notifyLimitThreshold) }
    }
    /// 用量通知——即将耗尽（重置前耗尽预测）。默认开。
    var notifyDepletion: Bool {
        didSet { defaults.set(notifyDepletion, forKey: Key.notifyDepletion) }
    }
    /// 用量通知——新窗口开始（检测到额度重置）。默认开。
    var notifyWindowReset: Bool {
        didSet { defaults.set(notifyWindowReset, forKey: Key.notifyWindowReset) }
    }
    /// 用量通知——Token 消耗骤增（平时 N 倍）。默认开。
    var notifyBurnSpike: Bool {
        didSet { defaults.set(notifyBurnSpike, forKey: Key.notifyBurnSpike) }
    }
    /// 活动通知——回归（空闲后重新开始时的问候）。默认开。
    var notifyComeback: Bool {
        didSet { defaults.set(notifyComeback, forKey: Key.notifyComeback) }
    }
    /// 活动通知——时段简报（早/午/晚摘要）。默认开。
    var notifyBriefing: Bool {
        didSet { defaults.set(notifyBriefing, forKey: Key.notifyBriefing) }
    }
    /// 活动通知——新版本发布。默认开。
    var notifyUpdate: Bool {
        didSet { defaults.set(notifyUpdate, forKey: Key.notifyUpdate) }
    }
    /// REAL 模式——通知标题替换为 AI 拟人化文案（撒娇·告别·傲娇）。默认关。
    var realMode: Bool {
        didSet { defaults.set(realMode, forKey: Key.realMode) }
    }
    /// 每种事件的自定义消息（customKey → config），序列化为单个 JSON 存储。
    var customMessages: [String: CustomMessageConfig] {
        didSet {
            if let data = try? JSONEncoder().encode(customMessages) {
                defaults.set(data, forKey: Key.customMessages)
            }
        }
    }
    /// 与 SMAppService 同步（接线在 LaunchAtLogin 中）。
    var launchAtLogin: Bool {
        didSet { defaults.set(launchAtLogin, forKey: Key.launchAtLogin) }
    }
    /// 菜单栏显示模式（MenubarMode rawValue）。默认 todayTokens。
    /// （旧版——已迁移到 menubarItems，新代码请使用 menubarItems。）
    var menubarMode: MenubarMode {
        didSet { defaults.set(menubarMode.rawValue, forKey: Key.menubarMode) }
    }
    /// 同时在菜单栏显示的条目集合（多选）。空集合时只显示 ✦ 图标。默认 [.todayTokens]。
    var menubarItems: Set<MenubarItem> {
        didSet { defaults.set(menubarItems.map(\.rawValue).sorted(), forKey: Key.menubarItems) }
    }

    /// 旧单一模式 → 新条目集合的一对一转换。
    static func migratedItems(from mode: MenubarMode) -> Set<MenubarItem> {
        switch mode {
        case .todayTokens: return [.todayTokens]
        case .burnRate:    return [.burnRate]
        case .maxPercent:  return [.usagePercent]  // 最高%（固定）→ 用轮换的使用率替代
        case .iconOnly:    return []        // 空集合 = 回退到 ✦
        }
    }
    /// 趣味——里程碑通知。默认开。
    var funMilestone: Bool {
        didSet { defaults.set(funMilestone, forKey: Key.funMilestone) }
    }
    /// 趣味——破纪录通知。默认开。
    var funRecord: Bool {
        didSet { defaults.set(funRecord, forKey: Key.funRecord) }
    }
    /// 趣味——简报中的连续使用天数标注。默认开。
    var funStreak: Bool {
        didSet { defaults.set(funStreak, forKey: Key.funStreak) }
    }
    /// 趣味——周一的周报。默认开。
    var funWeeklyReport: Bool {
        didSet { defaults.set(funWeeklyReport, forKey: Key.funWeeklyReport) }
    }
    /// 趣味——通知类事件播放音效。默认关。
    var funSoundEnabled: Bool {
        didSet { defaults.set(funSoundEnabled, forKey: Key.funSoundEnabled) }
    }
    /// 首次启动引导是否已完成。false 时启动显示引导向导。默认 false。
    var onboardingCompleted: Bool {
        didSet { defaults.set(onboardingCompleted, forKey: Key.onboardingCompleted) }
    }

    init() {
        Key.migrateLegacyKeys()
        warnThreshold = defaults.object(forKey: Key.warnThreshold) as? Double ?? 70
        critThreshold = defaults.object(forKey: Key.critThreshold) as? Double ?? 90
        notificationsEnabled = defaults.object(forKey: Key.notificationsEnabled) as? Bool ?? false
        notifyLimitThreshold = defaults.object(forKey: Key.notifyLimitThreshold) as? Bool ?? true
        notifyDepletion = defaults.object(forKey: Key.notifyDepletion) as? Bool ?? true
        notifyWindowReset = defaults.object(forKey: Key.notifyWindowReset) as? Bool ?? true
        notifyBurnSpike = defaults.object(forKey: Key.notifyBurnSpike) as? Bool ?? true
        notifyComeback = defaults.object(forKey: Key.notifyComeback) as? Bool ?? true
        notifyBriefing = defaults.object(forKey: Key.notifyBriefing) as? Bool ?? true
        notifyUpdate = defaults.object(forKey: Key.notifyUpdate) as? Bool ?? true
        realMode = defaults.object(forKey: Key.realMode) as? Bool ?? false
        if let data = defaults.data(forKey: Key.customMessages),
           let decoded = try? JSONDecoder().decode([String: CustomMessageConfig].self, from: data) {
            customMessages = decoded
        } else {
            customMessages = [:]
        }
        launchAtLogin = defaults.object(forKey: Key.launchAtLogin) as? Bool ?? false
        // 旧版 raw（todayAndBurn/serviceRotation 等）不匹配时回退默认值 = 已迁移。
        let resolvedMode = MenubarMode(rawValue: defaults.string(forKey: Key.menubarMode) ?? "") ?? .todayTokens
        menubarMode = resolvedMode
        funMilestone = defaults.object(forKey: Key.funMilestone) as? Bool ?? true
        funRecord = defaults.object(forKey: Key.funRecord) as? Bool ?? true
        funStreak = defaults.object(forKey: Key.funStreak) as? Bool ?? true
        funWeeklyReport = defaults.object(forKey: Key.funWeeklyReport) as? Bool ?? true
        funSoundEnabled = defaults.object(forKey: Key.funSoundEnabled) as? Bool ?? false
        onboardingCompleted = defaults.object(forKey: Key.onboardingCompleted) as? Bool ?? false
        // 菜单栏条目：有新格式键就直接使用，否则从旧 menubarMode 迁移一次。
        if let rawItems = defaults.stringArray(forKey: Key.menubarItems) {
            menubarItems = Set(rawItems.compactMap(MenubarItem.init(rawValue:)))
        } else {
            menubarItems = Self.migratedItems(from: resolvedMode)
        }
    }
}
