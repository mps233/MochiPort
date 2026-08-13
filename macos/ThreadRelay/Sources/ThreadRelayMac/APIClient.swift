import Foundation

struct HealthResponse: Codable, Equatable {
    let service: String
    let apiMajor: Int
    let ready: Bool
}

struct ManageLogDirectory: Decodable, Equatable {
    let directory: String
    let instanceId: String
}

struct ManageIMAccount: Decodable, Equatable, Identifiable {
    let platform: String
    let accountId: String
    let displayName: String?
    let enabled: Bool
    let configured: Bool
    let secretSet: Bool
    let connecting: Bool
    let polling: Bool
    let connected: Bool
    let lastError: String?
    let lastEventAtMs: Int64?
    let lastInboundAtMs: Int64?

    var id: String { "\(platform):\(accountId)" }
}

struct ManageIMAccountsResponse: Decodable, Equatable {
    let service: ManageDashboard.Service
    let accounts: [ManageIMAccount]
}

/// Common shape of authenticated account mutation responses so one request
/// path can decode and acknowledge every mutation variant.
protocol ManageMutationResponse: Decodable {
    var ok: Bool { get }
}

struct ManageIMAccountMutationResponse: Decodable, Equatable, ManageMutationResponse {
    let ok: Bool
    let platform: String
    let accountId: String
    let enabled: Bool?
}

struct ManageIMAccountConfigureResponse: Decodable, Equatable, ManageMutationResponse {
    let ok: Bool
    let platform: String
    let accountId: String
    let displayName: String?
}

struct ManageFeishuOnboardingStart: Decodable, Equatable {
    let verificationUri: String
    let verificationUriComplete: String
    let deviceCode: String
    let expiresIn: Int
    let interval: Int
    let qrSvg: String
}

struct ManageFeishuOnboardingPoll: Decodable, Equatable {
    let done: Bool
    let appId: String?
    let displayName: String?
    let error: String?
    let errorDescription: String?
}

struct ManageWechatOnboardingStart: Decodable, Equatable {
    let sessionKey: String
    let qrcodeUrl: String
    let qrSvg: String
    let expiresIn: Int
}

struct ManageWechatOnboardingPoll: Decodable, Equatable {
    let done: Bool
    let status: String?
    let needVerifyCode: Bool?
    let accountId: String?
    let alreadyConnected: Bool?
    let error: String?
}

struct ManageWecomOnboardingStart: Decodable, Equatable {
    let sessionKey: String
    let qrcodeUrl: String
    let qrSvg: String
    let expiresIn: Int
    let interval: Int
}

struct ManageWecomOnboardingPoll: Decodable, Equatable {
    let done: Bool
    let status: String?
    let accountId: String?
    let error: String?
}

struct ManageLifecycle: Decodable, Equatable {
    struct Service: Decodable, Equatable {
        let service: String
        let apiMajor: Int
        let ready: Bool
        let instanceId: String
        let pid: Int
        let startedAtMs: Int64
    }

    struct Runtime: Decodable, Equatable {
        let state: String
        let productVersion: String
        let apiMajor: Int
    }

    struct ProtectedWorkItems: Decodable, Equatable {
        let aiGatewayRequests: Int
        let codexTurns: Int
        let imStreams: Int
        let pendingApprovals: Int
        let remoteControlRequests: Int
        let total: Int
    }

    struct Management: Decodable, Equatable {
        let state: String
        let mode: String
        let canControl: Bool
        let installationId: String?
        let leaseGeneration: Int64?
        let leaseExpiresAtMs: Int64?
    }

    let service: Service
    let executable: String
    let configPath: String
    let bind: String
    let runtime: Runtime
    let protectedWorkItems: ProtectedWorkItems
    let management: Management
}

struct ManageDashboard: Decodable, Equatable {
    struct Service: Codable, Equatable {
        let service: String
        let apiMajor: Int
        let ready: Bool
        let instanceId: String
        let pid: Int
        let startedAtMs: Int64
    }

    struct Endpoint: Codable, Equatable {
        let configured: Bool
        let connected: Bool
    }

    struct ExecutionClients: Codable, Equatable {
        let codexApp: Endpoint
        let vscode: Endpoint
        let cli: Endpoint
    }

    struct MessageChannel: Codable, Equatable {
        let accountCount: Int
        let connectedAccountCount: Int
    }

