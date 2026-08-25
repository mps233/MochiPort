import Foundation

/// 文件级增量 JSONL 读取器。
///
/// `newLines` 保留旧的同步 API；生产采集路径使用 `nextBatch`，每次只把
/// 有界的一批完整记录交给调用方，并在调用方确认处理后提交 offset。
public final class IncrementalLineReader: @unchecked Sendable {
    /// A bounded chunk of complete JSONL records.
    public struct Batch: Sendable {
        public let lines: [Data]
        public let nextOffset: UInt64
        public let reachedEOF: Bool

        public init(lines: [Data], nextOffset: UInt64, reachedEOF: Bool) {
            self.lines = lines
            self.nextOffset = nextOffset
            self.reachedEOF = reachedEOF
        }
    }

    private struct Cursor: Sendable {
        let path: String
        let start: UInt64
    }

    private struct ReadResult: Sendable {
        let lines: [Data]
        let nextOffset: UInt64
        let reachedEOF: Bool
        let failed: Bool
    }

    /// Keep each foreground handoff small. The actual file read in
    /// `nextBatch` runs on a detached utility task.
    public static let defaultBatchBytes = 256 * 1024
    private static let readChunkBytes = 64 * 1024

    private var offsets: [String: UInt64] = [:]
    /// Today's files stay in memory only so a restart can rebuild today's
    /// totals from the beginning. Historical files may safely persist their
    /// offsets because their contents are no longer expected to change.
    private var volatileKeys: Set<String> = []
    private let lock = NSLock()

    // v2 invalidates the first experimental cache format. It is intentionally
    // a new UserDefaults key so an upgrade cannot silently skip old history.
    private static let offsetsKey = "aiglass.codexReaderOffsets.v2"

    public init() {
        if let persisted = UserDefaults.standard.dictionary(forKey: Self.offsetsKey) {
            offsets = persisted.reduce(into: [:]) { result, item in
                if let number = item.value as? NSNumber {
                    result[item.key] = number.uint64Value
                }
            }
        }
    }

    /// Reads one bounded batch on a utility executor. The returned offset must
    /// be committed by the caller after the batch has been parsed/applied.
    public func nextBatch(
        of url: URL,
        persistOffset: Bool = true,
        maxBytes: Int = IncrementalLineReader.defaultBatchBytes
    ) async -> Batch? {
        let cursor = prepareCursor(of: url, persistOffset: persistOffset)
        let result = await Task.detached(priority: .utility) {
            Self.readResult(path: cursor.path, start: cursor.start, maxBytes: maxBytes)
        }.value
        guard !result.failed else { return nil }
        guard !result.lines.isEmpty else {
            // A complete file with no new records still acknowledges EOF. In
            // particular, the preceding batch may have stopped exactly at the
            // byte budget, so its reachedEOF flag was necessarily false.
            if result.reachedEOF {
                commit(
                    of: url,
                    offset: result.nextOffset,
                    persistOffset: persistOffset,
                    reachedEOF: true)
            }
            return nil
        }
        return Batch(
            lines: result.lines,
            nextOffset: result.nextOffset,
            reachedEOF: result.reachedEOF)
    }

    /// Synchronous counterpart used by the compatibility `CodexCollector`
    /// entry point and small unit fixtures. It still reads in bounded chunks;
    /// production startup uses the async variant above.
    public func nextBatchSynchronously(
        of url: URL,
        persistOffset: Bool = true,
        maxBytes: Int = IncrementalLineReader.defaultBatchBytes
    ) -> Batch? {
        let cursor = prepareCursor(of: url, persistOffset: persistOffset)
        let result = Self.readResult(path: cursor.path, start: cursor.start, maxBytes: maxBytes)
        guard !result.failed else { return nil }
        guard !result.lines.isEmpty else {
            if result.reachedEOF {
                commit(
                    of: url,
                    offset: result.nextOffset,
                    persistOffset: persistOffset,
                    reachedEOF: true)
            }
            return nil
        }
        return Batch(lines: result.lines, nextOffset: result.nextOffset, reachedEOF: result.reachedEOF)
    }

    /// Commits a batch after its records have been applied to the usage store.
    /// Delaying this write prevents cancellation between read and parse from
    /// silently losing events on the next refresh.
    public func commit(
        of url: URL,
        offset: UInt64,
        persistOffset: Bool,
        reachedEOF: Bool
    ) {
        lock.lock()
        defer { lock.unlock() }
        let key = url.path
        let current = offsets[key] ?? 0
        // A stale batch must never move a cursor backwards if a newer batch
        // for the same file has already been committed.
        guard offset >= current else { return }
        offsets[key] = offset
        if persistOffset, reachedEOF {
            persistOffsetsLocked()
        }
    }

