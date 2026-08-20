import Foundation

public enum LogLocator {
    /// dir 아래를 재귀 탐색해 suffix로 끝나고 최근 N일 내 수정된 파일을 반환.
    public static func recentFiles(under dir: URL, suffix: String,
                                   modifiedWithinDays: Int = 8) -> [URL] {
        let cutoff = Date().addingTimeInterval(-Double(modifiedWithinDays) * 24 * 3600)
        guard let enumerator = FileManager.default.enumerator(
            at: dir, includingPropertiesForKeys: [.contentModificationDateKey],
            options: [.skipsHiddenFiles]) else { return [] }

        var result: [URL] = []
        for case let url as URL in enumerator {
            guard url.lastPathComponent.hasSuffix(suffix),
                  let mod = try? url.resourceValues(forKeys: [.contentModificationDateKey])
                      .contentModificationDate,
                  mod >= cutoff else { continue }
            result.append(url)
        }
        return result.sorted { $0.path < $1.path }
    }
}
