import CryptoKit
import Darwin
import Foundation

enum MochiPortStorage {
    static func currentDirectory(
        environment: [String: String] = ProcessInfo.processInfo.environment,
        applicationSupport: URL,
        fileManager: FileManager = .default
    ) -> URL? {
        if let path = environment["MOCHIPORT_HOME"], !path.isEmpty {
            return URL(fileURLWithPath: path, isDirectory: true)
        }

        let directory = applicationSupport.appendingPathComponent(
            "MochiPort",
            isDirectory: true
        )
        var isDirectory: ObjCBool = false
        guard fileManager.fileExists(atPath: directory.path, isDirectory: &isDirectory),
              isDirectory.boolValue
        else {
            return nil
        }
        return directory
    }

    static func dataDirectory(
        environment: [String: String] = ProcessInfo.processInfo.environment,
        applicationSupport: URL,
        fileManager: FileManager = .default
    ) -> URL {
        currentDirectory(
            environment: environment,
            applicationSupport: applicationSupport,
            fileManager: fileManager
        ) ?? applicationSupport.appendingPathComponent("MochiPort", isDirectory: true)
    }
}

enum DaemonLaunchError: LocalizedError, Equatable {
    case helperMissing
    case helperNotExecutable
    case launchdLabelInvalid(String)
    case runtimeBuildIdentifierInvalid(String)
    case runtimeDirectoryUnavailable
    case runtimeStageFailed
    case runtimeNotExecutable
    case runtimeVersionMismatch(expected: String, actual: String?)
    case runtimeContentsMismatch(buildIdentifier: String)
    case currentRuntimeInvalid
    case launchAgentDirectoryUnavailable
    case launchAgentWriteFailed
    case loadedAgentMismatch(expected: String, actual: String?)
    case loadedAgentUntrusted(String?)
    case upgradeUnavailable
    case upgradePreparationStale(expected: String?, actual: String?)
    case daemonStillRunning
    case runtimeActivationFailed
    case runtimeRollbackFailed
    case launchctlFailed(String)

    var errorDescription: String? {
        switch self {
        case .helperMissing:
            return "应用内未找到后台服务。请重新安装 MochiPort。"
        case .helperNotExecutable:
            return "应用内的后台服务不可执行。请重新安装 MochiPort。"
        case let .launchdLabelInvalid(label):
            return "后台服务测试标识无效：\(label)"
        case let .runtimeBuildIdentifierInvalid(identifier):
            return "后台服务构建标识无效：\(identifier)"
        case .runtimeDirectoryUnavailable:
            return "无法创建后台服务版本目录。"
        case .runtimeStageFailed:
            return "无法准备后台服务运行版本。"
        case .runtimeNotExecutable:
            return "准备好的后台服务不可执行。"
        case let .runtimeVersionMismatch(expected, actual):
            return "后台服务构建不匹配（应为 \(expected)，实际为 \(actual ?? "未知")）。"
        case let .runtimeContentsMismatch(buildIdentifier):
            return "后台服务构建 \(buildIdentifier) 的文件内容与应用内版本不一致。请使用新的构建号重新安装，应用不会覆盖正在使用的同构建运行版本。"
        case .currentRuntimeInvalid:
            return "现有后台服务运行版本无效。请在合适的时间手动修复或重新安装 MochiPort；应用不会自动替换它。"
        case .launchAgentDirectoryUnavailable:
            return "无法访问当前用户的后台服务目录。"
        case .launchAgentWriteFailed:
            return "无法保存后台服务启动配置。"
        case let .loadedAgentMismatch(expected, actual):
            let actualDescription = actual.map { "当前为 \($0)" } ?? "当前路径未知"
            return "后台服务启动配置指向了其他版本（应为 \(expected)，\(actualDescription)）。请重新安装 MochiPort 后重试。"
        case let .loadedAgentUntrusted(actual):
            let detail = actual.map { "（\($0)）" } ?? ""
            return "当前后台进程无法确认为 MochiPort 管理的运行版本\(detail)，已取消操作。"
        case .upgradeUnavailable:
            return "当前后台服务不支持自动版本切换。"
        case let .upgradePreparationStale(expected, actual):
            let expectedDescription = expected ?? "未知"
            let actualDescription = actual ?? "未知"
            return "后台服务版本在切换前发生变化（准备时为 \(expectedDescription)，当前为 \(actualDescription)），已取消操作。"
        case .daemonStillRunning:
            return "后台服务仍在运行，必须先完成安全排空后才能切换版本。"
        case .runtimeActivationFailed:
            return "无法切换后台服务运行版本。"
        case .runtimeRollbackFailed:
            return "后台服务版本切换失败，且旧版本恢复失败，请立即检查后台服务状态。"
        case let .launchctlFailed(detail):
            return detail.isEmpty ? "无法启动后台服务。" : "无法启动后台服务：\(detail)"
        }
    }
}

struct DaemonLaunchConfiguration: Equatable {
    static let label = "io.github.mps233.mochiport.daemon"
    fileprivate static let skipDesktopIntegrationEnvironments = [
        "MOCHIPORT_SKIP_DESKTOP_INTEGRATION",
    ]
#if DEBUG
    private static let testLabelPrefix = "io.github.mps233.mochiport.tests."
#endif

    let launchdLabel: String
    private let skipsDesktopIntegration: Bool
    let helperURL: URL
    let configURL: URL
    let launchAgentURL: URL
    let logURL: URL
    let homeURL: URL
    let buildIdentifier: String?

    init(
        helperURL: URL,
        configURL: URL,
        launchAgentURL: URL,
        logURL: URL,
        homeURL: URL,
        buildIdentifier: String?
    ) {
        self.init(
            launchdLabel: Self.label,
            skipsDesktopIntegration: false,
            helperURL: helperURL,
            configURL: configURL,
            launchAgentURL: launchAgentURL,
            logURL: logURL,
            homeURL: homeURL,
            buildIdentifier: buildIdentifier
        )
    }

#if DEBUG
    init(
        testLaunchdLabel: String,
        helperURL: URL,
        configURL: URL,
        launchAgentURL: URL,
        logURL: URL,
        homeURL: URL,
        buildIdentifier: String?
    ) throws {
        guard Self.isValidTestLaunchdLabel(testLaunchdLabel) else {
            throw DaemonLaunchError.launchdLabelInvalid(testLaunchdLabel)
        }
        self.init(
            launchdLabel: testLaunchdLabel,
            skipsDesktopIntegration: true,
            helperURL: helperURL,
            configURL: configURL,
            launchAgentURL: launchAgentURL,
            logURL: logURL,
            homeURL: homeURL,
            buildIdentifier: buildIdentifier
        )
    }
#endif

    private init(
        launchdLabel: String,
        skipsDesktopIntegration: Bool,
        helperURL: URL,
        configURL: URL,
        launchAgentURL: URL,
        logURL: URL,
        homeURL: URL,
        buildIdentifier: String?
    ) {
        self.launchdLabel = launchdLabel
        self.skipsDesktopIntegration = skipsDesktopIntegration
        self.helperURL = helperURL
        self.configURL = configURL
        self.launchAgentURL = launchAgentURL
        self.logURL = logURL
        self.homeURL = homeURL
        self.buildIdentifier = buildIdentifier
    }

#if DEBUG
    private static func isValidTestLaunchdLabel(_ label: String) -> Bool {
        guard label.hasPrefix(testLabelPrefix),
              label != Self.label,
              label.utf8.count <= 128
        else {
            return false
        }
        let suffix = label.dropFirst(testLabelPrefix.count)
        let allowed = CharacterSet(
            charactersIn: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-"
        )
        let edgeAllowed = CharacterSet(
            charactersIn: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
        )
        guard !suffix.isEmpty,
              suffix.rangeOfCharacter(from: allowed.inverted) == nil,
              String(suffix.prefix(1)).rangeOfCharacter(from: edgeAllowed.inverted) == nil,
              String(suffix.suffix(1)).rangeOfCharacter(from: edgeAllowed.inverted) == nil,
              !suffix.contains("..")
        else {
            return false
        }
        return true
    }
#endif

