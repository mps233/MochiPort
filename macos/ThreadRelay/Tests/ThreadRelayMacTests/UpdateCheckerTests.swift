import Foundation
import XCTest

#if canImport(ThreadRelayMac)
@testable import ThreadRelayMac
#elseif canImport(ThreadRelay)
@testable import ThreadRelay
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
        XCTAssertEqual(manifest.ui.assets["macos-universal"]?.normalizedSHA256, digest)
        XCTAssertEqual(manifest.daemon?.version, "0.5.4")
        XCTAssertEqual(manifest.daemon?.build, 451)
        XCTAssertEqual(manifest.daemon?.apiMajor, 1)
        XCTAssertEqual(manifest.daemon?.minimumUIVersion, "0.5.2")
        XCTAssertEqual(manifest.daemon?.minimumUIBuild, 440)
        XCTAssertEqual(
            manifest.daemon?.assets["macos-daemon-universal"]?.assetType,
            "executable"
        )
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

    func testAvailableUpdatesAndDaemonCompatibilityAreIndependent() throws {
        let manifest = try JSONDecoder().decode(
            UpdateManifest.self,
            from: Data(
                """
                {
                  "schemaVersion": 2,
                  "ui": {"version":"0.5.2","build":444,"assets":{}},
                  "daemon": {
                    "version":"0.5.4",
                    "build":451,
                    "apiMajor":1,
                    "minimumUIVersion":"0.5.2",
                    "minimumUIBuild":444,
                    "assets":{}
                  }
                }
                """.utf8
            )
        )

        let compatible = manifest.availableUpdates(
            currentUIVersion: "0.5.2",
            currentUIBuild: 444,
            currentDaemonVersion: "0.5.3",
            currentDaemonBuild: 449,
            supportedDaemonAPIMajor: 1
        )
        XCTAssertNil(compatible.ui)
        XCTAssertEqual(compatible.daemon?.version, "0.5.4")
        XCTAssertEqual(compatible.daemonCompatibility, .compatible)

        let oldUI = manifest.availableUpdates(
            currentUIVersion: "0.5.2",
            currentUIBuild: 443,
            currentDaemonVersion: "0.5.3",
            currentDaemonBuild: 449,
            supportedDaemonAPIMajor: 1
        )
        XCTAssertEqual(
            oldUI.daemonCompatibility,
            .requiresUIUpdate(minimumVersion: "0.5.2", minimumBuild: 444)
        )

        let unsupportedAPI = manifest.availableUpdates(
            currentUIVersion: "0.5.2",
            currentUIBuild: 444,
            currentDaemonVersion: "0.5.3",
            currentDaemonBuild: 449,
            supportedDaemonAPIMajor: 2
        )
        XCTAssertEqual(
            unsupportedAPI.daemonCompatibility,
            .unsupportedAPIMajor(required: 1)
        )
    }

    func testManifestRejectsForeignAssetsAndMalformedDigests() {
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
                        "assets": {
                          "macos-universal": {
                            "url":"https://evil.example/MochiPort.dmg"
                          }
                        }
                      }
                    }
                    """.utf8
                )
            )
        ) { error in
            XCTAssertEqual((error as? URLError)?.code, .unsupportedURL)
        }

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
                        "assets": {
                          "macos-universal": {
                            "url":"https://github.com/mps233/mochiport/releases/download/v0.5.2/MochiPort.dmg",
                            "sha256":"too-short"
                          }
                        }
                      }
                    }
                    """.utf8
                )
            )
        )
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

final class DaemonUpdateRecoveryTests: XCTestCase {
    func testDaemonReplacementMatchesStableCurrentSymlink() throws {
        let fileManager = FileManager.default
        let root = fileManager.temporaryDirectory
            .appendingPathComponent("mochiport-replacement-" + UUID().uuidString, isDirectory: true)
        let runtimes = root.appendingPathComponent("runtimes", isDirectory: true)
        let buildDirectory = runtimes.appendingPathComponent("451", isDirectory: true)
        let candidate = buildDirectory.appendingPathComponent("mochiport-daemon")
        let current = runtimes.appendingPathComponent("current", isDirectory: true)
        try fileManager.createDirectory(at: buildDirectory, withIntermediateDirectories: true)
        try Data("candidate".utf8).write(to: candidate)
        try fileManager.createSymbolicLink(at: current, withDestinationURL: buildDirectory)
        defer { try? fileManager.removeItem(at: root) }

        let lifecycle = ManageLifecycle(
            service: .init(
                service: "mochiport",
                apiMajor: 1,
                ready: true,
                instanceId: "new-instance",
                pid: 456,
                startedAtMs: 789
            ),
            executable: current.appendingPathComponent("mochiport-daemon").path,
            configPath: root.appendingPathComponent("config.toml").path,
            bind: "127.0.0.1:3847",
            runtime: .init(
                state: "active",
                productVersion: "0.5.1",
                buildNumber: 451,
                apiMajor: 1
            ),
            protectedWorkItems: .init(
                aiGatewayRequests: 0,
                codexTurns: 0,
                imStreams: 0,
                pendingApprovals: 0,
                remoteControlRequests: 0,
                total: 0
            ),
            management: .init(
                state: "managed",
                mode: "managed",
                canControl: true,
                installationId: "installation",
                leaseGeneration: 1,
                leaseExpiresAtMs: 9_999_999_999
            )
        )

        XCTAssertTrue(
            AppModel.daemonReplacementMatches(
                lifecycle,
                previousInstanceId: "old-instance",
                expectedBuild: 451,
                expectedExecutable: candidate.path
            )
        )
    }