    struct MessageChannels: Decodable, Equatable {
        let telegram: MessageChannel
        let feishu: MessageChannel
        let wechat: MessageChannel
        let wecom: MessageChannel

        /// Aggregate counts from the original v1 payload, whose schema did
        /// not identify the platform for each account.
        let legacyUnattributed: MessageChannel

        init(
            telegram: MessageChannel,
            feishu: MessageChannel,
            wechat: MessageChannel,
            wecom: MessageChannel,
            legacyUnattributed: MessageChannel = .zero
        ) {
            self.telegram = telegram
            self.feishu = feishu
            self.wechat = wechat
            self.wecom = wecom
            self.legacyUnattributed = legacyUnattributed
        }

        private enum CodingKeys: String, CodingKey {
            case telegram
            case feishu
            case wechat
            case wecom
        }

        init(from decoder: Decoder) throws {
            let container = try decoder.container(keyedBy: CodingKeys.self)
            telegram = try container.decode(MessageChannel.self, forKey: .telegram)
            feishu = try container.decode(MessageChannel.self, forKey: .feishu)
            wechat = try container.decode(MessageChannel.self, forKey: .wechat)
            wecom = try container.decode(MessageChannel.self, forKey: .wecom)
            legacyUnattributed = .zero
        }
    }

    let service: Service
    let bridgeRunning: Bool
    let remoteControlConnected: Bool
    let remoteControlHealthy: Bool
    let executionClients: ExecutionClients
    let messageChannels: MessageChannels
    let aiGatewayEnabled: Bool
    let aiGatewayProviderCount: Int
    let requestLoggingEnabled: Bool

    init(
        service: Service,
        bridgeRunning: Bool,
        remoteControlConnected: Bool,
        remoteControlHealthy: Bool,
        executionClients: ExecutionClients,
        messageChannels: MessageChannels,
        aiGatewayEnabled: Bool,
        aiGatewayProviderCount: Int,
        requestLoggingEnabled: Bool
    ) {
        self.service = service
        self.bridgeRunning = bridgeRunning
        self.remoteControlConnected = remoteControlConnected
        self.remoteControlHealthy = remoteControlHealthy
        self.executionClients = executionClients
        self.messageChannels = messageChannels
        self.aiGatewayEnabled = aiGatewayEnabled
        self.aiGatewayProviderCount = aiGatewayProviderCount
        self.requestLoggingEnabled = requestLoggingEnabled
    }

    private enum CodingKeys: String, CodingKey {
        case service
        case bridgeRunning
        case remoteControlConnected
        case remoteControlHealthy
        case executionClients
        case messageChannels
        case codexAppConfigured
        case imAccountCount
        case connectedImAccountCount
        case aiGatewayEnabled
        case aiGatewayProviderCount
        case requestLoggingEnabled
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)

        service = try container.decode(Service.self, forKey: .service)
        bridgeRunning = try container.decode(Bool.self, forKey: .bridgeRunning)
        remoteControlConnected = try container.decode(Bool.self, forKey: .remoteControlConnected)
        remoteControlHealthy = try container.decode(Bool.self, forKey: .remoteControlHealthy)
        aiGatewayEnabled = try container.decode(Bool.self, forKey: .aiGatewayEnabled)
        aiGatewayProviderCount = try container.decode(Int.self, forKey: .aiGatewayProviderCount)
        requestLoggingEnabled = try container.decode(Bool.self, forKey: .requestLoggingEnabled)

        if let currentExecutionClients = try container.decodeIfPresent(
            ExecutionClients.self,
            forKey: .executionClients
        ) {
            executionClients = currentExecutionClients
        } else {
            let configured = try container.decodeIfPresent(Bool.self, forKey: .codexAppConfigured)
            guard let configured else {
                throw DecodingError.keyNotFound(
                    CodingKeys.executionClients,
                    .init(
                        codingPath: decoder.codingPath,
                        debugDescription: "Expected executionClients or codexAppConfigured"
                    )
                )
            }
            executionClients = ExecutionClients(
                codexApp: Endpoint(configured: configured, connected: false),
                vscode: .unavailable,
                cli: .unavailable
            )
        }

