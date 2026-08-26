import Foundation
import Observation

/// 메뉴바 표시 모드. 자동 로테이션 없음 — 고정 표시 (깜빡임 방지, 사용자 결정).
/// 구버전 todayAndBurn/serviceRotation 저장값은 raw 불일치로 기본값(todayTokens) 폴백.
enum MenubarMode: String, CaseIterable, Identifiable {
    /// 오늘 누적 토큰 "✦ 612M" (기본).
    case todayTokens
    /// 소모 속도 "✦ 38K/m".
    case burnRate
    /// 켜진 에이전트 중 최고 사용률 "✦ N%".
    case maxPercent
    /// "✦"만 표시 (위험도 색).
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

/// 메뉴바에 동시 표시할 수 있는 항목 (다중 선택). 고정 순서로 렌더된다.
enum MenubarItem: String, CaseIterable, Identifiable {
    // 선언 순 = 메뉴바 렌더 순 = 설정 옵션 순.
    case usagePercent   // 사용률
    case resetCountdown // 리셋까지 남은 시간 (로테이션 서비스) "2h 15m"
    case todayTokens    // 오늘 누적 토큰 "612M"
    case burnRate       // 소모 속도 "38K/m"

    var id: String { rawValue }
    var label: String {
        switch self {
        case .usagePercent: return "使用率"
        case .resetCountdown: return "距离重置"
        case .todayTokens: return "今日请求 Token 总量"
        case .burnRate: return "请求 Token 速度（Token/分钟）"
        }
    }

    /// 주어진 집합을 allCases 고정 순으로 정렬한 배열 (렌더 순서).
    static func ordered(_ items: Set<MenubarItem>) -> [MenubarItem] {
        allCases.filter { items.contains($0) }
    }
}

/// 사용자 설정. UserDefaults 백킹, 네임스페이스 키 `aiglass.*`.
/// 단위 테스트는 코어가 아니므로 생략(수동 검증).
@MainActor
@Observable
final class AppSettings {
    private let defaults = UserDefaults.standard

    private enum Key {
        static let warnThreshold = "aiglass.warnThreshold"
        static let critThreshold = "aiglass.critThreshold"
        static let notificationsEnabled = "aiglass.notificationsEnabled"
        static let launchAtLogin = "aiglass.launchAtLogin"
        static let menubarMode = "aiglass.menubarMode"
        static let menubarItems = "aiglass.menubarItems"
        static let funMilestone = "aiglass.funMilestone"
        static let funRecord = "aiglass.funRecord"
        static let funStreak = "aiglass.funStreak"
        static let funWeeklyReport = "aiglass.funWeeklyReport"
        static let funSoundEnabled = "aiglass.funSoundEnabled"
        static let onboardingCompleted = "aiglass.onboardingCompleted"
        static let notifyLimitThreshold = "aiglass.notifyLimitThreshold"
        static let notifyDepletion = "aiglass.notifyDepletion"
        static let notifyWindowReset = "aiglass.notifyWindowReset"
        static let notifyBurnSpike = "aiglass.notifyBurnSpike"
        static let notifyComeback = "aiglass.notifyComeback"
        static let notifyBriefing = "aiglass.notifyBriefing"
        static let notifyUpdate = "aiglass.notifyUpdate"
        static let realMode = "aiglass.realMode"
        static let customMessages = "aiglass.customMessages"
    }

