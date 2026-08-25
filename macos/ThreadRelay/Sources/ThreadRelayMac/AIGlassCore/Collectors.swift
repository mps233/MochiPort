import Foundation

private let codexTokenCountASCII = Array("token_count".utf8)
private let codexSessionMetaASCII = Array("session_meta".utf8)
private let codexTurnContextASCII = Array("turn_context".utf8)

private struct CodexUsageSnapshot: Equatable, Hashable, Sendable {
    let input: Int
    let output: Int
    let cached: Int
    let total: Int
}

private struct CodexReplayKey: Hashable, Sendable {
    let sessionID: String
    let model: String
    let usage: CodexUsageSnapshot
    let cumulative: CodexCumulativeUsage

    /// UsageStore's compatibility API accepts string keys. Length-prefix the
    /// free-form fields so two IDs cannot collide through separator content.
    var storageKey: String {
        "codex-replay:\(sessionID.utf8.count):\(sessionID)\(model.utf8.count):\(model):" +
            "\(usage.input):\(usage.output):\(usage.cached):\(usage.total):" +
            "\(cumulative.inputTokens):\(cumulative.cachedInputTokens):" +
            "\(cumulative.outputTokens):\(cumulative.reasoningOutputTokens):" +
            "\(cumulative.totalTokens)"
    }
}

private struct CodexFileParseState: Sendable {
    var metadata: CodexSessionMeta
    var currentModel: String
    var previousSnapshot: CodexUsageSnapshot?
    var linesConsumed: Int
}

private struct CodexCollectedEvent: Sendable {
    let event: TokenEvent
    let dedupKey: String
}

private struct CodexParsedBatch: Sendable {
    let events: [CodexCollectedEvent]
    let limits: [LimitWindow]?
    let latestLimitTimestamp: Date
    let state: CodexFileParseState
}

private struct CodexRecentFile: Sendable {
    let url: URL
    let metadata: CodexSessionMeta?
    let persistOffset: Bool
}

/// Codex 会话日志采集器。
///
/// 与原 ai-glass 保持相同的数据范围：扫描最近 8 天的全部 JSONL 文件，
/// 每个文件从头读取一次，后续刷新只读取新增内容。这样趋势和当日统计不会
/// 因为文件数量或单文件大小被静默截断。
@MainActor
public final class CodexCollector {
    private let roots: [URL]
    /// File I/O remains bounded while avoiding thousands of observable-store
    /// mutations during a multi-gigabyte startup rehydration.
    private let hydrationBatchBytes: Int
    private let reader = IncrementalLineReader()
    private var latestLimitsTimestamp: Date = .distantPast
    /// 文件 path → session metadata 缓存（session_meta）。
    private var sessionMetaCache: [String: CodexSessionMeta?] = [:]
    /// Stateful fields required to resume parsing an append-only rollout.
    private var fileParseStates: [String: CodexFileParseState] = [:]
    /// UsageStore is memory-only, so rehydrate the recent event tail once per
    /// process before switching back to persisted incremental offsets.
    private var didHydrateRecentHistory = false

    public init(
        root: URL,
        hydrationBatchBytes: Int = 4 * 1024 * 1024
    ) {
        self.roots = [root]
        self.hydrationBatchBytes = max(64 * 1024, hydrationBatchBytes)
    }

    public init(
        roots: [URL],
        hydrationBatchBytes: Int = 4 * 1024 * 1024
    ) {
        self.roots = roots
        self.hydrationBatchBytes = max(64 * 1024, hydrationBatchBytes)
    }

    private struct HistoricalKey: Hashable {
        let day: String
        let source: String
        let model: String
        let project: String
    }

    private struct HistoricalAgg {
        var input = 0
        var output = 0
        var cacheRead = 0
        var cacheCreate = 0
        var usageTotal = 0

        mutating func add(_ event: TokenEvent) {
            input += event.inputTokens
            output += event.outputTokens
            cacheRead += event.cacheReadTokens
            cacheCreate += event.cacheCreationTokens
            usageTotal += event.reportedTotalTokens
        }
    }