        if let currentMessageChannels = try container.decodeIfPresent(
            MessageChannels.self,
            forKey: .messageChannels
        ) {
            messageChannels = currentMessageChannels
        } else {
            let accountCount = try container.decodeIfPresent(Int.self, forKey: .imAccountCount)
            let connectedAccountCount = try container.decodeIfPresent(
                Int.self,
                forKey: .connectedImAccountCount
            )
            guard let accountCount, let connectedAccountCount else {
                throw DecodingError.keyNotFound(
                    CodingKeys.messageChannels,
                    .init(
                        codingPath: decoder.codingPath,
                        debugDescription: "Expected messageChannels or legacy IM account counts"
                    )
                )
            }
            messageChannels = MessageChannels(
                telegram: .zero,
                feishu: .zero,
                wechat: .zero,
                wecom: .zero,
                legacyUnattributed: MessageChannel(
                    accountCount: accountCount,
                    connectedAccountCount: connectedAccountCount
                )
            )
        }
    }
}

private extension ManageDashboard.Endpoint {
    static let unavailable = Self(configured: false, connected: false)
}

private extension ManageDashboard.MessageChannel {
    static let zero = Self(accountCount: 0, connectedAccountCount: 0)
}

private struct ManagementControlFile: Decodable {
    let managementToken: String
}

struct ActiveDaemonLocator: Decodable, Equatable {
    let service: String
    let apiMajor: Int
    let instanceId: String
    let pid: Int
    let startedAtMs: Int64
    let baseURL: String
    let controlFile: String

    private enum CodingKeys: String, CodingKey {
        case service
        case apiMajor
        case instanceId
        case pid
        case startedAtMs
        case baseURL = "baseUrl"
        case controlFile
    }

    var validatedBaseURL: URL? {
        guard let url = URL(string: baseURL),
              let components = URLComponents(url: url, resolvingAgainstBaseURL: false),
              components.scheme?.lowercased() == "http",
              components.user == nil,
              components.password == nil,
              components.query == nil,
              components.fragment == nil,
              components.path.isEmpty || components.path == "/",
              let port = components.port,
              (1...65_535).contains(port),
              let host = components.host?.lowercased(),
              host == "127.0.0.1" || host == "::1" || host == "[::1]"
        else {
            return nil
        }
        return url
    }
}

struct ManagementCredentialCandidate: Equatable {
    let token: String
    let expectedInstanceId: String?
}

private struct ManagementConnection {
    let baseURL: URL
    let credentials: @Sendable () -> [ManagementCredentialCandidate]
}

enum ManagementCredentialStore {
    static func loadLocator(
        from path: URL? = activeDaemonLocatorPath()
    ) -> ActiveDaemonLocator? {
        guard let path,
              let data = try? Data(contentsOf: path),
              let locator = try? JSONDecoder().decode(ActiveDaemonLocator.self, from: data),
              locator.service == "threadrelay",
              locator.apiMajor == 1,
              !locator.instanceId.isEmpty,
              locator.pid > 0,
              locator.validatedBaseURL != nil,
              !locator.controlFile.isEmpty
        else {
            return nil
        }
        return locator
    }

    static func loadCredentialCandidates(
        locator: ActiveDaemonLocator? = loadLocator(),
        fallbackPaths: [URL] = candidatePaths()
    ) -> [ManagementCredentialCandidate] {
        var candidates: [ManagementCredentialCandidate] = []
        if let locator,
           let token = loadToken(from: URL(fileURLWithPath: locator.controlFile)) {
            candidates.append(
                ManagementCredentialCandidate(
                    token: token,
                    expectedInstanceId: locator.instanceId
                )
            )
        }
        for token in loadCandidates(from: fallbackPaths)
            where !candidates.contains(where: { $0.token == token && $0.expectedInstanceId == nil })
        {
            // Keep an unconstrained retry even when the locator references the
            // same token. The locator may be stale after a daemon replacement.
            candidates.append(ManagementCredentialCandidate(token: token, expectedInstanceId: nil))
        }
        return candidates
    }

    static func loadCandidates(
        from paths: [URL] = candidatePaths()
    ) -> [String] {
        var candidates: [String] = []
        for path in paths {
            guard let data = try? Data(contentsOf: path),
                  let file = try? JSONDecoder().decode(ManagementControlFile.self, from: data),
                  isValid(file.managementToken)
            else {
                continue
            }
            if !candidates.contains(file.managementToken) {
                candidates.append(file.managementToken)
            }
        }
        return candidates
    }

