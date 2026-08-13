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
        WindowGroup(id: "main") {
            RootView()
                .environmentObject(model)
                .frame(minWidth: 760, minHeight: 540)
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

private struct MenuBarStatusView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        Text("ThreadRelay")
            .font(.headline)
        Label(model.serviceStatus.title, systemImage: model.serviceStatus.symbol)
        Text(model.serviceStatus.detail)
            .foregroundStyle(.secondary)
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