    /// Rebuild the durable Codex history from every readable session log.
    /// The caller only clears the database after this method has proven that
    /// enumeration and all file reads completed, so a transient partial scan
    /// leaves the previous history untouched.
    @discardableResult
    public func rebuildHistoricalStats(into statsStore: DailyStatsStore) -> Bool {
        guard let rows = Self.historicalRows(roots: roots) else { return false }
        return statsStore.rebuildCodexStats(
            rows: rows,
            databaseBackupURL: statsStore.defaultRebuildBackupURL())
    }

    /// Runs without the collector's main-actor isolation so a large profile
    /// cannot freeze the GUI during the one-time migration.
    public nonisolated static func historicalRows(root: URL) -> [DailyStatsRow]? {
        historicalRows(roots: [root])
    }

    public nonisolated static func historicalRows(roots: [URL]) -> [DailyStatsRow]? {
        let existingRoots = roots.filter { FileManager.default.fileExists(atPath: $0.path) }
        guard !existingRoots.isEmpty else { return nil }
        var files: [URL] = []
        for root in existingRoots {
            guard let located = LogLocator.allFiles(under: root, suffix: ".jsonl") else {
                return nil
            }
            files.append(contentsOf: located)
        }
        files = Array(Set(files)).sorted { $0.path < $1.path }
        guard !files.isEmpty else { return nil }
        let dayFormatter: DateFormatter = {
            let formatter = DateFormatter()
            formatter.calendar = Calendar(identifier: .gregorian)
            formatter.timeZone = .current
            formatter.locale = Locale(identifier: "en_US_POSIX")
            formatter.dateFormat = "yyyy-MM-dd"
            return formatter
        }()
        var grouped: [HistoricalKey: HistoricalAgg] = [:]
        var replayCandidates: [CodexReplayKey: TokenEvent] = [:]

        func aggregate(_ event: TokenEvent) {
            let key = HistoricalKey(
                day: dayFormatter.string(from: event.timestamp),
                source: event.source,
                model: event.model,
                project: event.project ?? "")
            grouped[key, default: HistoricalAgg()].add(event)
        }

        for file in files {
            var state = Self.initialState(file: file, metadata: nil)
            let fallbackTimestamp = Self.fallbackTimestamp(from: file)
            var fileReadSucceeded = true
            Self.forEachLine(of: file, { lineData in
                state.linesConsumed += 1
                // Session logs contain large messages, tool output and other
                // records that cannot affect usage. Their record type is in
                // the small JSON header, so avoid full JSON decoding for them.
                if Self.containsASCII(codexSessionMetaASCII, inPrefixOf: lineData),
                   let details = CodexLogParser.parseSessionMetaDetails(
                    line: String(decoding: lineData, as: UTF8.self)) {
                    state.metadata = Self.metadata(details, fallingBackTo: file)
                    return
                }
                if Self.containsASCII(codexTurnContextASCII, inPrefixOf: lineData),
                   let model = CodexLogParser.parseTurnContextModel(
                    line: String(decoding: lineData, as: UTF8.self)) {
                    state.currentModel = model
                    return
                }
                guard Self.containsASCII(codexTokenCountASCII, inPrefixOf: lineData),
                      let parsed = CodexLogParser.parse(
                        line: String(decoding: lineData, as: UTF8.self),
                        model: state.currentModel.isEmpty ? "codex" : state.currentModel,
                        fallbackTimestamp: fallbackTimestamp),
                      let parsedEvent = parsed.event else { return }
                let snapshot = Self.snapshot(of: parsedEvent)
                // Codex can emit the same snapshot twice back-to-back in one
                // file. Only consecutive snapshots collapse at file scope.
                guard state.previousSnapshot != snapshot else { return }
                state.previousSnapshot = snapshot
                guard snapshot != CodexUsageSnapshot(input: 0, output: 0, cached: 0, total: 0)
                else { return }
                let event = TokenEvent(
                    service: .codex,
                    timestamp: parsedEvent.timestamp,
                    model: parsedEvent.model,
                    inputTokens: parsedEvent.inputTokens,
                    outputTokens: parsedEvent.outputTokens,
                    cacheReadTokens: parsedEvent.cacheReadTokens,
                    cacheCreationTokens: parsedEvent.cacheCreationTokens,
                    project: state.metadata.project,
                    source: state.metadata.source,
                    reportedInputTokens: parsedEvent.reportedInputTokens,
                    reportedTotalTokens: parsedEvent.reportedTotalTokens,
                    sessionID: state.metadata.sessionID,
                    cumulativeUsage: parsedEvent.cumulativeUsage)
                if let replayKey = Self.replayKey(for: event) {
                    if let kept = replayCandidates[replayKey] {
                        // ai-token-monitor attributes a replay to the earliest
                        // local day on which the turn appeared.
                        if dayFormatter.string(from: event.timestamp)
                            < dayFormatter.string(from: kept.timestamp) {
                            replayCandidates[replayKey] = event
                        }
                    } else {
                        replayCandidates[replayKey] = event
                    }
                } else {
                    // Without a cumulative snapshot there is no safe proof
                    // that equal-looking turns in separate files are replays.
                    aggregate(event)
                }
            }, onFailure: {
                fileReadSucceeded = false
            })
            if !fileReadSucceeded { return nil }
        }
        for event in replayCandidates.values { aggregate(event) }

        return grouped.map { key, aggregate in
            DailyStatsRow(
                day: key.day,
                service: ServiceID.codex.rawValue,
                source: key.source,
                model: key.model,
                project: key.project,
                input: aggregate.input,
                output: aggregate.output,
                cacheRead: aggregate.cacheRead,
                cacheCreate: aggregate.cacheCreate,
                usageTotal: aggregate.usageTotal)
        }
    }