    static func activeDaemonLocatorPath(
        applicationSupport: URL? = FileManager.default.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        ).first
    ) -> URL? {
        applicationSupport?
            .appendingPathComponent("ThreadRelay", isDirectory: true)
            .appendingPathComponent("threadrelay-active-daemon.json")
    }

    static func candidatePaths(
        environment: [String: String] = ProcessInfo.processInfo.environment,
        applicationSupport: URL? = FileManager.default.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        ).first
    ) -> [URL] {
        var directories: [URL] = []
        if let home = environment["THREADRELAY_HOME"] {
            directories.append(URL(fileURLWithPath: home, isDirectory: true))
        }
        if let home = environment["CODEXHUB_HOME"] {
            directories.append(URL(fileURLWithPath: home, isDirectory: true))
        }
        if let applicationSupport {
            directories.append(applicationSupport.appendingPathComponent("ThreadRelay", isDirectory: true))
            directories.append(applicationSupport.appendingPathComponent("CodexHub", isDirectory: true))
        }
        return directories.map { $0.appendingPathComponent("threadrelay-control.json") }
    }

    private static func isValid(_ token: String) -> Bool {
        !token.isEmpty && token.count <= 256 && token.trimmingCharacters(in: .whitespacesAndNewlines) == token
    }

    private static func loadToken(from path: URL) -> String? {
        guard let data = try? Data(contentsOf: path),
              let file = try? JSONDecoder().decode(ManagementControlFile.self, from: data),
              isValid(file.managementToken)
        else {
            return nil
        }
        return file.managementToken
    }
}

private struct LegacyStatusResponse: Codable {
    let service: String
}

enum ServiceProbe: Equatable {
    case versioned(HealthResponse)
    case legacy
}

enum APIClientError: LocalizedError, Equatable {
    case invalidResponse
    case incompatibleService
    case unsupportedAPIMajor(Int)
    case unauthorized
    case featureUnavailable
    case operationFailed(String)

    var errorDescription: String? {
        switch self {
        case .invalidResponse: "本地服务返回了无效响应。"
        case .incompatibleService: "ThreadRelay 端口正被其他服务占用。"
        case let .unsupportedAPIMajor(apiMajor):
            "当前 ThreadRelay 使用了不受支持的管理 API 版本 \(apiMajor)。"
        case .unauthorized: "本地服务拒绝了管理凭据。"
        case .featureUnavailable: "当前后台服务尚未支持此管理功能。"
        case let .operationFailed(message): message
        }
    }
}

struct APIClient {
    var session: URLSession = .shared
    private var connectionLoader: @Sendable () -> ManagementConnection

    init(
        baseURL: URL = URL(string: "http://127.0.0.1:3847")!,
        session: URLSession = .shared,
        credentialLoader: @escaping @Sendable () -> [ManagementCredentialCandidate],
        baseURLLoader: (@Sendable () -> URL)? = nil
    ) {
        self.session = session
        let resolvedBaseURL = baseURLLoader?() ?? baseURL
        connectionLoader = {
            ManagementConnection(baseURL: resolvedBaseURL, credentials: credentialLoader)
        }
    }

    init(
        baseURL: URL = URL(string: "http://127.0.0.1:3847")!,
        session: URLSession = .shared
    ) {
        self.session = session
        connectionLoader = {
            let locator = ManagementCredentialStore.loadLocator()
            return ManagementConnection(
                baseURL: locator?.validatedBaseURL ?? baseURL,
                credentials: {
                    ManagementCredentialStore.loadCredentialCandidates(locator: locator)
                }
            )
        }
    }

    func probe() async throws -> ServiceProbe {
        let baseURL = connectionLoader().baseURL
        let url = baseURL.appending(path: "healthz")
        var request = URLRequest(url: url)
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.timeoutInterval = 3

        let (data, response) = try await session.data(for: request)
        guard let httpResponse = response as? HTTPURLResponse else {
            throw APIClientError.invalidResponse
        }

        if httpResponse.statusCode == 404 {
            return try await legacyProbe(baseURL: baseURL)
        }
        guard httpResponse.statusCode == 200 else { throw APIClientError.invalidResponse }

        let health: HealthResponse
        do {
            health = try JSONDecoder().decode(HealthResponse.self, from: data)
        } catch {
            throw APIClientError.invalidResponse
        }
        guard health.service == "threadrelay" else {
            throw APIClientError.incompatibleService
        }
        guard health.apiMajor == 1 else {
            throw APIClientError.unsupportedAPIMajor(health.apiMajor)
        }
        return .versioned(health)
    }

