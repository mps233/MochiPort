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

/// 基于 SQLite 的每日 Token 统计持久化存储。
///
/// 以 (day, service, source, model, project) 为单位聚合，用 `INSERT OR REPLACE` 写入。
/// **注意：REPLACE 是替换而非累加**，因此调用方必须传入该日期的
/// 全部事件才能保持幂等（`UsageStore.events` 保留 8 天，所以每次传入最近 8 天
/// 的全量最安全）。
@MainActor
public final class DailyStatsStore {
    // SQLite owns this pointer and all access is serialized on the main actor.
    // `nonisolated(unsafe)` is needed only for the final C-level close in the
    // Swift 6 nonisolated deinitializer.
    nonisolated(unsafe) private var db: OpaquePointer?
    private var databasePath: String?
    /// Bumped on every successful write. Views cache the expensive daily
    /// aggregates against this value so a re-render does not re-run the
    /// SQLite query unless the underlying rows actually changed.
    public private(set) var revision: Int = 0
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

    // 强制 SQLite 自行复制绑定字符串的 transient destructor。
    private static let SQLITE_TRANSIENT = unsafeBitCast(-1, to: sqlite3_destructor_type.self)

    // day 列用的本地自然日 "yyyy-MM-dd" 格式化器。AI Token Monitor
    // 是把 timestamp 转换为当前系统时区后再聚合日期的。
    private static let dayFormatter: DateFormatter = {
        let f = DateFormatter()
        f.calendar = Calendar(identifier: .gregorian)
        f.timeZone = .current
        f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = "yyyy-MM-dd"
        return f
    }()

    /// 打开 DB 文件（不存在则创建）并确保 schema。失败时返回 nil。
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

    /// 将事件按 (day, service, source, model, project) 聚合后 `INSERT OR REPLACE`。
    /// project 为 nil 时以空字符串存储。
    public func upsert(events: [TokenEvent], calendar: Calendar = .current) {
        // Until the one-time raw-log rebuild completes, writing the bounded
        // event tail beside legacy rows would double-count the same day.
        guard !events.isEmpty, !needsCodexRebuild else { return }
        var grouped: [Key: Agg] = [:]
        // DateFormatter.string 每次事件约需数 µs，数万事件 × 每 60 秒 persist 会
        // 把主线程阻塞数十毫秒——按小时（epoch hour）缓存。
        //（本地日边界与小时边界对齐，因此同一 hour 得到相同的 day 字符串）。
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
        revision &+= 1
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
        revision &+= 1
        return true
    }

    /// Stable backup path used by the automatic one-time migration.
    public func defaultRebuildBackupURL() -> URL? {
        guard let path = databasePath else { return nil }
        return URL(fileURLWithPath: path + ".pre-reported-total-v4.bak")
    }

    // 计算指定 days 范围的起始日（本地自然日字符串）。
    private func cutoffDayString(days: Int, now: Date, calendar: Calendar) -> String {
        let start = calendar.date(byAdding: .day, value: -(days - 1), to: calendar.startOfDay(for: now)) ?? now
        return Self.dayFormatter.string(from: start)
    }

    /// 每日×每服务的 Codex reported-total 合计。最近 `days` 天。
    /// cache_read/cache_create 保留在 schema 中并继续用于成本计算。
    /// 返回顺序是确定的：（day 升序，service 按 ServiceID.allCases 固定顺序）。
    /// 保证图表的堆叠/系列在每次渲染中不会被打乱。
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
        // SQL 不保证 (day, service) 组内的 service 顺序，因此
        // 按（day 升序，service 按 allCases 索引）的固定顺序排序。
        let order = Dictionary(uniqueKeysWithValues: ServiceID.allCases.enumerated().map { ($1, $0) })
        return result.sorted { a, b in
            if a.day != b.day { return a.day < b.day }
            return (order[a.service] ?? 0) < (order[b.service] ?? 0)
        }
    }

    /// 每日 Token 合计（服务合计）。最近 `days` 天，day 升序。
    /// 给定 `services` 时只合计这些服务（用于分布图的 enabled 过滤）。nil 则全部。
    /// Token 为 0 的天没有行，会被省略（调用方按空格子处理）。
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


    /// 最近 `days` 天的估算成本合计（USD）。按 model 列应用 CostEstimator 单价。
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
            // 用临时事件委托单价计算（service/timestamp/project 与成本无关）。
            let synth = TokenEvent(service: .codex, timestamp: now, model: model,
                                   inputTokens: Int(sqlite3_column_int64(stmt, 1)),
                                   outputTokens: Int(sqlite3_column_int64(stmt, 2)),
                                   cacheReadTokens: Int(sqlite3_column_int64(stmt, 3)),
                                   cacheCreationTokens: Int(sqlite3_column_int64(stmt, 4)))
            total += CostEstimator.cost(of: synth)
        }
        return total
    }

    /// 显式日期区间 `[from, to)`（本地自然日，含 from、不含 to）的估算成本合计（USD）。
    /// 解决 `totalCost(days:)` 只支持"最近 N 天"、无法精确计算周报的上周 [D-7, D-1]
    /// 成本的问题（此前会混入上上周的成本）。
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

    // MARK: - 破纪录 / 连续使用（趣味逻辑）

    /// 返回每日 reported-total 合计的**最大值**。
    /// 排除 `excludingDay` 对应的本地日期。
    /// 若其他天一个都没有则返回 nil（没有破纪录的比较基准）。
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

    /// 从 `endingOn`（今天）往回**连续请求 Token > 0** 的天数。
    /// 今天 Token 为 0 则 streak 为 0（含今天口径）。遇到中间空档即中断。
    public func streakDays(endingOn: Date, calendar: Calendar = .current,
                           source: String? = nil) -> Int {
        // Token > 0 的天的 day 字符串集合。
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

    // MARK: - percent 快照（用于周粒度耗尽估算）

    /// 将 (day, service, kind) 的使用率（%）快照用 `INSERT OR REPLACE` 记录。
    /// 因为是 REPLACE，同一天多次调用只保留**最后一次观测值**。
    /// `day` 会被规范化为本地 "yyyy-MM-dd"（与 daily_stats 相同口径）。
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

    /// 返回 (service, kind) 最近 `days` 天的 percent 快照，day 升序。
    /// day 会还原为本地午夜的 Date。
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
