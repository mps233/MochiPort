import CryptoKit
import Darwin
import Foundation

enum MochiPortStorage {
    static let homeEnvironmentKeys = ["MOCHIPORT_HOME", "THREADRELAY_HOME", "CODEXHUB_HOME"]
    static let applicationSupportDirectoryNames = ["MochiPort", "ThreadRelay", "CodexHub"]

    static func dataDirectory(
        environment: [String: String] = ProcessInfo.processInfo.environment,
        applicationSupport: URL,
        fileManager: FileManager = .default
    ) -> URL {
        for key in homeEnvironmentKeys {
            if let path = environment[key], !path.isEmpty {
                return URL(fileURLWithPath: path, isDirectory: true)
            }
        }

        let defaults = applicationSupportDirectoryNames.map {
            applicationSupport.appendingPathComponent($0, isDirectory: true)
        }
        return defaults.first {
            fileManager.fileExists(atPath: $0.appendingPathComponent("config.toml").path)
        } ?? defaults[0]
    }

    static func candidateDirectories(
        environment: [String: String] = ProcessInfo.processInfo.environment,
        applicationSupport: URL?
    ) -> [URL] {
        var candidates = homeEnvironmentKeys.compactMap { key -> URL? in
            guard let path = environment[key], !path.isEmpty else { return nil }
            return URL(fileURLWithPath: path, isDirectory: true)
        }
        if let applicationSupport {
            candidates.append(contentsOf: applicationSupportDirectoryNames.map {
                applicationSupport.appendingPathComponent($0, isDirectory: true)
            })
        }

        var seen = Set<String>()
        return candidates.filter {
            seen.insert($0.standardizedFileURL.path).inserted
        }
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
    case guiExecutableMissing
    case guiExecutableNotExecutable
    case guiSupervisorMissing
    case guiSupervisorNotExecutable
    case launchAgentDirectoryUnavailable
    case launchAgentWriteFailed
    case loadedAgentMismatch(expected: String, actual: String?)
    case loadedAgentUntrusted(String?)
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
        case .guiExecutableMissing:
            return "应用内未找到 MochiPort 界面程序。请重新安装 MochiPort。"
        case .guiExecutableNotExecutable:
            return "应用内的 MochiPort 界面程序不可执行。请重新安装 MochiPort。"
        case .guiSupervisorMissing:
            return "应用内未找到 MochiPort 自动恢复服务。请重新安装 MochiPort。"
        case .guiSupervisorNotExecutable:
            return "应用内的 MochiPort 自动恢复服务不可执行。请重新安装 MochiPort。"
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
        case let .launchctlFailed(detail):
            return detail.isEmpty ? "无法启动后台服务。" : "无法启动后台服务：\(detail)"
        }
    }
}

struct DaemonLaunchConfiguration: Equatable {
    static let label = "io.github.mps233.threadrelay.daemon"
    fileprivate static let skipDesktopIntegrationEnvironments = [
        "MOCHIPORT_SKIP_DESKTOP_INTEGRATION",
        "THREADRELAY_SKIP_DESKTOP_INTEGRATION",
    ]
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

        let dataDirectory = MochiPortStorage.dataDirectory(
            environment: environment,
            applicationSupport: applicationSupport,
            fileManager: fileManager
        )
        let helpers = bundleURL
            .appendingPathComponent("Contents", isDirectory: true)
            .appendingPathComponent("Helpers", isDirectory: true)
        let mochiPortHelper = helpers.appendingPathComponent("mochiport-daemon")
        let embeddedHelper = fileManager.fileExists(atPath: mochiPortHelper.path)
            ? mochiPortHelper
            : helpers.appendingPathComponent("threadrelay-daemon")

