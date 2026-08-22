import Foundation
import XCTest

#if canImport(ThreadRelayMac)
@testable import ThreadRelayMac
#elseif canImport(ThreadRelay)
@testable import ThreadRelay
#endif

final class APIContractTests: XCTestCase {
    override func tearDown() {
        MockURLProtocol.reset()
        super.tearDown()
    }

    func testHealthFixtureDecodes() throws {
        let data = Data(#"{"service":"threadrelay","apiMajor":1,"ready":true}"#.utf8)
        let health = try JSONDecoder().decode(HealthResponse.self, from: data)

        XCTAssertEqual(
            health,
            HealthResponse(service: "threadrelay", apiMajor: 1, ready: true)
        )
    }

    func testManualUpdateVersionComparisonHandlesTagsAndUnevenComponents() {
        XCTAssertTrue(isNewerVersion("v0.5.1", than: "0.5.0"))
        XCTAssertTrue(isNewerVersion("0.6", than: "0.5.9"))
        XCTAssertFalse(isNewerVersion("v0.5.0", than: "0.5"))
        XCTAssertFalse(isNewerVersion("0.4.9", than: "0.5.0"))
    }

    func testSingleInstanceGuardRejectsSecondOwner() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        let lockURL = root.appendingPathComponent("threadrelay.gui.lock")
        defer { try? FileManager.default.removeItem(at: root) }

        do {
            let first = try SingleInstanceGuard.acquire(lockURL: lockURL)
            XCTAssertThrowsError(try SingleInstanceGuard.acquire(lockURL: lockURL)) { error in
                XCTAssertEqual(error as? SingleInstanceError, .alreadyRunning)
            }
            withExtendedLifetime(first) {}
        }
    }

    func testSingleInstanceGuardReleasesLockOnDeallocation() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        let lockURL = root.appendingPathComponent("threadrelay.gui.lock")
        defer { try? FileManager.default.removeItem(at: root) }

        var first: SingleInstanceGuard? = try SingleInstanceGuard.acquire(lockURL: lockURL)
        withExtendedLifetime(first) {}
        first = nil
        XCTAssertNoThrow(try SingleInstanceGuard.acquire(lockURL: lockURL))
    }

    func testSingleInstanceLockPathIsScopedToBundleIdentifier() {
        let home = URL(fileURLWithPath: "/fixture/home", isDirectory: true)
        XCTAssertEqual(
            SingleInstanceGuard.defaultLockURL(
                bundleIdentifier: "io.github.mps233.threadrelay.preview",
                environment: ["HOME": home.path]
            ).path,
            "/fixture/home/Library/Application Support/ThreadRelay/io.github.mps233.threadrelay.preview.gui.lock"
        )
    }

    func testManagementCredentialPathsHonorBothHomeOverridesBeforeDefaults() {
        let applicationSupport = URL(fileURLWithPath: "/fixture/Application Support", isDirectory: true)

        XCTAssertEqual(
            ManagementCredentialStore.candidatePaths(
                environment: [
                    "THREADRELAY_HOME": "/fixture/threadrelay",
                    "CODEXHUB_HOME": "/fixture/codexhub",
                ],
                applicationSupport: applicationSupport
            ).map(\.path),
            [
                "/fixture/threadrelay/threadrelay-control.json",
                "/fixture/codexhub/threadrelay-control.json",
                "/fixture/Application Support/ThreadRelay/threadrelay-control.json",
                "/fixture/Application Support/CodexHub/threadrelay-control.json",
            ]
        )
    }

    func testDaemonLaunchConfigurationUsesLegacyDataAndEmbeddedHelper() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        let home = root.appendingPathComponent("home", isDirectory: true)
        let appSupport = home.appendingPathComponent("Library/Application Support", isDirectory: true)
        let legacyDirectory = appSupport.appendingPathComponent("CodexHub", isDirectory: true)
        let bundle = root.appendingPathComponent("ThreadRelay.app", isDirectory: true)
        try FileManager.default.createDirectory(at: legacyDirectory, withIntermediateDirectories: true)
        try Data().write(to: legacyDirectory.appendingPathComponent("config.toml"))
        defer { try? FileManager.default.removeItem(at: root) }

        let configuration = try DaemonLaunchConfiguration.current(
            bundleURL: bundle,
            environment: ["HOME": home.path],
            fileManager: .default
        )

        XCTAssertEqual(
            configuration.helperURL.path,
            bundle.appendingPathComponent("Contents/Helpers/threadrelay-daemon").path
        )
        XCTAssertEqual(
            configuration.configURL.path,
            legacyDirectory.appendingPathComponent("config.toml").path
        )
        XCTAssertEqual(
            configuration.launchAgentURL.path,
            home.appendingPathComponent("Library/LaunchAgents/\(DaemonLaunchConfiguration.label).plist").path
        )
        XCTAssertEqual(
            try configuration.stagedHelperURL().path,
            legacyDirectory.appendingPathComponent("runtimes/dev/threadrelay-daemon").path
        )
        XCTAssertEqual(configuration.launchdLabel, DaemonLaunchConfiguration.label)
        XCTAssertEqual(
            configuration.launchdServiceTarget,
            "gui/\(getuid())/\(DaemonLaunchConfiguration.label)"
        )
    }

#if DEBUG
    func testDaemonLaunchConfigurationRejectsUnsafeTestLaunchdLabels() {
        let root = URL(fileURLWithPath: "/fixture", isDirectory: true)
        let labels = [
            DaemonLaunchConfiguration.label,
            "io.github.mps233.threadrelay.tests.",
            "io.github.mps233.threadrelay.tests.-leading",
            "io.github.mps233.threadrelay.tests.trailing-",
            "io.github.mps233.threadrelay.tests.two..dots",
            "io.github.mps233.threadrelay.tests.bad/value",
            "io.github.mps233.threadrelay.tests.bad value",
            "io.github.mps233.threadrelay.other.test",
            "io.github.mps233.threadrelay.tests.\(String(repeating: "a", count: 100))",
        ]

        for label in labels {
            XCTAssertThrowsError(
                try DaemonLaunchConfiguration(
                    testLaunchdLabel: label,
                    helperURL: root.appendingPathComponent("threadrelay-daemon"),
                    configURL: root.appendingPathComponent("config.toml"),
                    launchAgentURL: root.appendingPathComponent("daemon.plist"),
                    logURL: root.appendingPathComponent("daemon.log"),
                    homeURL: root,
                    buildIdentifier: "test"
                )
            ) { error in
                XCTAssertEqual(error as? DaemonLaunchError, .launchdLabelInvalid(label))
            }
        }
    }
