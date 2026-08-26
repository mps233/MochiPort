import CryptoKit
import Darwin
import Foundation

struct PreparedDaemonUpdate: Equatable, Sendable {
    let version: String
    let build: Int
    let sha256: String
    let executableURL: URL
}

struct DaemonLaunchMigrationPlan: Sendable {
    enum Kind: Sendable {
        case stableEntryAlreadyLoaded
        case legacyEntryDisabled
    }

    let kind: Kind
    fileprivate let configuration: DaemonLaunchConfiguration
    fileprivate let originalPropertyList: Data
    fileprivate let updatedPropertyList: Data
}

protocol DaemonUpdateInstalling: Sendable {
    func prepare(release: UpdateComponentRelease) async throws -> PreparedDaemonUpdate
    func prepareLaunchAgentMigration(
        lifecycle: ManageLifecycle,
        candidate: PreparedDaemonUpdate
    ) async throws -> DaemonLaunchMigrationPlan
    func cancelLaunchAgentMigration(_ plan: DaemonLaunchMigrationPlan) async
    func recoverLaunchAgentMigration(
        _ plan: DaemonLaunchMigrationPlan,
        previousPID: Int
    ) async
    func completeLaunchAgentMigration(
        _ plan: DaemonLaunchMigrationPlan,
        previousPID: Int
    ) async throws
}

enum DaemonUpdateInstallerError: LocalizedError, Equatable {
    case invalidManifestAsset
    case invalidDownloadResponse
    case downloadSizeMismatch(expected: Int64, actual: Int64)
    case digestMismatch
    case notMachO
    case signatureInvalid
    case signingIdentityMismatch
    case versionMismatch(expected: String, actual: String?)
    case buildMismatch(expected: Int, actual: Int?)
    case runtimeDirectoryUnsafe
    case runtimeAlreadyExists
    case runtimeInstallFailed
    case launchAgentUntrusted
    case launchAgentMigrationFailed(String)
    case daemonDidNotExit

    var errorDescription: String? {
        switch self {
        case .invalidManifestAsset:
            "后台服务更新清单缺少可信下载地址、大小、SHA-256 或签名声明。"
        case .invalidDownloadResponse:
            "后台服务更新下载响应无效。"
        case let .downloadSizeMismatch(expected, actual):
            "后台服务更新大小不匹配（应为 \(expected) 字节，实际为 \(actual) 字节）。"
        case .digestMismatch:
            "后台服务更新的 SHA-256 校验失败。"
        case .notMachO:
            "下载内容不是有效的 macOS 可执行文件。"
        case .signatureInvalid:
            "后台服务更新的代码签名无效。"
        case .signingIdentityMismatch:
            "后台服务更新与当前 MochiPort 的 Developer ID 或 Team ID 不一致。"
        case let .versionMismatch(expected, actual):
            "后台服务版本不匹配（应为 \(expected)，实际为 \(actual ?? "未知")）。"
        case let .buildMismatch(expected, actual):
            "后台服务构建不匹配（应为 \(expected)，实际为 \(actual.map(String.init) ?? "未知")）。"
        case .runtimeDirectoryUnsafe:
            "后台服务版本目录的所有者或权限不安全。"
        case .runtimeAlreadyExists:
            "相同构建号下已经存在不同的后台服务文件。"
        case .runtimeInstallFailed:
            "无法安全写入后台服务版本目录。"
        case .launchAgentUntrusted:
            "当前后台服务启动配置无法确认，已取消更新。"
        case let .launchAgentMigrationFailed(detail):
            detail.isEmpty ? "无法迁移后台服务启动配置。" : "无法迁移后台服务启动配置：\(detail)"
        case .daemonDidNotExit:
            "后台服务没有在安全更新请求后退出；启动配置未被强制卸载。"
        }
    }
}

