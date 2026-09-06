import Foundation
import Observation

@MainActor
@Observable
public final class UsageStore {
    public private(set) var limits: [ServiceID: [LimitWindow]] = [:]
    public private(set) var events: [TokenEvent] = []
    public private(set) var lastActivityAt: Date?
    private var dedupEventIndexes: [String: Int] = [:]
    /// 分钟粒度 Token 合计缓存（key = epoch秒 / 60）。用于 burn rate 计算，按服务分开。
    private var minuteBuckets: [ServiceID: [Int: Int]] = [:]
    /// 每服务最近一次事件 timestamp 的缓存——让 approxFullReset 不用全量扫描 events（对比 30fps 调用）。
    private var lastEventTimestamp: [ServiceID: Date] = [:]

    /// 每服务·每窗口类型的使用率（%）时间序列。在 `setLimits` 中 append，只保留最近 60 分钟。
    /// 作为耗尽预测（DepletionEstimator）的输入。
    public private(set) var percentHistory: [ServiceID: [LimitWindow.Kind: [(date: Date, percent: Double)]]] = [:]

    /// 事件保留期（天）。更早的事件在 ingest 时忽略。
    public static let retentionDays = 8

    /// 首次 addEvents（初始加载）是否完成。为 true 之前不记录回归空白。
    private var hasLoadedInitialBatch = false
    /// 至今通过 addEvents 添加的事件中的最大 timestamp。
    private var lastKnownMaxTimestamp: Date?
    /// 下次 consumeComebackGap() 调用要返回的空白（秒）。nil = 未检测到。
    private var pendingComebackGap: TimeInterval?

    public init() {}

    public func setLimits(_ windows: [LimitWindow], for service: ServiceID) {
        setLimits(windows, for: service, at: Date())
    }

    /// 测试用：可注入样本记录时间的变体。生产用 `setLimits(_:for:)`。
    func setLimits(_ windows: [LimitWindow], for service: ServiceID, at sampleDate: Date) {
        limits[service] = windows
        let trimCutoff = sampleDate.addingTimeInterval(-60 * 60)
        for window in windows {
            var series = percentHistory[service]?[window.kind] ?? []
            series.append((date: sampleDate, percent: window.usedPercent))
            series.removeAll { $0.date < trimCutoff }
            percentHistory[service, default: [:]][window.kind] = series
        }
    }

    /// 用服务的 **session5h 窗口** 时间序列估计耗尽（分钟级斜率）。
    /// 用户语义：5h 只在**自己的会话重置之前会用完**时才警告——
    /// 因此只在该窗口的 resetsAt 基础上 `willDepleteBeforeReset` 时返回 non-nil。
    /// （'重置后才耗尽'属于噪声，不显示。）
    /// 无 session5h 窗口或无法估计时返回 nil。（周窗口由 App 层用 statsStore 计算。）
    public func depletion(for service: ServiceID, now: Date) -> Depletion? {
        guard let windows = limits[service],
              let session = windows.first(where: { $0.kind == .session5h }) else { return nil }
        guard let series = percentHistory[service]?[.session5h] else { return nil }
        let samples = series.map { ($0.date, $0.percent) }
        guard let d = DepletionEstimator.estimate(samples: samples, resetsAt: session.resetsAt,
                                                  now: now, kind: .session5h) else { return nil }
        return d.willDepleteBeforeReset ? d : nil
    }

