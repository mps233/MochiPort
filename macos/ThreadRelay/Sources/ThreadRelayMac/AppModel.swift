import Foundation

@MainActor
final class AppModel: ObservableObject {
    @Published var selection: AppSection? = .overview
    @Published private(set) var serviceStatus: ServiceStatus = .checking
    @Published private(set) var lastCheckedAt: Date?
    @Published private(set) var dashboard: ManageDashboard?
    @Published private(set) var lifecycle: ManageLifecycle?
    @Published private(set) var imAccounts: [ManageIMAccount] = []
    @Published private(set) var imAccountsAvailability: MessagingAccountsAvailability = .loading
    @Published private(set) var dashboardState: DashboardState = .loading
    @Published var accountOperationError: String?

    private let apiClient: APIClient
    private let daemonLauncher: any DaemonLaunching
    private let fixtureStatus: ServiceStatus?
    private var refreshInFlight = false
    private var launchAttempted = false
    private var refreshTask: Task<Void, Never>?
    private var autoRefreshStarted = false
    private var windowVisible = true

    init(
        apiClient: APIClient = APIClient(),
        daemonLauncher: any DaemonLaunching = DaemonLauncher(),
        fixtureStatus: ServiceStatus? = nil
    ) {
        self.apiClient = apiClient
        self.daemonLauncher = daemonLauncher
        self.fixtureStatus = fixtureStatus
    }

    deinit {
        refreshTask?.cancel()
    }

    func startAutoRefresh() {
        guard !autoRefreshStarted else { return }
        autoRefreshStarted = true
        restartAutoRefresh()
    }

    private func restartAutoRefresh() {
        refreshTask?.cancel()
        refreshTask = Task { [weak self] in
            while !Task.isCancelled {
                guard let self else { return }
                let delay = self.windowVisible ? 15 : 60
                do {
                    try await Task.sleep(for: .seconds(delay))
                } catch {
                    return
                }
                guard !Task.isCancelled else { return }
                await self.refresh()
            }
        }
    }

    func setWindowVisible(_ visible: Bool) {
        guard windowVisible != visible else { return }
        windowVisible = visible
        if autoRefreshStarted {
            restartAutoRefresh()
        }
        if visible {
            Task { await refresh() }
        }
    }

    func refresh() async {
        guard !refreshInFlight else { return }
        refreshInFlight = true
        defer { refreshInFlight = false }

        // Preview runs use deterministic state and must never contact or
        // change the user's daemon.
        if let fixtureStatus {
            serviceStatus = fixtureStatus
            dashboard = fixtureDashboard(for: fixtureStatus)
            lifecycle = fixtureLifecycle(for: fixtureStatus)
            imAccounts = fixtureIMAccounts(for: fixtureStatus)
            imAccountsAvailability = fixtureStatus == .available ? .available : .unavailable(fixtureStatus.detail)
            dashboardState = fixtureDashboardState(for: fixtureStatus)
            lastCheckedAt = Date()
            return
        }

        dashboardState = dashboard == nil ? .loading : .refreshing
        let probeResult = await probeOrStartDaemon()
        switch probeResult {
        case let .success(probe):
            await loadServiceStatus(probe: probe)
        case let .failure(error):
            serviceStatus = .unavailable(userFacingMessage(for: error))
            imAccountsAvailability = .unavailable(userFacingMessage(for: error))
            dashboardState = dashboard == nil ? .offline : .stale
        }
        lastCheckedAt = Date()
    }

    private func probeOrStartDaemon() async -> Result<ServiceProbe, Error> {
        do {
            return .success(try await apiClient.probe())
        } catch let error as APIClientError {
            return .failure(error)
        } catch let error as URLError where error.code == .cannotConnectToHost {
            guard !launchAttempted else { return .failure(error) }
            launchAttempted = true
            do {
                try await daemonLauncher.startIfNeeded()
                dashboardState = .starting
                serviceStatus = .checking
                await waitForDaemonReadiness()
                return .success(try await apiClient.probe())
            } catch let launchError {
                return .failure(launchError)
            }
        } catch {
            return .failure(error)
        }
    }

    private func waitForDaemonReadiness() async {
        for attempt in 0..<30 {
            if let probe = try? await apiClient.probe(),
               case let .versioned(health) = probe,
               health.ready {
                return
            }
            guard attempt < 29 else { return }
            try? await Task.sleep(for: .milliseconds(250))
        }
    }

