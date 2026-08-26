import Foundation
import SQLite3

/// One complete daily aggregate produced by a verified Codex log scan.
/// `input` is already uncached input; cache columns are retained for cost and
/// context-volume diagnostics.
public struct DailyStatsRow: Hashable, Sendable {
    public let day: String
    public let service: String
    public let source: String
    public let model: String
    public let project: String
    public let input: Int
    public let output: Int
    public let cacheRead: Int
    public let cacheCreate: Int
    /// Sum of Codex's reported per-turn `total_tokens`. It cannot be rebuilt
    /// from component columns because compaction totals may differ.
    public let usageTotal: Int

    public init(day: String, service: String, source: String, model: String,
                project: String, input: Int, output: Int,
                cacheRead: Int, cacheCreate: Int, usageTotal: Int) {
        self.day = day
        self.service = service
        self.source = source
        self.model = model
        self.project = project
        self.input = input
        self.output = output
        self.cacheRead = cacheRead
        self.cacheCreate = cacheCreate
        self.usageTotal = usageTotal
    }
}

/// SQLite 기반 일별 토큰 통계 영구 저장소.
///
/// (day, service, source, model, project) 단위로 집계해 `INSERT OR REPLACE`로 저장한다.
/// **주의: REPLACE는 누적이 아니라 대체**이므로, 호출자는 해당 일자의 전체 이벤트를
/// 넘겨야 멱등성이 유지된다 (`UsageStore.events`가 8일 보존이므로 매번 최근 8일 전체를
/// 넘기는 것이 안전).
@MainActor
public final class DailyStatsStore {
    // SQLite owns this pointer and all access is serialized on the main actor.
    // `nonisolated(unsafe)` is needed only for the final C-level close in the
    // Swift 6 nonisolated deinitializer.
    nonisolated(unsafe) private var db: OpaquePointer?
    private var databasePath: String?
    // v4 switches Codex daily buckets from UTC to the user's local calendar
    // day. Existing rows must be rebuilt from raw logs rather than mixed with
    // the new boundary.
    private static let metricVersion = "4"
    private static let metricVersionKey = "codex_metric_version"
    private static let rebuildStateKey = "codex_rebuild_state"
    private static let rebuildAttemptKey = "codex_rebuild_attempt_at"
    private static let rebuildRetryAfterKey = "codex_rebuild_retry_after"
    private static let rebuildTargetVersionKey = "codex_rebuild_target_version"
    private static let rebuildRunningState = "running"
    private static let rebuildFailedState = "failed"
    private static let rebuildCompletedState = "completed"
    /// A full raw-log scan can outlive the GUI process. Persist a lease and
    /// avoid immediately starting the same expensive scan after every restart.
    static let codexRebuildRetryInterval: TimeInterval = 6 * 60 * 60
    /// Old rows are retained until a complete raw-log rebuild succeeds.
    public private(set) var needsCodexRebuild = false

    // SQLite가 바인딩 문자열을 자체 복사하도록 강제하는 transient destructor.
    private static let SQLITE_TRANSIENT = unsafeBitCast(-1, to: sqlite3_destructor_type.self)

