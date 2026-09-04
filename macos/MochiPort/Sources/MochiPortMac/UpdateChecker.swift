import Foundation

/// Metadata for one independently versioned MochiPort component. Update
/// manifests only inform the UI; installation stays on the release page.
struct UpdateComponentRelease: Decodable, Equatable, Sendable {
    let version: String
    let build: Int?
    let releaseURL: URL?
    let notes: String?
    private enum CodingKeys: String, CodingKey {
        case version
        case build
        case releaseURL = "releaseUrl"
        case notes
    }

    init(
        version: String,
        build: Int?,
        releaseURL: URL?,
        notes: String?
    ) {
        self.version = version
        self.build = build
        self.releaseURL = releaseURL
        self.notes = notes
    }

    var validatedReleaseURL: URL? {
        guard let releaseURL,
              releaseURL.scheme == "https",
              releaseURL.host?.lowercased() == "github.com",
              releaseURL.path.lowercased().hasPrefix("/mps233/mochiport/releases/")
        else {
            return nil
        }
        return releaseURL
    }

    func isNewer(thanVersion currentVersion: String, build currentBuild: Int?) -> Bool {
        isNewerComponentVersion(
            version,
            build: build,
            than: currentVersion,
            build: currentBuild
        )
    }
}

/// Version 2 contains independently published UI and daemon records. The
/// custom decoder also accepts the original top-level platform manifest as a
/// UI-only version 1 catalog.
struct UpdateManifest: Decodable, Equatable, Sendable {
    static let currentSchemaVersion = 2

    let schemaVersion: Int
    let ui: UpdateComponentRelease
    let daemon: UpdateComponentRelease?

    private enum CodingKeys: String, CodingKey {
        case schemaVersion
        case ui
        case daemon

        // Version 1 platform manifest keys.
        case version
        case build
        case releaseURL = "releaseUrl"
        case notes
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        if container.contains(.ui) {
            let schemaVersion = try container.decodeIfPresent(Int.self, forKey: .schemaVersion)
                ?? Self.currentSchemaVersion
            guard schemaVersion == Self.currentSchemaVersion else {
                throw DecodingError.dataCorruptedError(
                    forKey: .schemaVersion,
                    in: container,
                    debugDescription: "Unsupported update manifest schema version \(schemaVersion)"
                )
            }
            self.schemaVersion = schemaVersion
            self.ui = try container.decode(UpdateComponentRelease.self, forKey: .ui)
            self.daemon = try container.decodeIfPresent(UpdateComponentRelease.self, forKey: .daemon)
        } else {
            let schemaVersion = try container.decodeIfPresent(Int.self, forKey: .schemaVersion) ?? 1
            guard schemaVersion == 1 else {
                throw DecodingError.dataCorruptedError(
                    forKey: .schemaVersion,
                    in: container,
                    debugDescription: "Schema \(schemaVersion) requires a ui component"
                )
            }
            self.schemaVersion = schemaVersion
            self.ui = UpdateComponentRelease(
                version: try container.decode(String.self, forKey: .version),
                build: try container.decodeIfPresent(Int.self, forKey: .build),
                releaseURL: try container.decodeIfPresent(URL.self, forKey: .releaseURL),
                notes: try container.decodeIfPresent(String.self, forKey: .notes)
            )
            self.daemon = nil
        }

        try validate(container: container)
    }

    init(gitHubRelease release: GitHubRelease) {
        schemaVersion = 1
        ui = UpdateComponentRelease(
            version: release.tagName,
            build: nil,
            releaseURL: release.validatedURL,
            notes: release.body
        )
        daemon = nil
    }

    private func validate(container: KeyedDecodingContainer<CodingKeys>) throws {
        try validate(ui, key: .ui, container: container)
        if let daemon {
            guard schemaVersion == Self.currentSchemaVersion else {
                throw DecodingError.dataCorruptedError(
                    forKey: .daemon,
                    in: container,
                    debugDescription: "Daemon metadata requires schema version 2"
                )
            }
            guard daemon.build != nil else {
                throw DecodingError.dataCorruptedError(
                    forKey: .daemon,
                    in: container,
                    debugDescription: "Daemon metadata requires a build"
                )
            }
            try validate(daemon, key: .daemon, container: container)
        }
    }

