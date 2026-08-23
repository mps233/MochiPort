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
            "MochiPort 已在运行。"
        case let .directoryUnavailable(path):
            "无法准备 MochiPort 单实例锁目录：\(path)"
        case let .openFailed(error):
            "无法打开 MochiPort 单实例锁（错误码 \(error)）。"
        case let .lockFailed(error):
            "无法获取 MochiPort 单实例锁（错误码 \(error)）。"
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
        for key in ["MOCHIPORT_GUI_LOCK_PATH", "THREADRELAY_GUI_LOCK_PATH"] {
            if let override = environment[key], !override.isEmpty {
                return URL(fileURLWithPath: override)
            }
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
                NSLog("MochiPort GUI 自动恢复注册失败：%@", error.localizedDescription)
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
                        NSLog("MochiPort GUI 自动恢复重试失败：%@", error.localizedDescription)
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
            NSLog("MochiPort GUI 单实例保护不可用：%@", error.localizedDescription)
            let alert = NSAlert()
            alert.alertStyle = .critical
            alert.messageText = "MochiPort 无法启动"
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
            NSLog("MochiPort GUI 正常退出标记写入失败：%@", error.localizedDescription)
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
            NSLog("MochiPort 旧的正常退出标记清理失败：%@", error.localizedDescription)
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
    @StateObject private var glass: AIGlassCoordinator

    init() {
        _model = StateObject(wrappedValue: AppModel(fixtureStatus: Self.fixtureStatusFromEnvironment()))
        _glass = StateObject(wrappedValue: AIGlassCoordinator())
    }

    private static func fixtureStatusFromEnvironment() -> ServiceStatus? {
        let environment = ProcessInfo.processInfo.environment
        let fixture = environment["MOCHIPORT_PREVIEW_FIXTURE"]
            ?? environment["THREADRELAY_PREVIEW_FIXTURE"]
        switch fixture {
        case "available": return .available
        case "bridge": return .bridgeAvailable
        case "unavailable": return .unavailable("预览：后台服务已离线")
        default: return nil
        }
    }

    var body: some Scene {
        Window("MochiPort", id: "main") {
            RootView()
                .environmentObject(model)
                .environmentObject(glass)
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
                Button("关于 MochiPort") {
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

        MenuBarExtra {
            MenuBarStatusView(glass: glass, model: model)
        } label: {
            MenuBarStatusLabel(status: model.serviceStatus,
                               glass: glass,
                               settings: glass.settings)
        }
        .menuBarExtraStyle(.window)

        Settings {
            SettingsView()
                .environmentObject(model)
                .environmentObject(glass)
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
            .applicationName: "MochiPort",
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

private struct MenuBarStatusLabel: View {
    let status: ServiceStatus
    @ObservedObject var glass: AIGlassCoordinator
    @Bindable var settings: AppSettings

    private var tint: Color {
        switch status {
        case .checking: .secondary
        case .available: .green
        case .bridgeAvailable: .orange
        case .unavailable: .red
        }
    }

    private var title: String {
        switch status {
        case .checking: "连接中"
        case .available: "在线"
        case .bridgeAvailable: "兼容"
        case .unavailable: "离线"
        }
    }

    var body: some View {
        // Use the concrete Label<Text, Image> form supported by MenuBarExtra.
        // A free-form HStack or custom text icon can be measured as an
        // icon-only status item and silently drop the numeric title.
        Label(displayText, systemImage: "sparkles")
        .labelStyle(.titleAndIcon)
        .font(.system(size: 11, weight: .medium).monospacedDigit())
        .fixedSize(horizontal: true, vertical: false)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("MochiPort \(displayText) · " + title)
        .help("MochiPort：" + status.title)
    }

    private var displayText: String {
        let text = displayItems.map(value(for:)).joined(separator: " ")
        return text.isEmpty ? "—" : text
    }

    private func formatTokens(_ value: Int) -> String {
        switch value {
        case 1_000_000...: String(format: "%.1fM", Double(value) / 1_000_000)
        case 1_000...: String(format: "%.0fK", Double(value) / 1_000)
        default: String(value)
        }
    }

    private var displayItems: [MenubarItem] {
        MenubarItem.ordered(settings.menubarItems)
    }

    private func value(for item: MenubarItem) -> String {
        let now = Date()
        switch item {
        case .todayTokens:
            return formatTokens(glass.store.todayTokens(now: now))
        case .burnRate:
            return formatRate(glass.store.tokensPerMinute(windowMinutes: 3, now: now)) + "/m"
        case .usagePercent:
            return Theme.formatUsagePercent(glass.store.maxUsedPercent)
        case .resetCountdown:
            return nearestResetCountdown(now: now) ?? "—"
        }
    }

    private func formatRate(_ value: Double) -> String {
        switch value {
        case 1_000_000...: return String(format: "%.1fM", value / 1_000_000)
        case 1_000...: return String(format: "%.0fK", value / 1_000)
        default: return String(Int(value.rounded()))
        }
    }

    private func nearestResetCountdown(now: Date) -> String? {
        let dates = glass.store.limits.values
            .flatMap { $0 }
            .compactMap(\.resetsAt)
            .filter { $0 > now }
        guard let nearest = dates.min() else { return nil }
        return EventEngine.countdown(to: nearest, from: now)
    }
}

/// The menu bar uses ai-glass's real dashboard implementation. HUD-specific
/// panels and hotkeys are intentionally absent; history and notifications stay
/// available through the coordinator.
private struct MenuBarStatusView: View {
    @ObservedObject var glass: AIGlassCoordinator
    @ObservedObject var model: AppModel

    var body: some View {
        DashboardView(
            store: glass.store,
            statsStore: glass.statsStore,
            settings: glass.settings,
            providerUsage: model.gatewayProviderUsage,
            providerChannel: model.gatewayProviderChannel,
            eventLog: glass.eventLog,
            updateState: glass.updateState,
            onSettings: openSettings)
            .padding(8)
            .modifier(MenuBarWindowBackgroundModifier())
            .task {
                while !Task.isCancelled {
                    await model.refreshGatewayProviderUsage()
                    try? await Task.sleep(for: .seconds(60))
                }
            }
    }

    private func openSettings() {
        NSApplication.shared.sendAction(
            Selector(("showSettingsWindow:")), to: nil, from: nil)
    }
}

private struct MenuBarWindowBackgroundModifier: ViewModifier {
    func body(content: Content) -> some View {
        if #available(macOS 15.0, *) {
            content.containerBackground(.clear, for: .window)
        } else {
            content
        }
    }
}