    func testAcceptedUpdateRecoveryKeepsStableLaunchPathAndBootstraps() async throws {
        let fileManager = FileManager.default
        let home = fileManager.temporaryDirectory
            .appendingPathComponent(
                "mochiport-update-recovery-" + UUID().uuidString,
                isDirectory: true
            )
        let dataDirectory = home.appendingPathComponent("MochiPort", isDirectory: true)
        let launchAgentURL = home
            .appendingPathComponent("Library/LaunchAgents", isDirectory: true)
            .appendingPathComponent(DaemonLaunchConfiguration.label + ".plist")
        try fileManager.createDirectory(
            at: launchAgentURL.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        try fileManager.createDirectory(at: dataDirectory, withIntermediateDirectories: true)
        defer { try? fileManager.removeItem(at: home) }

        let configuration = DaemonLaunchConfiguration(
            helperURL: URL(fileURLWithPath: "/Applications/MochiPort.app/Contents/Helpers/mochiport-daemon"),
            configURL: dataDirectory.appendingPathComponent("config.toml"),
            launchAgentURL: launchAgentURL,
            logURL: dataDirectory.appendingPathComponent("logs/daemon.log"),
            homeURL: home,
            buildIdentifier: "440"
        )
        let oldExecutable = dataDirectory
            .appendingPathComponent("runtimes/440/mochiport-daemon")
        let originalPropertyList: [String: Any] = [
            "Label": configuration.launchdLabel,
            "ProgramArguments": [
                oldExecutable.path,
                "--config",
                configuration.configURL.path,
                "daemon",
            ],
        ]
        let originalData = try PropertyListSerialization.data(
            fromPropertyList: originalPropertyList,
            format: .xml,
            options: 0
        )
        try originalData.write(to: launchAgentURL, options: .atomic)

        let target = configuration.launchdServiceTarget
        let calls = LaunchctlCallRecorder()
        let processChecks = ProcessCheckerSequence([true, false, false])
        let installer = DaemonUpdateInstaller(
            configurationLoader: { configuration },
            commandRunner: { executable, arguments in
                calls.append(arguments)
                guard executable.path == "/bin/launchctl" else {
                    return CommandResult(exitCode: 1, output: "unexpected executable")
                }
                if arguments == ["print", target] {
                    let printCount = calls.count(for: ["print", target])
                    if printCount >= 4 {
                        return CommandResult(
                            exitCode: 0,
                            output: "state = running\npid = 456\nprogram = \(configuration.activeHelperURL().path)\n"
                        )
                    }
                    return CommandResult(
                        exitCode: 0,
                        output: "state = running\npid = 123\nprogram = \(oldExecutable.path)\n"
                    )
                }
                return CommandResult(exitCode: 0, output: "")
            },
            processChecker: { _ in processChecks.next() }
        )
        let lifecycle = ManageLifecycle(
            service: .init(
                service: "threadrelay",
                apiMajor: 1,
                ready: true,
                instanceId: "old-instance",
                pid: 123,
                startedAtMs: 456
            ),
            executable: oldExecutable.path,
            configPath: configuration.configURL.path,
            bind: "127.0.0.1:3847",
            runtime: .init(
                state: "active",
                productVersion: "0.5.0",
                buildNumber: 440,
                apiMajor: 1
            ),
            protectedWorkItems: .init(
                aiGatewayRequests: 0,
                codexTurns: 0,
                imStreams: 0,
                pendingApprovals: 0,
                remoteControlRequests: 0,
                total: 0
            ),
            management: .init(
                state: "managed",
                mode: "managed",
                canControl: true,
                installationId: "installation",
                leaseGeneration: 1,
                leaseExpiresAtMs: 9_999_999_999
            )
        )
        let candidate = PreparedDaemonUpdate(
            version: "0.5.1",
            build: 451,
            sha256: String(repeating: "a", count: 64),
            executableURL: dataDirectory
                .appendingPathComponent("runtimes/451/mochiport-daemon")
        )

        let plan = try await installer.prepareLaunchAgentMigration(
            lifecycle: lifecycle,
            candidate: candidate
        )
        await installer.recoverLaunchAgentMigration(plan, previousPID: 123)
        await installer.recoverLaunchAgentMigration(plan, previousPID: 123)

        let recovered = try Data(contentsOf: launchAgentURL)
        let recoveredPropertyList = try XCTUnwrap(
            try PropertyListSerialization.propertyList(
                from: recovered,
                options: [],
                format: nil
            ) as? [String: Any]
        )
        let arguments = try XCTUnwrap(recoveredPropertyList["ProgramArguments"] as? [String])
        XCTAssertEqual(arguments.first, configuration.activeHelperURL().path)
        XCTAssertEqual(
            calls.values,
            [
                ["print", target],
                ["disable", target],
                ["print", target],
                ["print", target],
                ["bootout", target],
                ["enable", target],
                ["bootstrap", "gui/\(getuid())", launchAgentURL.path],
                ["print", target],
            ]
        )
    }
}

private final class ProcessCheckerSequence: @unchecked Sendable {
    private let lock = NSLock()
    private var values: [Bool]

    init(_ values: [Bool]) {
        self.values = values
    }

    func next() -> Bool {
        lock.withLock {
            values.isEmpty ? false : values.removeFirst()
        }
    }
}

private final class LaunchctlCallRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var storage: [[String]] = []

    var values: [[String]] { lock.withLock { storage } }

    func append(_ arguments: [String]) {
        lock.withLock { storage.append(arguments) }
    }

    func count(for arguments: [String]) -> Int {
        lock.withLock { storage.filter { $0 == arguments }.count }
    }
}
