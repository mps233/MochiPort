import AppKit
import Combine
import Foundation

/// GUI-local usage coordinator. It deliberately has no HUD or daemon
/// lifecycle responsibilities: it reads local client logs, updates the
/// dashboard stores, and turns usage milestones into history/notifications.
@MainActor
final class MenubarCoordinator: ObservableObject {
    let store = UsageStore()
    let settings = AppSettings()
    let eventLog = EventLog()
    let eventEngine = EventEngine()
    let notifier = Notifier()
    let updateState = UpdateState()
    let milestoneTracker = MilestoneTracker()
    let briefingEngine = BriefingEngine()
    let statsStore: DailyStatsStore?

    /// Used only to rebuild the history database from the daemon instead of
    /// parsing every rollout log a second time. `APIClient()` resolves the
    /// daemon address and management credentials on its own, so the HUD keeps
    /// its offline behaviour: when the daemon is unreachable the rebuild falls
    /// back to the local collector below.
    private let apiClient = APIClient()

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
    /// Minutes derived by the daemon, applied to `UsageStore` on a low-frequency
    /// pass. Kept separate from the push-based local pipeline so a slow or absent
    /// daemon never delays the HUD.
    private var daemonMinutesTask: Task<Void, Never>?
    /// Cross-product rows from the daemon, applied on the next persistence pass.
    private var daemonBreakdown: [DailyStatsRow] = []
    /// When the daemon was last polled, used to space out pulls.
    private var lastDaemonPullAt = Date.distantPast

