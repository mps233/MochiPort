import AppKit
import Combine
import Foundation

/// GUI-local usage coordinator. It deliberately has no HUD or daemon
/// lifecycle responsibilities: it reads local client logs, updates the
/// dashboard stores, and turns usage milestones into history/notifications.
@MainActor
final class AIGlassCoordinator: ObservableObject {
    let store = UsageStore()
    let settings = AppSettings()
    let eventLog = EventLog()
    let eventEngine = EventEngine()
    let notifier = Notifier()
    let updateState = UpdateState()
    let milestoneTracker = MilestoneTracker()
    let briefingEngine = BriefingEngine()
    let statsStore: DailyStatsStore?

    private lazy var codexCollector = CodexCollector(roots: [
        Self.homePath(".codex/sessions"),
        Self.homePath(".codex/archived_sessions"),
    ])
    private var directoryWatcher: DirectoryWatcher?
    private var refreshTimer: Timer?
    private var updateTask: Task<Void, Never>?
    private var recentHydrationTask: Task<Void, Never>?
    private var historicalRebuildTask: Task<Void, Never>?
    private var lastStatsWrite = Date.distantPast
    private var lastRecentHydrationAttempt = Date.distantPast
    private var lastHistoricalRebuildAttempt = Date.distantPast
    private var lastBriefingEvaluation = Date.distantPast
    private var didStart = false
    private var lastRecordDay: String?

    init() {
        // 与原 ai-glass 共用历史数据库，确保迁移后趋势和 Codex 数值连续。
        let path = Self.homePath("Library/Application Support/AIGlass/stats.db")
        statsStore = DailyStatsStore(path: path.path)
        start()
    }