    var launchdServiceTarget: String {
        "gui/\(getuid())/\(launchdLabel)"
    }

    fileprivate var desktopIntegrationEnvironmentValue: String? {
        skipsDesktopIntegration ? "1" : nil
    }

    static func current(
        bundleURL: URL = Bundle.main.bundleURL,
        environment: [String: String] = ProcessInfo.processInfo.environment,
        fileManager: FileManager = .default
    ) throws -> Self {
        let homeURL = environment["HOME"].map {
            URL(fileURLWithPath: $0, isDirectory: true)
        } ?? fileManager.homeDirectoryForCurrentUser
        let applicationSupport = homeURL.appendingPathComponent(
            "Library/Application Support",
            isDirectory: true
        )

        let dataDirectory = MochiPortStorage.dataDirectory(
            environment: environment,
            applicationSupport: applicationSupport,
            fileManager: fileManager
        )
        let helpers = bundleURL
            .appendingPathComponent("Contents", isDirectory: true)
            .appendingPathComponent("Helpers", isDirectory: true)
        let embeddedHelper = helpers.appendingPathComponent("mochiport-daemon")

        return Self(
            helperURL: embeddedHelper,
            configURL: dataDirectory.appendingPathComponent("config.toml"),
            launchAgentURL: homeURL
                .appendingPathComponent("Library/LaunchAgents", isDirectory: true)
                .appendingPathComponent("\(label).plist"),
            logURL: dataDirectory
                .appendingPathComponent("logs", isDirectory: true)
                .appendingPathComponent("mochiport-daemon-launchd.log"),
            homeURL: homeURL,
            buildIdentifier: Self.embeddedDaemonBuildIdentifier(bundleURL: bundleURL)
        )
    }

    private static func embeddedDaemonBuildIdentifier(bundleURL: URL) -> String? {
        guard let bundle = Bundle(url: bundleURL),
              let value = (bundle.object(forInfoDictionaryKey: "MochiPortDaemonBuild")
                  ?? bundle.object(forInfoDictionaryKey: "CFBundleVersion")) as? String
        else { return nil }
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    func resolvedBuildIdentifier() throws -> String {
        guard let buildIdentifier else { return "dev" }
        let trimmed = buildIdentifier.trimmingCharacters(in: .whitespacesAndNewlines)
        guard Self.isSafeRuntimeBuildIdentifier(trimmed)
        else {
            throw DaemonLaunchError.runtimeBuildIdentifierInvalid(buildIdentifier)
        }
        return trimmed
    }

    func activeRuntimeBuildIdentifier(fileManager: FileManager = .default) throws -> String {
        let active = configURL
            .deletingLastPathComponent()
            .appendingPathComponent("runtimes", isDirectory: true)
            .appendingPathComponent("current", isDirectory: true)
        var info = stat()
        guard Darwin.lstat(active.path, &info) == 0,
              info.st_mode & mode_t(S_IFMT) == mode_t(S_IFLNK)
        else {
            throw DaemonLaunchError.currentRuntimeInvalid
        }
        guard let target = try? fileManager.destinationOfSymbolicLink(atPath: active.path),
              Self.isSafeRuntimeBuildIdentifier(target)
        else {
            throw DaemonLaunchError.currentRuntimeInvalid
        }
        return target
    }

    static func isSafeRuntimeBuildIdentifier(_ value: String) -> Bool {
        let allowed = CharacterSet(
            charactersIn: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-"
        )
        return !value.isEmpty
            && value != "."
            && value != ".."
            && value.rangeOfCharacter(from: allowed.inverted) == nil
    }

    func stagedHelperURL() throws -> URL {
        let buildIdentifier = try resolvedBuildIdentifier()
        return configURL
            .deletingLastPathComponent()
            .appendingPathComponent("runtimes", isDirectory: true)
            .appendingPathComponent(buildIdentifier, isDirectory: true)
            .appendingPathComponent(helperURL.lastPathComponent)
    }

    func activeHelperURL() -> URL {
        configURL
            .deletingLastPathComponent()
            .appendingPathComponent("runtimes", isDirectory: true)
            .appendingPathComponent("current", isDirectory: true)
            .appendingPathComponent(helperURL.lastPathComponent)
    }

    func propertyListData() throws -> Data {
        let activeBuildIdentifier = try activeRuntimeBuildIdentifier()
        let activeHelperURL = activeHelperURL()
        var environment: [String: String] = [
            "HOME": homeURL.path,
            "PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
            "MOCHIPORT_HOME": configURL.deletingLastPathComponent().path,
            "MOCHIPORT_BUNDLE_BUILD": activeBuildIdentifier,
        ]
        if let value = desktopIntegrationEnvironmentValue {
            for key in Self.skipDesktopIntegrationEnvironments {
                environment[key] = value
            }
        }
        let propertyList: [String: Any] = [
            "Label": launchdLabel,
            "ProgramArguments": [
                activeHelperURL.path,
                "--config",
                configURL.path,
                "daemon",
            ],
            "EnvironmentVariables": environment,
            "RunAtLoad": true,
            "KeepAlive": true,
            "ProcessType": "Background",
            "ThrottleInterval": 5,
            "StandardOutPath": logURL.path,
            "StandardErrorPath": logURL.path,
        ]
        return try PropertyListSerialization.data(
            fromPropertyList: propertyList,
            format: .xml,
            options: 0
        )
    }
}

struct CommandResult: Equatable {
    let exitCode: Int32
    let output: String
}

enum DaemonLaunchOutcome: Equatable, Sendable {
    /// A verified LaunchAgent already owns a running daemon.  Callers must
    /// wait for its health endpoint rather than interrupting it.
    case alreadyRunning
    /// No LaunchAgent was registered, so the current helper was installed and
    /// handed to launchd for the first time.
    case bootstrapped
    /// A verified LaunchAgent was registered but had no running daemon.  This
    /// uses launchctl kickstart without -k and never replaces its runtime.
    case resumedStoppedService
}

/// A staged daemon helper that is ready to become the launchd runtime after
/// the caller has completed its own lifecycle drain. Staging never changes
/// the active `current` symlink or the loaded LaunchAgent.
struct DaemonUpgradePreparation: Equatable, Sendable {
    let targetBuildIdentifier: String
    let previousBuildIdentifier: String?
    let previousLaunchAgentData: Data?

    init(
        targetBuildIdentifier: String,
        previousBuildIdentifier: String?,
        previousLaunchAgentData: Data? = nil
    ) {
        self.targetBuildIdentifier = targetBuildIdentifier
        self.previousBuildIdentifier = previousBuildIdentifier
        self.previousLaunchAgentData = previousLaunchAgentData
    }

    var requiresActivation: Bool {
        previousBuildIdentifier != targetBuildIdentifier
    }
}

enum DaemonUpgradeOutcome: Equatable, Sendable {
    case alreadyCurrent(buildIdentifier: String)
    case activated(previousBuildIdentifier: String?, buildIdentifier: String)
}

/// Stable name for the coordinated GUI/daemon handoff API. Keep the original
/// preparation spelling as a source-compatible alias for existing callers.
typealias DaemonRuntimeUpgradePlan = DaemonUpgradePreparation

protocol DaemonLaunching: Sendable {
    func startIfNeeded() async throws -> DaemonLaunchOutcome
    func verifiedDaemonIdentity(for lifecycle: ManageLifecycle) async throws -> ManageDaemonIdentity
    func prepareDaemonUpgradeIfNeeded() async throws -> DaemonUpgradePreparation
    func activateDaemonUpgrade(_ preparation: DaemonUpgradePreparation) async throws -> DaemonUpgradeOutcome
    func rollbackDaemonUpgrade(_ preparation: DaemonUpgradePreparation) async throws
    func prepareRuntimeUpgrade() async throws -> DaemonRuntimeUpgradePlan
    func activateRuntimeUpgrade(_ plan: DaemonRuntimeUpgradePlan) async throws -> DaemonUpgradeOutcome
    func rollbackRuntimeUpgrade(_ plan: DaemonRuntimeUpgradePlan) async throws
}

extension DaemonLaunching {
    func verifiedDaemonIdentity(for lifecycle: ManageLifecycle) async throws -> ManageDaemonIdentity {
        ManageDaemonIdentity(
            pid: lifecycle.service.pid,
            startedAtMs: lifecycle.service.startedAtMs,
            executable: lifecycle.executable,
            executableSha256: lifecycle.executableSha256 ?? "",
            bind: lifecycle.bind
        )
    }

