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

    var errorDescription: String? {
        switch self {
        case .invalidResponse: "The local service returned an invalid response."
        case .incompatibleService: "Another service is using the ThreadRelay port."
        case let .unsupportedAPIMajor(apiMajor):
            "This ThreadRelay service uses unsupported management API version \(apiMajor)."
        case .unauthorized: "The local service rejected the management credential."
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

    private func request(
        baseURL: URL,
        path: String,
        bearerToken: String
    ) async throws -> (Data, URLResponse) {
        let url = baseURL.appending(path: path)
        var request = URLRequest(url: url)
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.timeoutInterval = 3
        request.setValue("Bearer \(bearerToken)", forHTTPHeaderField: "Authorization")
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