    /// 只读取文件第一行，解析 session_meta 中的项目与 provider/source。
    /// This helper is nonisolated so the one-time recent-history scan can run
    /// it from a utility task instead of blocking the GUI actor.
    nonisolated private static func readFirstLine(of file: URL) -> String? {
        guard let fh = FileHandle(forReadingAtPath: file.path) else { return nil }
        defer { try? fh.close() }
        let chunkSize = 64 * 1024
        let maxBytes = 1024 * 1024
        var buffer = Data()
        while buffer.count < maxBytes {
            guard let chunk = try? fh.read(upToCount: chunkSize), !chunk.isEmpty else { break }
            buffer.append(chunk)
            if let newline = buffer.firstIndex(of: UInt8(ascii: "\n")) {
                return String(data: buffer[buffer.startIndex..<newline], encoding: .utf8)
            }
        }
        if buffer.count >= maxBytes { return nil }
        return buffer.isEmpty ? nil : String(data: buffer, encoding: .utf8)
    }

    private func sessionMeta(for file: URL) -> CodexSessionMeta? {
        let path = file.path
        if let cached = sessionMetaCache[path] { return cached }
        let result = Self.readFirstLine(of: file)
            .flatMap(CodexLogParser.parseSessionMetaDetails(line:))
        sessionMetaCache[path] = result
        return result
    }

    /// Enumerates files and reads their small session metadata record away
    /// from the main actor. This is intentionally separate from batch parsing
    /// so a large profile only crosses the actor boundary with a tiny list of
    /// immutable descriptors.
    nonisolated private static func recentFiles(roots: [URL]) -> [CodexRecentFile] {
        let files = Array(Set(roots.flatMap {
            LogLocator.recentFiles(under: $0, suffix: ".jsonl")
        })).sorted { $0.path < $1.path }
        return files.map { file in
            let metadata = Self.readFirstLine(of: file)
                .flatMap(CodexLogParser.parseSessionMetaDetails(line:))
            let modifiedAt = try? file.resourceValues(forKeys: [.contentModificationDateKey])
                .contentModificationDate
            let persistOffset = modifiedAt.map { !Calendar.current.isDateInToday($0) } ?? false
            return CodexRecentFile(
                url: file,
                metadata: metadata,
                persistOffset: persistOffset)
        }
    }