struct DaemonUpdateInstaller: DaemonUpdateInstalling, @unchecked Sendable {
    typealias CommandRunner = @Sendable (URL, [String]) throws -> CommandResult
    typealias SignatureValidator = @Sendable (URL, URL) throws -> Void
    typealias ProcessChecker = @Sendable (Int) -> Bool

    private struct AssetMetadata: Sendable {
        let url: URL
        let size: Int64
        let sha256: String
        let version: String
        let build: Int
    }

    private let session: URLSession
    private let configurationLoader: @Sendable () throws -> DaemonLaunchConfiguration
    private let signingReferenceLoader: @Sendable () throws -> URL
    private let commandRunner: CommandRunner
    private let signatureValidator: SignatureValidator
    private let processChecker: ProcessChecker

    init(
        session: URLSession = .shared,
        configurationLoader: @escaping @Sendable () throws -> DaemonLaunchConfiguration = {
            try .current()
        },
        signingReferenceLoader: @escaping @Sendable () throws -> URL = {
            try Self.defaultSigningReference()
        },
        commandRunner: @escaping CommandRunner = Self.runCommand,
        signatureValidator: @escaping SignatureValidator = Self.validateDeveloperIDSignature,
        processChecker: @escaping ProcessChecker = Self.processExists
    ) {
        self.session = session
        self.configurationLoader = configurationLoader
        self.signingReferenceLoader = signingReferenceLoader
        self.commandRunner = commandRunner
        self.signatureValidator = signatureValidator
        self.processChecker = processChecker
    }

    func prepare(release: UpdateComponentRelease) async throws -> PreparedDaemonUpdate {
        let metadata = try Self.assetMetadata(for: release)
        var request = URLRequest(url: metadata.url)
        request.timeoutInterval = 120
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.setValue("MochiPort daemon updater", forHTTPHeaderField: "User-Agent")
        request.setValue("application/octet-stream", forHTTPHeaderField: "Accept")

        let (downloadURL, response) = try await session.download(for: request)
        guard let response = response as? HTTPURLResponse,
              response.statusCode == 200,
              Self.isAllowedDownloadResponse(response.url)
        else {
            throw DaemonUpdateInstallerError.invalidDownloadResponse
        }

        let configurationLoader = configurationLoader
        let signingReferenceLoader = signingReferenceLoader
        let commandRunner = commandRunner
        let signatureValidator = signatureValidator
        return try await Task.detached(priority: .userInitiated) {
            let configuration = try configurationLoader()
            let signingReference = try signingReferenceLoader()
            return try Self.validateAndInstall(
                downloadedURL: downloadURL,
                metadata: metadata,
                configuration: configuration,
                signingReference: signingReference,
                commandRunner: commandRunner,
                signatureValidator: signatureValidator
            )
        }.value
    }

    func prepareLaunchAgentMigration(
        lifecycle: ManageLifecycle,
        candidate: PreparedDaemonUpdate
    ) async throws -> DaemonLaunchMigrationPlan {
        let configurationLoader = configurationLoader
        let commandRunner = commandRunner
        return try await Task.detached(priority: .userInitiated) {
            let configuration = try configurationLoader()
            return try Self.prepareLaunchAgentMigration(
                lifecycle: lifecycle,
                candidate: candidate,
                configuration: configuration,
                commandRunner: commandRunner
            )
        }.value
    }

    func cancelLaunchAgentMigration(_ plan: DaemonLaunchMigrationPlan) async {
        guard case .legacyEntryDisabled = plan.kind else { return }
        let commandRunner = commandRunner
        _ = try? await Task.detached(priority: .utility) {
            try plan.originalPropertyList.write(
                to: plan.configuration.launchAgentURL,
                options: .atomic
            )
            _ = try commandRunner(
                URL(fileURLWithPath: "/bin/launchctl"),
                ["enable", plan.configuration.launchdServiceTarget]
            )
        }.value
    }

