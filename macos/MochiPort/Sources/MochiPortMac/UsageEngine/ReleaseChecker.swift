import Foundation

/// 基于 MochiPort GitHub Releases 的版本检查。
///
/// 每天查询一次 `releases/latest`，若高于当前版本则以通知/徽标提示。
/// 不自动安装——点击只是打开 releases 页面（无 Sparkle 的最小实现）。
public enum ReleaseChecker {
    public struct Release: Equatable, Sendable {
        /// "0.11.0"——去掉 tag 的 "v" 前缀后的版本号。
        public let version: String
        /// 发布页（html_url）。
        public let url: URL

        public init(version: String, url: URL) {
            self.version = version
            self.url = url
        }
    }

    public static let latestReleaseURL =
        URL(string: "https://api.github.com/repos/mps233/mochiport/releases/latest")!

    /// 从 `releases/latest` 的 JSON 中提取版本号与页面 URL。格式不符返回 nil。
    public static func parseLatestRelease(jsonData: Data) -> Release? {
        guard let obj = try? JSONSerialization.jsonObject(with: jsonData) as? [String: Any],
              let tag = obj["tag_name"] as? String,
              let urlString = obj["html_url"] as? String,
              let url = URL(string: urlString),
              url.scheme?.lowercased() == "https",
              url.host?.lowercased() == "github.com",
              url.path.lowercased().hasPrefix("/mps233/mochiport/releases/") else { return nil }
        let version = tag.hasPrefix("v") ? String(tag.dropFirst()) : tag
        guard !version.isEmpty else { return nil }
        return Release(version: version, url: url)
    }

    /// 按语义化版本逐位数字比较——candidate 高于 current 时返回 true。
    /// 位数不足补 0（"0.10" == "0.10.0"），非数字片段按 0 处理。
    public static func isNewer(_ candidate: String, than current: String) -> Bool {
        let a = candidate.split(separator: ".").map { Int($0) ?? 0 }
        let b = current.split(separator: ".").map { Int($0) ?? 0 }
        for i in 0..<max(a.count, b.count) {
            let x = i < a.count ? a[i] : 0
            let y = i < b.count ? b[i] : 0
            if x != y { return x > y }
        }
        return false
    }

    /// 查询最新 release。网络/解析失败返回 nil（静默进入下一个周期）。
    public static func fetchLatest() async -> Release? {
        var request = URLRequest(url: latestReleaseURL)
        request.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
        request.timeoutInterval = 15
        guard let (data, response) = try? await URLSession.shared.data(for: request),
              (response as? HTTPURLResponse)?.statusCode == 200 else { return nil }
        return parseLatestRelease(jsonData: data)
    }
}
