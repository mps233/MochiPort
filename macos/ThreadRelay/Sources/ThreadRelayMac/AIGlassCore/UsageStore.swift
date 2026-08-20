import Foundation
import Observation

@MainActor
@Observable
public final class UsageStore {
    public private(set) var limits: [ServiceID: [LimitWindow]] = [:]
    public private(set) var events: [TokenEvent] = []
    public private(set) var lastActivityAt: Date?
    private var dedupKeys: Set<String> = []
    /// 분 단위 토큰 합계 캐시 (key = epoch초 / 60). burn rate 계산용. 서비스별로 분리.
    private var minuteBuckets: [ServiceID: [Int: Int]] = [:]
    /// 分钟级请求数缓存，用于最近活动占比计算。
    private var requestBuckets: [ServiceID: [Int: Int]] = [:]
    /// 서비스별 가장 최근 이벤트 timestamp 캐시 — approxFullReset이 events 전체 스캔 없이 사용 (30fps 호출 대비).
    private var lastEventTimestamp: [ServiceID: Date] = [:]

    /// 서비스·윈도우종류별 사용률(%) 시계열. `setLimits`에서 append되며 최근 60분만 유지.
    /// 소진 예측(DepletionEstimator)의 입력으로 사용.
    public private(set) var percentHistory: [ServiceID: [LimitWindow.Kind: [(date: Date, percent: Double)]]] = [:]

    /// 이벤트 보존 기간 (일). 이보다 오래된 이벤트는 ingest 시 무시.
    public static let retentionDays = 8

    /// 첫 addEvents(초기 로드) 완료 여부. true가 되기 전에는 컴백 공백을 기록하지 않는다.
    private var hasLoadedInitialBatch = false
    /// 지금까지 addEvents로 추가된 이벤트 중 최대 timestamp.
    private var lastKnownMaxTimestamp: Date?
    /// 다음 consumeComebackGap() 호출 시 반환할 공백(초). nil이면 미감지.
    private var pendingComebackGap: TimeInterval?

    public init() {}

    public func setLimits(_ windows: [LimitWindow], for service: ServiceID) {
        setLimits(windows, for: service, at: Date())
    }

    /// 테스트용: 샘플 기록 시각을 주입할 수 있는 변형. 프로덕션은 `setLimits(_:for:)` 사용.
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