    func fetchDashboard(bearerToken: String) async throws -> ManageDashboard {
        try await fetchDashboard(baseURL: connectionLoader().baseURL, bearerToken: bearerToken)
    }

    func fetchLogDirectory(bearerToken: String) async throws -> URL {
        try await fetchLogDirectoryPayload(
            baseURL: connectionLoader().baseURL,
            bearerToken: bearerToken
        ).validatedURL()
    }

    func fetchLifecycle(bearerToken: String) async throws -> ManageLifecycle {
        try await fetchLifecycle(
            baseURL: connectionLoader().baseURL,
            bearerToken: bearerToken
        )
    }

    func fetchIMAccounts(bearerToken: String) async throws -> ManageIMAccountsResponse {
        try await fetchIMAccounts(
            baseURL: connectionLoader().baseURL,
            bearerToken: bearerToken
        )
    }

    func dashboard() async throws -> ManageDashboard {
        let connection = connectionLoader()
        let baseURL = connection.baseURL
        let candidates = connection.credentials()
        guard !candidates.isEmpty else { throw APIClientError.unauthorized }

        for candidate in candidates {
            do {
                let dashboard = try await fetchDashboard(
                    baseURL: baseURL,
                    bearerToken: candidate.token
                )
                if let expectedInstanceId = candidate.expectedInstanceId,
                   dashboard.service.instanceId != expectedInstanceId {
                    continue
                }
                return dashboard
            } catch APIClientError.unauthorized {
                continue
            }
        }
        throw APIClientError.unauthorized
    }

    func logDirectory() async throws -> URL {
        let connection = connectionLoader()
        let baseURL = connection.baseURL
        let candidates = connection.credentials()
        guard !candidates.isEmpty else { throw APIClientError.unauthorized }

        for candidate in candidates {
            do {
                let payload = try await fetchLogDirectoryPayload(
                    baseURL: baseURL,
                    bearerToken: candidate.token
                )
                if let expectedInstanceId = candidate.expectedInstanceId,
                   payload.instanceId != expectedInstanceId {
                    continue
                }
                return try payload.validatedURL()
            } catch APIClientError.unauthorized {
                continue
            }
        }
        throw APIClientError.unauthorized
    }

    func lifecycle() async throws -> ManageLifecycle {
        let connection = connectionLoader()
        let baseURL = connection.baseURL
        let candidates = connection.credentials()
        guard !candidates.isEmpty else { throw APIClientError.unauthorized }

        for candidate in candidates {
            do {
                let lifecycle = try await fetchLifecycle(
                    baseURL: baseURL,
                    bearerToken: candidate.token
                )
                if let expectedInstanceId = candidate.expectedInstanceId,
                   lifecycle.service.instanceId != expectedInstanceId {
                    continue
                }
                return lifecycle
            } catch APIClientError.unauthorized {
                continue
            }
        }
        throw APIClientError.unauthorized
    }

    func imAccounts() async throws -> [ManageIMAccount] {
        let connection = connectionLoader()
        let candidates = connection.credentials()
        guard !candidates.isEmpty else { throw APIClientError.unauthorized }

        for candidate in candidates {
            do {
                let response = try await fetchIMAccounts(
                    baseURL: connection.baseURL,
                    bearerToken: candidate.token
                )
                if let expectedInstanceId = candidate.expectedInstanceId,
                   response.service.instanceId != expectedInstanceId {
                    continue
                }
                return response.accounts
            } catch APIClientError.unauthorized {
                continue
            }
        }
        throw APIClientError.unauthorized
    }

    func setIMAccountEnabled(
        platform: String,
        accountId: String,
        enabled: Bool
    ) async throws -> ManageIMAccountMutationResponse {
        let body = IMAccountEnabledRequest(
            platform: platform,
            accountId: accountId,
            enabled: enabled
        )
        return try await performIMMutation(
            path: "api/v1/manage/im/account/enabled",
            body: body
        )
    }