    private func loadServiceStatus(probe: ServiceProbe) async {
        switch probe {
        case let .versioned(health):
            serviceStatus = health.ready ? .available : .unavailable("服务正在启动")
            guard health.ready else {
                dashboardState = dashboard == nil ? .starting : .stale
                return
            }
            do {
                dashboard = try await apiClient.dashboard()
                lifecycle = try? await apiClient.lifecycle()
                // Account details were added after the first versioned
                // dashboard. Keep the dashboard usable when an older daemon
                // is still running, but expose the missing capability instead
                // of presenting it as an empty account list.
                await loadIMAccounts()
                dashboardState = .loaded
            } catch let error as APIClientError {
                if error == .unauthorized {
                    dashboard = nil
                    lifecycle = nil
                    imAccounts = []
                    imAccountsAvailability = .unauthorized
                }
                dashboardState = dashboardState(for: error)
            } catch {
                dashboardState = dashboard == nil ? .unavailable : .stale
            }
        case .legacy:
            serviceStatus = .bridgeAvailable
            dashboard = nil
            lifecycle = nil
            imAccounts = []
            imAccountsAvailability = .needsUpdate
            dashboardState = .legacy
        }
    }

    private func loadIMAccounts() async {
        do {
            imAccounts = try await apiClient.imAccounts()
            imAccountsAvailability = .available
        } catch let error as APIClientError {
            switch error {
            case .featureUnavailable:
                imAccounts = []
                imAccountsAvailability = .needsUpdate
            case .unauthorized:
                imAccounts = []
                imAccountsAvailability = .unauthorized
            default:
                imAccountsAvailability = .unavailable(error.localizedDescription)
            }
        } catch {
            imAccountsAvailability = .unavailable(userFacingMessage(for: error))
        }
    }

    func logDirectory() async -> URL? {
        guard fixtureStatus == nil else { return nil }
        return try? await apiClient.logDirectory()
    }

    /// Returns whether the daemon acknowledged the change so the caller can
    /// roll back optimistic switch state on failure.
    @discardableResult
    func setIMAccountEnabled(_ account: ManageIMAccount, enabled: Bool) async -> Bool {
        guard fixtureStatus == nil else { return true }
        do {
            _ = try await apiClient.setIMAccountEnabled(
                platform: account.platform,
                accountId: account.accountId,
                enabled: enabled
            )
            accountOperationError = nil
            await refresh()
            return true
        } catch {
            updateIMAccountsAvailability(for: error)
            accountOperationError = userFacingMessage(for: error)
            return false
        }
    }

    @discardableResult
    func deleteIMAccount(_ account: ManageIMAccount) async -> Bool {
        guard fixtureStatus == nil else {
            imAccounts.removeAll {
                $0.platform == account.platform && $0.accountId == account.accountId
            }
            return true
        }
        do {
            _ = try await apiClient.deleteIMAccount(
                platform: account.platform,
                accountId: account.accountId
            )
            accountOperationError = nil
            await refresh()
            return true
        } catch {
            updateIMAccountsAvailability(for: error)
            accountOperationError = userFacingMessage(for: error)
            return false
        }
    }

    /// Verify and store a Telegram bot token through the daemon, then reload
    /// the account list. Throws so the onboarding sheet can surface the error
    /// next to the credential form instead of the page-level banner.
    func configureTelegramAccount(
        botToken: String,
        mentionOnly: Bool
    ) async throws -> ManageIMAccountConfigureResponse {
        guard fixtureStatus == nil else {
            let account = ManageIMAccount(
                platform: "telegram",
                accountId: "preview-telegram-new",
                displayName: "预览 Telegram 机器人",
                enabled: true,
                configured: true,
                secretSet: true,
                connecting: false,
                polling: true,
                connected: true,
                lastError: nil,
                lastEventAtMs: Int64(Date().timeIntervalSince1970 * 1_000),
                lastInboundAtMs: nil
            )
            if !imAccounts.contains(where: { $0.id == account.id }) {
                imAccounts.append(account)
            }
            return ManageIMAccountConfigureResponse(
                ok: true,
                platform: account.platform,
                accountId: account.accountId,
                displayName: account.displayName
            )
        }
        do {
            let response = try await apiClient.configureTelegramAccount(
                botToken: botToken,
                mentionOnly: mentionOnly
            )
            accountOperationError = nil
            await refresh()
            return response
        } catch {
            updateIMAccountsAvailability(for: error)
            throw error
        }
    }