    nonisolated private static func metadata(
        _ metadata: CodexSessionMeta?,
        fallingBackTo file: URL
    ) -> CodexSessionMeta {
        let fallbackID = file.deletingPathExtension().lastPathComponent
        guard let metadata else {
            return CodexSessionMeta(project: nil, sessionID: fallbackID)
        }
        return CodexSessionMeta(
            project: metadata.project,
            source: metadata.source,
            sessionID: metadata.sessionID ?? fallbackID)
    }

    nonisolated private static func initialState(
        file: URL,
        metadata: CodexSessionMeta?
    ) -> CodexFileParseState {
        CodexFileParseState(
            metadata: Self.metadata(metadata, fallingBackTo: file),
            currentModel: "",
            previousSnapshot: nil,
            linesConsumed: 0)
    }

    /// `.../(sessions|archived_sessions)/YYYY/MM/DD/...` fallback used only
    /// when an event carries no usable timestamp.
    nonisolated private static func fallbackTimestamp(from file: URL) -> Date? {
        let components = file.pathComponents
        guard components.count >= 4 else { return nil }
        for index in 0...(components.count - 4) {
            guard components[index] == "sessions" || components[index] == "archived_sessions"
            else { continue }
            let date = components[(index + 1)...(index + 3)].joined(separator: "-")
            let formatter = DateFormatter()
            formatter.calendar = Calendar(identifier: .gregorian)
            formatter.timeZone = .current
            formatter.locale = Locale(identifier: "en_US_POSIX")
            formatter.dateFormat = "yyyy-MM-dd"
            if let parsed = formatter.date(from: date) { return parsed }
        }
        return nil
    }

    nonisolated private static func snapshot(of event: TokenEvent) -> CodexUsageSnapshot {
        CodexUsageSnapshot(
            input: event.reportedInputTokens,
            output: event.outputTokens,
            cached: event.cacheReadTokens,
            total: event.reportedTotalTokens)
    }

    nonisolated private static func replayKey(for event: TokenEvent) -> CodexReplayKey? {
        guard let sessionID = event.sessionID,
              let cumulative = event.cumulativeUsage else { return nil }
        return CodexReplayKey(
            sessionID: sessionID,
            model: CodexModel.normalize(event.model),
            usage: snapshot(of: event),
            cumulative: cumulative)
    }

    /// Parses one bounded line batch on a utility executor. No mutable
    /// collector or UsageStore state is touched here.
    nonisolated private static func parseBatch(
        lines: [Data],
        file: URL,
        initialState: CodexFileParseState,
        latestLimitTimestamp: Date
    ) -> CodexParsedBatch {
        var events: [CodexCollectedEvent] = []
        events.reserveCapacity(max(1, lines.count / 4))
        var latestTimestamp = latestLimitTimestamp
        var latestLimits: [LimitWindow]? = nil
        var state = initialState
        let fallbackTimestamp = Self.fallbackTimestamp(from: file)
        for lineData in lines {
            state.linesConsumed += 1
            if Self.containsASCII(codexSessionMetaASCII, inPrefixOf: lineData),
               let details = CodexLogParser.parseSessionMetaDetails(
                line: String(decoding: lineData, as: UTF8.self)) {
                state.metadata = Self.metadata(details, fallingBackTo: file)
                continue
            }
            if Self.containsASCII(codexTurnContextASCII, inPrefixOf: lineData),
               let model = CodexLogParser.parseTurnContextModel(
                line: String(decoding: lineData, as: UTF8.self)) {
                state.currentModel = model
                continue
            }
            // Most bytes in a rollout are messages/tool output. `token_count`
            // is emitted in the record header, so skip their expensive JSON
            // decoding while retaining a full parse as the correctness gate.
            guard Self.containsASCII(codexTokenCountASCII, inPrefixOf: lineData),
                  let parsed = CodexLogParser.parse(
                    line: String(decoding: lineData, as: UTF8.self),
                    model: state.currentModel.isEmpty ? "codex" : state.currentModel,
                    fallbackTimestamp: fallbackTimestamp)
            else { continue }
            if !parsed.limits.isEmpty, parsed.timestamp > latestTimestamp {
                latestTimestamp = parsed.timestamp
                latestLimits = parsed.limits
            }
            if let event = parsed.event {
                let snapshot = Self.snapshot(of: event)
                guard state.previousSnapshot != snapshot else { continue }
                state.previousSnapshot = snapshot
                guard snapshot != CodexUsageSnapshot(input: 0, output: 0, cached: 0, total: 0)
                else { continue }
                let eventWithMetadata = TokenEvent(
                    service: .codex,
                    timestamp: event.timestamp,
                    model: event.model,
                    inputTokens: event.inputTokens,
                    outputTokens: event.outputTokens,
                    cacheReadTokens: event.cacheReadTokens,
                    cacheCreationTokens: event.cacheCreationTokens,
                    project: state.metadata.project,
                    source: state.metadata.source,
                    reportedInputTokens: event.reportedInputTokens,
                    reportedTotalTokens: event.reportedTotalTokens,
                    sessionID: state.metadata.sessionID,
                    cumulativeUsage: event.cumulativeUsage)
                let key = Self.replayKey(for: eventWithMetadata)?.storageKey
                    ?? "codex-file:\(file.path.utf8.count):\(file.path):\(state.linesConsumed)"
                events.append(CodexCollectedEvent(event: eventWithMetadata, dedupKey: key))
            }
        }
        return CodexParsedBatch(
            events: events,
            limits: latestLimits,
            latestLimitTimestamp: latestTimestamp,
            state: state)
    }

