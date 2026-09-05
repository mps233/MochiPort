import Foundation

/// A transient success message shown in the shared feedback capsule. The
/// identifier restarts the auto-dismiss timer when a new message arrives.
struct ActionFeedback: Equatable, Identifiable {
    let id = UUID()
    let message: String
}

private enum DaemonStartupError: LocalizedError, Equatable {
    case readinessTimedOut(attempts: Int, serviceWasAlreadyRunning: Bool)

    var errorDescription: String? {
        switch self {
        case .readinessTimedOut(_, true):
            return "后台服务进程仍在运行，但健康检查未通过。为避免中断正在进行的工作，MochiPort 未自动重启它。"
        case let .readinessTimedOut(attempts, false):
            return "已尝试启动后台服务 \(attempts) 次，但服务未在预期时间内就绪。请查看诊断后重试。"
        }
    }
}

/// Immutable lease values captured when a destructive management confirmation
/// is presented. The confirmed operation must use this exact observation so a
/// refresh cannot silently retarget it to a different owner or generation.
struct DaemonManagementConfirmation: Equatable, Sendable {
    let daemonInstanceId: String
    let daemonPID: Int
    let daemonStartedAtMs: Int64
    let leaseOwnerInstallationId: String
    let leaseGeneration: Int64
    let managementTokenGeneration: Int64
}

enum ComponentUpdateCheckState: Equatable, Sendable {
    case idle
    case checking
    case checked
    case failed(String)
}

/// The user-facing update state. UI and daemon releases remain independently
/// versioned underneath, but the app presents them through one update entry.
enum UnifiedUpdateState: Equatable, Sendable {
    case notChecked
    case checking
    case upToDate
    case failed(String)
    case ui(UpdateComponentRelease)
    case daemon(UpdateComponentRelease)
    case both(ui: UpdateComponentRelease, daemon: UpdateComponentRelease)
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
    @Published private(set) var telegramProjectGroupAccounts: [ManageTelegramProjectGroupAccount] = []
    @Published private(set) var codexStatus: ManageCodexStatus?
    @Published private(set) var codexPreflight: ManageCodexPreflightResponse?
    @Published private(set) var codexEnhancedOperation: ManageEnhancedLaunchOperation?
    @Published private(set) var codexEnhancedWaitingForAppExit = false
    @Published private(set) var codexEnhancedUsesLegacyFallback = false
    @Published private(set) var codexEnhancedLaunchError: String?
    @Published private(set) var codexSessions: [ManageCodexSession] = []
    @Published private(set) var codexSessionProviders: [String] = []
    @Published private(set) var gateway: ManageGateway?
    /// Real Sub2API daily cost for the first enabled provider with a saved key.
    /// The menu-bar dashboard uses this when available and falls back to its
    /// local Sub2API-compatible estimate otherwise.
    @Published private(set) var gatewayProviderUsage: ManageProviderUsageResponse?
    /// Most recently used Sub2API channel for the dashboard provider.
    /// This is kept separate from the provider-level total balance.
    @Published private(set) var gatewayProviderChannel: ManageSub2ApiAccountPoolResponse.Account?
    @Published private(set) var sub2ApiAdmin: ManageSub2ApiAdmin?
    @Published private(set) var sub2ApiAccountPool: ManageSub2ApiAccountPoolResponse.Pool?
    @Published private(set) var sub2ApiAccountPoolLoading = false
    @Published private(set) var sub2ApiAccountPoolError: String?
    @Published private(set) var sub2ApiAccountPoolMutationIDs: Set<Int64> = []
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
    @Published private(set) var daemonTransitionInProgress = false
    @Published private(set) var daemonLeaseTakeoverInProgress = false
    @Published private(set) var managementCredentialRotationInProgress = false
    @Published private(set) var daemonManagementFeedback: String?
    @Published var daemonRecoveryError: String?
    @Published private(set) var availableUIUpdate: UpdateComponentRelease?
    @Published private(set) var availableDaemonUpdate: UpdateComponentRelease?
    @Published private(set) var updateCheckState: ComponentUpdateCheckState = .idle
    @Published var unifiedUpdateNoticeDismissed = false

