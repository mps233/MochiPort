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
        } catch SingleInstanceError.alreadyRunning {
            activateExistingInstance()
            NSApplication.shared.terminate(nil)
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
        false
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
                    Task { await model.refresh() }
                }
                .keyboardShortcut("r", modifiers: .command)
            }
        }

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

    var body: some View {
        Text("ThreadRelay")
            .font(.headline)
        Label(model.serviceStatus.title, systemImage: model.serviceStatus.symbol)
        Text(model.serviceStatus.detail)
            .foregroundStyle(.secondary)
        if let lifecycle = model.lifecycle {
            Divider()
            Label(
                lifecycle.management.canControl ? "已托管后台服务" : "只读后台服务",
                systemImage: lifecycle.management.canControl ? "lock.open" : "eye"
            )
            Text("v\(lifecycle.runtime.productVersion) · \(lifecycle.protectedWorkItems.total) 项受保护任务")
                .foregroundStyle(.secondary)
                .font(.caption)
        }
        Divider()
        Button("打开 ThreadRelay") {
            openWindow(id: "main")
        }
        Button("刷新") {
            Task { await model.refresh() }
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