    func prepareDaemonUpgradeIfNeeded() async throws -> DaemonUpgradePreparation {
        throw DaemonLaunchError.upgradeUnavailable
    }

    func activateDaemonUpgrade(
        _ preparation: DaemonUpgradePreparation
    ) async throws -> DaemonUpgradeOutcome {
        throw DaemonLaunchError.upgradeUnavailable
    }

    func rollbackDaemonUpgrade(
        _ preparation: DaemonUpgradePreparation
    ) async throws {
        _ = preparation
        throw DaemonLaunchError.upgradeUnavailable
    }

    func prepareRuntimeUpgrade() async throws -> DaemonRuntimeUpgradePlan {
        try await prepareDaemonUpgradeIfNeeded()
    }

    func activateRuntimeUpgrade(
        _ plan: DaemonRuntimeUpgradePlan
    ) async throws -> DaemonUpgradeOutcome {
        try await activateDaemonUpgrade(plan)
    }

    func rollbackRuntimeUpgrade(_ plan: DaemonRuntimeUpgradePlan) async throws {
        try await rollbackDaemonUpgrade(plan)
    }
}

struct DaemonLauncher: DaemonLaunching, @unchecked Sendable {
    private let configurationLoader: @Sendable () throws -> DaemonLaunchConfiguration
    private let commandRunner: @Sendable (URL, [String]) throws -> CommandResult
    private let activeDaemonLocatorLoader: @Sendable () -> ActiveDaemonLocator?

    init(
        configurationLoader: @escaping @Sendable () throws -> DaemonLaunchConfiguration = {
            try .current()
        },
        commandRunner: @escaping @Sendable (URL, [String]) throws -> CommandResult = Self.runCommand,
        activeDaemonLocatorLoader: @escaping @Sendable () -> ActiveDaemonLocator? = {
            ManagementCredentialStore.loadLocator()
        }
    ) {
        self.configurationLoader = configurationLoader
        self.commandRunner = commandRunner
        self.activeDaemonLocatorLoader = activeDaemonLocatorLoader
    }

    func startIfNeeded() async throws -> DaemonLaunchOutcome {
        let configurationLoader = configurationLoader
        let commandRunner = commandRunner
        return try await Task.detached(priority: .userInitiated) {
            let configuration = try configurationLoader()
            return try Self.installAndStart(
                configuration: configuration,
                commandRunner: commandRunner
            )
        }.value
    }

    /// Stage the helper embedded in the current GUI bundle without changing
    /// the active runtime or the loaded LaunchAgent. Callers should perform
    /// the daemon's lifecycle drain before invoking activation.
    func prepareDaemonUpgradeIfNeeded() async throws -> DaemonUpgradePreparation {
        let configurationLoader = configurationLoader
        let commandRunner = commandRunner
        return try await Task.detached(priority: .userInitiated) {
            let configuration = try configurationLoader()
            return try Self.prepareDaemonUpgrade(
                configuration: configuration,
                fileManager: .default,
                commandRunner: commandRunner
            )
        }.value
    }

    /// Atomically publish a previously staged helper and reload its LaunchAgent.
    /// Callers must have received an accepted lifecycle drain first. The
    /// reload uses bootout/bootstrap so launchd picks up the new plist and
    /// never relies on kickstart -k.
    func activateDaemonUpgrade(
        _ preparation: DaemonUpgradePreparation
    ) async throws -> DaemonUpgradeOutcome {
        let configurationLoader = configurationLoader
        let commandRunner = commandRunner
        return try await Task.detached(priority: .userInitiated) {
            let configuration = try configurationLoader()
            return try Self.activateDaemonUpgrade(
                preparation,
                configuration: configuration,
                fileManager: .default,
                commandRunner: commandRunner
            )
        }.value
    }

    func rollbackDaemonUpgrade(
        _ preparation: DaemonUpgradePreparation
    ) async throws {
        let configurationLoader = configurationLoader
        let commandRunner = commandRunner
        try await Task.detached(priority: .userInitiated) {
            let configuration = try configurationLoader()
            try Self.rollbackDaemonUpgrade(
                preparation,
                configuration: configuration,
                fileManager: .default,
                commandRunner: commandRunner
            )
        }.value
    }

    func prepareRuntimeUpgrade() async throws -> DaemonRuntimeUpgradePlan {
        try await prepareDaemonUpgradeIfNeeded()
    }

    func activateRuntimeUpgrade(
        _ plan: DaemonRuntimeUpgradePlan
    ) async throws -> DaemonUpgradeOutcome {
        try await activateDaemonUpgrade(plan)
    }

    func rollbackRuntimeUpgrade(_ plan: DaemonRuntimeUpgradePlan) async throws {
        try await rollbackDaemonUpgrade(plan)
    }

    /// Convenience for callers that have already completed the daemon drain.
    /// The two-step methods remain available when a caller needs to stage while
    /// the old process is still serving work.
    func upgradeDaemonIfNeeded() async throws -> DaemonUpgradeOutcome {
        let preparation = try await prepareDaemonUpgradeIfNeeded()
        guard preparation.requiresActivation else {
            return .alreadyCurrent(buildIdentifier: preparation.targetBuildIdentifier)
        }
        return try await activateDaemonUpgrade(preparation)
    }

    func verifiedDaemonIdentity(for lifecycle: ManageLifecycle) async throws -> ManageDaemonIdentity {
        let configurationLoader = configurationLoader
        let activeDaemonLocatorLoader = activeDaemonLocatorLoader
        let commandRunner = commandRunner
        return try await Task.detached(priority: .userInitiated) {
            let configuration = try configurationLoader()
            guard let locator = activeDaemonLocatorLoader() else {
                throw DaemonLaunchError.loadedAgentUntrusted(lifecycle.executable)
            }
            return try Self.verifyDaemonIdentity(
                lifecycle,
                configuration: configuration,
                locator: locator,
                commandRunner: commandRunner
            )
        }.value
    }

    private static func prepareDaemonUpgrade(
        configuration: DaemonLaunchConfiguration,
        fileManager: FileManager,
        commandRunner: @Sendable (URL, [String]) throws -> CommandResult
    ) throws -> DaemonUpgradePreparation {
        let embeddedBuild = try configuration.resolvedBuildIdentifier()
        let previousBuild = try currentRuntimeBuildIdentifierOrNil(
            configuration: configuration,
            fileManager: fileManager
        )
        // A runtime installed by a newer daemon-only package is authoritative;
        // opening an older GUI must never silently downgrade it. Unknown or
        // non-numeric identifiers are preserved fail-closed for the same
        // reason: a lexical comparison could accidentally select a downgrade.
        if let previousBuild,
           previousBuild != embeddedBuild,
           (!isNumericBuild(previousBuild)
               || !isNumericBuild(embeddedBuild)
               || Int(previousBuild)! > Int(embeddedBuild)!)
        {
            return DaemonUpgradePreparation(
                targetBuildIdentifier: previousBuild,
                previousBuildIdentifier: previousBuild
            )
        }

        // A successful rollback needs a runnable old runtime. Verify it
        // before staging or asking the serving daemon to drain.
        if let previousBuild {
            try validateRuntimeBuild(
                at: configuration.activeHelperURL(),
                expectedBuild: previousBuild,
                fileManager: fileManager,
                commandRunner: commandRunner
            )
        }

        try stageEmbeddedRuntime(
            configuration: configuration,
            expectedBuild: embeddedBuild,
            fileManager: fileManager,
            commandRunner: commandRunner
        )

        let previousLaunchAgentData: Data?
        if fileManager.fileExists(atPath: configuration.launchAgentURL.path) {
            do {
                previousLaunchAgentData = try Data(contentsOf: configuration.launchAgentURL)
            } catch {
                throw DaemonLaunchError.launchAgentWriteFailed
            }
        } else {
            previousLaunchAgentData = nil
        }

        return DaemonUpgradePreparation(
            targetBuildIdentifier: embeddedBuild,
            previousBuildIdentifier: previousBuild,
            previousLaunchAgentData: previousLaunchAgentData
        )
    }

