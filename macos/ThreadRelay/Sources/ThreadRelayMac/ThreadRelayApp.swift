import SwiftUI
import AppKit
import Darwin

enum SingleInstanceError: LocalizedError, Equatable {
    case alreadyRunning
    case directoryUnavailable(String)
    case openFailed(Int32)
    case lockFailed(Int32)

    var errorDescription: String? {
        switch self {
        case .alreadyRunning:
            "ThreadRelay 已在运行。"
        case let .directoryUnavailable(path):
            "无法准备 ThreadRelay 单实例锁目录：\(path)"
        case let .openFailed(error):
            "无法打开 ThreadRelay 单实例锁（错误码 \(error)）。"
        case let .lockFailed(error):
            "无法获取 ThreadRelay 单实例锁（错误码 \(error)）。"
        }
    }
}

/// An advisory lock held for the lifetime of the GUI process.  Launch Services
/// prevents normal double-click launches, while this guard also covers direct
/// executable launches and two launches racing before Launch Services settles.
final class SingleInstanceGuard: @unchecked Sendable {
    private let descriptor: Int32

    private init(descriptor: Int32) {
        self.descriptor = descriptor
    }

    deinit {
        _ = flock(descriptor, LOCK_UN)
        _ = close(descriptor)
    }

    static func acquire(
        lockURL: URL,
        fileManager: FileManager = .default
    ) throws -> SingleInstanceGuard {
        let directory = lockURL.deletingLastPathComponent()
        do {
            try fileManager.createDirectory(
                at: directory,
                withIntermediateDirectories: true,
                attributes: [FileAttributeKey.posixPermissions: 0o700]
            )
        } catch {
            throw SingleInstanceError.directoryUnavailable(directory.path)
        }

        let descriptor = Darwin.open(
            lockURL.path,
            O_CREAT | O_RDWR,
            mode_t(0o600)
        )
        guard descriptor >= 0 else {
            throw SingleInstanceError.openFailed(errno)
        }

        guard flock(descriptor, LOCK_EX | LOCK_NB) == 0 else {
            let error = errno
            _ = close(descriptor)
            if error == EWOULDBLOCK || error == EAGAIN {
                throw SingleInstanceError.alreadyRunning
            }
            throw SingleInstanceError.lockFailed(error)
        }

        // Keep the lock file private even when an old file was created with
        // broader permissions by a previous build.
        _ = Darwin.fchmod(descriptor, mode_t(0o600))
        return SingleInstanceGuard(descriptor: descriptor)
    }