    /// Compatibility API. It now uses bounded reads internally, so it no
    /// longer creates a `readToEnd` buffer plus a second split array.
    public func newLines(of url: URL, persistOffset: Bool = true) -> [String] {
        var result: [String] = []
        while let batch = nextBatchSynchronously(of: url, persistOffset: persistOffset) {
            result.append(contentsOf: batch.lines.map { String(decoding: $0, as: UTF8.self) })
            commit(
                of: url,
                offset: batch.nextOffset,
                persistOffset: persistOffset,
                reachedEOF: batch.reachedEOF)
            if batch.reachedEOF { break }
        }
        return result
    }

    /// Seeds offsets after a background scan has consumed complete files.
    /// Keeping this operation on the owning actor avoids re-reading the same
    /// historical bytes on the next timer tick.
    public func prime(offsets: [String: UInt64]) {
        lock.lock()
        defer { lock.unlock() }
        for (path, offset) in offsets {
            self.offsets[path] = offset
            volatileKeys.remove(path)
        }
        persistOffsetsLocked()
    }

    /// Starts a deliberate rehydration pass at byte zero without overwriting
    /// the last durable offset. If the pass is cancelled, a later process can
    /// still fall back to the previously completed cursor; reaching EOF will
    /// atomically replace it through `commit`.
    public func rewindForRehydration(of url: URL) {
        lock.lock()
        defer { lock.unlock() }
        offsets[url.path] = 0
        volatileKeys.remove(url.path)
    }

    private func prepareCursor(of url: URL, persistOffset: Bool) -> Cursor {
        let path = url.path
        lock.lock()
        defer { lock.unlock() }
        if !persistOffset, volatileKeys.insert(path).inserted {
            // Do not inherit an offset from a previous process for today's
            // files; the in-memory store needs their complete history.
            offsets[path] = 0
        }
        let attributes = try? FileManager.default.attributesOfItem(atPath: path)
        let size = (attributes?[.size] as? NSNumber)?.uint64Value ?? 0
        var start = offsets[path] ?? 0
        if start > size {
            start = 0 // file was replaced/rotated
            offsets[path] = 0
        }
        return Cursor(path: path, start: start)
    }

    private func persistOffsetsLocked() {
        UserDefaults.standard.set(offsets.mapValues(NSNumber.init(value:)), forKey: Self.offsetsKey)
    }

    /// Reads complete lines from `start`, stopping after approximately
    /// `maxBytes` of consumed data. Any unterminated tail remains unconsumed so
    /// the next refresh can retry it after Codex finishes writing the record.
    private static func readResult(path: String, start: UInt64, maxBytes: Int) -> ReadResult {
        guard let handle = try? FileHandle(forReadingFrom: URL(fileURLWithPath: path)) else {
            return ReadResult(lines: [], nextOffset: start, reachedEOF: false, failed: true)
        }
        defer { try? handle.close() }
        guard (try? handle.seek(toOffset: start)) != nil else {
            return ReadResult(lines: [], nextOffset: start, reachedEOF: false, failed: true)
        }

        let targetBytes = max(1, maxBytes)
        var buffer = Data()
        var consumed = 0
        var lines: [Data] = []
        var reachedEOF = false

        while true {
            // Stop once a bounded amount of complete data is ready. A single
            // oversized JSONL record is allowed through as one line instead of
            // being silently dropped.
            if consumed >= targetBytes, !lines.isEmpty { break }
            let chunk: Data
            do {
                chunk = try handle.read(upToCount: Self.readChunkBytes) ?? Data()
            } catch {
                // Complete lines already extracted remain safe to consume. The
                // caller will retry from the last committed offset otherwise.
                return ReadResult(
                    lines: lines,
                    nextOffset: start + UInt64(consumed),
                    reachedEOF: false,
                    failed: lines.isEmpty)
            }
            if chunk.isEmpty {
                reachedEOF = true
            } else {
                buffer.append(chunk)
            }

            var cursor = buffer.startIndex
            while let newline = buffer[cursor...].firstIndex(of: UInt8(ascii: "\n")) {
                let afterNewline = buffer.index(after: newline)
                lines.append(Data(buffer[cursor..<newline]))
                consumed += afterNewline - cursor
                cursor = afterNewline
                if consumed >= targetBytes { break }
            }
            if cursor > buffer.startIndex {
                // Remove consumed records once per read cycle, avoiding the
                // repeated front-removal cost of a line-by-line split loop.
                buffer.removeSubrange(..<cursor)
            }

            if !lines.isEmpty, consumed >= targetBytes { break }
            if reachedEOF { break }
        }

        return ReadResult(
            lines: lines,
            nextOffset: start + UInt64(consumed),
            reachedEOF: reachedEOF,
            failed: false)
    }
}