#endif

    func testGUIRecoveryConfigurationRestartsOnlyUnexpectedExit() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        let home = root.appendingPathComponent("home", isDirectory: true)
        let appSupport = home.appendingPathComponent("Library/Application Support", isDirectory: true)
        let legacyDirectory = appSupport.appendingPathComponent("CodexHub", isDirectory: true)
        let bundle = root.appendingPathComponent("ThreadRelay.app", isDirectory: true)
        try FileManager.default.createDirectory(at: legacyDirectory, withIntermediateDirectories: true)
        try Data().write(to: legacyDirectory.appendingPathComponent("config.toml"))
        defer { try? FileManager.default.removeItem(at: root) }

        let configuration = try GUIRecoveryConfiguration.current(
            bundleURL: bundle,
            environment: ["HOME": home.path],
            fileManager: .default
        )
        let plist = try XCTUnwrap(
            PropertyListSerialization.propertyList(
                from: configuration.propertyListData(),
                options: [],
                format: nil
            ) as? [String: Any]
        )

        XCTAssertEqual(plist["Label"] as? String, GUIRecoveryConfiguration.label)
        XCTAssertEqual(
            plist["ProgramArguments"] as? [String],
            [configuration.supervisorURL.path]
        )
        XCTAssertEqual(
            (plist["KeepAlive"] as? [String: Bool])?["SuccessfulExit"],
            false
        )
        XCTAssertEqual(plist["ProcessType"] as? String, "Interactive")
        XCTAssertEqual(
            configuration.launchAgentURL.path,
            home.appendingPathComponent(
                "Library/LaunchAgents/\(GUIRecoveryConfiguration.label).plist"
            ).path
        )
    }

    func testDaemonLauncherDoesNotStageRuntimeForLoadedService() async throws {
        let fixture = try makeDaemonLauncherFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let commands = CommandInvocationRecorder { arguments in
            if arguments.first == "print" {
                return CommandResult(
                    exitCode: 0,
                    output: """
                    state = running
                    program = \(fixture.configuration.helperURL.path)
                    arguments = {
                        \(fixture.configuration.helperURL.path)
                        --config
                        \(fixture.configuration.configURL.path)
                        daemon
                    }
                    """
                )
            }
            return CommandResult(exitCode: 1, output: "unexpected command")
        }
        let launcher = DaemonLauncher(
            configurationLoader: { fixture.configuration },
            commandRunner: commands.run
        )

        try await launcher.startIfNeeded()

        XCTAssertEqual(commands.arguments, [
            ["print", "gui/\(getuid())/\(DaemonLaunchConfiguration.label)"],
        ])
        XCTAssertEqual(commands.executablePaths.map(\.path), ["/bin/launchctl"])
        XCTAssertFalse(
            commands.arguments
                .compactMap(\.first)
                .contains(where: { ["bootout", "kickstart", "bootstrap"].contains($0) })
        )
        XCTAssertFalse(
            FileManager.default.fileExists(atPath: fixture.configuration.launchAgentURL.path)
        )
        XCTAssertFalse(
            FileManager.default.fileExists(atPath: try fixture.configuration.stagedHelperURL().path)
        )
    }

    func testDaemonLauncherLeavesLoadedServiceFromDifferentHelperRunning() async throws {
        let fixture = try makeDaemonLauncherFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let staleHelper = fixture.root.appendingPathComponent("OldThreadRelay.app/Contents/Helpers/threadrelay-daemon")
        let commands = CommandInvocationRecorder { arguments in
            guard arguments.first == "print" else {
                return CommandResult(exitCode: 1, output: "unexpected command")
            }
            return CommandResult(
                exitCode: 0,
                output: """
                state = running
                program = \(staleHelper.path)
                arguments = {
                    \(staleHelper.path)
                    --config
                    \(fixture.configuration.configURL.path)
                    daemon
                }
                """
            )
        }
        let launcher = DaemonLauncher(
            configurationLoader: { fixture.configuration },
            commandRunner: commands.run
        )

        try await launcher.startIfNeeded()

        XCTAssertEqual(commands.arguments.map(\.first), ["print"])
        XCTAssertFalse(
            commands.arguments
                .compactMap(\.first)
                .contains(where: { ["bootout", "kickstart", "bootstrap"].contains($0) })
        )
        XCTAssertFalse(FileManager.default.fileExists(atPath: fixture.configuration.launchAgentURL.path))
    }

    func testDaemonLauncherDoesNotReloadRunningServiceAfterBundleBuildChanges() async throws {
        let fixture = try makeDaemonLauncherFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let configuration = DaemonLaunchConfiguration(
            helperURL: fixture.configuration.helperURL,
            configURL: fixture.configuration.configURL,
            launchAgentURL: fixture.configuration.launchAgentURL,
            logURL: fixture.configuration.logURL,
            homeURL: fixture.configuration.homeURL,
            buildIdentifier: "390"
        )
        let previous = try installDaemonRuntime(build: "388", fixture: fixture)
        let commands = CommandInvocationRecorder { arguments in
            if arguments.first == "print" {
                return CommandResult(
                    exitCode: 0,
                    output: """
                    state = running
                    program = \(previous.path)
                    arguments = {
                        \(previous.path)
                        --config
                        \(configuration.configURL.path)
                        daemon
                    }
                    environment = {
                        THREADRELAY_HOME => \(configuration.configURL.deletingLastPathComponent().path)
                        THREADRELAY_BUNDLE_BUILD => 388
                    }
                    """
                )
            }
            return CommandResult(exitCode: 1, output: "unexpected command")
        }
        let launcher = DaemonLauncher(
            configurationLoader: { configuration },
            commandRunner: commands.run
        )

        try await launcher.startIfNeeded()

        XCTAssertEqual(commands.arguments.map(\.first), ["print"])
        let plistData = try Data(contentsOf: configuration.launchAgentURL)
        let plist = try XCTUnwrap(
            PropertyListSerialization.propertyList(from: plistData, options: [], format: nil)
                as? [String: Any]
        )
        XCTAssertEqual(
            plist["ProgramArguments"] as? [String],
            [
                previous.path,
                "--config",
                configuration.configURL.path,
                "daemon",
            ]
        )
        XCTAssertEqual(
            (plist["EnvironmentVariables"] as? [String: String])?["THREADRELAY_BUNDLE_BUILD"],
            "388"
        )
    }

    func testGUIRecoveryLauncherUpdatesStaleBuildWithoutReloadingRunningSupervisor() throws {
        let fixture = try makeGUIRecoveryLauncherFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let configuration = fixture.configuration
        let printOutput = guiRecoveryLaunchctlOutput(
            configuration: configuration,
            build: "388"
        )
        let commands = CommandInvocationRecorder { arguments in
            if arguments.first == "print" {
                return CommandResult(exitCode: 0, output: printOutput)
            }
            return CommandResult(exitCode: 1, output: "unexpected command")
        }
        let launcher = GUIRecoveryLauncher(
            configurationLoader: { configuration },
            commandRunner: commands.run
        )

        try launcher.startIfNeeded()

        let serviceTarget = "gui/\(getuid())/\(GUIRecoveryConfiguration.label)"
        XCTAssertEqual(commands.arguments, [["print", serviceTarget]])
        let plist = try guiRecoveryLaunchAgentPropertyList(at: configuration.launchAgentURL)
        let environment = try XCTUnwrap(plist["EnvironmentVariables"] as? [String: String])
        XCTAssertEqual(environment["HOME"], configuration.homeURL.path)
        XCTAssertEqual(environment["THREADRELAY_HOME"], configuration.dataDirectoryURL.path)
        XCTAssertEqual(environment["THREADRELAY_BUNDLE_BUILD"], "389")
    }

    func testGUIRecoveryLauncherKickstartsStoppedSupervisorWithoutReloadingIt() throws {
        let fixture = try makeGUIRecoveryLauncherFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let configuration = fixture.configuration
        let printOutput = guiRecoveryLaunchctlOutput(
            configuration: configuration,
            state: "exited",
            build: "388"
        )
        let launchAgentDataAtKickstart = LockedValue<Data?>(nil)
        let commands = CommandInvocationRecorder { arguments in
            if arguments.first == "print" {
                return CommandResult(exitCode: 0, output: printOutput)
            }
            if arguments.first == "kickstart" {
                launchAgentDataAtKickstart.value = try? Data(
                    contentsOf: configuration.launchAgentURL
                )
                return CommandResult(exitCode: 0, output: "")
            }
            return CommandResult(exitCode: 1, output: "unexpected command")
        }
        let launcher = GUIRecoveryLauncher(
            configurationLoader: { configuration },
            commandRunner: commands.run
        )

        try launcher.startIfNeeded()

        let serviceTarget = "gui/\(getuid())/\(GUIRecoveryConfiguration.label)"
        XCTAssertEqual(
            commands.arguments,
            [
                ["print", serviceTarget],
                ["kickstart", serviceTarget],
            ]
        )
        let launchAgentData = try XCTUnwrap(launchAgentDataAtKickstart.value)
        let launchAgentAtKickstart = try XCTUnwrap(
            PropertyListSerialization.propertyList(
                from: launchAgentData,
                options: [],
                format: nil
            ) as? [String: Any]
        )
        XCTAssertEqual(
            (launchAgentAtKickstart["EnvironmentVariables"] as? [String: String])?[
                "THREADRELAY_BUNDLE_BUILD"
            ],
            "389"
        )
        let plist = try guiRecoveryLaunchAgentPropertyList(at: configuration.launchAgentURL)
        let environment = try XCTUnwrap(plist["EnvironmentVariables"] as? [String: String])
        XCTAssertEqual(environment["THREADRELAY_BUNDLE_BUILD"], "389")
    }

    func testGUIRecoveryLauncherRejectsSupervisorFromAnotherBundlePath() throws {
        let fixture = try makeGUIRecoveryLauncherFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let configuration = fixture.configuration
        let otherSupervisor = fixture.root.appendingPathComponent(
            "Other.app/Contents/Helpers/threadrelay-gui-supervisor"
        )
        let printOutput = guiRecoveryLaunchctlOutput(
            configuration: configuration,
            supervisorURL: otherSupervisor,
            arguments: [otherSupervisor.path]
        )
        let commands = CommandInvocationRecorder { arguments in
            guard arguments.first == "print" else {
                return CommandResult(exitCode: 1, output: "unexpected command")
            }
            return CommandResult(exitCode: 0, output: printOutput)
        }
        let launcher = GUIRecoveryLauncher(
            configurationLoader: { configuration },
            commandRunner: commands.run
        )

        XCTAssertThrowsError(try launcher.startIfNeeded()) { error in
            XCTAssertEqual(
                error as? DaemonLaunchError,
                .loadedAgentMismatch(
                    expected: configuration.supervisorURL.path,
                    actual: otherSupervisor.path
                )
            )
        }
        let serviceTarget = "gui/\(getuid())/\(GUIRecoveryConfiguration.label)"
        XCTAssertEqual(commands.arguments, [["print", serviceTarget]])
        XCTAssertFalse(FileManager.default.fileExists(atPath: configuration.launchAgentURL.path))
    }

    func testGUIRecoveryLauncherRejectsSupervisorWithMismatchedIdentity() throws {
        let fixture = try makeGUIRecoveryLauncherFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let configuration = fixture.configuration
        let mismatches: [(home: URL, data: URL, arguments: [String])] = [
            (
                fixture.root.appendingPathComponent("other-home"),
                configuration.dataDirectoryURL,
                [configuration.supervisorURL.path]
            ),
            (
                configuration.homeURL,
                fixture.root.appendingPathComponent("other-data"),
                [configuration.supervisorURL.path]
            ),
            (
                configuration.homeURL,
                configuration.dataDirectoryURL,
                [configuration.supervisorURL.path, "--unexpected"]
            ),
        ]

        for mismatch in mismatches {
            let printOutput = guiRecoveryLaunchctlOutput(
                configuration: configuration,
                arguments: mismatch.arguments,
                homeURL: mismatch.home,
                dataDirectoryURL: mismatch.data
            )
            let commands = CommandInvocationRecorder { arguments in
                guard arguments.first == "print" else {
                    return CommandResult(exitCode: 1, output: "unexpected command")
                }
                return CommandResult(exitCode: 0, output: printOutput)
            }
            let launcher = GUIRecoveryLauncher(
                configurationLoader: { configuration },
                commandRunner: commands.run
            )

            XCTAssertThrowsError(try launcher.startIfNeeded()) { error in
                XCTAssertEqual(
                    error as? DaemonLaunchError,
                    .loadedAgentUntrusted(configuration.supervisorURL.path)
                )
            }
            let serviceTarget = "gui/\(getuid())/\(GUIRecoveryConfiguration.label)"
            XCTAssertEqual(commands.arguments, [["print", serviceTarget]])
        }
        XCTAssertFalse(FileManager.default.fileExists(atPath: configuration.launchAgentURL.path))
    }

    func testDaemonLauncherBootstrapsMissingService() async throws {
        let fixture = try makeDaemonLauncherFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let commands = CommandInvocationRecorder { arguments in
            if arguments == ["--version"] {
                return CommandResult(exitCode: 0, output: "threadrelay 0.5.0 (build 389)\n")
            }
            return CommandResult(exitCode: arguments.first == "bootstrap" ? 0 : 1, output: "")
        }
        let launcher = DaemonLauncher(
            configurationLoader: { fixture.configuration },
            commandRunner: commands.run
        )

        try await launcher.startIfNeeded()

        XCTAssertEqual(commands.arguments.map(\.first), ["print", "--version", "bootstrap"])
        let plistData = try Data(contentsOf: fixture.configuration.launchAgentURL)
        let plist = try XCTUnwrap(
            PropertyListSerialization.propertyList(from: plistData, options: [], format: nil)
                as? [String: Any]
        )
        XCTAssertEqual(plist["Label"] as? String, DaemonLaunchConfiguration.label)
        XCTAssertEqual(
            plist["ProgramArguments"] as? [String],
            [
                try fixture.configuration.stagedHelperURL().path,
                "--config",
                fixture.configuration.configURL.path,
                "daemon",
            ]
        )
        let environment = try XCTUnwrap(plist["EnvironmentVariables"] as? [String: String])
        XCTAssertEqual(
            environment["THREADRELAY_HOME"],
            fixture.configuration.configURL.deletingLastPathComponent().path
        )
        XCTAssertEqual(environment["THREADRELAY_BUNDLE_BUILD"], "389")
    }

    func testDaemonLauncherRejectsMismatchedRuntimeBuildBeforePublishing() async throws {
        let fixture = try makeDaemonLauncherFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let commands = CommandInvocationRecorder { arguments in
            guard arguments == ["--version"] else {
                return CommandResult(exitCode: 1, output: "unexpected command")
            }
            return CommandResult(exitCode: 0, output: "threadrelay 0.5.0 (build 388)\n")
        }
        let launcher = DaemonLauncher(
            configurationLoader: { fixture.configuration },
            commandRunner: commands.run
        )

        do {
            try await launcher.startIfNeeded()
            XCTFail("Expected mismatched runtime build to fail")
        } catch let error as DaemonLaunchError {
            XCTAssertEqual(
                error,
                .runtimeVersionMismatch(expected: "389", actual: "388")
            )
        }

        XCTAssertEqual(
            commands.arguments,
            [["print", "gui/\(getuid())/\(DaemonLaunchConfiguration.label)"], ["--version"]]
        )
        XCTAssertFalse(
            FileManager.default.fileExists(atPath: try fixture.configuration.stagedHelperURL().path)
        )
        XCTAssertFalse(
            FileManager.default.fileExists(atPath: fixture.configuration.launchAgentURL.path)
        )
        let runtimeDirectory = try fixture.configuration.stagedHelperURL()
            .deletingLastPathComponent()
        XCTAssertEqual(
            try FileManager.default.contentsOfDirectory(atPath: runtimeDirectory.path),
            []
        )
    }

    func testDaemonLauncherRejectsUnsafeRuntimeBuildIdentifier() async throws {
        let fixture = try makeDaemonLauncherFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let configuration = DaemonLaunchConfiguration(
            helperURL: fixture.configuration.helperURL,
            configURL: fixture.configuration.configURL,
            launchAgentURL: fixture.configuration.launchAgentURL,
            logURL: fixture.configuration.logURL,
            homeURL: fixture.configuration.homeURL,
            buildIdentifier: "../389"
        )
        let commands = CommandInvocationRecorder { _ in
            CommandResult(exitCode: 1, output: "unexpected command")
        }
        let launcher = DaemonLauncher(
            configurationLoader: { configuration },
            commandRunner: commands.run
        )

        do {
            try await launcher.startIfNeeded()
            XCTFail("Expected unsafe build identifier to fail")
        } catch let error as DaemonLaunchError {
            XCTAssertEqual(error, .runtimeBuildIdentifierInvalid("../389"))
        }
        XCTAssertTrue(commands.arguments.isEmpty)
    }

    func testDaemonLauncherVerifiesLifecycleIdentityAndComputesExecutableDigest() async throws {
        let fixture = try makeDaemonLauncherFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let executable = try installDaemonRuntime(build: "389", fixture: fixture)
        let expectedDigest = "e5789e49e5f4d6a8782d3633321311cc53e016637a8c664ca186b140f1c24a3a"
        let lifecycle = lifecycleFixture(
            instanceId: "verified-instance",
            build: 389,
            executable: executable.path,
            executableSha256: expectedDigest,
            configPath: fixture.configuration.configURL.path
        )
        let locator = ActiveDaemonLocator(
            service: "threadrelay",
            apiMajor: 1,
            instanceId: lifecycle.service.instanceId,
            pid: lifecycle.service.pid,
            startedAtMs: lifecycle.service.startedAtMs,
            baseURL: "http://127.0.0.1:3847",
            controlFile: fixture.configuration.configURL.deletingLastPathComponent()
                .appendingPathComponent("threadrelay-control.json").path
        )
        let loadedAgentOutput = launchctlOutput(
            program: executable,
            configuration: fixture.configuration,
            build: "389",
            pid: 123
        )
        let commands = CommandInvocationRecorder { arguments in
            guard arguments.first == "print" else {
                return CommandResult(exitCode: 1, output: "unexpected command")
            }
            return CommandResult(
                exitCode: 0,
                output: loadedAgentOutput
            )
        }
        let launcher = DaemonLauncher(
            configurationLoader: { fixture.configuration },
            commandRunner: commands.run,
            activeDaemonLocatorLoader: { locator }
        )

        let identity = try await launcher.verifiedDaemonIdentity(for: lifecycle)
        let legacyIdentity = try await launcher.verifiedDaemonIdentity(
            for: lifecycleFixture(
                instanceId: "verified-instance",
                build: 389,
                executable: executable.path,
                configPath: fixture.configuration.configURL.path
            )
        )

        XCTAssertEqual(identity.executableSha256, expectedDigest)
        XCTAssertEqual(legacyIdentity, identity)
        XCTAssertEqual(identity.pid, 123)
        XCTAssertEqual(identity.startedAtMs, 456)
        XCTAssertEqual(identity.bind, "127.0.0.1:3847")
        XCTAssertEqual(commands.arguments, [
            ["print", "gui/\(getuid())/\(DaemonLaunchConfiguration.label)"],
            ["print", "gui/\(getuid())/\(DaemonLaunchConfiguration.label)"],
        ])
    }

    func testDaemonLauncherRejectsLifecycleIdentityWithWrongDigestOrLocator() async throws {
        let fixture = try makeDaemonLauncherFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let executable = try installDaemonRuntime(build: "389", fixture: fixture)
        let lifecycle = lifecycleFixture(
            instanceId: "verified-instance",
            build: 389,
            executable: executable.path,
            executableSha256: String(repeating: "0", count: 64),
            configPath: fixture.configuration.configURL.path
        )
        let locator = ActiveDaemonLocator(
            service: "threadrelay",
            apiMajor: 1,
            instanceId: lifecycle.service.instanceId,
            pid: lifecycle.service.pid,
            startedAtMs: lifecycle.service.startedAtMs,
            baseURL: "http://127.0.0.1:3847",
            controlFile: fixture.configuration.configURL.deletingLastPathComponent()
                .appendingPathComponent("threadrelay-control.json").path
        )
        let loadedAgentOutput = launchctlOutput(
            program: executable,
            configuration: fixture.configuration,
            build: "389",
            pid: 123
        )
        let commands = CommandInvocationRecorder { _ in
            CommandResult(
                exitCode: 0,
                output: loadedAgentOutput
            )
        }
        let digestMismatch = DaemonLauncher(
            configurationLoader: { fixture.configuration },
            commandRunner: commands.run,
            activeDaemonLocatorLoader: { locator }
        )

        do {
            _ = try await digestMismatch.verifiedDaemonIdentity(for: lifecycle)
            XCTFail("Expected mismatched executable digest to be rejected")
        } catch {
            XCTAssertEqual(error as? DaemonLaunchError, .loadedAgentUntrusted(executable.path))
        }

        let wrongLocator = ActiveDaemonLocator(
            service: locator.service,
            apiMajor: locator.apiMajor,
            instanceId: "another-instance",
            pid: locator.pid,
            startedAtMs: locator.startedAtMs,
            baseURL: locator.baseURL,
            controlFile: locator.controlFile
        )
        let locatorMismatch = DaemonLauncher(
            configurationLoader: { fixture.configuration },
            commandRunner: commands.run,
            activeDaemonLocatorLoader: { wrongLocator }
        )
        do {
            _ = try await locatorMismatch.verifiedDaemonIdentity(for: lifecycle)
            XCTFail("Expected mismatched active locator to be rejected")
        } catch {
            XCTAssertEqual(error as? DaemonLaunchError, .loadedAgentUntrusted(executable.path))
        }

        let wrongControlFile = ActiveDaemonLocator(
            service: locator.service,
            apiMajor: locator.apiMajor,
            instanceId: locator.instanceId,
            pid: locator.pid,
            startedAtMs: locator.startedAtMs,
            baseURL: locator.baseURL,
            controlFile: fixture.root.appendingPathComponent("another-control.json").path
        )
        let controlFileMismatch = DaemonLauncher(
            configurationLoader: { fixture.configuration },
            commandRunner: commands.run,
            activeDaemonLocatorLoader: { wrongControlFile }
        )
        do {
            _ = try await controlFileMismatch.verifiedDaemonIdentity(for: lifecycle)
            XCTFail("Expected mismatched control file to be rejected")
        } catch {
            XCTAssertEqual(error as? DaemonLaunchError, .loadedAgentUntrusted(executable.path))
        }
    }

    func testManagementCredentialStoreLoadsAllValidUniqueCandidatesInPathOrder() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }

        let stale = directory.appendingPathComponent("stale.json")
        let duplicate = directory.appendingPathComponent("duplicate.json")
        let current = directory.appendingPathComponent("current.json")
        let invalid = directory.appendingPathComponent("invalid.json")
        try Data(#"{"managementToken":"stale-token"}"#.utf8).write(to: stale)
        try Data(#"{"managementToken":"stale-token"}"#.utf8).write(to: duplicate)
        try Data(#"{"managementToken":"current-token"}"#.utf8).write(to: current)
        try Data(#"{"managementToken":" has-whitespace "}"#.utf8).write(to: invalid)

        XCTAssertEqual(
            ManagementCredentialStore.loadCandidates(
                from: [stale, duplicate, invalid, current]
            ),
            ["stale-token", "current-token"]
        )
    }

    func testManagementCredentialStorePrefersActiveDaemonLocator() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }

        let activeControl = directory.appendingPathComponent("active-control.json")
        let fallbackControl = directory.appendingPathComponent("fallback-control.json")
        try Data(#"{"managementToken":"active-token"}"#.utf8).write(to: activeControl)
        try Data(#"{"managementToken":"fallback-token"}"#.utf8).write(to: fallbackControl)
        let locator = ActiveDaemonLocator(
            service: "threadrelay",
            apiMajor: 1,
            instanceId: "active-instance",
            pid: 123,
            startedAtMs: 456,
            baseURL: "http://127.0.0.1:3847",
            controlFile: activeControl.path
        )

        XCTAssertEqual(
            ManagementCredentialStore.loadCredentialCandidates(
                locator: locator,
                fallbackPaths: [fallbackControl]
            ),
            [
                .init(token: "active-token", expectedInstanceId: "active-instance"),
                .init(token: "fallback-token", expectedInstanceId: nil),
            ]
        )
    }

    func testManagementCredentialStoreKeepsUnconstrainedFallbackForLocatorToken() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }

        let control = directory.appendingPathComponent("control.json")
        try Data(#"{"managementToken":"shared-token"}"#.utf8).write(to: control)
        let locator = ActiveDaemonLocator(
            service: "threadrelay",
            apiMajor: 1,
            instanceId: "stale-instance",
            pid: 123,
            startedAtMs: 456,
            baseURL: "http://127.0.0.1:3847",
            controlFile: control.path
        )

        XCTAssertEqual(
            ManagementCredentialStore.loadCredentialCandidates(
                locator: locator,
                fallbackPaths: [control]
            ),
            [
                .init(token: "shared-token", expectedInstanceId: "stale-instance"),
                .init(token: "shared-token", expectedInstanceId: nil),
            ]
        )
    }

    func testActiveDaemonLocatorDecodesFromPrivateDiscoveryFile() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let path = directory.appendingPathComponent("threadrelay-active-daemon.json")
        try Data(#"{"service":"threadrelay","apiMajor":1,"instanceId":"fixture-instance","pid":123,"startedAtMs":456,"baseUrl":"http://127.0.0.1:3847","controlFile":"/fixture/threadrelay-control.json"}"#.utf8).write(to: path)

        XCTAssertEqual(
            ManagementCredentialStore.loadLocator(from: path),
            ActiveDaemonLocator(
                service: "threadrelay",
                apiMajor: 1,
                instanceId: "fixture-instance",
                pid: 123,
                startedAtMs: 456,
                baseURL: "http://127.0.0.1:3847",
                controlFile: "/fixture/threadrelay-control.json"
            )
        )
    }

    func testActiveDaemonLocatorOnlyAcceptsExplicitLoopbackHTTPOrigins() {
        func locator(baseURL: String) -> ActiveDaemonLocator {
            ActiveDaemonLocator(
                service: "threadrelay",
                apiMajor: 1,
                instanceId: "fixture-instance",
                pid: 123,
                startedAtMs: 456,
                baseURL: baseURL,
                controlFile: "/fixture/threadrelay-control.json"
            )
        }

        XCTAssertEqual(
            locator(baseURL: "http://127.0.0.1:49321").validatedBaseURL,
            URL(string: "http://127.0.0.1:49321")
        )
        XCTAssertEqual(
            locator(baseURL: "http://[::1]:49321").validatedBaseURL,
            URL(string: "http://[::1]:49321")
        )
        XCTAssertNil(locator(baseURL: "https://127.0.0.1:49321").validatedBaseURL)
        XCTAssertNil(locator(baseURL: "http://localhost:49321").validatedBaseURL)
        XCTAssertNil(locator(baseURL: "http://192.0.2.1:49321").validatedBaseURL)
        XCTAssertNil(locator(baseURL: "http://127.0.0.1:49321/path").validatedBaseURL)
        XCTAssertNil(locator(baseURL: "http://user@127.0.0.1:49321").validatedBaseURL)
        XCTAssertNil(locator(baseURL: "http://127.0.0.1").validatedBaseURL)
    }

    func testAPIClientUsesDiscoveredLoopbackBaseURL() async throws {
        let client = makeClient(
            baseURL: URL(string: "https://fallback.test")!,
            baseURLLoader: { URL(string: "http://127.0.0.1:49321")! }
        ) { request in
            XCTAssertEqual(request.url?.scheme, "http")
            XCTAssertEqual(request.url?.host, "127.0.0.1")
            XCTAssertEqual(request.url?.port, 49321)
            return MockResponse(statusCode: 200, json: Self.healthJSON)
        }

        _ = try await client.probe()
    }

    func testNavigationContainsPhaseZeroSections() {
        XCTAssertEqual(
            AppSection.allCases.map(\.title),
            ["概览", "Codex 接入", "AI 网关", "消息渠道", "会话", "请求日志"]
        )
    }

    func testNavigationGroupsFollowSetupOrder() {
        XCTAssertEqual(
            AppSectionGroup.allCases.flatMap { $0.sections }.map(\.title),
            ["概览", "Codex 接入", "AI 网关", "消息渠道", "会话", "请求日志"]
        )
        XCTAssertEqual(
            AppSectionGroup.allCases.map(\.title),
            [nil, "配置", "诊断"]
        )
    }

    func testServiceProbeKeepsVersionedHealthPayload() {
        let health = HealthResponse(service: "threadrelay", apiMajor: 1, ready: true)
        XCTAssertEqual(ServiceProbe.versioned(health), .versioned(health))
    }

    func testProbeUsesVersionedHealthEndpoint() async throws {
        let recorder = RequestRecorder()
        let client = makeClient { request in
            recorder.record(request.url?.path)
            return MockResponse(
                statusCode: 200,
                json: #"{"service":"threadrelay","apiMajor":1,"ready":true}"#
            )
        }

        let result = try await client.probe()

        XCTAssertEqual(
            result,
            .versioned(HealthResponse(service: "threadrelay", apiMajor: 1, ready: true))
        )
        XCTAssertEqual(recorder.paths, ["/healthz"])
    }

    func testProbeUsesInjectedNonDefaultLoopbackPort() async throws {
        let recorder = URLRecorder()
        let client = makeClient(
            baseURL: URL(string: "http://127.0.0.1:49321")!
        ) { request in
            recorder.record(request.url)
            return MockResponse(statusCode: 200, json: Self.healthJSON)
        }

        _ = try await client.probe()

        XCTAssertEqual(recorder.urls.first?.host, "127.0.0.1")
        XCTAssertEqual(recorder.urls.first?.port, 49321)
        XCTAssertEqual(recorder.urls.first?.path, "/healthz")
    }

    func testProbeFallsBackToLegacyStatusAfterHealthNotFound() async throws {
        let recorder = RequestRecorder()
        let client = makeClient { request in
            let path = request.url?.path
            recorder.record(path)

            switch path {
            case "/healthz":
                return MockResponse(statusCode: 404, json: #"{"error":"not found"}"#)
            case "/api/status":
                return MockResponse(statusCode: 200, json: #"{"service":"threadrelay"}"#)
            default:
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }

        let result = try await client.probe()

        XCTAssertEqual(result, .legacy)
        XCTAssertEqual(recorder.paths, ["/healthz", "/api/status"])
    }

    func testProbeRejectsAnotherServiceOnThreadRelayPort() async {
        let client = makeClient { _ in
            MockResponse(
                statusCode: 200,
                json: #"{"service":"another-service","apiMajor":1,"ready":true}"#
            )
        }

        await assertProbeError(.incompatibleService, from: client)
    }

    func testProbeRejectsUnsupportedThreadRelayAPIMajorSeparately() async {
        let client = makeClient { _ in
            MockResponse(
                statusCode: 200,
                json: #"{"service":"threadrelay","apiMajor":2,"ready":true}"#
            )
        }

        await assertProbeError(.unsupportedAPIMajor(2), from: client)
        XCTAssertEqual(
            APIClientError.unsupportedAPIMajor(2).localizedDescription,
            "当前 ThreadRelay 使用了不受支持的管理 API 版本 2。"
        )
        XCTAssertNotEqual(
            APIClientError.unsupportedAPIMajor(2).localizedDescription,
            APIClientError.incompatibleService.localizedDescription
        )
    }

    func testProbeMapsMalformedJSONToInvalidResponse() async {
        let client = makeClient { _ in
            MockResponse(statusCode: 200, json: #"{"service":"threadrelay""#)
        }

        await assertProbeError(.invalidResponse, from: client)
    }

    func testProbeMapsNonSuccessStatusToInvalidResponse() async {
        let client = makeClient { _ in
            MockResponse(statusCode: 503, json: #"{"error":"unavailable"}"#)
        }

        await assertProbeError(.invalidResponse, from: client)
    }

    func testFetchDashboardDecodesAggregateFixtureAndSendsBearerHeader() async throws {
        let client = makeClient { request in
            XCTAssertEqual(request.url?.path, "/api/v1/manage/dashboard")
            XCTAssertEqual(request.httpMethod, "GET")
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Authorization"),
                "Bearer explicit-fixture-token"
            )
            return MockResponse(
                statusCode: 200,
                json: #"{"service":{"service":"threadrelay","apiMajor":1,"ready":true,"instanceId":"fixture-instance","pid":123,"startedAtMs":456},"bridgeRunning":true,"remoteControlConnected":true,"remoteControlHealthy":true,"executionClients":{"codexApp":{"configured":true,"connected":true},"vscode":{"configured":true,"connected":true},"cli":{"configured":false,"connected":false}},"messageChannels":{"telegram":{"accountCount":2,"connectedAccountCount":1},"feishu":{"accountCount":1,"connectedAccountCount":1},"wechat":{"accountCount":1,"connectedAccountCount":1},"wecom":{"accountCount":0,"connectedAccountCount":0}},"aiGatewayEnabled":true,"aiGatewayProviderCount":2,"requestLoggingEnabled":true}"#
            )
        }

        let dashboard = try await client.fetchDashboard(bearerToken: "explicit-fixture-token")

        XCTAssertEqual(
            dashboard,
            ManageDashboard(
                service: ManageDashboard.Service(
                    service: "threadrelay",
                    apiMajor: 1,
                    ready: true,
                    instanceId: "fixture-instance",
                    pid: 123,
                    startedAtMs: 456
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
                    telegram: .init(accountCount: 2, connectedAccountCount: 1),
                    feishu: .init(accountCount: 1, connectedAccountCount: 1),
                    wechat: .init(accountCount: 1, connectedAccountCount: 1),
                    wecom: .init(accountCount: 0, connectedAccountCount: 0)
                ),
                aiGatewayEnabled: true,
                aiGatewayProviderCount: 2,
                requestLoggingEnabled: true
            )
        )
    }

    func testFetchIMAccountsDecodesAccountStateAndSendsBearerHeader() async throws {
        let client = makeClient { request in
            XCTAssertEqual(request.url?.path, "/api/v1/manage/im/accounts")
            XCTAssertEqual(request.httpMethod, "GET")
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Authorization"),
                "Bearer explicit-fixture-token"
            )
            return MockResponse(statusCode: 200, json: Self.imAccountsJSON)
        }

        let response = try await client.fetchIMAccounts(bearerToken: "explicit-fixture-token")

        XCTAssertEqual(response.service.service, "threadrelay")
        XCTAssertEqual(response.service.apiMajor, 1)
        XCTAssertEqual(response.service.instanceId, "fixture-instance")
        XCTAssertEqual(response.accounts.count, 2)
        XCTAssertEqual(
            response.accounts[0],
            ManageIMAccount(
                platform: "telegram",
                accountId: "telegram-main",
                displayName: "主 Telegram",
                avatarData: nil,
                enabled: true,
                configured: true,
                secretSet: true,
                connecting: false,
                polling: true,
                connected: true,
                lastError: nil,
                lastEventAtMs: 1_754_000_120_000,
                lastInboundAtMs: 1_754_000_100_000
            )
        )
        XCTAssertEqual(response.accounts[1].lastError, "连接失败")
        XCTAssertFalse(response.accounts[1].connected)
    }

    func testFetchIMAccountsMapsNotFoundToFeatureUnavailable() async {
        let client = makeClient { request in
            XCTAssertEqual(request.url?.path, "/api/v1/manage/im/accounts")
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Authorization"),
                "Bearer fixture-token"
            )
            return MockResponse(statusCode: 404, json: #"{"error":"not found"}"#)
        }

        do {
            _ = try await client.fetchIMAccounts(bearerToken: "fixture-token")
            XCTFail("Expected account management feature to be unavailable")
        } catch let error as APIClientError {
            XCTAssertEqual(error, .featureUnavailable)
        } catch {
            XCTFail("Expected APIClientError, received \(error)")
        }
    }

    func testSetIMAccountEnabledSendsPOSTBodyAndBearerHeader() async throws {
        let client = makeClient { request in
            XCTAssertEqual(request.url?.path, "/api/v1/manage/im/account/enabled")
            XCTAssertEqual(request.httpMethod, "POST")
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Authorization"),
                "Bearer fixture-token"
            )
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Content-Type"),
                "application/json"
            )
            let body = Self.jsonBody(from: request)
            XCTAssertEqual(body?["platform"] as? String, "telegram")
            XCTAssertEqual(body?["accountId"] as? String, "telegram-main")
            XCTAssertEqual(body?["enabled"] as? Bool, false)
            return MockResponse(
                statusCode: 200,
                json: #"{"ok":true,"platform":"telegram","accountId":"telegram-main","enabled":false}"#
            )
        }

        let response = try await client.setIMAccountEnabled(
            platform: "telegram",
            accountId: "telegram-main",
            enabled: false
        )

        XCTAssertTrue(response.ok)
        XCTAssertEqual(response.platform, "telegram")
        XCTAssertEqual(response.accountId, "telegram-main")
        XCTAssertEqual(response.enabled, false)
    }

    func testDeleteIMAccountSendsPOSTBodyAndMapsNotFoundError() async {
        let client = makeClient { request in
            XCTAssertEqual(request.url?.path, "/api/v1/manage/im/account/delete")
            XCTAssertEqual(request.httpMethod, "POST")
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Authorization"),
                "Bearer fixture-token"
            )
            let body = Self.jsonBody(from: request)
            XCTAssertEqual(body?["platform"] as? String, "telegram")
            XCTAssertEqual(body?["accountId"] as? String, "telegram-main")
            return MockResponse(
                statusCode: 404,
                json: #"{"ok":false,"error":"IM account not found"}"#
            )
        }

        do {
            _ = try await client.deleteIMAccount(
                platform: "telegram",
                accountId: "telegram-main"
            )
            XCTFail("Expected missing account error")
        } catch let error as APIClientError {
            XCTAssertEqual(error, .operationFailed("找不到该消息账号。"))
        } catch {
            XCTFail("Expected APIClientError, received \(error)")
        }
    }

    func testIMMutationMapsRouteMissingOnOlderDaemonToFeatureUnavailable() async {
        let client = makeClient { request in
            XCTAssertEqual(request.httpMethod, "POST")
            return MockResponse(statusCode: 404, json: "Not Found")
        }

        do {
            _ = try await client.setIMAccountEnabled(
                platform: "telegram",
                accountId: "telegram-main",
                enabled: false
            )
            XCTFail("Expected feature-unavailable error")
        } catch let error as APIClientError {
            XCTAssertEqual(error, .featureUnavailable)
        } catch {
            XCTFail("Expected APIClientError, received \(error)")
        }
    }

    func testConfigureTelegramAccountSendsTokenAndDecodesDerivedIdentity() async throws {
        let client = makeClient { request in
            XCTAssertEqual(request.url?.path, "/api/v1/manage/im/account/telegram")
            XCTAssertEqual(request.httpMethod, "POST")
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Authorization"),
                "Bearer fixture-token"
            )
            let body = Self.jsonBody(from: request)
            XCTAssertEqual(body?["botToken"] as? String, "12345:fixture")
            XCTAssertEqual(body?["mentionOnly"] as? Bool, true)
            return MockResponse(
                statusCode: 200,
                json: #"{"ok":true,"platform":"telegram","accountId":"tg_1000001","displayName":"Fixture Bot (@fixture_bot)"}"#
            )
        }

        let response = try await client.configureTelegramAccount(
            botToken: "12345:fixture",
            mentionOnly: true
        )

        XCTAssertTrue(response.ok)
        XCTAssertEqual(response.platform, "telegram")
        XCTAssertEqual(response.accountId, "tg_1000001")
        XCTAssertEqual(response.displayName, "Fixture Bot (@fixture_bot)")
    }

    func testConfigureTelegramAccountSurfacesDaemonValidationReason() async {
        let client = makeClient { _ in
            MockResponse(
                statusCode: 400,
                json: #"{"ok":false,"error":"telegram getMe failed: 401 Unauthorized"}"#
            )
        }

        do {
            _ = try await client.configureTelegramAccount(botToken: "bad", mentionOnly: false)
            XCTFail("Expected validation failure")
        } catch let error as APIClientError {
            XCTAssertEqual(error, .operationFailed("telegram getMe failed: 401 Unauthorized"))
        } catch {
            XCTFail("Expected APIClientError, received \(error)")
        }
    }

    func testConfigureFeishuAccountSendsCredentialsAndDecodesIdentity() async throws {
        let client = makeClient { request in
            XCTAssertEqual(request.url?.path, "/api/v1/manage/im/account/feishu")
            XCTAssertEqual(request.httpMethod, "POST")
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Authorization"),
                "Bearer fixture-token"
            )
            let body = Self.jsonBody(from: request)
            XCTAssertEqual(body?["appId"] as? String, "cli-fixture-app")
            XCTAssertEqual(body?["appSecret"] as? String, "fixture-secret")
            return MockResponse(
                statusCode: 200,
                json: #"{"ok":true,"platform":"feishu","accountId":"cli-fixture-app","displayName":"Fixture 应用"}"#
            )
        }

        let response = try await client.configureFeishuAccount(
            appId: "cli-fixture-app",
            appSecret: "fixture-secret"
        )

        XCTAssertTrue(response.ok)
        XCTAssertEqual(response.platform, "feishu")
        XCTAssertEqual(response.accountId, "cli-fixture-app")
        XCTAssertEqual(response.displayName, "Fixture 应用")
    }

    func testFeishuOnboardingStartAndPendingPollDecode() async throws {
        let client = makeClient { request in
            switch request.url?.path {
            case "/api/v1/manage/im/onboarding/feishu/start":
                XCTAssertEqual(request.httpMethod, "POST")
                return MockResponse(
                    statusCode: 200,
                    json: #"{"verificationUri":"https://f.example/v","verificationUriComplete":"https://f.example/v?code=1","deviceCode":"device-1","expiresIn":600,"interval":5,"qrSvg":"<svg/>"}"#
                )
            case "/api/v1/manage/im/onboarding/feishu/poll":
                let body = Self.jsonBody(from: request)
                XCTAssertEqual(body?["deviceCode"] as? String, "device-1")
                return MockResponse(
                    statusCode: 200,
                    json: #"{"done":false,"appId":null,"displayName":null,"error":"authorization_pending","errorDescription":null}"#
                )
            default:
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }

        let session = try await client.startFeishuOnboarding()
        XCTAssertEqual(session.deviceCode, "device-1")
        XCTAssertEqual(session.verificationUriComplete, "https://f.example/v?code=1")
        XCTAssertEqual(session.interval, 5)

        let poll = try await client.pollFeishuOnboarding(deviceCode: session.deviceCode)
        XCTAssertFalse(poll.done)
        XCTAssertEqual(poll.error, "authorization_pending")
        XCTAssertNil(poll.appId)
    }

    func testWechatOnboardingPollSendsVerifyCodeAndDecodesStates() async throws {
        let client = makeClient { request in
            switch request.url?.path {
            case "/api/v1/manage/im/onboarding/wechat/start":
                return MockResponse(
                    statusCode: 200,
                    json: #"{"sessionKey":"wechat-onboard-1","qrcodeUrl":"https://w.example/qr","qrSvg":"<svg/>","expiresIn":300}"#
                )
            case "/api/v1/manage/im/onboarding/wechat/poll":
                let body = Self.jsonBody(from: request)
                XCTAssertEqual(body?["sessionKey"] as? String, "wechat-onboard-1")
                if body?["verifyCode"] as? String == "246810" {
                    return MockResponse(
                        statusCode: 200,
                        json: #"{"done":true,"status":"confirmed","accountId":"ilink-bot-1","userId":"user-1"}"#
                    )
                }
                return MockResponse(
                    statusCode: 200,
                    json: #"{"done":false,"status":"need_verifycode","needVerifyCode":true,"error":null}"#
                )
            default:
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }

        let session = try await client.startWechatOnboarding()
        XCTAssertEqual(session.sessionKey, "wechat-onboard-1")
        XCTAssertEqual(session.qrcodeUrl, "https://w.example/qr")

        let pending = try await client.pollWechatOnboarding(sessionKey: session.sessionKey)
        XCTAssertFalse(pending.done)
        XCTAssertEqual(pending.needVerifyCode, true)

        let confirmed = try await client.pollWechatOnboarding(
            sessionKey: session.sessionKey,
            verifyCode: "246810"
        )
        XCTAssertTrue(confirmed.done)
        XCTAssertEqual(confirmed.accountId, "ilink-bot-1")
    }

    func testWecomOnboardingStartAndCompletionDecode() async throws {
        let client = makeClient { request in
            switch request.url?.path {
            case "/api/v1/manage/im/onboarding/wecom/start":
                return MockResponse(
                    statusCode: 200,
                    json: #"{"sessionKey":"wecom-onboard-1","qrcodeUrl":"https://q.example/auth","qrSvg":"<svg/>","expiresIn":300,"interval":3}"#
                )
            case "/api/v1/manage/im/onboarding/wecom/poll":
                let body = Self.jsonBody(from: request)
                XCTAssertEqual(body?["sessionKey"] as? String, "wecom-onboard-1")
                return MockResponse(
                    statusCode: 200,
                    json: #"{"done":true,"status":"success","accountId":"wecom-bot-1"}"#
                )
            default:
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }

        let session = try await client.startWecomOnboarding()
        XCTAssertEqual(session.sessionKey, "wecom-onboard-1")
        XCTAssertEqual(session.interval, 3)

        let poll = try await client.pollWecomOnboarding(sessionKey: session.sessionKey)
        XCTAssertTrue(poll.done)
        XCTAssertEqual(poll.accountId, "wecom-bot-1")
    }

    func testOnboardingSessionErrorsMapToFriendlyMessage() async {
        let client = makeClient { _ in
            MockResponse(
                statusCode: 400,
                json: #"{"done":false,"error":"missing_session"}"#
            )
        }

        do {
            _ = try await client.pollWecomOnboarding(sessionKey: "stale")
            XCTFail("Expected session failure")
        } catch let error as APIClientError {
            XCTAssertEqual(error, .operationFailed("扫码会话已失效，请重新获取二维码。"))
        } catch {
            XCTFail("Expected APIClientError, received \(error)")
        }
    }

    func testSupersededOnboardingStartMapsToFriendlyMessage() async {
        let client = makeClient { _ in
            MockResponse(statusCode: 409, json: #"{"error":"superseded"}"#)
        }

        do {
            _ = try await client.startWecomOnboarding()
            XCTFail("Expected superseded start failure")
        } catch let error as APIClientError {
            XCTAssertEqual(error, .operationFailed("当前扫码请求已被新的二维码替代。"))
        } catch {
            XCTFail("Expected APIClientError, received \(error)")
        }
    }

    func testFetchDashboardDecodesOriginalV1AggregatePayload() async throws {
        let client = makeClient { _ in
            MockResponse(statusCode: 200, json: Self.originalV1DashboardJSON)
        }

        let dashboard = try await client.fetchDashboard(bearerToken: "fixture-token")

        XCTAssertEqual(
            dashboard.executionClients.codexApp,
            .init(configured: true, connected: false)
        )
        XCTAssertEqual(
            dashboard.executionClients.vscode,
            .init(configured: false, connected: false)
        )
        XCTAssertEqual(
            dashboard.executionClients.cli,
            .init(configured: false, connected: false)
        )
        XCTAssertEqual(
            dashboard.messageChannels.legacyUnattributed,
            .init(accountCount: 5, connectedAccountCount: 3)
        )
        XCTAssertEqual(
            dashboard.messageChannels.telegram,
            .init(accountCount: 0, connectedAccountCount: 0)
        )
    }

    func testFetchDashboardRejectsUnsupportedThreadRelayAPIMajorSeparately() async {
        let client = makeClient { _ in
            MockResponse(
                statusCode: 200,
                json: Self.dashboardJSON.replacingOccurrences(
                    of: #""apiMajor":1"#,
                    with: #""apiMajor":7"#
                )
            )
        }

        await assertDashboardError(.unsupportedAPIMajor(7), from: client)
    }

    func testFetchDashboardMapsUnauthorizedResponse() async {
        let client = makeClient { _ in
            MockResponse(statusCode: 401, json: #"{"error":"unauthorized"}"#)
        }

        do {
            _ = try await client.fetchDashboard(bearerToken: "wrong-token")
            XCTFail("Expected unauthorized dashboard response")
        } catch let error as APIClientError {
            guard case .unauthorized = error else {
                XCTFail("Expected unauthorized, received \(error)")
                return
            }
        } catch {
            XCTFail("Expected APIClientError, received \(error)")
        }
    }

    func testFetchDashboardMapsServiceUnavailableToInvalidResponse() async {
        let client = makeClient { _ in
            MockResponse(statusCode: 503, json: #"{"error":"temporarily unavailable"}"#)
        }

        await assertDashboardError(.invalidResponse, from: client)
    }

    func testFetchDashboardMapsMalformedJSONToInvalidResponse() async {
        let client = makeClient { _ in
            MockResponse(statusCode: 200, json: #"{"service":{"service":"threadrelay""#)
        }

        await assertDashboardError(.invalidResponse, from: client)
    }

    func testDashboardTriesCurrentCredentialAfterStaleCandidateReturnsUnauthorized() async throws {
        let headers = StringRecorder()
        let client = makeClient(credentialCandidatesLoader: {
            [
                .init(token: "stale-token", expectedInstanceId: nil),
                .init(token: "current-token", expectedInstanceId: nil),
            ]
        }) { request in
            let authorization = request.value(forHTTPHeaderField: "Authorization") ?? ""
            headers.record(authorization)
            if authorization == "Bearer current-token" {
                return MockResponse(statusCode: 200, json: Self.dashboardJSON)
            }
            return MockResponse(statusCode: 401, json: #"{"error":"unauthorized"}"#)
        }

        let dashboard = try await client.dashboard()

        XCTAssertEqual(dashboard.service.instanceId, "fixture-instance")
        XCTAssertEqual(headers.values, ["Bearer stale-token", "Bearer current-token"])
    }

    func testDashboardReturnsUnauthorizedAfterAllCredentialCandidatesFail() async {
        let headers = StringRecorder()
        let client = makeClient(credentialCandidatesLoader: {
            [
                .init(token: "stale-token", expectedInstanceId: nil),
                .init(token: "also-stale-token", expectedInstanceId: nil),
            ]
        }) { request in
            headers.record(request.value(forHTTPHeaderField: "Authorization") ?? "")
            return MockResponse(statusCode: 401, json: #"{"error":"unauthorized"}"#)
        }

        do {
            _ = try await client.dashboard()
            XCTFail("Expected every credential candidate to be rejected")
        } catch let error as APIClientError {
            XCTAssertEqual(error, .unauthorized)
        } catch {
            XCTFail("Expected APIClientError, received \(error)")
        }
        XCTAssertEqual(headers.values, ["Bearer stale-token", "Bearer also-stale-token"])
    }

    func testDashboardRejectsLocatorTokenForAReplacedDaemonInstance() async throws {
        let headers = StringRecorder()
        let client = makeClient(credentialCandidatesLoader: {
            [
                .init(token: "locator-token", expectedInstanceId: "stale-instance"),
                .init(token: "fallback-token", expectedInstanceId: nil),
            ]
        }) { request in
            headers.record(request.value(forHTTPHeaderField: "Authorization") ?? "")
            return MockResponse(statusCode: 200, json: Self.dashboardJSON)
        }

        let dashboard = try await client.dashboard()

        XCTAssertEqual(dashboard.service.instanceId, "fixture-instance")
        XCTAssertEqual(headers.values, ["Bearer locator-token", "Bearer fallback-token"])
    }

    func testFetchLogDirectoryUsesProtectedManagementRoute() async throws {
        let client = makeClient { request in
            XCTAssertEqual(request.url?.path, "/api/v1/manage/log-directory")
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Authorization"),
                "Bearer fixture-token"
            )
            return MockResponse(
                statusCode: 200,
                json: #"{"directory":"/fixture/custom-state/logs","instanceId":"fixture-instance"}"#
            )
        }

        let directory = try await client.fetchLogDirectory(bearerToken: "fixture-token")
        XCTAssertEqual(directory.path, "/fixture/custom-state/logs")
    }

    func testFetchLifecycleDecodesReadOnlyRuntimeSnapshot() async throws {
        let client = makeClient { request in
            XCTAssertEqual(request.url?.path, "/api/v1/manage/lifecycle")
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Authorization"),
                "Bearer fixture-token"
            )
            return MockResponse(
                statusCode: 200,
                json: #"{"service":{"service":"threadrelay","apiMajor":1,"ready":true,"instanceId":"fixture-instance","pid":123,"startedAtMs":456},"executable":"/fixture/ThreadRelay","executableSha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","configPath":"/fixture/config.toml","bind":"127.0.0.1:3847","runtime":{"state":"active","productVersion":"0.5.0","buildNumber":388,"apiMajor":1},"protectedWorkItems":{"aiGatewayRequests":1,"codexTurns":2,"imStreams":3,"pendingApprovals":1,"remoteControlRequests":4,"total":11},"management":{"state":"unmanaged","mode":"readOnly","canControl":false,"installationId":null,"leaseGeneration":null,"leaseExpiresAtMs":null,"managementTokenGeneration":9}}"#
            )
        }

        let lifecycle = try await client.fetchLifecycle(bearerToken: "fixture-token")

        XCTAssertEqual(lifecycle.service.instanceId, "fixture-instance")
        XCTAssertEqual(lifecycle.runtime.productVersion, "0.5.0")
        XCTAssertEqual(lifecycle.runtime.buildNumber, 388)
        XCTAssertEqual(
            lifecycle.executableSha256,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        )
        XCTAssertEqual(lifecycle.protectedWorkItems.total, 11)
        XCTAssertEqual(lifecycle.management.mode, "readOnly")
        XCTAssertFalse(lifecycle.management.canControl)
        XCTAssertEqual(lifecycle.management.managementTokenGeneration, 9)
    }

    func testFetchLifecycleAcceptsOlderDaemonWithoutBuildNumber() async throws {
        let client = makeClient { _ in
            MockResponse(
                statusCode: 200,
                json: #"{"service":{"service":"threadrelay","apiMajor":1,"ready":true,"instanceId":"fixture-instance","pid":123,"startedAtMs":456},"executable":"/fixture/ThreadRelay","configPath":"/fixture/config.toml","bind":"127.0.0.1:3847","runtime":{"state":"active","productVersion":"0.4.21","apiMajor":1},"protectedWorkItems":{"aiGatewayRequests":0,"codexTurns":0,"imStreams":0,"pendingApprovals":0,"remoteControlRequests":0,"total":0},"management":{"state":"unmanaged","mode":"readOnly","canControl":false,"installationId":null,"leaseGeneration":null,"leaseExpiresAtMs":null}}"#
            )
        }

        let lifecycle = try await client.fetchLifecycle(bearerToken: "fixture-token")

        XCTAssertNil(lifecycle.runtime.buildNumber)
        XCTAssertNil(lifecycle.executableSha256)
        XCTAssertNil(lifecycle.management.managementTokenGeneration)
    }

    func testLifecycleMutationEndpointsUseVersionedRoutesAndIdentity() async throws {
        let identity = ManageDaemonIdentity(
            pid: 123,
            startedAtMs: 456,
            executable: "/fixture/ThreadRelay",
            executableSha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            bind: "127.0.0.1:3847"
        )
        let client = makeClient { request in
            let path = request.url?.path
            let body = Self.jsonBody(from: request)
            XCTAssertEqual(body?["installationId"] as? String, "installation-a")
            XCTAssertEqual(body?["daemonInstanceId"] as? String, "fixture-instance")

            switch path {
            case "/api/v1/manage/lifecycle/lease/claim",
                 "/api/v1/manage/lifecycle/lease/renew",
                 "/api/v1/manage/lifecycle/lease/release":
                let daemonIdentity = body?["daemonIdentity"] as? [String: Any]
                XCTAssertEqual(daemonIdentity?["pid"] as? Int, 123)
                XCTAssertEqual(daemonIdentity?["startedAtMs"] as? Int, 456)
                XCTAssertEqual(daemonIdentity?["executable"] as? String, "/fixture/ThreadRelay")
                XCTAssertEqual(
                    daemonIdentity?["executableSha256"] as? String,
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                )
                XCTAssertEqual(daemonIdentity?["bind"] as? String, "127.0.0.1:3847")
                return MockResponse(statusCode: 200, json: Self.lifecycleJSON)
            case "/api/v1/manage/lifecycle/lease/takeover":
                XCTAssertEqual(body?["expectedLeaseGeneration"] as? Int, 7)
                XCTAssertEqual(body?["expectedManagementTokenGeneration"] as? Int, 9)
                XCTAssertEqual(body?["requestId"] as? String, "request-takeover")
                XCTAssertEqual(body?["force"] as? Bool, true)
                XCTAssertNotNil(body?["daemonIdentity"] as? [String: Any])
                return MockResponse(
                    statusCode: 200,
                    json: #"{"ok":true,"rotated":true,"requestId":"request-takeover","managementTokenGeneration":10}"#
                )
            case "/api/v1/manage/lifecycle/credential/rotate":
                XCTAssertEqual(body?["leaseGeneration"] as? Int, 7)
                XCTAssertEqual(body?["expectedManagementTokenGeneration"] as? Int, 10)
                XCTAssertEqual(body?["requestId"] as? String, "request-rotate")
                XCTAssertEqual(body?["reason"] as? String, "leakRecovery")
                XCTAssertNil(body?["daemonIdentity"])
                return MockResponse(
                    statusCode: 200,
                    json: #"{"ok":true,"rotated":true,"requestId":"request-rotate","managementTokenGeneration":11}"#
                )
            case "/api/v1/manage/lifecycle/restart":
                XCTAssertEqual(body?["force"] as? Bool, false)
                XCTAssertEqual(body?["leaseGeneration"] as? Int, 7)
                return MockResponse(statusCode: 200, json: #"{"ok":true,"state":"restarting"}"#)
            default:
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }

        _ = try await client.claimLifecycleLease(
            installationId: "installation-a",
            daemonInstanceId: "fixture-instance",
            daemonIdentity: identity
        )
        _ = try await client.renewLifecycleLease(
            installationId: "installation-a",
            daemonInstanceId: "fixture-instance",
            daemonIdentity: identity
        )
        _ = try await client.releaseLifecycleLease(
            installationId: "installation-a",
            daemonInstanceId: "fixture-instance",
            daemonIdentity: identity
        )
        let takeover = try await client.takeOverLifecycleLease(
            installationId: "installation-a",
            daemonInstanceId: "fixture-instance",
            expectedLeaseGeneration: 7,
            expectedManagementTokenGeneration: 9,
            requestId: "request-takeover",
            daemonIdentity: identity
        )
        XCTAssertEqual(takeover.managementTokenGeneration, 10)
        let rotation = try await client.rotateManagementCredential(
            installationId: "installation-a",
            daemonInstanceId: "fixture-instance",
            leaseGeneration: 7,
            expectedManagementTokenGeneration: 10,
            requestId: "request-rotate"
        )
        XCTAssertEqual(rotation.managementTokenGeneration, 11)
        let restart = try await client.restartLifecycle(
            installationId: "installation-a",
            daemonInstanceId: "fixture-instance",
            leaseGeneration: 7
        )
        XCTAssertTrue(restart.ok)
    }

    func testIdempotentLifecycleMutationsRetryLostResponsesWithSameRequestId() async throws {
        let takeoverAttempts = IntCounter()
        let takeoverRequestIds = StringRecorder()
        let rotationAttempts = IntCounter()
        let rotationRequestIds = StringRecorder()
        let authorizations = StringRecorder()
        let credential = LockedValue("initial-token")
        let client = makeClient(credentialLoader: { credential.value }) { request in
            let requestId = Self.jsonBody(from: request)?["requestId"] as? String ?? ""
            authorizations.record(request.value(forHTTPHeaderField: "Authorization") ?? "")
            switch request.url?.path {
            case "/api/v1/manage/lifecycle/lease/takeover":
                takeoverRequestIds.record(requestId)
                guard takeoverAttempts.next() > 1 else {
                    credential.value = "takeover-token"
                    return MockResponse(error: URLError(.networkConnectionLost))
                }
                return MockResponse(
                    statusCode: 200,
                    json: #"{"ok":true,"rotated":false,"requestId":"request-takeover","managementTokenGeneration":10}"#
                )
            case "/api/v1/manage/lifecycle/credential/rotate":
                rotationRequestIds.record(requestId)
                guard rotationAttempts.next() > 1 else {
                    credential.value = "rotation-token"
                    return MockResponse(error: URLError(.timedOut))
                }
                return MockResponse(
                    statusCode: 200,
                    json: #"{"ok":true,"rotated":false,"requestId":"request-rotate","managementTokenGeneration":11}"#
                )
            default:
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }
        let identity = ManageDaemonIdentity(
            pid: 123,
            startedAtMs: 456,
            executable: "/fixture/ThreadRelay",
            executableSha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            bind: "127.0.0.1:3847"
        )

        let takeover = try await client.takeOverLifecycleLease(
            installationId: "installation-a",
            daemonInstanceId: "fixture-instance",
            expectedLeaseGeneration: 7,
            expectedManagementTokenGeneration: 9,
            requestId: "request-takeover",
            daemonIdentity: identity
        )
        let rotation = try await client.rotateManagementCredential(
            installationId: "installation-a",
            daemonInstanceId: "fixture-instance",
            leaseGeneration: 8,
            expectedManagementTokenGeneration: 10,
            requestId: "request-rotate"
        )

        XCTAssertFalse(takeover.rotated)
        XCTAssertFalse(rotation.rotated)
        XCTAssertEqual(takeoverRequestIds.values, ["request-takeover", "request-takeover"])
        XCTAssertEqual(rotationRequestIds.values, ["request-rotate", "request-rotate"])
        XCTAssertEqual(authorizations.values, [
            "Bearer initial-token",
            "Bearer takeover-token",
            "Bearer takeover-token",
            "Bearer rotation-token",
        ])
    }

    func testFetchLifecycleRejectsUnsupportedRuntimeAPIMajor() async {
        let client = makeClient { _ in
            MockResponse(
                statusCode: 200,
                json: #"{"service":{"service":"threadrelay","apiMajor":1,"ready":true,"instanceId":"fixture-instance","pid":123,"startedAtMs":456},"executable":"/fixture/ThreadRelay","configPath":"/fixture/config.toml","bind":"127.0.0.1:3847","runtime":{"state":"active","productVersion":"0.5.0","apiMajor":2},"protectedWorkItems":{"aiGatewayRequests":0,"codexTurns":0,"imStreams":0,"pendingApprovals":0,"remoteControlRequests":0,"total":0},"management":{"state":"unmanaged","mode":"readOnly","canControl":false,"installationId":null,"leaseGeneration":null,"leaseExpiresAtMs":null}}"#
            )
        }

        await assertLifecycleError(.unsupportedAPIMajor(2), from: client)
    }

    @MainActor
    func testAppModelOnlyTakesOverConflictingLeaseAfterExplicitRequest() async {
        let owner = LockedValue<String?>("other-installation")
        let tokenGeneration = LockedValue<Int64>(3)
        let takeoverCalls = IntCounter()
        let launcher = IdentityVerifyingDaemonLauncher()
        let client = makeClient { request in
            switch (request.httpMethod, request.url?.path) {
            case ("GET", "/healthz"):
                return MockResponse(statusCode: 200, json: Self.healthJSON)
            case ("GET", "/api/v1/manage/dashboard"):
                return MockResponse(statusCode: 200, json: Self.dashboardJSON)
            case ("GET", "/api/v1/manage/lifecycle"):
                return MockResponse(
                    statusCode: 200,
                    json: Self.lifecycleSecurityPayload(
                        installationId: owner.value,
                        canControl: owner.value != "other-installation",
                        managementTokenGeneration: tokenGeneration.value
                    )
                )
            case ("POST", "/api/v1/manage/lifecycle/lease/claim"):
                XCTFail("An active conflicting lease must never be claimed automatically")
                return MockResponse(statusCode: 409, json: #"{"error":"lease conflict"}"#)
            case ("POST", "/api/v1/manage/lifecycle/lease/takeover"):
                _ = takeoverCalls.next()
                let body = Self.jsonBody(from: request)
                let installationId = body?["installationId"] as? String
                let requestId = body?["requestId"] as? String ?? ""
                XCTAssertEqual(body?["expectedLeaseGeneration"] as? Int, 7)
                XCTAssertEqual(body?["expectedManagementTokenGeneration"] as? Int, 3)
                XCTAssertEqual(body?["force"] as? Bool, true)
                XCTAssertNotNil(body?["daemonIdentity"] as? [String: Any])
                owner.value = installationId
                tokenGeneration.value = 4
                return MockResponse(
                    statusCode: 200,
                    json: #"{"ok":true,"rotated":true,"requestId":"\#(requestId)","managementTokenGeneration":4}"#
                )
            default:
                return MockResponse(statusCode: 404, json: "")
            }
        }
        let model = AppModel(
            apiClient: client,
            daemonLauncher: launcher,
            guiBuildLoader: { "388" }
        )

        await model.refresh()

        XCTAssertTrue(model.daemonLeaseConflict)
        XCTAssertEqual(takeoverCalls.current, 0)
        XCTAssertEqual(launcher.verificationCount, 0)

        let confirmation = model.daemonLeaseTakeoverConfirmation
        XCTAssertNotNil(confirmation)
        let succeeded = await model.takeOverDaemonManagement(confirming: confirmation!)

        XCTAssertTrue(succeeded)
        XCTAssertTrue(model.ownsDaemonLease)
        XCTAssertEqual(takeoverCalls.current, 1)
        XCTAssertEqual(launcher.verificationCount, 1)
        XCTAssertFalse(model.daemonLeaseTakeoverInProgress)
        XCTAssertNil(model.managementOperationError)
        XCTAssertEqual(model.lifecycle?.management.managementTokenGeneration, 4)
        XCTAssertEqual(model.actionFeedback?.message, "已接管后台服务")
    }

    @MainActor
    func testAppModelRejectsTakeoverWhenConfirmedLeaseHasChanged() async {
        let owner = LockedValue<String?>("installation-a")
        let leaseGeneration = LockedValue<Int64>(7)
        let tokenGeneration = LockedValue<Int64>(3)
        let takeoverCalls = IntCounter()
        let client = makeClient { request in
            switch (request.httpMethod, request.url?.path) {
            case ("GET", "/healthz"):
                return MockResponse(statusCode: 200, json: Self.healthJSON)
            case ("GET", "/api/v1/manage/dashboard"):
                return MockResponse(statusCode: 200, json: Self.dashboardJSON)
            case ("GET", "/api/v1/manage/lifecycle"):
                return MockResponse(
                    statusCode: 200,
                    json: Self.lifecycleSecurityPayload(
                        installationId: owner.value,
                        canControl: false,
                        managementTokenGeneration: tokenGeneration.value,
                        leaseGeneration: leaseGeneration.value
                    )
                )
            case ("POST", "/api/v1/manage/lifecycle/lease/takeover"):
                _ = takeoverCalls.next()
                return MockResponse(statusCode: 500, json: #"{"error":"must not retarget"}"#)
            default:
                return MockResponse(statusCode: 404, json: "")
            }
        }
        let model = AppModel(
            apiClient: client,
            daemonLauncher: IdentityVerifyingDaemonLauncher(),
            guiBuildLoader: { "388" }
        )
        await model.refresh()
        let confirmation = model.daemonLeaseTakeoverConfirmation
        XCTAssertNotNil(confirmation)

        owner.value = "installation-b"
        leaseGeneration.value = 8
        tokenGeneration.value = 4
        await model.refresh()

        let succeeded = await model.takeOverDaemonManagement(confirming: confirmation!)

        XCTAssertFalse(succeeded)
        XCTAssertEqual(takeoverCalls.current, 0)
        XCTAssertTrue(model.managementOperationError?.contains("重新确认") == true)
        XCTAssertEqual(model.lifecycle?.management.installationId, "installation-b")
    }

    @MainActor
    func testTakeoverResultIsNotClobberedByAnOlderRefresh() async {
        let owner = LockedValue<String?>("other-installation")
        let leaseGeneration = LockedValue<Int64>(7)
        let tokenGeneration = LockedValue<Int64>(3)
        let lifecycleCalls = IntCounter()
        let staleRequestBlocked = LockedValue(false)
        let staleResponseGate = MockResponseGate()
        let client = makeClient { request in
            switch (request.httpMethod, request.url?.path) {
            case ("GET", "/healthz"):
                return MockResponse(statusCode: 200, json: Self.healthJSON)
            case ("GET", "/api/v1/manage/dashboard"):
                return MockResponse(statusCode: 200, json: Self.dashboardJSON)
            case ("GET", "/api/v1/manage/lifecycle"):
                let call = lifecycleCalls.next()
                let payload = Self.lifecycleSecurityPayload(
                    installationId: owner.value,
                    canControl: owner.value != "other-installation",
                    managementTokenGeneration: tokenGeneration.value,
                    leaseGeneration: leaseGeneration.value
                )
                if call == 2 {
                    staleRequestBlocked.value = true
                    return MockResponse(
                        statusCode: 200,
                        json: payload,
                        deliveryGate: staleResponseGate
                    )
                }
                return MockResponse(statusCode: 200, json: payload)
            case ("POST", "/api/v1/manage/lifecycle/lease/takeover"):
                let body = Self.jsonBody(from: request)
                owner.value = body?["installationId"] as? String
                leaseGeneration.value = 8
                tokenGeneration.value = 4
                let requestId = body?["requestId"] as? String ?? ""
                return MockResponse(
                    statusCode: 200,
                    json: #"{"ok":true,"rotated":true,"requestId":"\#(requestId)","managementTokenGeneration":4}"#
                )
            default:
                return MockResponse(statusCode: 404, json: "")
            }
        }
        let model = AppModel(
            apiClient: client,
            daemonLauncher: IdentityVerifyingDaemonLauncher(),
            guiBuildLoader: { "388" }
        )
        await model.refresh()
        let confirmation = model.daemonLeaseTakeoverConfirmation
        XCTAssertNotNil(confirmation)

        let staleRefresh = Task { await model.refresh() }
        for _ in 0..<100 where !staleRequestBlocked.value {
            try? await Task.sleep(for: .milliseconds(10))
        }
        XCTAssertTrue(staleRequestBlocked.value)

        let succeeded = await model.takeOverDaemonManagement(confirming: confirmation!)
        XCTAssertFalse(staleResponseGate.didTimeOut)
        XCTAssertEqual(lifecycleCalls.current, 3)
        staleResponseGate.open()
        await staleRefresh.value

        XCTAssertTrue(succeeded)
        XCTAssertTrue(model.ownsDaemonLease)
        XCTAssertEqual(model.lifecycle?.management.leaseGeneration, 8)
        XCTAssertEqual(model.lifecycle?.management.managementTokenGeneration, 4)
    }

    @MainActor
    func testAppModelClaimsUnmanagedLeaseThenRotatesCredentialExplicitly() async {
        let owner = LockedValue<String?>(nil)
        let tokenGeneration = LockedValue<Int64>(3)
        let claimCalls = IntCounter()
        let rotateCalls = IntCounter()
        let launcher = IdentityVerifyingDaemonLauncher()
        let client = makeClient { request in
            switch (request.httpMethod, request.url?.path) {
            case ("GET", "/healthz"):
                return MockResponse(statusCode: 200, json: Self.healthJSON)
            case ("GET", "/api/v1/manage/dashboard"):
                return MockResponse(statusCode: 200, json: Self.dashboardJSON)
            case ("GET", "/api/v1/manage/lifecycle"):
                return MockResponse(
                    statusCode: 200,
                    json: Self.lifecycleSecurityPayload(
                        installationId: owner.value,
                        canControl: owner.value != nil,
                        managementTokenGeneration: tokenGeneration.value
                    )
                )
            case ("POST", "/api/v1/manage/lifecycle/lease/claim"):
                _ = claimCalls.next()
                let body = Self.jsonBody(from: request)
                XCTAssertNotNil(body?["daemonIdentity"] as? [String: Any])
                owner.value = body?["installationId"] as? String
                return MockResponse(
                    statusCode: 200,
                    json: Self.lifecycleSecurityPayload(
                        installationId: owner.value,
                        canControl: true,
                        managementTokenGeneration: tokenGeneration.value
                    )
                )
            case ("POST", "/api/v1/manage/lifecycle/credential/rotate"):
                _ = rotateCalls.next()
                let body = Self.jsonBody(from: request)
                let requestId = body?["requestId"] as? String ?? ""
                XCTAssertEqual(body?["leaseGeneration"] as? Int, 7)
                XCTAssertEqual(body?["expectedManagementTokenGeneration"] as? Int, 3)
                XCTAssertEqual(body?["reason"] as? String, "leakRecovery")
                tokenGeneration.value = 4
                return MockResponse(
                    statusCode: 200,
                    json: #"{"ok":true,"rotated":true,"requestId":"\#(requestId)","managementTokenGeneration":4}"#
                )
            default:
                return MockResponse(statusCode: 404, json: "")
            }
        }
        let model = AppModel(
            apiClient: client,
            daemonLauncher: launcher,
            guiBuildLoader: { "388" }
        )

        await model.refresh()

        XCTAssertTrue(model.ownsDaemonLease)
        XCTAssertEqual(claimCalls.current, 1)
        XCTAssertEqual(launcher.verificationCount, 1)

        let confirmation = model.managementCredentialRotationConfirmation
        XCTAssertNotNil(confirmation)
        let succeeded = await model.rotateManagementCredential(confirming: confirmation!)

        XCTAssertTrue(succeeded)
        XCTAssertEqual(rotateCalls.current, 1)
        XCTAssertEqual(launcher.verificationCount, 1)
        XCTAssertFalse(model.managementCredentialRotationInProgress)
        XCTAssertNil(model.managementOperationError)
        XCTAssertEqual(model.lifecycle?.management.managementTokenGeneration, 4)
        XCTAssertEqual(model.actionFeedback?.message, "管理凭据已重新生成")
    }

    @MainActor
    func testAppModelRejectsCredentialRotationWhenConfirmedLeaseHasChanged() async {
        let owner = LockedValue<String?>(nil)
        let leaseGeneration = LockedValue<Int64>(7)
        let tokenGeneration = LockedValue<Int64>(3)
        let rotationCalls = IntCounter()
        let client = makeClient { request in
            switch (request.httpMethod, request.url?.path) {
            case ("GET", "/healthz"):
                return MockResponse(statusCode: 200, json: Self.healthJSON)
            case ("GET", "/api/v1/manage/dashboard"):
                return MockResponse(statusCode: 200, json: Self.dashboardJSON)
            case ("GET", "/api/v1/manage/lifecycle"):
                return MockResponse(
                    statusCode: 200,
                    json: Self.lifecycleSecurityPayload(
                        installationId: owner.value,
                        canControl: owner.value != nil,
                        managementTokenGeneration: tokenGeneration.value,
                        leaseGeneration: leaseGeneration.value
                    )
                )
            case ("POST", "/api/v1/manage/lifecycle/lease/claim"):
                owner.value = Self.jsonBody(from: request)?["installationId"] as? String
                return MockResponse(
                    statusCode: 200,
                    json: Self.lifecycleSecurityPayload(
                        installationId: owner.value,
                        canControl: true,
                        managementTokenGeneration: tokenGeneration.value,
                        leaseGeneration: leaseGeneration.value
                    )
                )
            case ("POST", "/api/v1/manage/lifecycle/credential/rotate"):
                _ = rotationCalls.next()
                return MockResponse(statusCode: 500, json: #"{"error":"must not retarget"}"#)
            default:
                return MockResponse(statusCode: 404, json: "")
            }
        }
        let model = AppModel(
            apiClient: client,
            daemonLauncher: IdentityVerifyingDaemonLauncher(),
            guiBuildLoader: { "388" }
        )
        await model.refresh()
        let confirmation = model.managementCredentialRotationConfirmation
        XCTAssertNotNil(confirmation)

        leaseGeneration.value = 8
        tokenGeneration.value = 4
        await model.refresh()

        let succeeded = await model.rotateManagementCredential(confirming: confirmation!)

        XCTAssertFalse(succeeded)
        XCTAssertEqual(rotationCalls.current, 0)
        XCTAssertTrue(model.managementOperationError?.contains("重新确认") == true)
        XCTAssertEqual(model.lifecycle?.management.leaseGeneration, 8)
        XCTAssertEqual(model.lifecycle?.management.managementTokenGeneration, 4)
    }

    @MainActor
    func testCredentialRotationResultIsNotClobberedByAnOlderRefresh() async {
        let owner = LockedValue<String?>(nil)
        let tokenGeneration = LockedValue<Int64>(3)
        let lifecycleCalls = IntCounter()
        let staleRequestBlocked = LockedValue(false)
        let staleResponseGate = MockResponseGate()
        let client = makeClient { request in
            switch (request.httpMethod, request.url?.path) {
            case ("GET", "/healthz"):
                return MockResponse(statusCode: 200, json: Self.healthJSON)
            case ("GET", "/api/v1/manage/dashboard"):
                return MockResponse(statusCode: 200, json: Self.dashboardJSON)
            case ("GET", "/api/v1/manage/lifecycle"):
                let call = lifecycleCalls.next()
                let payload = Self.lifecycleSecurityPayload(
                    installationId: owner.value,
                    canControl: owner.value != nil,
                    managementTokenGeneration: tokenGeneration.value
                )
                if call == 2 {
                    staleRequestBlocked.value = true
                    return MockResponse(
                        statusCode: 200,
                        json: payload,
                        deliveryGate: staleResponseGate
                    )
                }
                return MockResponse(statusCode: 200, json: payload)
            case ("POST", "/api/v1/manage/lifecycle/lease/claim"):
                owner.value = Self.jsonBody(from: request)?["installationId"] as? String
                return MockResponse(
                    statusCode: 200,
                    json: Self.lifecycleSecurityPayload(
                        installationId: owner.value,
                        canControl: true,
                        managementTokenGeneration: tokenGeneration.value
                    )
                )
            case ("POST", "/api/v1/manage/lifecycle/credential/rotate"):
                let body = Self.jsonBody(from: request)
                tokenGeneration.value = 4
                let requestId = body?["requestId"] as? String ?? ""
                return MockResponse(
                    statusCode: 200,
                    json: #"{"ok":true,"rotated":true,"requestId":"\#(requestId)","managementTokenGeneration":4}"#
                )
            default:
                return MockResponse(statusCode: 404, json: "")
            }
        }
        let model = AppModel(
            apiClient: client,
            daemonLauncher: IdentityVerifyingDaemonLauncher(),
            guiBuildLoader: { "388" }
        )
        await model.refresh()
        let confirmation = model.managementCredentialRotationConfirmation
        XCTAssertNotNil(confirmation)

        let staleRefresh = Task { await model.refresh() }
        for _ in 0..<100 where !staleRequestBlocked.value {
            try? await Task.sleep(for: .milliseconds(10))
        }
        XCTAssertTrue(staleRequestBlocked.value)

        let succeeded = await model.rotateManagementCredential(confirming: confirmation!)
        XCTAssertFalse(staleResponseGate.didTimeOut)
        XCTAssertEqual(lifecycleCalls.current, 3)
        staleResponseGate.open()
        await staleRefresh.value

        XCTAssertTrue(succeeded)
        XCTAssertTrue(model.ownsDaemonLease)
        XCTAssertEqual(model.lifecycle?.management.leaseGeneration, 7)
        XCTAssertEqual(model.lifecycle?.management.managementTokenGeneration, 4)
    }

    @MainActor
    func testAppModelReportsTakeoverIdentityAndCredentialRotationFailures() async {
        let conflictClient = makeClient { request in
            switch (request.httpMethod, request.url?.path) {
            case ("GET", "/healthz"):
                return MockResponse(statusCode: 200, json: Self.healthJSON)
            case ("GET", "/api/v1/manage/dashboard"):
                return MockResponse(statusCode: 200, json: Self.dashboardJSON)
            case ("GET", "/api/v1/manage/lifecycle"):
                return MockResponse(
                    statusCode: 200,
                    json: Self.lifecycleSecurityPayload(
                        installationId: "other-installation",
                        canControl: false,
                        managementTokenGeneration: 3
                    )
                )
            default:
                return MockResponse(statusCode: 404, json: "")
            }
        }
        let failingLauncher = IdentityVerifyingDaemonLauncher(
            error: .loadedAgentUntrusted("/fixture/ThreadRelay")
        )
        let takeoverModel = AppModel(
            apiClient: conflictClient,
            daemonLauncher: failingLauncher,
            guiBuildLoader: { "388" }
        )
        await takeoverModel.refresh()

        let takeoverConfirmation = takeoverModel.daemonLeaseTakeoverConfirmation
        XCTAssertNotNil(takeoverConfirmation)
        let takeoverSucceeded = await takeoverModel.takeOverDaemonManagement(
            confirming: takeoverConfirmation!
        )
        XCTAssertFalse(takeoverSucceeded)
        XCTAssertFalse(takeoverModel.daemonLeaseTakeoverInProgress)
        XCTAssertTrue(takeoverModel.managementOperationError?.contains("本地服务不可用") == true)

        let owner = LockedValue<String?>(nil)
        let rotateClient = makeClient { request in
            switch (request.httpMethod, request.url?.path) {
            case ("GET", "/healthz"):
                return MockResponse(statusCode: 200, json: Self.healthJSON)
            case ("GET", "/api/v1/manage/dashboard"):
                return MockResponse(statusCode: 200, json: Self.dashboardJSON)
            case ("GET", "/api/v1/manage/lifecycle"):
                return MockResponse(
                    statusCode: 200,
                    json: Self.lifecycleSecurityPayload(
                        installationId: owner.value,
                        canControl: owner.value != nil,
                        managementTokenGeneration: 3
                    )
                )
            case ("POST", "/api/v1/manage/lifecycle/lease/claim"):
                owner.value = Self.jsonBody(from: request)?["installationId"] as? String
                return MockResponse(
                    statusCode: 200,
                    json: Self.lifecycleSecurityPayload(
                        installationId: owner.value,
                        canControl: true,
                        managementTokenGeneration: 3
                    )
                )
            case ("POST", "/api/v1/manage/lifecycle/credential/rotate"):
                return MockResponse(statusCode: 409, json: #"{"error":"凭据代次已变化"}"#)
            default:
                return MockResponse(statusCode: 404, json: "")
            }
        }
        let rotateModel = AppModel(
            apiClient: rotateClient,
            daemonLauncher: IdentityVerifyingDaemonLauncher(),
            guiBuildLoader: { "388" }
        )
        await rotateModel.refresh()

        let rotationConfirmation = rotateModel.managementCredentialRotationConfirmation
        XCTAssertNotNil(rotationConfirmation)
        let rotationSucceeded = await rotateModel.rotateManagementCredential(
            confirming: rotationConfirmation!
        )
        XCTAssertFalse(rotationSucceeded)
        XCTAssertFalse(rotateModel.managementCredentialRotationInProgress)
        XCTAssertTrue(rotateModel.managementOperationError?.contains("凭据代次已变化") == true)
    }

    func testLogDirectoryRejectsLocatorResponseFromReplacedDaemon() async throws {
        let client = makeClient(credentialCandidatesLoader: {
            [
                .init(token: "locator-token", expectedInstanceId: "stale-instance"),
                .init(token: "fallback-token", expectedInstanceId: nil),
            ]
        }) { request in
            let token = request.value(forHTTPHeaderField: "Authorization")
            if token == "Bearer locator-token" {
                return MockResponse(
                    statusCode: 200,
                    json: #"{"directory":"/stale/logs","instanceId":"replacement-instance"}"#
                )
            }
            return MockResponse(
                statusCode: 200,
                json: #"{"directory":"/current/logs","instanceId":"replacement-instance"}"#
            )
        }

        let directory = try await client.logDirectory()

        XCTAssertEqual(directory.path, "/current/logs")
    }

    func testCodexAndSessionManagementUseProtectedVersionedRoutes() async throws {
        let client = makeClient { request in
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Authorization"),
                "Bearer fixture-token"
            )
            switch request.url?.path {
            case "/api/v1/manage/codex/status":
                return MockResponse(
                    statusCode: 200,
                    json: #"{"codexHome":"/fixture/.codex","configured":true,"configOk":true,"authOk":true,"providerOk":true,"configError":null,"authError":null,"guiConfigured":true,"guiError":null,"remoteControlSupported":true,"remoteControlConfigured":true,"remoteControlError":null,"providers":[{"name":"ai-gateway","baseUrl":"http://127.0.0.1:3847/backend-api","secretSet":true,"supportsWebsockets":true}],"imageGenerationEnabled":true,"connectionMode":"standard"}"#
                )
            case "/api/v1/manage/codex/enhanced/preflight":
                return MockResponse(
                    statusCode: 200,
                    json: #"{"ok":true,"status":{"running":false}}"#
                )
            case "/api/v1/manage/sessions":
                return MockResponse(
                    statusCode: 200,
                    json: #"{"ok":true,"threads":[{"id":"thread-1","preview":"修复登录","modelProvider":"openai","updatedAt":1754000120000,"path":"/fixture/rollout.jsonl","cwd":"/fixture/project","name":null},{"id":"legacy-thread"}],"providers":["openai"],"total":2}"#
                )
            case "/api/v1/manage/sessions/provider":
                let body = Self.jsonBody(from: request)
                XCTAssertEqual(body?["threadId"] as? String, "thread-1")
                XCTAssertEqual(body?["targetProvider"] as? String, "openai")
                XCTAssertNil(body?["rolloutPath"])
                return MockResponse(statusCode: 200, json: #"{"ok":true}"#)
            default:
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }

        let status = try await client.codexStatus()
        XCTAssertTrue(status.configured)
        XCTAssertTrue(status.providers[0].secretSet)
        XCTAssertEqual(status.providers[0].name, "ai-gateway")

        let preflight = try await client.codexEnhancedPreflight()
        XCTAssertFalse(preflight.status.running)

        let sessions = try await client.codexSessions()
        XCTAssertEqual(sessions.total, 2)
        XCTAssertEqual(sessions.threads[0].displayName, "修复登录")
        XCTAssertEqual(sessions.threads[0].cwd, "/fixture/project")
        XCTAssertEqual(sessions.threads[1].displayName, "未命名会话")
        XCTAssertEqual(sessions.threads[1].modelProvider, "openai")
        XCTAssertEqual(sessions.threads[1].updatedAt, 0)
        _ = try await client.moveCodexSession(
            threadId: "thread-1",
            targetProvider: "openai"
        )
    }

    func testCodexDirectApiModeUsesProtectedVersionedRoute() async throws {
        let client = makeClient { request in
            XCTAssertEqual(request.httpMethod, "POST")
            XCTAssertEqual(request.url?.path, "/api/v1/manage/codex/direct-api-mode")
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Authorization"),
                "Bearer fixture-token"
            )
            return MockResponse(
                statusCode: 200,
                json: #"{"ok":true,"report":{"mode":"direct-api","activeProvider":"custom"}}"#
            )
        }

        let response = try await client.switchCodexToDirectApiMode()
        XCTAssertTrue(response.ok)
    }

    func testEnhancedLaunchOperationUsesStartStatusAndCancelRoutes() async throws {
        let requestId = "enhanced-request-1"
        let client = makeClient { request in
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Authorization"),
                "Bearer fixture-token"
            )
            switch (request.httpMethod, request.url?.path) {
            case ("POST", "/api/v1/manage/codex/enhanced/operation/start"):
                XCTAssertEqual(Self.jsonBody(from: request)?["requestId"] as? String, requestId)
                return MockResponse(
                    statusCode: 202,
                    json: Self.enhancedOperationJSON(
                        requestId: requestId,
                        phase: "preparing",
                        canCancel: true,
                        message: "正在准备增强启动"
                    )
                )
            case ("GET", "/api/v1/manage/codex/enhanced/operation"):
                return MockResponse(
                    statusCode: 200,
                    json: Self.enhancedOperationJSON(
                        requestId: requestId,
                        phase: "injecting",
                        canCancel: true,
                        message: "正在应用模型与界面增强配置"
                    )
                )
            case ("POST", "/api/v1/manage/codex/enhanced/operation/cancel"):
                XCTAssertEqual(Self.jsonBody(from: request)?["requestId"] as? String, requestId)
                return MockResponse(
                    statusCode: 200,
                    json: Self.enhancedOperationJSON(
                        requestId: requestId,
                        phase: "cancelled",
                        canCancel: false,
                        message: "增强启动已取消",
                        recovery: "Codex App 已保留运行"
                    )
                )
            default:
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }

        let started = try await client.startCodexEnhancedOperation(requestId: requestId)
        XCTAssertEqual(started.phase, "preparing")
        XCTAssertTrue(started.isRunning)

        let fetchedCurrent = try await client.codexEnhancedOperation()
        let current = try XCTUnwrap(fetchedCurrent)
        XCTAssertEqual(current.phase, "injecting")
        XCTAssertTrue(current.canCancel)

        let cancelled = try await client.cancelCodexEnhancedOperation(requestId: requestId)
        XCTAssertEqual(cancelled.phase, "cancelled")
        XCTAssertTrue(cancelled.isTerminal)
        XCTAssertEqual(cancelled.recovery, "Codex App 已保留运行")
    }

    func testEnhancedLaunchOperationMissingRouteUsesLegacyCapabilityFallback() async {
        let client = makeClient { _ in
            MockResponse(statusCode: 404, json: #"{"error":"not found"}"#)
        }

        do {
            _ = try await client.startCodexEnhancedOperation(requestId: "legacy-request")
            XCTFail("Expected old daemon capability fallback")
        } catch {
            XCTAssertEqual(error as? APIClientError, .featureUnavailable)
        }
    }

    func testGatewayProviderMutationKeepsAPIKeyWriteOnly() async throws {
        let responseJSON = #"{"ok":true,"gateway":{"enabled":true,"filterImageGenerationTool":false,"requestLoggingEnabled":true,"requestLogDetailsEnabled":false,"codexVisibleModels":["model-a"],"providers":[{"name":"primary","enabled":true,"providerType":"open_ai_responses","compatibility":null,"baseUrl":"https://provider.example/v1","modelsUrl":null,"models":["model-a"],"modelAliases":{},"promptCacheRetention":null,"weight":100,"timeoutSecs":60,"secretSet":true}]}}"#
        let client = makeClient { request in
            XCTAssertEqual(request.url?.path, "/api/v1/manage/gateway/provider")
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Authorization"),
                "Bearer fixture-token"
            )
            let body = Self.jsonBody(from: request)
            XCTAssertEqual(body?["apiKey"] as? String, "write-only-key")
            XCTAssertEqual(body?["providerType"] as? String, "open_ai_responses")
            return MockResponse(statusCode: 200, json: responseJSON)
        }
        let provider = ManageGatewayProvider(
            name: "primary",
            enabled: true,
            providerType: "open_ai_responses",
            compatibility: nil,
            baseUrl: "https://provider.example/v1",
            modelsUrl: nil,
            models: ["model-a"],
            modelAliases: [:],
            promptCacheRetention: nil,
            weight: 100,
            timeoutSecs: 60,
            secretSet: false
        )

        let gateway = try await client.upsertGatewayProvider(
            originalName: nil,
            provider: provider,
            apiKey: "write-only-key"
        )

        XCTAssertTrue(gateway.providers[0].secretSet)
        XCTAssertEqual(gateway.providers[0].models, ["model-a"])
    }

    func testGatewayMutationRejectsAStaleDiscoveredDaemonBeforePOST() async {
        let client = makeClient(credentialCandidatesLoader: {
            [.init(token: "fixture-token", expectedInstanceId: "stale-instance")]
        }) { request in
            XCTAssertEqual(request.url?.path, "/api/v1/manage/status")
            return MockResponse(
                statusCode: 200,
                json: #"{"service":"threadrelay","apiMajor":1,"ready":true,"instanceId":"replacement-instance","pid":123,"startedAtMs":456}"#
            )
        }
        let provider = ManageGatewayProvider(
            name: "primary",
            enabled: true,
            providerType: "open_ai_responses",
            compatibility: nil,
            baseUrl: "https://provider.example/v1",
            modelsUrl: nil,
            models: ["model-a"],
            modelAliases: [:],
            promptCacheRetention: nil,
            weight: 100,
            timeoutSecs: 60,
            secretSet: false
        )

        do {
            _ = try await client.upsertGatewayProvider(
                originalName: nil,
                provider: provider,
                apiKey: "write-only-key"
            )
            XCTFail("Expected stale daemon identity to be rejected")
        } catch let error as APIClientError {
            XCTAssertEqual(error, .unauthorized)
        } catch {
            XCTFail("Unexpected error: \(error)")
        }
    }

    func testSettingsAndRequestLogsDecodeRealManagementPayloads() async throws {
        let client = makeClient { request in
            switch request.url?.path {
            case "/api/v1/manage/settings":
                if request.httpMethod == "POST" {
                    let body = Self.jsonBody(from: request)
                    XCTAssertEqual(body?["outboundProxyMode"] as? String, "direct")
                    return MockResponse(
                        statusCode: 200,
                        json: #"{"ok":true,"settings":{"language":"zh-CN","theme":"dark","localConnectionMode":"standard","bind":"127.0.0.1:3847","outboundProxy":{"mode":"direct","url":"<none>","credentialSet":false}}}"#
                    )
                }
                return MockResponse(
                    statusCode: 200,
                    json: #"{"language":"zh-CN","theme":"system","localConnectionMode":"standard","bind":"127.0.0.1:3847","outboundProxy":{"mode":"system","url":"<none>","credentialSet":false}}"#
                )
            case "/api/v1/manage/request-logs":
                return MockResponse(
                    statusCode: 200,
                    json: #"{"logs":[{"id":7,"requestId":"req-7","modelId":"model-a","stream":true,"channel":"primary","providerType":"openai_responses","status":"success","inputTokens":10,"outputTokens":20,"totalTokens":30,"readCacheTokens":null,"readCacheHitRate":null,"writeCacheTokens":null,"costUsd":0.01,"latencyMs":1200,"ttftMs":300,"createdAtMs":1754000120000,"createdAt":"2026-08-13T00:00:00Z","errorMessage":null,"upstreamRequestBodyBytes":128}]}"#
                )
            case "/api/v1/manage/request-logs/7":
                return MockResponse(
                    statusCode: 200,
                    json: #"{"log":{"id":7,"requestId":"req-7","modelId":"model-a","stream":true,"channel":"primary","providerType":"openai_responses","status":"success","inputTokens":10,"outputTokens":20,"totalTokens":30,"readCacheTokens":null,"readCacheHitRate":null,"writeCacheTokens":null,"costUsd":0.01,"latencyMs":1200,"ttftMs":300,"createdAtMs":1754000120000,"createdAt":"2026-08-13T00:00:00Z","errorMessage":null,"upstreamRequestBodyBytes":128,"requestHeadersJson":"{\"authorization\":\"<redacted>\"}","requestJson":"{\"model\":\"model-a\"}","upstreamRequestHeadersJson":null,"upstreamRequestJson":null,"upstreamResponseSse":null,"responseJson":"{\"ok\":true}"}}"#
                )
            default:
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }

        let settings = try await client.settings()
        XCTAssertEqual(settings.outboundProxy.mode, "system")
        let saved = try await client.updateSettings(
            language: "zh-CN",
            theme: "dark",
            localConnectionMode: "standard",
            outboundProxyMode: "direct",
            outboundProxyURL: nil
        )
        XCTAssertEqual(saved.theme, "dark")

        let logs = try await client.requestLogs().logs
        XCTAssertEqual(logs[0].totalTokens, 30)
        let detail = try await client.requestLogDetail(id: 7)
        XCTAssertEqual(detail.requestHeadersJson, #"{"authorization":"<redacted>"}"#)
    }

    @MainActor
    func testAppModelLoadsVersionedDashboard() async {
        let client = makeClient { request in
            switch request.url?.path {
            case "/healthz":
                MockResponse(statusCode: 200, json: Self.healthJSON)
            case "/api/v1/manage/dashboard":
                MockResponse(statusCode: 200, json: Self.dashboardJSON)
            case "/api/v1/manage/lifecycle":
                MockResponse(statusCode: 200, json: Self.lifecycleJSON)
            default:
                MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }
        let model = AppModel(apiClient: client)

        await model.refresh()

        XCTAssertEqual(model.serviceStatus, .available)
        XCTAssertEqual(model.dashboardState, .loaded)
        XCTAssertEqual(model.dashboard?.service.instanceId, "fixture-instance")
        XCTAssertNotNil(model.lastCheckedAt)
    }

    @MainActor
    func testAppModelShowsAccountManagementUpdateNoticeForOlderDaemon() async {
        let client = makeClient { request in
            switch request.url?.path {
            case "/healthz":
                MockResponse(statusCode: 200, json: Self.healthJSON)
            case "/api/v1/manage/dashboard":
                MockResponse(statusCode: 200, json: Self.dashboardJSON)
            case "/api/v1/manage/lifecycle":
                MockResponse(statusCode: 200, json: Self.lifecycleJSON)
            case "/api/v1/manage/im/accounts":
                MockResponse(statusCode: 404, json: #"{"error":"not found"}"#)
            default:
                MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }
        let model = AppModel(apiClient: client)

        await model.refresh()

        XCTAssertEqual(model.dashboardState, .loaded)
        XCTAssertEqual(model.imAccounts, [])
        XCTAssertEqual(model.imAccountsAvailability, .needsUpdate)
    }

    @MainActor
    func testAppModelLoadsCodexFromBuild410WithoutLifecycleSecurityFields() async {
        let lifecycleCalls = IntCounter()
        let client = makeClient { request in
            switch (request.httpMethod, request.url?.path) {
            case ("GET", "/healthz"):
                return MockResponse(statusCode: 200, json: Self.healthJSON)
            case ("GET", "/api/v1/manage/dashboard"):
                return MockResponse(statusCode: 200, json: Self.dashboardJSON)
            case ("GET", "/api/v1/manage/lifecycle"):
                let lifecycleCall = lifecycleCalls.next()
                let owner = lifecycleCall == 1
                    ? "other-installation"
                    : UserDefaults.standard.string(
                        forKey: "threadrelay.management.installation-id"
                    ) ?? "missing-installation"
                return MockResponse(
                    statusCode: 200,
                    json: Self.build410LifecyclePayload(
                        installationId: owner,
                        canControl: lifecycleCall != 1
                    )
                )
            case ("GET", "/api/v1/manage/im/accounts"):
                return MockResponse(statusCode: 200, json: Self.imAccountsJSON)
            case ("GET", "/api/v1/manage/codex/status"):
                return MockResponse(
                    statusCode: 200,
                    json: Self.codexStatusJSON
                )
            case ("GET", "/api/v1/manage/codex/enhanced/preflight"):
                return MockResponse(statusCode: 404, json: #"{"error":"not found"}"#)
            case ("POST", "/api/v1/manage/lifecycle/lease/claim"):
                XCTFail("An active build 410 lease must not be claimed automatically")
                return MockResponse(statusCode: 409, json: #"{"error":"lease conflict"}"#)
            default:
                return MockResponse(statusCode: 404, json: #"{"error":"not found"}"#)
            }
        }
        let model = AppModel(
            apiClient: client,
            daemonLauncher: IdentityVerifyingDaemonLauncher(),
            guiBuildLoader: { "421" }
        )

        await model.refresh()
        let loadedCodex = await model.loadSection(.codex)

        XCTAssertEqual(model.serviceStatus, .available)
        XCTAssertEqual(model.dashboardState, .loaded)
        XCTAssertEqual(model.lifecycle?.runtime.buildNumber, 410)
        XCTAssertNil(model.lifecycle?.executableSha256)
        XCTAssertNil(model.lifecycle?.management.managementTokenGeneration)
        XCTAssertTrue(model.daemonLeaseConflict)
        XCTAssertFalse(model.daemonTransitionInProgress)
        XCTAssertFalse(model.canTakeOverDaemonLease)
        XCTAssertNil(model.daemonLeaseTakeoverConfirmation)
        XCTAssertTrue(loadedCodex)
        XCTAssertTrue(model.codexStatus?.configured == true)
        XCTAssertNil(model.codexPreflight)

        await model.refresh()

        XCTAssertTrue(model.ownsDaemonLease)
        XCTAssertFalse(model.daemonTransitionInProgress)
        XCTAssertFalse(model.canRotateManagementCredential)
        XCTAssertNil(model.managementCredentialRotationConfirmation)
    }

    @MainActor
    func testAppModelResumesRunningEnhancedLaunchAndPublishesCompletion() async {
        let operationReads = IntCounter()
        let client = makeClient { request in
            switch (request.httpMethod, request.url?.path) {
            case ("GET", "/api/v1/manage/codex/status"):
                return MockResponse(statusCode: 200, json: Self.codexStatusJSON)
            case ("GET", "/api/v1/manage/codex/enhanced/preflight"):
                return MockResponse(
                    statusCode: 200,
                    json: #"{"ok":true,"status":{"running":false}}"#
                )
            case ("GET", "/api/v1/manage/codex/enhanced/operation"):
                let ready = operationReads.next() > 1
                return MockResponse(
                    statusCode: 200,
                    json: Self.enhancedOperationJSON(
                        requestId: "resumed-request",
                        phase: ready ? "ready" : "injecting",
                        canCancel: !ready,
                        message: ready ? "增强模式已就绪" : "正在应用模型与界面增强配置"
                    )
                )
            default:
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }
        let model = AppModel(
            apiClient: client,
            codexEnhancedOperationPollDelay: .milliseconds(1)
        )

        let loaded = await model.loadSection(.codex)
        XCTAssertTrue(loaded)
        XCTAssertEqual(model.codexEnhancedOperation?.phase, "injecting")

        for _ in 0..<20 where model.codexEnhancedOperation?.phase != "ready" {
            try? await Task.sleep(for: .milliseconds(5))
        }

        XCTAssertEqual(model.codexEnhancedOperation?.phase, "ready")
        XCTAssertFalse(model.codexEnhancedLaunchInProgress)
        XCTAssertEqual(model.actionFeedback?.message, "增强启动已完成")
        XCTAssertNil(model.codexEnhancedLaunchError)
        XCTAssertGreaterThanOrEqual(operationReads.current, 2)
    }

    @MainActor
    func testAppModelKeepsMonitoringEnhancedLaunchAfterTransientReadFailures() async {
        let operationReads = IntCounter()
        let client = makeClient { request in
            switch (request.httpMethod, request.url?.path) {
            case ("GET", "/api/v1/manage/codex/status"):
                return MockResponse(statusCode: 200, json: Self.codexStatusJSON)
            case ("GET", "/api/v1/manage/codex/enhanced/preflight"):
                return MockResponse(
                    statusCode: 200,
                    json: #"{"ok":true,"status":{"running":false}}"#
                )
            case ("GET", "/api/v1/manage/codex/enhanced/operation"):
                switch operationReads.next() {
                case 1:
                    return MockResponse(
                        statusCode: 200,
                        json: Self.enhancedOperationJSON(
                            requestId: "recovering-request",
                            phase: "injecting",
                            canCancel: true,
                            message: "正在应用模型与界面增强配置"
                        )
                    )
                case 2...4:
                    return MockResponse(
                        statusCode: 503,
                        json: #"{"error":"Service temporarily unavailable"}"#
                    )
                default:
                    return MockResponse(
                        statusCode: 200,
                        json: Self.enhancedOperationJSON(
                            requestId: "recovering-request",
                            phase: "ready",
                            canCancel: false,
                            message: "增强模式已就绪"
                        )
                    )
                }
            default:
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }
        let model = AppModel(
            apiClient: client,
            codexEnhancedOperationPollDelay: .milliseconds(1),
            codexEnhancedOperationRecoveryPollDelay: .milliseconds(1)
        )

        let loaded = await model.loadSection(.codex)
        XCTAssertTrue(loaded)
        for _ in 0..<40 where model.codexEnhancedOperation?.phase != "ready" {
            try? await Task.sleep(for: .milliseconds(5))
        }

        XCTAssertEqual(model.codexEnhancedOperation?.phase, "ready")
        XCTAssertFalse(model.codexEnhancedLaunchInProgress)
        XCTAssertNil(model.codexEnhancedLaunchError)
        XCTAssertGreaterThanOrEqual(operationReads.current, 5)
    }

    @MainActor
    func testAppModelFallsBackToLegacyEnhancedLaunchWithExplicitCancelLimit() async {
        let legacyLaunchStarted = LockedValue(false)
        let client = makeClient { request in
            switch (request.httpMethod, request.url?.path) {
            case ("GET", "/api/v1/manage/codex/enhanced/preflight"):
                return MockResponse(
                    statusCode: 200,
                    json: #"{"ok":true,"status":{"running":false}}"#
                )
            case ("POST", "/api/v1/manage/codex/enhanced/operation/start"):
                return MockResponse(statusCode: 404, json: #"{"error":"not found"}"#)
            case ("POST", "/api/v1/manage/codex/enhanced/launch"):
                legacyLaunchStarted.value = true
                Thread.sleep(forTimeInterval: 0.05)
                return MockResponse(statusCode: 200, json: #"{"ok":true}"#)
            default:
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }
        let model = AppModel(apiClient: client)

        let started = await model.beginCodexEnhancedLaunch()
        XCTAssertTrue(started)
        for _ in 0..<20 where !legacyLaunchStarted.value {
            try? await Task.sleep(for: .milliseconds(2))
        }
        XCTAssertTrue(model.codexEnhancedUsesLegacyFallback)
        XCTAssertTrue(model.canCancelCodexEnhancedLaunch)

        await model.cancelCodexEnhancedLaunch()

        XCTAssertEqual(model.codexEnhancedOperation?.phase, "cancelled")
        XCTAssertTrue(
            model.codexEnhancedOperation?.recovery?.contains("不支持服务端取消") == true
        )
    }

    @MainActor
    func testAppModelReportsFailedAccountToggleWithoutDowngradingAvailability() async throws {
        let client = makeClient { request in
            switch request.url?.path {
            case "/healthz":
                MockResponse(statusCode: 200, json: Self.healthJSON)
            case "/api/v1/manage/dashboard":
                MockResponse(statusCode: 200, json: Self.dashboardJSON)
            case "/api/v1/manage/lifecycle":
                MockResponse(statusCode: 200, json: Self.lifecycleJSON)
            case "/api/v1/manage/im/accounts":
                MockResponse(statusCode: 200, json: Self.imAccountsJSON)
            case "/api/v1/manage/im/account/enabled":
                MockResponse(statusCode: 404, json: #"{"ok":false,"error":"IM account not found"}"#)
            default:
                MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }
        let model = AppModel(apiClient: client)
        await model.refresh()
        XCTAssertEqual(model.imAccountsAvailability, .available)
        let account = try XCTUnwrap(model.imAccounts.first)

        let acknowledged = await model.setIMAccountEnabled(account, enabled: false)

        XCTAssertFalse(acknowledged)
        XCTAssertEqual(model.accountOperationError, "找不到该消息账号。")
        XCTAssertEqual(model.imAccountsAvailability, .available)
    }

    @MainActor
    func testAppModelAcknowledgesSuccessfulAccountToggle() async throws {
        let client = makeClient { request in
            switch request.url?.path {
            case "/healthz":
                MockResponse(statusCode: 200, json: Self.healthJSON)
            case "/api/v1/manage/dashboard":
                MockResponse(statusCode: 200, json: Self.dashboardJSON)
            case "/api/v1/manage/lifecycle":
                MockResponse(statusCode: 200, json: Self.lifecycleJSON)
            case "/api/v1/manage/im/accounts":
                MockResponse(statusCode: 200, json: Self.imAccountsJSON)
            case "/api/v1/manage/im/account/enabled":
                MockResponse(
                    statusCode: 200,
                    json: #"{"ok":true,"platform":"telegram","accountId":"telegram-main","enabled":false}"#
                )
            default:
                MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }
        let model = AppModel(apiClient: client)
        await model.refresh()
        let account = try XCTUnwrap(model.imAccounts.first)

        let acknowledged = await model.setIMAccountEnabled(account, enabled: false)

        XCTAssertTrue(acknowledged)
        XCTAssertNil(model.accountOperationError)
    }

    @MainActor
    func testAppModelClassifiesLegacyDaemon() async {
        let client = makeClient { request in
            switch request.url?.path {
            case "/healthz":
                MockResponse(statusCode: 404, json: #"{"error":"not found"}"#)
            case "/api/status":
                MockResponse(statusCode: 200, json: #"{"service":"codexhub"}"#)
            default:
                MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }
        let model = AppModel(apiClient: client)

        await model.refresh()

        XCTAssertEqual(model.serviceStatus, .bridgeAvailable)
        XCTAssertEqual(model.dashboardState, .legacy)
        XCTAssertNil(model.dashboard)
    }

    @MainActor
    func testAppModelMapsUnauthorizedDashboardAndClearsExistingData() async {
        let dashboardResponses = MockResponseSequence([
            MockResponse(statusCode: 200, json: Self.dashboardJSON),
            MockResponse(statusCode: 401, json: #"{"error":"unauthorized"}"#),
        ])
        let client = makeClient { request in
            if request.url?.path == "/api/v1/manage/dashboard" {
                return dashboardResponses.next()
            }
            return MockResponse(statusCode: 200, json: Self.healthJSON)
        }
        let model = AppModel(apiClient: client)

        await model.refresh()
        XCTAssertNotNil(model.dashboard)
        await model.refresh()

        XCTAssertEqual(model.serviceStatus, .available)
        XCTAssertEqual(model.dashboardState, .unauthorized)
        XCTAssertNil(model.dashboard)
    }

    @MainActor
    func testAppModelShowsDashboardUnavailableWithoutMarkingServiceOffline() async {
        let client = makeClient { request in
            switch request.url?.path {
            case "/healthz":
                MockResponse(statusCode: 200, json: Self.healthJSON)
            case "/api/v1/manage/dashboard":
                MockResponse(statusCode: 503, json: #"{"error":"temporarily unavailable"}"#)
            default:
                MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }
        let model = AppModel(apiClient: client)

        await model.refresh()

        XCTAssertEqual(model.serviceStatus, .available)
        XCTAssertEqual(model.dashboardState, .unavailable)
        XCTAssertNil(model.dashboard)
    }

    @MainActor
    func testAppModelClearsDashboardWhenDaemonBecomesLegacy() async {
        let healthResponses = MockResponseSequence([
            MockResponse(statusCode: 200, json: Self.healthJSON),
            MockResponse(statusCode: 404, json: #"{"error":"not found"}"#),
        ])
        let client = makeClient { request in
            switch request.url?.path {
            case "/healthz":
                healthResponses.next()
            case "/api/status":
                MockResponse(statusCode: 200, json: #"{"service":"codexhub"}"#)
            case "/api/v1/manage/dashboard":
                MockResponse(statusCode: 200, json: Self.dashboardJSON)
            default:
                MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }
        let model = AppModel(apiClient: client)

        await model.refresh()
        XCTAssertNotNil(model.dashboard)
        await model.refresh()

        XCTAssertEqual(model.serviceStatus, .bridgeAvailable)
        XCTAssertEqual(model.dashboardState, .legacy)
        XCTAssertNil(model.dashboard)
    }

    @MainActor
    func testAppModelShowsOfflineOnInitialProbeFailure() async {
        let client = makeClient { _ in
            MockResponse(statusCode: 503, json: #"{"error":"temporarily unavailable"}"#)
        }
        let model = AppModel(apiClient: client)

        await model.refresh()

        guard case .unavailable = model.serviceStatus else {
            XCTFail("Expected unavailable service state")
            return
        }
        XCTAssertEqual(model.dashboardState, .offline)
        XCTAssertNil(model.dashboard)
    }

    @MainActor
    func testAppModelKeepsLastDashboardWhenRefreshFails() async {
        let healthResponses = MockResponseSequence([
            MockResponse(statusCode: 200, json: Self.healthJSON),
            MockResponse(statusCode: 503, json: #"{"error":"temporarily unavailable"}"#),
        ])
        let client = makeClient { request in
            if request.url?.path == "/healthz" {
                return healthResponses.next()
            }
            return MockResponse(statusCode: 200, json: Self.dashboardJSON)
        }
        let model = AppModel(apiClient: client)

        await model.refresh()
        let firstDashboard = model.dashboard
        await model.refresh()

        XCTAssertEqual(model.dashboardState, .stale)
        XCTAssertEqual(model.dashboard, firstDashboard)
    }

    @MainActor
    func testAppModelReloadsCredentialAfterRotation() async {
        let credentials = StringSequence(["old-token", "new-token"])
        let client = makeClient(credentialLoader: { credentials.next() }) { request in
            switch request.url?.path {
            case "/healthz":
                MockResponse(statusCode: 200, json: Self.healthJSON)
            case "/api/v1/manage/dashboard":
                if request.value(forHTTPHeaderField: "Authorization") == "Bearer new-token" {
                    MockResponse(statusCode: 200, json: Self.dashboardJSON)
                } else {
                    MockResponse(statusCode: 401, json: #"{"error":"unauthorized"}"#)
                }
            default:
                MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }
        let model = AppModel(apiClient: client)

        await model.refresh()
        XCTAssertEqual(model.dashboardState, .unauthorized)
        await model.refresh()

        XCTAssertEqual(model.dashboardState, .loaded)
        XCTAssertNotNil(model.dashboard)
    }

    @MainActor
    func testForcedSectionReloadKeepsNewestResponseAndLoadingState() async {
        let calls = IntCounter()
        let client = makeClient { request in
            guard request.url?.path == "/api/v1/manage/sessions" else {
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
            let call = calls.next()
            if call == 1 {
                Thread.sleep(forTimeInterval: 0.15)
                return MockResponse(
                    statusCode: 200,
                    json: #"{"ok":true,"threads":[{"id":"older","preview":"older","modelProvider":"openai","updatedAt":1}],"providers":["openai"],"total":1}"#
                )
            }
            Thread.sleep(forTimeInterval: 0.01)
            return MockResponse(
                statusCode: 200,
                json: #"{"ok":true,"threads":[{"id":"newer","preview":"newer","modelProvider":"ai-gateway","updatedAt":2}],"providers":["ai-gateway"],"total":1}"#
            )
        }
        let model = AppModel(apiClient: client)

        let olderLoad = Task { await model.loadSection(.sessions, force: true) }
        try? await Task.sleep(for: .milliseconds(25))
        let newerLoad = Task { await model.loadSection(.sessions, force: true) }
        let newerSucceeded = await newerLoad.value
        let olderSucceeded = await olderLoad.value

        XCTAssertTrue(newerSucceeded)
        XCTAssertFalse(olderSucceeded)
        XCTAssertEqual(model.codexSessions.map(\.id), ["newer"])
        XCTAssertEqual(model.codexSessionProviders, ["ai-gateway", "openai"])
        XCTAssertFalse(model.isLoading(.sessions))
    }

    @MainActor
    func testManagementActionReportsPostMutationRefreshFailure() async {
        let listCalls = IntCounter()
        let client = makeClient { request in
            switch (request.httpMethod, request.url?.path) {
            case ("GET", "/api/v1/manage/request-logs"):
                if listCalls.next() == 1 {
                    return MockResponse(
                        statusCode: 200,
                        json: #"{"logs":[{"id":7,"requestId":"req-7","modelId":"model-a","stream":true,"channel":"primary","providerType":"openai_responses","status":"success","inputTokens":10,"outputTokens":20,"totalTokens":30,"readCacheTokens":null,"readCacheHitRate":null,"writeCacheTokens":null,"costUsd":0.01,"latencyMs":1200,"ttftMs":300,"createdAtMs":1754000120000,"createdAt":"2026-08-13T00:00:00Z","errorMessage":null,"upstreamRequestBodyBytes":128}]}"#
                    )
                }
                return MockResponse(
                    statusCode: 500,
                    json: #"{"error":"refresh unavailable"}"#
                )
            case ("POST", "/api/v1/manage/request-logs/clear"):
                return MockResponse(statusCode: 200, json: #"{"ok":true,"deleted":1}"#)
            default:
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }
        let model = AppModel(apiClient: client)
        let initialLoadSucceeded = await model.loadSection(.requestLogs)
        XCTAssertTrue(initialLoadSucceeded)
        XCTAssertEqual(model.requestLogs.count, 1)

        let succeeded = await model.clearRequestLogs()

        XCTAssertFalse(succeeded)
        XCTAssertEqual(model.requestLogs.count, 1)
        XCTAssertNotNil(model.sectionErrors[.requestLogs])
        XCTAssertNotNil(model.managementOperationError)
    }

    func testProviderTemplatesUseProtectedRouteAndDecodeOmittedOptionalKeys() async throws {
        let paths = RequestRecorder()
        let headers = StringRecorder()
        let client = makeClient { request in
            paths.record(request.url?.path)
            headers.record(request.value(forHTTPHeaderField: "Authorization") ?? "")
            return MockResponse(
                statusCode: 200,
                json: #"{"templates":[{"id":"openai","displayName":"OpenAI","providerType":"open_ai_responses","baseUrl":"https://api.openai.com/v1","models":[]},{"id":"glm","displayName":"GLM","providerType":"anthropic_messages","compatibility":"glm_anthropic","baseUrl":"https://open.bigmodel.cn/api/anthropic","modelsUrl":"https://open.bigmodel.cn/api/paas/v4/models","models":["glm-5"]}]}"#
            )
        }

        let templates = try await client.gatewayProviderTemplates()

        XCTAssertEqual(paths.paths, ["/api/v1/manage/gateway/provider-templates"])
        XCTAssertEqual(headers.values, ["Bearer fixture-token"])
        XCTAssertEqual(templates.count, 2)
        // Optional keys are omitted from the payload when absent.
        XCTAssertEqual(templates[0].id, "openai")
        XCTAssertNil(templates[0].compatibility)
        XCTAssertNil(templates[0].modelsUrl)
        XCTAssertEqual(templates[0].models, [])
        XCTAssertEqual(templates[1].id, "glm")
        XCTAssertEqual(templates[1].providerType, "anthropic_messages")
        XCTAssertEqual(templates[1].compatibility, "glm_anthropic")
        XCTAssertEqual(templates[1].modelsUrl, "https://open.bigmodel.cn/api/paas/v4/models")
        XCTAssertEqual(templates[1].models, ["glm-5"])
    }

    func testCodexModelCatalogUsesProtectedRouteAndDecodesEntries() async throws {
        let paths = RequestRecorder()
        let headers = StringRecorder()
        let client = makeClient { request in
            paths.record(request.url?.path)
            headers.record(request.value(forHTTPHeaderField: "Authorization") ?? "")
            return MockResponse(
                statusCode: 200,
                json: #"{"models":[{"id":"gpt-5.5","displayName":"GPT-5.5"},{"id":"gpt-5.5-codex","displayName":"GPT-5.5 Codex"}]}"#
            )
        }

        let models = try await client.codexModelCatalog()

        XCTAssertEqual(paths.paths, ["/api/v1/manage/codex/models/catalog"])
        XCTAssertEqual(headers.values, ["Bearer fixture-token"])
        XCTAssertEqual(models.map(\.id), ["gpt-5.5", "gpt-5.5-codex"])
        XCTAssertEqual(models.map(\.displayName), ["GPT-5.5", "GPT-5.5 Codex"])
    }

    func testClearRequestLogsUsesExtendedTimeoutAndReturnsDeletedCount() async throws {
        let timeouts = StringRecorder()
        let client = makeClient { request in
            guard request.url?.path == "/api/v1/manage/request-logs/clear" else {
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
            timeouts.record("\(request.timeoutInterval)")
            return MockResponse(statusCode: 200, json: #"{"ok":true,"deleted":137}"#)
        }

        let deleted = try await client.clearRequestLogs()

        XCTAssertEqual(deleted, 137)
        // Clearing runs DELETE + VACUUM in the daemon; the default 10-second
        // mutation timeout would misreport long clears as failures.
        XCTAssertEqual(timeouts.values, ["300.0"])
    }

    func testClearOldRequestLogsPostsDaysWithExtendedTimeout() async throws {
        let timeouts = StringRecorder()
        let client = makeClient { request in
            guard request.url?.path == "/api/v1/manage/request-logs/clear-old" else {
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
            XCTAssertEqual(request.httpMethod, "POST")
            timeouts.record("\(request.timeoutInterval)")
            let body = Self.jsonBody(from: request)
            XCTAssertEqual(body?["days"] as? Int, 3)
            return MockResponse(statusCode: 200, json: #"{"ok":true,"deleted":42}"#)
        }

        let deleted = try await client.clearOldRequestLogs()

        XCTAssertEqual(deleted, 42)
        XCTAssertEqual(timeouts.values, ["300.0"])
    }

    func testRequestLogsEncodeFiltersAndDecodePaginationMetadata() async throws {
        let urls = URLRecorder()
        let client = makeClient { request in
            urls.record(request.url)
            return MockResponse(
                statusCode: 200,
                json: Self.requestLogsPageJSON(
                    ids: [7],
                    nextCursor: "next-page",
                    hasMore: true
                )
            )
        }

        let response = try await client.requestLogs(
            filters: RequestLogFilters(
                query: "req 100% & ready",
                status: "error",
                channel: "primary/backup",
                modelId: "gpt/5?codex",
                sort: .oldest
            ),
            cursor: "1754:7+/=",
            limit: 25
        )

        let url = try XCTUnwrap(urls.urls.first)
        let components = try XCTUnwrap(
            URLComponents(url: url, resolvingAgainstBaseURL: false)
        )
        let query = Dictionary(
            uniqueKeysWithValues: (components.queryItems ?? []).map { ($0.name, $0.value) }
        )
        XCTAssertEqual(components.path, "/api/v1/manage/request-logs")
        XCTAssertEqual(query["limit"]!, "25")
        XCTAssertEqual(query["cursor"]!, "1754:7+/=")
        XCTAssertEqual(query["query"]!, "req 100% & ready")
        XCTAssertEqual(query["status"]!, "error")
        XCTAssertEqual(query["channel"]!, "primary/backup")
        XCTAssertEqual(query["modelId"]!, "gpt/5?codex")
        XCTAssertEqual(query["sort"]!, "oldest")
        XCTAssertEqual(response.logs.map(\.id), [7])
        XCTAssertEqual(response.nextCursor, "next-page")
        XCTAssertEqual(response.hasMore, true)
    }

    @MainActor
    func testLegacyRequestLogsResponseActsAsSinglePage() async {
        let urls = URLRecorder()
        let client = makeClient { request in
            urls.record(request.url)
            return MockResponse(
                statusCode: 200,
                json: Self.requestLogsPageJSON(ids: [9, 8, 7])
            )
        }
        let model = AppModel(apiClient: client)

        let loaded = await model.setRequestLogFilters(
            RequestLogFilters(
                query: "REQ-",
                status: "SUCCESS",
                channel: "PRIMARY",
                modelId: "MODEL-A",
                sort: .oldest
            )
        )
        let loadedMore = await model.loadMoreRequestLogs()

        XCTAssertTrue(loaded)
        XCTAssertEqual(model.requestLogs.map(\.id), [7, 8, 9])
        XCTAssertFalse(model.requestLogHasMore)
        XCTAssertFalse(model.requestLogLoadingMore)
        XCTAssertFalse(loadedMore)
        let components = urls.urls.first.flatMap {
            URLComponents(url: $0, resolvingAgainstBaseURL: false)
        }
        XCTAssertEqual(
            components?.queryItems?.first(where: { $0.name == "limit" })?.value,
            "200"
        )
    }

    @MainActor
    func testRequestLogsCursorWithoutHasMoreFlagStillEnablesNextPage() async {
        let client = makeClient { _ in
            MockResponse(
                statusCode: 200,
                json: Self.requestLogsPageJSON(ids: [7], nextCursor: "cursor-1")
            )
        }
        let model = AppModel(apiClient: client)

        let loaded = await model.loadSection(.requestLogs)

        XCTAssertTrue(loaded)
        XCTAssertTrue(model.requestLogHasMore)
    }

    @MainActor
    func testRequestLogPaginationDeduplicatesAndRefreshKeepsLoadedTail() async {
        let firstPageCalls = IntCounter()
        let client = makeClient { request in
            let components = request.url.flatMap {
                URLComponents(url: $0, resolvingAgainstBaseURL: false)
            }
            let cursor = components?.queryItems?.first(where: { $0.name == "cursor" })?.value
            if cursor == "cursor-1" {
                return MockResponse(
                    statusCode: 200,
                    json: Self.requestLogsPageJSON(ids: [6, 5], hasMore: false)
                )
            }
            let firstPageCall = firstPageCalls.next()
            if firstPageCall == 1 {
                return MockResponse(
                    statusCode: 200,
                    json: Self.requestLogsPageJSON(
                        ids: [7, 6],
                        nextCursor: "cursor-1",
                        hasMore: true
                    )
                )
            }
            if firstPageCall > 2 {
                return MockResponse(
                    statusCode: 200,
                    json: Self.requestLogsPageJSON(ids: [9], hasMore: false)
                )
            }
            return MockResponse(
                statusCode: 200,
                json: Self.requestLogsPageJSON(
                    ids: [8, 7],
                    nextCursor: "cursor-refreshed",
                    hasMore: true
                )
            )
        }
        let model = AppModel(apiClient: client)

        let firstPageLoaded = await model.loadSection(.requestLogs)
        XCTAssertTrue(firstPageLoaded)
        XCTAssertTrue(model.requestLogHasMore)
        let nextPageLoaded = await model.loadMoreRequestLogs()
        XCTAssertTrue(nextPageLoaded)
        XCTAssertEqual(model.requestLogs.map(\.id), [7, 6, 5])
        XCTAssertFalse(model.requestLogHasMore)

        let refreshed = await model.refreshRequestLogs()
        XCTAssertTrue(refreshed)
        XCTAssertEqual(model.requestLogs.map(\.id), [8, 7, 6, 5])
        XCTAssertFalse(model.requestLogHasMore)

        let exhaustedRefresh = await model.refreshRequestLogs()
        XCTAssertTrue(exhaustedRefresh)
        XCTAssertEqual(model.requestLogs.map(\.id), [9])
        XCTAssertFalse(model.requestLogHasMore)
    }

    @MainActor
    func testStaleRequestLogLoadMoreCannotMergeAfterFiltersChange() async {
        let client = makeClient { request in
            let components = request.url.flatMap {
                URLComponents(url: $0, resolvingAgainstBaseURL: false)
            }
            let items = components?.queryItems ?? []
            let cursor = items.first(where: { $0.name == "cursor" })?.value
            let query = items.first(where: { $0.name == "query" })?.value
            if cursor == "cursor-1" {
                Thread.sleep(forTimeInterval: 0.15)
                return MockResponse(
                    statusCode: 200,
                    json: Self.requestLogsPageJSON(ids: [1], hasMore: false)
                )
            }
            if query == "new-filter" {
                return MockResponse(
                    statusCode: 200,
                    json: Self.requestLogsPageJSON(ids: [10], hasMore: false)
                )
            }
            return MockResponse(
                statusCode: 200,
                json: Self.requestLogsPageJSON(
                    ids: [3, 2],
                    nextCursor: "cursor-1",
                    hasMore: true
                )
            )
        }
        let model = AppModel(apiClient: client)
        let firstPageLoaded = await model.loadSection(.requestLogs)
        XCTAssertTrue(firstPageLoaded)

        let staleLoadMore = Task { await model.loadMoreRequestLogs() }
        try? await Task.sleep(for: .milliseconds(25))
        let filterLoaded = await model.setRequestLogFilters(
            RequestLogFilters(query: "new-filter")
        )
        let staleMerged = await staleLoadMore.value

        XCTAssertTrue(filterLoaded)
        XCTAssertFalse(staleMerged)
        XCTAssertEqual(model.requestLogFilters.query, "new-filter")
        XCTAssertEqual(model.requestLogs.map(\.id), [10])
        XCTAssertFalse(model.requestLogLoadingMore)
    }

    @MainActor
    func testForcedRequestLogRefreshCannotClobberNewerResponse() async {
        let calls = IntCounter()
        let client = makeClient { _ in
            if calls.next() == 1 {
                Thread.sleep(forTimeInterval: 0.15)
                return MockResponse(
                    statusCode: 200,
                    json: Self.requestLogsPageJSON(ids: [1], hasMore: false)
                )
            }
            Thread.sleep(forTimeInterval: 0.01)
            return MockResponse(
                statusCode: 200,
                json: Self.requestLogsPageJSON(ids: [2], hasMore: false)
            )
        }
        let model = AppModel(apiClient: client)

        let olderRefresh = Task { await model.refreshRequestLogs() }
        try? await Task.sleep(for: .milliseconds(25))
        let newerRefresh = Task { await model.refreshRequestLogs() }
        let newerSucceeded = await newerRefresh.value
        let olderSucceeded = await olderRefresh.value

        XCTAssertTrue(newerSucceeded)
        XCTAssertFalse(olderSucceeded)
        XCTAssertEqual(model.requestLogs.map(\.id), [2])
        XCTAssertFalse(model.isLoading(.requestLogs))
    }

    @MainActor
    func testAppModelClearOldRequestLogsPublishesDeletedCountAndRefreshes() async {
        let listCalls = IntCounter()
        let client = makeClient { request in
            switch (request.httpMethod, request.url?.path) {
            case ("GET", "/api/v1/manage/request-logs"):
                _ = listCalls.next()
                return MockResponse(statusCode: 200, json: Self.requestLogsJSON)
            case ("POST", "/api/v1/manage/request-logs/clear-old"):
                let body = Self.jsonBody(from: request)
                XCTAssertEqual(body?["days"] as? Int, 3)
                return MockResponse(statusCode: 200, json: #"{"ok":true,"deleted":137}"#)
            default:
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }
        let model = AppModel(apiClient: client)
        let initialLoad = await model.loadSection(.requestLogs)
        XCTAssertTrue(initialLoad)

        let succeeded = await model.clearOldRequestLogs()

        XCTAssertTrue(succeeded)
        XCTAssertEqual(model.actionFeedback?.message, "已清理 137 条旧日志")
        XCTAssertNil(model.managementOperationError)
        // The list is re-fetched after the clear (initial load + refresh).
        XCTAssertEqual(listCalls.next(), 3)
    }

    func testRequestLogAndDetailDecodeCacheFieldsIncludingTTLSplit() async throws {
        let client = makeClient { request in
            switch request.url?.path {
            case "/api/v1/manage/request-logs":
                return MockResponse(
                    statusCode: 200,
                    json: #"{"logs":[{"id":9,"requestId":"req-9","modelId":"claude-opus-4-8","stream":true,"channel":"anthropic","providerType":"anthropic_messages","status":"success","inputTokens":1500,"outputTokens":200,"totalTokens":1700,"readCacheTokens":1200,"readCacheHitRate":0.8,"writeCacheTokens":3000,"writeCache5mTokens":2000,"writeCache1hTokens":1000,"costUsd":0.0125,"latencyMs":900,"ttftMs":120,"createdAtMs":1754000120000,"createdAt":"2026-08-13T00:00:00Z","errorMessage":null,"upstreamRequestBodyBytes":2048}]}"#
                )
            case "/api/v1/manage/request-logs/9":
                return MockResponse(
                    statusCode: 200,
                    json: #"{"log":{"id":9,"requestId":"req-9","modelId":"claude-opus-4-8","stream":true,"channel":"anthropic","providerType":"anthropic_messages","status":"success","inputTokens":1500,"outputTokens":200,"totalTokens":1700,"readCacheTokens":1200,"readCacheHitRate":0.8,"writeCacheTokens":3000,"writeCache5mTokens":2000,"writeCache1hTokens":1000,"costUsd":0.0125,"latencyMs":900,"ttftMs":120,"createdAtMs":1754000120000,"createdAt":"2026-08-13T00:00:00Z","errorMessage":null,"upstreamRequestBodyBytes":2048,"requestHeadersJson":null,"requestJson":null,"upstreamRequestHeadersJson":null,"upstreamRequestJson":null,"upstreamResponseSse":null,"responseJson":null}}"#
                )
            default:
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }

        let logs = try await client.requestLogs().logs
        XCTAssertEqual(logs[0].readCacheTokens, 1200)
        XCTAssertEqual(logs[0].readCacheHitRate, 0.8)
        XCTAssertEqual(logs[0].writeCacheTokens, 3000)
        XCTAssertEqual(logs[0].writeCache5mTokens, 2000)
        XCTAssertEqual(logs[0].writeCache1hTokens, 1000)

        let detail = try await client.requestLogDetail(id: 9)
        XCTAssertEqual(detail.readCacheTokens, 1200)
        XCTAssertEqual(detail.readCacheHitRate, 0.8)
        XCTAssertEqual(detail.writeCacheTokens, 3000)
        XCTAssertEqual(detail.writeCache5mTokens, 2000)
        XCTAssertEqual(detail.writeCache1hTokens, 1000)
    }

    /// The daemon omits the TTL-split keys entirely when the upstream did
    /// not report them; decoding must not require their presence.
    func testRequestLogDecodesWhenTTLSplitKeysAreOmitted() async throws {
        let client = makeClient { _ in
            MockResponse(statusCode: 200, json: Self.requestLogsJSON)
        }

        let logs = try await client.requestLogs().logs

        XCTAssertNil(logs[0].readCacheTokens)
        XCTAssertNil(logs[0].writeCache5mTokens)
        XCTAssertNil(logs[0].writeCache1hTokens)
    }

    func testRequestLogSummaryFormattersAlignWithLegacyGUI() {
        XCTAssertEqual(formatGroupedInt(0), "0")
        XCTAssertEqual(formatGroupedInt(999), "999")
        XCTAssertEqual(formatGroupedInt(1_234_567), "1,234,567")
        XCTAssertEqual(formatGroupedInt(-1_234), "-1,234")

        XCTAssertEqual(formatByteCount(512), "512 B")
        XCTAssertEqual(formatByteCount(1_536), "1.5 KB")
        XCTAssertEqual(formatByteCount(3_670_016), "3.50 MB")

        XCTAssertEqual(readCacheSummary(tokens: 1_200, hitRate: 0.833), "1,200 tokens(83.3%)")
        XCTAssertEqual(readCacheSummary(tokens: 1_200, hitRate: nil), "1,200 tokens")
        XCTAssertEqual(readCacheSummary(tokens: nil, hitRate: 0.8), "未记录")

        XCTAssertEqual(
            writeCacheSummary(tokens: 3_000, fiveMinuteTokens: 2_000, oneHourTokens: 1_000),
            "3,000 tokens [5m 2,000, 1h 1,000]"
        )
        XCTAssertEqual(
            writeCacheSummary(tokens: 3_000, fiveMinuteTokens: nil, oneHourTokens: nil),
            "3,000 tokens"
        )
        XCTAssertEqual(
            writeCacheSummary(tokens: nil, fiveMinuteTokens: 5, oneHourTokens: nil),
            "未记录"
        )
    }

    func testFetchProviderModelsSendsCamelCaseBodyAndOmitsEmptyOptionals() async throws {
        let timeouts = StringRecorder()
        let client = makeClient { request in
            guard request.url?.path == "/api/v1/manage/gateway/provider/models/fetch" else {
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
            XCTAssertEqual(request.httpMethod, "POST")
            timeouts.record("\(request.timeoutInterval)")
            let body = Self.jsonBody(from: request)
            XCTAssertEqual(body?["providerName"] as? String, "glm")
            XCTAssertEqual(body?["baseUrl"] as? String, "https://open.bigmodel.cn/api/anthropic")
            XCTAssertEqual(body?["modelsUrl"] as? String, "https://open.bigmodel.cn/api/paas/v4/models")
            XCTAssertEqual(body?["providerType"] as? String, "anthropic_messages")
            // The stored key should be reused when the user typed nothing.
            XCTAssertNil(body?["apiKey"])
            return MockResponse(
                statusCode: 200,
                json: #"{"ok":true,"models":["glm-5","glm-5-air"],"attempts":[{"url":"https://open.bigmodel.cn/api/paas/v4/models","status":200,"error":null,"preview":null}]}"#
            )
        }

        let response = try await client.fetchGatewayProviderModels(
            providerName: "glm",
            baseUrl: "https://open.bigmodel.cn/api/anthropic",
            modelsUrl: "https://open.bigmodel.cn/api/paas/v4/models",
            providerType: "anthropic_messages",
            apiKey: nil
        )

        XCTAssertTrue(response.ok)
        XCTAssertEqual(response.models, ["glm-5", "glm-5-air"])
        XCTAssertEqual(response.attempts.first?.status, 200)
        XCTAssertEqual(timeouts.values, ["60.0"])
    }

    func testFetchProviderModelsDecodesFailureAttempts() async throws {
        let client = makeClient { _ in
            MockResponse(
                statusCode: 200,
                json: #"{"ok":false,"models":[],"attempts":[{"url":"https://a.example/v1/models","status":401,"error":null,"preview":"{\"error\":\"unauthorized\"}"},{"url":"https://b.example/models","status":null,"error":"连接超时","preview":null}]}"#
            )
        }

        let response = try await client.fetchGatewayProviderModels(
            providerName: nil,
            baseUrl: "https://a.example",
            modelsUrl: nil,
            providerType: "open_ai_responses",
            apiKey: "new-key"
        )

        XCTAssertFalse(response.ok)
        XCTAssertEqual(response.attempts.count, 2)
        XCTAssertEqual(response.attempts[0].status, 401)
        XCTAssertEqual(response.attempts[1].error, "连接超时")
    }

    func testFetchProviderUsageSendsOnlyProviderNameAndDecodesNormalizedUsage() async throws {
        let timeouts = StringRecorder()
        let client = makeClient { request in
            XCTAssertEqual(
                request.url?.path,
                "/api/v1/manage/gateway/provider/usage"
            )
            XCTAssertEqual(request.httpMethod, "POST")
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Authorization"),
                "Bearer fixture-token"
            )
            timeouts.record("\(request.timeoutInterval)")
            let body = Self.jsonBody(from: request)
            XCTAssertEqual(body?.count, 1)
            XCTAssertEqual(body?["providerName"] as? String, "primary")
            XCTAssertNil(body?["apiKey"])
            return MockResponse(
                statusCode: 200,
                json: #"{"ok":true,"providerName":"primary","usage":{"source":"sub2api","balanceStatus":"available","billingStatus":"available","remaining":42.5,"unlimited":false,"unit":"USD","balanceMode":"credit","planName":"Pro","accountValid":true,"accountStatus":"active","todayCost":0.12,"todayActualCost":0.09,"groupRateMultiplier":1.2,"userRateMultiplier":0.8,"resolvedRateMultiplier":0.96,"effectiveRateMultiplier":1.44,"peakRateEnabled":true,"peakStart":"08:00","peakEnd":"10:00","peakRateMultiplier":1.5,"appliedPeakMultiplier":1.5,"timezone":"Asia/Shanghai","observedAt":"2026-08-15T12:00:00Z"}}"#
            )
        }

        let response = try await client.fetchGatewayProviderUsage(providerName: "primary")

        XCTAssertTrue(response.ok)
        XCTAssertEqual(response.providerName, "primary")
        XCTAssertEqual(response.usage.source, "sub2api")
        XCTAssertEqual(response.usage.remaining, 42.5)
        XCTAssertEqual(response.usage.unit, "USD")
        XCTAssertEqual(response.usage.accountStatus, "active")
        XCTAssertEqual(response.usage.todayCost, 0.12)
        XCTAssertEqual(response.usage.todayActualCost, 0.09)
        XCTAssertEqual(response.usage.resolvedRateMultiplier, 0.96)
        XCTAssertEqual(response.usage.effectiveRateMultiplier, 1.44)
        XCTAssertEqual(response.usage.appliedPeakMultiplier, 1.5)
        XCTAssertEqual(timeouts.values, ["30.0"])
    }

    func testCostEstimatorMatchesSub2ApiCodexFallbackAndMultiplier() {
        let event = TokenEvent(
            service: .codex,
            timestamp: Date(),
            model: "gpt-5.3-codex",
            inputTokens: 1_000_000,
            outputTokens: 500_000,
            cacheReadTokens: 100_000,
            cacheCreationTokens: 50_000
        )

        // Sub2API fallback: $1.50 input, $12 output, $0.15 cache-read,
        // $1.50 cache-write per million tokens, then ×0.6.
        let estimated = CostEstimator.cost(of: event, multiplier: 0.6)
        XCTAssertEqual(estimated, 4.554, accuracy: 0.000_001)
    }

    func testProviderUsageFormattingCoversPartialStatusesAndUnlimitedBalance() throws {
        let statuses = [
            "available": "可用",
            "unsupported": "服务商不支持查询",
            "unauthorized": "API Key 未授权",
            "forbidden": "无权查询",
            "temporarily_unavailable": "暂时不可用",
            "invalid_response": "响应格式异常",
        ]
        for (status, expected) in statuses {
            XCTAssertEqual(providerUsageStatusText(status), expected)
        }
        XCTAssertEqual(providerUsageStatusText("future_status"), "状态未知")
        XCTAssertNil(providerUsageAccountWarning("active"))
        XCTAssertEqual(providerUsageAccountWarning("disabled"), "API Key 已停用。")
        XCTAssertEqual(providerUsageAccountWarning("inactive"), "API Key 已停用。")
        XCTAssertEqual(providerUsageAccountWarning("quota_exhausted"), "API Key 额度已耗尽。")
        XCTAssertEqual(providerUsageAccountWarning("expired"), "API Key 已过期。")

        let data = Data(
            #"{"ok":false,"providerName":"partial","usage":{"source":"custom","balanceStatus":"available","billingStatus":"unsupported","unlimited":true,"peakRateEnabled":true,"peakStart":"22:00","peakEnd":"06:00","peakRateMultiplier":1.25,"timezone":"Asia/Shanghai"}}"#.utf8
        )
        let usage = try JSONDecoder().decode(ManageProviderUsageResponse.self, from: data).usage

        XCTAssertEqual(providerUsageBalanceText(usage), "无限")
        XCTAssertNil(providerUsageMultiplierText(usage.effectiveRateMultiplier))
        XCTAssertEqual(providerUsageStatusText(usage.billingStatus), "服务商不支持查询")
        XCTAssertEqual(providerUsageBillingText(usage), "服务商不支持查询")
        XCTAssertEqual(
            providerUsagePeakText(usage),
            "峰时 ×1.25 · 22:00–06:00 · Asia/Shanghai"
        )

        let newAPIData = Data(
            #"{"ok":true,"providerName":"new-api","usage":{"source":"new_api","balanceStatus":"available","billingStatus":"unsupported","remaining":2.5,"unlimited":false,"unit":"USD","accountValid":true,"accountStatus":"active"}}"#.utf8
        )
        let newAPIUsage = try JSONDecoder()
            .decode(ManageProviderUsageResponse.self, from: newAPIData)
            .usage
        XCTAssertEqual(providerUsageBalanceText(newAPIUsage), "2.5 USD")
        XCTAssertEqual(providerUsageBillingText(newAPIUsage), "未提供")
    }

    func testGatewayQuotaDockUsesSeparateProgressAndLowBalanceThresholds() throws {
        let data = Data(
            #"{"ok":true,"providerName":"primary","usage":{"source":"sub2api","balanceStatus":"available","billingStatus":"available","remaining":14766.67464804,"unlimited":false,"unit":"USD","accountValid":true,"effectiveRateMultiplier":1.0}}"#.utf8
        )
        let usage = try JSONDecoder().decode(ManageProviderUsageResponse.self, from: data).usage
        let presentation = gatewayQuotaMeterPresentation(usage)

        XCTAssertEqual(gatewayQuotaBalanceText(usage), "$14,766.67")
        XCTAssertEqual(presentation.fraction, 1)
        XCTAssertEqual(presentation.statusText, "余额充足")
        XCTAssertEqual(presentation.tone, .normal)
        XCTAssertEqual(presentation.warningThreshold, 3)

        let stalePresentation = gatewayQuotaCachedPresentation(
            presentation,
            state: .refreshFailed
        )
        XCTAssertEqual(stalePresentation.fraction, 1)
        XCTAssertEqual(stalePresentation.statusText, "刷新失败 · 上次数据")
        XCTAssertEqual(stalePresentation.tone, .warning)
        XCTAssertEqual(stalePresentation.warningThreshold, 3)

        let refreshingPresentation = gatewayQuotaCachedPresentation(
            presentation,
            state: .refreshing
        )
        XCTAssertEqual(refreshingPresentation.fraction, 1)
        XCTAssertEqual(refreshingPresentation.statusText, "正在刷新 · 上次数据")
        XCTAssertEqual(refreshingPresentation.tone, .unavailable)

        XCTAssertNotEqual(
            GatewayQuotaRequestID(
                providerName: "primary",
                gatewayGeneration: 1,
                refreshGeneration: 0
            ),
            GatewayQuotaRequestID(
                providerName: "primary",
                gatewayGeneration: 2,
                refreshGeneration: 0
            )
        )

        let availableData = Data(
            #"{"ok":true,"providerName":"primary","usage":{"source":"sub2api","balanceStatus":"available","billingStatus":"available","remaining":4.0,"unlimited":false,"unit":"USD","accountValid":true}}"#.utf8
        )
        let availableUsage = try JSONDecoder().decode(
            ManageProviderUsageResponse.self,
            from: availableData
        ).usage
        let availablePresentation = gatewayQuotaMeterPresentation(availableUsage)

        XCTAssertEqual(availablePresentation.fraction, 0.2)
        XCTAssertEqual(availablePresentation.statusText, "余额充足")
        XCTAssertEqual(availablePresentation.tone, .normal)

        let lowData = Data(
            #"{"ok":true,"providerName":"primary","usage":{"source":"sub2api","balanceStatus":"available","billingStatus":"available","remaining":2.5,"unlimited":false,"unit":"USD","accountValid":true}}"#.utf8
        )
        let lowUsage = try JSONDecoder().decode(
            ManageProviderUsageResponse.self,
            from: lowData
        ).usage
        let lowPresentation = gatewayQuotaMeterPresentation(lowUsage)

        XCTAssertEqual(lowPresentation.fraction, 0.125)
        XCTAssertEqual(lowPresentation.statusText, "余额偏低")
        XCTAssertEqual(lowPresentation.tone, .warning)
        XCTAssertEqual(gatewayQuotaAmountText(-2.5, unit: "CNY"), "-¥2.50")
        XCTAssertEqual(gatewayQuotaProgressReference(unit: "CNY"), 20)
        XCTAssertEqual(gatewayQuotaWarningThreshold(unit: "CNY"), 3)

        let accountBalance = ManageSub2ApiAccountPoolResponse.Account.Balance(
            state: "available",
            remaining: 7.74314181,
            unlimited: false,
            unit: "USD",
            mode: "unrestricted",
            planName: "钱包余额",
            accountValid: true,
            accountStatus: "active",
            observedAt: "2026-08-17T02:35:18+08:00"
        )
        let accountPresentation = gatewayQuotaMeterPresentation(accountBalance)
        XCTAssertEqual(sub2ApiBalanceText(accountBalance), "$7.74")
        XCTAssertEqual(accountPresentation.fraction ?? -1, 0.3871570905, accuracy: 0.000_001)
        XCTAssertEqual(accountPresentation.statusText, "余额充足")
        XCTAssertEqual(accountPresentation.tone, .normal)

        let boundaryBalance = ManageSub2ApiAccountPoolResponse.Account.Balance(
            state: "available",
            remaining: 3,
            unlimited: false,
            unit: "USD",
            mode: "unrestricted",
            planName: nil,
            accountValid: true,
            accountStatus: "active",
            observedAt: nil
        )
        let boundaryPresentation = gatewayQuotaMeterPresentation(boundaryBalance)
        XCTAssertEqual(boundaryPresentation.fraction, 0.15)
        XCTAssertEqual(boundaryPresentation.statusText, "余额充足")
        XCTAssertEqual(boundaryPresentation.tone, .normal)
        XCTAssertEqual(gatewayQuotaSiteDisplayName("https://vip.mdkj.lol"), "mdkj")
        XCTAssertEqual(gatewayQuotaSiteDisplayName("https://api.wanfeng.me/v1"), "wanfeng")
    }

    func testSub2ApiAdminConnectionIsWriteOnlyAndUsesExpectedRoutes() async throws {
        let requests = StringRecorder()
        let client = makeClient { request in
            let method = request.httpMethod ?? ""
            let path = request.url?.path ?? ""
            requests.record("\(method) \(path)")
            switch (method, path) {
            case ("GET", "/api/v1/manage/gateway/sub2api"):
                return MockResponse(
                    statusCode: 200,
                    json: #"{"configured":false,"baseUrl":"","secretSet":false}"#
                )
            case ("POST", "/api/v1/manage/gateway/sub2api/config"):
                let body = Self.jsonBody(from: request)
                XCTAssertEqual(body?["baseUrl"] as? String, "https://sub2api.example/v1")
                XCTAssertEqual(body?["adminApiKey"] as? String, "admin-secret")
                XCTAssertEqual(body?["clearAdminApiKey"] as? Bool, false)
                return MockResponse(
                    statusCode: 200,
                    json: #"{"ok":true,"sub2api":{"configured":true,"baseUrl":"https://sub2api.example/v1","secretSet":true}}"#
                )
            case ("POST", "/api/v1/manage/gateway/sub2api/disconnect"):
                return MockResponse(
                    statusCode: 200,
                    json: #"{"ok":true,"sub2api":{"configured":false,"baseUrl":"","secretSet":false}}"#
                )
            default:
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }

        let empty = try await client.sub2ApiAdmin()
        XCTAssertFalse(empty.configured)
        XCTAssertFalse(empty.secretSet)

        let connected = try await client.updateSub2ApiAdmin(
            baseUrl: "https://sub2api.example/v1",
            adminApiKey: "admin-secret"
        )
        XCTAssertTrue(connected.configured)
        XCTAssertTrue(connected.secretSet)
        XCTAssertEqual(connected.baseUrl, "https://sub2api.example/v1")

        let disconnected = try await client.disconnectSub2ApiAdmin()
        XCTAssertFalse(disconnected.configured)
        XCTAssertEqual(
            requests.values,
            [
                "GET /api/v1/manage/gateway/sub2api",
                "POST /api/v1/manage/gateway/sub2api/config",
                "POST /api/v1/manage/gateway/sub2api/disconnect",
            ]
        )
    }

    func testSub2ApiAccountPoolRequestAndDecodeKeepRatesSeparate() async throws {
        let timeouts = StringRecorder()
        let client = makeClient { request in
            XCTAssertEqual(
                request.url?.path,
                "/api/v1/manage/gateway/sub2api/accounts"
            )
            XCTAssertEqual(request.httpMethod, "POST")
            XCTAssertEqual(
                Self.jsonBody(from: request)?["forceBillingRefresh"] as? Bool,
                true
            )
            timeouts.record("\(request.timeoutInterval)")
            return MockResponse(
                statusCode: 200,
                json: #"{"ok":true,"pool":{"source":"sub2api_admin","fetchedAtMs":1786752000000,"accounts":[{"id":2,"name":"primary","siteUrl":"https://sub2api.example/v1","platform":"openai","accountType":"apikey","status":"active","schedulable":true,"localRateMultiplier":1.0,"upstreamBilling":{"state":"available","resolvedRateMultiplier":0.06,"effectiveRateMultiplier":0.09,"observedAt":"2026-08-15T00:00:00Z","freshUntil":"2026-08-15T00:05:00Z","stale":false},"upstreamBalance":{"state":"available","remaining":16.9999,"unlimited":false,"unit":"USD","mode":"unrestricted","planName":"钱包余额","accountValid":true,"accountStatus":"active","observedAt":"2026-08-15T00:00:00Z"}},{"id":13,"name":"unsupported","platform":"openai","accountType":"apikey","status":"active","schedulable":true,"localRateMultiplier":0.5,"upstreamBilling":{"state":"unsupported","stale":false},"upstreamBalance":{"state":"not_exposed","unlimited":false}}],"warnings":["usage_probe_not_exposed"]}}"#
            )
        }

        let response = try await client.fetchSub2ApiAccounts(forceBillingRefresh: true)

        XCTAssertTrue(response.ok)
        XCTAssertEqual(response.pool.accounts.count, 2)
        XCTAssertEqual(response.pool.accounts[0].siteUrl, "https://sub2api.example/v1")
        XCTAssertNil(response.pool.accounts[1].siteUrl)
        XCTAssertEqual(response.pool.accounts[0].localRateMultiplier, 1)
        XCTAssertEqual(
            response.pool.accounts[0].upstreamBilling.effectiveRateMultiplier,
            0.09
        )
        XCTAssertEqual(response.pool.accounts[0].upstreamBalance.remaining, 16.9999)
        XCTAssertEqual(response.pool.accounts[1].upstreamBilling.state, "unsupported")
        XCTAssertEqual(response.pool.warnings, ["usage_probe_not_exposed"])
        XCTAssertEqual(timeouts.values, ["60.0"])
    }

    func testProviderRecentAccountUsesExpectedProtectedRoute() async throws {
        let client = makeClient { request in
            XCTAssertEqual(
                request.url?.path,
                "/api/v1/manage/gateway/provider/recent-account"
            )
            XCTAssertEqual(request.httpMethod, "POST")
            XCTAssertEqual(
                Self.jsonBody(from: request)?["providerName"] as? String,
                "openai"
            )
            XCTAssertEqual(request.timeoutInterval, 15)
            return MockResponse(
                statusCode: 200,
                json: #"{"ok":true,"providerName":"openai","account":{"accountId":2,"accountName":"ChatGPT-Plus【高并发-特惠通道】","createdAt":"2026-08-17T02:35:18+08:00"}}"#
            )
        }

        let response = try await client.fetchGatewayProviderRecentAccount(
            providerName: "openai"
        )

        XCTAssertTrue(response.ok)
        XCTAssertEqual(response.providerName, "openai")
        XCTAssertEqual(response.account?.accountId, 2)
        XCTAssertEqual(response.account?.accountName, "ChatGPT-Plus【高并发-特惠通道】")
    }

    func testSub2ApiUpstreamErrorsRemainActionable() async {
        let client = makeClient { _ in
            MockResponse(
                statusCode: 502,
                json: #"{"error":"Sub2API 管理密钥无效"}"#
            )
        }

        do {
            _ = try await client.fetchSub2ApiAccounts(forceBillingRefresh: false)
            XCTFail("Expected the account-pool request to fail")
        } catch let error as APIClientError {
            XCTAssertEqual(
                error,
                .operationFailed("Sub2API 管理密钥无效")
            )
        } catch {
            XCTFail("Unexpected error: \(error)")
        }
    }

    func testSub2ApiOverviewFormattingCoversCapabilityStatesAndNegativeBalance() throws {
        let states = [
            "available": "可用",
            "not_applicable": "不适用",
            "not_exposed": "未提供",
            "unsupported": "不支持",
            "unauthorized": "未授权",
            "forbidden": "无权限",
            "temporarily_unavailable": "暂不可用",
            "invalid_response": "响应异常",
        ]
        for (state, expected) in states {
            XCTAssertEqual(sub2ApiCapabilityStateText(state), expected)
        }
        XCTAssertEqual(sub2ApiCapabilityStateText("future"), "未知")
        XCTAssertEqual(sub2ApiMultiplierText(0.089), "×0.089")
        XCTAssertEqual(sub2ApiMultiplierText(nil), "—")

        func decodeBalance(_ json: String) throws
            -> ManageSub2ApiAccountPoolResponse.Account.Balance
        {
            try JSONDecoder().decode(
                ManageSub2ApiAccountPoolResponse.Account.Balance.self,
                from: Data(json.utf8)
            )
        }

        XCTAssertEqual(
            sub2ApiBalanceText(
                try decodeBalance(
                    #"{"state":"available","remaining":16.9999,"unlimited":false,"unit":"USD"}"#
                )
            ),
            "$17.00"
        )
        XCTAssertEqual(
            sub2ApiBalanceText(
                try decodeBalance(
                    #"{"state":"available","remaining":-0.071,"unlimited":false,"unit":"USD"}"#
                )
            ),
            "-$0.07"
        )
        XCTAssertEqual(
            sub2ApiBalanceText(
                try decodeBalance(
                    #"{"state":"available","unlimited":true,"unit":"USD"}"#
                )
            ),
            "无限"
        )
        XCTAssertEqual(
            sub2ApiBalanceText(
                try decodeBalance(
                    #"{"state":"not_exposed","unlimited":false}"#
                )
            ),
            "未提供"
        )
    }

    @MainActor
    func testSub2ApiAccountPoolCachesAndPreservesLastSuccessOnRefreshFailure() async {
        let accountCalls = StringRecorder()
        let accountResponses = MockResponseSequence([
            MockResponse(
                statusCode: 200,
                json: #"{"ok":true,"pool":{"source":"sub2api_admin","fetchedAtMs":1786752000000,"accounts":[{"id":2,"name":"primary","platform":"openai","accountType":"apikey","status":"active","schedulable":true,"localRateMultiplier":1.0,"upstreamBilling":{"state":"available","effectiveRateMultiplier":0.09,"stale":false},"upstreamBalance":{"state":"available","remaining":17.0,"unlimited":false,"unit":"USD"}}]}}"#
            ),
            MockResponse(statusCode: 503, json: #"{"error":"temporarily unavailable"}"#),
        ])
        let client = makeClient { request in
            switch (request.httpMethod, request.url?.path) {
            case ("GET", "/api/v1/manage/gateway/sub2api"):
                return MockResponse(
                    statusCode: 200,
                    json: #"{"configured":true,"baseUrl":"https://sub2api.example","secretSet":true}"#
                )
            case ("POST", "/api/v1/manage/gateway/sub2api/accounts"):
                accountCalls.record("accounts")
                return accountResponses.next()
            case ("POST", "/api/v1/manage/gateway/sub2api/disconnect"):
                return MockResponse(
                    statusCode: 200,
                    json: #"{"ok":true,"sub2api":{"configured":false,"baseUrl":"","secretSet":false}}"#
                )
            default:
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }
        let model = AppModel(apiClient: client)
        let firstUpdate = Date(timeIntervalSince1970: 1_786_752_000)

        await model.refreshSub2ApiAccountPool(now: firstUpdate)
        XCTAssertEqual(model.sub2ApiAccountPool?.accounts.first?.name, "primary")
        XCTAssertNil(model.sub2ApiAccountPoolError)
        XCTAssertEqual(accountCalls.values.count, 1)

        await model.refreshSub2ApiAccountPool(
            now: firstUpdate.addingTimeInterval(60)
        )
        XCTAssertEqual(accountCalls.values.count, 1)

        await model.refreshSub2ApiAccountPool(
            forceBillingRefresh: true,
            now: firstUpdate.addingTimeInterval(120)
        )
        XCTAssertEqual(model.sub2ApiAccountPool?.accounts.first?.name, "primary")
        XCTAssertNotNil(model.sub2ApiAccountPoolError)
        XCTAssertEqual(accountCalls.values.count, 2)

        let disconnected = await model.disconnectSub2ApiAdmin()
        XCTAssertTrue(disconnected)
        XCTAssertNil(model.sub2ApiAccountPool)
        XCTAssertNil(model.sub2ApiAccountPoolError)
    }

    func testMergedModelLinesKeepOrderDedupeAndAppend() {
        XCTAssertEqual(
            mergedModelLines(
                existing: "glm-5\n\nglm-5-air\nglm-5",
                fetched: ["glm-5", "glm-5-flash", " glm-5-air ", "glm-5v"]
            ),
            "glm-5\nglm-5-air\nglm-5-flash\nglm-5v"
        )
        XCTAssertEqual(mergedModelLines(existing: "", fetched: ["a", "b"]), "a\nb")
        XCTAssertEqual(mergedModelLines(existing: "a,b", fetched: []), "a\nb")
    }

    func testProviderFetchAttemptLinesTruncatePreviewAndCapCount() {
        let longPreview = String(repeating: "x", count: 200)
        let attempts = (1...6).map { index in
            ManageProviderModelsFetchResponse.Attempt(
                url: "https://example.test/\(index)",
                status: index == 1 ? 500 : nil,
                error: index == 1 ? nil : "连接失败",
                preview: index == 1 ? longPreview : nil
            )
        }

        let lines = providerFetchAttemptLines(attempts)

        XCTAssertEqual(lines.count, 4)
        XCTAssertTrue(lines[0].hasPrefix("https://example.test/1 — HTTP 500 — "))
        XCTAssertEqual(lines[0].count, "https://example.test/1 — HTTP 500 — ".count + 120)
        XCTAssertEqual(lines[1], "https://example.test/2 — 连接失败")
    }

    func testInferredClaudeModelAliasesMatchLegacyRules() {
        let aliases = inferredModelAliases(models: [
            " Claude-Opus-4-8 ",
            "claude-sonnet-4-6",
            "glm-5",
        ])

        XCTAssertEqual(aliases["opus-4.8"], " Claude-Opus-4-8 ")
        XCTAssertEqual(aliases["sonnet-4.6"], "claude-sonnet-4-6")
        XCTAssertEqual(aliases.count, 2)
    }

    func testInferredAliasSkippedWhenModelListAlreadyContainsAliasName() {
        let aliases = inferredModelAliases(models: [
            "claude-opus-4-8",
            "opus-4.8",
        ])

        XCTAssertTrue(aliases.isEmpty)
    }

    func testMergedModelAliasesKeepExplicitMappingPriority() {
        let merged = mergedModelAliases(
            models: ["claude-opus-4-8", "claude-sonnet-4-6"],
            explicit: ["opus-4.8": "my-custom-target"]
        )

        XCTAssertEqual(merged["opus-4.8"], "my-custom-target")
        XCTAssertEqual(merged["sonnet-4.6"], "claude-sonnet-4-6")
        XCTAssertEqual(merged.count, 2)
    }

    func testBuildNumbersMismatchOnlyWhenBothBuildsAreKnown() {
        XCTAssertFalse(AppModel.buildNumbersMismatch(guiBuild: "388", daemonBuild: 388))
        XCTAssertTrue(AppModel.buildNumbersMismatch(guiBuild: "388", daemonBuild: 387))
        XCTAssertFalse(AppModel.buildNumbersMismatch(guiBuild: nil, daemonBuild: 387))
        XCTAssertFalse(AppModel.buildNumbersMismatch(guiBuild: "388", daemonBuild: nil))
        XCTAssertFalse(AppModel.buildNumbersMismatch(guiBuild: "development", daemonBuild: 387))
    }

    func testDaemonUpgradeDetailInterpolatesBuildNumbers() {
        XCTAssertEqual(
            AppModel.daemonUpgradeDetailText(
                guiBuild: "407",
                daemonBuild: 406
            ),
            "界面构建 407 高于后台 406，需要手动更新后台服务"
        )
        XCTAssertEqual(
            AppModel.daemonUpgradeDetailText(
                guiBuild: "407",
                daemonBuild: 408
            ),
            "后台构建 408 高于界面 407，需要手动处理"
        )
    }

    func testDaemonUpgradeOnlyTargetsAnOlderRunningBuild() {
        XCTAssertTrue(AppModel.daemonRequiresUpgrade(guiBuild: "404", daemonBuild: 392))
        XCTAssertFalse(AppModel.daemonRequiresUpgrade(guiBuild: "404", daemonBuild: 404))
        XCTAssertFalse(AppModel.daemonRequiresUpgrade(guiBuild: "404", daemonBuild: 405))
        XCTAssertFalse(AppModel.daemonRequiresUpgrade(guiBuild: nil, daemonBuild: 392))
        XCTAssertFalse(AppModel.daemonRequiresUpgrade(guiBuild: "development", daemonBuild: 392))
        XCTAssertFalse(AppModel.daemonRequiresUpgrade(guiBuild: "404", daemonBuild: nil))
    }

    func testLoadedDaemonAgentRejectsAStaleBundleBuild() throws {
        let configuration = DaemonLaunchConfiguration(
            helperURL: URL(fileURLWithPath: "/fixture/ThreadRelay.app/Contents/Helpers/threadrelay-daemon"),
            configURL: URL(fileURLWithPath: "/fixture/data/config.toml"),
            launchAgentURL: URL(fileURLWithPath: "/fixture/daemon.plist"),
            logURL: URL(fileURLWithPath: "/fixture/data/logs/daemon.log"),
            homeURL: URL(fileURLWithPath: "/fixture/home"),
            buildIdentifier: "389"
        )
        let stagedHelper = try configuration.stagedHelperURL()
        let output = """
        state = running
        program = \(stagedHelper.path)
        arguments = {
            \(stagedHelper.path)
            --config
            /fixture/data/config.toml
            daemon
        }
        environment = {
            THREADRELAY_HOME => /fixture/data
            THREADRELAY_BUNDLE_BUILD => 388
        }
        """

        XCTAssertFalse(DaemonLauncher.loadedAgentMatches(output: output, configuration: configuration))
    }

    func testGitHubReleaseDecodesAndValidatesDownloadPage() throws {
        let release = try JSONDecoder().decode(
            GitHubRelease.self,
            from: Data(
                #"{"tag_name":"v0.5.1","html_url":"https://github.com/mps233/threadrelay/releases/tag/v0.5.1","body":"发布说明"}"#.utf8
            )
        )
        XCTAssertEqual(release.tagName, "v0.5.1")
        XCTAssertEqual(release.body, "发布说明")
        XCTAssertEqual(
            release.validatedURL,
            URL(string: "https://github.com/mps233/threadrelay/releases/tag/v0.5.1")
        )

        func validatedURL(_ htmlURL: String) throws -> URL? {
            try JSONDecoder().decode(
                GitHubRelease.self,
                from: Data(#"{"tag_name":"v1","html_url":"\#(htmlURL)"}"#.utf8)
            ).validatedURL
        }

        XCTAssertNil(try validatedURL("http://github.com/mps233/threadrelay/releases/tag/v1"))
        XCTAssertNil(try validatedURL("https://evil.example/mps233/threadrelay/releases/tag/v1"))
        XCTAssertNil(try validatedURL("https://github.com/other/repo/releases/tag/v1"))
    }

    func testUpdateCheckerComparesVersionsAndKeepsSilentFailures() async {
        MockURLProtocol.install { request in
            XCTAssertEqual(request.url?.host, "api.github.com")
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Accept"),
                "application/vnd.github+json"
            )
            return MockResponse(
                statusCode: 200,
                json: #"{"tag_name":"v0.6.0","html_url":"https://github.com/mps233/threadrelay/releases/tag/v0.6.0","body":null}"#
            )
        }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [MockURLProtocol.self]
        let session = URLSession(configuration: configuration)

        let update = await UpdateChecker.availableUpdate(session: session, currentVersion: "0.5.0")
        XCTAssertEqual(
            update,
            AvailableUpdate(
                version: "v0.6.0",
                url: URL(string: "https://github.com/mps233/threadrelay/releases/tag/v0.6.0")!
            )
        )

        let current = await UpdateChecker.availableUpdate(session: session, currentVersion: "0.6.0")
        XCTAssertNil(current)

        MockURLProtocol.install { _ in
            MockResponse(statusCode: 500, json: #"{"error":"boom"}"#)
        }
        let failed = await UpdateChecker.availableUpdate(session: session, currentVersion: "0.5.0")
        XCTAssertNil(failed)
    }

    func testUpdateCheckerRejectsReleaseWithForeignDownloadPage() async {
        MockURLProtocol.install { _ in
            MockResponse(
                statusCode: 200,
                json: #"{"tag_name":"v0.6.0","html_url":"https://evil.example/download","body":null}"#
            )
        }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [MockURLProtocol.self]
        let session = URLSession(configuration: configuration)

        do {
            _ = try await UpdateChecker.fetchLatestRelease(session: session, currentVersion: "0.5.0")
            XCTFail("Expected the unvalidated release URL to be rejected")
        } catch {
            XCTAssertEqual((error as? URLError)?.code, .unsupportedURL)
        }
        let update = await UpdateChecker.availableUpdate(session: session, currentVersion: "0.5.0")
        XCTAssertNil(update)
    }

    @MainActor
    func testBatchSessionMoveReportsPartialFailureAndKeepsFailedIds() async {
        let client = makeClient { request in
            switch (request.httpMethod, request.url?.path) {
            case ("POST", "/api/v1/manage/sessions/provider"):
                let threadId = Self.jsonBody(from: request)?["threadId"] as? String
                if threadId == "thread-a" {
                    return MockResponse(statusCode: 200, json: #"{"ok":true,"deleted":null}"#)
                }
                return MockResponse(statusCode: 500, json: #"{"error":"move failed"}"#)
            case ("GET", "/api/v1/manage/sessions"):
                return MockResponse(
                    statusCode: 200,
                    json: #"{"ok":true,"threads":[],"providers":["openai"],"total":0}"#
                )
            default:
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }
        let model = AppModel(apiClient: client)

        let result = await model.moveCodexSessions(ids: ["thread-a", "thread-b"], to: "openai")

        XCTAssertEqual(result.movedIds, ["thread-a"])
        XCTAssertEqual(result.failedIds, ["thread-b"])
        XCTAssertNil(model.actionFeedback)
        let message = model.managementOperationError
        XCTAssertNotNil(message)
        XCTAssertTrue(message?.contains("成功 1") == true)
        XCTAssertTrue(message?.contains("失败 1") == true)
        XCTAssertEqual(model.sectionErrors[.sessions], message)
    }

    @MainActor
    func testBatchSessionMoveSuccessPublishesUnifiedFeedback() async {
        let client = makeClient { request in
            switch (request.httpMethod, request.url?.path) {
            case ("POST", "/api/v1/manage/sessions/provider"):
                return MockResponse(statusCode: 200, json: #"{"ok":true,"deleted":null}"#)
            case ("GET", "/api/v1/manage/sessions"):
                return MockResponse(
                    statusCode: 200,
                    json: #"{"ok":true,"threads":[],"providers":["openai"],"total":0}"#
                )
            default:
                return MockResponse(statusCode: 500, json: #"{"error":"unexpected path"}"#)
            }
        }
        let model = AppModel(apiClient: client)

        let result = await model.moveCodexSessions(ids: ["thread-a", "thread-b"], to: nil)

        XCTAssertEqual(result.movedIds, ["thread-a", "thread-b"])
        XCTAssertTrue(result.failedIds.isEmpty)
        XCTAssertNil(model.managementOperationError)
        XCTAssertEqual(model.actionFeedback?.message, "已移动 2 个会话")
    }

    private static let healthJSON = #"{"service":"threadrelay","apiMajor":1,"ready":true}"#
    private static let dashboardJSON = #"{"service":{"service":"threadrelay","apiMajor":1,"ready":true,"instanceId":"fixture-instance","pid":123,"startedAtMs":456},"bridgeRunning":true,"remoteControlConnected":true,"remoteControlHealthy":true,"executionClients":{"codexApp":{"configured":true,"connected":true},"vscode":{"configured":true,"connected":true},"cli":{"configured":false,"connected":false}},"messageChannels":{"telegram":{"accountCount":2,"connectedAccountCount":1},"feishu":{"accountCount":1,"connectedAccountCount":1},"wechat":{"accountCount":1,"connectedAccountCount":1},"wecom":{"accountCount":0,"connectedAccountCount":0}},"aiGatewayEnabled":true,"aiGatewayProviderCount":2,"requestLoggingEnabled":true}"#
    private static let imAccountsJSON = #"{"service":{"service":"threadrelay","apiMajor":1,"ready":true,"instanceId":"fixture-instance","pid":123,"startedAtMs":456},"accounts":[{"platform":"telegram","accountId":"telegram-main","displayName":"主 Telegram","enabled":true,"configured":true,"secretSet":true,"connecting":false,"polling":true,"connected":true,"lastError":null,"lastEventAtMs":1754000120000,"lastInboundAtMs":1754000100000},{"platform":"wecom","accountId":"wecom-offline","displayName":"企业微信","enabled":true,"configured":true,"secretSet":true,"connecting":false,"polling":false,"connected":false,"lastError":"连接失败","lastEventAtMs":null,"lastInboundAtMs":null}]}"#
    private static let codexStatusJSON = #"{"codexHome":"/fixture/.codex","configured":true,"configOk":true,"authOk":true,"providerOk":true,"configError":null,"authError":null,"guiConfigured":true,"guiError":null,"remoteControlSupported":true,"remoteControlConfigured":true,"remoteControlError":null,"providers":[{"name":"ai-gateway","baseUrl":"http://127.0.0.1:3847/backend-api","secretSet":true,"supportsWebsockets":true}],"imageGenerationEnabled":true,"connectionMode":"standard"}"#
    private static let lifecycleJSON = #"{"service":{"service":"threadrelay","apiMajor":1,"ready":true,"instanceId":"fixture-instance","pid":123,"startedAtMs":456},"executable":"/fixture/ThreadRelay","configPath":"/fixture/config.toml","bind":"127.0.0.1:3847","runtime":{"state":"active","productVersion":"0.5.0","buildNumber":388,"apiMajor":1},"protectedWorkItems":{"aiGatewayRequests":0,"codexTurns":0,"imStreams":0,"pendingApprovals":0,"remoteControlRequests":0,"total":0},"management":{"state":"unmanaged","mode":"readOnly","canControl":false,"installationId":null,"leaseGeneration":null,"leaseExpiresAtMs":null}}"#
    private static let originalV1DashboardJSON = #"{"service":{"service":"threadrelay","apiMajor":1,"ready":true,"instanceId":"legacy-instance","pid":456,"startedAtMs":789},"bridgeRunning":true,"remoteControlConnected":false,"remoteControlHealthy":false,"codexAppConfigured":true,"imAccountCount":5,"connectedImAccountCount":3,"aiGatewayEnabled":false,"aiGatewayProviderCount":1,"requestLoggingEnabled":true}"#
    // Mirrors the daemon payload where the Anthropic TTL-split keys are
    // omitted entirely when unreported (serde skip_serializing_if).
    private static let requestLogsJSON = #"{"logs":[{"id":7,"requestId":"req-7","modelId":"model-a","stream":true,"channel":"primary","providerType":"openai_responses","status":"success","inputTokens":10,"outputTokens":20,"totalTokens":30,"readCacheTokens":null,"readCacheHitRate":null,"writeCacheTokens":null,"costUsd":0.01,"latencyMs":1200,"ttftMs":300,"createdAtMs":1754000120000,"createdAt":"2026-08-13T00:00:00Z","errorMessage":null,"upstreamRequestBodyBytes":128}]}"#

    private static func lifecyclePayload(
        instanceId: String,
        build: Int,
        executable: String,
        installationId: String?
    ) -> String {
        let management: String
        if let installationId {
            management = #"{"state":"managed","mode":"managed","canControl":true,"installationId":"\#(installationId)","leaseGeneration":1,"leaseExpiresAtMs":4102444800000}"#
        } else {
            management = #"{"state":"unmanaged","mode":"readOnly","canControl":false,"installationId":null,"leaseGeneration":null,"leaseExpiresAtMs":null}"#
        }
        return #"{"service":{"service":"threadrelay","apiMajor":1,"ready":true,"instanceId":"\#(instanceId)","pid":123,"startedAtMs":456},"executable":"\#(executable)","configPath":"/fixture/config.toml","bind":"127.0.0.1:3847","runtime":{"state":"active","productVersion":"0.5.0","buildNumber":\#(build),"apiMajor":1},"protectedWorkItems":{"aiGatewayRequests":0,"codexTurns":0,"imStreams":0,"pendingApprovals":0,"remoteControlRequests":0,"total":0},"management":\#(management)}"#
    }

    /// Models the build 410 shape: both lifecycle security fields introduced
    /// later are intentionally absent from this fixture.
    private static func build410LifecyclePayload(
        installationId: String,
        canControl: Bool
    ) -> String {
        #"{"service":{"service":"threadrelay","apiMajor":1,"ready":true,"instanceId":"build-410-instance","pid":410,"startedAtMs":1786896000000},"executable":"/fixture/runtimes/410/threadrelay-daemon","configPath":"/fixture/config.toml","bind":"127.0.0.1:3847","runtime":{"state":"active","productVersion":"0.5.0","buildNumber":410,"apiMajor":1},"protectedWorkItems":{"aiGatewayRequests":0,"codexTurns":0,"imStreams":0,"pendingApprovals":0,"remoteControlRequests":0,"total":0},"management":{"state":"managed","mode":"managed","canControl":\#(canControl),"installationId":"\#(installationId)","leaseGeneration":7,"leaseExpiresAtMs":4102444800000}}"#
    }

    private static func enhancedOperationJSON(
        requestId: String,
        phase: String,
        canCancel: Bool,
        message: String,
        error: String? = nil,
        recovery: String? = nil
    ) -> String {
        let payload: [String: Any] = [
            "ok": true,
            "operation": [
                "requestId": requestId,
                "phase": phase,
                "startedAtMs": 1_786_896_000_000,
                "updatedAtMs": 1_786_896_000_100,
                "canCancel": canCancel,
                "message": message,
                "error": error.map { $0 as Any } ?? NSNull(),
                "recovery": recovery.map { $0 as Any } ?? NSNull(),
                "report": NSNull(),
            ],
        ]
        let data = try! JSONSerialization.data(withJSONObject: payload, options: [.sortedKeys])
        return String(decoding: data, as: UTF8.self)
    }

    private static func lifecycleSecurityPayload(
        installationId: String?,
        canControl: Bool,
        managementTokenGeneration: Int64,
        leaseGeneration: Int64 = 7
    ) -> String {
        let management: String
        if let installationId {
            management = #"{"state":"managed","mode":"managed","canControl":\#(canControl),"installationId":"\#(installationId)","leaseGeneration":\#(leaseGeneration),"leaseExpiresAtMs":4102444800000,"managementTokenGeneration":\#(managementTokenGeneration)}"#
        } else {
            management = #"{"state":"unmanaged","mode":"readOnly","canControl":false,"installationId":null,"leaseGeneration":null,"leaseExpiresAtMs":null,"managementTokenGeneration":\#(managementTokenGeneration)}"#
        }
        return #"{"service":{"service":"threadrelay","apiMajor":1,"ready":true,"instanceId":"fixture-instance","pid":123,"startedAtMs":456},"executable":"/fixture/ThreadRelay","executableSha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","configPath":"/fixture/config.toml","bind":"127.0.0.1:3847","runtime":{"state":"active","productVersion":"0.5.0","buildNumber":388,"apiMajor":1},"protectedWorkItems":{"aiGatewayRequests":0,"codexTurns":0,"imStreams":0,"pendingApprovals":0,"remoteControlRequests":0,"total":0},"management":\#(management)}"#
    }

    private static func requestLogsPageJSON(
        ids: [Int64],
        nextCursor: String? = nil,
        hasMore: Bool? = nil
    ) -> String {
        let logs = ids.map { id in
            #"{"id":\#(id),"requestId":"req-\#(id)","modelId":"model-a","stream":true,"channel":"primary","providerType":"openai_responses","status":"success","createdAtMs":\#(1_754_000_000_000 + id),"createdAt":"2026-08-13T00:00:00Z"}"#
        }
        var payload = #"{"logs":[\#(logs.joined(separator: ","))]"#
        if let nextCursor {
            payload += #", "nextCursor":"\#(nextCursor)""#
        } else if hasMore != nil {
            payload += #", "nextCursor":null"#
        }
        if let hasMore {
            payload += #", "hasMore":\#(hasMore)}"#
        } else {
            payload += "}"
        }
        return payload
    }

    private static func jsonBody(from request: URLRequest) -> [String: Any]? {
        let data: Data?
        if let httpBody = request.httpBody {
            data = httpBody
        } else if let stream = request.httpBodyStream {
            stream.open()
            defer { stream.close() }
            var result = Data()
            var buffer = [UInt8](repeating: 0, count: 4_096)
            while stream.hasBytesAvailable {
                let count = stream.read(&buffer, maxLength: buffer.count)
                guard count > 0 else { break }
                result.append(buffer, count: count)
            }
            data = result
        } else {
            data = nil
        }
        guard let data else { return nil }
        return try? JSONSerialization.jsonObject(with: data) as? [String: Any]
    }

    private func makeClient(
        baseURL: URL = URL(string: "https://threadrelay.test")!,
        credentialLoader: @escaping @Sendable () -> String? = { "fixture-token" },
        handler: @escaping MockURLProtocol.Handler
    ) -> APIClient {
        MockURLProtocol.install(handler)

        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [MockURLProtocol.self]
        let session = URLSession(configuration: configuration)
        return APIClient(
            baseURL: baseURL,
            session: session,
            credentialLoader: {
                credentialLoader().map {
                    [.init(token: $0, expectedInstanceId: nil)]
                } ?? []
            }
        )
    }


    private func makeGUIRecoveryLauncherFixture() throws -> (
        root: URL,
        configuration: GUIRecoveryConfiguration
    ) {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        let executable = root.appendingPathComponent(
            "ThreadRelay.app/Contents/MacOS/ThreadRelay"
        )
        let supervisor = root.appendingPathComponent(
            "ThreadRelay.app/Contents/Helpers/threadrelay-gui-supervisor"
        )
        try FileManager.default.createDirectory(
            at: executable.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        try FileManager.default.createDirectory(
            at: supervisor.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        XCTAssertTrue(FileManager.default.createFile(atPath: executable.path, contents: Data()))
        XCTAssertTrue(FileManager.default.createFile(atPath: supervisor.path, contents: Data()))
        try FileManager.default.setAttributes(
            [.posixPermissions: 0o755],
            ofItemAtPath: executable.path
        )
        try FileManager.default.setAttributes(
            [.posixPermissions: 0o755],
            ofItemAtPath: supervisor.path
        )
        return (
            root,
            GUIRecoveryConfiguration(
                executableURL: executable,
                supervisorURL: supervisor,
                launchAgentURL: root.appendingPathComponent(
                    "home/Library/LaunchAgents/gui.plist"
                ),
                logURL: root.appendingPathComponent("data/logs/gui.log"),
                homeURL: root.appendingPathComponent("home"),
                dataDirectoryURL: root.appendingPathComponent("data"),
                buildIdentifier: "389"
            )
        )
    }

    private func guiRecoveryLaunchAgentPropertyList(at url: URL) throws -> [String: Any] {
        try XCTUnwrap(
            PropertyListSerialization.propertyList(
                from: Data(contentsOf: url),
                options: [],
                format: nil
            ) as? [String: Any]
        )
    }

    private func guiRecoveryLaunchctlOutput(
        configuration: GUIRecoveryConfiguration,
        state: String = "running",
        build: String = "389",
        supervisorURL: URL? = nil,
        arguments: [String]? = nil,
        homeURL: URL? = nil,
        dataDirectoryURL: URL? = nil
    ) -> String {
        let resolvedSupervisor = supervisorURL ?? configuration.supervisorURL
        let resolvedArguments = arguments ?? [resolvedSupervisor.path]
        let argumentLines = resolvedArguments
            .map { "    \($0)" }
            .joined(separator: "\n")
        return """
        state = \(state)
        program = \(resolvedSupervisor.path)
        arguments = {
        \(argumentLines)
        }
        environment = {
            HOME => \((homeURL ?? configuration.homeURL).path)
            THREADRELAY_HOME => \((dataDirectoryURL ?? configuration.dataDirectoryURL).path)
            THREADRELAY_BUNDLE_BUILD => \(build)
        }
        """
    }

    private func makeDaemonLauncherFixture(
        testLaunchdLabel: String? = nil
    ) throws -> (
        root: URL,
        configuration: DaemonLaunchConfiguration
    ) {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        let helper = root.appendingPathComponent("ThreadRelay.app/Contents/Helpers/threadrelay-daemon")
        try FileManager.default.createDirectory(
            at: helper.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        XCTAssertTrue(
            FileManager.default.createFile(
                atPath: helper.path,
                contents: Data("embedded-runtime".utf8)
            )
        )
        try FileManager.default.setAttributes(
            [.posixPermissions: 0o755],
            ofItemAtPath: helper.path
        )
        let launchdLabel = testLaunchdLabel ?? DaemonLaunchConfiguration.label
        let launchAgentURL = root
            .appendingPathComponent("home/Library/LaunchAgents", isDirectory: true)
            .appendingPathComponent("\(launchdLabel).plist")
        let configuration: DaemonLaunchConfiguration
#if DEBUG
        if let testLaunchdLabel {
            configuration = try DaemonLaunchConfiguration(
                testLaunchdLabel: testLaunchdLabel,
                helperURL: helper,
                configURL: root.appendingPathComponent("data/config.toml"),
                launchAgentURL: launchAgentURL,
                logURL: root.appendingPathComponent("data/logs/daemon.log"),
                homeURL: root.appendingPathComponent("home", isDirectory: true),
                buildIdentifier: "389"
            )
        } else {
            configuration = DaemonLaunchConfiguration(
                helperURL: helper,
                configURL: root.appendingPathComponent("data/config.toml"),
                launchAgentURL: launchAgentURL,
                logURL: root.appendingPathComponent("data/logs/daemon.log"),
                homeURL: root.appendingPathComponent("home", isDirectory: true),
                buildIdentifier: "389"
            )
        }
#else
        XCTAssertNil(testLaunchdLabel)
        configuration = DaemonLaunchConfiguration(
            helperURL: helper,
            configURL: root.appendingPathComponent("data/config.toml"),
            launchAgentURL: launchAgentURL,
            logURL: root.appendingPathComponent("data/logs/daemon.log"),
            homeURL: root.appendingPathComponent("home", isDirectory: true),
            buildIdentifier: "389"
        )
#endif
        return (
            root,
            configuration
        )
    }

    private func installDaemonRuntime(
        build: String,
        fixture: (root: URL, configuration: DaemonLaunchConfiguration)
    ) throws -> URL {
        let configuration = DaemonLaunchConfiguration(
            helperURL: fixture.configuration.helperURL,
            configURL: fixture.configuration.configURL,
            launchAgentURL: fixture.configuration.launchAgentURL,
            logURL: fixture.configuration.logURL,
            homeURL: fixture.configuration.homeURL,
            buildIdentifier: build
        )
        let staged = try configuration.stagedHelperURL()
        try FileManager.default.createDirectory(
            at: staged.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        XCTAssertTrue(
            FileManager.default.createFile(
                atPath: staged.path,
                contents: Data("previous-runtime".utf8)
            )
        )
        try FileManager.default.setAttributes(
            [.posixPermissions: 0o755],
            ofItemAtPath: staged.path
        )
        try FileManager.default.createDirectory(
            at: fixture.configuration.launchAgentURL.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        try configuration.propertyListData().write(
            to: fixture.configuration.launchAgentURL,
            options: .atomic
        )
        return staged
    }

    private func daemonLaunchAgentPropertyList(at url: URL) throws -> [String: Any] {
        try XCTUnwrap(
            PropertyListSerialization.propertyList(
                from: Data(contentsOf: url),
                options: [],
                format: nil
            ) as? [String: Any]
        )
    }

    private func launchctlOutput(
        program: URL,
        configuration: DaemonLaunchConfiguration,
        build: String,
        pid: Int32 = 123,
        arguments: [String]? = nil,
        environmentOverrides: [String: String] = [:]
    ) -> String {
        let resolvedArguments = arguments ?? [
            program.path,
            "--config",
            configuration.configURL.path,
            "daemon",
        ]
        let argumentLines = resolvedArguments
            .map { "    \($0)" }
            .joined(separator: "\n")
        var environment = [
            "HOME": configuration.homeURL.path,
            "PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
            "THREADRELAY_HOME": configuration.configURL.deletingLastPathComponent().path,
            "THREADRELAY_BUNDLE_BUILD": build,
        ]
        if configuration.launchdLabel != DaemonLaunchConfiguration.label {
            environment["THREADRELAY_SKIP_DESKTOP_INTEGRATION"] = "1"
        }
        environment.merge(environmentOverrides) { _, override in override }
        let environmentLines = environment.keys.sorted()
            .map { "    \($0) => \(environment[$0] ?? "")" }
            .joined(separator: "\n")
        return """
        state = running
        pid = \(pid)
        program = \(program.path)
        arguments = {
        \(argumentLines)
        }
        environment = {
        \(environmentLines)
        }
        """
    }

    private func lifecycleFixture(
        instanceId: String,
        build: Int,
        executable: String,
        executableSha256: String? = nil,
        configPath: String = "/fixture/config.toml",
        bind: String = "127.0.0.1:3847",
        runtimeState: String = "active"
    ) -> ManageLifecycle {
        ManageLifecycle(
            service: .init(
                service: "threadrelay",
                apiMajor: 1,
                ready: true,
                instanceId: instanceId,
                pid: 123,
                startedAtMs: 456
            ),
            executable: executable,
            executableSha256: executableSha256,
            configPath: configPath,
            bind: bind,
            runtime: .init(
                state: runtimeState,
                productVersion: "0.5.0",
                buildNumber: build,
                apiMajor: 1
            ),
            protectedWorkItems: .init(
                aiGatewayRequests: 0,
                codexTurns: 0,
                imStreams: 0,
                pendingApprovals: 0,
                remoteControlRequests: 0,
                total: 0
            ),
            management: .init(
                state: "managed",
                mode: "managed",
                canControl: true,
                installationId: "fixture-installation",
                leaseGeneration: 1,
                leaseExpiresAtMs: 9_999_999_999
            )
        )
    }

    private func makeClient(
        credentialCandidatesLoader: @escaping @Sendable () -> [ManagementCredentialCandidate],
        handler: @escaping MockURLProtocol.Handler
    ) -> APIClient {
        MockURLProtocol.install(handler)

        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [MockURLProtocol.self]
        let session = URLSession(configuration: configuration)
        return APIClient(
            baseURL: URL(string: "https://threadrelay.test")!,
            session: session,
            credentialLoader: credentialCandidatesLoader
        )
    }

    private func makeClient(
        baseURL: URL,
        baseURLLoader: @escaping @Sendable () -> URL,
        handler: @escaping MockURLProtocol.Handler
    ) -> APIClient {
        MockURLProtocol.install(handler)

        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [MockURLProtocol.self]
        let session = URLSession(configuration: configuration)
        return APIClient(
            baseURL: baseURL,
            session: session,
            credentialLoader: { [.init(token: "fixture-token", expectedInstanceId: nil)] },
            baseURLLoader: baseURLLoader
        )
    }

    private func assertProbeError(
        _ expectedError: APIClientError,
        from client: APIClient,
        file: StaticString = #filePath,
        line: UInt = #line
    ) async {
        do {
            _ = try await client.probe()
            XCTFail("Expected probe to fail", file: file, line: line)
        } catch let error as APIClientError {
            XCTAssertEqual(error, expectedError, file: file, line: line)
        } catch {
            XCTFail("Expected APIClientError, received \(error)", file: file, line: line)
        }
    }

    private func assertDashboardError(
        _ expectedError: APIClientError,
        from client: APIClient,
        file: StaticString = #filePath,
        line: UInt = #line
    ) async {
        do {
            _ = try await client.fetchDashboard(bearerToken: "fixture-token")
            XCTFail("Expected dashboard request to fail", file: file, line: line)
        } catch let error as APIClientError {
            XCTAssertEqual(error, expectedError, file: file, line: line)
        } catch {
            XCTFail("Expected APIClientError, received \(error)", file: file, line: line)
        }
    }

    private func assertLifecycleError(
        _ expectedError: APIClientError,
        from client: APIClient,
        file: StaticString = #filePath,
        line: UInt = #line
    ) async {
        do {
            _ = try await client.fetchLifecycle(bearerToken: "fixture-token")
            XCTFail("Expected lifecycle request to fail", file: file, line: line)
        } catch let error as APIClientError {
            XCTAssertEqual(error, expectedError, file: file, line: line)
        } catch {
            XCTFail("Expected APIClientError, received \(error)", file: file, line: line)
        }
    }


}


private final class IdentityVerifyingDaemonLauncher: DaemonLaunching, @unchecked Sendable {
    private let lock = NSLock()
    private let error: DaemonLaunchError?
    private var verifications = 0

    init(error: DaemonLaunchError? = nil) {
        self.error = error
    }

    var verificationCount: Int {
        lock.withLock { verifications }
    }

    func startIfNeeded() async throws {}

    func verifiedDaemonIdentity(for lifecycle: ManageLifecycle) async throws -> ManageDaemonIdentity {
        lock.withLock { verifications += 1 }
        if let error { throw error }
        return ManageDaemonIdentity(
            pid: lifecycle.service.pid,
            startedAtMs: lifecycle.service.startedAtMs,
            executable: lifecycle.executable,
            executableSha256: lifecycle.executableSha256 ?? "",
            bind: lifecycle.bind
        )
    }
}


private struct MockResponse: Sendable {
    let statusCode: Int
    let body: Data
    let error: URLError?
    let deliveryGate: MockResponseGate?

    init(
        statusCode: Int,
        json: String,
        deliveryGate: MockResponseGate? = nil
    ) {
        self.statusCode = statusCode
        body = Data(json.utf8)
        error = nil
        self.deliveryGate = deliveryGate
    }

    init(error: URLError) {
        statusCode = 0
        body = Data()
        self.error = error
        deliveryGate = nil
    }
}

private final class MockResponseGate: @unchecked Sendable {
    private let lock = NSLock()
    private let semaphore = DispatchSemaphore(value: 0)
    private var timedOut = false

    var didTimeOut: Bool {
        lock.withLock { timedOut }
    }

    func wait() {
        guard semaphore.wait(timeout: .now() + 5) == .timedOut else { return }
        lock.withLock { timedOut = true }
    }

    func open() {
        semaphore.signal()
    }
}

private final class MockResponseSequence: @unchecked Sendable {
    private let lock = NSLock()
    private var responses: [MockResponse]

    init(_ responses: [MockResponse]) {
        self.responses = responses
    }

    func next() -> MockResponse {
        lock.withLock {
            guard responses.count > 1 else {
                return responses[0]
            }
            return responses.removeFirst()
        }
    }
}

private final class StringSequence: @unchecked Sendable {
    private let lock = NSLock()
    private var values: [String]

    init(_ values: [String]) {
        self.values = values
    }

    func next() -> String? {
        lock.withLock {
            guard values.count > 1 else { return values.first }
            return values.removeFirst()
        }
    }
}

private final class IntCounter: @unchecked Sendable {
    private let lock = NSLock()
    private var value = 0

    var current: Int {
        lock.withLock { value }
    }

    func next() -> Int {
        lock.withLock {
            value += 1
            return value
        }
    }
}

private final class LockedValue<Value>: @unchecked Sendable {
    private let lock = NSLock()
    private var storedValue: Value

    init(_ value: Value) {
        storedValue = value
    }

    var value: Value {
        get { lock.withLock { storedValue } }
        set { lock.withLock { storedValue = newValue } }
    }
}

private final class StringRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var recordedValues: [String] = []

    var values: [String] {
        lock.withLock { recordedValues }
    }

    func record(_ value: String) {
        lock.withLock {
            recordedValues.append(value)
        }
    }
}

private final class RequestRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var recordedPaths: [String] = []

    var paths: [String] {
        lock.withLock { recordedPaths }
    }

    func record(_ path: String?) {
        lock.withLock {
            recordedPaths.append(path ?? "")
        }
    }
}

private final class URLRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var recordedURLs: [URL] = []

    var urls: [URL] {
        lock.withLock { recordedURLs }
    }

    func record(_ url: URL?) {
        guard let url else { return }
        lock.withLock {
            recordedURLs.append(url)
        }
    }
}

private final class CommandInvocationRecorder: @unchecked Sendable {
    typealias Handler = @Sendable ([String]) -> CommandResult

    private let lock = NSLock()
    private let handler: Handler
    private var recordedArguments: [[String]] = []
    private var recordedExecutablePaths: [URL] = []

    init(handler: @escaping Handler) {
        self.handler = handler
    }

    var arguments: [[String]] {
        lock.withLock { recordedArguments }
    }

    var executablePaths: [URL] {
        lock.withLock { recordedExecutablePaths }
    }

    func run(_ executable: URL, arguments: [String]) -> CommandResult {
        lock.withLock {
            recordedArguments.append(arguments)
            recordedExecutablePaths.append(executable)
        }
        return handler(arguments)
    }
}

private final class MockURLProtocol: URLProtocol, @unchecked Sendable {
    typealias Handler = @Sendable (URLRequest) -> MockResponse

    private static let lock = NSLock()
    nonisolated(unsafe) private static var installedHandler: Handler?

    static func install(_ handler: @escaping Handler) {
        lock.withLock {
            installedHandler = handler
        }
    }

    static func reset() {
        lock.withLock {
            installedHandler = nil
        }
    }

    override class func canInit(with _: URLRequest) -> Bool {
        true
    }

    override class func canonicalRequest(for request: URLRequest) -> URLRequest {
        request
    }

    override func startLoading() {
        guard let handler = Self.lock.withLock({ Self.installedHandler }),
              let url = request.url
        else {
            client?.urlProtocol(self, didFailWithError: URLError(.badServerResponse))
            return
        }

        let mock = handler(request)
        if let deliveryGate = mock.deliveryGate {
            DispatchQueue.global(qos: .userInitiated).async { [self] in
                deliveryGate.wait()
                deliver(mock, for: url)
            }
            return
        }
        deliver(mock, for: url)
    }

    private func deliver(_ mock: MockResponse, for url: URL) {
        if let error = mock.error {
            client?.urlProtocol(self, didFailWithError: error)
            return
        }
        guard let response = HTTPURLResponse(
                  url: url,
                  statusCode: mock.statusCode,
                  httpVersion: "HTTP/1.1",
                  headerFields: ["Content-Type": "application/json"]
              )
        else {
            client?.urlProtocol(self, didFailWithError: URLError(.badServerResponse))
            return
        }

        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: mock.body)
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}
}
