import Foundation

/// A transient success message shown in the shared feedback capsule. The
/// identifier restarts the auto-dismiss timer when a new message arrives.
struct ActionFeedback: Equatable, Identifiable {
    let id = UUID()
    let message: String
}

/// Result of a batch session move; failed sessions stay selected in the UI.
struct SessionBatchMoveResult: Equatable {
    let movedIds: [String]
    let failedIds: [String]
}

@MainActor
final class AppModel: ObservableObject {
    @Published var selection: AppSection? = .overview
    @Published private(set) var serviceStatus: ServiceStatus = .checking
    @Published private(set) var lastCheckedAt: Date?
    @Published private(set) var dashboard: ManageDashboard?
    @Published private(set) var lifecycle: ManageLifecycle?
    @Published private(set) var imAccounts: [ManageIMAccount] = []
    @Published private(set) var imAccountsAvailability: MessagingAccountsAvailability = .loading
    @Published private(set) var codexStatus: ManageCodexStatus?
    @Published private(set) var codexPreflight: ManageCodexPreflightResponse?
    @Published private(set) var codexSessions: [ManageCodexSession] = []
    @Published private(set) var codexSessionProviders: [String] = []
    @Published private(set) var gateway: ManageGateway?
    @Published private(set) var sub2ApiAdmin: ManageSub2ApiAdmin?
    @Published private(set) var sub2ApiAccountPool: ManageSub2ApiAccountPoolResponse.Pool?
    @Published private(set) var sub2ApiAccountPoolLoading = false
    @Published private(set) var sub2ApiAccountPoolError: String?
    @Published private(set) var settings: ManageSettings?
    @Published private(set) var requestLogs: [ManageRequestLog] = []
    @Published private(set) var requestLogFilters = RequestLogFilters()
    @Published private(set) var requestLogHasMore = false
    @Published private(set) var requestLogLoadingMore = false
    @Published private(set) var requestLogDetail: ManageRequestLogDetail?
    @Published private(set) var loadingSections: Set<AppSection> = []
    @Published private(set) var sectionErrors: [AppSection: String] = [:]
    @Published private(set) var dashboardState: DashboardState = .loading
    @Published var accountOperationError: String?
    @Published var managementOperationError: String?
    @Published var actionFeedback: ActionFeedback?
    @Published private(set) var daemonRecoveryInProgress = false
    @Published var daemonRecoveryError: String?
    @Published private(set) var availableUpdate: AvailableUpdate?
    @Published var updateNoticeDismissed = false

    private let apiClient: APIClient
    private let daemonLauncher: any DaemonLaunching
    private let fixtureStatus: ServiceStatus?
    private var refreshInFlight = false
    private var launchAttempted = false
    private var daemonRuntimePreparedBuild: String?
    private var refreshTask: Task<Void, Never>?
    private var autoRefreshStarted = false
    private var windowVisible = true
    private var sectionLoadGenerations: [AppSection: Int] = [:]
    private var sectionActivityCounts: [AppSection: Int] = [:]
    private var requestLogDataGeneration = 0
    private var requestLogNextCursor: String?
    private var requestLogLoadedFilters: RequestLogFilters?
    private var requestLogLoadedPageCount = 0
    private var requestLogLoadMoreOperationID: UUID?
    private var daemonHealthFailureCount = 0
    private var daemonAutoRestartNotBefore: Date?
    private var startupUpdateCheckScheduled = false
    private var lifecycleLeaseTask: Task<Void, Never>?
    private let installationId: String
    private static let sub2ApiAccountPoolCacheLifetime: TimeInterval = 5 * 60
    private var sub2ApiAccountPoolGeneration: UInt64 = 0
    private var sub2ApiAccountPoolRefreshID: UUID?

    /// Mirrors the legacy GUI's unhealthy-daemon recovery policy: three
    /// consecutive probe failures trigger a restart, then a cooldown blocks
    /// the next automatic attempt.
    nonisolated static let daemonAutoRestartFailureThreshold = 3
    nonisolated static let daemonAutoRestartCooldown: TimeInterval = 60

    nonisolated static func daemonAutoRestartReady(
        failures: Int,
        now: Date,
        notBefore: Date?
    ) -> Bool {
        guard failures >= daemonAutoRestartFailureThreshold else { return false }
        guard let notBefore else { return true }
        return now >= notBefore
    }

    nonisolated static func daemonFailureAllowsAutoRestart(_ error: Error) -> Bool {
        guard let urlError = error as? URLError else { return false }
        return urlError.code == .cannotConnectToHost || urlError.code == .networkConnectionLost
    }

    private static let installationIDDefaultsKey = "threadrelay.management.installation-id"

    private static func loadInstallationID() -> String {
        if let existing = UserDefaults.standard.string(forKey: installationIDDefaultsKey),
           !existing.isEmpty
        {
            return existing
        }
        let generated = UUID().uuidString.lowercased()
        UserDefaults.standard.set(generated, forKey: installationIDDefaultsKey)
        return generated
    }

    /// Returns true only when both sides expose a valid build number and they
    /// differ. An older daemon without `runtime.buildNumber` is unknown, not
    /// a mismatch, so it remains usable while the backend is upgraded.
    nonisolated static func buildNumbersMismatch(
        guiBuild: String?,
        daemonBuild: Int?
    ) -> Bool {
        guard let guiBuild,
              let guiBuildNumber = Int(guiBuild),
              let daemonBuild
        else { return false }
        return guiBuildNumber != daemonBuild
    }

