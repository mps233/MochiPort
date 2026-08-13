import SwiftUI

struct RequestLogsBaselineView: View {
    @State private var query = ""

    private let rows = [
        RequestLogSample(path: "/v1/responses", provider: "Primary", status: "200", duration: "1.24 s"),
        RequestLogSample(path: "/v1/responses", provider: "Fallback", status: "503", duration: "5.03 s"),
        RequestLogSample(path: "/v1/messages", provider: "Primary", status: "200", duration: "840 ms"),
    ]

    var body: some View {
        Table(rows) {
            TableColumn("Endpoint", value: \.path)
            TableColumn("Provider", value: \.provider)
            TableColumn("Status", value: \.status)
                .width(70)
            TableColumn("Duration", value: \.duration)
                .width(90)
        }
        .searchable(text: $query, prompt: "Search requests")
        .accessibilityIdentifier("request-logs.table")
        .overlay(alignment: .bottomTrailing) {
            FloatingControlSurface {
                HStack(spacing: ThreadRelaySpacing.standard) {
                    Label("3 requests", systemImage: "list.bullet.rectangle")
                    Divider()
                        .frame(height: 18)
                    Button("Clear", role: .destructive) {}
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

struct OnboardingBaselineView: View {
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: ThreadRelaySpacing.section) {
            VStack(alignment: .leading, spacing: ThreadRelaySpacing.compact) {
                Image(systemName: "message.badge.waveform")
                    .font(.system(size: 28))
                    .foregroundStyle(.tint)
                Text("Connect a messaging channel")
                    .font(.title2.weight(.semibold))
                Text("Account onboarding will be connected in Phase 3. This sheet establishes the native overlay hierarchy and accessibility fallback.")
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer()
            HStack {
                Spacer()
                Button("Close", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button("Continue") { dismiss() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(true)
            }
        }
        .padding(ThreadRelaySpacing.page)
        .frame(width: 520, height: 300)
    }
}

#if DEBUG
#Preview("Overview - Available") {
    RootView()
        .environmentObject(AppModel(fixtureStatus: .available))
        .frame(width: 1040, height: 700)
}

#Preview("Overview - Service unavailable") {
    RootView()
        .environmentObject(AppModel(fixtureStatus: .unavailable("Fixture: daemon is offline")))
        .frame(width: 1040, height: 700)
}

#Preview("Request logs") {
    RequestLogsBaselineView()
        .frame(width: 860, height: 520)
}

#Preview("Messaging onboarding") {
    OnboardingBaselineView()
}
#endif