    private static func activateDaemonUpgrade(
        _ preparation: DaemonUpgradePreparation,
        configuration: DaemonLaunchConfiguration,
        fileManager: FileManager,
        commandRunner: @Sendable (URL, [String]) throws -> CommandResult
    ) throws -> DaemonUpgradeOutcome {
        let targetBuild = try configuration.resolvedBuildIdentifier()
        guard targetBuild == preparation.targetBuildIdentifier else {
            throw DaemonLaunchError.upgradePreparationStale(
                expected: preparation.targetBuildIdentifier,
                actual: targetBuild
            )
        }

        let currentBuild = try currentRuntimeBuildIdentifierOrNil(
            configuration: configuration,
            fileManager: fileManager
        )
        guard currentBuild == preparation.previousBuildIdentifier else {
            throw DaemonLaunchError.upgradePreparationStale(
                expected: preparation.previousBuildIdentifier,
                actual: currentBuild
            )
        }
        guard preparation.requiresActivation else {
            return .alreadyCurrent(buildIdentifier: preparation.targetBuildIdentifier)
        }

        try validateRuntimeBuild(
            at: configuration.configURL
                .deletingLastPathComponent()
                .appendingPathComponent("runtimes", isDirectory: true)
                .appendingPathComponent(preparation.targetBuildIdentifier, isDirectory: true)
                .appendingPathComponent(configuration.helperURL.lastPathComponent),
            expectedBuild: preparation.targetBuildIdentifier,
            fileManager: fileManager,
            commandRunner: commandRunner
        )

        let launchctl = URL(fileURLWithPath: "/bin/launchctl")
        let printResult = try commandRunner(
            launchctl,
            ["print", configuration.launchdServiceTarget]
        )
        let serviceLoaded: Bool
        if printResult.exitCode == 0 {
            guard loadedAgentMatches(output: printResult.output, configuration: configuration) else {
                throw DaemonLaunchError.loadedAgentMismatch(
                    expected: configuration.activeHelperURL().path,
                    actual: loadedProgram(from: printResult.output)
                )
            }
            switch launchdServiceState(from: printResult.output) {
            case .running:
                // The caller has already received an accepted drain response.
                // Keep the job loaded and let bootout stop the draining process
                // before bootstrap starts the staged runtime.
                serviceLoaded = true
            case .stopped:
                serviceLoaded = true
            case .unknown:
                throw DaemonLaunchError.launchctlFailed("无法确认已登记后台服务的运行状态。")
            }
        } else {
            guard explicitlyReportsMissingService(
                printResult,
                serviceTarget: configuration.launchdServiceTarget
            ) else {
                throw DaemonLaunchError.launchctlFailed(lastLine(of: printResult.output))
            }
            serviceLoaded = false
        }

        let previousLaunchAgentData = preparation.previousLaunchAgentData
        var activeRuntimePublished = false
        var serviceBootoutAttempted = false
        var serviceBootstrapAttempted = false
        do {
            try replaceActiveRuntime(
                configuration: configuration,
                buildIdentifier: preparation.targetBuildIdentifier,
                fileManager: fileManager
            )
            activeRuntimePublished = true

            try writeLaunchAgent(configuration, fileManager: fileManager)

            if serviceLoaded {
                serviceBootoutAttempted = true
                let bootout = try commandRunner(
                    launchctl,
                    ["bootout", configuration.launchdServiceTarget]
                )
                guard bootout.exitCode == 0 else {
                    throw DaemonLaunchError.launchctlFailed(lastLine(of: bootout.output))
                }
            }

            serviceBootstrapAttempted = true
            let bootstrap = try commandRunner(
                launchctl,
                ["bootstrap", "gui/\(getuid())", configuration.launchAgentURL.path]
            )
            guard bootstrap.exitCode == 0 else {
                throw DaemonLaunchError.launchctlFailed(lastLine(of: bootstrap.output))
            }

            return .activated(
                previousBuildIdentifier: preparation.previousBuildIdentifier,
                buildIdentifier: preparation.targetBuildIdentifier
            )
        } catch {
            let originalError = (error as? DaemonLaunchError)
                ?? DaemonLaunchError.runtimeActivationFailed
            do {
                if activeRuntimePublished {
                    try replaceActiveRuntime(
                        configuration: configuration,
                        buildIdentifier: preparation.previousBuildIdentifier,
                        fileManager: fileManager
                    )
                }
                try restoreLaunchAgent(
                    at: configuration.launchAgentURL,
                    data: previousLaunchAgentData,
                    fileManager: fileManager
                )

                if serviceBootoutAttempted || serviceBootstrapAttempted {
                    // A failed bootout/bootstrap can still have changed the
                    // job. Inspect launchd after restoring the old runtime
                    // and plist, then either preserve the verified old job
                    // or replace the partial new one.
                    try restoreLaunchdServiceIfNeeded(
                        configuration: configuration,
                        launchctl: launchctl,
                        commandRunner: commandRunner
                    )
                }
            } catch {
                throw DaemonLaunchError.runtimeRollbackFailed
            }
            throw originalError
        }
    }

    private static func rollbackDaemonUpgrade(
        _ preparation: DaemonUpgradePreparation,
        configuration: DaemonLaunchConfiguration,
        fileManager: FileManager,
        commandRunner: @Sendable (URL, [String]) throws -> CommandResult
    ) throws {
        guard preparation.previousBuildIdentifier != nil else {
            throw DaemonLaunchError.runtimeRollbackFailed
        }
        let launchctl = URL(fileURLWithPath: "/bin/launchctl")
        let target = configuration.launchdServiceTarget
        let printResult = try commandRunner(launchctl, ["print", target])
        if printResult.exitCode == 0 {
            let bootout = try commandRunner(launchctl, ["bootout", target])
            guard bootout.exitCode == 0 else {
                // launchctl may report failure after it has already removed
                // the job. Only continue when a second observation confirms
                // that no job remains to run the new runtime.
                let afterBootout = try commandRunner(launchctl, ["print", target])
                guard afterBootout.exitCode != 0,
                      explicitlyReportsMissingService(
                          afterBootout,
                          serviceTarget: target
                      )
                else {
                    throw DaemonLaunchError.runtimeRollbackFailed
                }
                // The partial bootout did remove the new job, so it is safe
                // to publish and bootstrap the verified old runtime below.
                return try restoreDaemonUpgradeAfterBootout(
                    preparation,
                    configuration: configuration,
                    fileManager: fileManager,
                    launchctl: launchctl,
                    commandRunner: commandRunner
                )
            }
        } else {
            guard explicitlyReportsMissingService(printResult, serviceTarget: target) else {
                throw DaemonLaunchError.runtimeRollbackFailed
            }
        }
        try restoreDaemonUpgradeAfterBootout(
            preparation,
            configuration: configuration,
            fileManager: fileManager,
            launchctl: launchctl,
            commandRunner: commandRunner
        )
    }

