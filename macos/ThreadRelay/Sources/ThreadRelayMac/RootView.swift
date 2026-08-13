import SwiftUI

struct RootView: View {
    @EnvironmentObject private var model: AppModel
    @State private var showsOnboardingBaseline = false

    var body: some View {
        NavigationSplitView {
            List(selection: $model.selection) {
                ForEach(AppSectionGroup.allCases) { group in
                    if let title = group.title {
                        Section(title) {
                            sectionRows(group.sections)
                        }
                    } else {
                        sectionRows(group.sections)
                    }
                }
            }
            .navigationTitle("ThreadRelay")
            .navigationSplitViewColumnWidth(min: 190, ideal: 220, max: 260)
        } detail: {
            Group {
                switch model.selection ?? .overview {
                case .overview:
                    OverviewView()
                case .requestLogs:
                    RequestLogsBaselineView()
                case let section:
                    PlaceholderView(section: section)
                }
            }
            .navigationTitle((model.selection ?? .overview).title)
        }
        .task { await model.refresh() }
        .toolbar {
            if model.selection == .messaging {
                ToolbarItem(placement: .primaryAction) {
                    Button {
                        showsOnboardingBaseline = true
                    } label: {
                        Label("Add Account", systemImage: "plus")
                    }
                }
            }
            ToolbarItem(placement: .primaryAction) {
                Button {
                    Task { await model.refresh() }
                } label: {
                    Label("Refresh", systemImage: "arrow.clockwise")
                }
                .help("Refresh")
            }
        }
        .sheet(isPresented: $showsOnboardingBaseline) {
            OnboardingBaselineView()
        }
    }

    @ViewBuilder
    private func sectionRows(_ sections: [AppSection]) -> some View {
        ForEach(sections) { section in
            Label(section.title, systemImage: section.symbol)
                .tag(section)
                .accessibilityIdentifier("sidebar.\(section.id)")
        }
    }
}

private struct OverviewView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {
                OverviewHeader(status: model.serviceStatus, lastCheckedAt: model.lastCheckedAt)

                VStack(spacing: 0) {
                    StatusRow(
                        title: "Local Service",
                        detail: model.serviceStatus.title,
                        symbol: model.serviceStatus.symbol,
                        tint: model.serviceStatus.tint
                    )
                    Divider()
                    StatusRow(title: "Execution Clients", detail: "Phase 1", symbol: "desktopcomputer")
                    Divider()
                    StatusRow(title: "Messaging Channels", detail: "Phase 3", symbol: "bubble.left.and.bubble.right")
                    Divider()
                    StatusRow(title: "AI Gateway", detail: "Phase 5", symbol: "point.3.connected.trianglepath.dotted")
                }
                .background(.background, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            }
            .frame(maxWidth: 720, alignment: .leading)
            .padding(28)
        }
        .background(Color(nsColor: .windowBackgroundColor))
    }
}

private struct OverviewHeader: View {
    let status: ServiceStatus
    let lastCheckedAt: Date?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label(status.title, systemImage: status.symbol)
                .font(.title.bold())
                .symbolRenderingMode(.hierarchical)
                .foregroundStyle(statusColor)
            Text(status.detail)
                .foregroundStyle(.secondary)
            if let lastCheckedAt {
                Text("Last checked \(lastCheckedAt.formatted(date: .omitted, time: .shortened))")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }
        }
    }

    private var statusColor: Color {
        switch status.tint {
        case .secondary: .secondary
        case .positive: .green
        case .caution: .orange
        case .negative: .red
        }
    }
}

private struct StatusRow: View {
    let title: String
    let detail: String
    let symbol: String
    var tint: StatusTint = .secondary

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: symbol)
                .foregroundStyle(color)
                .frame(width: 20)
            Text(title)
            Spacer()
            Text(detail)
                .foregroundStyle(.secondary)
        }
        .padding(.horizontal, 14)
        .frame(minHeight: 48)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("overview.status.\(title.lowercased().replacingOccurrences(of: " ", with: "-"))")
    }

    private var color: Color {
        switch tint {
        case .secondary: .secondary
        case .positive: .green
        case .caution: .orange
        case .negative: .red
        }
    }
}

private struct PlaceholderView: View {
    let section: AppSection

    var body: some View {
        EmptyStateView(
            title: section.title,
            message: "This section will be connected in its migration phase.",
            symbol: section.symbol
        )
    }
}

struct EmptyStateView: View {
    let title: String
    let message: String?
    let symbol: String

    var body: some View {
        VStack(spacing: 10) {
            Image(systemName: symbol)
                .font(.system(size: 32, weight: .regular))
                .foregroundStyle(.secondary)
            Text(title)
                .font(.title3.weight(.semibold))
            if let message {
                Text(message)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 360)
            }
        }
        .padding(32)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