    var warnThreshold: Double {
        didSet { defaults.set(warnThreshold, forKey: Key.warnThreshold) }
    }
    var critThreshold: Double {
        didSet { defaults.set(critThreshold, forKey: Key.critThreshold) }
    }
    /// 시스템 알림센터(macOS 배너)로도 보낼지. 기본 off.
    var notificationsEnabled: Bool {
        didSet { defaults.set(notificationsEnabled, forKey: Key.notificationsEnabled) }
    }
    /// 사용량 알림 — 한도 임박(70/90% 교차). 기본 on.
    var notifyLimitThreshold: Bool {
        didSet { defaults.set(notifyLimitThreshold, forKey: Key.notifyLimitThreshold) }
    }
    /// 사용량 알림 — 소진 임박(리셋 전 소진 예측). 기본 on.
    var notifyDepletion: Bool {
        didSet { defaults.set(notifyDepletion, forKey: Key.notifyDepletion) }
    }
    /// 사용량 알림 — 새 윈도우 시작(한도 리셋 감지). 기본 on.
    var notifyWindowReset: Bool {
        didSet { defaults.set(notifyWindowReset, forKey: Key.notifyWindowReset) }
    }
    /// 사용량 알림 — 토큰 사용량 급증(평소의 N배). 기본 on.
    var notifyBurnSpike: Bool {
        didSet { defaults.set(notifyBurnSpike, forKey: Key.notifyBurnSpike) }
    }
    /// 활동 알림 — 컴백(공백 후 재개 인사). 기본 on.
    var notifyComeback: Bool {
        didSet { defaults.set(notifyComeback, forKey: Key.notifyComeback) }
    }
    /// 활동 알림 — 시간대별 브리핑(아침/점심/저녁 요약). 기본 on.
    var notifyBriefing: Bool {
        didSet { defaults.set(notifyBriefing, forKey: Key.notifyBriefing) }
    }
    /// 활동 알림 — 새 앱 버전 출시. 기본 on.
    var notifyUpdate: Bool {
        didSet { defaults.set(notifyUpdate, forKey: Key.notifyUpdate) }
    }
    /// REAL Mode — 알림 제목을 AI 의인화 멘트(엄살·이별·츤데레)로 교체. 기본 off.
    var realMode: Bool {
        didSet { defaults.set(realMode, forKey: Key.realMode) }
    }
    /// 이벤트별 사용자 커스텀 메시지 (customKey → config). 단일 JSON으로 직렬화 저장.
    var customMessages: [String: CustomMessageConfig] {
        didSet {
            if let data = try? JSONEncoder().encode(customMessages) {
                defaults.set(data, forKey: Key.customMessages)
            }
        }
    }
    /// SMAppService와 동기화 (배선은 LaunchAtLogin에서).
    var launchAtLogin: Bool {
        didSet { defaults.set(launchAtLogin, forKey: Key.launchAtLogin) }
    }
    /// 메뉴바 표시 모드 (MenubarMode rawValue). 기본 todayTokens.
    /// (레거시 — menubarItems로 마이그레이션됨. 신규 코드는 menubarItems 사용.)
    var menubarMode: MenubarMode {
        didSet { defaults.set(menubarMode.rawValue, forKey: Key.menubarMode) }
    }
    /// 메뉴바에 동시 표시할 항목 집합 (다중). 빈 집합이면 ✦ 아이콘만. 기본 [.todayTokens].
    var menubarItems: Set<MenubarItem> {
        didSet { defaults.set(menubarItems.map(\.rawValue).sorted(), forKey: Key.menubarItems) }
    }

    /// 구 단일 모드 → 신 항목 집합 1:1 변환.
    static func migratedItems(from mode: MenubarMode) -> Set<MenubarItem> {
        switch mode {
        case .todayTokens: return [.todayTokens]
        case .burnRate:    return [.burnRate]
        case .maxPercent:  return [.usagePercent]  // 최고%(고정) → 사용률(로테이션)으로 대체
        case .iconOnly:    return []        // 빈 집합 = ✦ fallback
        }
    }
    /// 재미 — 마일스톤 알림. 기본 on.
    var funMilestone: Bool {
        didSet { defaults.set(funMilestone, forKey: Key.funMilestone) }
    }
    /// 재미 — 신기록 알림. 기본 on.
    var funRecord: Bool {
        didSet { defaults.set(funRecord, forKey: Key.funRecord) }
    }
    /// 재미 — 스트릭(연속 사용일) 브리핑 표기. 기본 on.
    var funStreak: Bool {
        didSet { defaults.set(funStreak, forKey: Key.funStreak) }
    }
    /// 재미 — 월요일 주간 리포트. 기본 on.
    var funWeeklyReport: Bool {
        didSet { defaults.set(funWeeklyReport, forKey: Key.funWeeklyReport) }
    }
    /// 재미 — 알림성 이벤트 시 사운드 재생. 기본 off.
    var funSoundEnabled: Bool {
        didSet { defaults.set(funSoundEnabled, forKey: Key.funSoundEnabled) }
    }
    /// 첫 실행 온보딩 완료 여부. false면 앱 시작 시 온보딩 위저드 표시. 기본 false.
    var onboardingCompleted: Bool {
        didSet { defaults.set(onboardingCompleted, forKey: Key.onboardingCompleted) }
    }

    init() {
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
        // 구버전 raw(todayAndBurn/serviceRotation 등) 불일치 시 기본값 폴백 = 마이그레이션.
        let resolvedMode = MenubarMode(rawValue: defaults.string(forKey: Key.menubarMode) ?? "") ?? .todayTokens
        menubarMode = resolvedMode
        funMilestone = defaults.object(forKey: Key.funMilestone) as? Bool ?? true
        funRecord = defaults.object(forKey: Key.funRecord) as? Bool ?? true
        funStreak = defaults.object(forKey: Key.funStreak) as? Bool ?? true
        funWeeklyReport = defaults.object(forKey: Key.funWeeklyReport) as? Bool ?? true
        funSoundEnabled = defaults.object(forKey: Key.funSoundEnabled) as? Bool ?? false
        onboardingCompleted = defaults.object(forKey: Key.onboardingCompleted) as? Bool ?? false
        // 메뉴바 항목: 신 포맷 키가 있으면 그대로, 없으면 구 menubarMode에서 1회 마이그레이션.
        if let rawItems = defaults.stringArray(forKey: Key.menubarItems) {
            menubarItems = Set(rawItems.compactMap(MenubarItem.init(rawValue:)))
        } else {
            menubarItems = Self.migratedItems(from: resolvedMode)
        }
    }
}