    func deleteIMAccount(
        platform: String,
        accountId: String
    ) async throws -> ManageIMAccountMutationResponse {
        let body = IMAccountDeleteRequest(platform: platform, accountId: accountId)
        return try await performIMMutation(
            path: "api/v1/manage/im/account/delete",
            body: body
        )
    }

    /// Submit a Telegram bot token for verification and persistence. The
    /// credential is write-only; the response never echoes it back.
    func configureTelegramAccount(
        botToken: String,
        mentionOnly: Bool
    ) async throws -> ManageIMAccountConfigureResponse {
        let body = TelegramConfigureRequest(botToken: botToken, mentionOnly: mentionOnly)
        return try await performIMMutation(
            path: "api/v1/manage/im/account/telegram",
            body: body
        )
    }

    /// Submit manually entered Feishu app credentials. The daemon validates
    /// them against the Feishu open platform before persisting; the response
    /// never echoes the secret.
    func configureFeishuAccount(
        appId: String,
        appSecret: String
    ) async throws -> ManageIMAccountConfigureResponse {
        let body = FeishuConfigureRequest(appId: appId, appSecret: appSecret)
        return try await performIMMutation(
            path: "api/v1/manage/im/account/feishu",
            body: body
        )
    }

    func startFeishuOnboarding() async throws -> ManageFeishuOnboardingStart {
        try await performManagePOST(
            path: "api/v1/manage/im/onboarding/feishu/start",
            body: EmptyRequestBody()
        )
    }

    func pollFeishuOnboarding(deviceCode: String) async throws -> ManageFeishuOnboardingPoll {
        try await performManagePOST(
            path: "api/v1/manage/im/onboarding/feishu/poll",
            body: FeishuOnboardingPollRequest(deviceCode: deviceCode)
        )
    }

    func startWechatOnboarding() async throws -> ManageWechatOnboardingStart {
        try await performManagePOST(
            path: "api/v1/manage/im/onboarding/wechat/start",
            body: EmptyRequestBody()
        )
    }

    func pollWechatOnboarding(
        sessionKey: String,
        verifyCode: String? = nil
    ) async throws -> ManageWechatOnboardingPoll {
        try await performManagePOST(
            path: "api/v1/manage/im/onboarding/wechat/poll",
            body: WechatOnboardingPollRequest(sessionKey: sessionKey, verifyCode: verifyCode)
        )
    }

    func startWecomOnboarding() async throws -> ManageWecomOnboardingStart {
        try await performManagePOST(
            path: "api/v1/manage/im/onboarding/wecom/start",
            body: EmptyRequestBody()
        )
    }

    func pollWecomOnboarding(sessionKey: String) async throws -> ManageWecomOnboardingPoll {
        try await performManagePOST(
            path: "api/v1/manage/im/onboarding/wecom/poll",
            body: WecomOnboardingPollRequest(sessionKey: sessionKey)
        )
    }

    private struct IMAccountEnabledRequest: Encodable {
        let platform: String
        let accountId: String
        let enabled: Bool
    }

    private struct IMAccountDeleteRequest: Encodable {
        let platform: String
        let accountId: String
    }

    private struct TelegramConfigureRequest: Encodable {
        let botToken: String
        let mentionOnly: Bool
    }

    private struct FeishuConfigureRequest: Encodable {
        let appId: String
        let appSecret: String
    }

    private struct FeishuOnboardingPollRequest: Encodable {
        let deviceCode: String
    }

    private struct WechatOnboardingPollRequest: Encodable {
        let sessionKey: String
        let verifyCode: String?
    }

    private struct WecomOnboardingPollRequest: Encodable {
        let sessionKey: String
    }

    private struct EmptyRequestBody: Encodable {}

    private func fetchDashboard(
        baseURL: URL,
        bearerToken: String
    ) async throws -> ManageDashboard {
        let (data, response) = try await request(
            baseURL: baseURL,
            path: "api/v1/manage/dashboard",
            bearerToken: bearerToken
        )
        guard let httpResponse = response as? HTTPURLResponse else {
            throw APIClientError.invalidResponse
        }
        guard httpResponse.statusCode != 401 else { throw APIClientError.unauthorized }
        guard httpResponse.statusCode == 200 else { throw APIClientError.invalidResponse }
        do {
            let dashboard = try JSONDecoder().decode(ManageDashboard.self, from: data)
            guard dashboard.service.service == "threadrelay" else {
                throw APIClientError.incompatibleService
            }
            guard dashboard.service.apiMajor == 1 else {
                throw APIClientError.unsupportedAPIMajor(dashboard.service.apiMajor)
            }
            return dashboard
        } catch let error as APIClientError {
            throw error
        } catch {
            throw APIClientError.invalidResponse
        }
    }

