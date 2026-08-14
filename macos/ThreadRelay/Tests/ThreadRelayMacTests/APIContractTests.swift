import Foundation
import XCTest

#if canImport(ThreadRelayMac)
@testable import ThreadRelayMac
#elseif canImport(ThreadRelay)
@testable import ThreadRelay
#endif

final class APIContractTests: XCTestCase {
    override func tearDown() {
        MockURLProtocol.reset()
        super.tearDown()
    }

    func testHealthFixtureDecodes() throws {
        let data = Data(#"{"service":"threadrelay","apiMajor":1,"ready":true}"#.utf8)
        let health = try JSONDecoder().decode(HealthResponse.self, from: data)

        XCTAssertEqual(
            health,
            HealthResponse(service: "threadrelay", apiMajor: 1, ready: true)
        )
    }

    func testManualUpdateVersionComparisonHandlesTagsAndUnevenComponents() {
        XCTAssertTrue(isNewerVersion("v0.5.1", than: "0.5.0"))
        XCTAssertTrue(isNewerVersion("0.6", than: "0.5.9"))
        XCTAssertFalse(isNewerVersion("v0.5.0", than: "0.5"))
        XCTAssertFalse(isNewerVersion("0.4.9", than: "0.5.0"))
    }

    func testSingleInstanceGuardRejectsSecondOwner() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        let lockURL = root.appendingPathComponent("threadrelay.gui.lock")
        defer { try? FileManager.default.removeItem(at: root) }

        do {
            let first = try SingleInstanceGuard.acquire(lockURL: lockURL)
            XCTAssertThrowsError(try SingleInstanceGuard.acquire(lockURL: lockURL)) { error in
                XCTAssertEqual(error as? SingleInstanceError, .alreadyRunning)
            }
            withExtendedLifetime(first) {}
        }
    }

    func testSingleInstanceGuardReleasesLockOnDeallocation() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        let lockURL = root.appendingPathComponent("threadrelay.gui.lock")
        defer { try? FileManager.default.removeItem(at: root) }

