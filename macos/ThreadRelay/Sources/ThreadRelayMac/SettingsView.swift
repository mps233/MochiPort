import SwiftUI

struct SettingsView: View {
    @AppStorage("closeBehavior") private var closeBehavior = "menuBar"

    var body: some View {
        TabView {
            Form {
                Section("Window") {
                    Picker("Closing the main window", selection: $closeBehavior) {
                        Text("Hide to Menu Bar").tag("menuBar")
                        Text("Quit the Interface").tag("quitGUI")
                    }
                    Text("The local service continues running in both modes.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            .padding(20)
            .tabItem { Label("General", systemImage: "gearshape") }

            SettingsPlaceholder(title: "Network", symbol: "network")
                .tabItem { Label("Network", systemImage: "network") }

            SettingsPlaceholder(title: "Local Service", symbol: "server.rack")
                .tabItem { Label("Local Service", systemImage: "server.rack") }

            SettingsPlaceholder(title: "Update & Diagnostics", symbol: "stethoscope")
                .tabItem { Label("Update & Diagnostics", systemImage: "stethoscope") }
        }
        .frame(width: 620, height: 360)
    }
}

private struct SettingsPlaceholder: View {
    let title: String
    let symbol: String

    var body: some View {
        EmptyStateView(title: title, message: nil, symbol: symbol)
    }
}