    public func addEvents(_ batch: [(event: TokenEvent, dedupKey: String?)]) {
        // 收集器只读最近 8 天的文件，此截止用于防御文件内的旧行
        let cutoff = Date().addingTimeInterval(-Double(Self.retentionDays) * 24 * 3600)
        var added = false
        // 回归检测必须以**实际新增的**事件为准——被去重丢弃的旧事件
        // （rotation 重解析等）混入 batchMin 会让 gap 变负导致不触发。
        var addedMin: Date?
        var addedMax: Date?
        for (event, key) in batch {
            guard event.timestamp >= cutoff else { continue }
            if let key {
                if let existingIndex = dedupEventIndexes[key] {
                    let existing = events[existingIndex]
                    let calendar = Calendar.current
                    if calendar.startOfDay(for: event.timestamp)
                        < calendar.startOfDay(for: existing.timestamp) {
                        replaceReplay(at: existingIndex, with: event)
                    }
                    continue
                }
                dedupEventIndexes[key] = events.count
            }
            events.append(event)
            let minute = Int(event.timestamp.timeIntervalSince1970) / 60
            // Burn rate/activity tracks request tokens. Cache reads are kept
            // on the event, but are not repeatedly counted as new usage.
            minuteBuckets[event.service, default: [:]][minute, default: 0] += event.requestTokens
            // 更新每服务最新 timestamp 缓存（approxFullReset 用）
            if let prev = lastEventTimestamp[event.service] {
                if event.timestamp > prev { lastEventTimestamp[event.service] = event.timestamp }
            } else {
                lastEventTimestamp[event.service] = event.timestamp
            }
            if addedMin == nil || event.timestamp < addedMin! { addedMin = event.timestamp }
            if addedMax == nil || event.timestamp > addedMax! { addedMax = event.timestamp }
            added = true
        }
        if added {
            lastActivityAt = Date()
        // 清除 48h 前的分钟桶（长期运行内存卫生；baseline 只用 24h）
        // 截止基准：批次内最新事件 timestamp → 保证测试可注入
            if let newest = batch.map(\.event.timestamp).max() {
                let cutoffMinute = Int(newest.timeIntervalSince1970) / 60 - 48 * 60
                for svc in minuteBuckets.keys {
                    minuteBuckets[svc] = minuteBuckets[svc]!.filter { $0.key > cutoffMinute }
                }
            }

        // 回归检测：比较上一个最新 timestamp 与本次实际新增的最小 timestamp
            if let prevMax = lastKnownMaxTimestamp, let addedMin {
                let gap = addedMin.timeIntervalSince(prevMax)
                if !hasLoadedInitialBatch {
                    // 首次加载完成——不记录空白，只立标志
                    hasLoadedInitialBatch = true
                } else if gap >= 3 * 3600 {
                    pendingComebackGap = gap
                }
            } else if !hasLoadedInitialBatch {
                hasLoadedInitialBatch = true
            }

            // 更新最新时间戳（按实际新增部分）
            if let addedMax, lastKnownMaxTimestamp == nil || addedMax > lastKnownMaxTimestamp! {
                lastKnownMaxTimestamp = addedMax
            }
        }
    }

    /// Replayed turns belong to the earliest local date on which Codex logged
    /// them. This path is rare and intentionally recomputes timestamp caches
    /// after moving one already-counted event.
    private func replaceReplay(at index: Int, with replacement: TokenEvent) {
        let existing = events[index]
        let oldMinute = Int(existing.timestamp.timeIntervalSince1970) / 60
        let newMinute = Int(replacement.timestamp.timeIntervalSince1970) / 60

        if let value = minuteBuckets[existing.service]?[oldMinute] {
            let next = value - existing.requestTokens
            if next > 0 {
                minuteBuckets[existing.service]?[oldMinute] = next
            } else {
                minuteBuckets[existing.service]?.removeValue(forKey: oldMinute)
            }
        }
        events[index] = replacement
        minuteBuckets[replacement.service, default: [:]][newMinute, default: 0]
            += replacement.requestTokens

        for service in Set([existing.service, replacement.service]) {
            lastEventTimestamp[service] = events
                .lazy
                .filter { $0.service == service }
                .map(\.timestamp)
                .max()
        }
        lastKnownMaxTimestamp = events.map(\.timestamp).max()
    }

    /// 返回检测到的回归空白并清除。没有则 nil。
    public func consumeComebackGap() -> TimeInterval? {
        defer { pendingComebackGap = nil }
        return pendingComebackGap
    }

    public var maxUsedPercent: Double {
        limits.values.flatMap { $0 }.map(\.usedPercent).max() ?? 0
    }