    private static func restoreDaemonUpgradeAfterBootout(
        _ preparation: DaemonUpgradePreparation,
        configuration: DaemonLaunchConfiguration,
        fileManager: FileManager,
        launchctl: URL,
        commandRunner: @Sendable (URL, [String]) throws -> CommandResult
    ) throws {
        guard let previousBuild = preparation.previousBuildIdentifier else {
            throw DaemonLaunchError.runtimeRollbackFailed
        }
        try replaceActiveRuntime(
            configuration: configuration,
            buildIdentifier: previousBuild,
            fileManager: fileManager
        )
        try restoreLaunchAgent(
            at: configuration.launchAgentURL,
            data: preparation.previousLaunchAgentData,
            fileManager: fileManager
        )
        guard preparation.previousLaunchAgentData != nil else { return }
        try bootstrapRestoredLaunchAgent(
            configuration: configuration,
            launchctl: launchctl,
            commandRunner: commandRunner
        )
    }

    private static func bootstrapRestoredLaunchAgent(
        configuration: DaemonLaunchConfiguration,
        launchctl: URL,
        commandRunner: @Sendable (URL, [String]) throws -> CommandResult
    ) throws {
        let bootstrap = try commandRunner(
            launchctl,
            ["bootstrap", "gui/\(getuid())", configuration.launchAgentURL.path]
        )
        guard bootstrap.exitCode == 0 else {
            throw DaemonLaunchError.runtimeRollbackFailed
        }
    }

    private static func restoreLaunchdServiceIfNeeded(
        configuration: DaemonLaunchConfiguration,
        launchctl: URL,
        commandRunner: @Sendable (URL, [String]) throws -> CommandResult
    ) throws {
        let printResult = try commandRunner(
            launchctl,
            ["print", configuration.launchdServiceTarget]
        )
        if printResult.exitCode == 0 {
            if !loadedAgentMatches(output: printResult.output, configuration: configuration) {
                let bootout = try commandRunner(
                    launchctl,
                    ["bootout", configuration.launchdServiceTarget]
                )
                guard bootout.exitCode == 0 else {
                    throw DaemonLaunchError.runtimeRollbackFailed
                }
                try bootstrapRestoredLaunchAgent(
                    configuration: configuration,
                    launchctl: launchctl,
                    commandRunner: commandRunner
                )
            }
            return
        }
        guard explicitlyReportsMissingService(
            printResult,
            serviceTarget: configuration.launchdServiceTarget
        ) else {
            throw DaemonLaunchError.runtimeRollbackFailed
        }
        try bootstrapRestoredLaunchAgent(
            configuration: configuration,
            launchctl: launchctl,
            commandRunner: commandRunner
        )
    }

    private static func stageEmbeddedRuntime(
        configuration: DaemonLaunchConfiguration,
        expectedBuild: String,
        fileManager: FileManager,
        commandRunner: @Sendable (URL, [String]) throws -> CommandResult
    ) throws {
        var isDirectory: ObjCBool = false
        guard fileManager.fileExists(
            atPath: configuration.helperURL.path,
            isDirectory: &isDirectory
        ), !isDirectory.boolValue else {
            throw DaemonLaunchError.helperMissing
        }
        guard fileManager.isExecutableFile(atPath: configuration.helperURL.path) else {
            throw DaemonLaunchError.helperNotExecutable
        }

        let destination = configuration.configURL
            .deletingLastPathComponent()
            .appendingPathComponent("runtimes", isDirectory: true)
            .appendingPathComponent(expectedBuild, isDirectory: true)
            .appendingPathComponent(configuration.helperURL.lastPathComponent)
        let runtimeDirectory = destination.deletingLastPathComponent()
        do {
            try fileManager.createDirectory(
                at: runtimeDirectory,
                withIntermediateDirectories: true
            )
        } catch {
            throw DaemonLaunchError.runtimeDirectoryUnavailable
        }

        if fileManager.fileExists(atPath: destination.path) {
            do {
                try validateRuntimeBuild(
                    at: destination,
                    expectedBuild: expectedBuild,
                    fileManager: fileManager,
                    commandRunner: commandRunner
                )
                guard try helpersHaveMatchingContents(
                    configuration.helperURL,
                    destination
                ) else {
                    throw DaemonLaunchError.runtimeContentsMismatch(
                        buildIdentifier: expectedBuild
                    )
                }
                return
            } catch let error as DaemonLaunchError {
                throw error
            } catch {
                throw DaemonLaunchError.runtimeStageFailed
            }
        }

        let temporary = runtimeDirectory.appendingPathComponent(
            ".mochiport-daemon-upgrade.\(UUID().uuidString).tmp"
        )
        defer { try? fileManager.removeItem(at: temporary) }
        do {
            try fileManager.copyItem(at: configuration.helperURL, to: temporary)
            try fileManager.setAttributes(
                [.posixPermissions: 0o755],
                ofItemAtPath: temporary.path
            )
        } catch {
            throw DaemonLaunchError.runtimeStageFailed
        }
        try validateRuntimePermissions(at: temporary, fileManager: fileManager)
        let versionResult: CommandResult
        do {
            versionResult = try commandRunner(temporary, ["--version"])
        } catch {
            throw DaemonLaunchError.runtimeVersionMismatch(
                expected: expectedBuild,
                actual: nil
            )
        }
        let actualBuild = daemonBuildIdentifier(fromVersionOutput: versionResult.output)
        guard versionResult.exitCode == 0, actualBuild == expectedBuild else {
            throw DaemonLaunchError.runtimeVersionMismatch(
                expected: expectedBuild,
                actual: actualBuild
            )
        }
        let renameResult = temporary.path.withCString { temporaryPath in
            destination.path.withCString { destinationPath in
                Darwin.rename(temporaryPath, destinationPath)
            }
        }
        guard renameResult == 0 else {
            throw DaemonLaunchError.runtimeStageFailed
        }
        try validateRuntimePermissions(at: destination, fileManager: fileManager)
    }

    private static func validateRuntimeBuild(
        at helperURL: URL,
        expectedBuild: String,
        fileManager: FileManager,
        commandRunner: @Sendable (URL, [String]) throws -> CommandResult
    ) throws {
        do {
            try validateRuntimePermissions(at: helperURL, fileManager: fileManager)
        } catch {
            throw DaemonLaunchError.currentRuntimeInvalid
        }
        let versionResult: CommandResult
        do {
            versionResult = try commandRunner(helperURL, ["--version"])
        } catch {
            throw DaemonLaunchError.currentRuntimeInvalid
        }
        let actualBuild = daemonBuildIdentifier(fromVersionOutput: versionResult.output)
        guard versionResult.exitCode == 0, actualBuild == expectedBuild else {
            throw DaemonLaunchError.currentRuntimeInvalid
        }
    }

    private static func helpersHaveMatchingContents(
        _ embeddedHelper: URL,
        _ runtimeHelper: URL
    ) throws -> Bool {
        do {
            let embedded = try Data(contentsOf: embeddedHelper, options: .mappedIfSafe)
            let runtime = try Data(contentsOf: runtimeHelper, options: .mappedIfSafe)
            return SHA256.hash(data: embedded) == SHA256.hash(data: runtime)
        } catch {
            throw DaemonLaunchError.runtimeStageFailed
        }
    }

    private static func currentRuntimeBuildIdentifierOrNil(
        configuration: DaemonLaunchConfiguration,
        fileManager: FileManager
    ) throws -> String? {
        let active = configuration.configURL
            .deletingLastPathComponent()
            .appendingPathComponent("runtimes", isDirectory: true)
            .appendingPathComponent("current", isDirectory: true)
        var info = stat()
        guard Darwin.lstat(active.path, &info) == 0 else {
            guard errno == ENOENT else { throw DaemonLaunchError.currentRuntimeInvalid }
            return nil
        }
        guard info.st_mode & mode_t(S_IFMT) == mode_t(S_IFLNK) else {
            throw DaemonLaunchError.currentRuntimeInvalid
        }
        guard let target = try? fileManager.destinationOfSymbolicLink(atPath: active.path),
              DaemonLaunchConfiguration.isSafeRuntimeBuildIdentifier(target)
        else {
            throw DaemonLaunchError.currentRuntimeInvalid
        }
        return target
    }