    init() {
        // 历史用量数据库并入 MochiPort 目录：旧 AIGlass 路径仅作一次性搬迁来源，
        // 保证迁移后趋势和 Codex 数值连续。
        let support = Self.homePath("Library/Application Support")
        let newPath = support.appendingPathComponent("MochiPort/stats.db")
        let legacyPath = support.appendingPathComponent("AIGlass/stats.db")
        if !FileManager.default.fileExists(atPath: newPath.path),
           FileManager.default.fileExists(atPath: legacyPath.path) {
            try? FileManager.default.copyItem(atPath: legacyPath.path, toPath: newPath.path)
        }
        statsStore = DailyStatsStore(path: newPath.path)
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

    /// Fetch the latest release and, if it is newer, surface it.
    ///
    /// The settings toggle gates the whole check, not just the notification:
    /// with it off the app makes no network request for updates at all.
    func checkForUpdates() async {
        guard settings.notifyUpdate else { return }
        guard let current = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String,
              let release = await ReleaseChecker.fetchLatest(),
              ReleaseChecker.isNewer(release.version, than: current) else { return }
        updateState.available = release
        notifier.notify(title: "MochiPort 有新版本", subtitle: "v\(release.version)")
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
        refreshDaemonMinutesIfNeeded()
        evaluateComeback()
        evaluateUsageEvents()
        persistStatsIfNeeded()
        evaluateBriefingIfNeeded()
        // The menu-bar label reads UsageStore through this coordinator. Keep
        // the small label in sync without adding a high-frequency timer.
        objectWillChange.send()
    }

    /// Pulls the daemon's minute buckets and hands them to `UsageStore`.
    ///
    /// Runs at most one request at a time and swallows every failure: the HUD is
    /// push-driven from the local pipeline, and this pass only exists to make the
    /// daemon authoritative for the minute-level totals behind the live rates.
    /// Minimum spacing between daemon pulls.
    ///
    /// `refresh()` is driven by a file watcher that can fire many times a second
    /// while Codex writes a session, and this request returns the full history.
    /// Without a floor, a busy session would keep a request permanently in flight.
    private static let daemonPullInterval: TimeInterval = 30
    /// Window requested by the periodic refresh, matching the history database's
    /// 105-day retention.
    private static let daemonRefreshDays = 105

    private func refreshDaemonMinutesIfNeeded() {
        guard daemonMinutesTask == nil else { return }
        let now = Date()
        guard now.timeIntervalSince(lastDaemonPullAt) >= Self.daemonPullInterval else { return }
        lastDaemonPullAt = now
        let client = apiClient
        daemonMinutesTask = Task { [weak self] in
            defer { self?.daemonMinutesTask = nil }
            // Each series is applied independently. Bailing out when any one of
            // them is empty would silently drop the others, and an empty series is
            // a normal response (a quiet hour, or no quota data on this machine).
            // A refresh only needs the window the history database keeps; a
            // rebuild (below) is the path that asks for everything.
            guard let history = try? await client.usageHistory(days: Self.daemonRefreshDays) else {
                // The daemon is unreachable (not running, or still the pre-usage
                // build). Hand persistence back to the local event pipeline:
                // holding the last successful breakdown forever would keep
                // replacing those days with a frozen snapshot and stop new events
                // from ever being recorded.
                self?.releaseDaemonBreakdown()
                return
            }
            guard !Task.isCancelled, let self else { return }
            // The daemon answered, so its rows are a complete statement about the
            // days they cover — including when it has none. Keeping a previous
            // snapshot here would freeze those days for the same reason a failed
            // refresh would.
            if history.breakdown.isEmpty {
                self.releaseDaemonBreakdown()
            } else {
                self.cacheDaemonBreakdown(history.breakdown)
            }
            var totals: [Int: Int] = [:]
            totals.reserveCapacity(history.minutes.count)
            for bucket in history.minutes {
                // The daemon reports absolute minutes; the store keys them the
                // same way, so no rebasing is needed.
                totals[Int(bucket.minute)] = Int(clamping: bucket.totals.totalTokens)
            }
            if !totals.isEmpty {
                self.store.replaceMinuteBuckets(totals, for: .codex)
            }

            // The minute-level quota series drives short-horizon depletion, so it
            // is replaced from the same response rather than accumulated locally.
            let samples = history.quotaMinutes.compactMap { point -> (
                kind: LimitWindow.Kind, date: Date, percent: Double
            )? in
                guard let kind = Self.quotaKind(point.kind) else { return nil }
                return (
                    kind: kind,
                    date: Date(timeIntervalSince1970: Double(point.minute) * 60),
                    percent: point.usedPercent
                )
            }
            self.store.replacePercentHistory(samples, for: .codex)

            // Quota windows travel with the same response, so they are applied
            // here rather than in a second request.
            if let quota = history.quota {
                self.store.applyQuotaWindows(
                    primary: quota.primary.map(Self.quotaWindow),
                    secondary: quota.secondary.map(Self.quotaWindow),
                    for: .codex
                )
            }
        }
    }

    /// Holds the daemon's most recent cross-product rows for the persistence pass.
    ///
    /// `persistStatsIfNeeded` runs synchronously on the main actor while the
    /// daemon request is async, so the rows are staged here rather than fetched
    /// from inside it.
    private func cacheDaemonBreakdown(_ rows: [UsageBreakdownRow]) {
        guard !rows.isEmpty else { return }
        daemonBreakdown = rows.map { row in
            DailyStatsRow(
                day: row.day,
                service: row.service,
                source: row.source,
                model: row.model,
                project: row.project,
                input: Int(clamping: row.totals.inputTokens),
                output: Int(clamping: row.totals.outputTokens),
                cacheRead: Int(clamping: row.totals.cachedInputTokens),
                cacheCreate: Int(clamping: row.totals.cacheWriteInputTokens),
                usageTotal: Int(clamping: row.totals.totalTokens)
            )
        }
    }

    /// Gives persistence back to the local event pipeline.
    ///
    /// Called when a daemon refresh fails. The staged rows only describe the
    /// daemon's last successful moment; continuing to apply them would both
    /// freeze those days and keep `upsert(events:)` from ever running, so new
    /// activity would stop being recorded at all.
    private func releaseDaemonBreakdown() {
        guard !daemonBreakdown.isEmpty else { return }
        daemonBreakdown = []
    }

    /// Maps the daemon's window name onto the client's window kind.
    ///
    /// Only the two kinds the dashboard renders are recognised; anything else is
    /// skipped rather than forced into the nearest bucket.
    private static func quotaKind(_ raw: String) -> LimitWindow.Kind? {
        switch raw {
        case "primary": return .session5h
        case "secondary": return .weekly
        default: return nil
        }
    }

    /// Converts a generated contract window into the shape `UsageStore` takes.
    private static func quotaWindow(
        _ window: UsageQuotaWindow
    ) -> (usedPercent: Double, windowMinutes: UInt32?, resetsAt: Date?) {
        (
            usedPercent: window.usedPercent,
            windowMinutes: window.windowMinutes,
            resetsAt: window.resetsAtMs.map { Date(timeIntervalSince1970: Double($0) / 1000) }
        )
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
            guard let self else { return }
            // Prefer the daemon: it already parses these same logs, so using it
            // keeps one implementation instead of two. The local collector stays
            // as the fallback because a failed rebuild parks the database behind
            // a multi-hour retry window while new events stop being recorded.
            let client = self.apiClient
            let fromDaemon = await Task.detached(priority: .background) {
                // No window: a rebuild replaces the database and must see every
                // day on disk.
                (try? await client.usageHistory())
                    .flatMap { DailyStatsRow.rows(fromDaemonBreakdown: $0.breakdown) }
            }.value

            let rows: [DailyStatsRow]?
            if let fromDaemon, !fromDaemon.isEmpty {
                rows = fromDaemon
            } else {
                rows = await Task.detached(priority: .background) {
                    CodexCollector.historicalRows(roots: roots)
                }.value
            }
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
        // Every enabled event in this pass is delivered. One evaluation can
        // produce several (for example a threshold crossing and a burn spike in
        // the same tick); taking only the first silently dropped the rest.
        for event in events where eventKindEnabled(event) {
            record(event)
            if settings.notificationsEnabled { notifier.notify(title: event.title, subtitle: event.subtitle) }
            if settings.funSoundEnabled { SoundPlayer.play() }
        }
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
        if !daemonBreakdown.isEmpty {
            // The daemon re-reads the same logs, so its rows are a complete
            // statement about the days they cover and replace what is stored.
            // Local event increments stop while this path is active; a failed or
            // empty daemon refresh clears the staged rows so they resume.
            statsStore.replaceCodexDays(rows: daemonBreakdown)
        } else {
            statsStore.upsert(events: store.events, calendar: .current)
        }
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
        let todayStart = calendar.startOfDay(for: now)

        // Yesterday is a closed day, so it comes from the persisted history the
        // daemon feeds rather than from the local event tail — the tail is
        // bounded and would under-report a long session that started earlier.
        //
        // The local tail is still the fallback, like the cost and project lookups
        // below. Before the one-time rebuild finishes the history table is empty
        // (or holds only other days), and reporting 0 would show a briefed
        // yesterday of zero tokens even though the events are right here.
        let yesterdayEvents = store.events.filter {
            $0.timestamp >= yesterdayStart && $0.timestamp < yesterdayEnd
        }
        let yesterdayTokens = statsStore?.dailyTotals(days: 2, now: now, calendar: calendar)
            .first { $0.day >= yesterdayStart && $0.day < yesterdayEnd }?
            .tokens
            ?? yesterdayEvents.reduce(0) { $0 + $1.reportedTotalTokens }
        let yesterdayCost = statsStore?.totalCost(from: yesterdayStart, to: yesterdayEnd, calendar: calendar)
            ?? CostEstimator.cost(of: yesterdayEvents)
        let yesterdayTopProject = statsStore?.topProject(from: yesterdayStart, to: yesterdayEnd, calendar: calendar)
            ?? topProject(in: yesterdayEvents)
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
        // `yesterdayCost` was resolved above from the persisted history.
        let todayCost = CostEstimator.cost(of: todayEvents)
        guard let event = briefingEngine.evaluate(now: now, data: .init(
            yesterdayTokens: yesterdayTokens,
            yesterdayCost: yesterdayCost,
            yesterdayTopProject: yesterdayTopProject,
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