    static func defaultLockURL(
        bundleIdentifier: String? = Bundle.main.bundleIdentifier,
        environment: [String: String] = ProcessInfo.processInfo.environment,
        fileManager: FileManager = .default
    ) -> URL {
        if let override = environment["THREADRELAY_GUI_LOCK_PATH"], !override.isEmpty {
            return URL(fileURLWithPath: override)
        }

        let home = environment["HOME"]
            .map { URL(fileURLWithPath: $0, isDirectory: true) }
            ?? fileManager.homeDirectoryForCurrentUser
        let identifier = (bundleIdentifier ?? "io.github.mps233.threadrelay")
            .replacingOccurrences(of: "/", with: "-")
        return home
            .appendingPathComponent("Library/Application Support/ThreadRelay", isDirectory: true)
            .appendingPathComponent("\(identifier).gui.lock")
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var instanceGuard: SingleInstanceGuard?

    func applicationWillFinishLaunching(_ notification: Notification) {
        do {
            instanceGuard = try SingleInstanceGuard.acquire(
                lockURL: SingleInstanceGuard.defaultLockURL()
            )
            do {
                try GUIRecoveryLauncher().startIfNeeded()
                clearNormalExitMarker()
            } catch {
                NSLog("ThreadRelay GUI 自动恢复注册失败：%@", error.localizedDescription)
                // Keep the previous marker until launchd registration succeeds;
                // otherwise a manual launch during a transient launchctl error
                // could disable recovery for the next crash. Retry once while
                // the app is already running.
                Task { [weak self] in
                    try? await Task.sleep(for: .seconds(2))
                    guard let self else { return }
                    do {
                        try GUIRecoveryLauncher().startIfNeeded()
                        clearNormalExitMarker()
                    } catch {
                        NSLog("ThreadRelay GUI 自动恢复重试失败：%@", error.localizedDescription)
                    }
                }
            }
        } catch SingleInstanceError.alreadyRunning {
            activateExistingInstance()
            Darwin.exit(73)
        } catch {
            // Fail closed: running without the lock can create duplicate
            // refresh loops and conflicting window state. Without a visible
            // alert the failure would look like the app silently not opening.
            NSLog("ThreadRelay GUI 单实例保护不可用：%@", error.localizedDescription)
            let alert = NSAlert()
            alert.alertStyle = .critical
            alert.messageText = "ThreadRelay 无法启动"
            alert.informativeText = error.localizedDescription
            alert.addButton(withTitle: "退出")
            alert.runModal()
            NSApplication.shared.terminate(nil)
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        UserDefaults.standard.string(forKey: "closeBehavior") == "quitGUI"
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        do {
            let configuration = try GUIRecoveryConfiguration.current()
            let markerDirectory = configuration.dataDirectoryURL
            try FileManager.default.createDirectory(
                at: markerDirectory,
                withIntermediateDirectories: true
            )
            let markerURL = markerDirectory.appendingPathComponent("gui-normal-exit.marker")
            // The marker must not suppress recovery after an app update. The
            // supervisor compares this value with the active bundle's
            // CFBundleVersion before honoring a normal quit. Data.atomic also
            // prevents a partially-written build number from being accepted.
            try Data(currentBuildIdentifier().utf8).write(to: markerURL, options: .atomic)
        } catch {
            NSLog("ThreadRelay GUI 正常退出标记写入失败：%@", error.localizedDescription)
        }
        return .terminateNow
    }

    private func currentBuildIdentifier() -> String {
        let build = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String
        let trimmed = build?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return trimmed.isEmpty ? "unknown" : trimmed
    }

    private func clearNormalExitMarker() {
        do {
            let configuration = try GUIRecoveryConfiguration.current()
            let markerURL = configuration.dataDirectoryURL
                .appendingPathComponent("gui-normal-exit.marker")
            try FileManager.default.removeItem(at: markerURL)
        } catch CocoaError.fileNoSuchFile {
            // A marker is only present after a normal quit; there is nothing
            // to clear on the first launch or after an unexpected exit.
        } catch {
            NSLog("ThreadRelay 旧的正常退出标记清理失败：%@", error.localizedDescription)
        }
    }

    private func activateExistingInstance() {
        guard let bundleIdentifier = Bundle.main.bundleIdentifier else { return }
        let currentPID = ProcessInfo.processInfo.processIdentifier
        let existing = NSRunningApplication.runningApplications(withBundleIdentifier: bundleIdentifier)
            .first { $0.processIdentifier != currentPID }
        existing?.activate(options: [.activateAllWindows, .activateIgnoringOtherApps])
    }
}

@main
struct ThreadRelayApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @StateObject private var model: AppModel

    init() {
        _model = StateObject(wrappedValue: AppModel(fixtureStatus: Self.fixtureStatusFromEnvironment()))
    }

    private static func fixtureStatusFromEnvironment() -> ServiceStatus? {
        switch ProcessInfo.processInfo.environment["THREADRELAY_PREVIEW_FIXTURE"] {
        case "available": .available
        case "bridge": .bridgeAvailable
        case "unavailable": .unavailable("预览：后台服务已离线")
        default: nil
        }
    }

    var body: some Scene {
        Window("ThreadRelay", id: "main") {
            RootView()
                .environmentObject(model)
                .preferredColorScheme(preferredColorScheme)
                .frame(minWidth: 760, minHeight: 540)
                .background(WindowVisibilityObserver { visible in
                    model.setWindowVisible(visible)
                })
        }
        .defaultSize(width: 1040, height: 700)
        .commands {
            SidebarCommands()
            CommandGroup(replacing: .appInfo) {
                Button("关于 ThreadRelay") {
                    openAboutWindow()
                }
            }
            CommandGroup(after: .sidebar) {
                Button("刷新") {
                    Task {
                        await model.refresh()
                        if let selection = model.selection,
                           selection != .overview,
                           selection != .messaging {
                            await model.loadSection(selection, force: true)
                        }
                    }
                }
                .keyboardShortcut("r", modifiers: .command)
            }
        }

        WindowGroup("请求日志详情", for: Int64.self) { $logID in
            if let logID {
                RequestLogDetailWindow(logID: logID)
                    .environmentObject(model)
                    .preferredColorScheme(preferredColorScheme)
            }
        }
        .defaultSize(width: 760, height: 620)

        MenuBarExtra {
            MenuBarStatusView()
                .environmentObject(model)
        } label: {
            Image(systemName: model.serviceStatus.symbol)
                .symbolRenderingMode(.hierarchical)
        }
        .menuBarExtraStyle(.menu)

        Settings {
            SettingsView()
                .environmentObject(model)
                .preferredColorScheme(preferredColorScheme)
        }
    }

    private var preferredColorScheme: ColorScheme? {
        switch model.settings?.theme {
        case "light": .light
        case "dark": .dark
        default: nil
        }
    }

    private func openAboutWindow() {
        NSApplication.shared.orderFrontStandardAboutPanel(options: [
            .applicationName: "ThreadRelay",
            .applicationVersion: Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "开发版",
            .version: Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "本地构建",
            .credits: NSAttributedString(string: "通过聊天远程控制编程智能体的本地优先桥接工具。"),
        ])
        NSApplication.shared.activate(ignoringOtherApps: true)
    }
}

private struct WindowVisibilityObserver: NSViewRepresentable {
    let onChange: @MainActor (Bool) -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(onChange: onChange)
    }

    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        context.coordinator.attach(to: view)
        return view
    }

    func updateNSView(_ view: NSView, context: Context) {
        context.coordinator.attach(to: view)
    }

    @MainActor
    final class Coordinator: @unchecked Sendable {
        private let onChange: @MainActor (Bool) -> Void
        private var observations: [NSObjectProtocol] = []
        private weak var window: NSWindow?

        init(onChange: @escaping @MainActor (Bool) -> Void) {
            self.onChange = onChange
        }

        func attach(to view: NSView) {
            Task { @MainActor [weak self, weak view] in
                guard let self, let window = view?.window, self.window !== window else { return }
                self.clearObservations()
                self.window = window
                let center = NotificationCenter.default
                for name in [
                    NSWindow.didChangeOcclusionStateNotification,
                    NSWindow.didMiniaturizeNotification,
                    NSWindow.didDeminiaturizeNotification,
                ] {
                    self.observations.append(center.addObserver(forName: name, object: window, queue: .main) { [weak self] _ in
                        Task { @MainActor in
                            self?.publishVisibility()
                        }
                    })
                }
                self.observations.append(
                    center.addObserver(
                        forName: NSWindow.willCloseNotification,
                        object: window,
                        queue: .main
                    ) { [weak self] _ in
                        Task { @MainActor in
                            self?.onChange(false)
                        }
                    }
                )
                for name in [
                    NSApplication.didBecomeActiveNotification,
                    NSApplication.didResignActiveNotification,
                ] {
                    self.observations.append(center.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                        Task { @MainActor in
                            self?.publishVisibility()
                        }
                    })
                }
                self.publishVisibility()
            }
        }

        func dismantle() {
            onChange(false)
            clearObservations()
            window = nil
        }

        private func clearObservations() {
            observations.forEach(NotificationCenter.default.removeObserver)
            observations.removeAll()
        }

        private func publishVisibility() {
            guard let window else { return }
            let visible = window.isVisible
                && !window.isMiniaturized
                && window.occlusionState.contains(.visible)
                && NSApplication.shared.isActive
            onChange(visible)
        }
    }

    static func dismantleNSView(_ view: NSView, coordinator: Coordinator) {
        coordinator.dismantle()
    }
}

