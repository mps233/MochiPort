import Foundation

public enum LogLocator {
    /// Enumerate every matching file below `dir` without a modification-date
    /// cutoff. A `nil` result means the directory could not be traversed and
    /// therefore must not be treated as a complete historical scan.
    public static func allFiles(under dir: URL, suffix: String) -> [URL]? {
        guard let enumerator = FileManager.default.enumerator(
            at: dir, includingPropertiesForKeys: [.isRegularFileKey],
            options: [.skipsHiddenFiles]) else { return nil }

        var result: [URL] = []
        for case let url as URL in enumerator {
            guard url.lastPathComponent.hasSuffix(suffix),
                  let values = try? url.resourceValues(forKeys: [.isRegularFileKey]),
                  values.isRegularFile == true else { continue }
            result.append(url)
        }
        return result.sorted { $0.path < $1.path }
    }

    /// 递归搜索 dir 下以 suffix 结尾且最近 N 天内修改过的文件。
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
