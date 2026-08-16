import Darwin
import Foundation

@_silgen_name("flock")
private func threadRelayFlock(_ descriptor: Int32, _ operation: Int32) -> Int32

enum DaemonLaunchError: LocalizedError, Equatable {
    case helperMissing
    case helperNotExecutable
    case launchdLabelInvalid(String)
    case runtimeBuildIdentifierInvalid(String)
    case runtimeDirectoryUnavailable
    case runtimeStageFailed
    case runtimeNotExecutable
    case runtimeVersionMismatch(expected: String, actual: String?)
    case guiExecutableMissing
    case guiExecutableNotExecutable
    case guiSupervisorMissing
    case guiSupervisorNotExecutable
    case launchAgentDirectoryUnavailable
    case launchAgentWriteFailed
    case launchAgentSnapshotUnavailable
    case runtimeSwitchBusy
    case runtimeSwitchRecoveryRequired
    case runtimeSwitchJournalFailed
    case loadedAgentMismatch(expected: String, actual: String?)
    case loadedAgentUntrusted(String?)
    case daemonProcessChanged(expected: Int32, actual: Int32?)
    case daemonFreezeFailed(Int32)
    case runtimeSwitchFailed(String)
    case runtimeRollbackFailed(String)
    case launchctlFailed(String)

    var errorDescription: String? {
        switch self {
        case .helperMissing:
            return "应用内未找到后台服务。请重新安装 ThreadRelay。"
        case .helperNotExecutable:
            return "应用内的后台服务不可执行。请重新安装 ThreadRelay。"
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
        case .guiExecutableMissing:
            return "应用内未找到 ThreadRelay 界面程序。请重新安装 ThreadRelay。"
        case .guiExecutableNotExecutable:
            return "应用内的 ThreadRelay 界面程序不可执行。请重新安装 ThreadRelay。"
        case .guiSupervisorMissing:
            return "应用内未找到 ThreadRelay 自动恢复服务。请重新安装 ThreadRelay。"
        case .guiSupervisorNotExecutable:
            return "应用内的 ThreadRelay 自动恢复服务不可执行。请重新安装 ThreadRelay。"
        case .launchAgentDirectoryUnavailable:
            return "无法访问当前用户的后台服务目录。"
        case .launchAgentWriteFailed:
            return "无法保存后台服务启动配置。"
        case .launchAgentSnapshotUnavailable:
            return "无法读取当前后台服务启动配置，已取消切换。"
        case .runtimeSwitchBusy:
            return "另一个后台服务切换正在进行，请稍后重试。"
        case .runtimeSwitchRecoveryRequired:
            return "检测到上次后台服务切换尚未收尾，请先完成恢复。"
        case .runtimeSwitchJournalFailed:
            return "无法保存后台服务切换记录，已取消切换。"
        case let .loadedAgentMismatch(expected, actual):
            let actualDescription = actual.map { "当前为 \($0)" } ?? "当前路径未知"
            return "后台服务启动配置指向了其他版本（应为 \(expected)，\(actualDescription)）。请重新安装 ThreadRelay 后重试。"
        case let .loadedAgentUntrusted(actual):
            let detail = actual.map { "（\($0)）" } ?? ""
            return "当前后台进程无法确认为 ThreadRelay 管理的运行版本\(detail)，已取消切换。"
        case let .daemonProcessChanged(expected, actual):
            let actualText = actual.map(String.init) ?? "未运行"
            return "后台服务进程已变化（预期 \(expected)，当前 \(actualText)），需要重新确认任务状态。"
        case let .daemonFreezeFailed(pid):
            return "无法锁定已排空的后台服务进程 \(pid)，已取消切换。"
        case let .runtimeSwitchFailed(detail):
            return detail.isEmpty ? "新后台服务启动失败。" : "新后台服务启动失败：\(detail)"
        case let .runtimeRollbackFailed(detail):
            return detail.isEmpty ? "新后台服务启动失败，且无法恢复上一版本。" : "新后台服务启动失败，且无法恢复上一版本：\(detail)"
        case let .launchctlFailed(detail):
            return detail.isEmpty ? "无法启动后台服务。" : "无法启动后台服务：\(detail)"
        }
    }
}

struct DaemonLaunchConfiguration: Equatable {
    static let label = "io.github.mps233.threadrelay.daemon"
    fileprivate static let skipDesktopIntegrationEnvironment =
        "THREADRELAY_SKIP_DESKTOP_INTEGRATION"
#if DEBUG
    private static let testLabelPrefix = "io.github.mps233.threadrelay.tests."
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

        let configuredHome = environment["THREADRELAY_HOME"] ?? environment["CODEXHUB_HOME"]
        let dataDirectory: URL
        if let configuredHome, !configuredHome.isEmpty {
            dataDirectory = URL(fileURLWithPath: configuredHome, isDirectory: true)
        } else {
            let threadRelayDirectory = applicationSupport.appendingPathComponent("ThreadRelay", isDirectory: true)
            let legacyDirectory = applicationSupport.appendingPathComponent("CodexHub", isDirectory: true)
            let threadRelayConfig = threadRelayDirectory.appendingPathComponent("config.toml")
            let legacyConfig = legacyDirectory.appendingPathComponent("config.toml")
            if fileManager.fileExists(atPath: threadRelayConfig.path) {
                dataDirectory = threadRelayDirectory
            } else if fileManager.fileExists(atPath: legacyConfig.path) {
                dataDirectory = legacyDirectory
            } else {
                dataDirectory = threadRelayDirectory
            }
        }