    private func validate(
        _ release: UpdateComponentRelease,
        key: CodingKeys,
        container: KeyedDecodingContainer<CodingKeys>
    ) throws {
        guard !release.version.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              versionComparison(release.version, "0") != nil
        else {
            throw DecodingError.dataCorruptedError(
                forKey: key,
                in: container,
                debugDescription: "\(key.stringValue) has an invalid version"
            )
        }
        if let build = release.build, build < 0 {
            throw DecodingError.dataCorruptedError(
                forKey: key,
                in: container,
                debugDescription: "\(key.stringValue) has an invalid build"
            )
        }
        if release.releaseURL != nil, release.validatedReleaseURL == nil {
            throw URLError(.unsupportedURL)
        }
    }
}

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
              htmlURL.path.lowercased().hasPrefix("/mps233/mochiport/releases/")
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
        string: "https://api.github.com/repos/mps233/mochiport/releases/latest"
    )!
    static let latestMacOSManifestURL = URL(
        string: "https://github.com/mps233/mochiport/releases/latest/download/latest-macos.json"
    )!

    /// Fetches the component-aware manifest. Current platform manifests use
    /// this URL, while the decoder continues to understand their version 1
    /// top-level shape.
    static func fetchUpdateManifest(
        session: URLSession = .shared,
        currentVersion: String
    ) async throws -> UpdateManifest {
        var request = URLRequest(url: latestMacOSManifestURL)
        request.timeoutInterval = 10
        request.setValue("MochiPort/\(currentVersion)", forHTTPHeaderField: "User-Agent")
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        let (data, response) = try await session.data(for: request)
        guard let response = response as? HTTPURLResponse,
              response.statusCode == 200
        else {
            throw URLError(.badServerResponse)
        }
        return try JSONDecoder().decode(UpdateManifest.self, from: data)
    }

    /// Prefers the component manifest and falls back to the existing GitHub
    /// Releases API. The fallback intentionally exposes only a UI release;
    /// daemon release information is shown only when the manifest provides it.
    static func fetchLatestManifest(
        session: URLSession = .shared,
        currentVersion: String
    ) async throws -> UpdateManifest {
        do {
            return try await fetchUpdateManifest(session: session, currentVersion: currentVersion)
        } catch {
            let release = try await fetchLatestRelease(
                session: session,
                currentVersion: currentVersion
            )
            return UpdateManifest(gitHubRelease: release)
        }
    }

    /// Fetches the latest release and rejects payloads whose download page
    /// does not validate. Throws on any network or validation failure.
    static func fetchLatestRelease(
        session: URLSession = .shared,
        currentVersion: String
    ) async throws -> GitHubRelease {
        var request = URLRequest(url: latestReleaseAPI)
        request.timeoutInterval = 10
        request.setValue("MochiPort/\(currentVersion)", forHTTPHeaderField: "User-Agent")
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

private enum PrereleaseIdentifier: Equatable {
    case numeric(Int)
    case text(String)
}

private struct ParsedVersion: Equatable {
    let core: [Int]
    let prerelease: [PrereleaseIdentifier]?
}

private func parsedVersion(_ version: String) -> ParsedVersion? {
    var normalized = version.trimmingCharacters(in: .whitespacesAndNewlines)
    if normalized.first == "v" || normalized.first == "V" {
        normalized.removeFirst()
    }
    guard !normalized.isEmpty else { return nil }

    let buildParts = normalized.split(
        separator: "+",
        maxSplits: 1,
        omittingEmptySubsequences: false
    )
    guard !buildParts[0].isEmpty,
          buildParts.count == 1 || !buildParts[1].isEmpty
    else { return nil }
    let releaseParts = buildParts[0].split(
        separator: "-",
        maxSplits: 1,
        omittingEmptySubsequences: false
    )
    let rawCore = releaseParts[0].split(separator: ".", omittingEmptySubsequences: false)
    guard !rawCore.isEmpty,
          rawCore.allSatisfy({ !$0.isEmpty }),
          rawCore.allSatisfy({ $0.allSatisfy(\.isNumber) }),
          rawCore.allSatisfy({ Int($0) != nil })
    else { return nil }

    let prerelease: [PrereleaseIdentifier]?
    if releaseParts.count == 2 {
        let identifiers = releaseParts[1].split(separator: ".", omittingEmptySubsequences: false)
        guard !identifiers.isEmpty,
              identifiers.allSatisfy({ !$0.isEmpty }),
              identifiers.allSatisfy({ identifier in
                  identifier.unicodeScalars.allSatisfy {
                      (48...57).contains($0.value)
                          || (65...90).contains($0.value)
                          || (97...122).contains($0.value)
                          || $0.value == 45
                  }
              })
        else { return nil }
        prerelease = identifiers.map { identifier in
            if identifier.allSatisfy(\.isNumber), let number = Int(identifier) {
                return .numeric(number)
            }
            return .text(String(identifier))
        }
    } else {
        prerelease = nil
    }
    return ParsedVersion(core: rawCore.compactMap { Int($0) }, prerelease: prerelease)
}

private func versionComparison(_ lhs: String, _ rhs: String) -> ComparisonResult? {
    guard let lhs = parsedVersion(lhs), let rhs = parsedVersion(rhs) else { return nil }
    let count = max(lhs.core.count, rhs.core.count)
    for index in 0..<count {
        let left = index < lhs.core.count ? lhs.core[index] : 0
        let right = index < rhs.core.count ? rhs.core[index] : 0
        if left != right { return left > right ? .orderedDescending : .orderedAscending }
    }
    switch (lhs.prerelease, rhs.prerelease) {
    case (nil, nil):
        return .orderedSame
    case (nil, .some):
        return .orderedDescending
    case (.some, nil):
        return .orderedAscending
    case let (.some(left), .some(right)):
        for index in 0..<min(left.count, right.count) {
            switch (left[index], right[index]) {
            case let (.numeric(a), .numeric(b)) where a != b:
                return a > b ? .orderedDescending : .orderedAscending
            case (.numeric, .text):
                return .orderedAscending
            case (.text, .numeric):
                return .orderedDescending
            case let (.text(a), .text(b)) where a != b:
                return a.compare(b, options: .literal) == .orderedDescending
                    ? .orderedDescending
                    : .orderedAscending
            default:
                continue
            }
        }
        if left.count != right.count {
            return left.count > right.count ? .orderedDescending : .orderedAscending
        }
        return .orderedSame
    }
}

func isNewerVersion(_ candidate: String, than current: String) -> Bool {
    versionComparison(candidate, current) == .orderedDescending
}

func isNewerComponentVersion(
    _ candidateVersion: String,
    build candidateBuild: Int?,
    than currentVersion: String,
    build currentBuild: Int?
) -> Bool {
    guard let comparison = versionComparison(candidateVersion, currentVersion) else { return false }
    if comparison != .orderedSame {
        return comparison == .orderedDescending
    }
    guard let candidateBuild, let currentBuild else { return false }
    return candidateBuild > currentBuild
}
