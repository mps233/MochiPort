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

    private static let healthJSON = #"{"service":"threadrelay","apiMajor":1,"ready":true}"#
    private static let dashboardJSON = #"{"service":{"service":"threadrelay","apiMajor":1,"ready":true,"instanceId":"fixture-instance","pid":123,"startedAtMs":456},"bridgeRunning":true,"remoteControlConnected":true,"remoteControlHealthy":true,"executionClients":{"codexApp":{"configured":true,"connected":true},"vscode":{"configured":true,"connected":true},"cli":{"configured":false,"connected":false}},"messageChannels":{"telegram":{"accountCount":2,"connectedAccountCount":1},"feishu":{"accountCount":1,"connectedAccountCount":1},"wechat":{"accountCount":1,"connectedAccountCount":1},"wecom":{"accountCount":0,"connectedAccountCount":0}},"aiGatewayEnabled":true,"aiGatewayProviderCount":2,"requestLoggingEnabled":true}"#
    private static let imAccountsJSON = #"{"service":{"service":"threadrelay","apiMajor":1,"ready":true,"instanceId":"fixture-instance","pid":123,"startedAtMs":456},"accounts":[{"platform":"telegram","accountId":"telegram-main","displayName":"主 Telegram","enabled":true,"configured":true,"secretSet":true,"connecting":false,"polling":true,"connected":true,"lastError":null,"lastEventAtMs":1754000120000,"lastInboundAtMs":1754000100000},{"platform":"wecom","accountId":"wecom-offline","displayName":"企业微信","enabled":true,"configured":true,"secretSet":true,"connecting":false,"polling":false,"connected":false,"lastError":"连接失败","lastEventAtMs":null,"lastInboundAtMs":null}]}"#
    private static let lifecycleJSON = #"{"service":{"service":"threadrelay","apiMajor":1,"ready":true,"instanceId":"fixture-instance","pid":123,"startedAtMs":456},"executable":"/fixture/ThreadRelay","configPath":"/fixture/config.toml","bind":"127.0.0.1:3847","runtime":{"state":"active","productVersion":"0.5.0","apiMajor":1},"protectedWorkItems":{"aiGatewayRequests":0,"codexTurns":0,"imStreams":0,"pendingApprovals":0,"remoteControlRequests":0,"total":0},"management":{"state":"unmanaged","mode":"readOnly","canControl":false,"installationId":null,"leaseGeneration":null,"leaseExpiresAtMs":null}}"#
    private static let originalV1DashboardJSON = #"{"service":{"service":"threadrelay","apiMajor":1,"ready":true,"instanceId":"legacy-instance","pid":456,"startedAtMs":789},"bridgeRunning":true,"remoteControlConnected":false,"remoteControlHealthy":false,"codexAppConfigured":true,"imAccountCount":5,"connectedImAccountCount":3,"aiGatewayEnabled":false,"aiGatewayProviderCount":1,"requestLoggingEnabled":true}"#

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