    /// 限定服务集合的最大使用率（%）。用于菜单栏·辉光（排除未开启的代理）。
    public func maxUsedPercent(in services: Set<ServiceID>) -> Double {
        limits.filter { services.contains($0.key) }
            .values.flatMap { $0 }.map(\.usedPercent).max() ?? 0
    }

    /// timestamp → 该 calendar 的午夜。以小时（epoch hour）为单位缓存——
    /// 日边界总是与小时边界对齐，因此同一 epoch hour 是同一天。
    /// `Calendar.startOfDay` 每次事件约 1µs，数万事件全量扫描会吃掉几十毫秒
    /// （UI body 每次求值都会调用），用缓存把次数降到 distinct hour 的数量级。
    private static func dayBucketer(calendar: Calendar) -> (Date) -> Date {
        var cache: [Int: Date] = [:]
        return { date in
            let hour = Int(date.timeIntervalSince1970.rounded(.down)) / 3600
            if let cached = cache[hour] { return cached }
            let day = calendar.startOfDay(for: date)
            cache[hour] = day
            return day
        }
    }

    public func dailyTotals(days: Int, now: Date, calendar: Calendar = .current) -> [(day: Date, tokens: Int)] {
        let today = calendar.startOfDay(for: now)
        let dayOf = Self.dayBucketer(calendar: calendar)
        var buckets: [Date: Int] = [:]
        for e in events {
            buckets[dayOf(e.timestamp), default: 0] += e.requestTokens
        }
        return (0..<days).reversed().map { offset in
            let day = calendar.date(byAdding: .day, value: -offset, to: today)!
            return (day, buckets[day] ?? 0)
        }
    }

    public func todayTokens(now: Date, calendar: Calendar = .current) -> Int {
        let start = calendar.startOfDay(for: now)
        return events.filter { $0.timestamp >= start }.reduce(0) { $0 + $1.requestTokens }
    }


    /// 全服务合计的 tokens/min（保持原有签名）。
    public func tokensPerMinute(windowMinutes: Int, now: Date) -> Double {
        guard windowMinutes > 0 else { return 0 }
        let nowMinute = Int(now.timeIntervalSince1970) / 60
        var total = 0
        for svcBuckets in minuteBuckets.values {
            for minute in (nowMinute - windowMinutes + 1)...nowMinute {
                total += svcBuckets[minute] ?? 0
            }
        }
        return Double(total) / Double(windowMinutes)
    }

    /// 指定服务的 tokens/min。
    public func tokensPerMinute(service: ServiceID, windowMinutes: Int, now: Date) -> Double {
        guard windowMinutes > 0 else { return 0 }
        let nowMinute = Int(now.timeIntervalSince1970) / 60
        guard let svcBuckets = minuteBuckets[service] else { return 0 }
        var total = 0
        for minute in (nowMinute - windowMinutes + 1)...nowMinute {
            total += svcBuckets[minute] ?? 0
        }
        return Double(total) / Double(windowMinutes)
    }

    /// 按 project·服务汇总的 Token 合计（total 降序）。排除 project == nil 的事件。
    public func projectServiceBreakdown(days: Int, now: Date, calendar: Calendar = .current)
        -> [(project: String, byService: [ServiceID: Int], total: Int)] {
        let cutoff = calendar.date(byAdding: .day, value: -days, to: now)!
        var byProject: [String: [ServiceID: Int]] = [:]
        for e in events where e.timestamp >= cutoff {
            guard let proj = e.project else { continue }
            byProject[proj, default: [:]][e.service, default: 0] += e.requestTokens
        }
        return byProject.map { (project, byService) in
            (project: project, byService: byService, total: byService.values.reduce(0, +))
        }.sorted { $0.total > $1.total }
    }

