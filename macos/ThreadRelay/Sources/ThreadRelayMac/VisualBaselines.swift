import SwiftUI

struct RequestLogsBaselineView: View {
    @State private var query = ""

    private let rows = [
        RequestLogSample(path: "/v1/responses", provider: "主要", status: "200", duration: "1.24 秒"),
        RequestLogSample(path: "/v1/responses", provider: "备用", status: "503", duration: "5.03 秒"),
        RequestLogSample(path: "/v1/messages", provider: "主要", status: "200", duration: "840 毫秒"),
    ]

    var body: some View {
        Table(rows) {
            TableColumn("端点", value: \.path)
            TableColumn("供应商", value: \.provider)
            TableColumn("状态", value: \.status)
                .width(70)
            TableColumn("耗时", value: \.duration)
                .width(90)
        }
        .searchable(text: $query, prompt: "搜索请求")
        .accessibilityIdentifier("request-logs.table")
        .overlay(alignment: .bottomTrailing) {
            FloatingControlSurface {
                HStack(spacing: ThreadRelaySpacing.standard) {
                    Label("3 条请求", systemImage: "list.bullet.rectangle")
                    Divider()
                        .frame(height: 18)
                    Button("清除", role: .destructive) {}
                }
            }
            .padding()
        }
    }
}

private struct RequestLogSample: Identifiable {
    let id = UUID()
    let path: String
    let provider: String
    let status: String
    let duration: String
}

#if DEBUG
#Preview("概览 - 可用") {
    RootView()
        .environmentObject(AppModel(fixtureStatus: .available))
        .frame(width: 1040, height: 700)
}

#Preview("概览 - 服务不可用") {
    RootView()
        .environmentObject(AppModel(fixtureStatus: .unavailable("预览：后台服务已离线")))
        .frame(width: 1040, height: 700)
}

#Preview("请求日志") {
    RequestLogsBaselineView()
        .frame(width: 860, height: 520)
}
#endif