    /// Leave launchd in a usable state when the daemon accepted the update but
    /// the handoff itself failed. The current runtime switch is already
    /// committed, so retain the stable launch path and retry bootstrapping it
    /// only after the old process has exited; never force-kill a live daemon.
    func recoverLaunchAgentMigration(
        _ plan: DaemonLaunchMigrationPlan,
        previousPID: Int
    ) async {
        guard case .legacyEntryDisabled = plan.kind else { return }
        let commandRunner = commandRunner
        let processChecker = processChecker
        await Task.detached(priority: .utility) {
            await Self.recoverLaunchAgentMigration(
                plan,
                previousPID: previousPID,
                commandRunner: commandRunner,
                processChecker: processChecker
            )
        }.value
    }

    func completeLaunchAgentMigration(
        _ plan: DaemonLaunchMigrationPlan,
        previousPID: Int
    ) async throws {
        guard case .legacyEntryDisabled = plan.kind else { return }

        for _ in 0..<150 {
            if !processChecker(previousPID) { break }
            try Task.checkCancellation()
            try await Task.sleep(for: .milliseconds(100))
        }
        guard !processChecker(previousPID) else {
            throw DaemonUpdateInstallerError.daemonDidNotExit
        }

        let commandRunner = commandRunner
        try await Task.detached(priority: .userInitiated) {
            let launchctl = URL(fileURLWithPath: "/bin/launchctl")
            let configuration = plan.configuration
            let printBefore = try commandRunner(
                launchctl,
                ["print", configuration.launchdServiceTarget]
            )
            if printBefore.exitCode == 0 {
                let bootout = try commandRunner(
                    launchctl,
                    ["bootout", configuration.launchdServiceTarget]
                )
                guard bootout.exitCode == 0 else {
                    throw DaemonUpdateInstallerError.launchAgentMigrationFailed(
                        Self.lastLine(of: bootout.output)
                    )
                }
            }

            let enabled = try commandRunner(
                launchctl,
                ["enable", configuration.launchdServiceTarget]
            )
            guard enabled.exitCode == 0 else {
                throw DaemonUpdateInstallerError.launchAgentMigrationFailed(
                    Self.lastLine(of: enabled.output)
                )
            }
            let bootstrap = try commandRunner(
                launchctl,
                [
                    "bootstrap",
                    "gui/\(getuid())",
                    configuration.launchAgentURL.path,
                ]
            )
            guard bootstrap.exitCode == 0 else {
                throw DaemonUpdateInstallerError.launchAgentMigrationFailed(
                    Self.lastLine(of: bootstrap.output)
                )
            }
        }.value
    }

    private static func recoverLaunchAgentMigration(
        _ plan: DaemonLaunchMigrationPlan,
        previousPID: Int,
        commandRunner: @escaping CommandRunner,
        processChecker: @escaping ProcessChecker
    ) async {
        let launchctl = URL(fileURLWithPath: "/bin/launchctl")
        let configuration = plan.configuration

        do {
            try plan.updatedPropertyList.write(
                to: configuration.launchAgentURL,
                options: .atomic
            )

            // The service was disabled before the update request. Keep it
            // disabled while the old process drains; enabling it here could
            // let launchd respawn the old absolute-path program before the
            // loaded job is booted out.
            var loaded = try commandRunner(
                launchctl,
                ["print", configuration.launchdServiceTarget]
            )
            if loaded.exitCode == 0 {
                if let loadedProgram = Self.loadedProgram(from: loaded.output),
                   Self.pathsMatch(loadedProgram, configuration.activeHelperURL().path)
                {
                    return
                }

                for _ in 0..<150 {
                    guard processChecker(previousPID) else { break }
                    try await Task.sleep(for: .milliseconds(100))
                }
                guard !processChecker(previousPID) else { return }

                // Re-read after the drain wait. launchd may have unloaded the
                // job or reported a different program while the old process
                // was exiting; only boot out a still-loaded old job.
                loaded = try commandRunner(
                    launchctl,
                    ["print", configuration.launchdServiceTarget]
                )
                if loaded.exitCode == 0 {
                    if let loadedProgram = Self.loadedProgram(from: loaded.output),
                       Self.pathsMatch(loadedProgram, configuration.activeHelperURL().path)
                    {
                        return
                    }
                    let bootout = try commandRunner(
                        launchctl,
                        ["bootout", configuration.launchdServiceTarget]
                    )
                    guard bootout.exitCode == 0 else { return }
                }
            }

            let enable = try commandRunner(
                launchctl,
                ["enable", configuration.launchdServiceTarget]
            )
            guard enable.exitCode == 0 else { return }

            _ = try commandRunner(
                launchctl,
                [
                    "bootstrap",
                    "gui/\(getuid())",
                    configuration.launchAgentURL.path,
                ]
            )
        } catch is CancellationError {
            return
        } catch {
            return
        }
    }