    /// 每日×每服务的 Token 合计。只返回有活动的组合（tokens > 0）。
    /// 返回顺序是确定的：（day 升序，service 按 ServiceID.allCases 固定顺序）。
    /// 保证图表堆叠/系列在每次渲染中不会被打乱。
    public func dailyTotalsByService(days: Int, now: Date, calendar: Calendar = .current) -> [(day: Date, service: ServiceID, tokens: Int)] {
        let today = calendar.startOfDay(for: now)
        let dayOf = Self.dayBucketer(calendar: calendar)
        var buckets: [Date: [ServiceID: Int]] = [:]
        for e in events {
            buckets[dayOf(e.timestamp), default: [:]][e.service, default: 0] += e.requestTokens
        }
        var result: [(day: Date, service: ServiceID, tokens: Int)] = []
        for offset in (0..<days).reversed() {
            let day = calendar.date(byAdding: .day, value: -offset, to: today)!
            guard let svcMap = buckets[day] else { continue }
            // 服务按 allCases 固定顺序（消除字典遍历的不确定性）。
            for svc in ServiceID.allCases {
                let tokens = svcMap[svc] ?? 0
                if tokens > 0 {
                    result.append((day: day, service: svc, tokens: tokens))
                }
            }
        }
        return result
    }

    /// 生成一个会话（from~to）的摘要字符串。
    /// 格式：`"上一会话：{tokens} tokens · 主要项目 {project} · 约 ${cost}"`。
    /// 没有项目时省略该节，估算成本低于 $0.01 时省略成本节。
    /// 期间内该服务没有事件时返回 nil。
    public func sessionSummary(service: ServiceID, from: Date, to: Date) -> String? {
        let scoped = events.filter {
            $0.service == service && $0.timestamp >= from && $0.timestamp <= to
        }
        guard !scoped.isEmpty else { return nil }

        let tokens = scoped.reduce(0) { $0 + $1.requestTokens }
        var parts = ["上一会话：\(Self.formatTokens(tokens)) tokens"]

        // 最多的项目
        var byProject: [String: Int] = [:]
        for e in scoped { if let p = e.project { byProject[p, default: 0] += e.requestTokens } }
        if let top = byProject.max(by: { $0.value < $1.value })?.key {
            parts.append("主要项目 \(top)")
        }

        let cost = CostEstimator.cost(of: scoped)
        if cost >= 0.01 {
            parts.append(String(format: "~$%.2f", cost))
        }
        return parts.joined(separator: " · ")
    }

    /// 把 Token 数缩写为 K/M 单位。
    static func formatTokens(_ n: Int) -> String {
        switch n {
        case 1_000_000...: return String(format: "%.1fM", Double(n) / 1_000_000)
        case 1_000...: return String(format: "%.1fK", Double(n) / 1_000)
        default: return "\(n)"
        }
    }

    /// 最近 24h 内有活动的分钟桶的平均 tokens/min（burn spike 的基准线）
    public func activeBaselineRate(now: Date) -> Double {
        let nowMinute = Int(now.timeIntervalSince1970) / 60
        let cutoffMinute = nowMinute - 24 * 60
        var active: [Int: Int] = [:]
        for svcBuckets in minuteBuckets.values {
            for (minute, tokens) in svcBuckets where minute > cutoffMinute && minute <= nowMinute {
                active[minute, default: 0] += tokens
            }
        }
        guard !active.isEmpty else { return 0 }
        return Double(active.values.reduce(0, +)) / Double(active.count)
    }


    /// 用服务最近一次事件时间 + 窗口长度返回近似重置时刻。
    /// - session5h：最后一次事件 + 300 分钟
    /// - weekly：最后一次事件 + 7 天
    /// - daily：nil（沿用现有 resetsAt）
    /// 计算结果早于 now（已重置）时返回 nil。没有事件时返回 nil。
    public func approxFullReset(service: ServiceID, kind: LimitWindow.Kind, now: Date) -> Date? {
        guard kind != .daily else { return nil }
        // O(1) 缓存查询，避免 UI 刷新时扫描全部事件。
        guard let latest = lastEventTimestamp[service] else { return nil }
        let interval: TimeInterval
        switch kind {
        case .session5h: interval = 300 * 60
        case .weekly:    interval = 7 * 24 * 3600
        case .daily:     return nil
        }
        let candidate = latest.addingTimeInterval(interval)
        return candidate > now ? candidate : nil
    }
}