        return Self(
            helperURL: embeddedHelper,
            configURL: dataDirectory.appendingPathComponent("config.toml"),
            launchAgentURL: homeURL
                .appendingPathComponent("Library/LaunchAgents", isDirectory: true)
                .appendingPathComponent("\(label).plist"),
            logURL: dataDirectory
                .appendingPathComponent("logs", isDirectory: true)
                .appendingPathComponent("threadrelay-daemon-launchd.log"),
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
        let resolvedBuildIdentifier = try resolvedBuildIdentifier()
        let activeHelperURL = activeHelperURL()
        var environment: [String: String] = [
            "HOME": homeURL.path,
            "PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
            "MOCHIPORT_HOME": configURL.deletingLastPathComponent().path,
            "THREADRELAY_HOME": configURL.deletingLastPathComponent().path,
            "MOCHIPORT_BUNDLE_BUILD": resolvedBuildIdentifier,
            "THREADRELAY_BUNDLE_BUILD": resolvedBuildIdentifier,
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
        let mochiPortSupervisor = helpers.appendingPathComponent("mochiport-gui-supervisor")
        let supervisor = fileManager.fileExists(atPath: mochiPortSupervisor.path)
            ? mochiPortSupervisor
            : helpers.appendingPathComponent("threadrelay-gui-supervisor")
        return Self(
            executableURL: contents.appendingPathComponent("MacOS/MochiPort"),
            supervisorURL: supervisor,
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
            "MOCHIPORT_HOME": dataDirectoryURL.path,
            "THREADRELAY_HOME": dataDirectoryURL.path,
        ]
        if let buildIdentifier {
            environment["MOCHIPORT_BUNDLE_BUILD"] = buildIdentifier
            environment["THREADRELAY_BUNDLE_BUILD"] = buildIdentifier
        }
        let propertyList: [String: Any] = [
            "Label": Self.label,
            "ProgramArguments": [supervisorURL.path],
            "EnvironmentVariables": environment,
            "RunAtLoad": true,
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

protocol DaemonLaunching: Sendable {
    func startIfNeeded() async throws
    func verifiedDaemonIdentity(for lifecycle: ManageLifecycle) async throws -> ManageDaemonIdentity
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

    func startIfNeeded() async throws {
        let configurationLoader = configurationLoader
        let commandRunner = commandRunner
        try await Task.detached(priority: .userInitiated) {
            let configuration = try configurationLoader()
            try Self.installAndStart(
                configuration: configuration,
                commandRunner: commandRunner
            )
        }.value
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

    private static func verifyDaemonIdentity(
        _ lifecycle: ManageLifecycle,
        configuration: DaemonLaunchConfiguration,
        locator: ActiveDaemonLocator,
        commandRunner: @Sendable (URL, [String]) throws -> CommandResult,
        fileManager: FileManager = .default
    ) throws -> ManageDaemonIdentity {
        guard lifecycle.service.service == "threadrelay",
              lifecycle.service.apiMajor == 1,
              let expectedPID = Int32(exactly: lifecycle.service.pid),
              expectedPID > 0,
              lifecycle.service.startedAtMs >= 0,
              pathsMatch(lifecycle.configPath, configuration.configURL.path),
              locator.service == "threadrelay",
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
        ) else {
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
    ) throws {
        _ = try configuration.resolvedBuildIdentifier()
        let launchctl = URL(fileURLWithPath: "/bin/launchctl")
        let printResult = try commandRunner(
            launchctl,
            ["print", configuration.launchdServiceTarget]
        )
        guard printResult.exitCode != 0 else {
            return
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
    }

    private static func stageRuntime(
        configuration: DaemonLaunchConfiguration,
        fileManager: FileManager,
        commandRunner: @Sendable (URL, [String]) throws -> CommandResult
    ) throws {
        let expectedBuild = try configuration.resolvedBuildIdentifier()
        if try preserveExistingActiveRuntime(
            configuration: configuration,
            expectedBuild: expectedBuild,
            fileManager: fileManager,
            commandRunner: commandRunner
        ) {
            return
        }

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
            buildIdentifier: expectedBuild,
            fileManager: fileManager
        )
    }

    /// Keep a daemon installed by an independent daemon update. A later UI
    /// launch must not copy its older embedded helper over a newer active
    /// runtime merely because launchd is currently stopped.
    private static func preserveExistingActiveRuntime(
        configuration: DaemonLaunchConfiguration,
        expectedBuild: String,
        fileManager: FileManager,
        commandRunner: @Sendable (URL, [String]) throws -> CommandResult
    ) throws -> Bool {
        let runtimes = configuration.configURL
            .deletingLastPathComponent()
            .appendingPathComponent("runtimes", isDirectory: true)
        let active = runtimes.appendingPathComponent("current", isDirectory: true)
        var info = stat()
        guard Darwin.lstat(active.path, &info) == 0 else {
            guard errno == ENOENT else { throw DaemonLaunchError.runtimeStageFailed }
            return false
        }
        guard info.st_mode & mode_t(S_IFMT) == mode_t(S_IFLNK) else {
            throw DaemonLaunchError.runtimeStageFailed
        }

        let target: String
        do {
            target = try fileManager.destinationOfSymbolicLink(atPath: active.path)
        } catch {
            throw DaemonLaunchError.runtimeStageFailed
        }
        guard isSafeRuntimeBuildIdentifier(target) else {
            throw DaemonLaunchError.runtimeStageFailed
        }

        let helper = active.appendingPathComponent(configuration.helperURL.lastPathComponent)
        try validateRuntimePermissions(at: helper, fileManager: fileManager)
        let versionResult: CommandResult
        do {
            versionResult = try commandRunner(helper, ["--version"])
        } catch {
            throw DaemonLaunchError.runtimeVersionMismatch(expected: target, actual: nil)
        }
        let actualBuild = daemonBuildIdentifier(fromVersionOutput: versionResult.output)
        guard versionResult.exitCode == 0, actualBuild == target else {
            throw DaemonLaunchError.runtimeVersionMismatch(
                expected: target,
                actual: actualBuild
            )
        }

        if target == expectedBuild {
            return true
        }
        if let targetNumber = Int(target),
           let expectedNumber = Int(expectedBuild),
           targetNumber > expectedNumber
        {
            return true
        }
        return false
    }

    private static func isSafeRuntimeBuildIdentifier(_ value: String) -> Bool {
        let allowed = CharacterSet(
            charactersIn: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-"
        )
        guard !value.isEmpty,
              value != ".",
              value != "..",
              value.rangeOfCharacter(from: allowed.inverted) == nil
        else {
            return false
        }
        return true
    }

    private static func activateRuntime(
        configuration: DaemonLaunchConfiguration,
        buildIdentifier: String,
        fileManager: FileManager
    ) throws {
        let runtimes = configuration.configURL
            .deletingLastPathComponent()
            .appendingPathComponent("runtimes", isDirectory: true)
        let active = runtimes.appendingPathComponent("current", isDirectory: true)
        let temporary = runtimes.appendingPathComponent(
            ".current.\(UUID().uuidString)",
            isDirectory: true
        )
        defer { try? fileManager.removeItem(at: temporary) }
        do {
            var info = stat()
            if Darwin.lstat(active.path, &info) == 0,
               info.st_mode & mode_t(S_IFMT) != mode_t(S_IFLNK)
            {
                throw DaemonLaunchError.runtimeStageFailed
            }
            let linkResult = buildIdentifier.withCString { destinationPath in
                temporary.path.withCString { linkPath in
                    Darwin.symlink(destinationPath, linkPath)
                }
            }
            guard linkResult == 0 else { throw DaemonLaunchError.runtimeStageFailed }
            let result = temporary.path.withCString { temporaryPath in
                active.path.withCString { activePath in
                    Darwin.rename(temporaryPath, activePath)
                }
            }
            guard result == 0 else { throw DaemonLaunchError.runtimeStageFailed }
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
        guard line.hasPrefix("mochiport ") || line.hasPrefix("threadrelay "),
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
        guard let expectedBuild = try? configuration.resolvedBuildIdentifier(),
              loadedProgram(from: output) == configuration.activeHelperURL().path,
              loadedHomeMatches(output, dataDirectory: configuration.configURL.deletingLastPathComponent()),
              loadedEnvironmentValue(
                  from: output,
                  keys: ["MOCHIPORT_BUNDLE_BUILD", "THREADRELAY_BUNDLE_BUILD"]
              ) == expectedBuild,
              loadedEnvironmentValue(
                  from: output,
                  keys: DaemonLaunchConfiguration.skipDesktopIntegrationEnvironments
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
        guard ["mochiport-daemon", "threadrelay-daemon"].contains(resolvedProgram.lastPathComponent),
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
        ["mochiport-control.json", "threadrelay-control.json"].contains {
            pathsMatch(path, dataDirectory.appendingPathComponent($0).path)
        }
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

    private static func loadedEnvironmentValue(from output: String, keys: [String]) -> String? {
        keys.lazy
            .compactMap { loadedEnvironmentValue(from: output, key: $0) }
            .first
    }

    private static func loadedHomeMatches(_ output: String, dataDirectory: URL) -> Bool {
        ["MOCHIPORT_HOME", "THREADRELAY_HOME", "CODEXHUB_HOME"].contains {
            loadedEnvironmentValue(from: output, key: $0) == dataDirectory.path
        }
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

        if printResult.exitCode == 0 {
            guard Self.loadedAgentHasSameIdentity(
                output: printResult.output,
                configuration: configuration
            ) else {
                let loadedProgram = Self.loadedProgram(from: printResult.output)
                if loadedProgram != configuration.supervisorURL.path {
                    throw DaemonLaunchError.loadedAgentMismatch(
                        expected: configuration.supervisorURL.path,
                        actual: loadedProgram
                    )
                }
                throw DaemonLaunchError.loadedAgentUntrusted(loadedProgram)
            }

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

        try Self.writeLaunchAgent(configuration, fileManager: fileManager)
        let result = try commandRunner(
            launchctl,
            ["bootstrap", domain, configuration.launchAgentURL.path]
        )
        guard result.exitCode == 0 else {
            throw DaemonLaunchError.launchctlFailed(Self.lastLine(of: result.output))
        }
    }

    private static func loadedAgentHasSameIdentity(
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
            isCompatibleSupervisorProgram(program, expected: configuration.supervisorURL),
            loadedEnvironmentValue(from: output, key: "HOME") == configuration.homeURL.path,
            loadedEnvironmentValue(from: output, key: "THREADRELAY_HOME")
            == configuration.dataDirectoryURL.path,
            let argumentsStart = lines.firstIndex(where: { $0 == "arguments = {" }),
            let argumentsEnd = lines[(argumentsStart + 1)...].firstIndex(of: "}")
        else {
            return false
        }
        let arguments = lines[(argumentsStart + 1)..<argumentsEnd]
            .filter { !$0.isEmpty }
            .map(unquote)
        return arguments.count == 1
            && isCompatibleSupervisorProgram(arguments[0], expected: configuration.supervisorURL)
    }

    private static func isCompatibleSupervisorProgram(
        _ program: String,
        expected: URL
    ) -> Bool {
        if program == expected.path { return true }
        let url = URL(fileURLWithPath: program).standardizedFileURL
        guard ["threadrelay-gui-supervisor", "mochiport-gui-supervisor"].contains(url.lastPathComponent) else {
            return false
        }
        let appURL = url
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let expectedAppURL = expected.standardizedFileURL
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        return ["ThreadRelay.app", "MochiPort.app"].contains(appURL.lastPathComponent)
            && appURL.deletingLastPathComponent() == expectedAppURL.deletingLastPathComponent()
    }

    private static func loadedProgram(from output: String) -> String? {
        output
            .split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .first(where: { $0.hasPrefix("program = ") })
            .map { String($0.dropFirst("program = ".count)) }
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
