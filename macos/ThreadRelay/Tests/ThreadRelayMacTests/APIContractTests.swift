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
            ["Overview", "Codex", "Sessions", "Messaging Channels", "AI Gateway", "Request Logs"]
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
            "This ThreadRelay service uses unsupported management API version 2."
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
    private static let originalV1DashboardJSON = #"{"service":{"service":"threadrelay","apiMajor":1,"ready":true,"instanceId":"legacy-instance","pid":456,"startedAtMs":789},"bridgeRunning":true,"remoteControlConnected":false,"remoteControlHealthy":false,"codexAppConfigured":true,"imAccountCount":5,"connectedImAccountCount":3,"aiGatewayEnabled":false,"aiGatewayProviderCount":1,"requestLoggingEnabled":true}"#

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