    /// A newer GUI may prepare its helper without replacing a daemon that is
    /// still serving work. Downgrades are never automatic.
    nonisolated static func daemonRequiresUpgrade(
        guiBuild: String?,
        daemonBuild: Int?
    ) -> Bool {
        guard let guiBuild,
              let guiBuildNumber = Int(guiBuild),
              let daemonBuild
        else { return false }
        return guiBuildNumber > daemonBuild
    }

    var daemonBuildMismatch: Bool {
        guard let daemonBuild = lifecycle?.runtime.buildNumber else { return false }
        let guiBuild = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String
        return Self.buildNumbersMismatch(guiBuild: guiBuild, daemonBuild: daemonBuild)
    }

    var daemonUpgradePending: Bool {
        guard let lifecycle,
              let guiBuild = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String
        else { return false }
        return Self.daemonRequiresUpgrade(
            guiBuild: guiBuild,
            daemonBuild: lifecycle.runtime.buildNumber
        )
    }

    var daemonUpgradePrepared: Bool {
        guard daemonUpgradePending,
              let guiBuild = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String
        else { return false }
        return daemonRuntimePreparedBuild == guiBuild
    }

    var daemonUpgradeDetail: String {
        Self.daemonUpgradeDetailText(
            guiBuild: Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String,
            daemonBuild: lifecycle?.runtime.buildNumber,
            prepared: daemonUpgradePrepared
        )
    }

    nonisolated static func daemonUpgradeDetailText(
        guiBuild: String?,
        daemonBuild: Int?,
        prepared: Bool
    ) -> String {
        guard let guiBuild,
              let guiBuildNumber = Int(guiBuild),
              let daemonBuild
        else { return "后台版本未知" }
        if daemonBuild > guiBuildNumber {
            return "后台构建 \(daemonBuild) 高于界面 \(guiBuildNumber)，不会自动降级"
        }
        if daemonBuild == guiBuildNumber {
            return "版本一致"
        }
        if prepared {
            return "已准备构建 \(guiBuildNumber)，安全重启后生效"
        }
        return "发现构建 \(guiBuildNumber)，正在准备运行版本"
    }

    var ownsDaemonLease: Bool {
        guard let management = lifecycle?.management,
              management.canControl,
              management.installationId == installationId
        else { return false }
        guard let expiresAt = management.leaseExpiresAtMs else { return true }
        return expiresAt > currentTimeMilliseconds
    }

    var daemonLeaseConflict: Bool {
        guard let management = lifecycle?.management,
              let owner = management.installationId,
              owner != installationId,
              let expiresAt = management.leaseExpiresAtMs
        else { return false }
        return expiresAt > currentTimeMilliseconds
    }

    var daemonManagementDetail: String {
        if ownsDaemonLease { return "已托管 · 运行中" }
        if daemonLeaseConflict { return "运行正常 · 其他安装管理" }
        return "运行正常 · 仅查看"
    }

    private var currentTimeMilliseconds: Int64 {
        Int64(Date().timeIntervalSince1970 * 1_000)
    }

    init(
        apiClient: APIClient = APIClient(),
        daemonLauncher: any DaemonLaunching = DaemonLauncher(),
        fixtureStatus: ServiceStatus? = nil
    ) {
        self.apiClient = apiClient
        self.daemonLauncher = daemonLauncher
        self.fixtureStatus = fixtureStatus
        self.installationId = Self.loadInstallationID()
    }

