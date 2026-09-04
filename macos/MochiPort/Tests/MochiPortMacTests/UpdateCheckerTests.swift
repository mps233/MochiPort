import Foundation
import XCTest

#if canImport(MochiPortMac)
@testable import MochiPortMac
#elseif canImport(MochiPort)
@testable import MochiPort
#endif

final class UpdateCheckerManifestTests: XCTestCase {
    override func tearDown() {
        UpdateManifestURLProtocol.reset()
        super.tearDown()
    }

    func testVersionTwoManifestDecodesIndependentUIAndDaemonReleases() throws {
        let digest = String(repeating: "a", count: 64)
        let manifest = try JSONDecoder().decode(
            UpdateManifest.self,
            from: Data(
                """
                {
                  "schemaVersion": 2,
                  "ui": {
                    "version": "0.5.2",
                    "build": 444,
                    "releaseUrl": "https://github.com/mps233/mochiport/releases/tag/ui-v0.5.2",
                    "notes": "UI notes",
                    "assets": {
                      "macos-universal": {
                        "type": "dmg",
                        "url": "https://github.com/mps233/mochiport/releases/download/ui-v0.5.2/MochiPort.dmg",
                        "sha256": "\(digest)",
                        "size": 123,
                        "signed": true,
                        "notarized": true
                      }
                    }
                  },
                  "daemon": {
                    "version": "0.5.4",
                    "build": 451,
                    "apiMajor": 1,
                    "minimumUIVersion": "0.5.2",
                    "minimumUIBuild": 440,
                    "releaseUrl": "https://github.com/mps233/mochiport/releases/tag/daemon-v0.5.4",
                    "notes": "Daemon notes",
                    "assets": {
                      "macos-daemon-universal": {
                        "type": "executable",
                        "url": "https://github.com/mps233/mochiport/releases/download/daemon-v0.5.4/mochiport-daemon",
                        "sha256": "\(digest)",
                        "size": 456,
                        "signed": true,
                        "notarized": true
                      }
                    }
                  }
                }
                """.utf8
            )
        )

        XCTAssertEqual(manifest.schemaVersion, 2)
        XCTAssertEqual(manifest.ui.version, "0.5.2")
        XCTAssertEqual(manifest.ui.build, 444)
        XCTAssertEqual(manifest.daemon?.version, "0.5.4")
        XCTAssertEqual(manifest.daemon?.build, 451)
    }

    func testLegacyPlatformManifestBecomesUIOnlyCatalog() throws {
        let manifest = try JSONDecoder().decode(
            UpdateManifest.self,
            from: Data(
                """
                {
                  "version": "v0.5.2",
                  "releaseUrl": "https://github.com/mps233/mochiport/releases/tag/v0.5.2",
                  "notes": "Legacy notes",
                  "assets": {
                    "macos-universal": {
                      "type": "dmg",
                      "url": "https://github.com/mps233/mochiport/releases/download/v0.5.2/MochiPort.dmg"
                    }
                  }
                }
                """.utf8
            )
        )

        XCTAssertEqual(manifest.schemaVersion, 1)
        XCTAssertEqual(manifest.ui.version, "v0.5.2")
        XCTAssertNil(manifest.ui.build)
        XCTAssertEqual(manifest.ui.notes, "Legacy notes")
        XCTAssertNil(manifest.daemon)
    }

    func testComponentVersionComparisonUsesSemanticVersionThenBuild() {
        XCTAssertTrue(isNewerVersion("1.0.0", than: "1.0.0-rc.1"))
        XCTAssertTrue(isNewerVersion("1.0.0-rc.2", than: "1.0.0-rc.1"))
        XCTAssertFalse(isNewerVersion("1.0.0-beta", than: "1.0.0"))
        XCTAssertFalse(isNewerVersion("1.not-a-number", than: "1.0.0"))

        XCTAssertTrue(
            isNewerComponentVersion("0.5.2", build: 445, than: "0.5.2", build: 444)
        )
        XCTAssertFalse(
            isNewerComponentVersion("0.5.2", build: 443, than: "0.5.2", build: 444)
        )
        XCTAssertFalse(
            isNewerComponentVersion("0.5.1", build: 999, than: "0.5.2", build: 444)
        )
    }

    func testManifestRejectsForeignReleasePage() {
        XCTAssertThrowsError(
            try JSONDecoder().decode(
                UpdateManifest.self,
                from: Data(
                    """
                    {
                      "schemaVersion":2,
                      "ui": {
                        "version":"0.5.2",
                        "build":444,
                        "releaseUrl":"https://evil.example/MochiPort.dmg"
                      }
                    }
                    """.utf8
                )
            )
        ) { error in
            XCTAssertEqual((error as? URLError)?.code, .unsupportedURL)
        }
    }

    func testComponentManifestFallsBackToGitHubReleaseAPIAsUIOnly() async throws {
        let requestedHosts = LockedStrings()
        UpdateManifestURLProtocol.install { request in
            requestedHosts.append(request.url?.host ?? "")
            if request.url?.host == "github.com" {
                return ManifestMockResponse(statusCode: 404, json: #"{"error":"missing"}"#)
            }
            return ManifestMockResponse(
                statusCode: 200,
                json: #"{"tag_name":"v0.6.0","html_url":"https://github.com/mps233/mochiport/releases/tag/v0.6.0","body":"notes"}"#
            )
        }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [UpdateManifestURLProtocol.self]
        let session = URLSession(configuration: configuration)

        let manifest = try await UpdateChecker.fetchLatestManifest(
            session: session,
            currentVersion: "0.5.2"
        )

        XCTAssertEqual(requestedHosts.values, ["github.com", "api.github.com"])
        XCTAssertEqual(manifest.schemaVersion, 1)
        XCTAssertEqual(manifest.ui.version, "v0.6.0")
        XCTAssertNil(manifest.daemon)
    }
}

private struct ManifestMockResponse: Sendable {
    let statusCode: Int
    let body: Data

    init(statusCode: Int, json: String) {
        self.statusCode = statusCode
        body = Data(json.utf8)
    }
}

private final class LockedStrings: @unchecked Sendable {
    private let lock = NSLock()
    private var storage: [String] = []

    var values: [String] {
        lock.withLock { storage }
    }

    func append(_ value: String) {
        lock.withLock { storage.append(value) }
    }
}

private final class UpdateManifestURLProtocol: URLProtocol, @unchecked Sendable {
    typealias Handler = @Sendable (URLRequest) -> ManifestMockResponse

    private static let lock = NSLock()
    nonisolated(unsafe) private static var handler: Handler?

    static func install(_ handler: @escaping Handler) {
        lock.withLock { self.handler = handler }
    }

    static func reset() {
        lock.withLock { handler = nil }
    }

    override class func canInit(with _: URLRequest) -> Bool {
        true
    }

    override class func canonicalRequest(for request: URLRequest) -> URLRequest {
        request
    }

    override func startLoading() {
        guard let handler = Self.lock.withLock({ Self.handler }),
              let url = request.url
        else {
            client?.urlProtocol(self, didFailWithError: URLError(.badServerResponse))
            return
        }
        let mock = handler(request)
        let response = HTTPURLResponse(
            url: url,
            statusCode: mock.statusCode,
            httpVersion: nil,
            headerFields: ["Content-Type": "application/json"]
        )!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: mock.body)
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}
}