    // day 컬럼용 로컬 자연일 "yyyy-MM-dd" 포맷터. AI Token Monitor는
    // timestamp를 현재 시스템 시간대로 변환한 뒤 날짜를 집계한다.
    private static let dayFormatter: DateFormatter = {
        let f = DateFormatter()
        f.calendar = Calendar(identifier: .gregorian)
        f.timeZone = .current
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
        databasePath = path
        let create = """
        CREATE TABLE IF NOT EXISTS daily_stats(
            day TEXT, service TEXT, source TEXT NOT NULL DEFAULT 'legacy',
            model TEXT, project TEXT,
            input INTEGER, output INTEGER, cache_read INTEGER, cache_create INTEGER,
            usage_total INTEGER,
            PRIMARY KEY(day, service, source, model, project)
        )
        """

        var tableExists = Self.tableExists(db, name: "daily_stats")
        // Recover a table left by an interrupted migration before deciding
        // that this is a brand-new database. SQLite normally rolls back the
        // transaction, but this guard also handles a manually copied DB.
        if !tableExists {
            for orphan in ["daily_stats_legacy", "daily_stats_migrating"]
                where Self.tableExists(db, name: orphan) {
                if sqlite3_exec(
                    db,
                    "ALTER TABLE \(orphan) RENAME TO daily_stats",
                    nil,
                    nil,
                    nil) == SQLITE_OK {
                    tableExists = true
                    break
                }
            }
        }
        let sourceKeyExists = tableExists && Self.hasSourcePrimaryKey(db)
        var didSchemaMigration = false
        if !tableExists {
            guard sqlite3_exec(db, create, nil, nil, nil) == SQLITE_OK else {
                sqlite3_close(db)
                return nil
            }
        } else if !sourceKeyExists {
            // Rebuild the table inside one SQLite transaction. Using a
            // separate temporary table is recoverable if the process is
            // interrupted; the old table remains intact until the final
            // DROP/RENAME sequence commits.
            let hasSourceColumn = Self.hasColumn(db, table: "daily_stats", name: "source")
            let sourceExpression = hasSourceColumn
                ? "COALESCE(NULLIF(TRIM(source), ''), 'legacy')"
                : "'legacy'"
            let usageTotalExpression = Self.hasColumn(
                db, table: "daily_stats", name: "usage_total")
                ? "usage_total"
                : "NULL"
            let migration = """
            BEGIN IMMEDIATE;
            DROP TABLE IF EXISTS daily_stats_migrating;
            CREATE TABLE daily_stats_migrating(
                day TEXT, service TEXT, source TEXT NOT NULL DEFAULT 'legacy',
                model TEXT, project TEXT,
                input INTEGER, output INTEGER, cache_read INTEGER, cache_create INTEGER,
                usage_total INTEGER,
                PRIMARY KEY(day, service, source, model, project)
            );
            INSERT INTO daily_stats_migrating
                (day, service, source, model, project, input, output, cache_read, cache_create,
                 usage_total)
            SELECT day, service, \(sourceExpression), model, project,
                   input, output, cache_read, cache_create, \(usageTotalExpression)
            FROM daily_stats;
            DROP TABLE daily_stats;
            ALTER TABLE daily_stats_migrating RENAME TO daily_stats;
            COMMIT;
            """
            guard sqlite3_exec(db, migration, nil, nil, nil) == SQLITE_OK else {
                sqlite3_close(db)
                return nil
            }
            needsCodexRebuild = true
            didSchemaMigration = true
        }

        // Metric v4 stores the reported total independently and uses local
        // calendar days. Existing rows
        // deliberately remain NULL until a verified raw-log rebuild; deriving
        // them from input/output would lose compaction totals.
        if !Self.hasColumn(db, table: "daily_stats", name: "usage_total") {
            guard sqlite3_exec(
                db,
                "ALTER TABLE daily_stats ADD COLUMN usage_total INTEGER",
                nil,
                nil,
                nil) == SQLITE_OK else {
                sqlite3_close(db)
                return nil
            }
            needsCodexRebuild = true
            didSchemaMigration = true
        }

        let createMeta = """
        CREATE TABLE IF NOT EXISTS stats_meta(
            key TEXT PRIMARY KEY, value TEXT NOT NULL
        )
        """
        guard sqlite3_exec(db, createMeta, nil, nil, nil) == SQLITE_OK else {
            sqlite3_close(db)
            return nil
        }
        if didSchemaMigration {
            needsCodexRebuild = true
        } else if let version = Self.readMeta(db, key: Self.metricVersionKey) {
            if version != Self.metricVersion { needsCodexRebuild = true }
        } else if tableExists && sourceKeyExists {
            // The previous source-aware build had no metric marker. Its rows
            // may still use the old context-volume unit, so rebuild once.
            needsCodexRebuild = true
        } else {
            // A brand-new database has no legacy rows to migrate.
            Self.writeMeta(db, key: Self.metricVersionKey, value: Self.metricVersion)
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

    private static func tableExists(_ db: OpaquePointer?, name: String) -> Bool {
        let sql = "SELECT 1 FROM sqlite_master WHERE type='table' AND name=? LIMIT 1"
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return false }
        defer { sqlite3_finalize(stmt) }
        sqlite3_bind_text(stmt, 1, name, -1, SQLITE_TRANSIENT)
        return sqlite3_step(stmt) == SQLITE_ROW
    }

    private static func hasColumn(_ db: OpaquePointer?, table: String, name: String) -> Bool {
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, "PRAGMA table_info(\(table))", -1, &stmt, nil) == SQLITE_OK else {
            return false
        }
        defer { sqlite3_finalize(stmt) }
        while sqlite3_step(stmt) == SQLITE_ROW {
            if let value = sqlite3_column_text(stmt, 1), String(cString: value) == name {
                return true
            }
        }
        return false
    }

