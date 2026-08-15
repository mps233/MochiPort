import Darwin
import Foundation

enum DaemonLaunchError: LocalizedError, Equatable {
    case helperMissing
    case helperNotExecutable
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
    case launchctlFailed(String)

    var errorDescription: String? {
        switch self {
        case .helperMissing:
            return "应用内未找到后台服务。请重新安装 ThreadRelay。"
        case .helperNotExecutable:
            return "应用内的后台服务不可执行。请重新安装 ThreadRelay。"
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
        case let .loadedAgentMismatch(expected, actual):
            let actualDescription = actual.map { "当前为 \($0)" } ?? "当前路径未知"
            return "后台服务启动配置指向了其他版本（应为 \(expected)，\(actualDescription)）。请重新安装 ThreadRelay 后重试。"
        case let .launchctlFailed(detail):
            return detail.isEmpty ? "无法启动后台服务。" : "无法启动后台服务：\(detail)"
        }
    }
}

struct DaemonLaunchConfiguration: Equatable {
    static let label = "io.github.mps233.threadrelay.daemon"

    let helperURL: URL
    let configURL: URL
    let launchAgentURL: URL
    let logURL: URL
    let homeURL: URL
    let buildIdentifier: String?

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

    func propertyListData() throws -> Data {
        let resolvedBuildIdentifier = try resolvedBuildIdentifier()
        let stagedHelperURL = try stagedHelperURL()
        var environment: [String: String] = [
            "HOME": homeURL.path,
            "PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
            "THREADRELAY_HOME": configURL.deletingLastPathComponent().path,
        ]
        environment["THREADRELAY_BUNDLE_BUILD"] = resolvedBuildIdentifier
        let propertyList: [String: Any] = [
            "Label": Self.label,
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

protocol DaemonLaunching: Sendable {
    /// Prepare the embedded daemon for a future launch without touching the
    /// currently loaded LaunchAgent or running process.
    func prepareRuntime() async throws
    func startIfNeeded() async throws
}

extension DaemonLaunching {
    func prepareRuntime() async throws {}
}

struct DaemonLauncher: DaemonLaunching, @unchecked Sendable {
    private let configurationLoader: @Sendable () throws -> DaemonLaunchConfiguration
    private let commandRunner: @Sendable (URL, [String]) throws -> CommandResult

    init(
        configurationLoader: @escaping @Sendable () throws -> DaemonLaunchConfiguration = {
            try .current()
        },
        commandRunner: @escaping @Sendable (URL, [String]) throws -> CommandResult = Self.runCommand
    ) {
        self.configurationLoader = configurationLoader
        self.commandRunner = commandRunner
    }

    func prepareRuntime() async throws {
        let configurationLoader = configurationLoader
        let commandRunner = commandRunner
        try await Task.detached(priority: .userInitiated) {
            let configuration = try configurationLoader()
            try Self.prepareRuntime(
                configuration: configuration,
                commandRunner: commandRunner
            )
        }.value
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

    private static func installAndStart(
        configuration: DaemonLaunchConfiguration,
        commandRunner: @Sendable (URL, [String]) throws -> CommandResult
    ) throws {
        let fileManager = FileManager.default
        try prepareRuntime(configuration: configuration, commandRunner: commandRunner)

        let launchctl = URL(fileURLWithPath: "/bin/launchctl")
        let domain = "gui/\(getuid())"
        let serviceTarget = "\(domain)/\(DaemonLaunchConfiguration.label)"
        let printResult = try commandRunner(launchctl, ["print", serviceTarget])

        try writeLaunchAgent(configuration, fileManager: fileManager)
        if printResult.exitCode == 0 {
            // Staging prepares the next launch only. An already loaded job may
            // still be serving protected work from an older path or build, so
            // this path must never bootout, kickstart, or bootstrap it.
            return
        }
        let result = try commandRunner(
            launchctl,
            ["bootstrap", domain, configuration.launchAgentURL.path]
        )
        guard result.exitCode == 0 else {
            throw DaemonLaunchError.launchctlFailed(lastLine(of: result.output))
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
            loadedEnvironmentValue(from: output, key: "THREADRELAY_BUNDLE_BUILD") == expectedBuild
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

    private static func unquote(_ value: String) -> String {
        guard value.count >= 2, value.first == "\"", value.last == "\"" else {
            return value
        }
        return String(value.dropFirst().dropLast())
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
