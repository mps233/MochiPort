import Foundation

/// Codex 会话日志采集器。
///
/// 与原 ai-glass 保持相同的数据范围：扫描最近 8 天的全部 JSONL 文件，
/// 每个文件从头读取一次，后续刷新只读取新增内容。这样趋势和当日统计不会
/// 因为文件数量或单文件大小被静默截断。
@MainActor
public final class CodexCollector {
    private let root: URL
    private let reader = IncrementalLineReader()
    private var latestLimitsTimestamp: Date = .distantPast
    /// 文件 path → project name 缓存（session_meta）。
    private var projectCache: [String: String?] = [:]

    public init(root: URL) { self.root = root }

    /// 只读取文件第一行，解析 session_meta 中的 cwd 作为项目名。
    private func readFirstLine(of file: URL) -> String? {
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

    private func project(for file: URL) -> String? {
        let path = file.path
        if let cached = projectCache[path] { return cached }
        let result = readFirstLine(of: file).flatMap(CodexLogParser.parseSessionMeta(line:))
        projectCache[path] = result
        return result
    }

    public func collect(into store: UsageStore) {
        var batch: [(TokenEvent, String?)] = []
        var latestLimits: [LimitWindow]?
        for file in LogLocator.recentFiles(under: root, suffix: ".jsonl") {
            let proj = project(for: file)
            let modifiedAt = try? file.resourceValues(forKeys: [.contentModificationDateKey])
                .contentModificationDate
            let persistOffset = modifiedAt.map { !Calendar.current.isDateInToday($0) } ?? false
            for line in reader.newLines(of: file, persistOffset: persistOffset) {
                guard let parsed = CodexLogParser.parse(line: line) else { continue }
                if let event = parsed.event {
                    // 文件轮换后即使重新解析，也通过稳定键避免重复计数。
                    let key = "codex:\(file.lastPathComponent):\(parsed.timestamp.timeIntervalSince1970):\(event.totalTokens)"
                    let eventWithProject = TokenEvent(
                        service: .codex,
                        timestamp: event.timestamp,
                        model: event.model,
                        inputTokens: event.inputTokens,
                        outputTokens: event.outputTokens,
                        cacheReadTokens: event.cacheReadTokens,
                        cacheCreationTokens: event.cacheCreationTokens,
                        project: proj)
                    batch.append((eventWithProject, key))
                }
                if !parsed.limits.isEmpty, parsed.timestamp > latestLimitsTimestamp {
                    latestLimitsTimestamp = parsed.timestamp
                    latestLimits = parsed.limits
                }
            }
        }
        if !batch.isEmpty { store.addEvents(batch) }
        if let latestLimits { store.setLimits(latestLimits, for: .codex) }
    }
}
