import AppKit
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
        .task {
            await model.refresh()
            model.startAutoRefresh()
        }
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
                .disabled(model.dashboardState == .loading || model.dashboardState == .refreshing)
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
                OverviewHeader(
                    status: model.serviceStatus,
                    dashboardState: model.dashboardState,
                    lastCheckedAt: model.lastCheckedAt
                )

                VStack(alignment: .leading, spacing: 0) {
                    DashboardSection(title: "Local service", symbol: "server.rack") {
                        StatusRow(
                            title: "Daemon",
                            detail: daemonDetail,
                            symbol: model.serviceStatus.symbol,
                            tint: model.serviceStatus.tint
                        )
                        if let lifecycle = model.lifecycle {
                            RowDivider()
                            StatusRow(
                                title: "Runtime",
                                detail: "v\(lifecycle.runtime.productVersion) · \(lifecycle.management.mode)",
                                symbol: "shippingbox",
                                tint: lifecycle.management.canControl ? .positive : .secondary
                            )
                            RowDivider()
                            StatusRow(
                                title: "Protected work",
                                detail: protectedWorkDetail(lifecycle.protectedWorkItems),
                                symbol: "pause.circle",
                                tint: lifecycle.protectedWorkItems.total == 0 ? .positive : .caution
                            )
                        }
                        HStack(spacing: 12) {
                            Button("Copy Diagnostics") {
                                copyDiagnostics()
                            }
                            Button("Open Logs") {
                                Task { await openLogDirectory() }
                            }
                        }
                        .buttonStyle(.link)
                        .padding(.horizontal, 14)
                        .padding(.bottom, 12)
                    }
                    SectionDivider()
                    DashboardSection(title: "Execution clients", symbol: "desktopcomputer") {
                        StatusRow(
                            title: "Codex App",
                            detail: endpointDetail(model.dashboard?.executionClients.codexApp),
                            symbol: "chevron.left.forwardslash.chevron.right",
                            tint: endpointTint(model.dashboard?.executionClients.codexApp)
                        )
                        RowDivider()
                        StatusRow(
                            title: "VS Code",
                            detail: sessionEndpointDetail(model.dashboard?.executionClients.vscode),
                            symbol: "chevron.left.forwardslash.chevron.right",
                            tint: sessionEndpointTint(model.dashboard?.executionClients.vscode)
                        )
                        RowDivider()
                        StatusRow(
                            title: "CLI",
                            detail: sessionEndpointDetail(model.dashboard?.executionClients.cli),
                            symbol: "terminal",
                            tint: sessionEndpointTint(model.dashboard?.executionClients.cli)
                        )
                        RowDivider()
                        StatusRow(
                            title: "Remote control",
                            detail: remoteDetail,
                            symbol: "arrow.triangle.2.circlepath",
                            tint: remoteTint
                        )
                    }
                    SectionDivider()
                    DashboardSection(title: "Messaging channels", symbol: "bubble.left.and.bubble.right") {
                        if let legacy = model.dashboard?.messageChannels.legacyUnattributed,
                           legacy.accountCount > 0 {
                            channelRow(
                                "Messaging accounts",
                                legacy,
                                symbol: "bubble.left.and.bubble.right.fill"
                            )
                        } else {
                            channelRow("Telegram", model.dashboard?.messageChannels.telegram, symbol: "paperplane")
                            RowDivider()
                            channelRow("Feishu", model.dashboard?.messageChannels.feishu, symbol: "bubble.left.and.text.bubble.right")
                            RowDivider()
                            channelRow("WeChat", model.dashboard?.messageChannels.wechat, symbol: "message")
                            RowDivider()
                            channelRow("WeCom", model.dashboard?.messageChannels.wecom, symbol: "person.2")
                        }
                        RowDivider()
                        StatusRow(
                            title: "Bridge",
                            detail: dashboardDetail(model.dashboard?.bridgeRunning),
                            symbol: "arrow.triangle.merge",
                            tint: boolTint(model.dashboard?.bridgeRunning)
                        )
                    }
                    SectionDivider()
                    DashboardSection(title: "AI Gateway", symbol: "point.3.connected.trianglepath.dotted") {
                        StatusRow(
                            title: "Gateway",
                            detail: dashboardDetail(model.dashboard?.aiGatewayEnabled),
                            symbol: "point.3.connected.trianglepath.dotted",
                            tint: boolTint(model.dashboard?.aiGatewayEnabled)
                        )
                        RowDivider()
                        StatusRow(
                            title: "Providers",
                            detail: providerDetail,
                            symbol: "server.rack",
                            tint: model.dashboard == nil ? .secondary : .positive
                        )
                    }
                }
                .background(.background)
            }
            .frame(maxWidth: 720, alignment: .leading)
            .padding(28)
        }
        .background(Color(nsColor: .windowBackgroundColor))
    }

    private var remoteDetail: String {
        guard let dashboard = model.dashboard else { return unavailableDetail }
        return dashboard.remoteControlHealthy ? "Healthy" : dashboard.remoteControlConnected ? "Connected" : "Offline"
    }

    private var daemonDetail: String {
        guard let lifecycle = model.lifecycle else { return model.serviceStatus.title }
        return lifecycle.management.canControl ? "Managed · \(lifecycle.runtime.state)" : "Available · read-only"
    }

    private func protectedWorkDetail(_ items: ManageLifecycle.ProtectedWorkItems) -> String {
        guard items.total > 0 else { return "None" }
        return "\(items.total) active"
    }

    private var remoteTint: StatusTint {
        guard let dashboard = model.dashboard else { return .secondary }
        return dashboard.remoteControlHealthy ? .positive : dashboard.remoteControlConnected ? .caution : .negative
    }

    private var providerDetail: String {
        guard let dashboard = model.dashboard else { return unavailableDetail }
        return "\(dashboard.aiGatewayProviderCount) configured"
    }

    private var unavailableDetail: String {
        switch model.dashboardState {
        case .legacy: "Update required"
        case .unauthorized: "Authorization required"
        case .unavailable: "Unavailable"
        case .offline: "Unavailable"
        case .stale: "Last known status"
        case .starting: "Starting"
        default: "Checking"
        }
    }

    private func dashboardDetail(_ value: Bool?) -> String {
        guard let value else { return unavailableDetail }
        return value ? "Ready" : "Not configured"
    }

    @ViewBuilder
    private func channelRow(
        _ title: String,
        _ channel: ManageDashboard.MessageChannel?,
        symbol: String
    ) -> some View {
        StatusRow(
            title: title,
            detail: channelDetail(channel),
            symbol: symbol,
            tint: channelTint(channel)
        )
    }

    private func channelDetail(_ channel: ManageDashboard.MessageChannel?) -> String {
        guard let channel else { return unavailableDetail }
        guard channel.accountCount > 0 else { return "Not configured" }
        return "\(channel.connectedAccountCount) of \(channel.accountCount) connected"
    }

    private func channelTint(_ channel: ManageDashboard.MessageChannel?) -> StatusTint {
        guard let channel else { return .secondary }
        guard channel.accountCount > 0 else { return .secondary }
        return channel.accountCount == channel.connectedAccountCount ? .positive : .caution
    }

    private func endpointDetail(_ endpoint: ManageDashboard.Endpoint?) -> String {
        guard let endpoint else { return unavailableDetail }
        if endpoint.connected { return "Connected" }
        return endpoint.configured ? "Available" : "Not detected"
    }

    private func endpointTint(_ endpoint: ManageDashboard.Endpoint?) -> StatusTint {
        guard let endpoint else { return .secondary }
        if endpoint.connected { return .positive }
        return endpoint.configured ? .caution : .secondary
    }

    private func sessionEndpointDetail(_ endpoint: ManageDashboard.Endpoint?) -> String {
        guard let endpoint else { return unavailableDetail }
        return endpoint.connected ? "Connected" : "No active session"
    }

    private func sessionEndpointTint(_ endpoint: ManageDashboard.Endpoint?) -> StatusTint {
        guard let endpoint else { return .secondary }
        return endpoint.connected ? .positive : .secondary
    }

    private func boolTint(_ value: Bool?) -> StatusTint {
        guard let value else { return .secondary }
        return value ? .positive : .caution
    }

    private func copyDiagnostics() {
        let dashboard = model.dashboard
        let lines = [
            "ThreadRelay status: \(model.serviceStatus.title)",
            "Dashboard state: \(String(describing: model.dashboardState))",
            "Service API: \(dashboard?.service.apiMajor.description ?? "unknown")",
            "Service ready: \(dashboard?.service.ready.description ?? "unknown")",
            "Remote control: \(remoteDetail)",
            "AI Gateway: \(dashboardDetail(dashboard?.aiGatewayEnabled))",
        ]
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(lines.joined(separator: "\n"), forType: .string)
    }

    private func openLogDirectory() async {
        guard let directory = await model.logDirectory() else { return }
        NSWorkspace.shared.open(directory)
    }
}