    /// Verify and store manually entered Feishu app credentials through the
    /// daemon, then reload the account list. Throws so the onboarding sheet
    /// can surface the error next to the credential form.
    func configureFeishuAccount(
        appId: String,
        appSecret: String
    ) async throws -> ManageIMAccountConfigureResponse {
        guard fixtureStatus == nil else {
            let account = appendFixtureAccount(
                platform: "feishu",
                accountId: appId.isEmpty ? "preview-feishu-new" : appId,
                displayName: "预览飞书应用"
            )
            return ManageIMAccountConfigureResponse(
                ok: true,
                platform: account.platform,
                accountId: account.accountId,
                displayName: account.displayName
            )
        }
        do {
            let response = try await apiClient.configureFeishuAccount(
                appId: appId,
                appSecret: appSecret
            )
            accountOperationError = nil
            await refresh()
            return response
        } catch {
            updateIMAccountsAvailability(for: error)
            throw error
        }
    }

    func startFeishuOnboarding() async throws -> ManageFeishuOnboardingStart {
        guard fixtureStatus == nil else {
            return ManageFeishuOnboardingStart(
                verificationUri: "https://example.invalid/feishu",
                verificationUriComplete: "https://example.invalid/feishu?code=preview",
                deviceCode: "preview-device-code",
                expiresIn: 600,
                interval: 5,
                qrSvg: ""
            )
        }
        return try await forwardOnboardingErrors {
            try await self.apiClient.startFeishuOnboarding()
        }
    }

    func pollFeishuOnboarding(deviceCode: String) async throws -> ManageFeishuOnboardingPoll {
        guard fixtureStatus == nil else {
            let account = appendFixtureAccount(
                platform: "feishu",
                accountId: "preview-feishu-scan",
                displayName: "预览飞书应用"
            )
            return ManageFeishuOnboardingPoll(
                done: true,
                appId: account.accountId,
                displayName: account.displayName,
                error: nil,
                errorDescription: nil
            )
        }
        let result = try await forwardOnboardingErrors {
            try await self.apiClient.pollFeishuOnboarding(deviceCode: deviceCode)
        }
        if result.done {
            accountOperationError = nil
            await refresh()
        }
        return result
    }

    func startWechatOnboarding() async throws -> ManageWechatOnboardingStart {
        guard fixtureStatus == nil else {
            return ManageWechatOnboardingStart(
                sessionKey: "preview-wechat-session",
                qrcodeUrl: "https://example.invalid/wechat-qr",
                qrSvg: "",
                expiresIn: 300
            )
        }
        return try await forwardOnboardingErrors {
            try await self.apiClient.startWechatOnboarding()
        }
    }

    func pollWechatOnboarding(
        sessionKey: String,
        verifyCode: String?
    ) async throws -> ManageWechatOnboardingPoll {
        guard fixtureStatus == nil else {
            let account = appendFixtureAccount(
                platform: "wechat",
                accountId: "preview-wechat-scan",
                displayName: "预览微信机器人"
            )
            return ManageWechatOnboardingPoll(
                done: true,
                status: "confirmed",
                needVerifyCode: false,
                accountId: account.accountId,
                alreadyConnected: false,
                error: nil
            )
        }
        let result = try await forwardOnboardingErrors {
            try await self.apiClient.pollWechatOnboarding(
                sessionKey: sessionKey,
                verifyCode: verifyCode
            )
        }
        if result.done {
            accountOperationError = nil
            await refresh()
        }
        return result
    }

    func startWecomOnboarding() async throws -> ManageWecomOnboardingStart {
        guard fixtureStatus == nil else {
            return ManageWecomOnboardingStart(
                sessionKey: "preview-wecom-session",
                qrcodeUrl: "https://example.invalid/wecom-qr",
                qrSvg: "",
                expiresIn: 300,
                interval: 3
            )
        }
        return try await forwardOnboardingErrors {
            try await self.apiClient.startWecomOnboarding()
        }
    }

    func pollWecomOnboarding(sessionKey: String) async throws -> ManageWecomOnboardingPoll {
        guard fixtureStatus == nil else {
            let account = appendFixtureAccount(
                platform: "wecom",
                accountId: "preview-wecom-scan",
                displayName: "预览企业微信机器人"
            )
            return ManageWecomOnboardingPoll(
                done: true,
                status: "success",
                accountId: account.accountId,
                error: nil
            )
        }
        let result = try await forwardOnboardingErrors {
            try await self.apiClient.pollWecomOnboarding(sessionKey: sessionKey)
        }
        if result.done {
            accountOperationError = nil
            await refresh()
        }
        return result
    }

