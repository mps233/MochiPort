import Foundation

/// Latest GitHub release payload; only the fields the update flow needs.
struct GitHubRelease: Decodable, Equatable {
    let tagName: String
    let htmlURL: URL
    let body: String?

    private enum CodingKeys: String, CodingKey {
        case tagName = "tag_name"
        case htmlURL = "html_url"
        case body
    }

    /// Only https links into this repository's releases pages are ever
    /// opened, so a compromised or spoofed API response cannot redirect the
    /// user elsewhere.
    var validatedURL: URL? {
        guard htmlURL.scheme == "https",
              htmlURL.host?.lowercased() == "github.com",
              htmlURL.path.lowercased().hasPrefix("/mps233/threadrelay/releases/")
        else {
            return nil
        }
        return htmlURL
    }
}

/// A newer release the app can offer to download.
struct AvailableUpdate: Equatable {
    let version: String
    let url: URL
}

/// Shared GitHub release check used by the Settings pane (interactive) and
/// the silent startup check.
enum UpdateChecker {
    static let latestReleaseAPI = URL(
        string: "https://api.github.com/repos/mps233/threadrelay/releases/latest"
    )!

    /// Fetches the latest release and rejects payloads whose download page
    /// does not validate. Throws on any network or validation failure.
    static func fetchLatestRelease(
        session: URLSession = .shared,
        currentVersion: String
    ) async throws -> GitHubRelease {
        var request = URLRequest(url: latestReleaseAPI)
        request.timeoutInterval = 10
        request.setValue("ThreadRelay/\(currentVersion)", forHTTPHeaderField: "User-Agent")
        request.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
        let (data, response) = try await session.data(for: request)
        guard let response = response as? HTTPURLResponse,
              response.statusCode == 200
        else {
            throw URLError(.badServerResponse)
        }
        let release = try JSONDecoder().decode(GitHubRelease.self, from: data)
        guard release.validatedURL != nil else {
            throw URLError(.unsupportedURL)
        }
        return release
    }

    /// Returns the update to surface, or `nil` when the current version is
    /// already the newest or the check failed. Never throws: the silent
    /// startup path treats every failure as "no update".
    static func availableUpdate(
        session: URLSession = .shared,
        currentVersion: String
    ) async -> AvailableUpdate? {
        guard let release = try? await fetchLatestRelease(
                  session: session,
                  currentVersion: currentVersion
              ),
              let url = release.validatedURL,
              isNewerVersion(release.tagName, than: currentVersion)
        else {
            return nil
        }
        return AvailableUpdate(version: release.tagName, url: url)
    }
}

private func versionComponents(_ version: String) -> [Int] {
    version
        .trimmingCharacters(in: CharacterSet(charactersIn: "vV"))
        .split(separator: "-", maxSplits: 1)
        .first?
        .split(separator: ".")
        .map { Int($0) ?? 0 }
        ?? []
}

func isNewerVersion(_ candidate: String, than current: String) -> Bool {
    let candidate = versionComponents(candidate)
    let current = versionComponents(current)
    let count = max(candidate.count, current.count)
    for index in 0..<count {
        let left = index < candidate.count ? candidate[index] : 0
        let right = index < current.count ? current[index] : 0
        if left != right { return left > right }
    }
    return false
}
