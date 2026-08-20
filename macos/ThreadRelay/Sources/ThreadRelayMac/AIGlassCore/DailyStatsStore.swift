import Foundation
import SQLite3

/// SQLite 기반 일별 토큰 통계 영구 저장소.
///
/// (day, service, model, project) 단위로 집계해 `INSERT OR REPLACE`로 저장한다.
/// **주의: REPLACE는 누적이 아니라 대체**이므로, 호출자는 해당 일자의 전체 이벤트를
/// 넘겨야 멱등성이 유지된다 (`UsageStore.events`가 8일 보존이므로 매번 최근 8일 전체를
/// 넘기는 것이 안전).
@MainActor
public final class DailyStatsStore {
    // SQLite owns this pointer and all access is serialized on the main actor.
    // `nonisolated(unsafe)` is needed only for the final C-level close in the
    // Swift 6 nonisolated deinitializer.
    nonisolated(unsafe) private var db: OpaquePointer?

    // SQLite가 바인딩 문자열을 자체 복사하도록 강제하는 transient destructor.
    private static let SQLITE_TRANSIENT = unsafeBitCast(-1, to: sqlite3_destructor_type.self)

    // day 컬럼용 UTC "yyyy-MM-dd" 포맷터.
    private static let dayFormatter: DateFormatter = {
        let f = DateFormatter()
        f.calendar = Calendar(identifier: .gregorian)
        f.timeZone = TimeZone(identifier: "UTC")!
        f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = "yyyy-MM-dd"
        return f
    }()

    /// DB 파일을 열고(없으면 생성) 스키마를 보장한다. 실패 시 nil.
    public init?(path: String) {
        let dir = (path as NSString).deletingLastPathComponent
        if !dir.isEmpty {
            try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
        }
        guard sqlite3_open(path, &db) == SQLITE_OK else {
            sqlite3_close(db)
            return nil
        }
        let create = """
        CREATE TABLE IF NOT EXISTS daily_stats(
            day TEXT, service TEXT, model TEXT, project TEXT,
            input INTEGER, output INTEGER, cache_read INTEGER, cache_create INTEGER,
            PRIMARY KEY(day, service, model, project)
        )
        """
        guard sqlite3_exec(db, create, nil, nil, nil) == SQLITE_OK else {
            sqlite3_close(db)
            return nil
        }
        let createSnapshots = """
        CREATE TABLE IF NOT EXISTS percent_snapshots(
            day TEXT, service TEXT, kind TEXT, percent REAL,
            PRIMARY KEY(day, service, kind)
        )
        """
        guard sqlite3_exec(db, createSnapshots, nil, nil, nil) == SQLITE_OK else {
            sqlite3_close(db)
            return nil
        }
    }

    deinit {
        sqlite3_close(db)
    }

    private struct Key: Hashable {
        let day: String
        let service: String
        let model: String
        let project: String
    }

    private struct Agg {
        var input = 0, output = 0, cacheRead = 0, cacheCreate = 0
    }

    /// 이벤트를 (day, service, model, project)로 집계해 `INSERT OR REPLACE`한다.
    /// project가 nil이면 빈 문자열로 저장한다.
    public func upsert(events: [TokenEvent], calendar: Calendar = .current) {
        guard !events.isEmpty else { return }
        var grouped: [Key: Agg] = [:]
        // DateFormatter.string이 이벤트당 ~수 µs라 수만 이벤트 × 60초 persist마다
        // 메인 스레드를 수십 ms 막는다 — 시(epoch hour) 단위로 캐시한다
        // (UTC 날 경계는 시 경계에 정렬되므로 같은 hour는 같은 day 문자열).
        var dayCache: [Int: String] = [:]
        for e in events {
            let hour = Int(e.timestamp.timeIntervalSince1970.rounded(.down)) / 3600
            let day: String
            if let cached = dayCache[hour] {
                day = cached
            } else {
                day = Self.dayFormatter.string(from: e.timestamp)
                dayCache[hour] = day
            }
            let key = Key(day: day,
                          service: e.service.rawValue,
                          model: e.model,
                          project: e.project ?? "")
            var agg = grouped[key] ?? Agg()
            agg.input += e.inputTokens
            agg.output += e.outputTokens
            agg.cacheRead += e.cacheReadTokens
            agg.cacheCreate += e.cacheCreationTokens
            grouped[key] = agg
        }

        let sql = """
        INSERT OR REPLACE INTO daily_stats
        (day, service, model, project, input, output, cache_read, cache_create)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        """
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(stmt) }