    private func forwardOnboardingErrors<T>(
        _ operation: () async throws -> T
    ) async throws -> T {
        do {
            return try await operation()
        } catch {
            updateIMAccountsAvailability(for: error)
            throw error
        }
    }

    @discardableResult
    private func appendFixtureAccount(
        platform: String,
        accountId: String,
        displayName: String
    ) -> ManageIMAccount {
        let account = ManageIMAccount(
            platform: platform,
            accountId: accountId,
            displayName: displayName,
            enabled: true,
            configured: true,
            secretSet: true,
            connecting: false,
            polling: true,
            connected: true,
            lastError: nil,
            lastEventAtMs: Int64(Date().timeIntervalSince1970 * 1_000),
            lastInboundAtMs: nil
        )
        if !imAccounts.contains(where: { $0.id == account.id }) {
            imAccounts.append(account)
        }
        return account
    }

    private func updateIMAccountsAvailability(for error: Error) {
        // A failed mutation only proves that one write failed; the account
        // list itself is still readable. Downgrade the list only when the
        // daemon lacks the feature or rejected the credential.
        guard let error = error as? APIClientError else { return }
        switch error {
        case .featureUnavailable:
            imAccountsAvailability = .needsUpdate
        case .unauthorized:
            imAccountsAvailability = .unauthorized
        default:
            break
        }
    }

    private func fixtureDashboard(for status: ServiceStatus) -> ManageDashboard? {
        guard status == .available else { return nil }
        return ManageDashboard(
            service: ManageDashboard.Service(
                service: "threadrelay",
                apiMajor: 1,
                ready: true,
                instanceId: "preview-instance",
                pid: 0,
                startedAtMs: 0
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
                telegram: .init(accountCount: 1, connectedAccountCount: 1),
                feishu: .init(accountCount: 1, connectedAccountCount: 1),
                wechat: .init(accountCount: 1, connectedAccountCount: 1),
                wecom: .init(accountCount: 1, connectedAccountCount: 0)
            ),
            aiGatewayEnabled: true,
            aiGatewayProviderCount: 2,
            requestLoggingEnabled: true
        )
    }

    private func fixtureLifecycle(for status: ServiceStatus) -> ManageLifecycle? {
        guard status == .available else { return nil }
        return ManageLifecycle(
            service: .init(
                service: "threadrelay",
                apiMajor: 1,
                ready: true,
                instanceId: "preview-instance",
                pid: 0,
                startedAtMs: 0
            ),
            executable: "/Preview/ThreadRelay",
            configPath: "/Preview/config.toml",
            bind: "127.0.0.1:3847",
            runtime: .init(state: "active", productVersion: "0.5.0", apiMajor: 1),
            protectedWorkItems: .init(
                aiGatewayRequests: 0,
                codexTurns: 0,
                imStreams: 0,
                pendingApprovals: 0,
                remoteControlRequests: 0,
                total: 0
            ),
            management: .init(
                state: "unmanaged",
                mode: "readOnly",
                canControl: false,
                installationId: nil,
                leaseGeneration: nil,
                leaseExpiresAtMs: nil
            )
        )
    }

    private func fixtureIMAccounts(for status: ServiceStatus) -> [ManageIMAccount] {
        guard status == .available else { return [] }
        let now = Int64(Date().timeIntervalSince1970 * 1_000)
        return [
            ManageIMAccount(
                platform: "telegram",
                accountId: "preview-telegram",
                displayName: "Telegram 机器人",
                enabled: true,
                configured: true,
                secretSet: true,
                connecting: false,
                polling: true,
                connected: true,
                lastError: nil,
                lastEventAtMs: now,
                lastInboundAtMs: now - 20_000
            ),
            ManageIMAccount(
                platform: "feishu",
                accountId: "preview-feishu",
                displayName: "工作空间",
                enabled: true,
                configured: true,
                secretSet: true,
                connecting: false,
                polling: false,
                connected: true,
                lastError: nil,
                lastEventAtMs: now - 60_000,
                lastInboundAtMs: now - 80_000
            ),
            ManageIMAccount(
                platform: "wechat",
                accountId: "preview-wechat",
                displayName: "客服微信",
                enabled: true,
                configured: true,
                secretSet: true,
                connecting: true,
                polling: true,
                connected: false,
                lastError: nil,
                lastEventAtMs: nil,
                lastInboundAtMs: nil
            ),
            ManageIMAccount(
                platform: "wecom",
                accountId: "preview-wecom",
                displayName: "企业微信",
                enabled: false,
                configured: false,
                secretSet: false,
                connecting: false,
                polling: false,
                connected: false,
                lastError: "尚未完成凭据配置",
                lastEventAtMs: nil,
                lastInboundAtMs: nil
            ),
        ]
    }

