import Foundation

@MainActor
final class AppModel: ObservableObject {
    @Published var selection: AppSection? = .overview
    @Published private(set) var serviceStatus: ServiceStatus = .checking
    @Published private(set) var lastCheckedAt: Date?
    @Published private(set) var dashboard: ManageDashboard?
    @Published private(set) var lifecycle: ManageLifecycle?
    @Published private(set) var dashboardState: DashboardState = .loading

    private let apiClient: APIClient
    private let fixtureStatus: ServiceStatus?
    private var refreshInFlight = false
    private var refreshTask: Task<Void, Never>?
    private var autoRefreshStarted = false
    private var windowVisible = true

    init(apiClient: APIClient = APIClient(), fixtureStatus: ServiceStatus? = nil) {
        self.apiClient = apiClient
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
            dashboardState = fixtureDashboardState(for: fixtureStatus)
            lastCheckedAt = Date()
            return
        }

        dashboardState = dashboard == nil ? .loading : .refreshing
        do {
            let probe = try await apiClient.probe()
            switch probe {
            case let .versioned(health):
                serviceStatus = health.ready ? .available : .unavailable("Service is starting")
                guard health.ready else {
                    dashboardState = dashboard == nil ? .starting : .stale
                    break
                }
                do {
                    dashboard = try await apiClient.dashboard()
                    lifecycle = try? await apiClient.lifecycle()
                    dashboardState = .loaded
                } catch let error as APIClientError {
                    if error == .unauthorized {
                        dashboard = nil
                        lifecycle = nil
                    }
                    dashboardState = dashboardState(for: error)
                } catch {
                    dashboardState = dashboard == nil ? .unavailable : .stale
                }
            case .legacy:
                serviceStatus = .bridgeAvailable
                dashboard = nil
                lifecycle = nil
                dashboardState = .legacy
            }
        } catch {
            serviceStatus = .unavailable(userFacingMessage(for: error))
            dashboardState = dashboard == nil ? .offline : .stale
        }
        lastCheckedAt = Date()
    }

    func logDirectory() async -> URL? {
        guard fixtureStatus == nil else { return nil }
        return try? await apiClient.logDirectory()
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
        return "The local service is unavailable."
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
}

enum ServiceStatus: Equatable {
    case checking
    case available
    case bridgeAvailable
    case unavailable(String)

    var title: String {
        switch self {
        case .checking: "Checking"
        case .available: "Available"
        case .bridgeAvailable: "Compatible Service"
        case .unavailable: "Unavailable"
        }
    }

    var detail: String {
        switch self {
        case .checking: "Connecting to the local service"
        case .available: "The local service is ready"
        case .bridgeAvailable: "Update the service to enable the versioned management API"
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
        case .overview: "Overview"
        case .codex: "Codex"
        case .sessions: "Sessions"
        case .messaging: "Messaging Channels"
        case .gateway: "AI Gateway"
        case .requestLogs: "Request Logs"
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
        case .workspace: "Workspace"
        case .connections: "Connections"
        }
    }

    var sections: [AppSection] {
        AppSection.allCases.filter { $0.group == self }
    }
}