    private static func replaceActiveRuntime(
        configuration: DaemonLaunchConfiguration,
        buildIdentifier: String?,
        fileManager: FileManager
    ) throws {
        let runtimes = configuration.configURL
            .deletingLastPathComponent()
            .appendingPathComponent("runtimes", isDirectory: true)
        let active = runtimes.appendingPathComponent("current", isDirectory: true)
        do {
            try fileManager.createDirectory(at: runtimes, withIntermediateDirectories: true)
            if let buildIdentifier {
                guard DaemonLaunchConfiguration.isSafeRuntimeBuildIdentifier(buildIdentifier) else {
                    throw DaemonLaunchError.runtimeActivationFailed
                }
                let temporary = runtimes.appendingPathComponent(
                    ".current-upgrade.\(UUID().uuidString)",
                    isDirectory: true
                )
                defer { try? fileManager.removeItem(at: temporary) }
                let linkResult = buildIdentifier.withCString { destinationPath in
                    temporary.path.withCString { linkPath in
                        Darwin.symlink(destinationPath, linkPath)
                    }
                }
                guard linkResult == 0 else {
                    throw DaemonLaunchError.runtimeActivationFailed
                }
                var info = stat()
                if Darwin.lstat(active.path, &info) == 0,
                   info.st_mode & mode_t(S_IFMT) != mode_t(S_IFLNK)
                {
                    throw DaemonLaunchError.runtimeActivationFailed
                }
                let renameResult = temporary.path.withCString { temporaryPath in
                    active.path.withCString { activePath in
                        Darwin.rename(temporaryPath, activePath)
                    }
                }
                guard renameResult == 0 else {
                    throw DaemonLaunchError.runtimeActivationFailed
                }
            } else {
                var info = stat()
                if Darwin.lstat(active.path, &info) == 0 {
                    guard info.st_mode & mode_t(S_IFMT) == mode_t(S_IFLNK) else {
                        throw DaemonLaunchError.runtimeActivationFailed
                    }
                    try fileManager.removeItem(at: active)
                } else {
                    guard errno == ENOENT else {
                        throw DaemonLaunchError.runtimeActivationFailed
                    }
                }
            }
        } catch let error as DaemonLaunchError {
            throw error
        } catch {
            throw DaemonLaunchError.runtimeActivationFailed
        }
    }

    private static func restoreLaunchAgent(
        at url: URL,
        data: Data?,
        fileManager: FileManager
    ) throws {
        do {
            if let data {
                try data.write(to: url, options: .atomic)
            } else if fileManager.fileExists(atPath: url.path) {
                try fileManager.removeItem(at: url)
            }
        } catch {
            throw DaemonLaunchError.runtimeRollbackFailed
        }
    }

    private static func isNumericBuild(_ value: String) -> Bool {
        guard let build = Int(value), build > 0 else { return false }
        return String(build) == value
    }

    private static func verifyDaemonIdentity(
        _ lifecycle: ManageLifecycle,
        configuration: DaemonLaunchConfiguration,
        locator: ActiveDaemonLocator,
        commandRunner: @Sendable (URL, [String]) throws -> CommandResult,
        fileManager: FileManager = .default
    ) throws -> ManageDaemonIdentity {
        guard lifecycle.service.service == "mochiport",
              lifecycle.service.apiMajor == 1,
              let expectedPID = Int32(exactly: lifecycle.service.pid),
              expectedPID > 0,
              lifecycle.service.startedAtMs >= 0,
              pathsMatch(lifecycle.configPath, configuration.configURL.path),
              locator.service == "mochiport",
              locator.apiMajor == 1,
              locator.instanceId == lifecycle.service.instanceId,
              locator.pid == lifecycle.service.pid,
              locator.startedAtMs == lifecycle.service.startedAtMs,
              isKnownControlFile(
                  locator.controlFile,
                  dataDirectory: configuration.configURL.deletingLastPathComponent()
              ),
              let locatorBaseURL = locator.validatedBaseURL,
              baseURL(locatorBaseURL, matchesBind: lifecycle.bind)
        else {
            throw DaemonLaunchError.loadedAgentUntrusted(lifecycle.executable)
        }

        let executableURL = URL(fileURLWithPath: lifecycle.executable)
        guard isManagedRuntimeProgram(
            executableURL,
            configuration: configuration,
            fileManager: fileManager
        ), pathsMatch(lifecycle.executable, configuration.activeHelperURL().path) else {
            throw DaemonLaunchError.loadedAgentUntrusted(lifecycle.executable)
        }

        let loaded = try commandRunner(
            URL(fileURLWithPath: "/bin/launchctl"),
            ["print", configuration.launchdServiceTarget]
        )
        guard loaded.exitCode == 0,
              loadedPID(from: loaded.output) == expectedPID,
              let loadedProgram = loadedProgram(from: loaded.output),
              pathsMatch(loadedProgram, lifecycle.executable),
              loadedArguments(from: loaded.output) == [
                  lifecycle.executable,
                  "--config",
                  lifecycle.configPath,
                  "daemon",
              ],
              loadedHomeMatches(loaded.output, dataDirectory: configuration.configURL.deletingLastPathComponent())
        else {
            throw DaemonLaunchError.loadedAgentUntrusted(loadedProgram(from: loaded.output))
        }

        let executableSha256: String
        do {
            let executableData = try Data(contentsOf: executableURL, options: .mappedIfSafe)
            executableSha256 = SHA256.hash(data: executableData)
                .map { String(format: "%02x", $0) }
                .joined()
        } catch {
            throw DaemonLaunchError.loadedAgentUntrusted(lifecycle.executable)
        }
        if let expectedSha256 = lifecycle.executableSha256,
           expectedSha256.lowercased() != executableSha256
        {
            throw DaemonLaunchError.loadedAgentUntrusted(lifecycle.executable)
        }

        return ManageDaemonIdentity(
            pid: lifecycle.service.pid,
            startedAtMs: lifecycle.service.startedAtMs,
            executable: lifecycle.executable,
            executableSha256: executableSha256,
            bind: lifecycle.bind
        )
    }

    private static func installAndStart(
        configuration: DaemonLaunchConfiguration,
        commandRunner: @Sendable (URL, [String]) throws -> CommandResult
    ) throws -> DaemonLaunchOutcome {
        let launchctl = URL(fileURLWithPath: "/bin/launchctl")
        let printResult = try commandRunner(
            launchctl,
            ["print", configuration.launchdServiceTarget]
        )
        if printResult.exitCode == 0 {
            guard loadedAgentMatches(output: printResult.output, configuration: configuration) else {
                throw DaemonLaunchError.loadedAgentMismatch(
                    expected: configuration.activeHelperURL().path,
                    actual: loadedProgram(from: printResult.output)
                )
            }

            switch launchdServiceState(from: printResult.output) {
            case .running:
                // Health may still be converging, but a running daemon must
                // never be restarted just because the GUI cannot reach it yet.
                return .alreadyRunning
            case .stopped:
                guard loadedPID(from: printResult.output) == nil else {
                    throw DaemonLaunchError.launchctlFailed("无法确认已登记后台服务的运行状态。")
                }
                guard let activeBuild = try currentRuntimeBuildIdentifierOrNil(
                    configuration: configuration,
                    fileManager: .default
                ) else {
                    throw DaemonLaunchError.currentRuntimeInvalid
                }
                try validateRuntimeBuild(
                    at: configuration.activeHelperURL(),
                    expectedBuild: activeBuild,
                    fileManager: .default,
                    commandRunner: commandRunner
                )
                let result = try commandRunner(
                    launchctl,
                    ["kickstart", configuration.launchdServiceTarget]
                )
                guard result.exitCode == 0 else {
                    throw DaemonLaunchError.launchctlFailed(lastLine(of: result.output))
                }
                return .resumedStoppedService
            case .unknown:
                throw DaemonLaunchError.launchctlFailed("无法确认已登记后台服务的运行状态。")
            }
        }

        guard explicitlyReportsMissingService(
            printResult,
            serviceTarget: configuration.launchdServiceTarget
        ) else {
            throw DaemonLaunchError.launchctlFailed(lastLine(of: printResult.output))
        }

        let fileManager = FileManager.default
        try stageRuntime(
            configuration: configuration,
            fileManager: fileManager,
            commandRunner: commandRunner
        )
        try writeLaunchAgent(configuration, fileManager: fileManager)
        let result = try commandRunner(
            launchctl,
            ["bootstrap", "gui/\(getuid())", configuration.launchAgentURL.path]
        )
        guard result.exitCode == 0 else {
            throw DaemonLaunchError.launchctlFailed(lastLine(of: result.output))
        }
        return .bootstrapped
    }