        var first: SingleInstanceGuard? = try SingleInstanceGuard.acquire(lockURL: lockURL)
        withExtendedLifetime(first) {}
        first = nil
        XCTAssertNoThrow(try SingleInstanceGuard.acquire(lockURL: lockURL))
    }

    func testSingleInstanceLockPathIsScopedToBundleIdentifier() {
        let home = URL(fileURLWithPath: "/fixture/home", isDirectory: true)
        XCTAssertEqual(
            SingleInstanceGuard.defaultLockURL(
                bundleIdentifier: "io.github.mps233.threadrelay.preview",
                environment: ["HOME": home.path]
            ).path,
            "/fixture/home/Library/Application Support/ThreadRelay/io.github.mps233.threadrelay.preview.gui.lock"
        )
    }

    func testManagementCredentialPathsHonorBothHomeOverridesBeforeDefaults() {
        let applicationSupport = URL(fileURLWithPath: "/fixture/Application Support", isDirectory: true)

        XCTAssertEqual(
            ManagementCredentialStore.candidatePaths(
                environment: [
                    "THREADRELAY_HOME": "/fixture/threadrelay",
                    "CODEXHUB_HOME": "/fixture/codexhub",
                ],
                applicationSupport: applicationSupport
            ).map(\.path),
            [
                "/fixture/threadrelay/threadrelay-control.json",
                "/fixture/codexhub/threadrelay-control.json",
                "/fixture/Application Support/ThreadRelay/threadrelay-control.json",
                "/fixture/Application Support/CodexHub/threadrelay-control.json",
            ]
        )
    }

    func testDaemonLaunchConfigurationUsesLegacyDataAndEmbeddedHelper() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        let home = root.appendingPathComponent("home", isDirectory: true)
        let appSupport = home.appendingPathComponent("Library/Application Support", isDirectory: true)
        let legacyDirectory = appSupport.appendingPathComponent("CodexHub", isDirectory: true)
        let bundle = root.appendingPathComponent("ThreadRelay.app", isDirectory: true)
        try FileManager.default.createDirectory(at: legacyDirectory, withIntermediateDirectories: true)
        try Data().write(to: legacyDirectory.appendingPathComponent("config.toml"))
        defer { try? FileManager.default.removeItem(at: root) }

        let configuration = try DaemonLaunchConfiguration.current(
            bundleURL: bundle,
            environment: ["HOME": home.path],
            fileManager: .default
        )

        XCTAssertEqual(
            configuration.helperURL.path,
            bundle.appendingPathComponent("Contents/Helpers/threadrelay-daemon").path
        )
        XCTAssertEqual(
            configuration.configURL.path,
            legacyDirectory.appendingPathComponent("config.toml").path
        )
        XCTAssertEqual(
            configuration.launchAgentURL.path,
            home.appendingPathComponent("Library/LaunchAgents/\(DaemonLaunchConfiguration.label).plist").path
        )
    }

    func testDaemonLauncherWakesLoadedServiceWithoutForcingRestart() async throws {
        let fixture = try makeDaemonLauncherFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let commands = CommandInvocationRecorder { arguments in
            if arguments.first == "print" {
                return CommandResult(
                    exitCode: 0,
                    output: """
                    state = running
                    program = \(fixture.configuration.helperURL.path)
                    arguments = {
                        \(fixture.configuration.helperURL.path)
                        --config
                        \(fixture.configuration.configURL.path)
                        daemon
                    }
                    """
                )
            }
            return CommandResult(exitCode: arguments.first == "kickstart" ? 0 : 1, output: "")
        }
        let launcher = DaemonLauncher(
            configurationLoader: { fixture.configuration },
            commandRunner: commands.run
        )

        try await launcher.startIfNeeded()

        XCTAssertEqual(commands.arguments, [
            ["print", "gui/\(getuid())/\(DaemonLaunchConfiguration.label)"],
            ["kickstart", "gui/\(getuid())/\(DaemonLaunchConfiguration.label)"],
        ])
        XCTAssertFalse(commands.arguments.flatMap { $0 }.contains("-k"))
        XCTAssertFalse(FileManager.default.fileExists(atPath: fixture.configuration.launchAgentURL.path))
    }

    func testDaemonLauncherRejectsLoadedServiceFromDifferentHelper() async throws {
        let fixture = try makeDaemonLauncherFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let staleHelper = fixture.root.appendingPathComponent("OldThreadRelay.app/Contents/Helpers/threadrelay-daemon")
        let commands = CommandInvocationRecorder { arguments in
            guard arguments.first == "print" else {
                return CommandResult(exitCode: 1, output: "unexpected command")
            }
            return CommandResult(
                exitCode: 0,
                output: """
                state = running
                program = \(staleHelper.path)
                arguments = {
                    \(staleHelper.path)
                    --config
                    \(fixture.configuration.configURL.path)
                    daemon
                }
                """
            )
        }
        let launcher = DaemonLauncher(
            configurationLoader: { fixture.configuration },
            commandRunner: commands.run
        )

        do {
            try await launcher.startIfNeeded()
            XCTFail("Expected stale launch agent to be rejected")
        } catch let error as DaemonLaunchError {
            XCTAssertEqual(
                error,
                .loadedAgentMismatch(expected: fixture.configuration.helperURL.path, actual: staleHelper.path)
            )
        }
        XCTAssertEqual(commands.arguments.map(\.first), ["print"])
    }

    func testDaemonLauncherBootstrapsMissingService() async throws {
        let fixture = try makeDaemonLauncherFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let commands = CommandInvocationRecorder { arguments in
            CommandResult(exitCode: arguments.first == "bootstrap" ? 0 : 1, output: "")
        }
        let launcher = DaemonLauncher(
            configurationLoader: { fixture.configuration },
            commandRunner: commands.run
        )

        try await launcher.startIfNeeded()

        XCTAssertEqual(commands.arguments.map(\.first), ["print", "bootstrap"])
        let plistData = try Data(contentsOf: fixture.configuration.launchAgentURL)
        let plist = try XCTUnwrap(
            PropertyListSerialization.propertyList(from: plistData, options: [], format: nil)
                as? [String: Any]
        )
        XCTAssertEqual(plist["Label"] as? String, DaemonLaunchConfiguration.label)
        XCTAssertEqual(
            plist["ProgramArguments"] as? [String],
            [
                fixture.configuration.helperURL.path,
                "--config",
                fixture.configuration.configURL.path,
                "daemon",
            ]
        )
    }

    func testManagementCredentialStoreLoadsAllValidUniqueCandidatesInPathOrder() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }

        let stale = directory.appendingPathComponent("stale.json")
        let duplicate = directory.appendingPathComponent("duplicate.json")
        let current = directory.appendingPathComponent("current.json")
        let invalid = directory.appendingPathComponent("invalid.json")
        try Data(#"{"managementToken":"stale-token"}"#.utf8).write(to: stale)
        try Data(#"{"managementToken":"stale-token"}"#.utf8).write(to: duplicate)
        try Data(#"{"managementToken":"current-token"}"#.utf8).write(to: current)
        try Data(#"{"managementToken":" has-whitespace "}"#.utf8).write(to: invalid)

        XCTAssertEqual(
            ManagementCredentialStore.loadCandidates(
                from: [stale, duplicate, invalid, current]
            ),
            ["stale-token", "current-token"]
        )
    }

    func testManagementCredentialStorePrefersActiveDaemonLocator() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }

        let activeControl = directory.appendingPathComponent("active-control.json")
        let fallbackControl = directory.appendingPathComponent("fallback-control.json")
        try Data(#"{"managementToken":"active-token"}"#.utf8).write(to: activeControl)
        try Data(#"{"managementToken":"fallback-token"}"#.utf8).write(to: fallbackControl)
        let locator = ActiveDaemonLocator(
            service: "threadrelay",
            apiMajor: 1,
            instanceId: "active-instance",
            pid: 123,
            startedAtMs: 456,
            baseURL: "http://127.0.0.1:3847",
            controlFile: activeControl.path
        )

        XCTAssertEqual(
            ManagementCredentialStore.loadCredentialCandidates(
                locator: locator,
                fallbackPaths: [fallbackControl]
            ),
            [
                .init(token: "active-token", expectedInstanceId: "active-instance"),
                .init(token: "fallback-token", expectedInstanceId: nil),
            ]
        )
    }

    func testManagementCredentialStoreKeepsUnconstrainedFallbackForLocatorToken() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }

        let control = directory.appendingPathComponent("control.json")
        try Data(#"{"managementToken":"shared-token"}"#.utf8).write(to: control)
        let locator = ActiveDaemonLocator(
            service: "threadrelay",
            apiMajor: 1,
            instanceId: "stale-instance",
            pid: 123,
            startedAtMs: 456,
            baseURL: "http://127.0.0.1:3847",
            controlFile: control.path
        )

        XCTAssertEqual(
            ManagementCredentialStore.loadCredentialCandidates(
                locator: locator,
                fallbackPaths: [control]
            ),
            [
                .init(token: "shared-token", expectedInstanceId: "stale-instance"),
                .init(token: "shared-token", expectedInstanceId: nil),
            ]
        )
    }

    func testActiveDaemonLocatorDecodesFromPrivateDiscoveryFile() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let path = directory.appendingPathComponent("threadrelay-active-daemon.json")
        try Data(#"{"service":"threadrelay","apiMajor":1,"instanceId":"fixture-instance","pid":123,"startedAtMs":456,"baseUrl":"http://127.0.0.1:3847","controlFile":"/fixture/threadrelay-control.json"}"#.utf8).write(to: path)

        XCTAssertEqual(
            ManagementCredentialStore.loadLocator(from: path),
            ActiveDaemonLocator(
                service: "threadrelay",
                apiMajor: 1,
                instanceId: "fixture-instance",
                pid: 123,
                startedAtMs: 456,
                baseURL: "http://127.0.0.1:3847",
                controlFile: "/fixture/threadrelay-control.json"
            )
        )
    }

    func testActiveDaemonLocatorOnlyAcceptsExplicitLoopbackHTTPOrigins() {
        func locator(baseURL: String) -> ActiveDaemonLocator {
            ActiveDaemonLocator(
                service: "threadrelay",
                apiMajor: 1,
                instanceId: "fixture-instance",
                pid: 123,
                startedAtMs: 456,
                baseURL: baseURL,
                controlFile: "/fixture/threadrelay-control.json"
            )
        }

        XCTAssertEqual(
            locator(baseURL: "http://127.0.0.1:49321").validatedBaseURL,
            URL(string: "http://127.0.0.1:49321")
        )
        XCTAssertEqual(
            locator(baseURL: "http://[::1]:49321").validatedBaseURL,
            URL(string: "http://[::1]:49321")
        )
        XCTAssertNil(locator(baseURL: "https://127.0.0.1:49321").validatedBaseURL)
        XCTAssertNil(locator(baseURL: "http://localhost:49321").validatedBaseURL)
        XCTAssertNil(locator(baseURL: "http://192.0.2.1:49321").validatedBaseURL)
        XCTAssertNil(locator(baseURL: "http://127.0.0.1:49321/path").validatedBaseURL)
        XCTAssertNil(locator(baseURL: "http://user@127.0.0.1:49321").validatedBaseURL)
        XCTAssertNil(locator(baseURL: "http://127.0.0.1").validatedBaseURL)
    }

    func testAPIClientUsesDiscoveredLoopbackBaseURL() async throws {
        let client = makeClient(
            baseURL: URL(string: "https://fallback.test")!,
            baseURLLoader: { URL(string: "http://127.0.0.1:49321")! }
        ) { request in
            XCTAssertEqual(request.url?.scheme, "http")
            XCTAssertEqual(request.url?.host, "127.0.0.1")
            XCTAssertEqual(request.url?.port, 49321)
            return MockResponse(statusCode: 200, json: Self.healthJSON)
        }

        _ = try await client.probe()
    }

    func testNavigationContainsPhaseZeroSections() {
        XCTAssertEqual(
            AppSection.allCases.map(\.title),
            ["概览", "Codex 接入", "会话", "消息渠道", "AI 网关", "请求日志"]
        )
    }

    func testServiceProbeKeepsVersionedHealthPayload() {
        let health = HealthResponse(service: "threadrelay", apiMajor: 1, ready: true)
        XCTAssertEqual(ServiceProbe.versioned(health), .versioned(health))
    }

    func testProbeUsesVersionedHealthEndpoint() async throws {
        let recorder = RequestRecorder()
        let client = makeClient { request in
            recorder.record(request.url?.path)
            return MockResponse(
                statusCode: 200,
                json: #"{"service":"threadrelay","apiMajor":1,"ready":true}"#
            )
        }

        let result = try await client.probe()

        XCTAssertEqual(
            result,
            .versioned(HealthResponse(service: "threadrelay", apiMajor: 1, ready: true))
        )
        XCTAssertEqual(recorder.paths, ["/healthz"])
    }

    func testProbeUsesInjectedNonDefaultLoopbackPort() async throws {
        let recorder = URLRecorder()
        let client = makeClient(
            baseURL: URL(string: "http://127.0.0.1:49321")!
        ) { request in
            recorder.record(request.url)
            return MockResponse(statusCode: 200, json: Self.healthJSON)
        }

        _ = try await client.probe()

        XCTAssertEqual(recorder.urls.first?.host, "127.0.0.1")
        XCTAssertEqual(recorder.urls.first?.port, 49321)
        XCTAssertEqual(recorder.urls.first?.path, "/healthz")
    }

    func testProbeFallsBackToLegacyStatusAfterHealthNotFound() async throws {
        let recorder = RequestRecorder()
        let client = makeClient { request in
            let path = request.url?.path
            recorder.record(path)

            switch path {
            case "/healthz":
                return MockResponse(statusCode: 404, json: #"{"error":"not found"}"#)
            case "/api/status":
                return MockResponse(statusCode: 200, json: #"{"service":"threadrelay"}"#)
            default:
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }

        let result = try await client.probe()

        XCTAssertEqual(result, .legacy)
        XCTAssertEqual(recorder.paths, ["/healthz", "/api/status"])
    }

    func testProbeRejectsAnotherServiceOnThreadRelayPort() async {
        let client = makeClient { _ in
            MockResponse(
                statusCode: 200,
                json: #"{"service":"another-service","apiMajor":1,"ready":true}"#
            )
        }

        await assertProbeError(.incompatibleService, from: client)
    }

    func testProbeRejectsUnsupportedThreadRelayAPIMajorSeparately() async {
        let client = makeClient { _ in
            MockResponse(
                statusCode: 200,
                json: #"{"service":"threadrelay","apiMajor":2,"ready":true}"#
            )
        }

        await assertProbeError(.unsupportedAPIMajor(2), from: client)
        XCTAssertEqual(
            APIClientError.unsupportedAPIMajor(2).localizedDescription,
            "当前 ThreadRelay 使用了不受支持的管理 API 版本 2。"
        )
        XCTAssertNotEqual(
            APIClientError.unsupportedAPIMajor(2).localizedDescription,
            APIClientError.incompatibleService.localizedDescription
        )
    }

    func testProbeMapsMalformedJSONToInvalidResponse() async {
        let client = makeClient { _ in
            MockResponse(statusCode: 200, json: #"{"service":"threadrelay""#)
        }

        await assertProbeError(.invalidResponse, from: client)
    }

    func testProbeMapsNonSuccessStatusToInvalidResponse() async {
        let client = makeClient { _ in
            MockResponse(statusCode: 503, json: #"{"error":"unavailable"}"#)
        }

        await assertProbeError(.invalidResponse, from: client)
    }

    func testFetchDashboardDecodesAggregateFixtureAndSendsBearerHeader() async throws {
        let client = makeClient { request in
            XCTAssertEqual(request.url?.path, "/api/v1/manage/dashboard")
            XCTAssertEqual(request.httpMethod, "GET")
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Authorization"),
                "Bearer explicit-fixture-token"
            )
            return MockResponse(
                statusCode: 200,
                json: #"{"service":{"service":"threadrelay","apiMajor":1,"ready":true,"instanceId":"fixture-instance","pid":123,"startedAtMs":456},"bridgeRunning":true,"remoteControlConnected":true,"remoteControlHealthy":true,"executionClients":{"codexApp":{"configured":true,"connected":true},"vscode":{"configured":true,"connected":true},"cli":{"configured":false,"connected":false}},"messageChannels":{"telegram":{"accountCount":2,"connectedAccountCount":1},"feishu":{"accountCount":1,"connectedAccountCount":1},"wechat":{"accountCount":1,"connectedAccountCount":1},"wecom":{"accountCount":0,"connectedAccountCount":0}},"aiGatewayEnabled":true,"aiGatewayProviderCount":2,"requestLoggingEnabled":true}"#
            )
        }

        let dashboard = try await client.fetchDashboard(bearerToken: "explicit-fixture-token")

        XCTAssertEqual(
            dashboard,
            ManageDashboard(
                service: ManageDashboard.Service(
                    service: "threadrelay",
                    apiMajor: 1,
                    ready: true,
                    instanceId: "fixture-instance",
                    pid: 123,
                    startedAtMs: 456
                ),
                bridgeRunning: true,
                remoteControlConnected: true,
                remoteControlHealthy: true,
                executionClients: ManageDashboard.ExecutionClients(
                    codexApp: .init(configured: true, connected: true),
                    vscode: .init(configured: true, connected: true),
                    cli: .init(configured: false, connected: false)
                ),
                messageChannels: ManageDashboard.MessageChannels(
                    telegram: .init(accountCount: 2, connectedAccountCount: 1),
                    feishu: .init(accountCount: 1, connectedAccountCount: 1),
                    wechat: .init(accountCount: 1, connectedAccountCount: 1),
                    wecom: .init(accountCount: 0, connectedAccountCount: 0)
                ),
                aiGatewayEnabled: true,
                aiGatewayProviderCount: 2,
                requestLoggingEnabled: true
            )
        )
    }

    func testFetchIMAccountsDecodesAccountStateAndSendsBearerHeader() async throws {
        let client = makeClient { request in
            XCTAssertEqual(request.url?.path, "/api/v1/manage/im/accounts")
            XCTAssertEqual(request.httpMethod, "GET")
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Authorization"),
                "Bearer explicit-fixture-token"
            )
            return MockResponse(statusCode: 200, json: Self.imAccountsJSON)
        }

        let response = try await client.fetchIMAccounts(bearerToken: "explicit-fixture-token")

        XCTAssertEqual(response.service.service, "threadrelay")
        XCTAssertEqual(response.service.apiMajor, 1)
        XCTAssertEqual(response.service.instanceId, "fixture-instance")
        XCTAssertEqual(response.accounts.count, 2)
        XCTAssertEqual(
            response.accounts[0],
            ManageIMAccount(
                platform: "telegram",
                accountId: "telegram-main",
                displayName: "主 Telegram",
                enabled: true,
                configured: true,
                secretSet: true,
                connecting: false,
                polling: true,
                connected: true,
                lastError: nil,
                lastEventAtMs: 1_754_000_120_000,
                lastInboundAtMs: 1_754_000_100_000
            )
        )
        XCTAssertEqual(response.accounts[1].lastError, "连接失败")
        XCTAssertFalse(response.accounts[1].connected)
    }

    func testFetchIMAccountsMapsNotFoundToFeatureUnavailable() async {
        let client = makeClient { request in
            XCTAssertEqual(request.url?.path, "/api/v1/manage/im/accounts")
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Authorization"),
                "Bearer fixture-token"
            )
            return MockResponse(statusCode: 404, json: #"{"error":"not found"}"#)
        }

        do {
            _ = try await client.fetchIMAccounts(bearerToken: "fixture-token")
            XCTFail("Expected account management feature to be unavailable")
        } catch let error as APIClientError {
            XCTAssertEqual(error, .featureUnavailable)
        } catch {
            XCTFail("Expected APIClientError, received \(error)")
        }
    }

    func testSetIMAccountEnabledSendsPOSTBodyAndBearerHeader() async throws {
        let client = makeClient { request in
            XCTAssertEqual(request.url?.path, "/api/v1/manage/im/account/enabled")
            XCTAssertEqual(request.httpMethod, "POST")
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Authorization"),
                "Bearer fixture-token"
            )
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Content-Type"),
                "application/json"
            )
            let body = Self.jsonBody(from: request)
            XCTAssertEqual(body?["platform"] as? String, "telegram")
            XCTAssertEqual(body?["accountId"] as? String, "telegram-main")
            XCTAssertEqual(body?["enabled"] as? Bool, false)
            return MockResponse(
                statusCode: 200,
                json: #"{"ok":true,"platform":"telegram","accountId":"telegram-main","enabled":false}"#
            )
        }

        let response = try await client.setIMAccountEnabled(
            platform: "telegram",
            accountId: "telegram-main",
            enabled: false
        )

        XCTAssertTrue(response.ok)
        XCTAssertEqual(response.platform, "telegram")
        XCTAssertEqual(response.accountId, "telegram-main")
        XCTAssertEqual(response.enabled, false)
    }

    func testDeleteIMAccountSendsPOSTBodyAndMapsNotFoundError() async {
        let client = makeClient { request in
            XCTAssertEqual(request.url?.path, "/api/v1/manage/im/account/delete")
            XCTAssertEqual(request.httpMethod, "POST")
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Authorization"),
                "Bearer fixture-token"
            )
            let body = Self.jsonBody(from: request)
            XCTAssertEqual(body?["platform"] as? String, "telegram")
            XCTAssertEqual(body?["accountId"] as? String, "telegram-main")
            return MockResponse(
                statusCode: 404,
                json: #"{"ok":false,"error":"IM account not found"}"#
            )
        }

        do {
            _ = try await client.deleteIMAccount(
                platform: "telegram",
                accountId: "telegram-main"
            )
            XCTFail("Expected missing account error")
        } catch let error as APIClientError {
            XCTAssertEqual(error, .operationFailed("找不到该消息账号。"))
        } catch {
            XCTFail("Expected APIClientError, received \(error)")
        }
    }

    func testIMMutationMapsRouteMissingOnOlderDaemonToFeatureUnavailable() async {
        let client = makeClient { request in
            XCTAssertEqual(request.httpMethod, "POST")
            return MockResponse(statusCode: 404, json: "Not Found")
        }

        do {
            _ = try await client.setIMAccountEnabled(
                platform: "telegram",
                accountId: "telegram-main",
                enabled: false
            )
            XCTFail("Expected feature-unavailable error")
        } catch let error as APIClientError {
            XCTAssertEqual(error, .featureUnavailable)
        } catch {
            XCTFail("Expected APIClientError, received \(error)")
        }
    }

    func testConfigureTelegramAccountSendsTokenAndDecodesDerivedIdentity() async throws {
        let client = makeClient { request in
            XCTAssertEqual(request.url?.path, "/api/v1/manage/im/account/telegram")
            XCTAssertEqual(request.httpMethod, "POST")
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Authorization"),
                "Bearer fixture-token"
            )
            let body = Self.jsonBody(from: request)
            XCTAssertEqual(body?["botToken"] as? String, "12345:fixture")
            XCTAssertEqual(body?["mentionOnly"] as? Bool, true)
            return MockResponse(
                statusCode: 200,
                json: #"{"ok":true,"platform":"telegram","accountId":"tg_1000001","displayName":"Fixture Bot (@fixture_bot)"}"#
            )
        }

        let response = try await client.configureTelegramAccount(
            botToken: "12345:fixture",
            mentionOnly: true
        )

        XCTAssertTrue(response.ok)
        XCTAssertEqual(response.platform, "telegram")
        XCTAssertEqual(response.accountId, "tg_1000001")
        XCTAssertEqual(response.displayName, "Fixture Bot (@fixture_bot)")
    }

    func testConfigureTelegramAccountSurfacesDaemonValidationReason() async {
        let client = makeClient { _ in
            MockResponse(
                statusCode: 400,
                json: #"{"ok":false,"error":"telegram getMe failed: 401 Unauthorized"}"#
            )
        }

        do {
            _ = try await client.configureTelegramAccount(botToken: "bad", mentionOnly: false)
            XCTFail("Expected validation failure")
        } catch let error as APIClientError {
            XCTAssertEqual(error, .operationFailed("telegram getMe failed: 401 Unauthorized"))
        } catch {
            XCTFail("Expected APIClientError, received \(error)")
        }
    }

    func testConfigureFeishuAccountSendsCredentialsAndDecodesIdentity() async throws {
        let client = makeClient { request in
            XCTAssertEqual(request.url?.path, "/api/v1/manage/im/account/feishu")
            XCTAssertEqual(request.httpMethod, "POST")
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Authorization"),
                "Bearer fixture-token"
            )
            let body = Self.jsonBody(from: request)
            XCTAssertEqual(body?["appId"] as? String, "cli-fixture-app")
            XCTAssertEqual(body?["appSecret"] as? String, "fixture-secret")
            return MockResponse(
                statusCode: 200,
                json: #"{"ok":true,"platform":"feishu","accountId":"cli-fixture-app","displayName":"Fixture 应用"}"#
            )
        }

        let response = try await client.configureFeishuAccount(
            appId: "cli-fixture-app",
            appSecret: "fixture-secret"
        )

        XCTAssertTrue(response.ok)
        XCTAssertEqual(response.platform, "feishu")
        XCTAssertEqual(response.accountId, "cli-fixture-app")
        XCTAssertEqual(response.displayName, "Fixture 应用")
    }

    func testFeishuOnboardingStartAndPendingPollDecode() async throws {
        let client = makeClient { request in
            switch request.url?.path {
            case "/api/v1/manage/im/onboarding/feishu/start":
                XCTAssertEqual(request.httpMethod, "POST")
                return MockResponse(
                    statusCode: 200,
                    json: #"{"verificationUri":"https://f.example/v","verificationUriComplete":"https://f.example/v?code=1","deviceCode":"device-1","expiresIn":600,"interval":5,"qrSvg":"<svg/>"}"#
                )
            case "/api/v1/manage/im/onboarding/feishu/poll":
                let body = Self.jsonBody(from: request)
                XCTAssertEqual(body?["deviceCode"] as? String, "device-1")
                return MockResponse(
                    statusCode: 200,
                    json: #"{"done":false,"appId":null,"displayName":null,"error":"authorization_pending","errorDescription":null}"#
                )
            default:
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }

        let session = try await client.startFeishuOnboarding()
        XCTAssertEqual(session.deviceCode, "device-1")
        XCTAssertEqual(session.verificationUriComplete, "https://f.example/v?code=1")
        XCTAssertEqual(session.interval, 5)

        let poll = try await client.pollFeishuOnboarding(deviceCode: session.deviceCode)
        XCTAssertFalse(poll.done)
        XCTAssertEqual(poll.error, "authorization_pending")
        XCTAssertNil(poll.appId)
    }

    func testWechatOnboardingPollSendsVerifyCodeAndDecodesStates() async throws {
        let client = makeClient { request in
            switch request.url?.path {
            case "/api/v1/manage/im/onboarding/wechat/start":
                return MockResponse(
                    statusCode: 200,
                    json: #"{"sessionKey":"wechat-onboard-1","qrcodeUrl":"https://w.example/qr","qrSvg":"<svg/>","expiresIn":300}"#
                )
            case "/api/v1/manage/im/onboarding/wechat/poll":
                let body = Self.jsonBody(from: request)
                XCTAssertEqual(body?["sessionKey"] as? String, "wechat-onboard-1")
                if body?["verifyCode"] as? String == "246810" {
                    return MockResponse(
                        statusCode: 200,
                        json: #"{"done":true,"status":"confirmed","accountId":"ilink-bot-1","userId":"user-1"}"#
                    )
                }
                return MockResponse(
                    statusCode: 200,
                    json: #"{"done":false,"status":"need_verifycode","needVerifyCode":true,"error":null}"#
                )
            default:
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }

        let session = try await client.startWechatOnboarding()
        XCTAssertEqual(session.sessionKey, "wechat-onboard-1")
        XCTAssertEqual(session.qrcodeUrl, "https://w.example/qr")

        let pending = try await client.pollWechatOnboarding(sessionKey: session.sessionKey)
        XCTAssertFalse(pending.done)
        XCTAssertEqual(pending.needVerifyCode, true)

        let confirmed = try await client.pollWechatOnboarding(
            sessionKey: session.sessionKey,
            verifyCode: "246810"
        )
        XCTAssertTrue(confirmed.done)
        XCTAssertEqual(confirmed.accountId, "ilink-bot-1")
    }

    func testWecomOnboardingStartAndCompletionDecode() async throws {
        let client = makeClient { request in
            switch request.url?.path {
            case "/api/v1/manage/im/onboarding/wecom/start":
                return MockResponse(
                    statusCode: 200,
                    json: #"{"sessionKey":"wecom-onboard-1","qrcodeUrl":"https://q.example/auth","qrSvg":"<svg/>","expiresIn":300,"interval":3}"#
                )
            case "/api/v1/manage/im/onboarding/wecom/poll":
                let body = Self.jsonBody(from: request)
                XCTAssertEqual(body?["sessionKey"] as? String, "wecom-onboard-1")
                return MockResponse(
                    statusCode: 200,
                    json: #"{"done":true,"status":"success","accountId":"wecom-bot-1"}"#
                )
            default:
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }

        let session = try await client.startWecomOnboarding()
        XCTAssertEqual(session.sessionKey, "wecom-onboard-1")
        XCTAssertEqual(session.interval, 3)

        let poll = try await client.pollWecomOnboarding(sessionKey: session.sessionKey)
        XCTAssertTrue(poll.done)
        XCTAssertEqual(poll.accountId, "wecom-bot-1")
    }

    func testOnboardingSessionErrorsMapToFriendlyMessage() async {
        let client = makeClient { _ in
            MockResponse(
                statusCode: 400,
                json: #"{"done":false,"error":"missing_session"}"#
            )
        }

        do {
            _ = try await client.pollWecomOnboarding(sessionKey: "stale")
            XCTFail("Expected session failure")
        } catch let error as APIClientError {
            XCTAssertEqual(error, .operationFailed("扫码会话已失效，请重新获取二维码。"))
        } catch {
            XCTFail("Expected APIClientError, received \(error)")
        }
    }

    func testFetchDashboardDecodesOriginalV1AggregatePayload() async throws {
        let client = makeClient { _ in
            MockResponse(statusCode: 200, json: Self.originalV1DashboardJSON)
        }

        let dashboard = try await client.fetchDashboard(bearerToken: "fixture-token")

        XCTAssertEqual(
            dashboard.executionClients.codexApp,
            .init(configured: true, connected: false)
        )
        XCTAssertEqual(
            dashboard.executionClients.vscode,
            .init(configured: false, connected: false)
        )
        XCTAssertEqual(
            dashboard.executionClients.cli,
            .init(configured: false, connected: false)
        )
        XCTAssertEqual(
            dashboard.messageChannels.legacyUnattributed,
            .init(accountCount: 5, connectedAccountCount: 3)
        )
        XCTAssertEqual(
            dashboard.messageChannels.telegram,
            .init(accountCount: 0, connectedAccountCount: 0)
        )
    }

    func testFetchDashboardRejectsUnsupportedThreadRelayAPIMajorSeparately() async {
        let client = makeClient { _ in
            MockResponse(
                statusCode: 200,
                json: Self.dashboardJSON.replacingOccurrences(
                    of: #""apiMajor":1"#,
                    with: #""apiMajor":7"#
                )
            )
        }

        await assertDashboardError(.unsupportedAPIMajor(7), from: client)
    }

    func testFetchDashboardMapsUnauthorizedResponse() async {
        let client = makeClient { _ in
            MockResponse(statusCode: 401, json: #"{"error":"unauthorized"}"#)
        }

        do {
            _ = try await client.fetchDashboard(bearerToken: "wrong-token")
            XCTFail("Expected unauthorized dashboard response")
        } catch let error as APIClientError {
            guard case .unauthorized = error else {
                XCTFail("Expected unauthorized, received \(error)")
                return
            }
        } catch {
            XCTFail("Expected APIClientError, received \(error)")
        }
    }

    func testFetchDashboardMapsServiceUnavailableToInvalidResponse() async {
        let client = makeClient { _ in
            MockResponse(statusCode: 503, json: #"{"error":"temporarily unavailable"}"#)
        }

        await assertDashboardError(.invalidResponse, from: client)
    }

    func testFetchDashboardMapsMalformedJSONToInvalidResponse() async {
        let client = makeClient { _ in
            MockResponse(statusCode: 200, json: #"{"service":{"service":"threadrelay""#)
        }

        await assertDashboardError(.invalidResponse, from: client)
    }

    func testDashboardTriesCurrentCredentialAfterStaleCandidateReturnsUnauthorized() async throws {
        let headers = StringRecorder()
        let client = makeClient(credentialCandidatesLoader: {
            [
                .init(token: "stale-token", expectedInstanceId: nil),
                .init(token: "current-token", expectedInstanceId: nil),
            ]
        }) { request in
            let authorization = request.value(forHTTPHeaderField: "Authorization") ?? ""
            headers.record(authorization)
            if authorization == "Bearer current-token" {
                return MockResponse(statusCode: 200, json: Self.dashboardJSON)
            }
            return MockResponse(statusCode: 401, json: #"{"error":"unauthorized"}"#)
        }

        let dashboard = try await client.dashboard()

        XCTAssertEqual(dashboard.service.instanceId, "fixture-instance")
        XCTAssertEqual(headers.values, ["Bearer stale-token", "Bearer current-token"])
    }

    func testDashboardReturnsUnauthorizedAfterAllCredentialCandidatesFail() async {
        let headers = StringRecorder()
        let client = makeClient(credentialCandidatesLoader: {
            [
                .init(token: "stale-token", expectedInstanceId: nil),
                .init(token: "also-stale-token", expectedInstanceId: nil),
            ]
        }) { request in
            headers.record(request.value(forHTTPHeaderField: "Authorization") ?? "")
            return MockResponse(statusCode: 401, json: #"{"error":"unauthorized"}"#)
        }

        do {
            _ = try await client.dashboard()
            XCTFail("Expected every credential candidate to be rejected")
        } catch let error as APIClientError {
            XCTAssertEqual(error, .unauthorized)
        } catch {
            XCTFail("Expected APIClientError, received \(error)")
        }
        XCTAssertEqual(headers.values, ["Bearer stale-token", "Bearer also-stale-token"])
    }

    func testDashboardRejectsLocatorTokenForAReplacedDaemonInstance() async throws {
        let headers = StringRecorder()
        let client = makeClient(credentialCandidatesLoader: {
            [
                .init(token: "locator-token", expectedInstanceId: "stale-instance"),
                .init(token: "fallback-token", expectedInstanceId: nil),
            ]
        }) { request in
            headers.record(request.value(forHTTPHeaderField: "Authorization") ?? "")
            return MockResponse(statusCode: 200, json: Self.dashboardJSON)
        }

        let dashboard = try await client.dashboard()

        XCTAssertEqual(dashboard.service.instanceId, "fixture-instance")
        XCTAssertEqual(headers.values, ["Bearer locator-token", "Bearer fallback-token"])
    }

    func testFetchLogDirectoryUsesProtectedManagementRoute() async throws {
        let client = makeClient { request in
            XCTAssertEqual(request.url?.path, "/api/v1/manage/log-directory")
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Authorization"),
                "Bearer fixture-token"
            )
            return MockResponse(
                statusCode: 200,
                json: #"{"directory":"/fixture/custom-state/logs","instanceId":"fixture-instance"}"#
            )
        }

        let directory = try await client.fetchLogDirectory(bearerToken: "fixture-token")
        XCTAssertEqual(directory.path, "/fixture/custom-state/logs")
    }

    func testFetchLifecycleDecodesReadOnlyRuntimeSnapshot() async throws {
        let client = makeClient { request in
            XCTAssertEqual(request.url?.path, "/api/v1/manage/lifecycle")
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Authorization"),
                "Bearer fixture-token"
            )
            return MockResponse(
                statusCode: 200,
                json: #"{"service":{"service":"threadrelay","apiMajor":1,"ready":true,"instanceId":"fixture-instance","pid":123,"startedAtMs":456},"executable":"/fixture/ThreadRelay","configPath":"/fixture/config.toml","bind":"127.0.0.1:3847","runtime":{"state":"active","productVersion":"0.5.0","apiMajor":1},"protectedWorkItems":{"aiGatewayRequests":1,"codexTurns":2,"imStreams":3,"pendingApprovals":1,"remoteControlRequests":4,"total":11},"management":{"state":"unmanaged","mode":"readOnly","canControl":false,"installationId":null,"leaseGeneration":null,"leaseExpiresAtMs":null}}"#
            )
        }

        let lifecycle = try await client.fetchLifecycle(bearerToken: "fixture-token")

        XCTAssertEqual(lifecycle.service.instanceId, "fixture-instance")
        XCTAssertEqual(lifecycle.runtime.productVersion, "0.5.0")
        XCTAssertEqual(lifecycle.protectedWorkItems.total, 11)
        XCTAssertEqual(lifecycle.management.mode, "readOnly")
        XCTAssertFalse(lifecycle.management.canControl)
    }

    func testFetchLifecycleRejectsUnsupportedRuntimeAPIMajor() async {
        let client = makeClient { _ in
            MockResponse(
                statusCode: 200,
                json: #"{"service":{"service":"threadrelay","apiMajor":1,"ready":true,"instanceId":"fixture-instance","pid":123,"startedAtMs":456},"executable":"/fixture/ThreadRelay","configPath":"/fixture/config.toml","bind":"127.0.0.1:3847","runtime":{"state":"active","productVersion":"0.5.0","apiMajor":2},"protectedWorkItems":{"aiGatewayRequests":0,"codexTurns":0,"imStreams":0,"pendingApprovals":0,"remoteControlRequests":0,"total":0},"management":{"state":"unmanaged","mode":"readOnly","canControl":false,"installationId":null,"leaseGeneration":null,"leaseExpiresAtMs":null}}"#
            )
        }

        await assertLifecycleError(.unsupportedAPIMajor(2), from: client)
    }

    func testLogDirectoryRejectsLocatorResponseFromReplacedDaemon() async throws {
        let client = makeClient(credentialCandidatesLoader: {
            [
                .init(token: "locator-token", expectedInstanceId: "stale-instance"),
                .init(token: "fallback-token", expectedInstanceId: nil),
            ]
        }) { request in
            let token = request.value(forHTTPHeaderField: "Authorization")
            if token == "Bearer locator-token" {
                return MockResponse(
                    statusCode: 200,
                    json: #"{"directory":"/stale/logs","instanceId":"replacement-instance"}"#
                )
            }
            return MockResponse(
                statusCode: 200,
                json: #"{"directory":"/current/logs","instanceId":"replacement-instance"}"#
            )
        }

        let directory = try await client.logDirectory()

        XCTAssertEqual(directory.path, "/current/logs")
    }

    func testCodexAndSessionManagementUseProtectedVersionedRoutes() async throws {
        let client = makeClient { request in
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Authorization"),
                "Bearer fixture-token"
            )
            switch request.url?.path {
            case "/api/v1/manage/codex/status":
                return MockResponse(
                    statusCode: 200,
                    json: #"{"codexHome":"/fixture/.codex","configured":true,"configOk":true,"authOk":true,"providerOk":true,"configError":null,"authError":null,"guiConfigured":true,"guiError":null,"remoteControlSupported":true,"remoteControlConfigured":true,"remoteControlError":null,"providers":[{"name":"ai-gateway","baseUrl":"http://127.0.0.1:3847/backend-api","secretSet":true,"supportsWebsockets":true}],"imageGenerationEnabled":true,"connectionMode":"standard"}"#
                )
            case "/api/v1/manage/codex/enhanced/preflight":
                return MockResponse(
                    statusCode: 200,
                    json: #"{"ok":true,"status":{"running":false}}"#
                )
            case "/api/v1/manage/sessions":
                return MockResponse(
                    statusCode: 200,
                    json: #"{"ok":true,"threads":[{"id":"thread-1","preview":"修复登录","modelProvider":"openai","updatedAt":1754000120000,"path":"/fixture/rollout.jsonl","name":null},{"id":"legacy-thread"}],"providers":["openai"],"total":2}"#
                )
            case "/api/v1/manage/sessions/provider":
                let body = Self.jsonBody(from: request)
                XCTAssertEqual(body?["threadId"] as? String, "thread-1")
                XCTAssertEqual(body?["targetProvider"] as? String, "openai")
                XCTAssertNil(body?["rolloutPath"])
                return MockResponse(statusCode: 200, json: #"{"ok":true}"#)
            default:
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }

        let status = try await client.codexStatus()
        XCTAssertTrue(status.configured)
        XCTAssertTrue(status.providers[0].secretSet)
        XCTAssertEqual(status.providers[0].name, "ai-gateway")

        let preflight = try await client.codexEnhancedPreflight()
        XCTAssertFalse(preflight.status.running)

        let sessions = try await client.codexSessions()
        XCTAssertEqual(sessions.total, 2)
        XCTAssertEqual(sessions.threads[0].displayName, "修复登录")
        XCTAssertEqual(sessions.threads[1].displayName, "未命名会话")
        XCTAssertEqual(sessions.threads[1].modelProvider, "openai")
        XCTAssertEqual(sessions.threads[1].updatedAt, 0)
        _ = try await client.moveCodexSession(
            threadId: "thread-1",
            targetProvider: "openai"
        )
    }

    func testGatewayProviderMutationKeepsAPIKeyWriteOnly() async throws {
        let responseJSON = #"{"ok":true,"gateway":{"enabled":true,"filterImageGenerationTool":false,"requestLoggingEnabled":true,"requestLogDetailsEnabled":false,"codexVisibleModels":["model-a"],"providers":[{"name":"primary","enabled":true,"providerType":"open_ai_responses","compatibility":null,"baseUrl":"https://provider.example/v1","modelsUrl":null,"models":["model-a"],"modelAliases":{},"promptCacheRetention":null,"weight":100,"timeoutSecs":60,"secretSet":true}]}}"#
        let client = makeClient { request in
            XCTAssertEqual(request.url?.path, "/api/v1/manage/gateway/provider")
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Authorization"),
                "Bearer fixture-token"
            )
            let body = Self.jsonBody(from: request)
            XCTAssertEqual(body?["apiKey"] as? String, "write-only-key")
            XCTAssertEqual(body?["providerType"] as? String, "open_ai_responses")
            return MockResponse(statusCode: 200, json: responseJSON)
        }
        let provider = ManageGatewayProvider(
            name: "primary",
            enabled: true,
            providerType: "open_ai_responses",
            compatibility: nil,
            baseUrl: "https://provider.example/v1",
            modelsUrl: nil,
            models: ["model-a"],
            modelAliases: [:],
            promptCacheRetention: nil,
            weight: 100,
            timeoutSecs: 60,
            secretSet: false
        )

        let gateway = try await client.upsertGatewayProvider(
            originalName: nil,
            provider: provider,
            apiKey: "write-only-key"
        )

        XCTAssertTrue(gateway.providers[0].secretSet)
        XCTAssertEqual(gateway.providers[0].models, ["model-a"])
    }

    func testGatewayMutationRejectsAStaleDiscoveredDaemonBeforePOST() async {
        let client = makeClient(credentialCandidatesLoader: {
            [.init(token: "fixture-token", expectedInstanceId: "stale-instance")]
        }) { request in
            XCTAssertEqual(request.url?.path, "/api/v1/manage/status")
            return MockResponse(
                statusCode: 200,
                json: #"{"service":"threadrelay","apiMajor":1,"ready":true,"instanceId":"replacement-instance","pid":123,"startedAtMs":456}"#
            )
        }
        let provider = ManageGatewayProvider(
            name: "primary",
            enabled: true,
            providerType: "open_ai_responses",
            compatibility: nil,
            baseUrl: "https://provider.example/v1",
            modelsUrl: nil,
            models: ["model-a"],
            modelAliases: [:],
            promptCacheRetention: nil,
            weight: 100,
            timeoutSecs: 60,
            secretSet: false
        )

        do {
            _ = try await client.upsertGatewayProvider(
                originalName: nil,
                provider: provider,
                apiKey: "write-only-key"
            )
            XCTFail("Expected stale daemon identity to be rejected")
        } catch let error as APIClientError {
            XCTAssertEqual(error, .unauthorized)
        } catch {
            XCTFail("Unexpected error: \(error)")
        }
    }

    func testSettingsAndRequestLogsDecodeRealManagementPayloads() async throws {
        let client = makeClient { request in
            switch request.url?.path {
            case "/api/v1/manage/settings":
                if request.httpMethod == "POST" {
                    let body = Self.jsonBody(from: request)
                    XCTAssertEqual(body?["outboundProxyMode"] as? String, "direct")
                    return MockResponse(
                        statusCode: 200,
                        json: #"{"ok":true,"settings":{"language":"zh-CN","theme":"dark","localConnectionMode":"standard","bind":"127.0.0.1:3847","outboundProxy":{"mode":"direct","url":"<none>","credentialSet":false}}}"#
                    )
                }
                return MockResponse(
                    statusCode: 200,
                    json: #"{"language":"zh-CN","theme":"system","localConnectionMode":"standard","bind":"127.0.0.1:3847","outboundProxy":{"mode":"system","url":"<none>","credentialSet":false}}"#
                )
            case "/api/v1/manage/request-logs":
                return MockResponse(
                    statusCode: 200,
                    json: #"{"logs":[{"id":7,"requestId":"req-7","modelId":"model-a","stream":true,"channel":"primary","providerType":"openai_responses","status":"success","inputTokens":10,"outputTokens":20,"totalTokens":30,"readCacheTokens":null,"readCacheHitRate":null,"writeCacheTokens":null,"costUsd":0.01,"latencyMs":1200,"ttftMs":300,"createdAtMs":1754000120000,"createdAt":"2026-08-13T00:00:00Z","errorMessage":null,"upstreamRequestBodyBytes":128}]}"#
                )
            case "/api/v1/manage/request-logs/7":
                return MockResponse(
                    statusCode: 200,
                    json: #"{"log":{"id":7,"requestId":"req-7","modelId":"model-a","stream":true,"channel":"primary","providerType":"openai_responses","status":"success","inputTokens":10,"outputTokens":20,"totalTokens":30,"readCacheTokens":null,"readCacheHitRate":null,"writeCacheTokens":null,"costUsd":0.01,"latencyMs":1200,"ttftMs":300,"createdAtMs":1754000120000,"createdAt":"2026-08-13T00:00:00Z","errorMessage":null,"upstreamRequestBodyBytes":128,"requestHeadersJson":"{\"authorization\":\"<redacted>\"}","requestJson":"{\"model\":\"model-a\"}","upstreamRequestHeadersJson":null,"upstreamRequestJson":null,"upstreamResponseSse":null,"responseJson":"{\"ok\":true}"}}"#
                )
            default:
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }

        let settings = try await client.settings()
        XCTAssertEqual(settings.outboundProxy.mode, "system")
        let saved = try await client.updateSettings(
            language: "zh-CN",
            theme: "dark",
            localConnectionMode: "standard",
            outboundProxyMode: "direct",
            outboundProxyURL: nil
        )
        XCTAssertEqual(saved.theme, "dark")

        let logs = try await client.requestLogs()
        XCTAssertEqual(logs[0].totalTokens, 30)
        let detail = try await client.requestLogDetail(id: 7)
        XCTAssertEqual(detail.requestHeadersJson, #"{"authorization":"<redacted>"}"#)
    }

    @MainActor
    func testAppModelLoadsVersionedDashboard() async {
        let client = makeClient { request in
            switch request.url?.path {
            case "/healthz":
                MockResponse(statusCode: 200, json: Self.healthJSON)
            case "/api/v1/manage/dashboard":
                MockResponse(statusCode: 200, json: Self.dashboardJSON)
            case "/api/v1/manage/lifecycle":
                MockResponse(statusCode: 200, json: Self.lifecycleJSON)
            default:
                MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }
        let model = AppModel(apiClient: client)

        await model.refresh()

        XCTAssertEqual(model.serviceStatus, .available)
        XCTAssertEqual(model.dashboardState, .loaded)
        XCTAssertEqual(model.dashboard?.service.instanceId, "fixture-instance")
        XCTAssertNotNil(model.lastCheckedAt)
    }

    @MainActor
    func testAppModelShowsAccountManagementUpdateNoticeForOlderDaemon() async {
        let client = makeClient { request in
            switch request.url?.path {
            case "/healthz":
                MockResponse(statusCode: 200, json: Self.healthJSON)
            case "/api/v1/manage/dashboard":
                MockResponse(statusCode: 200, json: Self.dashboardJSON)
            case "/api/v1/manage/lifecycle":
                MockResponse(statusCode: 200, json: Self.lifecycleJSON)
            case "/api/v1/manage/im/accounts":
                MockResponse(statusCode: 404, json: #"{"error":"not found"}"#)
            default:
                MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }
        let model = AppModel(apiClient: client)

        await model.refresh()

        XCTAssertEqual(model.dashboardState, .loaded)
        XCTAssertEqual(model.imAccounts, [])
        XCTAssertEqual(model.imAccountsAvailability, .needsUpdate)
    }

    @MainActor
    func testAppModelReportsFailedAccountToggleWithoutDowngradingAvailability() async throws {
        let client = makeClient { request in
            switch request.url?.path {
            case "/healthz":
                MockResponse(statusCode: 200, json: Self.healthJSON)
            case "/api/v1/manage/dashboard":
                MockResponse(statusCode: 200, json: Self.dashboardJSON)
            case "/api/v1/manage/lifecycle":
                MockResponse(statusCode: 200, json: Self.lifecycleJSON)
            case "/api/v1/manage/im/accounts":
                MockResponse(statusCode: 200, json: Self.imAccountsJSON)
            case "/api/v1/manage/im/account/enabled":
                MockResponse(statusCode: 404, json: #"{"ok":false,"error":"IM account not found"}"#)
            default:
                MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }
        let model = AppModel(apiClient: client)
        await model.refresh()
        XCTAssertEqual(model.imAccountsAvailability, .available)
        let account = try XCTUnwrap(model.imAccounts.first)

        let acknowledged = await model.setIMAccountEnabled(account, enabled: false)

        XCTAssertFalse(acknowledged)
        XCTAssertEqual(model.accountOperationError, "找不到该消息账号。")
        XCTAssertEqual(model.imAccountsAvailability, .available)
    }

    @MainActor
    func testAppModelAcknowledgesSuccessfulAccountToggle() async throws {
        let client = makeClient { request in
            switch request.url?.path {
            case "/healthz":
                MockResponse(statusCode: 200, json: Self.healthJSON)
            case "/api/v1/manage/dashboard":
                MockResponse(statusCode: 200, json: Self.dashboardJSON)
            case "/api/v1/manage/lifecycle":
                MockResponse(statusCode: 200, json: Self.lifecycleJSON)
            case "/api/v1/manage/im/accounts":
                MockResponse(statusCode: 200, json: Self.imAccountsJSON)
            case "/api/v1/manage/im/account/enabled":
                MockResponse(
                    statusCode: 200,
                    json: #"{"ok":true,"platform":"telegram","accountId":"telegram-main","enabled":false}"#
                )
            default:
                MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }
        let model = AppModel(apiClient: client)
        await model.refresh()
        let account = try XCTUnwrap(model.imAccounts.first)

        let acknowledged = await model.setIMAccountEnabled(account, enabled: false)

        XCTAssertTrue(acknowledged)
        XCTAssertNil(model.accountOperationError)
    }

    @MainActor
    func testAppModelClassifiesLegacyDaemon() async {
        let client = makeClient { request in
            switch request.url?.path {
            case "/healthz":
                MockResponse(statusCode: 404, json: #"{"error":"not found"}"#)
            case "/api/status":
                MockResponse(statusCode: 200, json: #"{"service":"codexhub"}"#)
            default:
                MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }
        let model = AppModel(apiClient: client)

        await model.refresh()

        XCTAssertEqual(model.serviceStatus, .bridgeAvailable)
        XCTAssertEqual(model.dashboardState, .legacy)
        XCTAssertNil(model.dashboard)
    }

    @MainActor
    func testAppModelMapsUnauthorizedDashboardAndClearsExistingData() async {
        let dashboardResponses = MockResponseSequence([
            MockResponse(statusCode: 200, json: Self.dashboardJSON),
            MockResponse(statusCode: 401, json: #"{"error":"unauthorized"}"#),
        ])
        let client = makeClient { request in
            if request.url?.path == "/api/v1/manage/dashboard" {
                return dashboardResponses.next()
            }
            return MockResponse(statusCode: 200, json: Self.healthJSON)
        }
        let model = AppModel(apiClient: client)

        await model.refresh()
        XCTAssertNotNil(model.dashboard)
        await model.refresh()

        XCTAssertEqual(model.serviceStatus, .available)
        XCTAssertEqual(model.dashboardState, .unauthorized)
        XCTAssertNil(model.dashboard)
    }

    @MainActor
    func testAppModelShowsDashboardUnavailableWithoutMarkingServiceOffline() async {
        let client = makeClient { request in
            switch request.url?.path {
            case "/healthz":
                MockResponse(statusCode: 200, json: Self.healthJSON)
            case "/api/v1/manage/dashboard":
                MockResponse(statusCode: 503, json: #"{"error":"temporarily unavailable"}"#)
            default:
                MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }
        let model = AppModel(apiClient: client)

        await model.refresh()

        XCTAssertEqual(model.serviceStatus, .available)
        XCTAssertEqual(model.dashboardState, .unavailable)
        XCTAssertNil(model.dashboard)
    }

    @MainActor
    func testAppModelClearsDashboardWhenDaemonBecomesLegacy() async {
        let healthResponses = MockResponseSequence([
            MockResponse(statusCode: 200, json: Self.healthJSON),
            MockResponse(statusCode: 404, json: #"{"error":"not found"}"#),
        ])
        let client = makeClient { request in
            switch request.url?.path {
            case "/healthz":
                healthResponses.next()
            case "/api/status":
                MockResponse(statusCode: 200, json: #"{"service":"codexhub"}"#)
            case "/api/v1/manage/dashboard":
                MockResponse(statusCode: 200, json: Self.dashboardJSON)
            default:
                MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }
        let model = AppModel(apiClient: client)

        await model.refresh()
        XCTAssertNotNil(model.dashboard)
        await model.refresh()

        XCTAssertEqual(model.serviceStatus, .bridgeAvailable)
        XCTAssertEqual(model.dashboardState, .legacy)
        XCTAssertNil(model.dashboard)
    }

    @MainActor
    func testAppModelShowsOfflineOnInitialProbeFailure() async {
        let client = makeClient { _ in
            MockResponse(statusCode: 503, json: #"{"error":"temporarily unavailable"}"#)
        }
        let model = AppModel(apiClient: client)

        await model.refresh()

        guard case .unavailable = model.serviceStatus else {
            XCTFail("Expected unavailable service state")
            return
        }
        XCTAssertEqual(model.dashboardState, .offline)
        XCTAssertNil(model.dashboard)
    }

    @MainActor
    func testAppModelKeepsLastDashboardWhenRefreshFails() async {
        let healthResponses = MockResponseSequence([
            MockResponse(statusCode: 200, json: Self.healthJSON),
            MockResponse(statusCode: 503, json: #"{"error":"temporarily unavailable"}"#),
        ])
        let client = makeClient { request in
            if request.url?.path == "/healthz" {
                return healthResponses.next()
            }
            return MockResponse(statusCode: 200, json: Self.dashboardJSON)
        }
        let model = AppModel(apiClient: client)

        await model.refresh()
        let firstDashboard = model.dashboard
        await model.refresh()

        XCTAssertEqual(model.dashboardState, .stale)
        XCTAssertEqual(model.dashboard, firstDashboard)
    }

    @MainActor
    func testAppModelReloadsCredentialAfterRotation() async {
        let credentials = StringSequence(["old-token", "new-token"])
        let client = makeClient(credentialLoader: { credentials.next() }) { request in
            switch request.url?.path {
            case "/healthz":
                MockResponse(statusCode: 200, json: Self.healthJSON)
            case "/api/v1/manage/dashboard":
                if request.value(forHTTPHeaderField: "Authorization") == "Bearer new-token" {
                    MockResponse(statusCode: 200, json: Self.dashboardJSON)
                } else {
                    MockResponse(statusCode: 401, json: #"{"error":"unauthorized"}"#)
                }
            default:
                MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }
        let model = AppModel(apiClient: client)

        await model.refresh()
        XCTAssertEqual(model.dashboardState, .unauthorized)
        await model.refresh()

        XCTAssertEqual(model.dashboardState, .loaded)
        XCTAssertNotNil(model.dashboard)
    }

    @MainActor
    func testForcedSectionReloadKeepsNewestResponseAndLoadingState() async {
        let calls = IntCounter()
        let client = makeClient { request in
            guard request.url?.path == "/api/v1/manage/sessions" else {
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
            let call = calls.next()
            if call == 1 {
                Thread.sleep(forTimeInterval: 0.15)
                return MockResponse(
                    statusCode: 200,
                    json: #"{"ok":true,"threads":[{"id":"older","preview":"older","modelProvider":"openai","updatedAt":1}],"providers":["openai"],"total":1}"#
                )
            }
            Thread.sleep(forTimeInterval: 0.01)
            return MockResponse(
                statusCode: 200,
                json: #"{"ok":true,"threads":[{"id":"newer","preview":"newer","modelProvider":"ai-gateway","updatedAt":2}],"providers":["ai-gateway"],"total":1}"#
            )
        }
        let model = AppModel(apiClient: client)

        let olderLoad = Task { await model.loadSection(.sessions, force: true) }
        try? await Task.sleep(for: .milliseconds(25))
        let newerLoad = Task { await model.loadSection(.sessions, force: true) }
        let newerSucceeded = await newerLoad.value
        let olderSucceeded = await olderLoad.value

        XCTAssertTrue(newerSucceeded)
        XCTAssertFalse(olderSucceeded)
        XCTAssertEqual(model.codexSessions.map(\.id), ["newer"])
        XCTAssertEqual(model.codexSessionProviders, ["ai-gateway", "openai"])
        XCTAssertFalse(model.isLoading(.sessions))
    }

    @MainActor
    func testManagementActionReportsPostMutationRefreshFailure() async {
        let listCalls = IntCounter()
        let client = makeClient { request in
            switch (request.httpMethod, request.url?.path) {
            case ("GET", "/api/v1/manage/request-logs"):
                if listCalls.next() == 1 {
                    return MockResponse(
                        statusCode: 200,
                        json: #"{"logs":[{"id":7,"requestId":"req-7","modelId":"model-a","stream":true,"channel":"primary","providerType":"openai_responses","status":"success","inputTokens":10,"outputTokens":20,"totalTokens":30,"readCacheTokens":null,"readCacheHitRate":null,"writeCacheTokens":null,"costUsd":0.01,"latencyMs":1200,"ttftMs":300,"createdAtMs":1754000120000,"createdAt":"2026-08-13T00:00:00Z","errorMessage":null,"upstreamRequestBodyBytes":128}]}"#
                    )
                }
                return MockResponse(
                    statusCode: 500,
                    json: #"{"error":"refresh unavailable"}"#
                )
            case ("POST", "/api/v1/manage/request-logs/clear"):
                return MockResponse(statusCode: 200, json: #"{"ok":true,"deleted":1}"#)
            default:
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }
        let model = AppModel(apiClient: client)
        let initialLoadSucceeded = await model.loadSection(.requestLogs)
        XCTAssertTrue(initialLoadSucceeded)
        XCTAssertEqual(model.requestLogs.count, 1)

        let succeeded = await model.clearRequestLogs()

        XCTAssertFalse(succeeded)
        XCTAssertEqual(model.requestLogs.count, 1)
        XCTAssertNotNil(model.sectionErrors[.requestLogs])
        XCTAssertNotNil(model.managementOperationError)
    }

    func testProviderTemplatesUseProtectedRouteAndDecodeOmittedOptionalKeys() async throws {
        let paths = RequestRecorder()
        let headers = StringRecorder()
        let client = makeClient { request in
            paths.record(request.url?.path)
            headers.record(request.value(forHTTPHeaderField: "Authorization") ?? "")
            return MockResponse(
                statusCode: 200,
                json: #"{"templates":[{"id":"openai","displayName":"OpenAI","providerType":"open_ai_responses","baseUrl":"https://api.openai.com/v1","models":[]},{"id":"glm","displayName":"GLM","providerType":"anthropic_messages","compatibility":"glm_anthropic","baseUrl":"https://open.bigmodel.cn/api/anthropic","modelsUrl":"https://open.bigmodel.cn/api/paas/v4/models","models":["glm-5"]}]}"#
            )
        }

        let templates = try await client.gatewayProviderTemplates()

        XCTAssertEqual(paths.paths, ["/api/v1/manage/gateway/provider-templates"])
        XCTAssertEqual(headers.values, ["Bearer fixture-token"])
        XCTAssertEqual(templates.count, 2)
        // Optional keys are omitted from the payload when absent.
        XCTAssertEqual(templates[0].id, "openai")
        XCTAssertNil(templates[0].compatibility)
        XCTAssertNil(templates[0].modelsUrl)
        XCTAssertEqual(templates[0].models, [])
        XCTAssertEqual(templates[1].id, "glm")
        XCTAssertEqual(templates[1].providerType, "anthropic_messages")
        XCTAssertEqual(templates[1].compatibility, "glm_anthropic")
        XCTAssertEqual(templates[1].modelsUrl, "https://open.bigmodel.cn/api/paas/v4/models")
        XCTAssertEqual(templates[1].models, ["glm-5"])
    }

    func testCodexModelCatalogUsesProtectedRouteAndDecodesEntries() async throws {
        let paths = RequestRecorder()
        let headers = StringRecorder()
        let client = makeClient { request in
            paths.record(request.url?.path)
            headers.record(request.value(forHTTPHeaderField: "Authorization") ?? "")
            return MockResponse(
                statusCode: 200,
                json: #"{"models":[{"id":"gpt-5.5","displayName":"GPT-5.5"},{"id":"gpt-5.5-codex","displayName":"GPT-5.5 Codex"}]}"#
            )
        }

        let models = try await client.codexModelCatalog()

        XCTAssertEqual(paths.paths, ["/api/v1/manage/codex/models/catalog"])
        XCTAssertEqual(headers.values, ["Bearer fixture-token"])
        XCTAssertEqual(models.map(\.id), ["gpt-5.5", "gpt-5.5-codex"])
        XCTAssertEqual(models.map(\.displayName), ["GPT-5.5", "GPT-5.5 Codex"])
    }

    func testClearRequestLogsUsesExtendedTimeoutAndReturnsDeletedCount() async throws {
        let timeouts = StringRecorder()
        let client = makeClient { request in
            guard request.url?.path == "/api/v1/manage/request-logs/clear" else {
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
            timeouts.record("\(request.timeoutInterval)")
            return MockResponse(statusCode: 200, json: #"{"ok":true,"deleted":137}"#)
        }

        let deleted = try await client.clearRequestLogs()

        XCTAssertEqual(deleted, 137)
        // Clearing runs DELETE + VACUUM in the daemon; the default 10-second
        // mutation timeout would misreport long clears as failures.
        XCTAssertEqual(timeouts.values, ["300.0"])
    }

    func testClearOldRequestLogsPostsDaysWithExtendedTimeout() async throws {
        let timeouts = StringRecorder()
        let client = makeClient { request in
            guard request.url?.path == "/api/v1/manage/request-logs/clear-old" else {
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
            XCTAssertEqual(request.httpMethod, "POST")
            timeouts.record("\(request.timeoutInterval)")
            let body = Self.jsonBody(from: request)
            XCTAssertEqual(body?["days"] as? Int, 3)
            return MockResponse(statusCode: 200, json: #"{"ok":true,"deleted":42}"#)
        }

        let deleted = try await client.clearOldRequestLogs()

        XCTAssertEqual(deleted, 42)
        XCTAssertEqual(timeouts.values, ["300.0"])
    }

    @MainActor
    func testAppModelClearOldRequestLogsPublishesDeletedCountAndRefreshes() async {
        let listCalls = IntCounter()
        let client = makeClient { request in
            switch (request.httpMethod, request.url?.path) {
            case ("GET", "/api/v1/manage/request-logs"):
                _ = listCalls.next()
                return MockResponse(statusCode: 200, json: Self.requestLogsJSON)
            case ("POST", "/api/v1/manage/request-logs/clear-old"):
                let body = Self.jsonBody(from: request)
                XCTAssertEqual(body?["days"] as? Int, 3)
                return MockResponse(statusCode: 200, json: #"{"ok":true,"deleted":137}"#)
            default:
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }
        let model = AppModel(apiClient: client)
        let initialLoad = await model.loadSection(.requestLogs)
        XCTAssertTrue(initialLoad)

        let succeeded = await model.clearOldRequestLogs()

        XCTAssertTrue(succeeded)
        XCTAssertEqual(model.actionFeedback?.message, "已清理 137 条旧日志")
        XCTAssertNil(model.managementOperationError)
        // The list is re-fetched after the clear (initial load + refresh).
        XCTAssertEqual(listCalls.next(), 3)
    }

    func testRequestLogAndDetailDecodeCacheFieldsIncludingTTLSplit() async throws {
        let client = makeClient { request in
            switch request.url?.path {
            case "/api/v1/manage/request-logs":
                return MockResponse(
                    statusCode: 200,
                    json: #"{"logs":[{"id":9,"requestId":"req-9","modelId":"claude-opus-4-8","stream":true,"channel":"anthropic","providerType":"anthropic_messages","status":"success","inputTokens":1500,"outputTokens":200,"totalTokens":1700,"readCacheTokens":1200,"readCacheHitRate":0.8,"writeCacheTokens":3000,"writeCache5mTokens":2000,"writeCache1hTokens":1000,"costUsd":0.0125,"latencyMs":900,"ttftMs":120,"createdAtMs":1754000120000,"createdAt":"2026-08-13T00:00:00Z","errorMessage":null,"upstreamRequestBodyBytes":2048}]}"#
                )
            case "/api/v1/manage/request-logs/9":
                return MockResponse(
                    statusCode: 200,
                    json: #"{"log":{"id":9,"requestId":"req-9","modelId":"claude-opus-4-8","stream":true,"channel":"anthropic","providerType":"anthropic_messages","status":"success","inputTokens":1500,"outputTokens":200,"totalTokens":1700,"readCacheTokens":1200,"readCacheHitRate":0.8,"writeCacheTokens":3000,"writeCache5mTokens":2000,"writeCache1hTokens":1000,"costUsd":0.0125,"latencyMs":900,"ttftMs":120,"createdAtMs":1754000120000,"createdAt":"2026-08-13T00:00:00Z","errorMessage":null,"upstreamRequestBodyBytes":2048,"requestHeadersJson":null,"requestJson":null,"upstreamRequestHeadersJson":null,"upstreamRequestJson":null,"upstreamResponseSse":null,"responseJson":null}}"#
                )
            default:
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }

        let logs = try await client.requestLogs()
        XCTAssertEqual(logs[0].readCacheTokens, 1200)
        XCTAssertEqual(logs[0].readCacheHitRate, 0.8)
        XCTAssertEqual(logs[0].writeCacheTokens, 3000)
        XCTAssertEqual(logs[0].writeCache5mTokens, 2000)
        XCTAssertEqual(logs[0].writeCache1hTokens, 1000)

        let detail = try await client.requestLogDetail(id: 9)
        XCTAssertEqual(detail.readCacheTokens, 1200)
        XCTAssertEqual(detail.readCacheHitRate, 0.8)
        XCTAssertEqual(detail.writeCacheTokens, 3000)
        XCTAssertEqual(detail.writeCache5mTokens, 2000)
        XCTAssertEqual(detail.writeCache1hTokens, 1000)
    }

    /// The daemon omits the TTL-split keys entirely when the upstream did
    /// not report them; decoding must not require their presence.
    func testRequestLogDecodesWhenTTLSplitKeysAreOmitted() async throws {
        let client = makeClient { _ in
            MockResponse(statusCode: 200, json: Self.requestLogsJSON)
        }

        let logs = try await client.requestLogs()

        XCTAssertNil(logs[0].readCacheTokens)
        XCTAssertNil(logs[0].writeCache5mTokens)
        XCTAssertNil(logs[0].writeCache1hTokens)
    }

    func testRequestLogSummaryFormattersAlignWithLegacyGUI() {
        XCTAssertEqual(formatGroupedInt(0), "0")
        XCTAssertEqual(formatGroupedInt(999), "999")
        XCTAssertEqual(formatGroupedInt(1_234_567), "1,234,567")
        XCTAssertEqual(formatGroupedInt(-1_234), "-1,234")

        XCTAssertEqual(formatByteCount(512), "512 B")
        XCTAssertEqual(formatByteCount(1_536), "1.5 KB")
        XCTAssertEqual(formatByteCount(3_670_016), "3.50 MB")

        XCTAssertEqual(readCacheSummary(tokens: 1_200, hitRate: 0.833), "1,200 tokens(83.3%)")
        XCTAssertEqual(readCacheSummary(tokens: 1_200, hitRate: nil), "1,200 tokens")
        XCTAssertEqual(readCacheSummary(tokens: nil, hitRate: 0.8), "未记录")

        XCTAssertEqual(
            writeCacheSummary(tokens: 3_000, fiveMinuteTokens: 2_000, oneHourTokens: 1_000),
            "3,000 tokens [5m 2,000, 1h 1,000]"
        )
        XCTAssertEqual(
            writeCacheSummary(tokens: 3_000, fiveMinuteTokens: nil, oneHourTokens: nil),
            "3,000 tokens"
        )
        XCTAssertEqual(
            writeCacheSummary(tokens: nil, fiveMinuteTokens: 5, oneHourTokens: nil),
            "未记录"
        )
    }

    func testFetchProviderModelsSendsCamelCaseBodyAndOmitsEmptyOptionals() async throws {
        let timeouts = StringRecorder()
        let client = makeClient { request in
            guard request.url?.path == "/api/v1/manage/gateway/provider/models/fetch" else {
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
            XCTAssertEqual(request.httpMethod, "POST")
            timeouts.record("\(request.timeoutInterval)")
            let body = Self.jsonBody(from: request)
            XCTAssertEqual(body?["providerName"] as? String, "glm")
            XCTAssertEqual(body?["baseUrl"] as? String, "https://open.bigmodel.cn/api/anthropic")
            XCTAssertEqual(body?["modelsUrl"] as? String, "https://open.bigmodel.cn/api/paas/v4/models")
            XCTAssertEqual(body?["providerType"] as? String, "anthropic_messages")
            // The stored key should be reused when the user typed nothing.
            XCTAssertNil(body?["apiKey"])
            return MockResponse(
                statusCode: 200,
                json: #"{"ok":true,"models":["glm-5","glm-5-air"],"attempts":[{"url":"https://open.bigmodel.cn/api/paas/v4/models","status":200,"error":null,"preview":null}]}"#
            )
        }

        let response = try await client.fetchGatewayProviderModels(
            providerName: "glm",
            baseUrl: "https://open.bigmodel.cn/api/anthropic",
            modelsUrl: "https://open.bigmodel.cn/api/paas/v4/models",
            providerType: "anthropic_messages",
            apiKey: nil
        )

        XCTAssertTrue(response.ok)
        XCTAssertEqual(response.models, ["glm-5", "glm-5-air"])
        XCTAssertEqual(response.attempts.first?.status, 200)
        XCTAssertEqual(timeouts.values, ["60.0"])
    }

    func testFetchProviderModelsDecodesFailureAttempts() async throws {
        let client = makeClient { _ in
            MockResponse(
                statusCode: 200,
                json: #"{"ok":false,"models":[],"attempts":[{"url":"https://a.example/v1/models","status":401,"error":null,"preview":"{\"error\":\"unauthorized\"}"},{"url":"https://b.example/models","status":null,"error":"连接超时","preview":null}]}"#
            )
        }

        let response = try await client.fetchGatewayProviderModels(
            providerName: nil,
            baseUrl: "https://a.example",
            modelsUrl: nil,
            providerType: "open_ai_responses",
            apiKey: "new-key"
        )

        XCTAssertFalse(response.ok)
        XCTAssertEqual(response.attempts.count, 2)
        XCTAssertEqual(response.attempts[0].status, 401)
        XCTAssertEqual(response.attempts[1].error, "连接超时")
    }

    func testMergedModelLinesKeepOrderDedupeAndAppend() {
        XCTAssertEqual(
            mergedModelLines(
                existing: "glm-5\n\nglm-5-air\nglm-5",
                fetched: ["glm-5", "glm-5-flash", " glm-5-air ", "glm-5v"]
            ),
            "glm-5\nglm-5-air\nglm-5-flash\nglm-5v"
        )
        XCTAssertEqual(mergedModelLines(existing: "", fetched: ["a", "b"]), "a\nb")
        XCTAssertEqual(mergedModelLines(existing: "a,b", fetched: []), "a\nb")
    }

    func testProviderFetchAttemptLinesTruncatePreviewAndCapCount() {
        let longPreview = String(repeating: "x", count: 200)
        let attempts = (1...6).map { index in
            ManageProviderModelsFetchResponse.Attempt(
                url: "https://example.test/\(index)",
                status: index == 1 ? 500 : nil,
                error: index == 1 ? nil : "连接失败",
                preview: index == 1 ? longPreview : nil
            )
        }

        let lines = providerFetchAttemptLines(attempts)

        XCTAssertEqual(lines.count, 4)
        XCTAssertTrue(lines[0].hasPrefix("https://example.test/1 — HTTP 500 — "))
        XCTAssertEqual(lines[0].count, "https://example.test/1 — HTTP 500 — ".count + 120)
        XCTAssertEqual(lines[1], "https://example.test/2 — 连接失败")
    }

    func testInferredClaudeModelAliasesMatchLegacyRules() {
        let aliases = inferredModelAliases(models: [
            " Claude-Opus-4-8 ",
            "claude-sonnet-4-6",
            "glm-5",
        ])

        XCTAssertEqual(aliases["opus-4.8"], " Claude-Opus-4-8 ")
        XCTAssertEqual(aliases["sonnet-4.6"], "claude-sonnet-4-6")
        XCTAssertEqual(aliases.count, 2)
    }

    func testInferredAliasSkippedWhenModelListAlreadyContainsAliasName() {
        let aliases = inferredModelAliases(models: [
            "claude-opus-4-8",
            "opus-4.8",
        ])

        XCTAssertTrue(aliases.isEmpty)
    }

    func testMergedModelAliasesKeepExplicitMappingPriority() {
        let merged = mergedModelAliases(
            models: ["claude-opus-4-8", "claude-sonnet-4-6"],
            explicit: ["opus-4.8": "my-custom-target"]
        )

        XCTAssertEqual(merged["opus-4.8"], "my-custom-target")
        XCTAssertEqual(merged["sonnet-4.6"], "claude-sonnet-4-6")
        XCTAssertEqual(merged.count, 2)
    }

    func testDaemonAutoRestartReadyRequiresThresholdAndCooldown() {
        let now = Date(timeIntervalSince1970: 100)
        XCTAssertFalse(AppModel.daemonAutoRestartReady(failures: 2, now: now, notBefore: nil))
        XCTAssertFalse(
            AppModel.daemonAutoRestartReady(
                failures: 3,
                now: now,
                notBefore: now.addingTimeInterval(1)
            )
        )
        XCTAssertTrue(AppModel.daemonAutoRestartReady(failures: 3, now: now, notBefore: now))
        XCTAssertTrue(AppModel.daemonAutoRestartReady(failures: 3, now: now, notBefore: nil))
    }

    @MainActor
    func testAppModelAutoRestartsDaemonAfterThreeFailuresAndHonorsCooldown() async {
        let client = makeClient { _ in
            MockResponse(statusCode: 503, json: #"{"error":"temporarily unavailable"}"#)
        }
        let launcher = RecordingDaemonLauncher()
        let model = AppModel(apiClient: client, daemonLauncher: launcher)

        await model.refresh()
        await model.refresh()
        XCTAssertEqual(launcher.startCount, 0)

        await model.refresh()
        await waitForDaemonLaunches(launcher, count: 1)
        XCTAssertEqual(launcher.startCount, 1)

        // Additional failures inside the 60-second cooldown never trigger a
        // second automatic restart.
        await model.refresh()
        await model.refresh()
        await model.refresh()
        try? await Task.sleep(for: .milliseconds(100))
        XCTAssertEqual(launcher.startCount, 1)
    }

    @MainActor
    func testAppModelResetsDaemonFailureCountAfterSuccessfulProbe() async {
        let healthResponses = MockResponseSequence([
            MockResponse(statusCode: 503, json: #"{"error":"down"}"#),
            MockResponse(statusCode: 503, json: #"{"error":"down"}"#),
            MockResponse(statusCode: 200, json: Self.healthJSON),
            MockResponse(statusCode: 503, json: #"{"error":"down"}"#),
        ])
        let client = makeClient { request in
            if request.url?.path == "/healthz" {
                return healthResponses.next()
            }
            return MockResponse(statusCode: 503, json: #"{"error":"down"}"#)
        }
        let launcher = RecordingDaemonLauncher()
        let model = AppModel(apiClient: client, daemonLauncher: launcher)

        // fail, fail, success: the success resets the consecutive counter.
        await model.refresh()
        await model.refresh()
        await model.refresh()
        // fail, fail: only two consecutive failures after the reset.
        await model.refresh()
        await model.refresh()
        try? await Task.sleep(for: .milliseconds(100))
        XCTAssertEqual(launcher.startCount, 0)

        // The third consecutive failure triggers the restart.
        await model.refresh()
        await waitForDaemonLaunches(launcher, count: 1)
        XCTAssertEqual(launcher.startCount, 1)
    }

    func testGitHubReleaseDecodesAndValidatesDownloadPage() throws {
        let release = try JSONDecoder().decode(
            GitHubRelease.self,
            from: Data(
                #"{"tag_name":"v0.5.1","html_url":"https://github.com/mps233/threadrelay/releases/tag/v0.5.1","body":"发布说明"}"#.utf8
            )
        )
        XCTAssertEqual(release.tagName, "v0.5.1")
        XCTAssertEqual(release.body, "发布说明")
        XCTAssertEqual(
            release.validatedURL,
            URL(string: "https://github.com/mps233/threadrelay/releases/tag/v0.5.1")
        )

        func validatedURL(_ htmlURL: String) throws -> URL? {
            try JSONDecoder().decode(
                GitHubRelease.self,
                from: Data(#"{"tag_name":"v1","html_url":"\#(htmlURL)"}"#.utf8)
            ).validatedURL
        }

        XCTAssertNil(try validatedURL("http://github.com/mps233/threadrelay/releases/tag/v1"))
        XCTAssertNil(try validatedURL("https://evil.example/mps233/threadrelay/releases/tag/v1"))
        XCTAssertNil(try validatedURL("https://github.com/other/repo/releases/tag/v1"))
    }

    func testUpdateCheckerComparesVersionsAndKeepsSilentFailures() async {
        MockURLProtocol.install { request in
            XCTAssertEqual(request.url?.host, "api.github.com")
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Accept"),
                "application/vnd.github+json"
            )
            return MockResponse(
                statusCode: 200,
                json: #"{"tag_name":"v0.6.0","html_url":"https://github.com/mps233/threadrelay/releases/tag/v0.6.0","body":null}"#
            )
        }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [MockURLProtocol.self]
        let session = URLSession(configuration: configuration)

        let update = await UpdateChecker.availableUpdate(session: session, currentVersion: "0.5.0")
        XCTAssertEqual(
            update,
            AvailableUpdate(
                version: "v0.6.0",
                url: URL(string: "https://github.com/mps233/threadrelay/releases/tag/v0.6.0")!
            )
        )

        let current = await UpdateChecker.availableUpdate(session: session, currentVersion: "0.6.0")
        XCTAssertNil(current)

        MockURLProtocol.install { _ in
            MockResponse(statusCode: 500, json: #"{"error":"boom"}"#)
        }
        let failed = await UpdateChecker.availableUpdate(session: session, currentVersion: "0.5.0")
        XCTAssertNil(failed)
    }

    func testUpdateCheckerRejectsReleaseWithForeignDownloadPage() async {
        MockURLProtocol.install { _ in
            MockResponse(
                statusCode: 200,
                json: #"{"tag_name":"v0.6.0","html_url":"https://evil.example/download","body":null}"#
            )
        }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [MockURLProtocol.self]
        let session = URLSession(configuration: configuration)

        do {
            _ = try await UpdateChecker.fetchLatestRelease(session: session, currentVersion: "0.5.0")
            XCTFail("Expected the unvalidated release URL to be rejected")
        } catch {
            XCTAssertEqual((error as? URLError)?.code, .unsupportedURL)
        }
        let update = await UpdateChecker.availableUpdate(session: session, currentVersion: "0.5.0")
        XCTAssertNil(update)
    }

    @MainActor
    func testBatchSessionMoveReportsPartialFailureAndKeepsFailedIds() async {
        let client = makeClient { request in
            switch (request.httpMethod, request.url?.path) {
            case ("POST", "/api/v1/manage/sessions/provider"):
                let threadId = Self.jsonBody(from: request)?["threadId"] as? String
                if threadId == "thread-a" {
                    return MockResponse(statusCode: 200, json: #"{"ok":true,"deleted":null}"#)
                }
                return MockResponse(statusCode: 500, json: #"{"error":"move failed"}"#)
            case ("GET", "/api/v1/manage/sessions"):
                return MockResponse(
                    statusCode: 200,
                    json: #"{"ok":true,"threads":[],"providers":["openai"],"total":0}"#
                )
            default:
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }
        let model = AppModel(apiClient: client)

        let result = await model.moveCodexSessions(ids: ["thread-a", "thread-b"], to: "openai")

        XCTAssertEqual(result.movedIds, ["thread-a"])
        XCTAssertEqual(result.failedIds, ["thread-b"])
        XCTAssertNil(model.actionFeedback)
        let message = model.managementOperationError
        XCTAssertNotNil(message)
        XCTAssertTrue(message?.contains("成功 1") == true)
        XCTAssertTrue(message?.contains("失败 1") == true)
        XCTAssertEqual(model.sectionErrors[.sessions], message)
    }

    @MainActor
    func testBatchSessionMoveSuccessPublishesUnifiedFeedback() async {
        let client = makeClient { request in
            switch (request.httpMethod, request.url?.path) {
            case ("POST", "/api/v1/manage/sessions/provider"):
                return MockResponse(statusCode: 200, json: #"{"ok":true,"deleted":null}"#)
            case ("GET", "/api/v1/manage/sessions"):
                return MockResponse(
                    statusCode: 200,
                    json: #"{"ok":true,"threads":[],"providers":["openai"],"total":0}"#
                )
            default:
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }
        let model = AppModel(apiClient: client)

        let result = await model.moveCodexSessions(ids: ["thread-a", "thread-b"], to: nil)

        XCTAssertEqual(result.movedIds, ["thread-a", "thread-b"])
        XCTAssertTrue(result.failedIds.isEmpty)
        XCTAssertNil(model.managementOperationError)
        XCTAssertEqual(model.actionFeedback?.message, "已移动 2 个会话")
    }

    private static let healthJSON = #"{"service":"threadrelay","apiMajor":1,"ready":true}"#
    private static let dashboardJSON = #"{"service":{"service":"threadrelay","apiMajor":1,"ready":true,"instanceId":"fixture-instance","pid":123,"startedAtMs":456},"bridgeRunning":true,"remoteControlConnected":true,"remoteControlHealthy":true,"executionClients":{"codexApp":{"configured":true,"connected":true},"vscode":{"configured":true,"connected":true},"cli":{"configured":false,"connected":false}},"messageChannels":{"telegram":{"accountCount":2,"connectedAccountCount":1},"feishu":{"accountCount":1,"connectedAccountCount":1},"wechat":{"accountCount":1,"connectedAccountCount":1},"wecom":{"accountCount":0,"connectedAccountCount":0}},"aiGatewayEnabled":true,"aiGatewayProviderCount":2,"requestLoggingEnabled":true}"#
    private static let imAccountsJSON = #"{"service":{"service":"threadrelay","apiMajor":1,"ready":true,"instanceId":"fixture-instance","pid":123,"startedAtMs":456},"accounts":[{"platform":"telegram","accountId":"telegram-main","displayName":"主 Telegram","enabled":true,"configured":true,"secretSet":true,"connecting":false,"polling":true,"connected":true,"lastError":null,"lastEventAtMs":1754000120000,"lastInboundAtMs":1754000100000},{"platform":"wecom","accountId":"wecom-offline","displayName":"企业微信","enabled":true,"configured":true,"secretSet":true,"connecting":false,"polling":false,"connected":false,"lastError":"连接失败","lastEventAtMs":null,"lastInboundAtMs":null}]}"#
    private static let lifecycleJSON = #"{"service":{"service":"threadrelay","apiMajor":1,"ready":true,"instanceId":"fixture-instance","pid":123,"startedAtMs":456},"executable":"/fixture/ThreadRelay","configPath":"/fixture/config.toml","bind":"127.0.0.1:3847","runtime":{"state":"active","productVersion":"0.5.0","apiMajor":1},"protectedWorkItems":{"aiGatewayRequests":0,"codexTurns":0,"imStreams":0,"pendingApprovals":0,"remoteControlRequests":0,"total":0},"management":{"state":"unmanaged","mode":"readOnly","canControl":false,"installationId":null,"leaseGeneration":null,"leaseExpiresAtMs":null}}"#
    private static let originalV1DashboardJSON = #"{"service":{"service":"threadrelay","apiMajor":1,"ready":true,"instanceId":"legacy-instance","pid":456,"startedAtMs":789},"bridgeRunning":true,"remoteControlConnected":false,"remoteControlHealthy":false,"codexAppConfigured":true,"imAccountCount":5,"connectedImAccountCount":3,"aiGatewayEnabled":false,"aiGatewayProviderCount":1,"requestLoggingEnabled":true}"#
    // Mirrors the daemon payload where the Anthropic TTL-split keys are
    // omitted entirely when unreported (serde skip_serializing_if).
    private static let requestLogsJSON = #"{"logs":[{"id":7,"requestId":"req-7","modelId":"model-a","stream":true,"channel":"primary","providerType":"openai_responses","status":"success","inputTokens":10,"outputTokens":20,"totalTokens":30,"readCacheTokens":null,"readCacheHitRate":null,"writeCacheTokens":null,"costUsd":0.01,"latencyMs":1200,"ttftMs":300,"createdAtMs":1754000120000,"createdAt":"2026-08-13T00:00:00Z","errorMessage":null,"upstreamRequestBodyBytes":128}]}"#

    private static func jsonBody(from request: URLRequest) -> [String: Any]? {
        let data: Data?
        if let httpBody = request.httpBody {
            data = httpBody
        } else if let stream = request.httpBodyStream {
            stream.open()
            defer { stream.close() }
            var result = Data()
            var buffer = [UInt8](repeating: 0, count: 4_096)
            while stream.hasBytesAvailable {
                let count = stream.read(&buffer, maxLength: buffer.count)
                guard count > 0 else { break }
                result.append(buffer, count: count)
            }
            data = result
        } else {
            data = nil
        }
        guard let data else { return nil }
        return try? JSONSerialization.jsonObject(with: data) as? [String: Any]
    }

    private func makeClient(
        baseURL: URL = URL(string: "https://threadrelay.test")!,
        credentialLoader: @escaping @Sendable () -> String? = { "fixture-token" },
        handler: @escaping MockURLProtocol.Handler
    ) -> APIClient {
        MockURLProtocol.install(handler)

        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [MockURLProtocol.self]
        let session = URLSession(configuration: configuration)
        return APIClient(
            baseURL: baseURL,
            session: session,
            credentialLoader: {
                credentialLoader().map {
                    [.init(token: $0, expectedInstanceId: nil)]
                } ?? []
            }
        )
    }

    private func makeDaemonLauncherFixture() throws -> (
        root: URL,
        configuration: DaemonLaunchConfiguration
    ) {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        let helper = root.appendingPathComponent("ThreadRelay.app/Contents/Helpers/threadrelay-daemon")
        try FileManager.default.createDirectory(
            at: helper.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        XCTAssertTrue(FileManager.default.createFile(atPath: helper.path, contents: Data()))
        try FileManager.default.setAttributes(
            [.posixPermissions: 0o755],
            ofItemAtPath: helper.path
        )
        return (
            root,
            DaemonLaunchConfiguration(
                helperURL: helper,
                configURL: root.appendingPathComponent("data/config.toml"),
                launchAgentURL: root.appendingPathComponent("home/Library/LaunchAgents/daemon.plist"),
                logURL: root.appendingPathComponent("data/logs/daemon.log"),
                homeURL: root.appendingPathComponent("home", isDirectory: true)
            )
        )
    }

    private func makeClient(
        credentialCandidatesLoader: @escaping @Sendable () -> [ManagementCredentialCandidate],
        handler: @escaping MockURLProtocol.Handler
    ) -> APIClient {
        MockURLProtocol.install(handler)

        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [MockURLProtocol.self]
        let session = URLSession(configuration: configuration)
        return APIClient(
            baseURL: URL(string: "https://threadrelay.test")!,
            session: session,
            credentialLoader: credentialCandidatesLoader
        )
    }

    private func makeClient(
        baseURL: URL,
        baseURLLoader: @escaping @Sendable () -> URL,
        handler: @escaping MockURLProtocol.Handler
    ) -> APIClient {
        MockURLProtocol.install(handler)

        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [MockURLProtocol.self]
        let session = URLSession(configuration: configuration)
        return APIClient(
            baseURL: baseURL,
            session: session,
            credentialLoader: { [.init(token: "fixture-token", expectedInstanceId: nil)] },
            baseURLLoader: baseURLLoader
        )
    }

    private func assertProbeError(
        _ expectedError: APIClientError,
        from client: APIClient,
        file: StaticString = #filePath,
        line: UInt = #line
    ) async {
        do {
            _ = try await client.probe()
            XCTFail("Expected probe to fail", file: file, line: line)
        } catch let error as APIClientError {
            XCTAssertEqual(error, expectedError, file: file, line: line)
        } catch {
            XCTFail("Expected APIClientError, received \(error)", file: file, line: line)
        }
    }

    private func assertDashboardError(
        _ expectedError: APIClientError,
        from client: APIClient,
        file: StaticString = #filePath,
        line: UInt = #line
    ) async {
        do {
            _ = try await client.fetchDashboard(bearerToken: "fixture-token")
            XCTFail("Expected dashboard request to fail", file: file, line: line)
        } catch let error as APIClientError {
            XCTAssertEqual(error, expectedError, file: file, line: line)
        } catch {
            XCTFail("Expected APIClientError, received \(error)", file: file, line: line)
        }
    }

    private func assertLifecycleError(
        _ expectedError: APIClientError,
        from client: APIClient,
        file: StaticString = #filePath,
        line: UInt = #line
    ) async {
        do {
            _ = try await client.fetchLifecycle(bearerToken: "fixture-token")
            XCTFail("Expected lifecycle request to fail", file: file, line: line)
        } catch let error as APIClientError {
            XCTAssertEqual(error, expectedError, file: file, line: line)
        } catch {
            XCTFail("Expected APIClientError, received \(error)", file: file, line: line)
        }
    }

    @MainActor
    private func waitForDaemonLaunches(
        _ launcher: RecordingDaemonLauncher,
        count: Int,
        timeoutMs: Int = 2_000
    ) async {
        for _ in 0..<(timeoutMs / 10) {
            if launcher.startCount >= count { return }
            try? await Task.sleep(for: .milliseconds(10))
        }
    }
}

/// DaemonLaunching mock that records start attempts. It throws a launch
/// error so the recovery path returns quickly instead of polling readiness.
private final class RecordingDaemonLauncher: DaemonLaunching, @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0

    var startCount: Int {
        lock.withLock { count }
    }

    func startIfNeeded() async throws {
        lock.withLock { count += 1 }
        throw DaemonLaunchError.helperMissing
    }
}

private struct MockResponse: Sendable {
    let statusCode: Int
    let body: Data

    init(statusCode: Int, json: String) {
        self.statusCode = statusCode
        body = Data(json.utf8)
    }
}

private final class MockResponseSequence: @unchecked Sendable {
    private let lock = NSLock()
    private var responses: [MockResponse]

    init(_ responses: [MockResponse]) {
        self.responses = responses
    }

    func next() -> MockResponse {
        lock.withLock {
            guard responses.count > 1 else {
                return responses[0]
            }
            return responses.removeFirst()
        }
    }
}

private final class StringSequence: @unchecked Sendable {
    private let lock = NSLock()
    private var values: [String]

    init(_ values: [String]) {
        self.values = values
    }

    func next() -> String? {
        lock.withLock {
            guard values.count > 1 else { return values.first }
            return values.removeFirst()
        }
    }
}

private final class IntCounter: @unchecked Sendable {
    private let lock = NSLock()
    private var value = 0

    func next() -> Int {
        lock.withLock {
            value += 1
            return value
        }
    }
}

private final class StringRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var recordedValues: [String] = []

    var values: [String] {
        lock.withLock { recordedValues }
    }

    func record(_ value: String) {
        lock.withLock {
            recordedValues.append(value)
        }
    }
}

private final class RequestRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var recordedPaths: [String] = []

    var paths: [String] {
        lock.withLock { recordedPaths }
    }

    func record(_ path: String?) {
        lock.withLock {
            recordedPaths.append(path ?? "")
        }
    }
}

private final class URLRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var recordedURLs: [URL] = []

    var urls: [URL] {
        lock.withLock { recordedURLs }
    }

    func record(_ url: URL?) {
        guard let url else { return }
        lock.withLock {
            recordedURLs.append(url)
        }
    }
}

private final class CommandInvocationRecorder: @unchecked Sendable {
    typealias Handler = @Sendable ([String]) -> CommandResult

    private let lock = NSLock()
    private let handler: Handler
    private var recordedArguments: [[String]] = []

    init(handler: @escaping Handler) {
        self.handler = handler
    }

    var arguments: [[String]] {
        lock.withLock { recordedArguments }
    }

    func run(_: URL, arguments: [String]) -> CommandResult {
        lock.withLock {
            recordedArguments.append(arguments)
        }
        return handler(arguments)
    }
}

private final class MockURLProtocol: URLProtocol, @unchecked Sendable {
    typealias Handler = @Sendable (URLRequest) -> MockResponse

    private static let lock = NSLock()
    nonisolated(unsafe) private static var installedHandler: Handler?

    static func install(_ handler: @escaping Handler) {
        lock.withLock {
            installedHandler = handler
        }
    }

    static func reset() {
        lock.withLock {
            installedHandler = nil
        }
    }

    override class func canInit(with _: URLRequest) -> Bool {
        true
    }

    override class func canonicalRequest(for request: URLRequest) -> URLRequest {
        request
    }

    override func startLoading() {
        guard let handler = Self.lock.withLock({ Self.installedHandler }),
              let url = request.url
        else {
            client?.urlProtocol(self, didFailWithError: URLError(.badServerResponse))
            return
        }

        let mock = handler(request)
        guard let response = HTTPURLResponse(
                  url: url,
                  statusCode: mock.statusCode,
                  httpVersion: "HTTP/1.1",
                  headerFields: ["Content-Type": "application/json"]
              )
        else {
            client?.urlProtocol(self, didFailWithError: URLError(.badServerResponse))
            return
        }

        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: mock.body)
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}
}