    /// Searches only the small JSON header without allocating a String for
    /// the overwhelming majority of unrelated rollout records.
    nonisolated private static func containsASCII(
        _ pattern: [UInt8],
        inPrefixOf data: Data,
        maxBytes: Int = 1024
    ) -> Bool {
        let count = min(data.count, maxBytes)
        guard !pattern.isEmpty, count >= pattern.count else { return false }
        return data.withUnsafeBytes { raw in
            let bytes = raw.bindMemory(to: UInt8.self)
            let last = count - pattern.count
            for start in 0...last {
                var matches = true
                for index in pattern.indices where bytes[start + index] != pattern[index] {
                    matches = false
                    break
                }
                if matches { return true }
            }
            return false
        }
    }

    /// Rehydrates the in-memory tail in bounded asynchronous batches. File
    /// reads, metadata lookup, and JSON parsing happen on utility tasks; the
    /// main actor only applies one bounded batch at a time.
    public func hydrateRecentHistory(into store: UsageStore) async -> Bool {
        let roots = self.roots
        let files = await Task.detached(priority: .utility) {
            Self.recentFiles(roots: roots)
        }.value
        guard !files.isEmpty else {
            didHydrateRecentHistory = true
            return true
        }
        for descriptor in files {
            sessionMetaCache[descriptor.url.path] = descriptor.metadata
        }

        var latestLimitsTimestamp = Date.distantPast
        for descriptor in files {
            guard !Task.isCancelled else { return false }
            // UsageStore is memory-only. Ignore a previous process's durable
            // cursor for this one-time pass so every recent event is restored;
            // the historical file's offset is persisted again only at EOF.
            reader.rewindForRehydration(of: descriptor.url)
            var parseState = Self.initialState(file: descriptor.url, metadata: descriptor.metadata)
            var reachedEOF = false

            while !reachedEOF {
                guard let batch = await reader.nextBatch(
                    of: descriptor.url,
                    persistOffset: descriptor.persistOffset,
                    maxBytes: self.hydrationBatchBytes
                ) else {
                    // Empty files and an unterminated final line have no
                    // complete records to apply; the next refresh can retry
                    // the uncommitted tail.
                    break
                }

                let lines = batch.lines
                let file = descriptor.url
                let initialState = parseState
                let limitTimestamp = latestLimitsTimestamp
                let parsed = await Task.detached(priority: .utility) { [lines, file, initialState, limitTimestamp] in
                    Self.parseBatch(
                        lines: lines,
                        file: file,
                        initialState: initialState,
                        latestLimitTimestamp: limitTimestamp)
                }.value
                let events = parsed.events.map { (event: $0.event, dedupKey: Optional($0.dedupKey)) }
                if !events.isEmpty { store.addEvents(events) }
                if let limits = parsed.limits { store.setLimits(limits, for: .codex) }
                latestLimitsTimestamp = parsed.latestLimitTimestamp
                parseState = parsed.state
                reader.commit(
                    of: descriptor.url,
                    offset: batch.nextOffset,
                    persistOffset: descriptor.persistOffset,
                    reachedEOF: batch.reachedEOF)
                reachedEOF = batch.reachedEOF
                await Task.yield()
            }
            fileParseStates[descriptor.url.path] = parseState
        }

        guard !Task.isCancelled else { return false }
        self.latestLimitsTimestamp = max(self.latestLimitsTimestamp, latestLimitsTimestamp)
        didHydrateRecentHistory = true
        return true
    }