    private enum LaunchdServiceState {
        case running
        case stopped
        case unknown
    }

    private static func launchdServiceState(from output: String) -> LaunchdServiceState {
        guard let state = output
            .split(whereSeparator: \.isNewline)
            .map({ $0.trimmingCharacters(in: .whitespacesAndNewlines) })
            .first(where: { $0.hasPrefix("state = ") })
            .map({ String($0.dropFirst("state = ".count)).lowercased() })
        else {
            return .unknown
        }
        switch state {
        case "running":
            return .running
        case "waiting", "exited", "throttled", "not running":
            return .stopped
        default:
            return .unknown
        }
    }

    /// `launchctl print` uses a non-zero exit code both for an absent job and
    /// for permission/system errors.  Only the canonical not-found response
    /// is safe to treat as a first-install opportunity.
    private static func explicitlyReportsMissingService(
        _ result: CommandResult,
        serviceTarget: String
    ) -> Bool {
        guard result.exitCode != 0 else { return false }
        let output = result.output.lowercased()
        let target = serviceTarget.lowercased()
        let label = target.split(separator: "/").last.map(String.init) ?? target
        return (
            output.contains("could not find service \"\(target)\"")
                || output.contains("could not find service \"\(label)\"")
        ) && output.contains("in domain")
    }

    private static func stageRuntime(
        configuration: DaemonLaunchConfiguration,
        fileManager: FileManager,
        commandRunner: @Sendable (URL, [String]) throws -> CommandResult
    ) throws {
        if try preserveExistingActiveRuntime(
            configuration: configuration,
            fileManager: fileManager,
            commandRunner: commandRunner
        ) {
            return
        }

        let expectedBuild = try configuration.resolvedBuildIdentifier()

        var isDirectory: ObjCBool = false
        guard fileManager.fileExists(
            atPath: configuration.helperURL.path,
            isDirectory: &isDirectory
        ), !isDirectory.boolValue else {
            throw DaemonLaunchError.helperMissing
        }
        guard fileManager.isExecutableFile(atPath: configuration.helperURL.path) else {
            throw DaemonLaunchError.helperNotExecutable
        }

        let destination = try configuration.stagedHelperURL()
        let runtimeDirectory = destination.deletingLastPathComponent()
        do {
            try fileManager.createDirectory(
                at: runtimeDirectory,
                withIntermediateDirectories: true
            )
        } catch {
            throw DaemonLaunchError.runtimeDirectoryUnavailable
        }

        let temporary = runtimeDirectory.appendingPathComponent(
            ".mochiport-daemon.\(UUID().uuidString).tmp"
        )
        defer { try? fileManager.removeItem(at: temporary) }
        do {
            try fileManager.copyItem(at: configuration.helperURL, to: temporary)
            try fileManager.setAttributes(
                [.posixPermissions: 0o755],
                ofItemAtPath: temporary.path
            )
        } catch {
            throw DaemonLaunchError.runtimeStageFailed
        }
        try validateRuntimePermissions(at: temporary, fileManager: fileManager)

        let versionResult: CommandResult
        do {
            versionResult = try commandRunner(temporary, ["--version"])
        } catch {
            throw DaemonLaunchError.runtimeVersionMismatch(
                expected: expectedBuild,
                actual: nil
            )
        }
        let actualBuild = daemonBuildIdentifier(fromVersionOutput: versionResult.output)
        guard versionResult.exitCode == 0, actualBuild == expectedBuild else {
            throw DaemonLaunchError.runtimeVersionMismatch(
                expected: expectedBuild,
                actual: actualBuild
            )
        }

        let renameResult = temporary.path.withCString { temporaryPath in
            destination.path.withCString { destinationPath in
                Darwin.rename(temporaryPath, destinationPath)
            }
        }
        guard renameResult == 0 else {
            throw DaemonLaunchError.runtimeStageFailed
        }
        try validateRuntimePermissions(at: destination, fileManager: fileManager)
        try activateRuntime(
            configuration: configuration,
            buildIdentifier: expectedBuild
        )
    }

    /// A valid current runtime is authoritative. GUI launches may start it,
    /// but must never replace it based on the embedded helper's build.
    private static func preserveExistingActiveRuntime(
        configuration: DaemonLaunchConfiguration,
        fileManager: FileManager,
        commandRunner: @Sendable (URL, [String]) throws -> CommandResult
    ) throws -> Bool {
        let runtimes = configuration.configURL
            .deletingLastPathComponent()
            .appendingPathComponent("runtimes", isDirectory: true)
        let active = runtimes.appendingPathComponent("current", isDirectory: true)
        var info = stat()
        guard Darwin.lstat(active.path, &info) == 0 else {
            guard errno == ENOENT else { throw DaemonLaunchError.currentRuntimeInvalid }
            return false
        }
        guard info.st_mode & mode_t(S_IFMT) == mode_t(S_IFLNK) else {
            throw DaemonLaunchError.currentRuntimeInvalid
        }

        let target: String
        do {
            target = try fileManager.destinationOfSymbolicLink(atPath: active.path)
        } catch {
            throw DaemonLaunchError.currentRuntimeInvalid
        }
        guard DaemonLaunchConfiguration.isSafeRuntimeBuildIdentifier(target) else {
            throw DaemonLaunchError.currentRuntimeInvalid
        }

        let helper = active.appendingPathComponent(configuration.helperURL.lastPathComponent)
        do {
            try validateRuntimePermissions(at: helper, fileManager: fileManager)
        } catch {
            throw DaemonLaunchError.currentRuntimeInvalid
        }
        let versionResult: CommandResult
        do {
            versionResult = try commandRunner(helper, ["--version"])
        } catch {
            throw DaemonLaunchError.currentRuntimeInvalid
        }
        let actualBuild = daemonBuildIdentifier(fromVersionOutput: versionResult.output)
        guard versionResult.exitCode == 0, actualBuild == target
        else {
            throw DaemonLaunchError.currentRuntimeInvalid
        }
        return true
    }

    private static func activateRuntime(
        configuration: DaemonLaunchConfiguration,
        buildIdentifier: String
    ) throws {
        let runtimes = configuration.configURL
            .deletingLastPathComponent()
            .appendingPathComponent("runtimes", isDirectory: true)
        let active = runtimes.appendingPathComponent("current", isDirectory: true)
        do {
            let linkResult = buildIdentifier.withCString { destinationPath in
                active.path.withCString { linkPath in
                    Darwin.symlink(destinationPath, linkPath)
                }
            }
            guard linkResult == 0 else {
                if errno == EEXIST {
                    throw DaemonLaunchError.currentRuntimeInvalid
                }
                throw DaemonLaunchError.runtimeStageFailed
            }
        } catch let error as DaemonLaunchError {
            throw error
        } catch {
            throw DaemonLaunchError.runtimeStageFailed
        }
    }

    private static func validateRuntimePermissions(
        at url: URL,
        fileManager: FileManager
    ) throws {
        let attributes: [FileAttributeKey: Any]
        do {
            attributes = try fileManager.attributesOfItem(atPath: url.path)
        } catch {
            throw DaemonLaunchError.runtimeNotExecutable
        }
        let permissions = (attributes[.posixPermissions] as? NSNumber)?.intValue
        guard attributes[.type] as? FileAttributeType == .typeRegular,
              permissions.map({ $0 & 0o777 }) == 0o755,
              fileManager.isExecutableFile(atPath: url.path)
        else {
            throw DaemonLaunchError.runtimeNotExecutable
        }
    }