        return Self(
            helperURL: bundleURL
                .appendingPathComponent("Contents", isDirectory: true)
                .appendingPathComponent("Helpers", isDirectory: true)
                .appendingPathComponent("threadrelay-daemon"),
            configURL: dataDirectory.appendingPathComponent("config.toml"),
            launchAgentURL: homeURL
                .appendingPathComponent("Library/LaunchAgents", isDirectory: true)
                .appendingPathComponent("\(label).plist"),
            logURL: dataDirectory
                .appendingPathComponent("logs", isDirectory: true)
                .appendingPathComponent("threadrelay-daemon-launchd.log"),
            homeURL: homeURL,
            buildIdentifier: Self.bundleBuildIdentifier(bundleURL: bundleURL)
        )
    }

    private static func bundleBuildIdentifier(bundleURL: URL) -> String? {
        guard let bundle = Bundle(url: bundleURL),
              let value = bundle.object(forInfoDictionaryKey: "CFBundleVersion") as? String
        else {
            return nil
        }
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    func resolvedBuildIdentifier() throws -> String {
        guard let buildIdentifier else { return "dev" }
        let trimmed = buildIdentifier.trimmingCharacters(in: .whitespacesAndNewlines)
        let allowed = CharacterSet(
            charactersIn: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-"
        )
        guard !trimmed.isEmpty,
              trimmed != ".",
              trimmed != "..",
              trimmed.rangeOfCharacter(from: allowed.inverted) == nil
        else {
            throw DaemonLaunchError.runtimeBuildIdentifierInvalid(buildIdentifier)
        }
        return trimmed
    }

    func stagedHelperURL() throws -> URL {
        let buildIdentifier = try resolvedBuildIdentifier()
        return configURL
            .deletingLastPathComponent()
            .appendingPathComponent("runtimes", isDirectory: true)
            .appendingPathComponent(buildIdentifier, isDirectory: true)
            .appendingPathComponent("threadrelay-daemon")
    }

    func propertyListData(runtimeSwitchHold: Bool = false) throws -> Data {
        let resolvedBuildIdentifier = try resolvedBuildIdentifier()
        let stagedHelperURL = try stagedHelperURL()
        var environment: [String: String] = [
            "HOME": homeURL.path,
            "PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
            "THREADRELAY_HOME": configURL.deletingLastPathComponent().path,
        ]
        environment["THREADRELAY_BUNDLE_BUILD"] = resolvedBuildIdentifier
        if let value = desktopIntegrationEnvironmentValue {
            environment[Self.skipDesktopIntegrationEnvironment] = value
        }
        if runtimeSwitchHold {
            environment["THREADRELAY_RUNTIME_SWITCH_HOLD"] = "1"
        }
        let propertyList: [String: Any] = [
            "Label": launchdLabel,
            "ProgramArguments": [
                stagedHelperURL.path,
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

struct GUIRecoveryConfiguration: Equatable {
    static let label = "io.github.mps233.threadrelay.gui"

    let executableURL: URL
    let supervisorURL: URL
    let launchAgentURL: URL
    let logURL: URL
    let homeURL: URL
    let dataDirectoryURL: URL
    let buildIdentifier: String?

    static func current(
        bundleURL: URL = Bundle.main.bundleURL,
        environment: [String: String] = ProcessInfo.processInfo.environment,
        fileManager: FileManager = .default
    ) throws -> Self {
        let daemon = try DaemonLaunchConfiguration.current(
            bundleURL: bundleURL,
            environment: environment,
            fileManager: fileManager
        )
        let contents = bundleURL.appendingPathComponent("Contents", isDirectory: true)
        let helpers = contents.appendingPathComponent("Helpers", isDirectory: true)
        return Self(
            executableURL: contents.appendingPathComponent("MacOS/ThreadRelay"),
            supervisorURL: helpers.appendingPathComponent("threadrelay-gui-supervisor"),
            launchAgentURL: daemon.launchAgentURL
                .deletingLastPathComponent()
                .appendingPathComponent("\(label).plist"),
            logURL: daemon.logURL
                .deletingLastPathComponent()
                .appendingPathComponent("threadrelay-gui-launchd.log"),
            homeURL: daemon.homeURL,
            dataDirectoryURL: daemon.configURL.deletingLastPathComponent(),
            buildIdentifier: daemon.buildIdentifier
        )
    }

    func propertyListData() throws -> Data {
        var environment: [String: String] = [
            "HOME": homeURL.path,
            "PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
            "THREADRELAY_HOME": dataDirectoryURL.path,
        ]
        if let buildIdentifier {
            environment["THREADRELAY_BUNDLE_BUILD"] = buildIdentifier
        }
        let propertyList: [String: Any] = [
            "Label": Self.label,
            "ProgramArguments": [supervisorURL.path],
            "EnvironmentVariables": environment,
            "RunAtLoad": true,
            // A normal menu-bar Quit exits successfully and should stay quit.
            // Signals and crashes are failures that launchd should recover.
            "KeepAlive": ["SuccessfulExit": false],
            "ProcessType": "Interactive",
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

enum DaemonRuntimeSwitchPhase: String, Codable, Sendable {
    case prepared
    case freezingPrevious
    case previousStopped
    case candidateStarted
    case rollingBack
    case rolledBack
    case committed
}

struct DaemonRuntimeSwitchJournal: Codable, Equatable, Sendable {
    let schemaVersion: Int
    let transactionId: String
    var phase: DaemonRuntimeSwitchPhase
    let previousLaunchAgentData: Data
    let previousProgramPath: String
    let previousBuild: String
    let previousInstanceId: String
    let previousPID: Int32
    let candidateLaunchAgentData: Data
    let candidateProgramPath: String
    let candidateBuild: String
    let createdAtMilliseconds: Int64
    var updatedAtMilliseconds: Int64
}

private final class DaemonRuntimeSwitchLock: @unchecked Sendable {
    private let stateLock = NSLock()
    private var descriptor: Int32?

    init(url: URL, fileManager: FileManager) throws {
        do {
            try fileManager.createDirectory(
                at: url.deletingLastPathComponent(),
                withIntermediateDirectories: true
            )
        } catch {
            throw DaemonLaunchError.runtimeSwitchJournalFailed
        }
        let descriptor = url.path.withCString {
            Darwin.open($0, O_CREAT | O_RDWR | O_CLOEXEC, S_IRUSR | S_IWUSR)
        }
        guard descriptor >= 0 else {
            throw DaemonLaunchError.runtimeSwitchJournalFailed
        }
        guard threadRelayFlock(descriptor, LOCK_EX | LOCK_NB) == 0 else {
            Darwin.close(descriptor)
            throw DaemonLaunchError.runtimeSwitchBusy
        }
        _ = Darwin.fchmod(descriptor, S_IRUSR | S_IWUSR)
        self.descriptor = descriptor
    }

    func release() {
        stateLock.withLock {
            guard let descriptor else { return }
            _ = threadRelayFlock(descriptor, LOCK_UN)
            Darwin.close(descriptor)
            self.descriptor = nil
        }
    }

    deinit {
        release()
    }
}

final class DaemonRuntimeSwitch: @unchecked Sendable {
    fileprivate let stateLock = NSLock()
    private let operationLock = NSLock()
    fileprivate var storedJournal: DaemonRuntimeSwitchJournal
    fileprivate let transactionLock: DaemonRuntimeSwitchLock?

    init(journal: DaemonRuntimeSwitchJournal) {
        storedJournal = journal
        transactionLock = nil
    }

    fileprivate init(
        journal: DaemonRuntimeSwitchJournal,
        transactionLock: DaemonRuntimeSwitchLock
    ) {
        storedJournal = journal
        self.transactionLock = transactionLock
    }

    var journal: DaemonRuntimeSwitchJournal {
        stateLock.withLock { storedJournal }
    }

    fileprivate func updateJournal(_ journal: DaemonRuntimeSwitchJournal) {
        stateLock.withLock { storedJournal = journal }
    }

    fileprivate func releaseLock() {
        transactionLock?.release()
    }

    fileprivate func withExclusiveOperation<T>(_ operation: () throws -> T) rethrows -> T {
        operationLock.lock()
        defer { operationLock.unlock() }
        return try operation()
    }
}

protocol DaemonLaunching: Sendable {
    /// Prepare the embedded daemon for a future launch without touching the
    /// currently loaded LaunchAgent or running process.
    func prepareRuntime() async throws
    func startIfNeeded() async throws
    func prepareRuntimeSwitch(
        expectedPID: Int32,
        expectedInstanceId: String,
        expectedExecutable: String
    ) async throws -> DaemonRuntimeSwitch
    func activatePreparedRuntime(
        _ transaction: DaemonRuntimeSwitch,
        expectedPID: Int32,
        expectedExecutable: String
    ) async throws
    func rollbackRuntime(
        _ transaction: DaemonRuntimeSwitch,
        expectedPID: Int32?,
        expectedExecutable: String?
    ) async throws
    func cancelRuntimeSwitch(_ transaction: DaemonRuntimeSwitch) async throws
    func commitRuntimeSwitch(_ transaction: DaemonRuntimeSwitch) async throws
    func loadPendingRuntimeSwitch() async throws -> DaemonRuntimeSwitch?
}

extension DaemonLaunching {
    func prepareRuntime() async throws {}
    func prepareRuntimeSwitch(
        expectedPID _: Int32,
        expectedInstanceId _: String,
        expectedExecutable _: String
    ) async throws -> DaemonRuntimeSwitch {
        throw DaemonLaunchError.runtimeSwitchFailed("当前启动器不支持版本切换。")
    }
    func activatePreparedRuntime(
        _: DaemonRuntimeSwitch,
        expectedPID _: Int32,
        expectedExecutable _: String
    ) async throws {}
    func rollbackRuntime(
        _: DaemonRuntimeSwitch,
        expectedPID _: Int32?,
        expectedExecutable _: String?
    ) async throws {}
    func cancelRuntimeSwitch(_: DaemonRuntimeSwitch) async throws {}
    func commitRuntimeSwitch(_: DaemonRuntimeSwitch) async throws {}
    func loadPendingRuntimeSwitch() async throws -> DaemonRuntimeSwitch? { nil }
}

struct DaemonLauncher: DaemonLaunching, @unchecked Sendable {
    private let configurationLoader: @Sendable () throws -> DaemonLaunchConfiguration
    private let commandRunner: @Sendable (URL, [String]) throws -> CommandResult
    private let processSignaler: @Sendable (Int32, Int32) -> Int32

    init(
        configurationLoader: @escaping @Sendable () throws -> DaemonLaunchConfiguration = {
            try .current()
        },
        commandRunner: @escaping @Sendable (URL, [String]) throws -> CommandResult = Self.runCommand,
        processSignaler: @escaping @Sendable (Int32, Int32) -> Int32 = { pid, signal in
            Darwin.kill(pid, signal)
        }
    ) {
        self.configurationLoader = configurationLoader
        self.commandRunner = commandRunner
        self.processSignaler = processSignaler
    }

    func prepareRuntime() async throws {
        let configurationLoader = configurationLoader
        let commandRunner = commandRunner
        try await Task.detached(priority: .userInitiated) {
            let configuration = try configurationLoader()
            try Self.withRuntimeSwitchLock(configuration: configuration) {
                try Self.requireNoPendingRuntimeSwitch(configuration: configuration)
                try Self.prepareRuntime(
                    configuration: configuration,
                    commandRunner: commandRunner
                )
            }
        }.value
    }

    func startIfNeeded() async throws {
        let configurationLoader = configurationLoader
        let commandRunner = commandRunner
        try await Task.detached(priority: .userInitiated) {
            let configuration = try configurationLoader()
            try Self.withRuntimeSwitchLock(configuration: configuration) {
                try Self.requireNoPendingRuntimeSwitch(configuration: configuration)
                try Self.installAndStart(
                    configuration: configuration,
                    commandRunner: commandRunner
                )
            }
        }.value
    }

    func prepareRuntimeSwitch(
        expectedPID: Int32,
        expectedInstanceId: String,
        expectedExecutable: String
    ) async throws -> DaemonRuntimeSwitch {
        let configurationLoader = configurationLoader
        let commandRunner = commandRunner
        return try await Task.detached(priority: .userInitiated) {
            let configuration = try configurationLoader()
            return try Self.prepareRuntimeSwitch(
                configuration: configuration,
                expectedPID: expectedPID,
                expectedInstanceId: expectedInstanceId,
                expectedExecutable: expectedExecutable,
                commandRunner: commandRunner
            )
        }.value
    }

    func activatePreparedRuntime(
        _ transaction: DaemonRuntimeSwitch,
        expectedPID: Int32,
        expectedExecutable: String
    ) async throws {
        let configurationLoader = configurationLoader
        let commandRunner = commandRunner
        let processSignaler = processSignaler
        try await Task.detached(priority: .userInitiated) {
            let configuration = try configurationLoader()
            try transaction.withExclusiveOperation {
                try Self.activatePreparedRuntime(
                    transaction,
                    configuration: configuration,
                    expectedPID: expectedPID,
                    expectedExecutable: expectedExecutable,
                    commandRunner: commandRunner,
                    processSignaler: processSignaler
                )
            }
        }.value
    }

    func rollbackRuntime(
        _ transaction: DaemonRuntimeSwitch,
        expectedPID: Int32?,
        expectedExecutable: String?
    ) async throws {
        let configurationLoader = configurationLoader
        let commandRunner = commandRunner
        let processSignaler = processSignaler
        try await Task.detached(priority: .userInitiated) {
            let configuration = try configurationLoader()
            try transaction.withExclusiveOperation {
                try Self.rollbackRuntime(
                    transaction,
                    configuration: configuration,
                    expectedPID: expectedPID,
                    expectedExecutable: expectedExecutable,
                    commandRunner: commandRunner,
                    processSignaler: processSignaler
                )
            }
        }.value
    }

    func cancelRuntimeSwitch(_ transaction: DaemonRuntimeSwitch) async throws {
        let configurationLoader = configurationLoader
        let commandRunner = commandRunner
        let processSignaler = processSignaler
        try await Task.detached(priority: .userInitiated) {
            let configuration = try configurationLoader()
            try transaction.withExclusiveOperation {
                try Self.cancelRuntimeSwitch(
                    transaction,
                    configuration: configuration,
                    commandRunner: commandRunner,
                    processSignaler: processSignaler
                )
            }
        }.value
    }

    func commitRuntimeSwitch(_ transaction: DaemonRuntimeSwitch) async throws {
        let configurationLoader = configurationLoader
        try await Task.detached(priority: .userInitiated) {
            let configuration = try configurationLoader()
            try transaction.withExclusiveOperation {
                try Self.commitRuntimeSwitch(transaction, configuration: configuration)
            }
        }.value
    }

    func loadPendingRuntimeSwitch() async throws -> DaemonRuntimeSwitch? {
        let configurationLoader = configurationLoader
        let commandRunner = commandRunner
        let processSignaler = processSignaler
        return try await Task.detached(priority: .userInitiated) {
            let configuration = try configurationLoader()
            return try Self.loadPendingRuntimeSwitch(
                configuration: configuration,
                commandRunner: commandRunner,
                processSignaler: processSignaler
            )
        }.value
    }

    private static func installAndStart(
        configuration: DaemonLaunchConfiguration,
        commandRunner: @Sendable (URL, [String]) throws -> CommandResult
    ) throws {
        let fileManager = FileManager.default
        try prepareRuntime(configuration: configuration, commandRunner: commandRunner)

        let launchctl = URL(fileURLWithPath: "/bin/launchctl")
        let domain = "gui/\(getuid())"
        let serviceTarget = configuration.launchdServiceTarget
        let printResult = try commandRunner(launchctl, ["print", serviceTarget])

        if printResult.exitCode == 0 {
            // Staging prepares the next launch only. An already loaded job may
            // still be serving protected work from an older path or build.
            // Keep both the loaded job and its on-disk plist unchanged.
            return
        }
        try writeLaunchAgent(configuration, fileManager: fileManager)
        let result = try commandRunner(
            launchctl,
            ["bootstrap", domain, configuration.launchAgentURL.path]
        )
        guard result.exitCode == 0 else {
            throw DaemonLaunchError.launchctlFailed(lastLine(of: result.output))
        }
    }

    private struct RuntimeSnapshot {
        let launchAgentData: Data
        let programURL: URL
        let build: String
    }

    private static func prepareRuntimeSwitch(
        configuration: DaemonLaunchConfiguration,
        expectedPID: Int32,
        expectedInstanceId: String,
        expectedExecutable: String,
        commandRunner: @Sendable (URL, [String]) throws -> CommandResult
    ) throws -> DaemonRuntimeSwitch {
        let fileManager = FileManager.default
        let transactionLock = try acquireRuntimeSwitchLock(
            configuration: configuration,
            fileManager: fileManager
        )
        do {
            try requireNoPendingRuntimeSwitch(configuration: configuration)
            try prepareRuntime(configuration: configuration, commandRunner: commandRunner)

            let launchctl = URL(fileURLWithPath: "/bin/launchctl")
            let serviceTarget = configuration.launchdServiceTarget
            let printResult = try commandRunner(launchctl, ["print", serviceTarget])
            guard printResult.exitCode == 0 else {
                throw DaemonLaunchError.launchAgentSnapshotUnavailable
            }
            let previous = try validatedRuntimeSnapshot(
                configuration: configuration,
                loadedAgentOutput: printResult.output,
                fileManager: fileManager
            )
            guard loadedPID(from: printResult.output) == expectedPID else {
                throw DaemonLaunchError.daemonProcessChanged(
                    expected: expectedPID,
                    actual: loadedPID(from: printResult.output)
                )
            }
            guard canonicalPath(expectedExecutable) == canonicalPath(previous.programURL.path) else {
                throw DaemonLaunchError.loadedAgentUntrusted(expectedExecutable)
            }

            let now = currentTimeMilliseconds()
            let journal = DaemonRuntimeSwitchJournal(
                schemaVersion: 1,
                transactionId: UUID().uuidString.lowercased(),
                phase: .prepared,
                previousLaunchAgentData: previous.launchAgentData,
                previousProgramPath: previous.programURL.path,
                previousBuild: previous.build,
                previousInstanceId: expectedInstanceId,
                previousPID: expectedPID,
                candidateLaunchAgentData: try configuration.propertyListData(runtimeSwitchHold: true),
                candidateProgramPath: try configuration.stagedHelperURL().path,
                candidateBuild: try configuration.resolvedBuildIdentifier(),
                createdAtMilliseconds: now,
                updatedAtMilliseconds: now
            )
            try persistRuntimeSwitchJournal(journal, configuration: configuration)
            return DaemonRuntimeSwitch(journal: journal, transactionLock: transactionLock)
        } catch {
            transactionLock.release()
            throw error
        }
    }

    private static func activatePreparedRuntime(
        _ transaction: DaemonRuntimeSwitch,
        configuration: DaemonLaunchConfiguration,
        expectedPID: Int32,
        expectedExecutable: String,
        commandRunner: @escaping @Sendable (URL, [String]) throws -> CommandResult,
        processSignaler: @escaping @Sendable (Int32, Int32) -> Int32
    ) throws {
        let journal = try validatedTransaction(transaction, configuration: configuration)
        guard journal.phase == .prepared || journal.phase == .freezingPrevious else {
            throw DaemonLaunchError.runtimeSwitchRecoveryRequired
        }
        guard canonicalPath(expectedExecutable) == canonicalPath(journal.previousProgramPath) else {
            throw DaemonLaunchError.loadedAgentUntrusted(expectedExecutable)
        }

        let launchctl = URL(fileURLWithPath: "/bin/launchctl")
        let domain = "gui/\(getuid())"
        let serviceTarget = configuration.launchdServiceTarget
        let printResult = try commandRunner(launchctl, ["print", serviceTarget])
        try requireLoadedAgent(
            printResult,
            launchAgentData: journal.previousLaunchAgentData,
            expectedPID: expectedPID,
            configuration: configuration
        )

        try updateRuntimeSwitchPhase(
            .freezingPrevious,
            transaction: transaction,
            configuration: configuration
        )
        do {
            try freezeAndUnloadAgent(
                expectedPID: expectedPID,
                expectedLaunchAgentData: journal.previousLaunchAgentData,
                configuration: configuration,
                launchctl: launchctl,
                serviceTarget: serviceTarget,
                commandRunner: commandRunner,
                processSignaler: processSignaler
            )
            try updateRuntimeSwitchPhase(
                .previousStopped,
                transaction: transaction,
                configuration: configuration
            )
            try writeLaunchAgentData(
                journal.candidateLaunchAgentData,
                to: configuration.launchAgentURL,
                logURL: configuration.logURL,
                fileManager: .default
            )
            let bootstrap = try commandRunner(
                launchctl,
                ["bootstrap", domain, configuration.launchAgentURL.path]
            )
            guard bootstrap.exitCode == 0 else {
                throw DaemonLaunchError.launchctlFailed(lastLine(of: bootstrap.output))
            }
            try updateRuntimeSwitchPhase(
                .candidateStarted,
                transaction: transaction,
                configuration: configuration
            )
        } catch let error as DaemonLaunchError {
            switch error {
            case .daemonProcessChanged, .daemonFreezeFailed, .loadedAgentUntrusted:
                throw error
            default:
                throw DaemonLaunchError.runtimeSwitchFailed(error.localizedDescription)
            }
        }
    }

    private static func rollbackRuntime(
        _ transaction: DaemonRuntimeSwitch,
        configuration: DaemonLaunchConfiguration,
        expectedPID: Int32?,
        expectedExecutable: String?,
        commandRunner: @escaping @Sendable (URL, [String]) throws -> CommandResult,
        processSignaler: @escaping @Sendable (Int32, Int32) -> Int32
    ) throws {
        let journal = try validatedTransaction(transaction, configuration: configuration)
        let launchctl = URL(fileURLWithPath: "/bin/launchctl")
        let domain = "gui/\(getuid())"
        let serviceTarget = configuration.launchdServiceTarget
        var previousIsLoaded = false
        let printResult = try commandRunner(launchctl, ["print", serviceTarget])

        try updateRuntimeSwitchPhase(
            .rollingBack,
            transaction: transaction,
            configuration: configuration
        )
        if printResult.exitCode == 0 {
            if loadedAgentMatches(
                output: printResult.output,
                launchAgentData: journal.previousLaunchAgentData,
                configuration: configuration
            ) {
                previousIsLoaded = true
                if let pid = loadedPID(from: printResult.output) {
                    _ = processSignaler(pid, SIGCONT)
                }
            } else if loadedAgentMatches(
                output: printResult.output,
                launchAgentData: journal.candidateLaunchAgentData,
                configuration: configuration
            ) {
                if let expectedPID, let expectedExecutable,
                   canonicalPath(expectedExecutable) == canonicalPath(journal.candidateProgramPath)
                {
                    try freezeAndUnloadAgent(
                        expectedPID: expectedPID,
                        expectedLaunchAgentData: journal.candidateLaunchAgentData,
                        configuration: configuration,
                        launchctl: launchctl,
                        serviceTarget: serviceTarget,
                        commandRunner: commandRunner,
                        processSignaler: processSignaler
                    )
                } else if expectedPID == nil,
                          expectedExecutable == nil,
                          launchAgentHasRuntimeSwitchHold(journal.candidateLaunchAgentData)
                {
                    try unloadHeldCandidateAgent(
                        expectedLaunchAgentData: journal.candidateLaunchAgentData,
                        configuration: configuration,
                        launchctl: launchctl,
                        serviceTarget: serviceTarget,
                        commandRunner: commandRunner
                    )
                } else {
                    throw DaemonLaunchError.runtimeSwitchRecoveryRequired
                }
            } else {
                throw DaemonLaunchError.loadedAgentUntrusted(
                    loadedProgram(from: printResult.output)
                )
            }
        }

        try writeLaunchAgentData(
            journal.previousLaunchAgentData,
            to: configuration.launchAgentURL,
            logURL: configuration.logURL,
            fileManager: .default
        )
        if !previousIsLoaded {
            let bootstrap = try commandRunner(
                launchctl,
                ["bootstrap", domain, configuration.launchAgentURL.path]
            )
            guard bootstrap.exitCode == 0 else {
                throw DaemonLaunchError.runtimeRollbackFailed(lastLine(of: bootstrap.output))
            }
        }
        try updateRuntimeSwitchPhase(
            .rolledBack,
            transaction: transaction,
            configuration: configuration
        )
    }

    private static func cancelRuntimeSwitch(
        _ transaction: DaemonRuntimeSwitch,
        configuration: DaemonLaunchConfiguration,
        commandRunner: @Sendable (URL, [String]) throws -> CommandResult,
        processSignaler: @Sendable (Int32, Int32) -> Int32
    ) throws {
        let journal = try validatedTransaction(transaction, configuration: configuration)
        guard journal.phase == .prepared || journal.phase == .freezingPrevious else {
            throw DaemonLaunchError.runtimeSwitchRecoveryRequired
        }
        let launchctl = URL(fileURLWithPath: "/bin/launchctl")
        let serviceTarget = configuration.launchdServiceTarget
        let printResult = try commandRunner(launchctl, ["print", serviceTarget])
        guard printResult.exitCode == 0,
              loadedAgentMatches(
                  output: printResult.output,
                  launchAgentData: journal.previousLaunchAgentData,
                  configuration: configuration
              )
        else {
            throw DaemonLaunchError.runtimeSwitchRecoveryRequired
        }
        if journal.phase == .freezingPrevious,
           let pid = loadedPID(from: printResult.output)
        {
            _ = processSignaler(pid, SIGCONT)
        }
        try writeLaunchAgentData(
            journal.previousLaunchAgentData,
            to: configuration.launchAgentURL,
            logURL: configuration.logURL,
            fileManager: .default
        )
        try removeRuntimeSwitchJournal(configuration: configuration)
        transaction.releaseLock()
    }

    private static func commitRuntimeSwitch(
        _ transaction: DaemonRuntimeSwitch,
        configuration: DaemonLaunchConfiguration
    ) throws {
        let journal = try validatedTransaction(transaction, configuration: configuration)
        switch journal.phase {
        case .candidateStarted:
            try writeLaunchAgentData(
                try configuration.propertyListData(),
                to: configuration.launchAgentURL,
                logURL: configuration.logURL,
                fileManager: .default
            )
            try updateRuntimeSwitchPhase(
                .committed,
                transaction: transaction,
                configuration: configuration
            )
        case .committed:
            try writeLaunchAgentData(
                try configuration.propertyListData(),
                to: configuration.launchAgentURL,
                logURL: configuration.logURL,
                fileManager: .default
            )
        case .rolledBack:
            break
        default:
            throw DaemonLaunchError.runtimeSwitchRecoveryRequired
        }
        try removeRuntimeSwitchJournal(configuration: configuration)
        transaction.releaseLock()
    }

    private static func loadPendingRuntimeSwitch(
        configuration: DaemonLaunchConfiguration,
        commandRunner: @Sendable (URL, [String]) throws -> CommandResult,
        processSignaler: @Sendable (Int32, Int32) -> Int32
    ) throws -> DaemonRuntimeSwitch? {
        let fileManager = FileManager.default
        let transactionLock = try acquireRuntimeSwitchLock(
            configuration: configuration,
            fileManager: fileManager
        )
        do {
            let journalURL = runtimeSwitchJournalURL(configuration: configuration)
            guard fileManager.fileExists(atPath: journalURL.path) else {
                transactionLock.release()
                return nil
            }
            guard let data = try? Data(contentsOf: journalURL),
                  let journal = try? JSONDecoder().decode(
                      DaemonRuntimeSwitchJournal.self,
                      from: data
                  )
            else {
                throw DaemonLaunchError.runtimeSwitchRecoveryRequired
            }
            try validateRuntimeSwitchJournal(journal, configuration: configuration)
            let transaction = DaemonRuntimeSwitch(
                journal: journal,
                transactionLock: transactionLock
            )

            let launchctl = URL(fileURLWithPath: "/bin/launchctl")
            let domain = "gui/\(getuid())"
            let serviceTarget = configuration.launchdServiceTarget
            let printResult = try commandRunner(launchctl, ["print", serviceTarget])
            if printResult.exitCode != 0 {
                try writeLaunchAgentData(
                    journal.previousLaunchAgentData,
                    to: configuration.launchAgentURL,
                    logURL: configuration.logURL,
                    fileManager: fileManager
                )
                let bootstrap = try commandRunner(
                    launchctl,
                    ["bootstrap", domain, configuration.launchAgentURL.path]
                )
                guard bootstrap.exitCode == 0 else {
                    throw DaemonLaunchError.runtimeRollbackFailed(lastLine(of: bootstrap.output))
                }
                try updateRuntimeSwitchPhase(
                    .rolledBack,
                    transaction: transaction,
                    configuration: configuration
                )
                return transaction
            }

            if loadedAgentMatches(
                output: printResult.output,
                launchAgentData: journal.previousLaunchAgentData,
                configuration: configuration
            ) {
                if let pid = loadedPID(from: printResult.output) {
                    _ = processSignaler(pid, SIGCONT)
                }
                try writeLaunchAgentData(
                    journal.previousLaunchAgentData,
                    to: configuration.launchAgentURL,
                    logURL: configuration.logURL,
                    fileManager: fileManager
                )
                try updateRuntimeSwitchPhase(
                    .rolledBack,
                    transaction: transaction,
                    configuration: configuration
                )
                return transaction
            }
            if loadedAgentMatches(
                output: printResult.output,
                launchAgentData: journal.candidateLaunchAgentData,
                configuration: configuration
            ) {
                guard journal.phase == .previousStopped
                    || journal.phase == .candidateStarted
                    || journal.phase == .rollingBack
                    || journal.phase == .committed
                else {
                    throw DaemonLaunchError.runtimeSwitchRecoveryRequired
                }
                try writeLaunchAgentData(
                    journal.candidateLaunchAgentData,
                    to: configuration.launchAgentURL,
                    logURL: configuration.logURL,
                    fileManager: fileManager
                )
                if journal.phase == .previousStopped {
                    try updateRuntimeSwitchPhase(
                        .candidateStarted,
                        transaction: transaction,
                        configuration: configuration
                    )
                } else if journal.phase == .rollingBack,
                          let pid = loadedPID(from: printResult.output)
                {
                    _ = processSignaler(pid, SIGCONT)
                }
                return transaction
            }
            throw DaemonLaunchError.runtimeSwitchRecoveryRequired
        } catch {
            transactionLock.release()
            throw error
        }
    }

    private static func prepareRuntime(
        configuration: DaemonLaunchConfiguration,
        commandRunner: @Sendable (URL, [String]) throws -> CommandResult
    ) throws {
        let fileManager = FileManager.default
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

        _ = try stageRuntime(
            configuration: configuration,
            fileManager: fileManager,
            commandRunner: commandRunner
        )
    }

    private static func validatedRuntimeSnapshot(
        configuration: DaemonLaunchConfiguration,
        loadedAgentOutput: String,
        fileManager: FileManager
    ) throws -> RuntimeSnapshot {
        guard let data = try? Data(contentsOf: configuration.launchAgentURL),
              let propertyList = try? PropertyListSerialization.propertyList(
                  from: data,
                  options: [],
                  format: nil
              ) as? [String: Any],
              propertyList["Label"] as? String == configuration.launchdLabel,
              let arguments = propertyList["ProgramArguments"] as? [String],
              arguments.count == 4,
              let programPath = arguments.first,
              arguments == [
                  programPath,
                  "--config",
                  configuration.configURL.path,
                  "daemon",
              ],
              let environment = propertyList["EnvironmentVariables"] as? [String: String],
              environment["THREADRELAY_HOME"]
                  == configuration.configURL.deletingLastPathComponent().path,
              environment["THREADRELAY_BUNDLE_BUILD"]
                  == URL(fileURLWithPath: programPath).deletingLastPathComponent().lastPathComponent,
              environment[DaemonLaunchConfiguration.skipDesktopIntegrationEnvironment]
                  == configuration.desktopIntegrationEnvironmentValue
        else {
            throw DaemonLaunchError.launchAgentSnapshotUnavailable
        }

        let programURL = URL(fileURLWithPath: programPath)
        guard isManagedRuntimeProgram(
            programURL,
            configuration: configuration,
            fileManager: fileManager
        ) else {
            throw DaemonLaunchError.loadedAgentUntrusted(programPath)
        }
        guard loadedProgram(from: loadedAgentOutput) == programPath,
              loadedArguments(from: loadedAgentOutput) == arguments,
              loadedEnvironmentMatches(
                  output: loadedAgentOutput,
                  expected: environment
              )
        else {
            throw DaemonLaunchError.loadedAgentUntrusted(
                loadedProgram(from: loadedAgentOutput)
            )
        }

        return RuntimeSnapshot(
            launchAgentData: data,
            programURL: programURL,
            build: environment["THREADRELAY_BUNDLE_BUILD"] ?? ""
        )
    }

    private static func loadedAgentMatches(
        output: String,
        launchAgentData: Data,
        configuration: DaemonLaunchConfiguration
    ) -> Bool {
        guard let propertyList = try? PropertyListSerialization.propertyList(
            from: launchAgentData,
            options: [],
            format: nil
        ) as? [String: Any],
        let arguments = propertyList["ProgramArguments"] as? [String],
        let programPath = arguments.first,
        let environment = propertyList["EnvironmentVariables"] as? [String: String]
        else {
            return false
        }
        return loadedProgram(from: output) == programPath
            && loadedArguments(from: output) == arguments
            && loadedEnvironmentMatches(output: output, expected: environment)
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
        guard resolvedProgram.lastPathComponent == "threadrelay-daemon",
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

    private static func stageRuntime(
        configuration: DaemonLaunchConfiguration,
        fileManager: FileManager,
        commandRunner: @Sendable (URL, [String]) throws -> CommandResult
    ) throws -> URL {
        let expectedBuild = try configuration.resolvedBuildIdentifier()
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
            ".threadrelay-daemon.\(UUID().uuidString).tmp"
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
        return destination
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
        guard line.hasPrefix("threadrelay "),
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
        guard let stagedHelperURL = try? configuration.stagedHelperURL(),
              let expectedBuild = try? configuration.resolvedBuildIdentifier(),
              let program = loadedProgram(from: output),
              program == stagedHelperURL.path
        else {
            return false
        }
        guard loadedEnvironmentValue(from: output, key: "THREADRELAY_HOME")
            == configuration.configURL.deletingLastPathComponent().path,
            loadedEnvironmentValue(from: output, key: "THREADRELAY_BUNDLE_BUILD") == expectedBuild,
            loadedEnvironmentValue(
                from: output,
                key: DaemonLaunchConfiguration.skipDesktopIntegrationEnvironment
            ) == configuration.desktopIntegrationEnvironmentValue,
            loadedEnvironmentValue(from: output, key: "THREADRELAY_RUNTIME_SWITCH_HOLD") == nil
        else {
            return false
        }
        guard let argumentsStart = lines.firstIndex(where: { $0 == "arguments = {" }),
              let argumentsEnd = lines[(argumentsStart + 1)...].firstIndex(of: "}") else {
            return false
        }
        let arguments = lines[(argumentsStart + 1)..<argumentsEnd]
            .filter { !$0.isEmpty }
            .map { unquote($0) }
        return arguments == [
            stagedHelperURL.path,
            "--config",
            configuration.configURL.path,
            "daemon",
        ]
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

    private static func loadedEnvironmentMatches(
        output: String,
        expected: [String: String]
    ) -> Bool {
        guard expected.allSatisfy({ key, value in
            loadedEnvironmentValue(from: output, key: key) == value
        }) else {
            return false
        }
        for optionalKey in [
            DaemonLaunchConfiguration.skipDesktopIntegrationEnvironment,
            "THREADRELAY_RUNTIME_SWITCH_HOLD",
        ] {
            let actualValue = loadedEnvironmentValue(from: output, key: optionalKey)
            guard actualValue == expected[optionalKey] else {
                return false
            }
        }
        return true
    }

    private static func launchAgentHasRuntimeSwitchHold(_ data: Data) -> Bool {
        guard let propertyList = try? PropertyListSerialization.propertyList(
            from: data,
            options: [],
            format: nil
        ) as? [String: Any],
        let environment = propertyList["EnvironmentVariables"] as? [String: String]
        else {
            return false
        }
        return environment["THREADRELAY_RUNTIME_SWITCH_HOLD"] == "1"
    }

    private static func unquote(_ value: String) -> String {
        guard value.count >= 2, value.first == "\"", value.last == "\"" else {
            return value
        }
        return String(value.dropFirst().dropLast())
    }

    private struct LaunchAgentIdentity {
        let programPath: String
        let build: String
        let arguments: [String]
        let environment: [String: String]
    }

    private static func runtimeSwitchJournalURL(
        configuration: DaemonLaunchConfiguration
    ) -> URL {
        configuration.configURL.deletingLastPathComponent()
            .appendingPathComponent("threadrelay-runtime-switch.json")
    }

    private static func runtimeSwitchLockURL(
        configuration: DaemonLaunchConfiguration
    ) -> URL {
        configuration.configURL.deletingLastPathComponent()
            .appendingPathComponent("threadrelay-runtime-switch.lock")
    }

    private static func currentTimeMilliseconds() -> Int64 {
        Int64(Date().timeIntervalSince1970 * 1_000)
    }

    private static func canonicalPath(_ path: String) -> String {
        URL(fileURLWithPath: path).standardizedFileURL.resolvingSymlinksInPath().path
    }

    private static func acquireRuntimeSwitchLock(
        configuration: DaemonLaunchConfiguration,
        fileManager: FileManager
    ) throws -> DaemonRuntimeSwitchLock {
        try DaemonRuntimeSwitchLock(
            url: runtimeSwitchLockURL(configuration: configuration),
            fileManager: fileManager
        )
    }

    private static func withRuntimeSwitchLock<T>(
        configuration: DaemonLaunchConfiguration,
        operation: () throws -> T
    ) throws -> T {
        let lock = try acquireRuntimeSwitchLock(
            configuration: configuration,
            fileManager: .default
        )
        defer { lock.release() }
        return try operation()
    }

    private static func requireNoPendingRuntimeSwitch(
        configuration: DaemonLaunchConfiguration
    ) throws {
        if FileManager.default.fileExists(
            atPath: runtimeSwitchJournalURL(configuration: configuration).path
        ) {
            throw DaemonLaunchError.runtimeSwitchRecoveryRequired
        }
    }

    private static func launchAgentIdentity(
        from data: Data,
        configuration: DaemonLaunchConfiguration
    ) throws -> LaunchAgentIdentity {
        guard let propertyList = try? PropertyListSerialization.propertyList(
            from: data,
            options: [],
            format: nil
        ) as? [String: Any],
        propertyList["Label"] as? String == configuration.launchdLabel,
        let arguments = propertyList["ProgramArguments"] as? [String],
        arguments.count == 4,
        let programPath = arguments.first,
        arguments == [
            programPath,
            "--config",
            configuration.configURL.path,
            "daemon",
        ],
        let environment = propertyList["EnvironmentVariables"] as? [String: String],
        environment["HOME"] == configuration.homeURL.path,
        environment["PATH"] == "/usr/bin:/bin:/usr/sbin:/sbin",
        environment["THREADRELAY_HOME"]
            == configuration.configURL.deletingLastPathComponent().path,
        environment[DaemonLaunchConfiguration.skipDesktopIntegrationEnvironment]
            == configuration.desktopIntegrationEnvironmentValue,
        let build = environment["THREADRELAY_BUNDLE_BUILD"],
        build == URL(fileURLWithPath: programPath).deletingLastPathComponent().lastPathComponent,
        Set(environment.keys).subtracting([
            "HOME",
            "PATH",
            "THREADRELAY_HOME",
            "THREADRELAY_BUNDLE_BUILD",
            DaemonLaunchConfiguration.skipDesktopIntegrationEnvironment,
            "THREADRELAY_RUNTIME_SWITCH_HOLD",
        ]).isEmpty,
        environment["THREADRELAY_RUNTIME_SWITCH_HOLD"] == nil
            || environment["THREADRELAY_RUNTIME_SWITCH_HOLD"] == "1"
        else {
            throw DaemonLaunchError.runtimeSwitchRecoveryRequired
        }
        guard isManagedRuntimeProgram(
            URL(fileURLWithPath: programPath),
            configuration: configuration,
            fileManager: .default
        ) else {
            throw DaemonLaunchError.loadedAgentUntrusted(programPath)
        }
        return LaunchAgentIdentity(
            programPath: programPath,
            build: build,
            arguments: arguments,
            environment: environment
        )
    }

    private static func validateRuntimeSwitchJournal(
        _ journal: DaemonRuntimeSwitchJournal,
        configuration: DaemonLaunchConfiguration
    ) throws {
        guard journal.schemaVersion == 1,
              !journal.transactionId.isEmpty,
              !journal.previousInstanceId.isEmpty,
              journal.previousPID > 0
        else {
            throw DaemonLaunchError.runtimeSwitchRecoveryRequired
        }
        let previous = try launchAgentIdentity(
            from: journal.previousLaunchAgentData,
            configuration: configuration
        )
        let candidate = try launchAgentIdentity(
            from: journal.candidateLaunchAgentData,
            configuration: configuration
        )
        let expectedCandidatePath = try configuration.stagedHelperURL().path
        let expectedCandidateBuild = try configuration.resolvedBuildIdentifier()
        guard canonicalPath(previous.programPath) == canonicalPath(journal.previousProgramPath),
              previous.build == journal.previousBuild,
              canonicalPath(candidate.programPath) == canonicalPath(journal.candidateProgramPath),
              candidate.build == journal.candidateBuild,
              canonicalPath(candidate.programPath) == canonicalPath(expectedCandidatePath),
              candidate.build == expectedCandidateBuild,
              previous.environment["THREADRELAY_RUNTIME_SWITCH_HOLD"] == nil,
              candidate.environment["THREADRELAY_RUNTIME_SWITCH_HOLD"] == "1"
        else {
            throw DaemonLaunchError.runtimeSwitchRecoveryRequired
        }
    }

    private static func validatedTransaction(
        _ transaction: DaemonRuntimeSwitch,
        configuration: DaemonLaunchConfiguration
    ) throws -> DaemonRuntimeSwitchJournal {
        let journalURL = runtimeSwitchJournalURL(configuration: configuration)
        guard let data = try? Data(contentsOf: journalURL),
              let persisted = try? JSONDecoder().decode(
                  DaemonRuntimeSwitchJournal.self,
                  from: data
              ),
              persisted == transaction.journal
        else {
            throw DaemonLaunchError.runtimeSwitchRecoveryRequired
        }
        try validateRuntimeSwitchJournal(persisted, configuration: configuration)
        return persisted
    }

    private static func persistRuntimeSwitchJournal(
        _ journal: DaemonRuntimeSwitchJournal,
        configuration: DaemonLaunchConfiguration
    ) throws {
        let fileManager = FileManager.default
        let destination = runtimeSwitchJournalURL(configuration: configuration)
        let directory = destination.deletingLastPathComponent()
        let temporary = directory.appendingPathComponent(
            ".threadrelay-runtime-switch.\(UUID().uuidString).tmp"
        )
        defer { try? fileManager.removeItem(at: temporary) }
        do {
            try fileManager.createDirectory(at: directory, withIntermediateDirectories: true)
            let encoder = JSONEncoder()
            encoder.outputFormatting = [.sortedKeys]
            try encoder.encode(journal).write(to: temporary)
            try fileManager.setAttributes(
                [.posixPermissions: 0o600],
                ofItemAtPath: temporary.path
            )
            let handle = try FileHandle(forWritingTo: temporary)
            try handle.synchronize()
            try handle.close()
            let renameResult = temporary.path.withCString { temporaryPath in
                destination.path.withCString { destinationPath in
                    Darwin.rename(temporaryPath, destinationPath)
                }
            }
            guard renameResult == 0 else {
                throw DaemonLaunchError.runtimeSwitchJournalFailed
            }
            synchronizeDirectory(directory)
        } catch let error as DaemonLaunchError {
            throw error
        } catch {
            throw DaemonLaunchError.runtimeSwitchJournalFailed
        }
    }

    private static func removeRuntimeSwitchJournal(
        configuration: DaemonLaunchConfiguration
    ) throws {
        let url = runtimeSwitchJournalURL(configuration: configuration)
        guard unlink(url.path) == 0 || errno == ENOENT else {
            throw DaemonLaunchError.runtimeSwitchJournalFailed
        }
        synchronizeDirectory(url.deletingLastPathComponent())
    }

    private static func synchronizeDirectory(_ directory: URL) {
        let descriptor = directory.path.withCString {
            Darwin.open($0, O_RDONLY | O_CLOEXEC)
        }
        guard descriptor >= 0 else { return }
        _ = Darwin.fsync(descriptor)
        Darwin.close(descriptor)
    }

    private static func updateRuntimeSwitchPhase(
        _ phase: DaemonRuntimeSwitchPhase,
        transaction: DaemonRuntimeSwitch,
        configuration: DaemonLaunchConfiguration
    ) throws {
        var journal = try validatedTransaction(transaction, configuration: configuration)
        journal.phase = phase
        journal.updatedAtMilliseconds = currentTimeMilliseconds()
        try persistRuntimeSwitchJournal(journal, configuration: configuration)
        transaction.updateJournal(journal)
    }

    private static func requireLoadedAgent(
        _ printResult: CommandResult,
        launchAgentData: Data,
        expectedPID: Int32,
        configuration: DaemonLaunchConfiguration
    ) throws {
        let actualPID = loadedPID(from: printResult.output)
        guard printResult.exitCode == 0, actualPID == expectedPID else {
            throw DaemonLaunchError.daemonProcessChanged(
                expected: expectedPID,
                actual: actualPID
            )
        }
        guard loadedAgentMatches(
            output: printResult.output,
            launchAgentData: launchAgentData,
            configuration: configuration
        ) else {
            throw DaemonLaunchError.loadedAgentUntrusted(
                loadedProgram(from: printResult.output)
            )
        }
    }

    private static func freezeAndUnloadAgent(
        expectedPID: Int32,
        expectedLaunchAgentData: Data,
        configuration: DaemonLaunchConfiguration,
        launchctl: URL,
        serviceTarget: String,
        commandRunner: @escaping @Sendable (URL, [String]) throws -> CommandResult,
        processSignaler: @escaping @Sendable (Int32, Int32) -> Int32
    ) throws {
        let beforeFreeze = try commandRunner(launchctl, ["print", serviceTarget])
        try requireLoadedAgent(
            beforeFreeze,
            launchAgentData: expectedLaunchAgentData,
            expectedPID: expectedPID,
            configuration: configuration
        )
        guard processSignaler(expectedPID, SIGSTOP) == 0 else {
            throw DaemonLaunchError.daemonFreezeFailed(expectedPID)
        }
        let resumeIfStillLoaded = {
            guard let current = try? commandRunner(launchctl, ["print", serviceTarget]),
                  current.exitCode == 0,
                  loadedPID(from: current.output) == expectedPID,
                  loadedAgentMatches(
                      output: current.output,
                      launchAgentData: expectedLaunchAgentData,
                      configuration: configuration
                  )
            else { return }
            _ = processSignaler(expectedPID, SIGCONT)
        }
        var shouldResume = true
        defer {
            if shouldResume {
                resumeIfStillLoaded()
            }
        }

        let frozen = try commandRunner(launchctl, ["print", serviceTarget])
        try requireLoadedAgent(
            frozen,
            launchAgentData: expectedLaunchAgentData,
            expectedPID: expectedPID,
            configuration: configuration
        )

        let bootout = try commandRunner(launchctl, ["bootout", serviceTarget])
        if bootout.exitCode != 0 {
            let afterBootout = try commandRunner(launchctl, ["print", serviceTarget])
            if afterBootout.exitCode == 0 {
                try requireLoadedAgent(
                    afterBootout,
                    launchAgentData: expectedLaunchAgentData,
                    expectedPID: expectedPID,
                    configuration: configuration
                )
                throw DaemonLaunchError.launchctlFailed(lastLine(of: bootout.output))
            }
            shouldResume = false
            return
        }

        for attempt in 0..<20 {
            let result = try commandRunner(launchctl, ["print", serviceTarget])
            if result.exitCode != 0 {
                shouldResume = false
                return
            }
            if attempt < 19 {
                Thread.sleep(forTimeInterval: 0.05)
            }
        }
        throw DaemonLaunchError.launchctlFailed("后台服务未在预期时间内停止。")
    }

    private static func unloadHeldCandidateAgent(
        expectedLaunchAgentData: Data,
        configuration: DaemonLaunchConfiguration,
        launchctl: URL,
        serviceTarget: String,
        commandRunner: @escaping @Sendable (URL, [String]) throws -> CommandResult
    ) throws {
        let current = try commandRunner(launchctl, ["print", serviceTarget])
        guard current.exitCode == 0 else { return }
        guard loadedAgentMatches(
            output: current.output,
            launchAgentData: expectedLaunchAgentData,
            configuration: configuration
        ) else {
            throw DaemonLaunchError.loadedAgentUntrusted(loadedProgram(from: current.output))
        }
        try unloadAgent(
            launchctl: launchctl,
            serviceTarget: serviceTarget,
            commandRunner: commandRunner
        )
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
        } catch {
            throw DaemonLaunchError.launchAgentWriteFailed
        }
    }

    private static func writeLaunchAgentData(
        _ data: Data,
        to launchAgentURL: URL,
        logURL: URL,
        fileManager: FileManager
    ) throws {
        do {
            try fileManager.createDirectory(
                at: launchAgentURL.deletingLastPathComponent(),
                withIntermediateDirectories: true
            )
            try fileManager.createDirectory(
                at: logURL.deletingLastPathComponent(),
                withIntermediateDirectories: true
            )
            try data.write(to: launchAgentURL, options: .atomic)
        } catch {
            throw DaemonLaunchError.launchAgentWriteFailed
        }
    }

    private static func unloadAgent(
        launchctl: URL,
        serviceTarget: String,
        commandRunner: @Sendable (URL, [String]) throws -> CommandResult
    ) throws {
        let bootout = try commandRunner(launchctl, ["bootout", serviceTarget])
        if bootout.exitCode != 0 {
            let afterBootout = try commandRunner(launchctl, ["print", serviceTarget])
            guard afterBootout.exitCode != 0 else {
                throw DaemonLaunchError.launchctlFailed(lastLine(of: bootout.output))
            }
            return
        }

        for attempt in 0..<20 {
            let result = try commandRunner(launchctl, ["print", serviceTarget])
            if result.exitCode != 0 {
                return
            }
            if attempt < 19 {
                Thread.sleep(forTimeInterval: 0.05)
            }
        }
        throw DaemonLaunchError.launchctlFailed("后台服务未在预期时间内停止。")
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

    private static func lastLine(of output: String) -> String {
        output
            .split(whereSeparator: \.isNewline)
            .last
            .map(String.init) ?? ""
    }
}

struct GUIRecoveryLauncher: @unchecked Sendable {
    private let configurationLoader: @Sendable () throws -> GUIRecoveryConfiguration
    private let commandRunner: @Sendable (URL, [String]) throws -> CommandResult

    init(
        configurationLoader: @escaping @Sendable () throws -> GUIRecoveryConfiguration = {
            try .current()
        },
        commandRunner: @escaping @Sendable (URL, [String]) throws -> CommandResult = Self.runCommand
    ) {
        self.configurationLoader = configurationLoader
        self.commandRunner = commandRunner
    }

    func startIfNeeded() throws {
        let configuration = try configurationLoader()
        let fileManager = FileManager.default
        var isDirectory: ObjCBool = false
        guard fileManager.fileExists(
            atPath: configuration.executableURL.path,
            isDirectory: &isDirectory
        ), !isDirectory.boolValue else {
            throw DaemonLaunchError.guiExecutableMissing
        }
        guard fileManager.isExecutableFile(atPath: configuration.executableURL.path) else {
            throw DaemonLaunchError.guiExecutableNotExecutable
        }
        var supervisorIsDirectory: ObjCBool = false
        guard fileManager.fileExists(
            atPath: configuration.supervisorURL.path,
            isDirectory: &supervisorIsDirectory
        ), !supervisorIsDirectory.boolValue else {
            throw DaemonLaunchError.guiSupervisorMissing
        }
        guard fileManager.isExecutableFile(atPath: configuration.supervisorURL.path) else {
            throw DaemonLaunchError.guiSupervisorNotExecutable
        }

        let launchctl = URL(fileURLWithPath: "/bin/launchctl")
        let domain = "gui/\(getuid())"
        let serviceTarget = "\(domain)/\(GUIRecoveryConfiguration.label)"
        let printResult = try commandRunner(launchctl, ["print", serviceTarget])

        if printResult.exitCode == 0,
           Self.loadedAgentMatches(output: printResult.output, configuration: configuration)
        {
            try Self.writeLaunchAgent(configuration, fileManager: fileManager)
            if printResult.output.contains("state = running") {
                return
            }
            let kickstart = try commandRunner(launchctl, ["kickstart", serviceTarget])
            guard kickstart.exitCode == 0 else {
                throw DaemonLaunchError.launchctlFailed(Self.lastLine(of: kickstart.output))
            }
            return
        }

        // A stale job may still point at a previous bundle. Remove it before
        // installing the active bundle's path so the next crash cannot revive
        // an older copy.
        if printResult.exitCode == 0 {
            guard Self.loadedProgram(from: printResult.output) == configuration.supervisorURL.path else {
                throw DaemonLaunchError.loadedAgentMismatch(
                    expected: configuration.supervisorURL.path,
                    actual: Self.loadedProgram(from: printResult.output)
                )
            }
            try Self.unloadAgent(
                launchctl: launchctl,
                serviceTarget: serviceTarget,
                commandRunner: commandRunner
            )
        }

        try Self.writeLaunchAgent(configuration, fileManager: fileManager)
        let result = try commandRunner(
            launchctl,
            ["bootstrap", domain, configuration.launchAgentURL.path]
        )
        guard result.exitCode == 0 else {
            throw DaemonLaunchError.launchctlFailed(Self.lastLine(of: result.output))
        }
    }

    private static func loadedAgentMatches(
        output: String,
        configuration: GUIRecoveryConfiguration
    ) -> Bool {
        let lines = output.split(whereSeparator: \.isNewline).map {
            $0.trimmingCharacters(in: .whitespacesAndNewlines)
        }
        guard let program = lines
            .first(where: { $0.hasPrefix("program = ") })
            .map({ String($0.dropFirst("program = ".count)) })
            .map(unquote),
            program == configuration.supervisorURL.path
        else {
            return false
        }
        if let buildIdentifier = configuration.buildIdentifier {
            guard loadedEnvironmentValue(from: output, key: "THREADRELAY_HOME")
                == configuration.dataDirectoryURL.path,
                loadedEnvironmentValue(from: output, key: "THREADRELAY_BUNDLE_BUILD") == buildIdentifier
            else {
                return false
            }
        }
        guard let argumentsStart = lines.firstIndex(where: { $0 == "arguments = {" }),
              let argumentsEnd = lines[(argumentsStart + 1)...].firstIndex(of: "}") else {
            return false
        }
        let arguments = lines[(argumentsStart + 1)..<argumentsEnd]
            .filter { !$0.isEmpty }
            .map(unquote)
        return arguments == [configuration.supervisorURL.path]
    }

    private static func loadedProgram(from output: String) -> String? {
        output
            .split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .first(where: { $0.hasPrefix("program = ") })
            .map { String($0.dropFirst("program = ".count)) }
            .map(unquote)
    }

    private static func unloadAgent(
        launchctl: URL,
        serviceTarget: String,
        commandRunner: @Sendable (URL, [String]) throws -> CommandResult
    ) throws {
        let bootout = try commandRunner(launchctl, ["bootout", serviceTarget])
        if bootout.exitCode != 0 {
            let afterBootout = try commandRunner(launchctl, ["print", serviceTarget])
            if afterBootout.exitCode == 0 {
                throw DaemonLaunchError.launchctlFailed(Self.lastLine(of: bootout.output))
            }
            return
        }
        Self.waitForAgentToDisappear(
            launchctl: launchctl,
            serviceTarget: serviceTarget,
            commandRunner: commandRunner
        )
    }

    private static func waitForAgentToDisappear(
        launchctl: URL,
        serviceTarget: String,
        commandRunner: @Sendable (URL, [String]) throws -> CommandResult
    ) {
        for attempt in 0..<10 {
            if let result = try? commandRunner(launchctl, ["print", serviceTarget]), result.exitCode != 0 {
                return
            }
            if attempt < 9 {
                Thread.sleep(forTimeInterval: 0.1)
            }
        }
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

    private static func writeLaunchAgent(
        _ configuration: GUIRecoveryConfiguration,
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