    private static func hasSourcePrimaryKey(_ db: OpaquePointer?) -> Bool {
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, "PRAGMA table_info(daily_stats)", -1, &stmt, nil) == SQLITE_OK else {
            return false
        }
        defer { sqlite3_finalize(stmt) }
        while sqlite3_step(stmt) == SQLITE_ROW {
            guard let value = sqlite3_column_text(stmt, 1),
                  String(cString: value) == "source" else { continue }
            return sqlite3_column_int(stmt, 5) > 0
        }
        return false
    }

    private static func readMeta(_ db: OpaquePointer?, key: String) -> String? {
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, "SELECT value FROM stats_meta WHERE key = ?", -1, &stmt, nil) == SQLITE_OK else {
            return nil
        }
        defer { sqlite3_finalize(stmt) }
        sqlite3_bind_text(stmt, 1, key, -1, SQLITE_TRANSIENT)
        guard sqlite3_step(stmt) == SQLITE_ROW, let value = sqlite3_column_text(stmt, 0) else { return nil }
        return String(cString: value)
    }

    private static func readMetaDate(_ db: OpaquePointer?, key: String) -> Date? {
        guard let raw = readMeta(db, key: key),
              let seconds = Double(raw),
              seconds.isFinite
        else { return nil }
        return Date(timeIntervalSince1970: seconds)
    }

    @discardableResult
    private static func writeMeta(_ db: OpaquePointer?, key: String, value: String) -> Bool {
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(
            db,
            "INSERT OR REPLACE INTO stats_meta(key, value) VALUES (?, ?)",
            -1,
            &stmt,
            nil) == SQLITE_OK else { return false }
        defer { sqlite3_finalize(stmt) }
        sqlite3_bind_text(stmt, 1, key, -1, SQLITE_TRANSIENT)
        sqlite3_bind_text(stmt, 2, value, -1, SQLITE_TRANSIENT)
        return sqlite3_step(stmt) == SQLITE_DONE
    }

    @discardableResult
    private static func deleteMeta(_ db: OpaquePointer?, key: String) -> Bool {
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(
            db,
            "DELETE FROM stats_meta WHERE key = ?",
            -1,
            &stmt,
            nil) == SQLITE_OK else { return false }
        defer { sqlite3_finalize(stmt) }
        sqlite3_bind_text(stmt, 1, key, -1, SQLITE_TRANSIENT)
        return sqlite3_step(stmt) == SQLITE_DONE
    }

    deinit {
        sqlite3_close(db)
    }

    /// Claims the one-time Codex history rebuild for this process.
    ///
    /// The claim is persisted in the stats metadata so a GUI restart does not
    /// start another multi-gigabyte scan immediately after an interrupted one.
    /// A stale running claim is retried after the same cooldown used for failed
    /// scans; a successful rebuild clears the claim transactionally.
    @discardableResult
    public func beginCodexRebuildIfAllowed(now: Date = Date()) -> Bool {
        guard needsCodexRebuild else { return false }

        let state = Self.readMeta(db, key: Self.rebuildStateKey)
        // A lease left by an older metric cannot block the v4 rebuild after a
        // process restart; it was scanning for a different aggregate shape.
        let sameTarget = Self.readMeta(db, key: Self.rebuildTargetVersionKey)
            == Self.metricVersion
        if sameTarget {
            switch state {
            case Self.rebuildRunningState:
                guard let attemptAt = Self.readMetaDate(db, key: Self.rebuildAttemptKey) else {
                    return false
                }
                guard now.timeIntervalSince(attemptAt) >= Self.codexRebuildRetryInterval else {
                    return false
                }
            case Self.rebuildFailedState:
                if let retryAfter = Self.readMetaDate(db, key: Self.rebuildRetryAfterKey),
                   now < retryAfter {
                    return false
                }
            default:
                break
            }
        }

        guard Self.writeMeta(db, key: Self.rebuildStateKey, value: Self.rebuildRunningState),
              Self.writeMeta(
                db,
                key: Self.rebuildTargetVersionKey,
                value: Self.metricVersion
              ),
              Self.writeMeta(
                db,
                key: Self.rebuildAttemptKey,
                value: String(now.timeIntervalSince1970)
              )
        else {
            return false
        }
        _ = Self.deleteMeta(db, key: Self.rebuildRetryAfterKey)
        return true
    }

    /// Records an interrupted or failed scan without deleting existing rows.
    public func markCodexRebuildFailed(now: Date = Date()) {
        guard needsCodexRebuild else { return }
        _ = Self.writeMeta(db, key: Self.rebuildStateKey, value: Self.rebuildFailedState)
        _ = Self.writeMeta(
            db,
            key: Self.rebuildRetryAfterKey,
            value: String(now.addingTimeInterval(Self.codexRebuildRetryInterval).timeIntervalSince1970)
        )
    }

    private struct Key: Hashable {
        let day: String
        let service: String
        let source: String
        let model: String
        let project: String
    }

    private struct Agg {
        var input = 0, output = 0, cacheRead = 0, cacheCreate = 0, usageTotal = 0
    }

    /// 이벤트를 (day, service, source, model, project)로 집계해 `INSERT OR REPLACE`한다.
    /// project가 nil이면 빈 문자열로 저장한다.
    public func upsert(events: [TokenEvent], calendar: Calendar = .current) {
        // Until the one-time raw-log rebuild completes, writing the bounded
        // event tail beside legacy rows would double-count the same day.
        guard !events.isEmpty, !needsCodexRebuild else { return }
        var grouped: [Key: Agg] = [:]
        // DateFormatter.string이 이벤트당 ~수 µs라 수만 이벤트 × 60초 persist마다
        // 메인 스레드를 수십 ms 막는다 — 시(epoch hour) 단위로 캐시한다
        // (로컬 날 경계는 시 경계에 정렬되므로 같은 hour는 같은 day 문자열).
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
                          source: e.source,
                          model: e.model,
                          project: e.project ?? "")
            var agg = grouped[key] ?? Agg()
            agg.input += e.inputTokens
            agg.output += e.outputTokens
            agg.cacheRead += e.cacheReadTokens
            agg.cacheCreate += e.cacheCreationTokens
            agg.usageTotal += e.reportedTotalTokens
            grouped[key] = agg
        }

        let sql = """
        INSERT OR REPLACE INTO daily_stats
        (day, service, source, model, project, input, output, cache_read, cache_create,
         usage_total)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(stmt) }

        sqlite3_exec(db, "BEGIN", nil, nil, nil)
        for (key, agg) in grouped {
            sqlite3_bind_text(stmt, 1, key.day, -1, Self.SQLITE_TRANSIENT)
            sqlite3_bind_text(stmt, 2, key.service, -1, Self.SQLITE_TRANSIENT)
            sqlite3_bind_text(stmt, 3, key.source, -1, Self.SQLITE_TRANSIENT)
            sqlite3_bind_text(stmt, 4, key.model, -1, Self.SQLITE_TRANSIENT)
            sqlite3_bind_text(stmt, 5, key.project, -1, Self.SQLITE_TRANSIENT)
            sqlite3_bind_int64(stmt, 6, Int64(agg.input))
            sqlite3_bind_int64(stmt, 7, Int64(agg.output))
            sqlite3_bind_int64(stmt, 8, Int64(agg.cacheRead))
            sqlite3_bind_int64(stmt, 9, Int64(agg.cacheCreate))
            sqlite3_bind_int64(stmt, 10, Int64(agg.usageTotal))
            sqlite3_step(stmt)
            sqlite3_reset(stmt)
        }
        sqlite3_exec(db, "COMMIT", nil, nil, nil)
    }

    /// Replace every Codex row after a verified complete scan of the raw
    /// session logs. This is the only operation allowed to remove legacy
    /// rows, so a partial or failed scan cannot silently undercount history.
    @discardableResult
    public func rebuildCodexStats(rows: [DailyStatsRow], databaseBackupURL: URL? = nil) -> Bool {
        guard let db, let databasePath else { return false }
        if let databaseBackupURL,
           !FileManager.default.fileExists(atPath: databaseBackupURL.path) {
            do {
                try FileManager.default.copyItem(
                    at: URL(fileURLWithPath: databasePath),
                    to: databaseBackupURL)
            } catch {
                return false
            }
        }

        let insert = """
        INSERT OR REPLACE INTO daily_stats
        (day, service, source, model, project, input, output, cache_read, cache_create,
         usage_total)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, insert, -1, &stmt, nil) == SQLITE_OK else { return false }
        defer { sqlite3_finalize(stmt) }

        guard sqlite3_exec(db, "BEGIN IMMEDIATE", nil, nil, nil) == SQLITE_OK else { return false }
        let deleteOK = sqlite3_exec(
            db,
            "DELETE FROM daily_stats WHERE service = 'codex'",
            nil,
            nil,
            nil) == SQLITE_OK
        guard deleteOK else {
            sqlite3_exec(db, "ROLLBACK", nil, nil, nil)
            return false
        }
        for row in rows where row.service == ServiceID.codex.rawValue {
            sqlite3_bind_text(stmt, 1, row.day, -1, Self.SQLITE_TRANSIENT)
            sqlite3_bind_text(stmt, 2, row.service, -1, Self.SQLITE_TRANSIENT)
            sqlite3_bind_text(stmt, 3, row.source, -1, Self.SQLITE_TRANSIENT)
            sqlite3_bind_text(stmt, 4, row.model, -1, Self.SQLITE_TRANSIENT)
            sqlite3_bind_text(stmt, 5, row.project, -1, Self.SQLITE_TRANSIENT)
            sqlite3_bind_int64(stmt, 6, Int64(row.input))
            sqlite3_bind_int64(stmt, 7, Int64(row.output))
            sqlite3_bind_int64(stmt, 8, Int64(row.cacheRead))
            sqlite3_bind_int64(stmt, 9, Int64(row.cacheCreate))
            sqlite3_bind_int64(stmt, 10, Int64(row.usageTotal))
            guard sqlite3_step(stmt) == SQLITE_DONE else {
                sqlite3_reset(stmt)
                sqlite3_exec(db, "ROLLBACK", nil, nil, nil)
                return false
            }
            sqlite3_reset(stmt)
        }
        guard Self.writeMeta(db, key: Self.metricVersionKey, value: Self.metricVersion),
              Self.writeMeta(db, key: Self.rebuildStateKey, value: Self.rebuildCompletedState),
              Self.deleteMeta(db, key: Self.rebuildAttemptKey),
              Self.deleteMeta(db, key: Self.rebuildRetryAfterKey),
              Self.deleteMeta(db, key: Self.rebuildTargetVersionKey)
        else {
            sqlite3_exec(db, "ROLLBACK", nil, nil, nil)
            return false
        }
        guard sqlite3_exec(db, "COMMIT", nil, nil, nil) == SQLITE_OK else {
            sqlite3_exec(db, "ROLLBACK", nil, nil, nil)
            return false
        }
        needsCodexRebuild = false
        return true
    }

    /// Stable backup path used by the automatic one-time migration.
    public func defaultRebuildBackupURL() -> URL? {
        guard let path = databasePath else { return nil }
        return URL(fileURLWithPath: path + ".pre-reported-total-v4.bak")
    }

    // 지정 days 범위의 시작일(로컬 자연일 문자열) 계산.
    private func cutoffDayString(days: Int, now: Date, calendar: Calendar) -> String {
        let start = calendar.date(byAdding: .day, value: -(days - 1), to: calendar.startOfDay(for: now)) ?? now
        return Self.dayFormatter.string(from: start)
    }

    /// 일별×서비스별 Codex reported-total 합계. 최근 `days`일.
    /// cache_read/cache_create는 스키마에 보존되며 비용 계산에는 계속 사용된다.
    /// 반환 순서는 결정적: (day 오름차순, service는 ServiceID.allCases 고정 순).
    /// 차트 스택/시리즈가 렌더마다 뒤바뀌지 않도록 보장한다.
    public func dailyTotalsByService(days: Int, now: Date, calendar: Calendar = .current,
                                     source: String? = nil) -> [(day: Date, service: ServiceID, tokens: Int)] {
        let cutoff = cutoffDayString(days: days, now: now, calendar: calendar)
        let sql = """
        SELECT day, service, SUM(usage_total)
        FROM daily_stats WHERE day >= ? AND service = 'codex'
          AND (? IS NULL OR source = ?)
        GROUP BY day, service ORDER BY day
        """
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return [] }
        defer { sqlite3_finalize(stmt) }
        sqlite3_bind_text(stmt, 1, cutoff, -1, Self.SQLITE_TRANSIENT)
        if let source {
            sqlite3_bind_text(stmt, 2, source, -1, Self.SQLITE_TRANSIENT)
            sqlite3_bind_text(stmt, 3, source, -1, Self.SQLITE_TRANSIENT)
        } else {
            sqlite3_bind_null(stmt, 2)
            sqlite3_bind_null(stmt, 3)
        }

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
                            services: Set<ServiceID>? = nil,
                            source: String? = nil) -> [(day: Date, tokens: Int)] {
        let byService = dailyTotalsByService(days: days, now: now, calendar: calendar, source: source)
        var sums: [Date: Int] = [:]
        for row in byService {
            if let services, !services.contains(row.service) { continue }
            sums[row.day, default: 0] += row.tokens
        }
        return sums.map { (day: $0.key, tokens: $0.value) }.sorted { $0.day < $1.day }
    }

    /// project별 reported-total 합계 (내림차순). 빈 프로젝트 문자열은 제외. 최근 `days`일.
    public func projectBreakdown(days: Int, now: Date, calendar: Calendar = .current,
                                 source: String? = nil) -> [(project: String, tokens: Int)] {
        let cutoff = cutoffDayString(days: days, now: now, calendar: calendar)
        let sql = """
        SELECT project, SUM(usage_total)
        FROM daily_stats WHERE day >= ? AND service = 'codex' AND project <> ''
          AND (? IS NULL OR source = ?)
        GROUP BY project
        """
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return [] }
        defer { sqlite3_finalize(stmt) }
        sqlite3_bind_text(stmt, 1, cutoff, -1, Self.SQLITE_TRANSIENT)
        if let source {
            sqlite3_bind_text(stmt, 2, source, -1, Self.SQLITE_TRANSIENT)
            sqlite3_bind_text(stmt, 3, source, -1, Self.SQLITE_TRANSIENT)
        } else {
            sqlite3_bind_null(stmt, 2)
            sqlite3_bind_null(stmt, 3)
        }

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
    public func totalCost(days: Int, now: Date, calendar: Calendar = .current,
                          source: String? = nil) -> Double {
        let cutoff = cutoffDayString(days: days, now: now, calendar: calendar)
        let sql = """
        SELECT model, SUM(input), SUM(output), SUM(cache_read), SUM(cache_create)
        FROM daily_stats WHERE day >= ? AND service = 'codex'
          AND (? IS NULL OR source = ?)
        GROUP BY model
        """
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return 0 }
        defer { sqlite3_finalize(stmt) }
        sqlite3_bind_text(stmt, 1, cutoff, -1, Self.SQLITE_TRANSIENT)
        if let source {
            sqlite3_bind_text(stmt, 2, source, -1, Self.SQLITE_TRANSIENT)
            sqlite3_bind_text(stmt, 3, source, -1, Self.SQLITE_TRANSIENT)
        } else {
            sqlite3_bind_null(stmt, 2)
            sqlite3_bind_null(stmt, 3)
        }

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

    /// 명시적 일 범위 `[from, to)`(로컬 자연일 기준, from 포함·to 제외)의 추정 비용 합계(USD).
    /// `totalCost(days:)`가 "최근 N일"만 지원해 주간 리포트의 지난주 [D-7, D-1] 비용을
    /// 정확히 못 구하던 문제를 해결한다(전전주 비용이 섞였음).
    public func totalCost(from: Date, to: Date, calendar: Calendar = .current,
                          source: String? = nil) -> Double {
        let fromStr = Self.dayFormatter.string(from: from)
        let toStr = Self.dayFormatter.string(from: to)
        let sql = """
        SELECT model, SUM(input), SUM(output), SUM(cache_read), SUM(cache_create)
        FROM daily_stats WHERE day >= ? AND day < ? AND service = 'codex'
          AND (? IS NULL OR source = ?)
        GROUP BY model
        """
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return 0 }
        defer { sqlite3_finalize(stmt) }
        sqlite3_bind_text(stmt, 1, fromStr, -1, Self.SQLITE_TRANSIENT)
        sqlite3_bind_text(stmt, 2, toStr, -1, Self.SQLITE_TRANSIENT)
        if let source {
            sqlite3_bind_text(stmt, 3, source, -1, Self.SQLITE_TRANSIENT)
            sqlite3_bind_text(stmt, 4, source, -1, Self.SQLITE_TRANSIENT)
        } else {
            sqlite3_bind_null(stmt, 3)
            sqlite3_bind_null(stmt, 4)
        }

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

    /// 일별 reported-total 합의 **최댓값**을 반환한다.
    /// `excludingDay`에 해당하는 로컬 날짜는 제외.
    /// 다른 날이 하나도 없으면 nil (신기록 비교 기준이 없음).
    public func maxDailyTokens(excludingDay: Date, calendar: Calendar = .current,
                               source: String? = nil) -> Int? {
        let excludeStr = Self.dayFormatter.string(from: excludingDay)
        let sql = """
        SELECT day, SUM(usage_total) AS total
        FROM daily_stats WHERE day <> ? AND service = 'codex'
          AND (? IS NULL OR source = ?)
        GROUP BY day ORDER BY total DESC LIMIT 1
        """
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return nil }
        defer { sqlite3_finalize(stmt) }
        sqlite3_bind_text(stmt, 1, excludeStr, -1, Self.SQLITE_TRANSIENT)
        if let source {
            sqlite3_bind_text(stmt, 2, source, -1, Self.SQLITE_TRANSIENT)
            sqlite3_bind_text(stmt, 3, source, -1, Self.SQLITE_TRANSIENT)
        } else {
            sqlite3_bind_null(stmt, 2)
            sqlite3_bind_null(stmt, 3)
        }
        guard sqlite3_step(stmt) == SQLITE_ROW else { return nil }
        return Int(sqlite3_column_int64(stmt, 1))
    }

    /// `endingOn`(오늘)부터 거꾸로 **연속으로 요청 토큰 > 0**인 일수.
    /// 오늘 토큰이 0이면 streak 0 (오늘 포함 기준). 중간 공백을 만나면 중단.
    public func streakDays(endingOn: Date, calendar: Calendar = .current,
                           source: String? = nil) -> Int {
        // 토큰 > 0 인 날들의 day 문자열 집합.
        let sql = """
        SELECT day FROM daily_stats
        WHERE service = 'codex'
          AND (? IS NULL OR source = ?)
        GROUP BY day HAVING SUM(usage_total) > 0
        """
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return 0 }
        defer { sqlite3_finalize(stmt) }
        if let source {
            sqlite3_bind_text(stmt, 1, source, -1, Self.SQLITE_TRANSIENT)
            sqlite3_bind_text(stmt, 2, source, -1, Self.SQLITE_TRANSIENT)
        } else {
            sqlite3_bind_null(stmt, 1)
            sqlite3_bind_null(stmt, 2)
        }
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
    /// `day`는 로컬 "yyyy-MM-dd"로 정규화된다 (daily_stats와 동일 기준).
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
    /// day는 로컬 자정 Date로 복원된다.
    public func percentSnapshots(service: ServiceID, kind: LimitWindow.Kind, days: Int,
                                 now: Date = Date(), calendar: Calendar = .current) -> [(day: Date, percent: Double)] {
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