        sqlite3_exec(db, "BEGIN", nil, nil, nil)
        for (key, agg) in grouped {
            sqlite3_bind_text(stmt, 1, key.day, -1, Self.SQLITE_TRANSIENT)
            sqlite3_bind_text(stmt, 2, key.service, -1, Self.SQLITE_TRANSIENT)
            sqlite3_bind_text(stmt, 3, key.model, -1, Self.SQLITE_TRANSIENT)
            sqlite3_bind_text(stmt, 4, key.project, -1, Self.SQLITE_TRANSIENT)
            sqlite3_bind_int64(stmt, 5, Int64(agg.input))
            sqlite3_bind_int64(stmt, 6, Int64(agg.output))
            sqlite3_bind_int64(stmt, 7, Int64(agg.cacheRead))
            sqlite3_bind_int64(stmt, 8, Int64(agg.cacheCreate))
            sqlite3_step(stmt)
            sqlite3_reset(stmt)
        }
        sqlite3_exec(db, "COMMIT", nil, nil, nil)
    }

    // 지정 days 범위의 시작일(UTC 문자열) 계산.
    private func cutoffDayString(days: Int, now: Date, calendar: Calendar) -> String {
        let start = calendar.date(byAdding: .day, value: -(days - 1), to: calendar.startOfDay(for: now)) ?? now
        return Self.dayFormatter.string(from: start)
    }

    /// 일별×서비스별 토큰 합계 (tokens = 4컬럼 합). 최근 `days`일.
    /// 반환 순서는 결정적: (day 오름차순, service는 ServiceID.allCases 고정 순).
    /// 차트 스택/시리즈가 렌더마다 뒤바뀌지 않도록 보장한다.
    public func dailyTotalsByService(days: Int, now: Date, calendar: Calendar = .current) -> [(day: Date, service: ServiceID, tokens: Int)] {
        let cutoff = cutoffDayString(days: days, now: now, calendar: calendar)
        let sql = """
        SELECT day, service, SUM(input + output + cache_read + cache_create)
        FROM daily_stats WHERE day >= ? AND service = 'codex'
        GROUP BY day, service ORDER BY day
        """
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return [] }
        defer { sqlite3_finalize(stmt) }
        sqlite3_bind_text(stmt, 1, cutoff, -1, Self.SQLITE_TRANSIENT)

        var result: [(day: Date, service: ServiceID, tokens: Int)] = []
        while sqlite3_step(stmt) == SQLITE_ROW {
            guard let dayC = sqlite3_column_text(stmt, 0),
                  let svcC = sqlite3_column_text(stmt, 1) else { continue }
            let dayStr = String(cString: dayC)
            let svcStr = String(cString: svcC)
            guard let day = Self.dayFormatter.date(from: dayStr),
                  let service = ServiceID(rawValue: svcStr) else { continue }
            let tokens = Int(sqlite3_column_int64(stmt, 2))
            result.append((day: day, service: service, tokens: tokens))
        }
        // SQL은 (day, service) 그룹의 service 순서를 보장하지 않으므로
        // (day 오름차순, service는 allCases 인덱스) 고정 순으로 정렬한다.
        let order = Dictionary(uniqueKeysWithValues: ServiceID.allCases.enumerated().map { ($1, $0) })
        return result.sorted { a, b in
            if a.day != b.day { return a.day < b.day }
            return (order[a.service] ?? 0) < (order[b.service] ?? 0)
        }
    }

    /// 일별 토큰 합계(서비스 합산). 최근 `days`일, day 오름차순.
    /// `services`가 주어지면 해당 서비스만 합산(잔디 히트맵의 enabled 필터용). nil이면 전체.
    /// 토큰이 0인 날은 행이 없으므로 생략된다(호출자가 빈 셀로 처리).
    public func dailyTotals(days: Int, now: Date, calendar: Calendar = .current,
                            services: Set<ServiceID>? = nil) -> [(day: Date, tokens: Int)] {
        let byService = dailyTotalsByService(days: days, now: now, calendar: calendar)
        var sums: [Date: Int] = [:]
        for row in byService {
            if let services, !services.contains(row.service) { continue }
            sums[row.day, default: 0] += row.tokens
        }
        return sums.map { (day: $0.key, tokens: $0.value) }.sorted { $0.day < $1.day }
    }

    /// project별 토큰 합계 (내림차순). 빈 프로젝트 문자열은 제외. 최근 `days`일.
    public func projectBreakdown(days: Int, now: Date, calendar: Calendar = .current) -> [(project: String, tokens: Int)] {
        let cutoff = cutoffDayString(days: days, now: now, calendar: calendar)
        let sql = """
        SELECT project, SUM(input + output + cache_read + cache_create)
        FROM daily_stats WHERE day >= ? AND service = 'codex' AND project <> ''
        GROUP BY project
        """
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return [] }
        defer { sqlite3_finalize(stmt) }
        sqlite3_bind_text(stmt, 1, cutoff, -1, Self.SQLITE_TRANSIENT)

        var result: [(project: String, tokens: Int)] = []
        while sqlite3_step(stmt) == SQLITE_ROW {
            guard let projC = sqlite3_column_text(stmt, 0) else { continue }
            let project = String(cString: projC)
            let tokens = Int(sqlite3_column_int64(stmt, 1))
            result.append((project: project, tokens: tokens))
        }
        return result.sorted { $0.tokens > $1.tokens }
    }

    /// 최근 `days`일의 추정 비용 합계(USD). model 컬럼 기준으로 CostEstimator 단가를 적용한다.
    public func totalCost(days: Int, now: Date, calendar: Calendar = .current) -> Double {
        let cutoff = cutoffDayString(days: days, now: now, calendar: calendar)
        let sql = """
        SELECT model, SUM(input), SUM(output), SUM(cache_read), SUM(cache_create)
        FROM daily_stats WHERE day >= ? AND service = 'codex'
        GROUP BY model
        """
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return 0 }
        defer { sqlite3_finalize(stmt) }
        sqlite3_bind_text(stmt, 1, cutoff, -1, Self.SQLITE_TRANSIENT)

        var total = 0.0
        while sqlite3_step(stmt) == SQLITE_ROW {
            guard let modelC = sqlite3_column_text(stmt, 0) else { continue }
            let model = String(cString: modelC)
            // 임시 이벤트로 단가 계산 위임 (service/timestamp/project는 비용에 무관).
            let synth = TokenEvent(service: .codex, timestamp: now, model: model,
                                   inputTokens: Int(sqlite3_column_int64(stmt, 1)),
                                   outputTokens: Int(sqlite3_column_int64(stmt, 2)),
                                   cacheReadTokens: Int(sqlite3_column_int64(stmt, 3)),
                                   cacheCreationTokens: Int(sqlite3_column_int64(stmt, 4)))
            total += CostEstimator.cost(of: synth)
        }
        return total
    }

    /// 명시적 일 범위 `[from, to)`(UTC day 기준, from 포함·to 제외)의 추정 비용 합계(USD).
    /// `totalCost(days:)`가 "최근 N일"만 지원해 주간 리포트의 지난주 [D-7, D-1] 비용을
    /// 정확히 못 구하던 문제를 해결한다(전전주 비용이 섞였음).
    public func totalCost(from: Date, to: Date, calendar: Calendar = .current) -> Double {
        let fromStr = Self.dayFormatter.string(from: from)
        let toStr = Self.dayFormatter.string(from: to)
        let sql = """
        SELECT model, SUM(input), SUM(output), SUM(cache_read), SUM(cache_create)
        FROM daily_stats WHERE day >= ? AND day < ? AND service = 'codex'
        GROUP BY model
        """
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return 0 }
        defer { sqlite3_finalize(stmt) }
        sqlite3_bind_text(stmt, 1, fromStr, -1, Self.SQLITE_TRANSIENT)
        sqlite3_bind_text(stmt, 2, toStr, -1, Self.SQLITE_TRANSIENT)

        var total = 0.0
        while sqlite3_step(stmt) == SQLITE_ROW {
            guard let modelC = sqlite3_column_text(stmt, 0) else { continue }
            let model = String(cString: modelC)
            let synth = TokenEvent(service: .codex, timestamp: from, model: model,
                                   inputTokens: Int(sqlite3_column_int64(stmt, 1)),
                                   outputTokens: Int(sqlite3_column_int64(stmt, 2)),
                                   cacheReadTokens: Int(sqlite3_column_int64(stmt, 3)),
                                   cacheCreationTokens: Int(sqlite3_column_int64(stmt, 4)))
            total += CostEstimator.cost(of: synth)
        }
        return total
    }

    // MARK: - 신기록 / 스트릭 (재미 로직)

    /// 일별 토큰 합(4컬럼 합)의 **최댓값**을 반환한다. `excludingDay`에 해당하는 날(UTC)은 제외.
    /// 다른 날이 하나도 없으면 nil (신기록 비교 기준이 없음).
    public func maxDailyTokens(excludingDay: Date, calendar: Calendar = .current) -> Int? {
        let excludeStr = Self.dayFormatter.string(from: excludingDay)
        let sql = """
        SELECT day, SUM(input + output + cache_read + cache_create) AS total
        FROM daily_stats WHERE day <> ? AND service = 'codex'
        GROUP BY day ORDER BY total DESC LIMIT 1
        """
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return nil }
        defer { sqlite3_finalize(stmt) }
        sqlite3_bind_text(stmt, 1, excludeStr, -1, Self.SQLITE_TRANSIENT)
        guard sqlite3_step(stmt) == SQLITE_ROW else { return nil }
        return Int(sqlite3_column_int64(stmt, 1))
    }

    /// `endingOn`(오늘)부터 거꾸로 **연속으로 토큰 > 0**인 일수.
    /// 오늘 토큰이 0이면 streak 0 (오늘 포함 기준). 중간 공백을 만나면 중단.
    public func streakDays(endingOn: Date, calendar: Calendar = .current) -> Int {
        // 토큰 > 0 인 날들의 day 문자열 집합.
        let sql = """
        SELECT day FROM daily_stats
        WHERE service = 'codex'
        GROUP BY day HAVING SUM(input + output + cache_read + cache_create) > 0
        """
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return 0 }
        defer { sqlite3_finalize(stmt) }
        var activeDays = Set<String>()
        while sqlite3_step(stmt) == SQLITE_ROW {
            guard let dayC = sqlite3_column_text(stmt, 0) else { continue }
            activeDays.insert(String(cString: dayC))
        }

        var count = 0
        var cursor = calendar.startOfDay(for: endingOn)
        while true {
            let dayStr = Self.dayFormatter.string(from: cursor)
            guard activeDays.contains(dayStr) else { break }
            count += 1
            guard let prev = calendar.date(byAdding: .day, value: -1, to: cursor) else { break }
            cursor = prev
        }
        return count
    }

    // MARK: - percent 스냅샷 (주간 일단위 소진 추정용)

    /// (day, service, kind)에 사용률(%) 스냅샷을 `INSERT OR REPLACE`로 기록한다.
    /// REPLACE이므로 같은 날 여러 번 호출하면 **마지막 관측값**만 남는다.
    /// `day`는 UTC "yyyy-MM-dd"로 정규화된다 (daily_stats와 동일 기준).
    public func recordPercentSnapshot(service: ServiceID, kind: LimitWindow.Kind, percent: Double, day: Date) {
        let dayStr = Self.dayFormatter.string(from: day)
        let sql = """
        INSERT OR REPLACE INTO percent_snapshots(day, service, kind, percent)
        VALUES (?, ?, ?, ?)
        """
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(stmt) }
        sqlite3_bind_text(stmt, 1, dayStr, -1, Self.SQLITE_TRANSIENT)
        sqlite3_bind_text(stmt, 2, service.rawValue, -1, Self.SQLITE_TRANSIENT)
        sqlite3_bind_text(stmt, 3, kind.rawValue, -1, Self.SQLITE_TRANSIENT)
        sqlite3_bind_double(stmt, 4, percent)
        sqlite3_step(stmt)
    }

    /// (service, kind)의 최근 `days`일 percent 스냅샷을 day 오름차순으로 반환한다.
    /// day는 UTC 자정 Date로 복원된다.
    public func percentSnapshots(service: ServiceID, kind: LimitWindow.Kind, days: Int,
                                 now: Date = Date(), calendar: Calendar = .utc) -> [(day: Date, percent: Double)] {
        let cutoff = cutoffDayString(days: days, now: now, calendar: calendar)
        let sql = """
        SELECT day, percent FROM percent_snapshots
        WHERE service = ? AND kind = ? AND day >= ?
        ORDER BY day
        """
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return [] }
        defer { sqlite3_finalize(stmt) }
        sqlite3_bind_text(stmt, 1, service.rawValue, -1, Self.SQLITE_TRANSIENT)
        sqlite3_bind_text(stmt, 2, kind.rawValue, -1, Self.SQLITE_TRANSIENT)
        sqlite3_bind_text(stmt, 3, cutoff, -1, Self.SQLITE_TRANSIENT)

        var result: [(day: Date, percent: Double)] = []
        while sqlite3_step(stmt) == SQLITE_ROW {
            guard let dayC = sqlite3_column_text(stmt, 0) else { continue }
            let dayStr = String(cString: dayC)
            guard let day = Self.dayFormatter.date(from: dayStr) else { continue }
            let percent = sqlite3_column_double(stmt, 1)
            result.append((day: day, percent: percent))
        }
        return result
    }
}