    private static func daemonBuildIdentifier(fromVersionOutput output: String) -> String? {
        let line = output.trimmingCharacters(in: .whitespacesAndNewlines)
        guard line.hasPrefix("mochiport "),
              let buildStart = line.range(of: "(build "),
              line.hasSuffix(")")
        else { return nil }
        let value = line[buildStart.upperBound..<line.index(before: line.endIndex)]
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return value.isEmpty ? nil : value
    }

    static func loadedAgentMatches(
        output: String,
        configuration: DaemonLaunchConfiguration
    ) -> Bool {
        let lines = output.split(whereSeparator: \.isNewline).map {
            $0.trimmingCharacters(in: .whitespacesAndNewlines)
        }
        guard let expectedBuild = try? configuration.activeRuntimeBuildIdentifier(),
              loadedProgram(from: output) == configuration.activeHelperURL().path,
              loadedHomeMatches(output, dataDirectory: configuration.configURL.deletingLastPathComponent()),
              loadedEnvironmentValue(from: output, key: "MOCHIPORT_BUNDLE_BUILD") == expectedBuild,
              loadedEnvironmentValue(
                  from: output,
                  key: "MOCHIPORT_SKIP_DESKTOP_INTEGRATION"
              ) == configuration.desktopIntegrationEnvironmentValue,
              let argumentsStart = lines.firstIndex(where: { $0 == "arguments = {" }),
              let argumentsEnd = lines[(argumentsStart + 1)...].firstIndex(of: "}")
        else {
            return false
        }
        let arguments = lines[(argumentsStart + 1)..<argumentsEnd]
            .filter { !$0.isEmpty }
            .map(unquote)
        return arguments == [
            configuration.activeHelperURL().path,
            "--config",
            configuration.configURL.path,
            "daemon",
        ]
    }

    private static func isManagedRuntimeProgram(
        _ programURL: URL,
        configuration: DaemonLaunchConfiguration,
        fileManager: FileManager
    ) -> Bool {
        let resolvedProgram = programURL.standardizedFileURL.resolvingSymlinksInPath()
        let runtimeRoot = configuration.configURL
            .deletingLastPathComponent()
            .appendingPathComponent("runtimes", isDirectory: true)
            .standardizedFileURL
            .resolvingSymlinksInPath()
        guard resolvedProgram.lastPathComponent == "mochiport-daemon",
              resolvedProgram.deletingLastPathComponent().deletingLastPathComponent() == runtimeRoot
        else {
            return false
        }
        let buildIdentifier = resolvedProgram.deletingLastPathComponent().lastPathComponent
        let allowed = CharacterSet(
            charactersIn: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-"
        )
        guard !buildIdentifier.isEmpty,
              buildIdentifier != ".",
              buildIdentifier != "..",
              buildIdentifier.rangeOfCharacter(from: allowed.inverted) == nil,
              let attributes = try? fileManager.attributesOfItem(atPath: resolvedProgram.path),
              attributes[.type] as? FileAttributeType == .typeRegular,
              fileManager.isExecutableFile(atPath: resolvedProgram.path)
        else {
            return false
        }
        return true
    }

    private static func baseURL(_ baseURL: URL, matchesBind bind: String) -> Bool {
        guard let baseComponents = URLComponents(
            url: baseURL,
            resolvingAgainstBaseURL: false
        ),
        let bindComponents = URLComponents(
            string: bind.contains("://") ? bind : "http://\(bind)"
        ),
        baseComponents.scheme?.lowercased() == "http",
        bindComponents.scheme?.lowercased() == "http",
        baseComponents.port == bindComponents.port,
        let baseHost = baseComponents.host?.lowercased(),
        let bindHost = bindComponents.host?.lowercased()
        else {
            return false
        }
        return normalizedLoopbackHost(baseHost) == normalizedLoopbackHost(bindHost)
    }

    private static func normalizedLoopbackHost(_ host: String) -> String? {
        switch host.trimmingCharacters(in: CharacterSet(charactersIn: "[]")) {
        case "127.0.0.1": "127.0.0.1"
        case "::1": "::1"
        default: nil
        }
    }

    private static func pathsMatch(_ lhs: String, _ rhs: String) -> Bool {
        URL(fileURLWithPath: lhs).standardizedFileURL.resolvingSymlinksInPath()
            == URL(fileURLWithPath: rhs).standardizedFileURL.resolvingSymlinksInPath()
    }

    private static func isKnownControlFile(_ path: String, dataDirectory: URL) -> Bool {
        pathsMatch(path, dataDirectory.appendingPathComponent("mochiport-control.json").path)
    }

    private static func loadedProgram(from output: String) -> String? {
        output
            .split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .first(where: { $0.hasPrefix("program = ") })
            .map { String($0.dropFirst("program = ".count)) }
            .map(unquote)
    }

    private static func loadedPID(from output: String) -> Int32? {
        output
            .split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .first(where: { $0.hasPrefix("pid = ") })
            .flatMap { Int32($0.dropFirst("pid = ".count)) }
    }

    private static func loadedArguments(from output: String) -> [String]? {
        let lines = output.split(whereSeparator: \.isNewline).map {
            $0.trimmingCharacters(in: .whitespacesAndNewlines)
        }
        guard let argumentsStart = lines.firstIndex(where: { $0 == "arguments = {" }),
              let argumentsEnd = lines[(argumentsStart + 1)...].firstIndex(of: "}")
        else {
            return nil
        }
        return lines[(argumentsStart + 1)..<argumentsEnd]
            .filter { !$0.isEmpty }
            .map(unquote)
    }

    private static func loadedEnvironmentValue(from output: String, key: String) -> String? {
        output
            .split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .compactMap { line -> String? in
                let separator = line.contains("=>") ? "=>" : "="
                guard let range = line.range(of: separator) else { return nil }
                let name = line[..<range.lowerBound].trimmingCharacters(in: .whitespacesAndNewlines)
                guard name == key else { return nil }
                return unquote(
                    line[range.upperBound...].trimmingCharacters(in: .whitespacesAndNewlines)
                )
            }
            .first
    }

    private static func loadedHomeMatches(_ output: String, dataDirectory: URL) -> Bool {
        loadedEnvironmentValue(from: output, key: "MOCHIPORT_HOME") == dataDirectory.path
    }

    private static func writeLaunchAgent(
        _ configuration: DaemonLaunchConfiguration,
        fileManager: FileManager
    ) throws {
        do {
            try fileManager.createDirectory(
                at: configuration.launchAgentURL.deletingLastPathComponent(),
                withIntermediateDirectories: true
            )
            try fileManager.createDirectory(
                at: configuration.logURL.deletingLastPathComponent(),
                withIntermediateDirectories: true
            )
        } catch {
            throw DaemonLaunchError.launchAgentDirectoryUnavailable
        }
        do {
            let data = try configuration.propertyListData()
            if (try? Data(contentsOf: configuration.launchAgentURL)) != data {
                try data.write(to: configuration.launchAgentURL, options: .atomic)
            }
        } catch let error as DaemonLaunchError {
            throw error
        } catch {
            throw DaemonLaunchError.launchAgentWriteFailed
        }
    }

    private static func runCommand(_ executable: URL, _ arguments: [String]) throws -> CommandResult {
        let process = Process()
        let output = Pipe()
        process.executableURL = executable
        process.arguments = arguments
        process.standardOutput = output
        process.standardError = output
        try process.run()
        process.waitUntilExit()
        let data = output.fileHandleForReading.readDataToEndOfFile()
        return CommandResult(
            exitCode: process.terminationStatus,
            output: String(decoding: data, as: UTF8.self)
        )
    }

    private static func unquote(_ value: String) -> String {
        guard value.count >= 2, value.first == "\"", value.last == "\"" else {
            return value
        }
        return String(value.dropFirst().dropLast())
    }

    private static func lastLine(of output: String) -> String {
        output
            .split(whereSeparator: \.isNewline)
            .last
            .map(String.init) ?? ""
    }
}