private struct DashboardSection<Content: View>: View {
    let title: String
    let symbol: String
    @ViewBuilder let content: Content

    init(title: String, symbol: String, @ViewBuilder content: () -> Content) {
        self.title = title
        self.symbol = symbol
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Label(title, systemImage: symbol)
                .font(.headline)
                .foregroundStyle(.secondary)
                .padding(.horizontal, 14)
                .padding(.top, 16)
            VStack(spacing: 0) {
                content
            }
        }
    }
}

private struct SectionDivider: View {
    var body: some View {
        Divider()
            .padding(.vertical, 4)
    }
}

private struct RowDivider: View {
    var body: some View {
        Divider()
            .padding(.leading, 48)
    }
}

private struct OverviewHeader: View {
    let status: ServiceStatus
    let dashboardState: DashboardState
    let lastCheckedAt: Date?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label(status.title, systemImage: status.symbol)
                .font(.title.bold())
                .symbolRenderingMode(.hierarchical)
                .foregroundStyle(statusColor)
            Text(status.detail)
                .foregroundStyle(.secondary)
            if let notice {
                Label(notice.text, systemImage: notice.symbol)
                    .font(.callout)
                    .foregroundStyle(notice.color)
                    .padding(.top, 2)
            }
            if let lastCheckedAt {
                Text("Last checked \(lastCheckedAt.formatted(date: .omitted, time: .shortened))")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }
        }
    }

    private var notice: (text: String, symbol: String, color: Color)? {
        switch dashboardState {
        case .loading:
            ("Loading service status…", "arrow.clockwise", .secondary)
        case .refreshing:
            ("Refreshing…", "arrow.clockwise", .secondary)
        case .starting:
            ("The local service is still starting.", "clock", .orange)
        case .legacy:
            ("Update the daemon to view the full dashboard.", "arrow.triangle.2.circlepath", .orange)
        case .unauthorized:
            ("The management credential changed. Refresh after the control file is available.", "lock.trianglebadge.exclamationmark", .red)
        case .unavailable:
            ("The local service is ready, but its dashboard could not be loaded.", "exclamationmark.triangle", .red)
        case .offline:
            ("The local service could not be reached.", "network.slash", .red)
        case .stale:
            ("Showing the last known status because refresh failed.", "clock.badge.exclamationmark", .orange)
        case .loaded:
            nil
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