    private static func assetMetadata(
        for release: UpdateComponentRelease
    ) throws -> AssetMetadata {
        guard let build = release.build, build > 0,
              let asset = release.assets["macos-daemon-universal"],
              asset.assetType == "executable",
              asset.signed == true,
              let url = asset.validatedDownloadURL,
              let size = asset.size, size > 0,
              let sha256 = asset.normalizedSHA256,
              let version = normalizedDaemonVersion(release.version)
        else {
            throw DaemonUpdateInstallerError.invalidManifestAsset
        }
        return AssetMetadata(
            url: url,
            size: size,
            sha256: sha256,
            version: version,
            build: build
        )
    }

    private static func normalizedDaemonVersion(_ raw: String) -> String? {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        let normalized = (trimmed.first == "v" || trimmed.first == "V")
            ? String(trimmed.dropFirst())
            : trimmed
        guard !normalized.isEmpty, isNewerVersion(normalized, than: "0") else { return nil }
        return normalized
    }

    private static func isAllowedDownloadResponse(_ url: URL?) -> Bool {
        guard let url, url.scheme?.lowercased() == "https",
              let host = url.host?.lowercased()
        else { return false }
        if host == "github.com" {
            return url.path.lowercased().hasPrefix("/mps233/mochiport/releases/download/")
        }
        return host == "objects.githubusercontent.com"
            || host == "release-assets.githubusercontent.com"
            || host == "github-releases.githubusercontent.com"
    }

