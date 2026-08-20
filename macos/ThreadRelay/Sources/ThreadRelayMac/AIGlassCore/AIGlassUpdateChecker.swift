import Foundation

/// ThreadRelay GitHub Releases 기반 새 버전 확인.
///
/// 하루 1회 `releases/latest`를 조회해 현재 버전보다 높으면 알림/배지로 안내한다.
/// 자동 설치는 하지 않는다 — 클릭 시 릴리스 페이지를 열 뿐 (Sparkle 없이 최소 구현).
public enum AIGlassUpdateChecker {
    public struct Release: Equatable, Sendable {
        /// "0.11.0" — 태그의 "v" 접두사를 제거한 버전.
        public let version: String
        /// 릴리스 페이지 (html_url).
        public let url: URL

        public init(version: String, url: URL) {
            self.version = version
            self.url = url
        }
    }

    public static let latestReleaseURL =
        URL(string: "https://api.github.com/repos/mps233/threadrelay/releases/latest")!

    /// `releases/latest` 응답 JSON에서 버전·페이지 URL을 뽑는다. 형식이 다르면 nil.
    public static func parseLatestRelease(jsonData: Data) -> Release? {
        guard let obj = try? JSONSerialization.jsonObject(with: jsonData) as? [String: Any],
              let tag = obj["tag_name"] as? String,
              let urlString = obj["html_url"] as? String,
              let url = URL(string: urlString) else { return nil }
        let version = tag.hasPrefix("v") ? String(tag.dropFirst()) : tag
        guard !version.isEmpty else { return nil }
        return Release(version: version, url: url)
    }

    /// semver 자리수별 숫자 비교 — candidate가 current보다 높으면 true.
    /// 자리수가 모자라면 0으로 채움 ("0.10" == "0.10.0"). 숫자 아닌 조각은 0 취급.
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

    /// 최신 릴리스를 조회한다. 네트워크/파싱 실패 시 nil (조용히 다음 주기).
    public static func fetchLatest() async -> Release? {
        var request = URLRequest(url: latestReleaseURL)
        request.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
        request.timeoutInterval = 15
        guard let (data, response) = try? await URLSession.shared.data(for: request),
              (response as? HTTPURLResponse)?.statusCode == 200 else { return nil }
        return parseLatestRelease(jsonData: data)
    }
}