    private let apiClient: APIClient
    private let daemonLauncher: any DaemonLaunching
    private let fixtureStatus: ServiceStatus?
    private let guiBuildLoader: @Sendable () -> String?
    private let embeddedDaemonBuildLoader: @Sendable () -> String?
    private let guiVersionLoader: @Sendable () -> String
    private let updateManifestLoader: @Sendable (String) async throws -> UpdateManifest
    private let daemonReplacementAttemptLimit: Int
    private let daemonReplacementPollDelay: Duration
    private let daemonReplacementStableProbeCount: Int
    private let codexEnhancedOperationPollDelay: Duration
    private let codexEnhancedOperationRecoveryPollDelay: Duration
    private let automaticDaemonStartupAttemptLimit: Int
    private let daemonStartupReadinessAttemptLimit: Int
    private let daemonStartupReadinessPollDelay: Duration
    private let automaticDaemonStartupRetryDelay: Duration
    private var refreshInFlight = false
    private var automaticDaemonStartupAttempts = 0
    private var lastAutomaticDaemonStartupError: Error?
    private var startupRefreshStarted = false
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
    private var codexEnhancedWaitTask: Task<Void, Never>?
    private var codexEnhancedMonitorTask: Task<Void, Never>?
    private var codexEnhancedMonitorRequestId: String?
    private var codexEnhancedLegacyTask: Task<Void, Never>?
    private var startupUpdateCheckScheduled = false
    private var lifecycleLeaseTask: Task<Void, Never>?
    private var daemonTransitionGeneration: UInt64 = 0
    private var lifecycleObservationGeneration: UInt64 = 0
    private var cachedDaemonIdentity: ManageDaemonIdentity?
    private var cachedDaemonIdentityInstanceId: String?
    private let installationId: String
    private static let sub2ApiAccountPoolCacheLifetime: TimeInterval = 5 * 60
    private var sub2ApiAccountPoolGeneration: UInt64 = 0
    private var sub2ApiAccountPoolRefreshID: UUID?
    private struct Sub2ApiAccountPoolMutation {
        let generation: UInt64
        let previousSchedulable: Bool
        let requestedSchedulable: Bool
        var awaitingRefreshConfirmation: Bool
    }
    private var sub2ApiAccountPoolMutationGenerations: [Int64: UInt64] = [:]
    private var sub2ApiAccountPoolMutations: [Int64: Sub2ApiAccountPoolMutation] = [:]

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
        expectedDaemonBuild: String?,
        daemonBuild: Int?
    ) -> Bool {
        guard let expectedDaemonBuild,
              let expectedDaemonBuildNumber = Int(expectedDaemonBuild),
              expectedDaemonBuildNumber > 0,
              String(expectedDaemonBuildNumber) == expectedDaemonBuild,
              let daemonBuild
        else { return false }
        return expectedDaemonBuildNumber != daemonBuild
    }

    /// Compatibility helper for existing call sites that compare a GUI build
    /// with a daemon build. New daemon handoff code must use the explicit
    /// `expectedDaemonBuild` spelling above.
    nonisolated static func buildNumbersMismatch(
        guiBuild: String?,
        daemonBuild: Int?
    ) -> Bool {
        buildNumbersMismatch(expectedDaemonBuild: guiBuild, daemonBuild: daemonBuild)
    }

    /// A newer GUI can coordinate a safe daemon handoff after installation.
    nonisolated static func daemonRequiresUpgrade(
        expectedDaemonBuild: String?,
        daemonBuild: Int?
    ) -> Bool {
        guard let expectedDaemonBuild,
              let expectedDaemonBuildNumber = Int(expectedDaemonBuild),
              expectedDaemonBuildNumber > 0,
              String(expectedDaemonBuildNumber) == expectedDaemonBuild,
              let daemonBuild
        else { return false }
        return expectedDaemonBuildNumber > daemonBuild
    }

    /// Compatibility helper for existing call sites that compare a GUI build
    /// with a daemon build. New daemon handoff code must use the explicit
    /// `expectedDaemonBuild` spelling above.
    nonisolated static func daemonRequiresUpgrade(
        guiBuild: String?,
        daemonBuild: Int?
    ) -> Bool {
        daemonRequiresUpgrade(expectedDaemonBuild: guiBuild, daemonBuild: daemonBuild)
    }

    var daemonBuildMismatch: Bool {
        guard let daemonBuild = lifecycle?.runtime.buildNumber else { return false }
        return Self.buildNumbersMismatch(
            expectedDaemonBuild: embeddedDaemonBuildLoader(),
            daemonBuild: daemonBuild
        )
    }

    var daemonUpgradePending: Bool {
        guard let lifecycle,
              let embeddedDaemonBuild = embeddedDaemonBuildLoader()
        else { return false }
        return Self.daemonRequiresUpgrade(
            expectedDaemonBuild: embeddedDaemonBuild,
            daemonBuild: lifecycle.runtime.buildNumber
        )
    }

    var daemonUpgradeDetail: String {
        Self.embeddedDaemonUpgradeDetailText(
            daemonBuild: embeddedDaemonBuildLoader(),
            runningDaemonBuild: lifecycle?.runtime.buildNumber
        )
    }

    var currentUIVersion: String { guiVersionLoader() }

    var unifiedUpdateState: UnifiedUpdateState {
        switch (availableUIUpdate, availableDaemonUpdate) {
        case let (.some(ui), .some(daemon)):
            return .both(ui: ui, daemon: daemon)
        case let (.some(ui), .none):
            return .ui(ui)
        case let (.none, .some(daemon)):
            return .daemon(daemon)
        case (.none, .none):
            switch updateCheckState {
            case .checking:
                return .checking
            case let .failed(message):
                return .failed(message)
            case .idle:
                return .notChecked
            case .checked:
                return .upToDate
            }
        }
    }

    var hasAvailableUnifiedUpdate: Bool {
        availableUIUpdate != nil || availableDaemonUpdate != nil
    }

    var currentUIBuild: Int? {
        guiBuildLoader().flatMap(Int.init)
    }

    nonisolated static func daemonUpgradeDetailText(
        guiBuild: String?,
        daemonBuild: Int?
    ) -> String {
        guard let guiBuild,
              let guiBuildNumber = Int(guiBuild),
              let daemonBuild
        else { return "后台版本未知" }
        if daemonBuild > guiBuildNumber {
            return "后台构建 \(daemonBuild) 高于界面 \(guiBuildNumber)，需要手动处理"
        }
        if daemonBuild == guiBuildNumber {
            return "版本一致"
        }
        return "界面构建 \(guiBuildNumber) 高于后台 \(daemonBuild)，安装新版 MochiPort 后会自动安全切换"
    }

    nonisolated static func embeddedDaemonUpgradeDetailText(
        daemonBuild: String?,
        runningDaemonBuild: Int?
    ) -> String {
        guard let daemonBuild,
              let expectedBuild = Int(daemonBuild),
              let runningDaemonBuild
        else { return "内置后台版本未知" }
        if runningDaemonBuild > expectedBuild {
            return "运行中的后台构建 \(runningDaemonBuild) 高于内置构建 \(expectedBuild)，不会自动降级"
        }
        if runningDaemonBuild == expectedBuild {
            return "后台版本一致"
        }
        return "内置后台构建 \(expectedBuild) 高于运行中的后台 \(runningDaemonBuild)，将自动安全切换"
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

    var canTakeOverDaemonLease: Bool {
        guard daemonLeaseConflict,
              lifecycle?.management.leaseGeneration != nil,
              lifecycle?.management.managementTokenGeneration != nil
        else { return false }
        return !daemonTransitionInProgress
            && !daemonLeaseTakeoverInProgress
            && !managementCredentialRotationInProgress
    }

    var canRotateManagementCredential: Bool {
        guard ownsDaemonLease,
              lifecycle?.management.leaseGeneration != nil,
              lifecycle?.management.managementTokenGeneration != nil
        else { return false }
        return !daemonTransitionInProgress
            && !daemonLeaseTakeoverInProgress
            && !managementCredentialRotationInProgress
    }

    var daemonLeaseTakeoverConfirmation: DaemonManagementConfirmation? {
        guard canTakeOverDaemonLease, let lifecycle else { return nil }
        return daemonManagementConfirmation(for: lifecycle)
    }

    var managementCredentialRotationConfirmation: DaemonManagementConfirmation? {
        guard canRotateManagementCredential, let lifecycle else { return nil }
        return daemonManagementConfirmation(for: lifecycle)
    }

    var codexEnhancedLaunchInProgress: Bool {
        codexEnhancedWaitingForAppExit
            || codexEnhancedOperation?.isRunning == true
            || codexEnhancedLegacyTask != nil
    }

    var canCancelCodexEnhancedLaunch: Bool {
        codexEnhancedWaitingForAppExit
            || codexEnhancedOperation?.canCancel == true
            || codexEnhancedLegacyTask != nil
    }

    private var currentTimeMilliseconds: Int64 {
        Int64(Date().timeIntervalSince1970 * 1_000)
    }

    private func daemonManagementConfirmation(
        for lifecycle: ManageLifecycle
    ) -> DaemonManagementConfirmation? {
        guard let owner = lifecycle.management.installationId,
              let leaseGeneration = lifecycle.management.leaseGeneration,
              let managementTokenGeneration = lifecycle.management.managementTokenGeneration
        else { return nil }
        return DaemonManagementConfirmation(
            daemonInstanceId: lifecycle.service.instanceId,
            daemonPID: lifecycle.service.pid,
            daemonStartedAtMs: lifecycle.service.startedAtMs,
            leaseOwnerInstallationId: owner,
            leaseGeneration: leaseGeneration,
            managementTokenGeneration: managementTokenGeneration
        )
    }

    private func lifecycleMatchesConfirmation(
        _ lifecycle: ManageLifecycle,
        _ confirmation: DaemonManagementConfirmation
    ) -> Bool {
        lifecycle.service.instanceId == confirmation.daemonInstanceId
            && lifecycle.service.pid == confirmation.daemonPID
            && lifecycle.service.startedAtMs == confirmation.daemonStartedAtMs
            && lifecycle.management.installationId == confirmation.leaseOwnerInstallationId
            && lifecycle.management.leaseGeneration == confirmation.leaseGeneration
            && lifecycle.management.managementTokenGeneration
                == confirmation.managementTokenGeneration
    }

    private func lifecycleObservationIsCurrent(_ generation: UInt64) -> Bool {
        generation == lifecycleObservationGeneration
            && !daemonLeaseTakeoverInProgress
            && !managementCredentialRotationInProgress
    }

    init(
        apiClient: APIClient = APIClient(),
        daemonLauncher: any DaemonLaunching = DaemonLauncher(),
        fixtureStatus: ServiceStatus? = nil,
        guiBuildLoader: @escaping @Sendable () -> String? = {
            Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String
        },
        embeddedDaemonBuildLoader: @escaping @Sendable () -> String? = {
            let value = Bundle.main.object(forInfoDictionaryKey: "MochiPortDaemonBuild")
                ?? Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion")
            if let string = value as? String {
                let trimmed = string.trimmingCharacters(in: .whitespacesAndNewlines)
                return trimmed.isEmpty ? nil : trimmed
            }
            if let number = value as? NSNumber {
                return number.stringValue
            }
            return nil
        },
        guiVersionLoader: @escaping @Sendable () -> String = {
            Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String
                ?? "0.0.0"
        },
        updateManifestLoader: @escaping @Sendable (String) async throws -> UpdateManifest = {
            try await UpdateChecker.fetchLatestManifest(currentVersion: $0)
        },
        daemonReplacementAttemptLimit: Int = 80,
        daemonReplacementPollDelay: Duration = .milliseconds(250),
        daemonReplacementStableProbeCount: Int = 3,
        codexEnhancedOperationPollDelay: Duration = .milliseconds(400),
        codexEnhancedOperationRecoveryPollDelay: Duration = .seconds(2),
        automaticDaemonStartupAttemptLimit: Int = 2,
        daemonStartupReadinessAttemptLimit: Int = 30,
        daemonStartupReadinessPollDelay: Duration = .milliseconds(250),
        automaticDaemonStartupRetryDelay: Duration = .seconds(1)
    ) {
        self.apiClient = apiClient
        self.daemonLauncher = daemonLauncher
        self.fixtureStatus = fixtureStatus
        self.guiBuildLoader = guiBuildLoader
        self.embeddedDaemonBuildLoader = embeddedDaemonBuildLoader
        self.guiVersionLoader = guiVersionLoader
        self.updateManifestLoader = updateManifestLoader
        self.daemonReplacementAttemptLimit = max(1, daemonReplacementAttemptLimit)
        self.daemonReplacementPollDelay = daemonReplacementPollDelay
        self.daemonReplacementStableProbeCount = max(1, daemonReplacementStableProbeCount)
        self.codexEnhancedOperationPollDelay = codexEnhancedOperationPollDelay
        self.codexEnhancedOperationRecoveryPollDelay = codexEnhancedOperationRecoveryPollDelay
        self.automaticDaemonStartupAttemptLimit = max(1, automaticDaemonStartupAttemptLimit)
        self.daemonStartupReadinessAttemptLimit = max(1, daemonStartupReadinessAttemptLimit)
        self.daemonStartupReadinessPollDelay = daemonStartupReadinessPollDelay
        self.automaticDaemonStartupRetryDelay = automaticDaemonStartupRetryDelay
        self.installationId = Self.loadInstallationID()
    }

    deinit {
        refreshTask?.cancel()
        lifecycleLeaseTask?.cancel()
        codexEnhancedWaitTask?.cancel()
        codexEnhancedMonitorTask?.cancel()
        codexEnhancedLegacyTask?.cancel()
    }

    func startAutoRefresh() {
        guard !autoRefreshStarted else { return }
        autoRefreshStarted = true
        restartAutoRefresh()
    }

    /// Starts the first daemon probe independently of the main window. The
    /// menu-bar-only launch path may never materialize `RootView`, so keeping
    /// this work behind the window task leaves the daemon unregistered.
    func startAtAppLaunch() async {
        guard !startupRefreshStarted else { return }
        startupRefreshStarted = true
        await refresh()
        startAutoRefresh()
        scheduleStartupUpdateCheck()
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
            telegramProjectGroupAccounts = []
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
            let probe = try await apiClient.probe()
            resetAutomaticDaemonStartupAttempts()
            return .success(probe)
        } catch let error as APIClientError {
            return .failure(error)
        } catch {
            guard isDaemonTransportUnavailable(error) else {
                return .failure(error)
            }
            let remainingAttempts = automaticDaemonStartupAttemptLimit
                - automaticDaemonStartupAttempts
            guard remainingAttempts > 0 else {
                return .failure(lastAutomaticDaemonStartupError ?? error)
            }
            do {
                let probe = try await startDaemonAndWait(
                    attemptLimit: remainingAttempts,
                    countsTowardAutomaticLimit: true
                )
                resetAutomaticDaemonStartupAttempts()
                return .success(probe)
            } catch {
                lastAutomaticDaemonStartupError = error
                return .failure(error)
            }
        }
    }

    private func startDaemonAndWait(
        attemptLimit: Int,
        countsTowardAutomaticLimit: Bool
    ) async throws -> ServiceProbe {
        var lastOutcome: DaemonLaunchOutcome?
        for attempt in 0..<max(1, attemptLimit) {
            if countsTowardAutomaticLimit {
                automaticDaemonStartupAttempts += 1
            }
            lastOutcome = try await daemonLauncher.startIfNeeded()
            dashboardState = dashboard == nil ? .starting : dashboardState
            serviceStatus = .checking
            if let probe = try await waitForDaemonReadiness() {
                return probe
            }
            // A live process may be finishing startup or be unhealthy.  Do
            // not reinterpret its failed health check as permission to issue
            // a second launch command.
            if lastOutcome == .alreadyRunning {
                break
            }
            guard attempt + 1 < max(1, attemptLimit) else { break }
            try await Task.sleep(for: automaticDaemonStartupRetryDelay)
        }
        throw DaemonStartupError.readinessTimedOut(
            attempts: max(1, attemptLimit),
            serviceWasAlreadyRunning: lastOutcome == .alreadyRunning
        )
    }

    private func waitForDaemonReadiness() async throws -> ServiceProbe? {
        for attempt in 0..<daemonStartupReadinessAttemptLimit {
            try Task.checkCancellation()
            do {
                let probe = try await apiClient.probe()
                try Task.checkCancellation()
                if case let .versioned(health) = probe, health.ready {
                    return probe
                }
            } catch is CancellationError {
                throw CancellationError()
            } catch let error as URLError where error.code == .cancelled {
                throw CancellationError()
            } catch {
                try Task.checkCancellation()
            }
            guard attempt + 1 < daemonStartupReadinessAttemptLimit else { break }
            try await Task.sleep(for: daemonStartupReadinessPollDelay)
        }
        try Task.checkCancellation()
        return nil
    }

    private func isDaemonTransportUnavailable(_ error: Error) -> Bool {
        guard let urlError = error as? URLError else { return false }
        switch urlError.code {
        case .cannotConnectToHost, .networkConnectionLost, .notConnectedToInternet, .timedOut:
            return true
        default:
            return false
        }
    }

    private func resetAutomaticDaemonStartupAttempts() {
        automaticDaemonStartupAttempts = 0
        lastAutomaticDaemonStartupError = nil
    }

    private func loadServiceStatus(probe: ServiceProbe) async {
        let transitionGeneration = daemonTransitionGeneration
        let observationGeneration = lifecycleObservationGeneration
        switch probe {
        case let .versioned(health):
            serviceStatus = health.ready ? .available : .unavailable("服务正在启动")
            guard health.ready else {
                dashboardState = dashboard == nil ? .starting : .stale
                return
            }
            do {
                let loadedDashboard = try await apiClient.dashboard()
                guard transitionGeneration == daemonTransitionGeneration else { return }
                dashboard = loadedDashboard
                let loadedLifecycle: ManageLifecycle?
                do {
                    loadedLifecycle = try await apiClient.lifecycle()
                } catch let error as APIClientError where error == .featureUnavailable {
                    loadedLifecycle = nil
                } catch {
                    loadedLifecycle = nil
                }
                guard transitionGeneration == daemonTransitionGeneration,
                      lifecycleObservationIsCurrent(observationGeneration)
                else { return }
                lifecycle = loadedLifecycle
                if daemonTransitionInProgress {
                    stopLifecycleLeaseHeartbeat()
                } else {
                    await reconcileLifecycleLease()
                    _ = await coordinateDaemonUpgradeIfNeeded()
                    guard transitionGeneration == daemonTransitionGeneration,
                          lifecycleObservationIsCurrent(observationGeneration),
                          !daemonTransitionInProgress
                    else { return }
                }
                settings = try? await apiClient.settings()
                guard transitionGeneration == daemonTransitionGeneration,
                      lifecycleObservationIsCurrent(observationGeneration),
                      !daemonTransitionInProgress
                else { return }
                // Account details were added after the first versioned
                // dashboard. Keep the dashboard usable when an older daemon
                // is still running, but expose the missing capability instead
                // of presenting it as an empty account list.
                await loadIMAccounts()
                guard transitionGeneration == daemonTransitionGeneration,
                      lifecycleObservationIsCurrent(observationGeneration),
                      !daemonTransitionInProgress
                else { return }
                await loadTelegramProjectGroups()
                guard transitionGeneration == daemonTransitionGeneration,
                      lifecycleObservationIsCurrent(observationGeneration),
                      !daemonTransitionInProgress
                else { return }
                dashboardState = .loaded
            } catch let error as APIClientError {
                guard lifecycleObservationIsCurrent(observationGeneration) else { return }
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
        }
    }

    private func clearManagementPages() {
        stopLifecycleLeaseHeartbeat()
        stopCodexEnhancedClientTasks()
        for section in AppSection.allCases {
            sectionLoadGenerations[section, default: 0] += 1
        }
        codexStatus = nil
        codexPreflight = nil
        codexEnhancedOperation = nil
        codexEnhancedWaitingForAppExit = false
        codexEnhancedUsesLegacyFallback = false
        codexEnhancedLaunchError = nil
        codexSessions = []
        codexSessionProviders = []
        gateway = nil
        gatewayProviderUsage = nil
        gatewayProviderChannel = nil
        sub2ApiAdmin = nil
        invalidateSub2ApiAccountPool()
        settings = nil
        resetRequestLogPagination(clearLogs: true)
        requestLogDetail = nil
        sectionErrors = [:]
    }

    /// Treat an embedded-daemon/runtime version mismatch as one coordinated
    /// lifecycle transaction. The daemon owns the safety decision through its
    /// lease and drain API; launchd is reloaded only after that request is
    /// accepted.
    @discardableResult
    private func coordinateDaemonUpgradeIfNeeded() async -> Bool {
        guard fixtureStatus == nil,
              !daemonTransitionInProgress,
              daemonUpgradePending,
              let observedLifecycle = lifecycle,
              let embeddedDaemonBuild = embeddedDaemonBuildLoader()
        else {
            return false
        }

        guard observedLifecycle.protectedWorkItems.total == 0 else {
            managementOperationError = "内置后台构建 \(embeddedDaemonBuild) 高于运行中的构建 \(observedLifecycle.runtime.buildNumber.map(String.init) ?? "未知")，但后台仍有 \(observedLifecycle.protectedWorkItems.total) 项受保护任务，已暂缓自动切换。"
            return false
        }
        guard ownsDaemonLease,
              observedLifecycle.management.leaseGeneration != nil
        else {
            managementOperationError = "内置后台构建 \(embeddedDaemonBuild) 高于当前后台版本，但当前界面没有有效的后台管理租约，未自动切换。"
            return false
        }
        guard let expectedBuild = Int(embeddedDaemonBuild), expectedBuild > 0 else {
            managementOperationError = "内置后台构建号无效（\(embeddedDaemonBuild)），未自动切换后台服务。"
            return false
        }

        daemonTransitionInProgress = true
        daemonTransitionGeneration &+= 1
        lifecycleObservationGeneration &+= 1
        stopLifecycleLeaseHeartbeat()
        defer {
            daemonTransitionInProgress = false
            if ownsDaemonLease {
                startLifecycleLeaseHeartbeat()
            }
        }

        let previousInstanceId = observedLifecycle.service.instanceId
        dashboardState = .starting
        serviceStatus = .checking
        actionFeedback = ActionFeedback(message: "正在将后台服务切换到最新版本")
        managementOperationError = nil

        var activatedPreparation: DaemonRuntimeUpgradePlan?
        do {
            let preparation = try await daemonLauncher.prepareRuntimeUpgrade()
            guard preparation.targetBuildIdentifier == embeddedDaemonBuild,
                  let preparationBuild = Int(preparation.targetBuildIdentifier),
                  preparationBuild == expectedBuild
            else {
                throw APIClientError.operationFailed(
                    "准备的后台构建 \(preparation.targetBuildIdentifier) 与内置构建 \(embeddedDaemonBuild) 不一致，已取消切换。"
                )
            }
            guard preparation.requiresActivation,
                  let observedBuild = observedLifecycle.runtime.buildNumber,
                  preparation.previousBuildIdentifier == String(observedBuild)
            else {
                throw APIClientError.operationFailed(
                    "后台运行版本在准备期间发生变化，请刷新后重试。"
                )
            }
            guard lifecycle?.service.instanceId == observedLifecycle.service.instanceId,
                  lifecycle?.service.pid == observedLifecycle.service.pid,
                  lifecycle?.service.startedAtMs == observedLifecycle.service.startedAtMs,
                  lifecycle?.management.installationId == installationId,
                  lifecycle?.management.leaseGeneration
                    == observedLifecycle.management.leaseGeneration
            else {
                throw APIClientError.operationFailed("后台服务状态在版本切换前发生变化，请刷新后重试。")
            }
            try await requestLifecycleRestart(observedLifecycle)
            let activation = try await daemonLauncher.activateRuntimeUpgrade(preparation)
            guard case .activated = activation else {
                throw APIClientError.operationFailed("后台运行版本在切换前发生变化，请刷新后重试。")
            }
            activatedPreparation = preparation

            guard let replacement = try await waitForDaemonReplacement(
                previousInstanceId: previousInstanceId,
                expectedBuild: expectedBuild
            ) else {
                throw APIClientError.operationFailed("新版本后台服务未在预期时间内就绪。")
            }
            guard let reclaimed = try await claimLifecycleForRecoveryWait(replacement) else {
                throw APIClientError.operationFailed("新版本后台服务已就绪，但当前界面未能重新取得管理权。")
            }
            lifecycle = reclaimed
            clearCachedDaemonIdentity()
            dashboard = try? await apiClient.dashboard()
            settings = try? await apiClient.settings()
            actionFeedback = ActionFeedback(message: "后台服务已自动切换到最新版本")
            markDaemonReachable()
            return true
        } catch {
            if let preparation = activatedPreparation {
                do {
                    try await daemonLauncher.rollbackRuntimeUpgrade(preparation)
                    guard let previousBuild = preparation.previousBuildIdentifier,
                          let previousBuildNumber = Int(previousBuild),
                          let replacement = try await waitForDaemonReplacement(
                              previousInstanceId: previousInstanceId,
                              expectedBuild: previousBuildNumber
                          ),
                          let reclaimed = try await claimLifecycleForRecoveryWait(replacement)
                    else {
                        throw APIClientError.operationFailed("旧版本后台服务未能恢复并通过身份校验。")
                    }
                    lifecycle = reclaimed
                    clearCachedDaemonIdentity()
                    dashboard = try? await apiClient.dashboard()
                    settings = try? await apiClient.settings()
                    markDaemonReachable()
                    managementOperationError = "后台服务新版本未就绪，已自动回滚到构建 \(preparation.previousBuildIdentifier ?? "未知")。"
                } catch {
                    managementOperationError = "后台服务版本切换失败，且自动回滚未完成：\(userFacingMessage(for: error))"
                }
            } else {
                managementOperationError = "后台服务版本切换失败：\(userFacingMessage(for: error))"
            }
            actionFeedback = nil
            return false
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
            gatewayProviderUsage = nil
            gatewayProviderChannel = nil
            sub2ApiAdmin = nil
            invalidateSub2ApiAccountPool()
        case .accountPool:
            sub2ApiAdmin = nil
            invalidateSub2ApiAccountPool()
        case .requestLogs:
            resetRequestLogPagination(clearLogs: true)
            requestLogDetail = nil
        }
    }

    private func reconcileLifecycleLease() async {
        guard fixtureStatus == nil,
              !daemonTransitionInProgress,
              !daemonLeaseTakeoverInProgress,
              !managementCredentialRotationInProgress,
              let lifecycle
        else {
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
            let transitionGeneration = daemonTransitionGeneration
            let observationGeneration = lifecycleObservationGeneration
            let instanceId = lifecycle.service.instanceId
            let daemonIdentity = try await verifiedDaemonIdentity(
                for: lifecycle,
                forceRefresh: true
            )
            let claimed = try await apiClient.claimLifecycleLease(
                installationId: installationId,
                daemonInstanceId: instanceId,
                daemonIdentity: daemonIdentity
            )
            guard transitionGeneration == daemonTransitionGeneration,
                  lifecycleObservationIsCurrent(observationGeneration),
                  !daemonTransitionInProgress,
                  self.lifecycle?.service.instanceId == instanceId
            else { return }
            self.lifecycle = claimed
            startLifecycleLeaseHeartbeat()
        } catch {
            // Claiming is opportunistic. A second installation may own the
            // lease, or an older daemon may not expose the endpoint yet.
            stopLifecycleLeaseHeartbeat()
        }
    }

    private func startLifecycleLeaseHeartbeat() {
        guard lifecycleLeaseTask == nil,
              !daemonTransitionInProgress,
              !daemonLeaseTakeoverInProgress,
              !managementCredentialRotationInProgress
        else { return }
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
        guard !daemonTransitionInProgress,
              !daemonLeaseTakeoverInProgress,
              !managementCredentialRotationInProgress,
              ownsDaemonLease,
              let observedLifecycle = lifecycle
        else {
            stopLifecycleLeaseHeartbeat()
            return
        }
        let transitionGeneration = daemonTransitionGeneration
        let observationGeneration = lifecycleObservationGeneration
        let instanceId = observedLifecycle.service.instanceId
        do {
            let daemonIdentity = try await verifiedDaemonIdentity(for: observedLifecycle)
            let renewed = try await apiClient.renewLifecycleLease(
                installationId: installationId,
                daemonInstanceId: instanceId,
                daemonIdentity: daemonIdentity
            )
            guard !Task.isCancelled,
                  transitionGeneration == daemonTransitionGeneration,
                  lifecycleObservationIsCurrent(observationGeneration),
                  !daemonTransitionInProgress,
                  !daemonLeaseTakeoverInProgress,
                  !managementCredentialRotationInProgress,
                  lifecycle?.service.instanceId == instanceId
            else { return }
            lifecycle = renewed
        } catch {
            guard !Task.isCancelled,
                  transitionGeneration == daemonTransitionGeneration,
                  lifecycleObservationIsCurrent(observationGeneration),
                  !daemonTransitionInProgress,
                  !daemonLeaseTakeoverInProgress,
                  !managementCredentialRotationInProgress
            else { return }
            stopLifecycleLeaseHeartbeat()
            clearCachedDaemonIdentity()
            let refreshed = try? await apiClient.lifecycle()
            guard !Task.isCancelled,
                  transitionGeneration == daemonTransitionGeneration,
                  lifecycleObservationIsCurrent(observationGeneration),
                  !daemonTransitionInProgress,
                  !daemonLeaseTakeoverInProgress,
                  !managementCredentialRotationInProgress
            else { return }
            lifecycle = refreshed
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

    private func loadTelegramProjectGroups() async {
        guard fixtureStatus == nil else { return }
        do {
            telegramProjectGroupAccounts = try await apiClient.telegramProjectGroups().accounts
        } catch let error as APIClientError where error == .featureUnavailable {
            telegramProjectGroupAccounts = []
        } catch {
            // Project-group support is optional on older daemons. Keep the
            // account page usable even when this secondary request fails.
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
                async let preflightRequest = try? apiClient.codexEnhancedPreflight()
                async let operationRequest = try? apiClient.codexEnhancedOperation()
                let status = try await apiClient.codexStatus()
                let preflight = await preflightRequest
                let operation = await operationRequest
                guard isCurrentLoad(section, generation: generation) else { return false }
                codexStatus = status
                codexPreflight = preflight
                if let operation {
                    codexEnhancedOperation = operation
                    codexEnhancedUsesLegacyFallback = false
                    updateCodexEnhancedPresentation(for: operation)
                    if operation.isRunning {
                        monitorCodexEnhancedOperation(requestId: operation.requestId)
                    }
                }
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
                await loadTelegramProjectGroups()
            case .gateway:
                async let gatewayResponse = apiClient.gateway()
                async let sub2ApiResponse = try? apiClient.sub2ApiAdmin()
                let response = try await gatewayResponse
                let admin = await sub2ApiResponse
                guard isCurrentLoad(section, generation: generation) else { return false }
                gateway = response
                sub2ApiAdmin = admin
            case .accountPool:
                // The gateway config is fetched only to suggest a Sub2API
                // base URL in the connect form; the pool refresh itself is
                // cache-guarded and skips the request within its lifetime.
                if gateway == nil {
                    gateway = try? await apiClient.gateway()
                }
                guard isCurrentLoad(section, generation: generation) else { return false }
                await refreshSub2ApiAccountPool(forceBillingRefresh: force)
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
            let currentGateway = try await self.gatewaySnapshot()
            let gatewayWasEnabled = currentGateway.enabled
            if !gatewayWasEnabled {
                self.gateway = try await self.updateGateway(currentGateway, enabled: true)
            }

            do {
                _ = try await self.apiClient.configureCodex()
            } catch {
                if !gatewayWasEnabled {
                    self.gateway = try? await self.updateGateway(currentGateway, enabled: false)
                }
                throw error
            }

            try await self.requireSectionRefresh(.codex)
            return "已连接 MochiPort"
        }
    }

    @discardableResult
    func switchCodexToDirectApiMode() async -> Bool {
        await performManagementAction(section: .codex) {
            _ = try await self.apiClient.switchCodexToDirectApiMode()
            try await self.requireSectionRefresh(.codex)
            return "已切换到原来的连接"
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
            let currentGateway = try await self.gatewaySnapshot()
            let response = try await self.apiClient.uninstallCodex()
            self.gateway = try await self.updateGateway(currentGateway, enabled: false)
            try await self.requireSectionRefresh(.codex)
            if response.requiresCodexRestart == true {
                return "已恢复设置，MochiPort 已关闭\n请完全退出并重新打开 Codex"
            }
            return "已恢复原来的 Codex 设置，MochiPort 已关闭"
        }
    }

    private func gatewaySnapshot() async throws -> ManageGateway {
        if let gateway {
            return gateway
        }
        return try await apiClient.gateway()
    }

    private func updateGateway(
        _ current: ManageGateway,
        enabled: Bool
    ) async throws -> ManageGateway {
        guard current.enabled != enabled else { return current }
        return try await apiClient.updateGateway(
            enabled: enabled,
            filterImageGenerationTool: current.filterImageGenerationTool,
            requestLoggingEnabled: current.requestLoggingEnabled,
            requestLogDetailsEnabled: current.requestLogDetailsEnabled,
            codexVisibleModels: current.codexVisibleModels
        )
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
    func beginCodexEnhancedLaunch() async -> Bool {
        guard fixtureStatus == nil, !codexEnhancedLaunchInProgress else { return false }
        stopCodexEnhancedClientTasks()
        codexEnhancedOperation = nil
        codexEnhancedLaunchError = nil
        codexEnhancedUsesLegacyFallback = false
        managementOperationError = nil

        do {
            let preflight = try await apiClient.codexEnhancedPreflight()
            codexPreflight = preflight
            if preflight.status.running {
                waitForCodexAppExit()
            } else {
                await startCodexEnhancedOperation()
            }
            return true
        } catch let error as APIClientError where error == .featureUnavailable {
            // Older daemons do not expose a preflight route. Their synchronous
            // launch endpoint remains the only compatible path.
            startLegacyCodexEnhancedLaunch()
            return true
        } catch {
            codexEnhancedLaunchError = userFacingMessage(for: error)
            return false
        }
    }

    func cancelCodexEnhancedLaunch() async {
        if codexEnhancedWaitingForAppExit {
            codexEnhancedWaitTask?.cancel()
            codexEnhancedWaitTask = nil
            codexEnhancedWaitingForAppExit = false
            actionFeedback = ActionFeedback(message: "已取消增强启动")
            return
        }

        if let legacyTask = codexEnhancedLegacyTask {
            let requestId = codexEnhancedOperation?.requestId ?? UUID().uuidString.lowercased()
            legacyTask.cancel()
            codexEnhancedLegacyTask = nil
            codexEnhancedOperation = localEnhancedOperation(
                requestId: requestId,
                phase: "cancelled",
                canCancel: false,
                message: "已停止等待旧版后台服务",
                recovery: "旧版后台服务不支持服务端取消；Codex App 仍可能继续启动。"
            )
            return
        }

        guard let operation = codexEnhancedOperation,
              operation.isRunning,
              operation.canCancel
        else { return }
        do {
            let cancelled = try await apiClient.cancelCodexEnhancedOperation(
                requestId: operation.requestId
            )
            codexEnhancedOperation = cancelled
            updateCodexEnhancedPresentation(for: cancelled)
        } catch {
            codexEnhancedLaunchError = "取消失败：\(userFacingMessage(for: error))"
        }
    }

    private func waitForCodexAppExit() {
        codexEnhancedWaitingForAppExit = true
        codexEnhancedWaitTask?.cancel()
        codexEnhancedWaitTask = Task { [weak self] in
            guard let self else { return }
            while !Task.isCancelled {
                do {
                    let preflight = try await self.apiClient.codexEnhancedPreflight()
                    try Task.checkCancellation()
                    self.codexPreflight = preflight
                    if !preflight.status.running {
                        self.codexEnhancedWaitingForAppExit = false
                        self.codexEnhancedWaitTask = nil
                        await self.startCodexEnhancedOperation()
                        return
                    }
                    try await Task.sleep(for: .seconds(1))
                } catch is CancellationError {
                    return
                } catch {
                    guard !Task.isCancelled else { return }
                    self.codexEnhancedWaitingForAppExit = false
                    self.codexEnhancedWaitTask = nil
                    self.codexEnhancedLaunchError =
                        "无法确认 Codex App 是否已退出：\(self.userFacingMessage(for: error))"
                    return
                }
            }
        }
    }

    private func startCodexEnhancedOperation() async {
        let requestId = UUID().uuidString.lowercased()
        do {
            let operation = try await apiClient.startCodexEnhancedOperation(requestId: requestId)
            codexEnhancedOperation = operation
            codexEnhancedUsesLegacyFallback = false
            updateCodexEnhancedPresentation(for: operation)
            if operation.isRunning {
                monitorCodexEnhancedOperation(requestId: operation.requestId)
            }
        } catch let error as APIClientError where error == .featureUnavailable {
            startLegacyCodexEnhancedLaunch(requestId: requestId)
        } catch {
            // A response can be lost after the daemon accepted the request.
            // Re-read the singleton operation before reporting a start error.
            if let operation = try? await apiClient.codexEnhancedOperation(),
               operation.isRunning
            {
                codexEnhancedOperation = operation
                codexEnhancedUsesLegacyFallback = false
                updateCodexEnhancedPresentation(for: operation)
                monitorCodexEnhancedOperation(requestId: operation.requestId)
            } else {
                codexEnhancedLaunchError = userFacingMessage(for: error)
            }
        }
    }

    private func monitorCodexEnhancedOperation(requestId: String) {
        if codexEnhancedMonitorTask != nil,
           codexEnhancedMonitorRequestId == requestId
        {
            return
        }
        codexEnhancedMonitorTask?.cancel()
        codexEnhancedMonitorRequestId = requestId
        codexEnhancedMonitorTask = Task { [weak self] in
            guard let self else { return }
            var consecutiveFailures = 0
            while !Task.isCancelled {
                do {
                    let pollDelay = consecutiveFailures >= 3
                        ? self.codexEnhancedOperationRecoveryPollDelay
                        : self.codexEnhancedOperationPollDelay
                    try await Task.sleep(for: pollDelay)
                    try Task.checkCancellation()
                    guard let operation = try await self.apiClient.codexEnhancedOperation(),
                          operation.requestId == requestId
                    else {
                        throw APIClientError.invalidResponse
                    }
                    try Task.checkCancellation()
                    consecutiveFailures = 0
                    self.codexEnhancedOperation = operation
                    self.updateCodexEnhancedPresentation(for: operation)
                    if operation.isTerminal {
                        self.codexEnhancedMonitorTask = nil
                        self.codexEnhancedMonitorRequestId = nil
                        await self.finishCodexEnhancedOperation(operation)
                        return
                    }
                } catch is CancellationError {
                    return
                } catch {
                    guard !Task.isCancelled else { return }
                    consecutiveFailures += 1
                    if consecutiveFailures >= 3 {
                        self.codexEnhancedLaunchError =
                            "暂时无法读取增强启动进度：\(self.userFacingMessage(for: error))"
                    }
                }
            }
        }
    }

    private func startLegacyCodexEnhancedLaunch(requestId: String = UUID().uuidString.lowercased()) {
        let startedAt = currentTimeMilliseconds
        codexEnhancedUsesLegacyFallback = true
        codexEnhancedLaunchError = nil
        codexEnhancedOperation = ManageEnhancedLaunchOperation(
            requestId: requestId,
            phase: "launching",
            startedAtMs: startedAt,
            updatedAtMs: startedAt,
            canCancel: true,
            message: "正在等待旧版后台服务完成增强启动",
            error: nil,
            recovery: "取消只能停止本机等待，无法中止旧版后台服务中的启动。"
        )
        codexEnhancedLegacyTask?.cancel()
        codexEnhancedLegacyTask = Task { [weak self] in
            guard let self else { return }
            do {
                _ = try await self.apiClient.launchCodexEnhanced()
                try Task.checkCancellation()
                self.codexEnhancedLegacyTask = nil
                self.codexEnhancedOperation = self.localEnhancedOperation(
                    requestId: requestId,
                    phase: "ready",
                    canCancel: false,
                    message: "增强启动已完成"
                )
                self.actionFeedback = ActionFeedback(message: "增强启动已完成")
                _ = await self.loadSection(.codex, force: true)
            } catch is CancellationError {
                return
            } catch {
                guard !Task.isCancelled else { return }
                self.codexEnhancedLegacyTask = nil
                let message = self.userFacingMessage(for: error)
                self.codexEnhancedOperation = self.localEnhancedOperation(
                    requestId: requestId,
                    phase: "failed",
                    canCancel: false,
                    message: "增强启动失败",
                    error: message
                )
                self.codexEnhancedLaunchError = message
            }
        }
    }

    private func updateCodexEnhancedPresentation(for operation: ManageEnhancedLaunchOperation) {
        switch operation.phase {
        case "failed":
            codexEnhancedLaunchError = operation.error ?? operation.message
        case "cancelled":
            codexEnhancedLaunchError = nil
        default:
            codexEnhancedLaunchError = nil
        }
    }

    private func finishCodexEnhancedOperation(_ operation: ManageEnhancedLaunchOperation) async {
        updateCodexEnhancedPresentation(for: operation)
        if operation.phase == "ready" {
            actionFeedback = ActionFeedback(message: "增强启动已完成")
            _ = await loadSection(.codex, force: true)
        }
    }

    private func localEnhancedOperation(
        requestId: String,
        phase: String,
        canCancel: Bool,
        message: String,
        error: String? = nil,
        recovery: String? = nil
    ) -> ManageEnhancedLaunchOperation {
        let now = currentTimeMilliseconds
        return ManageEnhancedLaunchOperation(
            requestId: requestId,
            phase: phase,
            startedAtMs: codexEnhancedOperation?.startedAtMs ?? now,
            updatedAtMs: now,
            canCancel: canCancel,
            message: message,
            error: error,
            recovery: recovery
        )
    }

    private func stopCodexEnhancedClientTasks() {
        codexEnhancedWaitTask?.cancel()
        codexEnhancedWaitTask = nil
        codexEnhancedMonitorTask?.cancel()
        codexEnhancedMonitorTask = nil
        codexEnhancedMonitorRequestId = nil
        codexEnhancedLegacyTask?.cancel()
        codexEnhancedLegacyTask = nil
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
        await performManagementAction(section: .accountPool) {
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
        await performManagementAction(section: .accountPool) {
            self.sub2ApiAdmin = try await self.apiClient.disconnectSub2ApiAdmin()
            self.invalidateSub2ApiAccountPool()
            return "已断开 Sub2API 账号池"
        }
    }

    /// Optimistically changes one Sub2API account's scheduling participation.
    /// The daemon owns the upstream admin credential; this model only keeps a
    /// short-lived local override until the next pool refresh confirms it.
    @discardableResult
    func toggleSub2ApiAccountSchedulable(
        accountID: Int64,
        schedulable: Bool
    ) async -> Bool {
        guard accountID > 0 else {
            let message = "账号标识无效。"
            sub2ApiAccountPoolError = message
            sectionErrors[.accountPool] = message
            return false
        }
        guard !sub2ApiAccountPoolMutationIDs.contains(accountID) else {
            return false
        }
        guard let account = sub2ApiAccountPool?.accounts.first(where: { $0.id == accountID }) else {
            let message = "找不到该账号池账号。"
            sub2ApiAccountPoolError = message
            sectionErrors[.accountPool] = message
            return false
        }

        if fixtureStatus != nil {
            replaceSub2ApiAccountSchedulable(accountID: accountID, schedulable: schedulable)
            actionFeedback = ActionFeedback(
                message: schedulable ? "预览模式：已开启账号调度" : "预览模式：已暂停账号调度"
            )
            return true
        }
        guard sub2ApiAdmin?.configured == true else {
            let message = "尚未连接 Sub2API 账号池。"
            sub2ApiAccountPoolError = message
            sectionErrors[.accountPool] = message
            return false
        }

        let generation = (sub2ApiAccountPoolMutationGenerations[accountID] ?? 0) &+ 1
        sub2ApiAccountPoolMutationGenerations[accountID] = generation
        sub2ApiAccountPoolMutations[accountID] = Sub2ApiAccountPoolMutation(
            generation: generation,
            previousSchedulable: account.schedulable,
            requestedSchedulable: schedulable,
            awaitingRefreshConfirmation: false
        )
        sub2ApiAccountPoolMutationIDs.insert(accountID)
        sub2ApiAccountPoolError = nil
        replaceSub2ApiAccountSchedulable(accountID: accountID, schedulable: schedulable)

        do {
            let response = try await apiClient.setSub2ApiAccountSchedulable(
                accountID: accountID,
                schedulable: schedulable
            )
            guard isCurrentSub2ApiAccountMutation(accountID: accountID, generation: generation) else {
                return false
            }
            guard response.ok,
                  response.accountId == accountID,
                  response.schedulable == schedulable
            else {
                throw APIClientError.invalidResponse
            }

            // Keep the local value until a later pool response confirms the
            // server state, so an in-flight stale refresh cannot undo success.
            if var mutation = sub2ApiAccountPoolMutations[accountID] {
                mutation.awaitingRefreshConfirmation = true
                sub2ApiAccountPoolMutations[accountID] = mutation
            }
            sub2ApiAccountPoolMutationIDs.remove(accountID)
            sub2ApiAccountPoolError = nil
            sectionErrors[.accountPool] = nil
            managementOperationError = nil
            actionFeedback = ActionFeedback(
                message: schedulable ? "账号已开启调度" : "账号已暂停调度"
            )
            return true
        } catch {
            guard isCurrentSub2ApiAccountMutation(accountID: accountID, generation: generation) else {
                return false
            }
            replaceSub2ApiAccountSchedulable(
                accountID: accountID,
                schedulable: account.schedulable
            )
            sub2ApiAccountPoolMutations.removeValue(forKey: accountID)
            sub2ApiAccountPoolMutationIDs.remove(accountID)
            let message = userFacingMessage(for: error)
            sub2ApiAccountPoolError = message
            sectionErrors[.accountPool] = message
            managementOperationError = message
            return false
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
            let pool = poolApplyingSub2ApiAccountMutationOverrides(response.pool)
            sub2ApiAccountPool = pool
            if let channel = gatewayProviderChannel,
               let updated = pool.accounts.first(where: { $0.id == channel.id })
            {
                gatewayProviderChannel = updated
            }
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
        sub2ApiAccountPoolMutationIDs.removeAll()
        sub2ApiAccountPoolMutationGenerations.removeAll()
        sub2ApiAccountPoolMutations.removeAll()
    }

    private func isCurrentSub2ApiAccountMutation(
        accountID: Int64,
        generation: UInt64
    ) -> Bool {
        sub2ApiAccountPoolMutationGenerations[accountID] == generation
    }

    private func replaceSub2ApiAccountSchedulable(
        accountID: Int64,
        schedulable: Bool
    ) {
        if let pool = sub2ApiAccountPool {
            let accounts = pool.accounts.map { account in
                account.id == accountID
                    ? accountWithSchedulable(account, schedulable: schedulable)
                    : account
            }
            sub2ApiAccountPool = ManageSub2ApiAccountPoolResponse.Pool(
                source: pool.source,
                fetchedAtMs: pool.fetchedAtMs,
                accounts: accounts,
                warnings: pool.warnings
            )
        }
        if let channel = gatewayProviderChannel, channel.id == accountID {
            gatewayProviderChannel = accountWithSchedulable(channel, schedulable: schedulable)
        }
    }

    private func poolApplyingSub2ApiAccountMutationOverrides(
        _ pool: ManageSub2ApiAccountPoolResponse.Pool
    ) -> ManageSub2ApiAccountPoolResponse.Pool {
        guard !sub2ApiAccountPoolMutations.isEmpty else { return pool }
        var confirmedIDs: [Int64] = []
        let accounts = pool.accounts.map { account in
            guard let mutation = sub2ApiAccountPoolMutations[account.id] else {
                return account
            }
            if mutation.awaitingRefreshConfirmation,
               account.schedulable == mutation.requestedSchedulable
            {
                confirmedIDs.append(account.id)
            }
            return accountWithSchedulable(
                account,
                schedulable: mutation.requestedSchedulable
            )
        }
        for accountID in confirmedIDs {
            sub2ApiAccountPoolMutations.removeValue(forKey: accountID)
        }
        return ManageSub2ApiAccountPoolResponse.Pool(
            source: pool.source,
            fetchedAtMs: pool.fetchedAtMs,
            accounts: accounts,
            warnings: pool.warnings
        )
    }

    private func accountWithSchedulable(
        _ account: ManageSub2ApiAccountPoolResponse.Account,
        schedulable: Bool
    ) -> ManageSub2ApiAccountPoolResponse.Account {
        ManageSub2ApiAccountPoolResponse.Account(
            id: account.id,
            name: account.name,
            siteUrl: account.siteUrl,
            siteName: account.siteName,
            platform: account.platform,
            accountType: account.accountType,
            status: account.status,
            schedulable: schedulable,
            localRateMultiplier: account.localRateMultiplier,
            upstreamBilling: account.upstreamBilling,
            upstreamBalance: account.upstreamBalance
        )
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
                    todayCost: nil,
                    todayActualCost: nil,
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

    /// Refreshes the usage snapshot used by the menu-bar dashboard. The first
    /// enabled provider with a saved key matches the gateway quota dock's
    /// default selection. A failed or unsupported probe is kept as nil so the
    /// dashboard can use its local Sub2API-compatible estimate.
    func refreshGatewayProviderUsage() async {
        guard fixtureStatus == nil else {
            gatewayProviderUsage = nil
            gatewayProviderChannel = nil
            return
        }
        let currentGateway: ManageGateway?
        if let gateway {
            currentGateway = gateway
        } else {
            currentGateway = try? await apiClient.gateway()
        }
        guard let provider = currentGateway?.providers.first(where: { $0.enabled && $0.secretSet }) else {
            gatewayProviderUsage = nil
            gatewayProviderChannel = nil
            return
        }
        gatewayProviderUsage = try? await apiClient.fetchGatewayProviderUsage(
            providerName: provider.name
        )
        guard !Task.isCancelled else { return }

        // Provider usage is the total balance. Resolve the latest account
        // separately so the dashboard can show the actual channel balance.
        gatewayProviderChannel = nil
        if let recent = try? await apiClient.fetchGatewayProviderRecentAccount(
            providerName: provider.name
        ), let accountID = recent.account?.accountId {
            await refreshSub2ApiAccountPool()
            // The quota dock may already be refreshing the shared account
            // pool. Wait for that request to publish its result before
            // resolving the dashboard's current channel, otherwise the
            // overview can briefly render a missing multiplier.
            while sub2ApiAccountPoolLoading, !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(50))
            }
            guard !Task.isCancelled else { return }
            gatewayProviderChannel = sub2ApiAccountPool?.accounts.first {
                $0.id == accountID
            }
        }
    }

    func fetchGatewayProviderRecentAccount(
        providerName: String
    ) async throws -> ManageProviderRecentAccountResponse {
        guard fixtureStatus == nil else {
            return ManageProviderRecentAccountResponse(
                ok: true,
                providerName: providerName,
                account: nil
            )
        }
        return try await apiClient.fetchGatewayProviderRecentAccount(
            providerName: providerName
        )
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
        guard fixtureStatus == nil,
              !daemonRecoveryInProgress,
              !daemonTransitionInProgress
        else { return }
        daemonRecoveryInProgress = true
        defer { daemonRecoveryInProgress = false }
        daemonRecoveryError = nil
        do {
            _ = try await startDaemonAndWait(
                attemptLimit: 1,
                countsTowardAutomaticLimit: false
            )
            resetAutomaticDaemonStartupAttempts()
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

    /// Explicitly replaces another installation's still-active management
    /// lease. This is only called after the user confirms the takeover in
    /// Settings; background refresh never enters this path.
    @discardableResult
    func takeOverDaemonManagement(
        confirming confirmation: DaemonManagementConfirmation
    ) async -> Bool {
        guard fixtureStatus == nil else { return false }
        guard !daemonLeaseTakeoverInProgress,
              !managementCredentialRotationInProgress,
              !daemonTransitionInProgress
        else { return false }
        guard daemonLeaseConflict,
              let observedLifecycle = lifecycle,
              lifecycleMatchesConfirmation(observedLifecycle, confirmation)
        else {
            managementOperationError = "确认后后台服务管理租约已变化，请刷新状态并重新确认。"
            return false
        }

        lifecycleObservationGeneration &+= 1
        daemonLeaseTakeoverInProgress = true
        managementOperationError = nil
        daemonManagementFeedback = "正在核验后台服务身份并接管管理权…"
        stopLifecycleLeaseHeartbeat()
        defer {
            lifecycleObservationGeneration &+= 1
            daemonLeaseTakeoverInProgress = false
            if ownsDaemonLease {
                startLifecycleLeaseHeartbeat()
            }
        }

        let requestId = UUID().uuidString
        do {
            let identity = try await verifiedDaemonIdentity(
                for: observedLifecycle,
                forceRefresh: true
            )
            let response = try await apiClient.takeOverLifecycleLease(
                installationId: installationId,
                daemonInstanceId: confirmation.daemonInstanceId,
                expectedLeaseGeneration: confirmation.leaseGeneration,
                expectedManagementTokenGeneration: confirmation.managementTokenGeneration,
                requestId: requestId,
                daemonIdentity: identity
            )
            let refreshed = try await validatedLifecycle(
                after: response,
                requestId: requestId,
                expectedInstanceId: observedLifecycle.service.instanceId
            )
            lifecycle = refreshed
            daemonManagementFeedback = "已接管后台服务，其他安装的管理权限和旧凭据已失效。"
            actionFeedback = ActionFeedback(message: "已接管后台服务")
            return true
        } catch {
            daemonManagementFeedback = nil
            managementOperationError = "接管后台服务失败：\(userFacingMessage(for: error))"
            return false
        }
    }

    /// Rotates the shared management credential while retaining the current
    /// installation's lease. The daemon never returns the new secret; the
    /// next lifecycle read discovers it from the protected control file.
    @discardableResult
    func rotateManagementCredential(
        confirming confirmation: DaemonManagementConfirmation
    ) async -> Bool {
        guard fixtureStatus == nil else { return false }
        guard !daemonLeaseTakeoverInProgress,
              !managementCredentialRotationInProgress,
              !daemonTransitionInProgress
        else { return false }
        guard ownsDaemonLease,
              let observedLifecycle = lifecycle,
              lifecycleMatchesConfirmation(observedLifecycle, confirmation)
        else {
            managementOperationError = "确认后后台服务管理状态已变化，请刷新状态并重新确认。"
            return false
        }

        lifecycleObservationGeneration &+= 1
        managementCredentialRotationInProgress = true
        managementOperationError = nil
        daemonManagementFeedback = "正在重新生成管理凭据…"
        stopLifecycleLeaseHeartbeat()
        defer {
            lifecycleObservationGeneration &+= 1
            managementCredentialRotationInProgress = false
            if ownsDaemonLease {
                startLifecycleLeaseHeartbeat()
            }
        }

        let requestId = UUID().uuidString
        do {
            let response = try await apiClient.rotateManagementCredential(
                installationId: installationId,
                daemonInstanceId: confirmation.daemonInstanceId,
                leaseGeneration: confirmation.leaseGeneration,
                expectedManagementTokenGeneration: confirmation.managementTokenGeneration,
                requestId: requestId
            )
            let refreshed = try await validatedLifecycle(
                after: response,
                requestId: requestId,
                expectedInstanceId: observedLifecycle.service.instanceId
            )
            lifecycle = refreshed
            daemonManagementFeedback = "管理凭据已重新生成，旧凭据已立即失效。"
            actionFeedback = ActionFeedback(message: "管理凭据已重新生成")
            return true
        } catch {
            daemonManagementFeedback = nil
            managementOperationError = "重新生成管理凭据失败：\(userFacingMessage(for: error))"
            return false
        }
    }

    private func validatedLifecycle(
        after response: ManageLifecycleCredentialMutationResponse,
        requestId: String,
        expectedInstanceId: String
    ) async throws -> ManageLifecycle {
        guard response.ok, response.requestId == requestId else {
            throw APIClientError.operationFailed("后台服务没有确认管理凭据变更。")
        }
        let refreshed = try await apiClient.lifecycle()
        guard refreshed.service.instanceId == expectedInstanceId else {
            throw APIClientError.operationFailed("后台服务实例已变化，请刷新状态后重试。")
        }
        guard refreshed.management.canControl,
              refreshed.management.installationId == installationId,
              refreshed.management.managementTokenGeneration
                == response.managementTokenGeneration
        else {
            throw APIClientError.operationFailed("后台服务管理状态校验失败，请刷新后重试。")
        }
        return refreshed
    }

    func restartDaemon() async {
        guard fixtureStatus == nil,
              !daemonTransitionInProgress,
              ownsDaemonLease,
              let lifecycle
        else {
            managementOperationError = "当前界面没有后台服务管理权。"
            return
        }
        daemonTransitionInProgress = true
        daemonTransitionGeneration &+= 1
        stopLifecycleLeaseHeartbeat()
        defer {
            daemonTransitionInProgress = false
            if ownsDaemonLease {
                startLifecycleLeaseHeartbeat()
            }
        }

        let previousInstanceId = lifecycle.service.instanceId
        managementOperationError = nil
        actionFeedback = ActionFeedback(message: "正在安全重启本地服务")
        dashboardState = .starting
        serviceStatus = .checking

        do {
            try await requestLifecycleRestart(lifecycle)
            guard let replacement = try await waitForDaemonReplacement(
                previousInstanceId: previousInstanceId
            ) else {
                actionFeedback = nil
                managementOperationError = "后台服务未能在预期时间内恢复。"
                return
            }
            self.lifecycle = try await reclaimLifecycle(replacement)
            markDaemonReachable()
            actionFeedback = ActionFeedback(message: "后台服务已安全重启")
        } catch {
            actionFeedback = nil
            managementOperationError = userFacingMessage(for: error)
        }
    }

    private func requestLifecycleRestart(_ lifecycle: ManageLifecycle) async throws {
        guard let leaseGeneration = lifecycle.management.leaseGeneration else {
            throw APIClientError.operationFailed("后台服务管理租约已失效。")
        }
        let response = try await apiClient.restartLifecycle(
            installationId: installationId,
            daemonInstanceId: lifecycle.service.instanceId,
            leaseGeneration: leaseGeneration
        )
        guard response.ok else {
            throw APIClientError.operationFailed("后台服务没有接受重启请求。")
        }
    }

    private func reclaimLifecycle(_ lifecycle: ManageLifecycle) async throws -> ManageLifecycle {
        try await claimLifecycleForRecoveryWait(lifecycle) ?? lifecycle
    }

    private func markDaemonReachable() {
        serviceStatus = .available
        dashboardState = dashboard == nil ? .loading : .stale
        lastCheckedAt = Date()
    }

    private func waitForDaemonReplacement(
        previousInstanceId: String,
        expectedBuild: Int? = nil,
        expectedExecutable: String? = nil
    ) async throws -> ManageLifecycle? {
        var consecutiveMatches = 0
        var stableService: ManageLifecycle.Service?
        for attempt in 0..<daemonReplacementAttemptLimit {
            try Task.checkCancellation()
            if let current = try await lifecycleForRecoveryWait(), Self.daemonReplacementMatches(
                   current,
                   previousInstanceId: previousInstanceId,
                   expectedBuild: expectedBuild,
                   expectedExecutable: expectedExecutable
               )
            {
                if stableService == current.service {
                    consecutiveMatches += 1
                } else {
                    stableService = current.service
                    consecutiveMatches = 1
                }
                if consecutiveMatches >= daemonReplacementStableProbeCount {
                    return current
                }
            } else {
                consecutiveMatches = 0
                stableService = nil
            }
            guard attempt < daemonReplacementAttemptLimit - 1 else {
                try Task.checkCancellation()
                return nil
            }
            try await Task.sleep(for: daemonReplacementPollDelay)
        }
        try Task.checkCancellation()
        return nil
    }

    /// Recovery polling treats ordinary daemon/API failures as a missing
    /// observation, but cancellation must escape immediately. URLSession can
    /// surface task cancellation as either CancellationError or
    /// URLError(.cancelled), so check the task after every request as well.
    private func lifecycleForRecoveryWait() async throws -> ManageLifecycle? {
        try Task.checkCancellation()
        do {
            let lifecycle = try await apiClient.lifecycle()
            try Task.checkCancellation()
            return lifecycle
        } catch is CancellationError {
            throw CancellationError()
        } catch {
            try Task.checkCancellation()
            return nil
        }
    }

    private func claimLifecycleForRecoveryWait(
        _ lifecycle: ManageLifecycle
    ) async throws -> ManageLifecycle? {
        try Task.checkCancellation()
        do {
            let claimed = try await apiClient.claimLifecycleLease(
                installationId: installationId,
                daemonInstanceId: lifecycle.service.instanceId,
                daemonIdentity: try await verifiedDaemonIdentity(
                    for: lifecycle,
                    forceRefresh: true
                )
            )
            try Task.checkCancellation()
            return claimed
        } catch is CancellationError {
            throw CancellationError()
        } catch {
            try Task.checkCancellation()
            return nil
        }
    }

    private func verifiedDaemonIdentity(
        for lifecycle: ManageLifecycle,
        forceRefresh: Bool = false
    ) async throws -> ManageDaemonIdentity {
        if !forceRefresh,
           let cachedDaemonIdentity,
           cachedDaemonIdentityInstanceId == lifecycle.service.instanceId,
           cachedDaemonIdentity.pid == lifecycle.service.pid,
           cachedDaemonIdentity.startedAtMs == lifecycle.service.startedAtMs,
           cachedDaemonIdentity.executable == lifecycle.executable,
           cachedDaemonIdentity.bind == lifecycle.bind,
           lifecycle.executableSha256.map({
               $0.caseInsensitiveCompare(cachedDaemonIdentity.executableSha256) == .orderedSame
           }) ?? true
        {
            return cachedDaemonIdentity
        }
        do {
            let verified = try await daemonLauncher.verifiedDaemonIdentity(for: lifecycle)
            cachedDaemonIdentity = verified
            cachedDaemonIdentityInstanceId = lifecycle.service.instanceId
            return verified
        } catch {
            clearCachedDaemonIdentity()
            throw error
        }
    }

    private func clearCachedDaemonIdentity() {
        cachedDaemonIdentity = nil
        cachedDaemonIdentityInstanceId = nil
    }

    nonisolated static func daemonReplacementMatches(
        _ lifecycle: ManageLifecycle,
        previousInstanceId: String,
        expectedBuild: Int?,
        expectedExecutable: String?
    ) -> Bool {
        guard lifecycle.service.ready,
              lifecycle.service.instanceId != previousInstanceId
        else {
            return false
        }
        if let expectedBuild, lifecycle.runtime.buildNumber != expectedBuild {
            return false
        }
        if let expectedExecutable,
           URL(fileURLWithPath: lifecycle.executable).standardizedFileURL
                .resolvingSymlinksInPath()
            != URL(fileURLWithPath: expectedExecutable).standardizedFileURL
                .resolvingSymlinksInPath()
        {
            return false
        }
        return true
    }

    func checkForUpdates() async {
        await loadComponentUpdates(silent: false)
    }

    private func loadComponentUpdates(silent: Bool) async {
        guard fixtureStatus == nil else { return }
        if !silent {
            guard updateCheckState != .checking else { return }
            updateCheckState = .checking
        }
        do {
            let manifest = try await updateManifestLoader(currentUIVersion)
            applyUpdateManifest(manifest)
            updateCheckState = .checked
        } catch {
            if !silent {
                updateCheckState = .failed(userFacingMessage(for: error))
            }
        }
    }

    private func applyUpdateManifest(_ manifest: UpdateManifest) {
        let previousUIVersion = availableUIUpdate?.version
        let previousDaemonBuild = availableDaemonUpdate?.build
        let uiUpdate = manifest.ui.isNewer(
            thanVersion: currentUIVersion,
            build: currentUIBuild
        ) ? manifest.ui : nil

        let daemonUpdate = lifecycle.flatMap { lifecycle in
            manifest.daemon.flatMap { release in
                release.isNewer(
                    thanVersion: lifecycle.runtime.productVersion,
                    build: lifecycle.runtime.buildNumber
                ) ? release : nil
            }
        }

        availableUIUpdate = uiUpdate
        availableDaemonUpdate = daemonUpdate

        if previousUIVersion != uiUpdate?.version
            || previousDaemonBuild != daemonUpdate?.build
        {
            unifiedUpdateNoticeDismissed = false
        }
    }

    /// Silently checks GitHub for independently published UI and daemon
    /// releases once per app launch. The delay keeps this away from startup.
    func scheduleStartupUpdateCheck(delay: Duration = .seconds(5)) {
        guard fixtureStatus == nil, !startupUpdateCheckScheduled else { return }
        startupUpdateCheckScheduled = true
        Task { [weak self] in
            try? await Task.sleep(for: delay)
            guard let self, !Task.isCancelled else { return }
            await self.loadComponentUpdates(silent: true)
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
            if account.platform == "telegram" {
                telegramProjectGroupAccounts.removeAll { $0.accountId == account.accountId }
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

    func telegramProjectGroups(for accountId: String) -> [ManageTelegramProjectGroup] {
        telegramProjectGroupAccounts.first(where: { $0.accountId == accountId })?.projectGroups ?? []
    }

    @discardableResult
    func saveTelegramProjectGroups(
        accountId: String,
        projectGroups: [ManageTelegramProjectGroup]
    ) async -> Bool {
        if fixtureStatus != nil {
            let next = ManageTelegramProjectGroupAccount(accountId: accountId, projectGroups: projectGroups)
            telegramProjectGroupAccounts.removeAll { $0.accountId == accountId }
            telegramProjectGroupAccounts.append(next)
            actionFeedback = ActionFeedback(message: "项目群配置已保存；重启后台服务后生效")
            return true
        }
        do {
            let response = try await apiClient.updateTelegramProjectGroups(
                accountId: accountId,
                projectGroups: projectGroups
            )
            telegramProjectGroupAccounts.removeAll { $0.accountId == accountId }
            telegramProjectGroupAccounts.append(
                ManageTelegramProjectGroupAccount(
                    accountId: response.accountId,
                    projectGroups: response.projectGroups
                )
            )
            accountOperationError = nil
            actionFeedback = ActionFeedback(message: "项目群配置已保存；重启后台服务后生效")
            return true
        } catch {
            accountOperationError = userFacingMessage(for: error)
            return false
        }
    }

    @discardableResult
    func syncTelegramTopics(accountId: String, chatId: String) async -> Bool {
        if fixtureStatus != nil {
            actionFeedback = ActionFeedback(message: "预览模式：已模拟同步 Telegram 会话 Topic")
            return true
        }
        do {
            let response = try await apiClient.syncTelegramTopics(
                accountId: accountId,
                chatId: chatId
            )
            accountOperationError = nil
            let details = response.items.compactMap { item -> String? in
                guard let reason = item.error?.trimmingCharacters(in: .whitespacesAndNewlines),
                      !reason.isEmpty else { return nil }
                return "「\(item.title)」：\(reason)"
            }
            let detailText: String
            if details.isEmpty {
                detailText = ""
            } else {
                let visible = details.prefix(20).joined(separator: "\n")
                let remaining = details.count - min(details.count, 20)
                detailText = remaining > 0
                    ? "\n\(visible)\n还有 \(remaining) 个会话未展开"
                    : "\n\(visible)"
            }
            actionFeedback = ActionFeedback(
                message: "Topic 同步完成：共 \(response.total) 个，创建 \(response.created) 个，跳过 \(response.skipped) 个，失败 \(response.failed) 个\(detailText)"
            )
            return true
        } catch {
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
                avatarData: nil,
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

    private func forwardOnboardingErrors<T: Sendable>(
        _ operation: @Sendable () async throws -> T
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
            avatarData: nil,
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
                service: "mochiport",
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
                service: "mochiport",
                apiMajor: 1,
                ready: true,
                instanceId: "preview-instance",
                pid: 0,
                startedAtMs: 0
            ),
            executable: "/Preview/MochiPort",
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
                avatarData: nil,
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
                avatarData: nil,
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
                avatarData: nil,
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
                avatarData: nil,
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
        if let localizedError = error as? LocalizedError,
           let description = localizedError.errorDescription,
           !description.isEmpty {
            return description
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
    case unavailable(String)

    var title: String {
        switch self {
        case .checking: "检查中"
        case .available: "运行正常"
        case .unavailable: "不可用"
        }
    }

    var detail: String {
        switch self {
        case .checking: "正在连接本地服务"
        case .available: "本地服务已就绪"
        case let .unavailable(message): message
        }
    }

    var symbol: String {
        switch self {
        case .checking: "arrow.2.circlepath"
        case .available: "checkmark.circle.fill"
        case .unavailable: "exclamationmark.triangle.fill"
        }
    }

    var tint: StatusTint {
        switch self {
        case .checking: .secondary
        case .available: .positive
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
    case gateway
    case accountPool
    case messaging
    case sessions
    case requestLogs

    var id: String { rawValue }

    var title: String {
        switch self {
        case .overview: "概览"
        case .codex: "Codex 接入"
        case .gateway: "AI 网关"
        case .accountPool: "账号池"
        case .messaging: "消息渠道"
        case .sessions: "会话"
        case .requestLogs: "请求日志"
        }
    }

    var symbol: String {
        switch self {
        case .overview: "rectangle.grid.1x2"
        case .codex: "chevron.left.forwardslash.chevron.right"
        case .gateway: "point.3.connected.trianglepath.dotted"
        case .accountPool: "person.3.sequence"
        case .messaging: "bubble.left.and.bubble.right"
        case .sessions: "clock.arrow.circlepath"
        case .requestLogs: "list.bullet.rectangle"
        }
    }

    var group: AppSectionGroup {
        switch self {
        case .overview: .overview
        case .codex, .gateway, .accountPool: .configuration
        case .messaging, .sessions: .configuration
        case .requestLogs: .diagnostics
        }
    }
}

enum AppSectionGroup: String, CaseIterable, Identifiable {
    case overview
    case configuration
    case diagnostics

    var id: String { rawValue }

    var title: String? {
        switch self {
        case .overview: nil
        case .configuration: "配置"
        case .diagnostics: "诊断"
        }
    }

    var sections: [AppSection] {
        AppSection.allCases.filter { $0.group == self }
    }
}