    private static func validateAndInstall(
        downloadedURL: URL,
        metadata: AssetMetadata,
        configuration: DaemonLaunchConfiguration,
        signingReference: URL,
        commandRunner: CommandRunner,
        signatureValidator: SignatureValidator,
        fileManager: FileManager = .default
    ) throws -> PreparedDaemonUpdate {
        let attributes = try fileManager.attributesOfItem(atPath: downloadedURL.path)
        let size = (attributes[.size] as? NSNumber)?.int64Value ?? -1
        guard size == metadata.size else {
            throw DaemonUpdateInstallerError.downloadSizeMismatch(
                expected: metadata.size,
                actual: size
            )
        }
        guard try sha256(of: downloadedURL) == metadata.sha256 else {
            throw DaemonUpdateInstallerError.digestMismatch
        }
        guard try isMachO(downloadedURL) else {
            throw DaemonUpdateInstallerError.notMachO
        }
        try fileManager.setAttributes(
            [.posixPermissions: 0o755],
            ofItemAtPath: downloadedURL.path
        )
        try signatureValidator(downloadedURL, signingReference)
        try validateVersion(
            at: downloadedURL,
            expectedVersion: metadata.version,
            expectedBuild: metadata.build,
            commandRunner: commandRunner
        )

        let runtimes = configuration.configURL
            .deletingLastPathComponent()
            .appendingPathComponent("runtimes", isDirectory: true)
        try createOrValidateOwnedDirectory(runtimes, fileManager: fileManager)
        let buildDirectory = runtimes.appendingPathComponent(
            String(metadata.build),
            isDirectory: true
        )
        try createOrValidateOwnedDirectory(buildDirectory, fileManager: fileManager)
        let destination = buildDirectory.appendingPathComponent("mochiport-daemon")

        if fileManager.fileExists(atPath: destination.path) {
            guard try isSecureRegularExecutable(destination),
                  try sha256(of: destination) == metadata.sha256
            else {
                throw DaemonUpdateInstallerError.runtimeAlreadyExists
            }
            try signatureValidator(destination, signingReference)
            try validateVersion(
                at: destination,
                expectedVersion: metadata.version,
                expectedBuild: metadata.build,
                commandRunner: commandRunner
            )
            return PreparedDaemonUpdate(
                version: metadata.version,
                build: metadata.build,
                sha256: metadata.sha256,
                executableURL: destination
            )
        }

        let temporary = buildDirectory.appendingPathComponent(
            ".mochiport-daemon.\(UUID().uuidString).tmp"
        )
        defer { try? fileManager.removeItem(at: temporary) }
        do {
            try fileManager.copyItem(at: downloadedURL, to: temporary)
            try fileManager.setAttributes(
                [.posixPermissions: 0o755],
                ofItemAtPath: temporary.path
            )
            guard try isSecureRegularExecutable(temporary),
                  try sha256(of: temporary) == metadata.sha256
            else {
                throw DaemonUpdateInstallerError.runtimeInstallFailed
            }
            let installed = temporary.path.withCString { temporaryPath in
                destination.path.withCString { destinationPath in
                    Darwin.link(temporaryPath, destinationPath)
                }
            }
            guard installed == 0 else {
                if errno == EEXIST {
                    throw DaemonUpdateInstallerError.runtimeAlreadyExists
                }
                throw DaemonUpdateInstallerError.runtimeInstallFailed
            }
            try fileManager.removeItem(at: temporary)
            guard try isSecureRegularExecutable(destination) else {
                throw DaemonUpdateInstallerError.runtimeInstallFailed
            }
        } catch let error as DaemonUpdateInstallerError {
            throw error
        } catch {
            throw DaemonUpdateInstallerError.runtimeInstallFailed
        }

        return PreparedDaemonUpdate(
            version: metadata.version,
            build: metadata.build,
            sha256: metadata.sha256,
            executableURL: destination
        )
    }

    private static func validateVersion(
        at executable: URL,
        expectedVersion: String,
        expectedBuild: Int,
        commandRunner: CommandRunner
    ) throws {
        let result = try commandRunner(executable, ["--version"])
        guard result.exitCode == 0,
              let parsed = daemonVersion(from: result.output)
        else {
            throw DaemonUpdateInstallerError.versionMismatch(
                expected: expectedVersion,
                actual: nil
            )
        }
        guard parsed.version == expectedVersion else {
            throw DaemonUpdateInstallerError.versionMismatch(
                expected: expectedVersion,
                actual: parsed.version
            )
        }
        guard parsed.build == expectedBuild else {
            throw DaemonUpdateInstallerError.buildMismatch(
                expected: expectedBuild,
                actual: parsed.build
            )
        }
    }

    private static func daemonVersion(from output: String) -> (version: String, build: Int)? {
        let line = output.trimmingCharacters(in: .whitespacesAndNewlines)
        guard line.hasPrefix("mochiport "), line.hasSuffix(")"),
              let buildRange = line.range(of: " (build ")
        else { return nil }
        let version = String(line["mochiport ".endIndex..<buildRange.lowerBound])
        let buildText = line[buildRange.upperBound..<line.index(before: line.endIndex)]
        guard !version.isEmpty, let build = Int(buildText), build > 0 else { return nil }
        return (version, build)
    }

    private static func isMachO(_ url: URL) throws -> Bool {
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        guard let data = try handle.read(upToCount: 4), data.count == 4 else { return false }
        let magic = data.reduce(UInt32(0)) { ($0 << 8) | UInt32($1) }
        return [
            UInt32(0xFEEDFACE), 0xCEFAEDFE, 0xFEEDFACF, 0xCFFAEDFE,
            0xCAFEBABE, 0xBEBAFECA, 0xCAFEBABF, 0xBFBAFECA,
        ].contains(magic)
    }

