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
        if let override = environment["MOCHIPORT_GUI_LOCK_PATH"], !override.isEmpty {
            return URL(fileURLWithPath: override)
        }

        let home = environment["HOME"]
            .map { URL(fileURLWithPath: $0, isDirectory: true) }
            ?? fileManager.homeDirectoryForCurrentUser
        let identifier = (bundleIdentifier ?? "io.github.mps233.mochiport")
            .replacingOccurrences(of: "/", with: "-")
        return home
            .appendingPathComponent("Library/Application Support/MochiPort", isDirectory: true)
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

    private func activateExistingInstance() {
        guard let bundleIdentifier = Bundle.main.bundleIdentifier else { return }
        let currentPID = ProcessInfo.processInfo.processIdentifier
        let existing = NSRunningApplication.runningApplications(withBundleIdentifier: bundleIdentifier)
            .first { $0.processIdentifier != currentPID }
        existing?.activate(options: [.activateAllWindows])
    }
}

@main
struct MochiPortApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @StateObject private var model: AppModel
    @StateObject private var menubar: MenubarCoordinator

    init() {
        let appModel = AppModel(fixtureStatus: Self.fixtureStatusFromEnvironment())
        _model = StateObject(wrappedValue: appModel)
        _menubar = StateObject(wrappedValue: MenubarCoordinator())

        // SwiftUI may defer both the main window and the menu-bar content.
        // Start the first daemon probe from the App lifecycle itself so a
        // menu-bar-only launch can still register a missing backend.
        Task { @MainActor in
            await appModel.startAtAppLaunch()
        }
    }

    private static func fixtureStatusFromEnvironment() -> ServiceStatus? {
        let environment = ProcessInfo.processInfo.environment
        let fixture = environment["MOCHIPORT_PREVIEW_FIXTURE"]
        switch fixture {
        case "available": return .available
        case "unavailable": return .unavailable("预览：后台服务已离线")
        default: return nil
        }
    }

    var body: some Scene {
        Window("MochiPort", id: "main") {
            RootView()
                .environmentObject(model)
                .environmentObject(menubar)
                .preferredColorScheme(preferredColorScheme)
                .frame(minWidth: 760, minHeight: 540)
                .background(WindowVisibilityObserver { visible in
                    model.setWindowVisible(visible)
                })
                .background(WindowFrameRestorationGuard())
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
            MenuBarStatusView(menubar: menubar, model: model)
        } label: {
            MenuBarStatusLabel(model: model,
                               status: model.serviceStatus,
                               menubar: menubar,
                               settings: menubar.settings)
        }
        .menuBarExtraStyle(.window)

        Settings {
            SettingsView()
                .environmentObject(model)
                .environmentObject(menubar)
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

/// 窗口恢复位置的纯几何判断。macOS 会把上次关闭时的窗口位置原样恢复；
/// 当用户接过外接屏或模拟器虚拟屏（如 MuMu）后，保存的位置可能落在
/// 看不见的屏幕区域，App 看起来就像没有打开。
enum WindowFrameRestoration {
    /// 窗口至少要有这一比例的面积落在真实屏幕的可视区域内，否则视为
    /// 「恢复到了不可见的屏幕」，需要拉回主屏。
    static let minimumVisibleFraction: CGFloat = 0.2

    /// 返回 nil 表示窗口位置可以保留；否则返回应移动到的原点。
    /// screens 为各屏幕的可视区域，第一项按 AppKit 约定是主屏幕。
    static func relocatedOrigin(frame: CGRect, screens: [CGRect]) -> CGPoint? {
        guard !screens.isEmpty else { return nil }
        let frameArea = frame.width * frame.height
        guard frameArea > 0 else { return nil }

        var visibleArea: CGFloat = 0
        var bestScreen = screens[0]
        var bestOverlap: CGFloat = 0
        for screen in screens {
            let intersection = screen.intersection(frame)
            let area = intersection.isNull ? 0 : intersection.width * intersection.height
            visibleArea += area
            if area > bestOverlap {
                bestOverlap = area
                bestScreen = screen
            }
        }
        guard visibleArea / frameArea < minimumVisibleFraction else { return nil }

        // 拉回重叠最多的屏幕并居中；窗口比屏幕大时按可视区左上角对齐。
        return CGPoint(
            x: max(bestScreen.minX, min(bestScreen.midX - frame.width / 2, bestScreen.maxX - frame.width)),
            y: max(bestScreen.minY, min(bestScreen.midY - frame.height / 2, bestScreen.maxY - frame.height))
        )
    }
}

/// 窗口首次可见时校验恢复出来的位置；落不到真实屏幕上就拉回主屏。
private struct WindowFrameRestorationGuard: NSViewRepresentable {
    func makeCoordinator() -> Coordinator {
        Coordinator()
    }

    func makeNSView(context: Context) -> NSView {
        NSView()
    }

    func updateNSView(_ view: NSView, context: Context) {
        context.coordinator.attach(to: view)
    }

    static func dismantleNSView(_ view: NSView, coordinator: Coordinator) {
        coordinator.dismantle()
    }

    @MainActor
    final class Coordinator: @unchecked Sendable {
        private var observations: [NSObjectProtocol] = []
        private weak var window: NSWindow?
        private var validated = false

        func attach(to view: NSView) {
            Task { @MainActor [weak self, weak view] in
                guard let self, let window = view?.window, self.window !== window else { return }
                clear()
                self.window = window
                observations.append(
                    NotificationCenter.default.addObserver(
                        forName: NSWindow.didChangeOcclusionStateNotification,
                        object: window,
                        queue: .main
                    ) { [weak self] _ in
                        Task { @MainActor in
                            self?.validateIfNeeded()
                        }
                    }
                )
                validateIfNeeded()
            }
        }

        func dismantle() {
            clear()
            window = nil
        }

        private func clear() {
            observations.forEach(NotificationCenter.default.removeObserver)
            observations.removeAll()
        }

        private func validateIfNeeded() {
            guard !validated, let window, window.isVisible else { return }
            validated = true
            clear()
            guard !window.styleMask.contains(.fullScreen) else { return }
            guard let origin = WindowFrameRestoration.relocatedOrigin(
                frame: window.frame,
                screens: NSScreen.screens.map(\.visibleFrame)
            ) else { return }
            window.setFrameOrigin(origin)
        }
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
    @ObservedObject var model: AppModel
    let status: ServiceStatus
    @ObservedObject var menubar: MenubarCoordinator
    @Bindable var settings: AppSettings

    private var tint: Color {
        switch status {
        case .checking: .secondary
        case .available: .green
        case .unavailable: .red
        }
    }

    private var title: String {
        switch status {
        case .checking: "连接中"
        case .available: "在线"
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
        .task {
            await model.startAtAppLaunch()
        }
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
            return formatTokens(menubar.store.todayTokens(now: now))
        case .burnRate:
            // store 的度量是 token/分钟，除以 60 得到每秒。
            return formatRate(menubar.store.tokensPerMinute(windowMinutes: 3, now: now) / 60) + "/s"
        case .usagePercent:
            return Theme.formatUsagePercent(menubar.store.maxUsedPercent)
        case .resetCountdown:
            return nearestResetCountdown(now: now) ?? "—"
        }
    }

    /// 每秒速率：数值通常很小，保留一位小数才看得出变化。
    private func formatRate(_ value: Double) -> String {
        guard value.isFinite else { return "0" }
        let v = max(0, value)
        switch v {
        case 1_000_000...: return String(format: "%.1fM", v / 1_000_000)
        case 1_000...: return String(format: "%.1fK", v / 1_000)
        case 100...: return String(format: "%.0f", v)
        default: return String(format: "%.1f", v)
        }
    }

    private func nearestResetCountdown(now: Date) -> String? {
        let dates = menubar.store.limits.values
            .flatMap { $0 }
            .compactMap(\.resetsAt)
            .filter { $0 > now }
        guard let nearest = dates.min() else { return nil }
        return EventEngine.countdown(to: nearest, from: now)
    }
}

/// The menu bar uses ai-menubar's real dashboard implementation. HUD-specific
/// panels and hotkeys are intentionally absent; history and notifications stay
/// available through the coordinator.
private struct MenuBarStatusView: View {
    @ObservedObject var menubar: MenubarCoordinator
    @ObservedObject var model: AppModel

    var body: some View {
        DashboardView(
            store: menubar.store,
            statsStore: menubar.statsStore,
            settings: menubar.settings,
            providerUsage: model.gatewayProviderUsage,
            providerChannel: model.gatewayProviderChannel,
            eventLog: menubar.eventLog,
            updateState: menubar.updateState,
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