    private func fixtureDashboardState(for status: ServiceStatus) -> DashboardState {
        switch status {
        case .available: .loaded
        case .bridgeAvailable: .legacy
        case .unavailable: .offline
        case .checking: .loading
        }
    }

    private func dashboardState(for error: APIClientError) -> DashboardState {
        switch error {
        case .unauthorized: .unauthorized
        default: dashboard == nil ? .unavailable : .stale
        }
    }

    private func userFacingMessage(for error: Error) -> String {
        if let apiError = error as? APIClientError {
            return apiError.localizedDescription
        }
        return "本地服务不可用。"
    }
}

enum DashboardState: Equatable {
    case loading
    case refreshing
    case starting
    case loaded
    case legacy
    case unauthorized
    case unavailable
    case offline
    case stale

    var isRefreshing: Bool {
        switch self {
        case .loading, .refreshing: true
        default: false
        }
    }

    var title: String {
        switch self {
        case .loading: "正在加载"
        case .refreshing: "正在刷新"
        case .starting: "正在启动"
        case .loaded: "已加载"
        case .legacy: "兼容模式"
        case .unauthorized: "需要授权"
        case .unavailable: "不可用"
        case .offline: "离线"
        case .stale: "上次状态"
        }
    }
}

enum MessagingAccountsAvailability: Equatable {
    case loading
    case available
    case needsUpdate
    case unauthorized
    case unavailable(String)
}

enum ServiceStatus: Equatable {
    case checking
    case available
    case bridgeAvailable
    case unavailable(String)

    var title: String {
        switch self {
        case .checking: "检查中"
        case .available: "运行正常"
        case .bridgeAvailable: "兼容服务"
        case .unavailable: "不可用"
        }
    }

    var detail: String {
        switch self {
        case .checking: "正在连接本地服务"
        case .available: "本地服务已就绪"
        case .bridgeAvailable: "请更新服务以启用版本化管理 API"
        case let .unavailable(message): message
        }
    }

    var symbol: String {
        switch self {
        case .checking: "arrow.trianglehead.2.clockwise.rotate.90"
        case .available: "checkmark.circle.fill"
        case .bridgeAvailable: "arrow.triangle.2.circlepath.circle.fill"
        case .unavailable: "exclamationmark.triangle.fill"
        }
    }

    var tint: StatusTint {
        switch self {
        case .checking: .secondary
        case .available: .positive
        case .bridgeAvailable: .caution
        case .unavailable: .negative
        }
    }
}

enum StatusTint {
    case secondary
    case positive
    case caution
    case negative
}

enum AppSection: String, CaseIterable, Identifiable {
    case overview
    case codex
    case sessions
    case messaging
    case gateway
    case requestLogs

    var id: String { rawValue }

    var title: String {
        switch self {
        case .overview: "概览"
        case .codex: "Codex 接入"
        case .sessions: "会话"
        case .messaging: "消息渠道"
        case .gateway: "AI 网关"
        case .requestLogs: "请求日志"
        }
    }

    var symbol: String {
        switch self {
        case .overview: "rectangle.grid.1x2"
        case .codex: "chevron.left.forwardslash.chevron.right"
        case .sessions: "clock.arrow.circlepath"
        case .messaging: "bubble.left.and.bubble.right"
        case .gateway: "point.3.connected.trianglepath.dotted"
        case .requestLogs: "list.bullet.rectangle"
        }
    }

    var group: AppSectionGroup {
        switch self {
        case .overview: .overview
        case .codex, .sessions: .workspace
        case .messaging, .gateway, .requestLogs: .connections
        }
    }
}

enum AppSectionGroup: String, CaseIterable, Identifiable {
    case overview
    case workspace
    case connections

    var id: String { rawValue }

    var title: String? {
        switch self {
        case .overview: nil
        case .workspace: "工作区"
        case .connections: "连接"
        }
    }

    var sections: [AppSection] {
        AppSection.allCases.filter { $0.group == self }
    }
}