    private static func sha256(of url: URL) throws -> String {
        let data = try Data(contentsOf: url, options: .mappedIfSafe)
        return SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    }

    private static func createOrValidateOwnedDirectory(
        _ url: URL,
        fileManager: FileManager
    ) throws {
        if !fileManager.fileExists(atPath: url.path) {
            do {
                try fileManager.createDirectory(at: url, withIntermediateDirectories: false)
                try fileManager.setAttributes(
                    [.posixPermissions: 0o755],
                    ofItemAtPath: url.path
                )
            } catch {
                throw DaemonUpdateInstallerError.runtimeDirectoryUnsafe
            }
        }
        var info = stat()
        guard Darwin.lstat(url.path, &info) == 0,
              info.st_mode & mode_t(S_IFMT) == mode_t(S_IFDIR),
              info.st_uid == getuid(),
              info.st_mode & 0o022 == 0
        else {
            throw DaemonUpdateInstallerError.runtimeDirectoryUnsafe
        }
    }

    private static func isSecureRegularExecutable(_ url: URL) throws -> Bool {
        var info = stat()
        guard Darwin.lstat(url.path, &info) == 0 else { return false }
        return info.st_mode & mode_t(S_IFMT) == mode_t(S_IFREG)
            && info.st_uid == getuid()
            && info.st_nlink == 1
            && info.st_mode & 0o777 == 0o755
            && FileManager.default.isExecutableFile(atPath: url.path)
    }

    private static func defaultSigningReference() throws -> URL {
        let helpers = Bundle.main.bundleURL
            .appendingPathComponent("Contents/Helpers", isDirectory: true)
        let daemon = helpers.appendingPathComponent("mochiport-daemon")
        if FileManager.default.fileExists(atPath: daemon.path) { return daemon }
        guard Bundle.main.bundleURL.pathExtension == "app" else {
            throw DaemonUpdateInstallerError.signatureInvalid
        }
        return Bundle.main.bundleURL
    }

    private static func validateDeveloperIDSignature(
        candidate: URL,
        reference: URL
    ) throws {
        let codesign = URL(fileURLWithPath: "/usr/bin/codesign")
        let verify = try runCommand(
            codesign,
            ["--verify", "--strict", "--verbose=4", candidate.path]
        )
        guard verify.exitCode == 0 else {
            throw DaemonUpdateInstallerError.signatureInvalid
        }
        let candidateInfo = try signingInfo(candidate)
        let referenceInfo = try signingInfo(reference)
        guard candidateInfo.teamID == referenceInfo.teamID,
              candidateInfo.authority == referenceInfo.authority
        else {
            throw DaemonUpdateInstallerError.signingIdentityMismatch
        }
    }

    private static func signingInfo(_ url: URL) throws -> (teamID: String, authority: String) {
        let result = try runCommand(
            URL(fileURLWithPath: "/usr/bin/codesign"),
            ["--display", "--verbose=4", url.path]
        )
        let lines = result.output.split(whereSeparator: \.isNewline).map(String.init)
        guard result.exitCode == 0,
              let teamID = lines.first(where: { $0.hasPrefix("TeamIdentifier=") })?
                .dropFirst("TeamIdentifier=".count),
              !teamID.isEmpty,
              let authority = lines.first(where: {
                  $0.hasPrefix("Authority=Developer ID Application:")
              })?.dropFirst("Authority=".count),
              !authority.isEmpty
        else {
            throw DaemonUpdateInstallerError.signatureInvalid
        }
        return (String(teamID), String(authority))
    }