    nonisolated private static func forEachLine(
        of file: URL,
        _ body: (Data) -> Void,
        onFailure: () -> Void = {},
        onComplete: (UInt64) -> Void = { _ in }) {
        guard let handle = FileHandle(forReadingAtPath: file.path) else {
            onFailure()
            return
        }
        defer { try? handle.close() }
        var buffer = Data()
        var consumedOffset: UInt64 = 0
        while true {
            let chunk: Data
            do {
                chunk = try handle.read(upToCount: 64 * 1024) ?? Data()
            } catch {
                onFailure()
                return
            }
            if chunk.isEmpty { break }
            buffer.append(chunk)
            var cursor = buffer.startIndex
            while let newline = buffer[cursor...].firstIndex(of: UInt8(ascii: "\n")) {
                let afterNewline = buffer.index(after: newline)
                let line = buffer[cursor..<newline]
                body(line)
                consumedOffset += UInt64(afterNewline - cursor)
                cursor = afterNewline
            }
            if cursor > buffer.startIndex {
                // Remove all consumed records once per read cycle. Repeated
                // front-removal for every line turns large files into an
                // avoidable quadratic scan.
                buffer.removeSubrange(..<cursor)
            }
        }
        // An unterminated final line is intentionally ignored. Codex writes
        // token events as complete JSONL records; a partial tail will be
        // picked up by the normal incremental collector on the next refresh.
        onComplete(consumedOffset)
    }

    public var hasHydratedRecentHistory: Bool { didHydrateRecentHistory }

    public func collect(into store: UsageStore) {
        let shouldHydrateRecentHistory = !didHydrateRecentHistory
        for file in Self.recentFiles(roots: roots).map(\.url) {
            let sessionMeta = sessionMeta(for: file)
            let modifiedAt = try? file.resourceValues(forKeys: [.contentModificationDateKey])
                .contentModificationDate
            let persistOffset = modifiedAt.map { !Calendar.current.isDateInToday($0) } ?? false
            // A fresh UsageStore has no historical events after a GUI restart.
            // Read the recent tail from the beginning once; subsequent refreshes
            // retain the normal incremental behavior, including today's files.
            let effectivePersistOffset = shouldHydrateRecentHistory ? false : persistOffset
            var parseState = shouldHydrateRecentHistory
                ? Self.initialState(file: file, metadata: sessionMeta)
                : (fileParseStates[file.path]
                    ?? Self.initialState(file: file, metadata: sessionMeta))
            while let lineBatch = reader.nextBatchSynchronously(
                of: file,
                persistOffset: effectivePersistOffset
            ) {
                let parsed = Self.parseBatch(
                    lines: lineBatch.lines,
                    file: file,
                    initialState: parseState,
                    latestLimitTimestamp: latestLimitsTimestamp)
                let events = parsed.events.map { (event: $0.event, dedupKey: Optional($0.dedupKey)) }
                if !events.isEmpty { store.addEvents(events) }
                if let limits = parsed.limits { store.setLimits(limits, for: .codex) }
                latestLimitsTimestamp = parsed.latestLimitTimestamp
                parseState = parsed.state
                reader.commit(
                    of: file,
                    offset: lineBatch.nextOffset,
                    persistOffset: effectivePersistOffset,
                    reachedEOF: lineBatch.reachedEOF)
                if lineBatch.reachedEOF { break }
            }
            fileParseStates[file.path] = parseState
        }
        didHydrateRecentHistory = true
    }
}