    private func fetchLogDirectoryPayload(
        baseURL: URL,
        bearerToken: String
    ) async throws -> ManageLogDirectory {
        let (data, response) = try await request(
            baseURL: baseURL,
            path: "api/v1/manage/log-directory",
            bearerToken: bearerToken
        )
        guard let httpResponse = response as? HTTPURLResponse else {
            throw APIClientError.invalidResponse
        }
        guard httpResponse.statusCode != 401 else { throw APIClientError.unauthorized }
        guard httpResponse.statusCode == 200 else { throw APIClientError.invalidResponse }
        do {
            return try JSONDecoder().decode(ManageLogDirectory.self, from: data)
        } catch {
            throw APIClientError.invalidResponse
        }
    }

    private func fetchLifecycle(
        baseURL: URL,
        bearerToken: String
    ) async throws -> ManageLifecycle {
        let (data, response) = try await request(
            baseURL: baseURL,
            path: "api/v1/manage/lifecycle",
            bearerToken: bearerToken
        )
        guard let httpResponse = response as? HTTPURLResponse else {
            throw APIClientError.invalidResponse
        }
        guard httpResponse.statusCode != 401 else { throw APIClientError.unauthorized }
        guard httpResponse.statusCode == 200 else { throw APIClientError.invalidResponse }
        do {
            let lifecycle = try JSONDecoder().decode(ManageLifecycle.self, from: data)
            guard lifecycle.service.service == "threadrelay" else {
                throw APIClientError.incompatibleService
            }
            guard lifecycle.service.apiMajor == 1 else {
                throw APIClientError.unsupportedAPIMajor(lifecycle.service.apiMajor)
            }
            guard lifecycle.runtime.apiMajor == 1 else {
                throw APIClientError.unsupportedAPIMajor(lifecycle.runtime.apiMajor)
            }
            return lifecycle
        } catch let error as APIClientError {
            throw error
        } catch {
            throw APIClientError.invalidResponse
        }
    }

    private func fetchIMAccounts(
        baseURL: URL,
        bearerToken: String
    ) async throws -> ManageIMAccountsResponse {
        let (data, response) = try await request(
            baseURL: baseURL,
            path: "api/v1/manage/im/accounts",
            bearerToken: bearerToken
        )
        guard let httpResponse = response as? HTTPURLResponse else {
            throw APIClientError.invalidResponse
        }
        guard httpResponse.statusCode != 401 else { throw APIClientError.unauthorized }
        guard httpResponse.statusCode != 404 else { throw APIClientError.featureUnavailable }
        guard httpResponse.statusCode == 200 else { throw APIClientError.invalidResponse }
        do {
            let accounts = try JSONDecoder().decode(ManageIMAccountsResponse.self, from: data)
            guard accounts.service.service == "threadrelay" else {
                throw APIClientError.incompatibleService
            }
            guard accounts.service.apiMajor == 1 else {
                throw APIClientError.unsupportedAPIMajor(accounts.service.apiMajor)
            }
            return accounts
        } catch let error as APIClientError {
            throw error
        } catch {
            throw APIClientError.invalidResponse
        }
    }

    private func performIMMutation<Body: Encodable, Response: ManageMutationResponse>(
        path: String,
        body: Body
    ) async throws -> Response {
        let result: Response = try await performManagePOST(path: path, body: body)
        guard result.ok else {
            throw APIClientError.operationFailed("后台服务未完成账号操作。")
        }
        return result
    }

