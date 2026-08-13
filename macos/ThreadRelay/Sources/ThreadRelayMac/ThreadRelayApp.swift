import SwiftUI
import AppKit

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
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
        case "unavailable": .unavailable("Fixture: daemon is offline")
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
                Button("About ThreadRelay") {
                    openAboutWindow()
                }
            }
            CommandGroup(after: .sidebar) {
                Button("Refresh") {
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
            .applicationVersion: Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "Development",
            .version: Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "Local",
            .credits: NSAttributedString(string: "Local-first bridge for controlling coding agents from chat."),
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
                lifecycle.management.canControl ? "Managed daemon" : "Read-only daemon",
                systemImage: lifecycle.management.canControl ? "lock.open" : "eye"
            )
            Text("v\(lifecycle.runtime.productVersion) · \(lifecycle.protectedWorkItems.total) protected work item(s)")
                .foregroundStyle(.secondary)
                .font(.caption)
        }
        Divider()
        Button("Open ThreadRelay") {
            openWindow(id: "main")
        }
        Button("Refresh") {
            Task { await model.refresh() }
        }
        if #available(macOS 14, *) {
            SettingsLink {
                Text("Settings…")
            }
        } else {
            Button("Settings…") {
                NSApplication.shared.sendAction(
                    Selector(("showSettingsWindow:")),
                    to: nil,
                    from: nil
                )
            }
        }
        Divider()
        Button("Quit ThreadRelay") {
            NSApplication.shared.terminate(nil)
        }
        .keyboardShortcut("q")
    }
}