    deinit {
        refreshTask?.cancel()
        lifecycleLeaseTask?.cancel()
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

    /// Read by page-local refresh loops (for example the request-log list)
    /// so they can pause while the window is hidden.
    var isWindowVisible: Bool { windowVisible }

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
            daemonHealthFailureCount = 0
            await loadServiceStatus(probe: probe)
        case let .failure(error):
            serviceStatus = .unavailable(userFacingMessage(for: error))
            imAccountsAvailability = .unavailable(userFacingMessage(for: error))
            dashboardState = dashboard == nil ? .offline : .stale
            registerDaemonProbeFailure(error)
        }
        lastCheckedAt = Date()
    }

    /// Counts consecutive loopback connection losses and automatically tries
    /// recovery after three of them, with a 60-second cooldown. HTTP errors,
    /// incompatible services, and unsupported API versions stay diagnostic
    /// only so recovery cannot start a crash loop against an occupied port.
    private func registerDaemonProbeFailure(_ error: Error) {
        guard Self.daemonFailureAllowsAutoRestart(error) else {
            daemonHealthFailureCount = 0
            return
        }
        daemonHealthFailureCount += 1
        guard fixtureStatus == nil,
              !daemonRecoveryInProgress,
              Self.daemonAutoRestartReady(
                  failures: daemonHealthFailureCount,
                  now: Date(),
                  notBefore: daemonAutoRestartNotBefore
              )
        else { return }
        daemonAutoRestartNotBefore = Date().addingTimeInterval(Self.daemonAutoRestartCooldown)
        daemonHealthFailureCount = 0
        // Spawned instead of awaited: the recovery path re-runs refresh()
        // once the daemon answers, which must not nest inside this refresh.
        Task { [weak self] in
            await self?.startDaemonManually()
        }
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
                if let lifecycle {
                    await prepareDaemonRuntimeIfNeeded(lifecycle)
                }
                await reconcileLifecycleLease()
                settings = try? await apiClient.settings()
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
                    clearManagementPages()
                }
                dashboardState = dashboardState(for: error)
            } catch {
                dashboardState = dashboard == nil ? .unavailable : .stale
            }
        case .legacy:
            serviceStatus = .bridgeAvailable
            dashboard = nil
            lifecycle = nil
            stopLifecycleLeaseHeartbeat()
            imAccounts = []
            imAccountsAvailability = .needsUpdate
            clearManagementPages()
            dashboardState = .legacy
        }
    }

    private func clearManagementPages() {
        stopLifecycleLeaseHeartbeat()
        for section in AppSection.allCases {
            sectionLoadGenerations[section, default: 0] += 1
        }
        codexStatus = nil
        codexPreflight = nil
        codexSessions = []
        codexSessionProviders = []
        gateway = nil
        sub2ApiAdmin = nil
        invalidateSub2ApiAccountPool()
        settings = nil
        resetRequestLogPagination(clearLogs: true)
        requestLogDetail = nil
        sectionErrors = [:]
    }

    private func prepareDaemonRuntimeIfNeeded(_ lifecycle: ManageLifecycle) async {
        guard Self.daemonRequiresUpgrade(
            guiBuild: Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String,
            daemonBuild: lifecycle.runtime.buildNumber
        ),
        let guiBuild = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String,
        daemonRuntimePreparedBuild != guiBuild
        else { return }

        do {
            try await daemonLauncher.prepareRuntime()
            daemonRuntimePreparedBuild = guiBuild
            daemonRecoveryError = nil
        } catch let error as DaemonLaunchError {
            daemonRecoveryError = error.localizedDescription
        } catch {
            daemonRecoveryError = userFacingMessage(for: error)
        }
    }

    private func clearSectionData(_ section: AppSection) {
        switch section {
        case .overview:
            dashboard = nil
            lifecycle = nil
        case .codex:
            codexStatus = nil
            codexPreflight = nil
        case .sessions:
            codexSessions = []
            codexSessionProviders = []
        case .messaging:
            imAccounts = []
        case .gateway:
            gateway = nil
            sub2ApiAdmin = nil
            invalidateSub2ApiAccountPool()
        case .requestLogs:
            resetRequestLogPagination(clearLogs: true)
            requestLogDetail = nil
        }
    }

    private func reconcileLifecycleLease() async {
        guard fixtureStatus == nil, let lifecycle else {
            stopLifecycleLeaseHeartbeat()
            return
        }
        if ownsDaemonLease {
            startLifecycleLeaseHeartbeat()
            return
        }
        if daemonLeaseConflict {
            stopLifecycleLeaseHeartbeat()
            return
        }
        do {
            let claimed = try await apiClient.claimLifecycleLease(
                installationId: installationId,
                daemonInstanceId: lifecycle.service.instanceId
            )
            self.lifecycle = claimed
            startLifecycleLeaseHeartbeat()
        } catch {
            // Claiming is opportunistic. A second installation may own the
            // lease, or an older daemon may not expose the endpoint yet.
            stopLifecycleLeaseHeartbeat()
        }
    }

    private func startLifecycleLeaseHeartbeat() {
        guard lifecycleLeaseTask == nil else { return }
        lifecycleLeaseTask = Task { [weak self] in
            while !Task.isCancelled {
                do {
                    try await Task.sleep(for: .seconds(10))
                } catch {
                    return
                }
                guard let self, !Task.isCancelled else { return }
                await self.renewLifecycleLease()
            }
        }
    }

    private func stopLifecycleLeaseHeartbeat() {
        lifecycleLeaseTask?.cancel()
        lifecycleLeaseTask = nil
    }

    private func renewLifecycleLease() async {
        guard ownsDaemonLease, let instanceId = lifecycle?.service.instanceId else {
            stopLifecycleLeaseHeartbeat()
            return
        }
        do {
            lifecycle = try await apiClient.renewLifecycleLease(
                installationId: installationId,
                daemonInstanceId: instanceId
            )
        } catch {
            stopLifecycleLeaseHeartbeat()
            lifecycle = try? await apiClient.lifecycle()
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

    @discardableResult
    func loadSection(_ section: AppSection, force: Bool = false) async -> Bool {
        guard fixtureStatus == nil else { return true }
        guard force || sectionActivityCounts[section, default: 0] == 0 else { return false }
        let generation = sectionLoadGenerations[section, default: 0] + 1
        sectionLoadGenerations[section] = generation
        beginSectionActivity(section)
        sectionErrors[section] = nil
        defer { endSectionActivity(section) }

        do {
            switch section {
            case .overview:
                await refresh()
            case .codex:
                let status = try await apiClient.codexStatus()
                let preflight = try? await apiClient.codexEnhancedPreflight()
                guard isCurrentLoad(section, generation: generation) else { return false }
                codexStatus = status
                codexPreflight = preflight
            case .sessions:
                let response = try await apiClient.codexSessions()
                guard isCurrentLoad(section, generation: generation) else { return false }
                codexSessions = response.threads
                var providers = response.providers + response.threads.map(\.modelProvider)
                providers.append("openai")
                codexSessionProviders = Array(
                    Set(
                        providers
                            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
                            .filter { !$0.isEmpty }
                    )
                )
                .sorted()
            case .messaging:
                await loadIMAccounts()
            case .gateway:
                async let gatewayResponse = apiClient.gateway()
                async let sub2ApiResponse = try? apiClient.sub2ApiAdmin()
                let response = try await gatewayResponse
                let admin = await sub2ApiResponse
                guard isCurrentLoad(section, generation: generation) else { return false }
                gateway = response
                sub2ApiAdmin = admin
            case .requestLogs:
                let filters = requestLogFilters
                let dataGeneration = beginRequestLogFirstPageLoad()
                let previousLogs = requestLogs
                let previousCursor = requestLogNextCursor
                let previousHasMore = requestLogHasMore
                let previousPageCount = requestLogLoadedPageCount
                let preserveLoadedTail = requestLogLoadedFilters == filters
                    && previousPageCount > 0
                let response = Self.applyingLegacyRequestLogFilters(
                    to: try await apiClient.requestLogs(filters: filters),
                    filters: filters
                )
                guard isCurrentLoad(section, generation: generation),
                      requestLogDataGeneration == dataGeneration,
                      requestLogFilters == filters
                else { return false }
                applyRequestLogFirstPage(
                    response,
                    filters: filters,
                    previousLogs: previousLogs,
                    previousCursor: previousCursor,
                    previousHasMore: previousHasMore,
                    previousPageCount: previousPageCount,
                    preserveLoadedTail: preserveLoadedTail
                )
            }
            guard isCurrentLoad(section, generation: generation) else { return false }
            return true
        } catch {
            guard isCurrentLoad(section, generation: generation) else { return false }
            if let apiError = error as? APIClientError,
               apiError == .featureUnavailable || apiError == .unauthorized {
                clearSectionData(section)
            }
            sectionErrors[section] = userFacingMessage(for: error)
            return false
        }
    }

    func isLoading(_ section: AppSection) -> Bool {
        loadingSections.contains(section)
    }

    /// Replaces the active server-side filters and loads their first page.
    /// Any response still in flight for an older filter set is ignored.
    @discardableResult
    func setRequestLogFilters(_ filters: RequestLogFilters) async -> Bool {
        requestLogFilters = filters
        resetRequestLogPagination(clearLogs: true)
        return await loadSection(.requestLogs, force: true)
    }

    @discardableResult
    func resetRequestLogFilters() async -> Bool {
        await setRequestLogFilters(RequestLogFilters())
    }

    @discardableResult
    func refreshRequestLogs() async -> Bool {
        await loadSection(.requestLogs, force: true)
    }

    /// Loads the next keyset page and appends only IDs that are not already
    /// present. A first-page refresh or filter change invalidates this merge.
    @discardableResult
    func loadMoreRequestLogs() async -> Bool {
        guard fixtureStatus == nil else { return true }
        guard !requestLogLoadingMore,
              !isLoading(.requestLogs),
              requestLogHasMore,
              let cursor = requestLogNextCursor,
              requestLogLoadedFilters == requestLogFilters
        else { return false }

        let filters = requestLogFilters
        let dataGeneration = requestLogDataGeneration
        let operationID = UUID()
        requestLogLoadMoreOperationID = operationID
        requestLogLoadingMore = true
        defer {
            if requestLogLoadMoreOperationID == operationID {
                requestLogLoadMoreOperationID = nil
                requestLogLoadingMore = false
            }
        }

        do {
            let response = try await apiClient.requestLogs(
                filters: filters,
                cursor: cursor
            )
            guard !Task.isCancelled,
                  requestLogLoadMoreOperationID == operationID,
                  requestLogDataGeneration == dataGeneration,
                  requestLogFilters == filters
            else { return false }

            requestLogs = Self.mergingRequestLogs(
                requestLogs,
                after: response.logs
            )
            let nextCursor = response.nextCursor
            requestLogNextCursor = nextCursor
            requestLogHasMore = Self.requestLogPageHasMore(
                response,
                previousCursor: cursor
            )
            requestLogLoadedPageCount += 1
            sectionErrors[.requestLogs] = nil
            return true
        } catch {
            guard !Task.isCancelled,
                  requestLogLoadMoreOperationID == operationID,
                  requestLogDataGeneration == dataGeneration,
                  requestLogFilters == filters
            else { return false }
            sectionErrors[.requestLogs] = userFacingMessage(for: error)
            return false
        }
    }

    private func beginRequestLogFirstPageLoad() -> Int {
        requestLogDataGeneration += 1
        requestLogLoadMoreOperationID = nil
        requestLogLoadingMore = false
        return requestLogDataGeneration
    }

    private func applyRequestLogFirstPage(
        _ response: ManageRequestLogsResponse,
        filters: RequestLogFilters,
        previousLogs: [ManageRequestLog],
        previousCursor: String?,
        previousHasMore: Bool,
        previousPageCount: Int,
        preserveLoadedTail: Bool
    ) {
        let responseHasMore = Self.requestLogPageHasMore(response)
        if preserveLoadedTail, responseHasMore {
            requestLogs = Self.mergingRequestLogs(response.logs, after: previousLogs)
        } else {
            requestLogs = Self.mergingRequestLogs(response.logs, after: [])
        }

        if preserveLoadedTail, previousPageCount > 1, responseHasMore {
            requestLogNextCursor = previousCursor
            requestLogHasMore = previousHasMore
            requestLogLoadedPageCount = previousPageCount
        } else {
            requestLogNextCursor = response.nextCursor
            requestLogHasMore = responseHasMore
            requestLogLoadedPageCount = 1
        }
        requestLogLoadedFilters = filters
    }

    private func resetRequestLogPagination(clearLogs: Bool) {
        requestLogDataGeneration += 1
        requestLogNextCursor = nil
        requestLogHasMore = false
        requestLogLoadedFilters = nil
        requestLogLoadedPageCount = 0
        requestLogLoadMoreOperationID = nil
        requestLogLoadingMore = false
        if clearLogs {
            requestLogs = []
        }
    }

    private static func mergingRequestLogs(
        _ leading: [ManageRequestLog],
        after trailing: [ManageRequestLog]
    ) -> [ManageRequestLog] {
        var seen = Set<Int64>()
        return (leading + trailing).filter { seen.insert($0.id).inserted }
    }

    private static func requestLogPageHasMore(
        _ response: ManageRequestLogsResponse,
        previousCursor: String? = nil
    ) -> Bool {
        guard let nextCursor = response.nextCursor,
              !nextCursor.isEmpty,
              nextCursor != previousCursor
        else { return false }
        return response.hasMore ?? true
    }

    /// Daemons released before server-side pagination ignore unknown query
    /// parameters and omit both metadata keys. Keep search, exact filters and
    /// ordering functional against their bounded 200-row response.
    private static func applyingLegacyRequestLogFilters(
        to response: ManageRequestLogsResponse,
        filters: RequestLogFilters
    ) -> ManageRequestLogsResponse {
        guard response.nextCursor == nil, response.hasMore == nil else {
            return response
        }

        let query = filters.query
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .lowercased()
        let logs = response.logs
            .filter { log in
                exactMatch(log.status, expected: filters.status)
                    && exactMatch(log.channel, expected: filters.channel)
                    && exactMatch(log.modelId, expected: filters.modelId)
                    && (
                        query.isEmpty
                            || [
                                log.requestId,
                                log.modelId,
                                log.channel,
                                log.status,
                                log.providerType,
                            ].contains { $0.lowercased().contains(query) }
                    )
            }
            .sorted { lhs, rhs in
                let isAscending = filters.sort == .oldest
                if lhs.createdAtMs == rhs.createdAtMs {
                    return isAscending ? lhs.id < rhs.id : lhs.id > rhs.id
                }
                return isAscending
                    ? lhs.createdAtMs < rhs.createdAtMs
                    : lhs.createdAtMs > rhs.createdAtMs
            }
        return ManageRequestLogsResponse(
            logs: logs,
            nextCursor: nil,
            hasMore: nil
        )
    }

    private static func exactMatch(_ value: String, expected: String?) -> Bool {
        guard let expected = expected?.trimmingCharacters(in: .whitespacesAndNewlines),
              !expected.isEmpty
        else { return true }
        return value.caseInsensitiveCompare(expected) == .orderedSame
    }

    private func isCurrentLoad(_ section: AppSection, generation: Int) -> Bool {
        !Task.isCancelled && sectionLoadGenerations[section] == generation
    }

    private func beginSectionActivity(_ section: AppSection) {
        sectionActivityCounts[section, default: 0] += 1
        loadingSections.insert(section)
    }

    private func endSectionActivity(_ section: AppSection) {
        let remaining = max(sectionActivityCounts[section, default: 1] - 1, 0)
        if remaining == 0 {
            sectionActivityCounts[section] = nil
            loadingSections.remove(section)
        } else {
            sectionActivityCounts[section] = remaining
        }
    }

    @discardableResult
    func configureCodex() async -> Bool {
        await performManagementAction(section: .codex) {
            _ = try await self.apiClient.configureCodex()
            try await self.requireSectionRefresh(.codex)
            return "已写入 Codex 配置"
        }
    }

    @discardableResult
    func repairCodex() async -> Bool {
        await performManagementAction(section: .codex) {
            _ = try await self.apiClient.repairCodex()
            try await self.requireSectionRefresh(.codex)
            return "已修复 GUI 环境"
        }
    }

    @discardableResult
    func uninstallCodex() async -> Bool {
        await performManagementAction(section: .codex) {
            _ = try await self.apiClient.uninstallCodex()
            try await self.requireSectionRefresh(.codex)
            return "已卸载 Codex 接入"
        }
    }

    @discardableResult
    func refreshCodexModels() async -> Bool {
        await performManagementAction(section: .codex) {
            _ = try await self.apiClient.refreshCodexModels()
            try await self.requireSectionRefresh(.codex)
            return "已刷新模型列表"
        }
    }

    @discardableResult
    func launchCodexEnhanced() async -> Bool {
        await performManagementAction(section: .codex) {
            _ = try await self.apiClient.launchCodexEnhanced()
            try await self.requireSectionRefresh(.codex)
            return "增强启动已完成"
        }
    }

    /// Re-reads the enhanced-launch preflight and returns whether Codex App
    /// is currently running. `nil` means the check itself failed.
    func checkCodexAppRunning() async -> Bool? {
        guard fixtureStatus == nil else { return false }
        guard let preflight = try? await apiClient.codexEnhancedPreflight() else { return nil }
        codexPreflight = preflight
        return preflight.status.running
    }

    @discardableResult
    func moveCodexSession(_ session: ManageCodexSession, to provider: String?) async -> Bool {
        await performManagementAction(section: .sessions) {
            _ = try await self.apiClient.moveCodexSession(
                threadId: session.id,
                targetProvider: provider
            )
            try await self.requireSectionRefresh(.sessions)
            return "已移动 1 个会话"
        }
    }

    /// Moves each session in order and reports a partial-failure summary.
    /// Successful ids are returned so the view can deselect them while
    /// keeping failed ones selected.
    func moveCodexSessions(ids: [String], to provider: String?) async -> SessionBatchMoveResult {
        guard fixtureStatus == nil, !ids.isEmpty else {
            return SessionBatchMoveResult(movedIds: [], failedIds: [])
        }
        guard sectionActivityCounts[.sessions, default: 0] == 0 else {
            return SessionBatchMoveResult(movedIds: [], failedIds: ids)
        }
        beginSectionActivity(.sessions)
        defer { endSectionActivity(.sessions) }
        managementOperationError = nil

        var movedIds: [String] = []
        var failedIds: [String] = []
        var firstErrorMessage: String?
        for id in ids {
            do {
                _ = try await apiClient.moveCodexSession(threadId: id, targetProvider: provider)
                movedIds.append(id)
            } catch {
                failedIds.append(id)
                if firstErrorMessage == nil {
                    firstErrorMessage = userFacingMessage(for: error)
                }
            }
        }
        _ = await loadSection(.sessions, force: true)
        if failedIds.isEmpty {
            sectionErrors[.sessions] = nil
            actionFeedback = ActionFeedback(message: "已移动 \(movedIds.count) 个会话")
        } else {
            var message = "移动完成：成功 \(movedIds.count) 条、失败 \(failedIds.count) 条。"
            if let firstErrorMessage {
                message += firstErrorMessage
            }
            managementOperationError = message
            sectionErrors[.sessions] = message
        }
        return SessionBatchMoveResult(movedIds: movedIds, failedIds: failedIds)
    }

    @discardableResult
    func saveGatewaySettings(
        enabled: Bool,
        filterImageGenerationTool: Bool,
        requestLoggingEnabled: Bool,
        requestLogDetailsEnabled: Bool,
        codexVisibleModels: [String]
    ) async -> Bool {
        await performManagementAction(section: .gateway) {
            self.gateway = try await self.apiClient.updateGateway(
                enabled: enabled,
                filterImageGenerationTool: filterImageGenerationTool,
                requestLoggingEnabled: requestLoggingEnabled,
                requestLogDetailsEnabled: requestLogDetailsEnabled,
                codexVisibleModels: codexVisibleModels
            )
            return "已保存网关设置"
        }
    }

    @discardableResult
    func saveGatewayProvider(
        originalName: String?,
        provider: ManageGatewayProvider,
        apiKey: String?,
        clearAPIKey: Bool = false
    ) async -> Bool {
        await performManagementAction(section: .gateway) {
            self.gateway = try await self.apiClient.upsertGatewayProvider(
                originalName: originalName,
                provider: provider,
                apiKey: apiKey,
                clearAPIKey: clearAPIKey
            )
            return "已保存 Provider「\(provider.name)」"
        }
    }

    @discardableResult
    func deleteGatewayProvider(_ provider: ManageGatewayProvider) async -> Bool {
        await performManagementAction(section: .gateway) {
            self.gateway = try await self.apiClient.deleteGatewayProvider(name: provider.name)
            return "已删除 Provider「\(provider.name)」"
        }
    }

    @discardableResult
    func saveSub2ApiAdmin(baseUrl: String, adminApiKey: String?) async -> Bool {
        await performManagementAction(section: .gateway) {
            self.sub2ApiAdmin = try await self.apiClient.updateSub2ApiAdmin(
                baseUrl: baseUrl,
                adminApiKey: adminApiKey
            )
            self.invalidateSub2ApiAccountPool()
            return "已连接 Sub2API 账号池"
        }
    }

    @discardableResult
    func disconnectSub2ApiAdmin() async -> Bool {
        await performManagementAction(section: .gateway) {
            self.sub2ApiAdmin = try await self.apiClient.disconnectSub2ApiAdmin()
            self.invalidateSub2ApiAccountPool()
            return "已断开 Sub2API 账号池"
        }
    }

    func refreshSub2ApiAccountPool(
        forceBillingRefresh: Bool = false,
        now: Date = Date()
    ) async {
        guard !sub2ApiAccountPoolLoading else { return }
        if !forceBillingRefresh,
           let fetchedAtMs = sub2ApiAccountPool?.fetchedAtMs,
           now.timeIntervalSince1970 - (Double(fetchedAtMs) / 1_000)
               < Self.sub2ApiAccountPoolCacheLifetime
        {
            return
        }

        let refreshID = UUID()
        let generation = sub2ApiAccountPoolGeneration
        sub2ApiAccountPoolRefreshID = refreshID
        sub2ApiAccountPoolLoading = true
        sub2ApiAccountPoolError = nil
        defer {
            if sub2ApiAccountPoolRefreshID == refreshID,
               sub2ApiAccountPoolGeneration == generation
            {
                sub2ApiAccountPoolRefreshID = nil
                sub2ApiAccountPoolLoading = false
            }
        }

        if sub2ApiAdmin == nil {
            let admin = try? await apiClient.sub2ApiAdmin()
            guard isCurrentSub2ApiAccountPoolRefresh(
                id: refreshID,
                generation: generation
            ) else { return }
            sub2ApiAdmin = admin
        }
        guard sub2ApiAdmin?.configured == true else {
            sub2ApiAccountPool = nil
            sub2ApiAccountPoolError = nil
            return
        }

        do {
            let response = try await apiClient.fetchSub2ApiAccounts(
                forceBillingRefresh: forceBillingRefresh
            )
            guard isCurrentSub2ApiAccountPoolRefresh(
                id: refreshID,
                generation: generation
            ) else { return }
            sub2ApiAccountPool = response.pool
        } catch {
            guard isCurrentSub2ApiAccountPoolRefresh(
                id: refreshID,
                generation: generation
            ) else { return }
            sub2ApiAccountPoolError = userFacingMessage(for: error)
        }
    }

    func cancelSub2ApiAccountPoolRefresh() {
        sub2ApiAccountPoolGeneration &+= 1
        sub2ApiAccountPoolRefreshID = nil
        sub2ApiAccountPoolLoading = false
    }

    private func invalidateSub2ApiAccountPool() {
        sub2ApiAccountPoolGeneration &+= 1
        sub2ApiAccountPoolRefreshID = nil
        sub2ApiAccountPool = nil
        sub2ApiAccountPoolLoading = false
        sub2ApiAccountPoolError = nil
    }

    private func isCurrentSub2ApiAccountPoolRefresh(
        id: UUID,
        generation: UInt64
    ) -> Bool {
        !Task.isCancelled
            && sub2ApiAccountPoolRefreshID == id
            && sub2ApiAccountPoolGeneration == generation
    }

    @discardableResult
    func saveSettings(
        language: String?,
        theme: String?,
        localConnectionMode: String,
        outboundProxyMode: String,
        outboundProxyURL: String?
    ) async -> Bool {
        await performManagementAction(section: .overview) {
            self.settings = try await self.apiClient.updateSettings(
                language: language,
                theme: theme,
                localConnectionMode: localConnectionMode,
                outboundProxyMode: outboundProxyMode,
                outboundProxyURL: outboundProxyURL
            )
            return "已保存设置"
        }
    }

    func loadSettings() async {
        guard fixtureStatus == nil else { return }
        do {
            settings = try await apiClient.settings()
            managementOperationError = nil
        } catch {
            managementOperationError = userFacingMessage(for: error)
        }
    }

    func loadRequestLogDetail(id: Int64) async {
        guard fixtureStatus == nil else { return }
        do {
            let detail = try await apiClient.requestLogDetail(id: id)
            guard !Task.isCancelled else { return }
            requestLogDetail = detail
            managementOperationError = nil
            sectionErrors[.requestLogs] = nil
        } catch {
            guard !Task.isCancelled else { return }
            let message = userFacingMessage(for: error)
            managementOperationError = message
            sectionErrors[.requestLogs] = message
        }
    }

    func clearRequestLogDetail() {
        requestLogDetail = nil
    }

    @discardableResult
    func clearRequestLogs() async -> Bool {
        await performManagementAction(section: .requestLogs) {
            let deleted = try await self.apiClient.clearRequestLogs()
            self.requestLogDetail = nil
            self.resetRequestLogPagination(clearLogs: false)
            try await self.requireSectionRefresh(.requestLogs)
            return "已清空 \(deleted) 条日志"
        }
    }

    /// Deletes logs older than `days` while keeping the recent window. The
    /// loaded detail stays untouched; the view clears its selection only
    /// when the selected row disappears from the refreshed list.
    @discardableResult
    func clearOldRequestLogs(days: Int = 3) async -> Bool {
        await performManagementAction(section: .requestLogs) {
            let deleted = try await self.apiClient.clearOldRequestLogs(days: days)
            self.resetRequestLogPagination(clearLogs: false)
            try await self.requireSectionRefresh(.requestLogs)
            return "已清理 \(deleted) 条旧日志"
        }
    }

    /// Asks the daemon to list models from an upstream provider. Throws so
    /// the editor sheet can render inline success or attempt summaries
    /// instead of the page-level feedback.
    func fetchGatewayProviderModels(
        providerName: String?,
        baseUrl: String,
        modelsUrl: String?,
        providerType: String,
        apiKey: String?
    ) async throws -> ManageProviderModelsFetchResponse {
        guard fixtureStatus == nil else {
            return ManageProviderModelsFetchResponse(ok: false, models: [], attempts: [])
        }
        return try await apiClient.fetchGatewayProviderModels(
            providerName: providerName,
            baseUrl: baseUrl,
            modelsUrl: modelsUrl,
            providerType: providerType,
            apiKey: apiKey
        )
    }

    /// Queries normalized balance and rate information for a Provider's
    /// already-saved API key. Callers own the transient result so usage checks
    /// do not put the entire Gateway section into a loading state.
    func fetchGatewayProviderUsage(
        providerName: String
    ) async throws -> ManageProviderUsageResponse {
        guard fixtureStatus == nil else {
            return ManageProviderUsageResponse(
                ok: true,
                providerName: providerName,
                usage: ManageProviderUsageResponse.Usage(
                    source: "fixture",
                    balanceStatus: "available",
                    billingStatus: "available",
                    remaining: 42.50,
                    unlimited: false,
                    unit: "USD",
                    balanceMode: "credit",
                    planName: "Fixture",
                    accountValid: true,
                    accountStatus: "active",
                    groupRateMultiplier: 1,
                    userRateMultiplier: 1,
                    resolvedRateMultiplier: 1,
                    effectiveRateMultiplier: 1,
                    peakRateEnabled: false,
                    peakStart: nil,
                    peakEnd: nil,
                    peakRateMultiplier: nil,
                    appliedPeakMultiplier: nil,
                    timezone: nil,
                    observedAt: nil
                )
            )
        }
        return try await apiClient.fetchGatewayProviderUsage(providerName: providerName)
    }

    /// Provider templates for the editor. Returns `nil` when the daemon does
    /// not support the endpoint yet (or the call fails); callers hide the
    /// template UI silently in that case.
    func loadGatewayProviderTemplates() async -> [ManageProviderTemplate]? {
        guard fixtureStatus == nil else { return nil }
        return try? await apiClient.gatewayProviderTemplates()
    }

    /// Built-in Codex model catalog for the visible-models checklist. `nil`
    /// means the UI falls back to the plain text editor.
    func loadCodexModelCatalog() async -> [ManageCodexCatalogModel]? {
        guard fixtureStatus == nil else { return nil }
        return try? await apiClient.codexModelCatalog()
    }

    /// Manually (re)starts the local daemon. Unlike the automatic launch path
    /// this is not limited to a single attempt per app lifetime.
    func startDaemonManually() async {
        guard fixtureStatus == nil, !daemonRecoveryInProgress else { return }
        daemonRecoveryInProgress = true
        defer { daemonRecoveryInProgress = false }
        daemonRecoveryError = nil
        do {
            try await daemonLauncher.startIfNeeded()
            launchAttempted = true
            dashboardState = dashboard == nil ? .starting : dashboardState
            serviceStatus = .checking
            await waitForDaemonReadiness()
            await refresh()
            if serviceStatus == .available {
                actionFeedback = ActionFeedback(message: "本地服务已启动")
            }
        } catch let error as DaemonLaunchError {
            daemonRecoveryError = error.localizedDescription
        } catch {
            daemonRecoveryError = userFacingMessage(for: error)
        }
    }

    func restartDaemon() async {
        guard fixtureStatus == nil, ownsDaemonLease,
              let lifecycle,
              let leaseGeneration = lifecycle.management.leaseGeneration
        else {
            managementOperationError = "当前界面没有后台服务管理权。"
            return
        }
        let instanceId = lifecycle.service.instanceId
        managementOperationError = nil
        do {
            let response = try await apiClient.restartLifecycle(
                installationId: installationId,
                daemonInstanceId: instanceId,
                leaseGeneration: leaseGeneration
            )
            guard response.ok else {
                throw APIClientError.operationFailed("后台服务没有接受重启请求。")
            }
            actionFeedback = ActionFeedback(message: "正在安全重启本地服务")
            dashboardState = .starting
            serviceStatus = .checking
            _ = await waitForDaemonReplacement(previousInstanceId: instanceId)
            await refresh()
        } catch {
            managementOperationError = userFacingMessage(for: error)
        }
    }

    private func waitForDaemonReplacement(previousInstanceId: String) async -> Bool {
        for attempt in 0..<80 {
            if let current = try? await apiClient.lifecycle(),
               current.service.instanceId != previousInstanceId
            {
                return true
            }
            guard attempt < 79 else { return false }
            try? await Task.sleep(for: .milliseconds(250))
        }
        return false
    }

    /// Silently checks GitHub for a newer release once per app launch. The
    /// delay keeps the check away from the startup network burst; every
    /// failure path stays silent.
    func scheduleStartupUpdateCheck(delay: Duration = .seconds(5)) {
        guard fixtureStatus == nil, !startupUpdateCheckScheduled else { return }
        startupUpdateCheckScheduled = true
        Task { [weak self] in
            try? await Task.sleep(for: delay)
            guard let self, !Task.isCancelled else { return }
            let currentVersion = Bundle.main.object(
                forInfoDictionaryKey: "CFBundleShortVersionString"
            ) as? String ?? "0.0.0"
            guard let update = await UpdateChecker.availableUpdate(
                currentVersion: currentVersion
            ) else { return }
            self.availableUpdate = update
        }
    }

    private func requireSectionRefresh(_ section: AppSection) async throws {
        guard await loadSection(section, force: true) else {
            throw ManagementRefreshError(
                message: sectionErrors[section] ?? "操作已经完成，但刷新页面数据失败。"
            )
        }
    }

    /// Runs a mutation and publishes a transient success message (returned by
    /// the operation) through the shared feedback capsule.
    private func performManagementAction(
        section: AppSection,
        operation: () async throws -> String?
    ) async -> Bool {
        guard sectionActivityCounts[section, default: 0] == 0 else { return false }
        beginSectionActivity(section)
        defer { endSectionActivity(section) }
        managementOperationError = nil
        do {
            let successMessage = try await operation()
            sectionErrors[section] = nil
            if let successMessage {
                actionFeedback = ActionFeedback(message: successMessage)
            }
            return true
        } catch {
            let message = userFacingMessage(for: error)
            managementOperationError = message
            sectionErrors[section] = message
            return false
        }
    }

    func dismissSectionError(_ section: AppSection) {
        sectionErrors[section] = nil
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
            runtime: .init(state: "active", productVersion: "0.5.0", buildNumber: nil, apiMajor: 1),
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

private struct ManagementRefreshError: LocalizedError {
    let message: String

    var errorDescription: String? { message }
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
        case .checking: "arrow.2.circlepath"
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
