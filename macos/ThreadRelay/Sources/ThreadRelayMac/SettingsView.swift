import SwiftUI

struct SettingsView: View {
    @AppStorage("closeBehavior") private var closeBehavior = "menuBar"

    var body: some View {
        TabView {
            Form {
                Section("窗口") {
                    Picker("关闭主窗口时", selection: $closeBehavior) {
                        Text("隐藏到菜单栏").tag("menuBar")
                        Text("退出界面").tag("quitGUI")
                    }
                    Text("无论选择哪种方式，本地服务都会继续运行。")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            .padding(20)
            .tabItem { Label("通用", systemImage: "gearshape") }

            SettingsPlaceholder(title: "网络", symbol: "network")
                .tabItem { Label("网络", systemImage: "network") }

            SettingsPlaceholder(title: "本地服务", symbol: "server.rack")
                .tabItem { Label("本地服务", systemImage: "server.rack") }

            SettingsPlaceholder(title: "更新与诊断", symbol: "stethoscope")
                .tabItem { Label("更新与诊断", systemImage: "stethoscope") }
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