private struct MenuBarStatusView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.openWindow) private var openWindow
    @Environment(\.openURL) private var openURL

    private var serviceUnavailable: Bool {
        if case .unavailable = model.serviceStatus { return true }
        return false
    }

    var body: some View {
        Text("ThreadRelay")
            .font(.headline)
        Label(model.serviceStatus.title, systemImage: model.serviceStatus.symbol)
        Text(model.serviceStatus.detail)
            .foregroundStyle(.secondary)
        if let lifecycle = model.lifecycle {
            Divider()
            Label(
                model.ownsDaemonLease
                    ? "已托管后台服务"
                    : model.daemonLeaseConflict ? "其他安装已托管" : "仅查看后台服务",
                systemImage: model.ownsDaemonLease ? "lock.open" : "eye"
            )
            let build = lifecycle.runtime.buildNumber.map { " · 构建 \($0)" } ?? ""
            Text("v\(lifecycle.runtime.productVersion)\(build) · \(lifecycle.protectedWorkItems.total) 项受保护任务")
                .foregroundStyle(.secondary)
                .font(.caption)
            if model.daemonBuildMismatch {
                Label("界面与后台服务构建不一致", systemImage: "exclamationmark.triangle")
                    .foregroundStyle(.orange)
                    .font(.caption)
            }
        }
        Divider()
        Button("打开 ThreadRelay") {
            openWindow(id: "main")
        }
        Button("刷新") {
            Task { await model.refresh() }
        }
        Button(model.daemonRecoveryInProgress ? "正在启动本地服务…" : "启动本地服务") {
            Task { await model.startDaemonManually() }
        }
        .disabled(!serviceUnavailable || model.daemonRecoveryInProgress)
        if let update = model.availableUpdate {
            Button("下载新版本 \(update.version)") {
                openURL(update.url)
            }
            .accessibilityLabel("下载新版本 \(update.version)")
        }
        if #available(macOS 14, *) {
            SettingsLink {
                Text("设置…")
            }
        } else {
            Button("设置…") {
                NSApplication.shared.sendAction(
                    Selector(("showSettingsWindow:")),
                    to: nil,
                    from: nil
                )
            }
        }
        Divider()
        Button("退出 ThreadRelay") {
            NSApplication.shared.terminate(nil)
        }
        .keyboardShortcut("q")
    }
}