    /// 서비스의 **session5h 윈도우** 시계열로 소진을 추정한다 (분 단위 기울기).
    /// 사용자 의미론: 5h는 **자기 세션 리셋 전에 다 쓸 때만** 경고한다 —
    /// 따라서 그 윈도우 resetsAt 기준 `willDepleteBeforeReset`일 때만 non-nil을 반환한다.
    /// ('리셋 후 소진 예정'은 노이즈이므로 표시하지 않는다.)
    /// session5h 윈도우가 없거나 추정 불가 시 nil. (주간은 App 레벨에서 statsStore로 계산.)
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
        // 수집기는 최근 8일 파일만 읽지만, 파일 내 오래된 라인 방어용 컷오프
        let cutoff = Date().addingTimeInterval(-Double(Self.retentionDays) * 24 * 3600)
        var added = false
        // 컴백 감지는 **실제 추가된** 이벤트 기준이어야 한다 — dedup으로 버려진 옛 이벤트
        // (rotation 재파싱 등)가 batchMin에 섞이면 gap이 음수가 되어 미발화한다.
        var addedMin: Date?
        var addedMax: Date?
        for (event, key) in batch {
            guard event.timestamp >= cutoff else { continue }
            if let key {
                if dedupKeys.contains(key) { continue }
                dedupKeys.insert(key)
            }
            events.append(event)
            let minute = Int(event.timestamp.timeIntervalSince1970) / 60
            minuteBuckets[event.service, default: [:]][minute, default: 0] += event.totalTokens
            requestBuckets[event.service, default: [:]][minute, default: 0] += 1
            // 서비스별 최신 timestamp 캐시 갱신 (approxFullReset용)
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
            // 48h 이전 분 버킷 제거 (장기 실행 메모리 hygiene; baseline은 24h만 사용)
            // 컷오프 기준: 배치 내 가장 최신 이벤트 timestamp → 테스트 주입성 보장
            if let newest = batch.map(\.event.timestamp).max() {
                let cutoffMinute = Int(newest.timeIntervalSince1970) / 60 - 48 * 60
                for svc in minuteBuckets.keys {
                    minuteBuckets[svc] = minuteBuckets[svc]!.filter { $0.key > cutoffMinute }
                }
                for svc in requestBuckets.keys {
                    requestBuckets[svc] = requestBuckets[svc]!.filter { $0.key > cutoffMinute }
                }
            }

            // 컴백 감지: 직전 최신 timestamp와 이번에 실제 추가된 최소 timestamp 비교
            if let prevMax = lastKnownMaxTimestamp, let addedMin {
                let gap = addedMin.timeIntervalSince(prevMax)
                if !hasLoadedInitialBatch {
                    // 첫 로드 완료 — 공백 기록 없이 플래그만 세움
                    hasLoadedInitialBatch = true
                } else if gap >= 3 * 3600 {
                    pendingComebackGap = gap
                }
            } else if !hasLoadedInitialBatch {
                hasLoadedInitialBatch = true
            }

            // 최신 타임스탬프 갱신 (실제 추가분 기준)
            if let addedMax, lastKnownMaxTimestamp == nil || addedMax > lastKnownMaxTimestamp! {
                lastKnownMaxTimestamp = addedMax
            }
        }
    }

    /// 감지된 컴백 공백을 반환하고 클리어한다. 없으면 nil.
    public func consumeComebackGap() -> TimeInterval? {
        defer { pendingComebackGap = nil }
        return pendingComebackGap
    }

    public var maxUsedPercent: Double {
        limits.values.flatMap { $0 }.map(\.usedPercent).max() ?? 0
    }

    /// 주어진 서비스 집합 한정 최대 사용률(%). 메뉴바·글로우용 (꺼진 에이전트 제외).
    public func maxUsedPercent(in services: Set<ServiceID>) -> Double {
        limits.filter { services.contains($0.key) }
            .values.flatMap { $0 }.map(\.usedPercent).max() ?? 0
    }

    /// 타임스탬프 → 해당 calendar의 자정. 시(epoch hour) 단위 캐시 —
    /// 날 경계는 항상 시 경계에 정렬되므로 같은 epoch hour는 같은 날이다.
    /// `Calendar.startOfDay`가 이벤트당 ~1µs라 수만 이벤트 풀스캔 시 수십 ms를 먹는데
    /// (UI body 평가마다 호출됨), 캐시로 distinct hour 수만큼으로 줄인다.
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
            buckets[dayOf(e.timestamp), default: 0] += e.totalTokens
        }
        return (0..<days).reversed().map { offset in
            let day = calendar.date(byAdding: .day, value: -offset, to: today)!
            return (day, buckets[day] ?? 0)
        }
    }

    public func modelBreakdown(days: Int, now: Date, calendar: Calendar = .current) -> [(model: String, tokens: Int)] {
        let cutoff = calendar.date(byAdding: .day, value: -days, to: now)!
        var byModel: [String: Int] = [:]
        for e in events where e.timestamp >= cutoff {
            byModel[e.model, default: 0] += e.totalTokens
        }
        return byModel.map { (model: $0.key, tokens: $0.value) }.sorted { $0.tokens > $1.tokens }
    }

    public func todayTokens(now: Date, calendar: Calendar = .current) -> Int {
        let start = calendar.startOfDay(for: now)
        return events.filter { $0.timestamp >= start }.reduce(0) { $0 + $1.totalTokens }
    }

    public func todayRequests(now: Date, calendar: Calendar = .current) -> Int {
        let start = calendar.startOfDay(for: now)
        let tokenReqs = events.filter { $0.timestamp >= start }.count
        return tokenReqs
    }

    /// 전 서비스 합산 tokens/min (기존 시그니처 유지).
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

    /// 특정 서비스의 tokens/min.
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

    /// 최근 windowMinutes 내 서비스별 토큰 비중 (합 1.0). 활동 없으면 빈 dict.
    public func recentShare(windowMinutes: Int = 3, now: Date) -> [ServiceID: Double] {
        guard windowMinutes > 0 else { return [:] }
        let nowMinute = Int(now.timeIntervalSince1970) / 60
        var totals: [ServiceID: Int] = [:]
        for (svc, svcBuckets) in minuteBuckets {
            var sum = 0
            for minute in (nowMinute - windowMinutes + 1)...nowMinute {
                sum += svcBuckets[minute] ?? 0
            }
            if sum > 0 { totals[svc] = sum }
        }
        let grand = totals.values.reduce(0, +)
        guard grand > 0 else { return [:] }
        return totals.mapValues { Double($0) / Double(grand) }
    }

    /// 最近 windowMinutes 内各服务的请求活动占比；没有 token 的 CLI 也可纳入统计。
    public func recentActivityShare(windowMinutes: Int = 3, now: Date) -> [ServiceID: Double] {
        let totals = recentRequestTotals(windowMinutes: windowMinutes, now: now)
        let grand = totals.values.reduce(0, +)
        guard grand > 0 else { return [:] }
        return totals.mapValues { Double($0) / Double(grand) }
    }

    /// 根据最近请求数计算活动强度，供菜单栏活动指标使用。
    public func requestActivityLevel(windowMinutes: Int = 3, now: Date) -> Double {
        let count = recentRequestTotals(windowMinutes: windowMinutes, now: now).values.reduce(0, +)
        guard windowMinutes > 0, count > 0 else { return 0 }
        return min(1.0, Double(count) / Double(windowMinutes))
    }

    private func recentRequestTotals(windowMinutes: Int, now: Date) -> [ServiceID: Int] {
        guard windowMinutes > 0 else { return [:] }
        let nowMinute = Int(now.timeIntervalSince1970) / 60
        let range = (nowMinute - windowMinutes + 1)...nowMinute
        var totals: [ServiceID: Int] = [:]
        for (svc, buckets) in requestBuckets {
            var count = 0
            for minute in range { count += buckets[minute] ?? 0 }
            if count > 0 { totals[svc, default: 0] += count }
        }
        return totals
    }

    /// project별 토큰 합계 (내림차순). project == nil 이벤트는 제외.
    public func projectBreakdown(days: Int, now: Date, calendar: Calendar = .current) -> [(project: String, tokens: Int)] {
        let cutoff = calendar.date(byAdding: .day, value: -days, to: now)!
        var byProject: [String: Int] = [:]
        for e in events where e.timestamp >= cutoff {
            guard let proj = e.project else { continue }
            byProject[proj, default: 0] += e.totalTokens
        }
        return byProject.map { (project: $0.key, tokens: $0.value) }.sorted { $0.tokens > $1.tokens }
    }

    /// project별·서비스별 토큰 합계 (total 내림차순). project == nil 이벤트는 제외.
    public func projectServiceBreakdown(days: Int, now: Date, calendar: Calendar = .current)
        -> [(project: String, byService: [ServiceID: Int], total: Int)] {
        let cutoff = calendar.date(byAdding: .day, value: -days, to: now)!
        var byProject: [String: [ServiceID: Int]] = [:]
        for e in events where e.timestamp >= cutoff {
            guard let proj = e.project else { continue }
            byProject[proj, default: [:]][e.service, default: 0] += e.totalTokens
        }
        return byProject.map { (project, byService) in
            (project: project, byService: byService, total: byService.values.reduce(0, +))
        }.sorted { $0.total > $1.total }
    }

    /// 일별×서비스별 토큰 합계. 활동이 있는 조합만 반환 (tokens > 0).
    /// 반환 순서는 결정적: (day 오름차순, service는 ServiceID.allCases 고정 순).
    /// 차트 스택/시리즈가 렌더마다 뒤바뀌지 않도록 보장한다.
    public func dailyTotalsByService(days: Int, now: Date, calendar: Calendar = .current) -> [(day: Date, service: ServiceID, tokens: Int)] {
        let today = calendar.startOfDay(for: now)
        let dayOf = Self.dayBucketer(calendar: calendar)
        var buckets: [Date: [ServiceID: Int]] = [:]
        for e in events {
            buckets[dayOf(e.timestamp), default: [:]][e.service, default: 0] += e.totalTokens
        }
        var result: [(day: Date, service: ServiceID, tokens: Int)] = []
        for offset in (0..<days).reversed() {
            let day = calendar.date(byAdding: .day, value: -offset, to: today)!
            guard let svcMap = buckets[day] else { continue }
            // 서비스는 allCases 고정 순으로 (dictionary 순회 비결정성 제거).
            for svc in ServiceID.allCases {
                let tokens = svcMap[svc] ?? 0
                if tokens > 0 {
                    result.append((day: day, service: svc, tokens: tokens))
                }
            }
        }
        return result
    }

    /// 한 세션(from~to)의 요약 문자열을 만든다.
    /// 格式：`"上一会话：{tokens} tokens · 主要项目 {project} · 约 ${cost}"`。
    /// 프로젝트가 없으면 그 절 생략, 추정 비용이 $0.01 미만이면 비용 절 생략.
    /// 기간 내 해당 서비스 이벤트가 없으면 nil.
    public func sessionSummary(service: ServiceID, from: Date, to: Date) -> String? {
        let scoped = events.filter {
            $0.service == service && $0.timestamp >= from && $0.timestamp <= to
        }
        guard !scoped.isEmpty else { return nil }

        let tokens = scoped.reduce(0) { $0 + $1.totalTokens }
        var parts = ["上一会话：\(Self.formatTokens(tokens)) tokens"]

        // 최다 프로젝트
        var byProject: [String: Int] = [:]
        for e in scoped { if let p = e.project { byProject[p, default: 0] += e.totalTokens } }
        if let top = byProject.max(by: { $0.value < $1.value })?.key {
            parts.append("主要项目 \(top)")
        }

        let cost = CostEstimator.cost(of: scoped)
        if cost >= 0.01 {
            parts.append(String(format: "~$%.2f", cost))
        }
        return parts.joined(separator: " · ")
    }

    /// 토큰 수를 K/M 단위로 축약한다.
    static func formatTokens(_ n: Int) -> String {
        switch n {
        case 1_000_000...: return String(format: "%.1fM", Double(n) / 1_000_000)
        case 1_000...: return String(format: "%.1fK", Double(n) / 1_000)
        default: return "\(n)"
        }
    }

    /// 최근 24h 중 활동이 있던 분 단위 버킷들의 평균 tokens/min (burn spike 기저선)
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

    /// 펄스 웨이브 진폭 (0...1). 100k tokens/min에서 최대.
    /// 3분 창: 30fps 파형의 jitter와 반응성 균형
    public func activityLevel(now: Date) -> Double {
        min(1.0, tokensPerMinute(windowMinutes: 3, now: now) / 100_000.0)
    }

    /// 서비스의 가장 최근 이벤트 시각 + 윈도우 길이로 근사 리셋 시각을 반환한다.
    /// - session5h: 마지막 이벤트 + 300분
    /// - weekly: 마지막 이벤트 + 7일
    /// - daily: nil (기존 resetsAt 사용)
    /// 계산 결과가 now 이전이면(이미 리셋됨) nil. 이벤트 없으면 nil.
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