    private func performManagePOST<Body: Encodable, Response: Decodable>(
        path: String,
        body: Body
    ) async throws -> Response {
        let connection = connectionLoader()
        let candidates = connection.credentials()
        guard !candidates.isEmpty else { throw APIClientError.unauthorized }
        let encodedBody = try JSONEncoder().encode(body)

        for candidate in candidates {
            do {
                let (data, response) = try await request(
                    baseURL: connection.baseURL,
                    path: path,
                    method: "POST",
                    body: encodedBody,
                    bearerToken: candidate.token
                )
                guard let httpResponse = response as? HTTPURLResponse else {
                    throw APIClientError.invalidResponse
                }
                guard httpResponse.statusCode != 401 else { throw APIClientError.unauthorized }
                if httpResponse.statusCode == 404 {
                    // A current daemon uses 404 for a missing account and
                    // includes a stable JSON error. An older daemon has no
                    // versioned route and normally returns an empty/plain 404.
                    // Keep those cases distinct so the UI can offer an update
                    // only when the feature itself is absent.
                    let payload = try? JSONDecoder().decode(ErrorPayload.self, from: data)
                    if payload?.error == "IM account not found" {
                        throw operationError(from: data, statusCode: httpResponse.statusCode)
                    }
                    throw APIClientError.featureUnavailable
                }
                guard (200...299).contains(httpResponse.statusCode) else {
                    throw operationError(from: data, statusCode: httpResponse.statusCode)
                }
                do {
                    return try JSONDecoder().decode(Response.self, from: data)
                } catch {
                    throw APIClientError.invalidResponse
                }
            } catch APIClientError.unauthorized {
                continue
            }
        }
        throw APIClientError.unauthorized
    }

    private struct ErrorPayload: Decodable {
        let error: String?
    }

    private func operationError(from data: Data, statusCode: Int) -> APIClientError {
        let raw = (try? JSONDecoder().decode(ErrorPayload.self, from: data))?.error?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let message: String
        switch raw {
        // Values matched here are stable API contract strings; see
        // IM_ACCOUNT_NOT_FOUND_ERROR in src/web/im_api.rs.
        case "IM account not found": message = "找不到该消息账号。"
        case "missing accountId": message = "缺少账号标识。"
        case "unknown IM platform": message = "不支持的消息平台。"
        case "missing botToken": message = "请先填写机器人 Token。"
        case "missing appId": message = "请先填写 App ID。"
        case "missing appSecret": message = "请先填写 App Secret。"
        case "missing_session", "invalid_session": message = "扫码会话已失效，请重新获取二维码。"
        case let raw? where statusCode < 500 && !raw.isEmpty:
            // Validation failures carry a specific, already-sanitized reason
            // (for example a Telegram token rejection); show it as-is.
            message = raw
        default: message = statusCode >= 500 ? "后台服务操作失败。" : "账号操作未完成。"
        }
        return .operationFailed(message)
    }

    private func request(
        baseURL: URL,
        path: String,
        method: String = "GET",
        body: Data? = nil,
        bearerToken: String
    ) async throws -> (Data, URLResponse) {
        let url = baseURL.appending(path: path)
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.cachePolicy = .reloadIgnoringLocalCacheData
        // Mutations persist config and may verify credentials upstream (for
        // example the daemon calls Telegram getMe with a 5-second budget), so
        // they get a larger timeout than cheap status reads.
        request.timeoutInterval = method == "GET" ? 3 : 10
        request.setValue("Bearer \(bearerToken)", forHTTPHeaderField: "Authorization")
        if let body {
            request.httpBody = body
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        return try await session.data(for: request)
    }

    private func legacyProbe(baseURL: URL) async throws -> ServiceProbe {
        let url = baseURL.appending(path: "api/status")
        var request = URLRequest(url: url)
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.timeoutInterval = 3
        let (data, response) = try await session.data(for: request)
        guard let httpResponse = response as? HTTPURLResponse,
              httpResponse.statusCode == 200
        else {
            throw APIClientError.invalidResponse
        }

        let status: LegacyStatusResponse
        do {
            status = try JSONDecoder().decode(LegacyStatusResponse.self, from: data)
        } catch {
            throw APIClientError.invalidResponse
        }
        guard status.service == "threadrelay" || status.service == "codexhub" else {
            throw APIClientError.incompatibleService
        }
        return .legacy
    }
}

private extension ManageLogDirectory {
    func validatedURL() throws -> URL {
        guard !instanceId.isEmpty else { throw APIClientError.invalidResponse }
        let url = URL(fileURLWithPath: directory, isDirectory: true)
        guard !directory.isEmpty, url.path == directory else {
            throw APIClientError.invalidResponse
        }
        return url
    }
}