    func start() {
        guard !didStart else { return }
        didStart = true

        let home = FileManager.default.homeDirectoryForCurrentUser.path
        directoryWatcher = DirectoryWatcher(paths: [
            home + "/.codex/sessions",
            home + "/.codex/archived_sessions",
        ]) { [weak self] in
            self?.refresh()
        }

        refreshTimer = Timer.scheduledTimer(withTimeInterval: 30, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.refresh() }
        }
        refresh()
        updateTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(15))
            guard !Task.isCancelled else { return }
            await self?.checkForUpdates()
        }
    }

    func refresh() {
        if !codexCollector.hasHydratedRecentHistory {
            if recentHydrationTask == nil,
               Date().timeIntervalSince(lastRecentHydrationAttempt) >= 60 {
                scheduleRecentHydration()
            }
        } else {
            if let statsStore,
               statsStore.needsCodexRebuild,
               historicalRebuildTask == nil,
               Date().timeIntervalSince(lastHistoricalRebuildAttempt) >= 60,
               statsStore.beginCodexRebuildIfAllowed() {
                scheduleHistoricalRebuild(statsStore)
            }
            // The hydration pass has already advanced every reader cursor.
            // Do not synchronously enumerate the log tree again in the same
            // refresh; the watcher/timer will collect appended bytes later.
            if recentHydrationTask == nil {
                codexCollector.collect(into: store)
            }
        }
        evaluateComeback()
        evaluateUsageEvents()
        persistStatsIfNeeded()
        evaluateBriefingIfNeeded()
        // The menu-bar label reads UsageStore through this coordinator. Keep
        // the small label in sync without adding a high-frequency timer.
        objectWillChange.send()
    }

    /// Hydrate recent Codex events off the main actor in bounded batches.
    /// SwiftUI can render between batches instead of waiting for a full file
    /// (some local session files are hundreds of megabytes) to be parsed.
    private func scheduleRecentHydration() {
        lastRecentHydrationAttempt = Date()
        recentHydrationTask = Task { [weak self] in
            guard let self else { return }
            defer { recentHydrationTask = nil }
            guard await codexCollector.hydrateRecentHistory(into: store), !Task.isCancelled else { return }
            // Re-enter while this task is still registered: refresh can start
            // the delayed historical rebuild but skips a duplicate synchronous
            // traversal of the same recent files.
            refresh()
        }
    }

    private func scheduleHistoricalRebuild(_ statsStore: DailyStatsStore) {
        lastHistoricalRebuildAttempt = Date()
        let roots = [
            Self.homePath(".codex/sessions"),
            Self.homePath(".codex/archived_sessions"),
        ]
        historicalRebuildTask = Task { [weak self] in
            defer {
                self?.historicalRebuildTask = nil
            }
            // Let the initial window settle before competing for disk and
            // CPU with the UI's first render and event batches.
            try? await Task.sleep(for: .seconds(10))
            guard !Task.isCancelled else { return }
            let rows = await Task.detached(priority: .background) {
                CodexCollector.historicalRows(roots: roots)
            }.value
            guard let self else { return }
            guard !Task.isCancelled, let rows else {
                statsStore.markCodexRebuildFailed()
                return
            }
            let rebuilt = statsStore.rebuildCodexStats(
                rows: rows,
                databaseBackupURL: statsStore.defaultRebuildBackupURL())
            if !rebuilt { statsStore.markCodexRebuildFailed() }
            if rebuilt {
                // Avoid an immediate second full `events` pass after the
                // rebuild; the normal timer will persist new tail events.
                self.lastStatsWrite = Date()
            }
            self.objectWillChange.send()
        }
    }

    private func evaluateUsageEvents() {
        let now = Date()
        eventEngine.thresholds = [Int(settings.warnThreshold), Int(settings.critThreshold)]
        eventEngine.realMode = settings.realMode
        eventEngine.customMessages = settings.customMessages
        let enabledLimits = store.limits.filter { $0.key == .codex }
        let events = eventEngine.evaluate(
            limits: enabledLimits,
            burnRate: store.tokensPerMinute(windowMinutes: 10, now: now),
            baseline: store.activeBaselineRate(now: now),
            now: now,
            depletions: depletionMap(now: now),
            reportProvider: { [store] service in
                store.sessionSummary(service: service, from: now.addingTimeInterval(-5 * 3600), to: now)
            })
        guard let event = events.first(where: eventKindEnabled) else { return }
        record(event)
        if settings.notificationsEnabled { notifier.notify(title: event.title, subtitle: event.subtitle) }
        if settings.funSoundEnabled { SoundPlayer.play() }
    }

    private func depletionMap(now: Date) -> [ServiceID: [Depletion]] {
        var result: [ServiceID: [Depletion]] = [:]
        for service in ServiceID.allCases {
            if let depletion = store.depletion(for: service, now: now) {
                result[service, default: []].append(depletion)
            }
            guard let statsStore,
                  let weekly = store.limits[service]?.first(where: { $0.kind == .weekly }),
                  let rate = DepletionEstimator.weeklyDailyRate(
                    snapshots: statsStore.percentSnapshots(service: service, kind: .weekly, days: 8, now: now)),
                  let depletion = DepletionEstimator.weeklyDepletion(
                    current: weekly.usedPercent, rate: rate, resetsAt: weekly.resetsAt, now: now),
                  depletion.willDepleteBeforeReset else { continue }
            result[service, default: []].append(depletion)
        }
        return result
    }

    private func eventKindEnabled(_ event: HUDEvent) -> Bool {
        switch event.kind {
        case .limitThreshold: settings.notifyLimitThreshold
        case .depletionRisk: settings.notifyDepletion
        case .windowReset: settings.notifyWindowReset
        case .burnSpike: settings.notifyBurnSpike
        case .briefing: settings.notifyBriefing
        case .comeback: settings.notifyComeback
        case .milestone: settings.funMilestone
        case .record: settings.funRecord
        case .update: settings.notifyUpdate
        }
    }

    private func evaluateComeback() {
        guard settings.notifyComeback,
              let gap = store.consumeComebackGap(), gap >= 3 * 3600 else { return }
        let event = HUDEvent(
            kind: .comeback,
            title: "欢迎回来",
            subtitle: "间隔 \(EventEngine.countdown(to: Date().addingTimeInterval(gap), from: Date())) 后继续工作",
            percent: nil)
        record(event)
        if settings.funSoundEnabled { SoundPlayer.play() }
    }

    private func persistStatsIfNeeded() {
        guard let statsStore, Date().timeIntervalSince(lastStatsWrite) >= 60 else { return }
        lastStatsWrite = Date()
        statsStore.upsert(events: store.events, calendar: .current)
        for (service, windows) in store.limits {
            for window in windows {
                statsStore.recordPercentSnapshot(service: service, kind: window.kind,
                                                 percent: window.usedPercent, day: Date())
            }
        }

        let today = store.todayTokens(now: Date())
        if let milestone = milestoneTracker.check(todayTokens: today, day: Date()), settings.funMilestone {
            record(HUDEvent(kind: .milestone, title: "里程碑达成", subtitle: "今日累计 \(formatTokens(milestone)) tokens", percent: nil))
            if settings.funSoundEnabled { SoundPlayer.play() }
        }
        if settings.funRecord {
            let day = Self.dayString(Date())
            if lastRecordDay != day,
               let previous = statsStore.maxDailyTokens(excludingDay: Date(), calendar: .current),
               previous > 0, today > previous {
                lastRecordDay = day
                record(HUDEvent(kind: .record, title: "今日创下新纪录", subtitle: "超过此前 \(formatTokens(previous)) tokens", percent: nil))
                if settings.funSoundEnabled { SoundPlayer.play() }
            }
        }
    }

    private func evaluateBriefingIfNeeded() {
        guard settings.notifyBriefing,
              Date().timeIntervalSince(lastBriefingEvaluation) >= 5 * 60 else { return }
        lastBriefingEvaluation = Date()
        briefingEngine.realMode = settings.realMode
        briefingEngine.customMessages = settings.customMessages
        let now = Date()
        let today = store.todayTokens(now: now)
        let calendar = Calendar.current
        let yesterdayEnd = calendar.startOfDay(for: now)
        let yesterdayStart = yesterdayEnd.addingTimeInterval(-24 * 3600)
        let yesterdayEvents = store.events.filter { $0.timestamp >= yesterdayStart && $0.timestamp < yesterdayEnd }
        let yesterdayTokens = yesterdayEvents.reduce(0) { $0 + $1.requestTokens }
        let todayStart = calendar.startOfDay(for: now)
        let todayEvents = store.events.filter { $0.timestamp >= todayStart }

        // Weekly fields are intentionally best-effort: UsageStore retains the
        // recent event tail, while SQLite fills in older days after the first
        // persistence pass.
        let local = Calendar.current
        let localToday = local.startOfDay(for: now)
        let lastWeekStart = local.date(byAdding: .day, value: -7, to: localToday) ?? localToday
        let previousWeekStart = local.date(byAdding: .day, value: -14, to: localToday) ?? lastWeekStart
        let daily = statsStore?.dailyTotals(days: 15, now: now, calendar: local) ?? []
        let lastWeekTokens = daily
            .filter { $0.day >= lastWeekStart && $0.day < localToday }
            .reduce(0) { $0 + $1.tokens }
        let prevWeekTokens = daily
            .filter { $0.day >= previousWeekStart && $0.day < lastWeekStart }
            .reduce(0) { $0 + $1.tokens }
        let lastWeekEvents = store.events.filter {
            $0.timestamp >= lastWeekStart && $0.timestamp < localToday
        }
        let yesterdayCost = CostEstimator.cost(of: yesterdayEvents)
        let todayCost = CostEstimator.cost(of: todayEvents)
        guard let event = briefingEngine.evaluate(now: now, data: .init(
            yesterdayTokens: yesterdayTokens,
            yesterdayCost: yesterdayCost,
            yesterdayTopProject: topProject(in: yesterdayEvents),
            todayTokens: today,
            todayCost: todayCost,
            todayTopService: topServiceToday(now: now),
            lastWeekTokens: settings.funWeeklyReport && lastWeekTokens > 0 ? lastWeekTokens : nil,
            lastWeekCost: settings.funWeeklyReport
                ? statsStore?.totalCost(from: lastWeekStart, to: localToday, calendar: local)
                : nil,
            prevWeekTokens: settings.funWeeklyReport && prevWeekTokens > 0 ? prevWeekTokens : nil,
            lastWeekTopProject: settings.funWeeklyReport ? topProject(in: lastWeekEvents) : nil,
            streakDays: settings.funStreak
                ? (statsStore?.streakDays(endingOn: now, calendar: local) ?? 0)
                : 0
        )) else { return }
        record(event)
        if settings.notificationsEnabled { notifier.notify(title: event.title, subtitle: event.subtitle) }
        if settings.funSoundEnabled { SoundPlayer.play() }
    }

    private func topProject(in events: [TokenEvent]) -> String? {
        let totals = events.reduce(into: [String: Int]()) { result, event in
            guard let project = event.project, !project.isEmpty else { return }
            result[project, default: 0] += event.requestTokens
        }
        return totals.max(by: { $0.value < $1.value })?.key
    }

    private func topServiceToday(now: Date) -> (service: ServiceID, share: Double)? {
        let start = Calendar.current.startOfDay(for: now)
        var totals: [ServiceID: Int] = [:]
        for event in store.events where event.timestamp >= start {
            totals[event.service, default: 0] += event.requestTokens
        }
        let total = totals.values.reduce(0, +)
        guard total > 0, let top = totals.max(by: { $0.value < $1.value }) else { return nil }
        return (top.key, Double(top.value) / Double(total))
    }

    private func record(_ event: HUDEvent) {
        eventLog.append(event)
        objectWillChange.send()
    }

    func checkForUpdates() async {
        guard let current = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String,
              let release = await AIGlassUpdateChecker.fetchLatest(),
              AIGlassUpdateChecker.isNewer(release.version, than: current) else { return }
        updateState.available = release
        if settings.notifyUpdate { notifier.notify(title: "MochiPort 有新版本", subtitle: "v\(release.version)") }
    }

    private static func homePath(_ suffix: String) -> URL {
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(suffix)
    }

    private static func dayString(_ date: Date) -> String {
        let formatter = DateFormatter()
        formatter.calendar = Calendar(identifier: .gregorian)
        formatter.timeZone = .current
        formatter.dateFormat = "yyyy-MM-dd"
        return formatter.string(from: date)
    }

    private func formatTokens(_ value: Int) -> String {
        switch value {
        case 1_000_000...: String(format: "%.1fM", Double(value) / 1_000_000)
        case 1_000...: String(format: "%.1fK", Double(value) / 1_000)
        default: String(value)
        }
    }
}