    private static func prepareLaunchAgentMigration(
        lifecycle: ManageLifecycle,
        candidate: PreparedDaemonUpdate,
        configuration: DaemonLaunchConfiguration,
        commandRunner: CommandRunner,
        fileManager: FileManager = .default
    ) throws -> DaemonLaunchMigrationPlan {
        guard URL(fileURLWithPath: lifecycle.configPath).standardizedFileURL
                == configuration.configURL.standardizedFileURL,
              candidate.executableURL == configuration.configURL
                .deletingLastPathComponent()
                .appendingPathComponent("runtimes/\(candidate.build)/mochiport-daemon")
        else {
            throw DaemonUpdateInstallerError.launchAgentUntrusted
        }

        let launchctl = URL(fileURLWithPath: "/bin/launchctl")
        let loaded = try commandRunner(
            launchctl,
            ["print", configuration.launchdServiceTarget]
        )
        guard loaded.exitCode == 0,
              loadedPID(from: loaded.output) == lifecycle.service.pid,
              let loadedProgram = loadedProgram(from: loaded.output),
              pathsMatch(loadedProgram, lifecycle.executable)
        else {
            throw DaemonUpdateInstallerError.launchAgentUntrusted
        }

        let original = try Data(contentsOf: configuration.launchAgentURL)
        guard var propertyList = try PropertyListSerialization.propertyList(
            from: original,
            options: [],
            format: nil
        ) as? [String: Any],
        propertyList["Label"] as? String == configuration.launchdLabel,
        var arguments = propertyList["ProgramArguments"] as? [String],
        arguments.count == 4,
        arguments[1] == "--config",
        pathsMatch(arguments[2], lifecycle.configPath),
        arguments[3] == "daemon"
        else {
            throw DaemonUpdateInstallerError.launchAgentUntrusted
        }

        let stableProgram = configuration.activeHelperURL().path
        arguments[0] = stableProgram
        propertyList["ProgramArguments"] = arguments
        let updated = try PropertyListSerialization.data(
            fromPropertyList: propertyList,
            format: .xml,
            options: 0
        )
        try updated.write(to: configuration.launchAgentURL, options: .atomic)

        if loadedProgram == stableProgram {
            return DaemonLaunchMigrationPlan(
                kind: .stableEntryAlreadyLoaded,
                configuration: configuration,
                originalPropertyList: original,
                updatedPropertyList: updated
            )
        }

        let disabled = try commandRunner(
            launchctl,
            ["disable", configuration.launchdServiceTarget]
        )
        guard disabled.exitCode == 0 else {
            try? original.write(to: configuration.launchAgentURL, options: .atomic)
            throw DaemonUpdateInstallerError.launchAgentMigrationFailed(
                lastLine(of: disabled.output)
            )
        }
        return DaemonLaunchMigrationPlan(
            kind: .legacyEntryDisabled,
            configuration: configuration,
            originalPropertyList: original,
            updatedPropertyList: updated
        )
    }

    private static func processExists(_ pid: Int) -> Bool {
        guard pid > 0, let processID = pid_t(exactly: pid) else { return false }
        if Darwin.kill(processID, 0) == 0 { return true }
        return errno == EPERM
    }

    private static func pathsMatch(_ lhs: String, _ rhs: String) -> Bool {
        URL(fileURLWithPath: lhs).standardizedFileURL.resolvingSymlinksInPath()
            == URL(fileURLWithPath: rhs).standardizedFileURL.resolvingSymlinksInPath()
    }

    private static func loadedProgram(from output: String) -> String? {
        output.split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .first(where: { $0.hasPrefix("program = ") })
            .map { String($0.dropFirst("program = ".count)) }
            .map(unquote)
    }

    private static func loadedPID(from output: String) -> Int? {
        output.split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .first(where: { $0.hasPrefix("pid = ") })
            .flatMap { Int($0.dropFirst("pid = ".count)) }
    }

    private static func unquote(_ value: String) -> String {
        guard value.count >= 2, value.first == "\"", value.last == "\"" else {
            return value
        }
        return String(value.dropFirst().dropLast())
    }

    private static func lastLine(of output: String) -> String {
        output.split(whereSeparator: \.isNewline).last.map(String.init) ?? ""
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
}
