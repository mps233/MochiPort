import Foundation

/// 파일별 바이트 오프셋을 기억해 새로 추가된 완성 라인만 돌려준다.
public final class IncrementalLineReader {
    private var offsets: [String: UInt64] = [:]
    /// Today's files stay in memory only so a restart can rebuild today's
    /// totals from the beginning. Historical files may safely persist their
    /// offsets because their contents are no longer expected to change.
    private var volatileKeys: Set<String> = []
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

    public func newLines(of url: URL, persistOffset: Bool = true) -> [String] {
        guard let handle = try? FileHandle(forReadingFrom: url) else { return [] }
        defer { try? handle.close() }

        let key = url.path
        if !persistOffset, volatileKeys.insert(key).inserted {
            // Do not inherit an offset from a previous process for today's
            // files; the in-memory store needs their complete history.
            offsets[key] = 0
        }
        let size = (try? handle.seekToEnd()) ?? 0
        var start = offsets[key] ?? 0
        if start > size { start = 0 } // 파일이 줄었다 = 교체/순환 → 처음부터
        guard size > start else {
            offsets[key] = size
            if persistOffset { persistOffsets() }
            return []
        }

        guard (try? handle.seek(toOffset: start)) != nil,
              let data = try? handle.readToEnd(), !data.isEmpty else { return [] }

        // 완성된 줄(\n까지)만 소비하고 오프셋 전진
        guard let lastNewline = data.lastIndex(of: UInt8(ascii: "\n")) else { return [] }
        let consumed = data[data.startIndex...lastNewline]
        offsets[key] = start + UInt64(consumed.count)
        if persistOffset { persistOffsets() }
        return String(decoding: consumed, as: UTF8.self)
            .split(separator: "\n", omittingEmptySubsequences: true)
            .map(String.init)
    }

    private func persistOffsets() {
        UserDefaults.standard.set(offsets.mapValues(NSNumber.init(value:)), forKey: Self.offsetsKey)
    }
}
