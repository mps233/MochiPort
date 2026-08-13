import AppKit
import SwiftUI

struct RootView: View {
    @EnvironmentObject private var model: AppModel
    @State private var showsAccountOnboarding = false

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
            .scrollContentBackground(.hidden)
            .background(Color(nsColor: .windowBackgroundColor))
        } detail: {
            Group {
                switch model.selection ?? .overview {
                case .overview:
                    OverviewView()
                case .requestLogs:
                    RequestLogsBaselineView()
                case .messaging:
                    MessagingAccountsView(
                        accounts: model.imAccounts.map(MessagingAccountSummary.init),
                        availability: model.imAccountsAvailability,
                        onAdd: { showsAccountOnboarding = true },
                        onToggle: { account, enabled in
                            let live = model.imAccounts.first {
                                $0.platform == account.platform.rawValue && $0.accountId == account.accountID
                            }
                            guard let live else { return false }
                            return await model.setIMAccountEnabled(live, enabled: enabled)
                        },
                        onDelete: { account in
                            let live = model.imAccounts.first {
                                $0.platform == account.platform.rawValue && $0.accountId == account.accountID
                            }
                            guard let live else { return }
                            Task { await model.deleteIMAccount(live) }
                        }
                    )
                    .overlay(alignment: .bottom) {
                        if let error = model.accountOperationError {
                            HStack(spacing: 10) {
                                Label(error, systemImage: "exclamationmark.triangle")
                                Button {
                                    model.accountOperationError = nil
                                } label: {
                                    Image(systemName: "xmark")
                                }
                                .buttonStyle(.plain)
                                .foregroundStyle(.secondary)
                                .help("关闭提示")
                            }
                            .font(.callout)
                            .foregroundStyle(.red)
                            .padding(.horizontal, 14)
                            .padding(.vertical, 9)
                            .background(.regularMaterial, in: Capsule())
                            .padding(.bottom, 14)
                        }
                    }
                case let section:
                    PlaceholderView(section: section)
                }
            }
            .navigationTitle((model.selection ?? .overview).title)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(Color(nsColor: .windowBackgroundColor))
        }
        .task {
            await model.refresh()
            model.startAutoRefresh()
        }
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button {
                    Task { await model.refresh() }
                } label: {
                    Label("刷新", systemImage: "arrow.clockwise")
                }
                .disabled(model.dashboardState == .loading || model.dashboardState == .refreshing)
                .help("刷新")
            }
        }
        .sheet(isPresented: $showsAccountOnboarding) {
            MessagingOnboardingView(
                actions: MessagingOnboardingActions(
                    configureTelegram: { botToken, mentionOnly in
                        try await model.configureTelegramAccount(
                            botToken: botToken,
                            mentionOnly: mentionOnly
                        )
                    },
                    configureFeishu: { appId, appSecret in
                        try await model.configureFeishuAccount(
                            appId: appId,
                            appSecret: appSecret
                        )
                    },
                    startFeishuScan: { try await model.startFeishuOnboarding() },
                    pollFeishuScan: { deviceCode in
                        try await model.pollFeishuOnboarding(deviceCode: deviceCode)
                    },
                    startWechatScan: { try await model.startWechatOnboarding() },
                    pollWechatScan: { sessionKey, verifyCode in
                        try await model.pollWechatOnboarding(
                            sessionKey: sessionKey,
                            verifyCode: verifyCode
                        )
                    },
                    startWecomScan: { try await model.startWecomOnboarding() },
                    pollWecomScan: { sessionKey in
                        try await model.pollWecomOnboarding(sessionKey: sessionKey)
                    }
                )
            )
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

                ConnectionTopologyView()

                VStack(alignment: .leading, spacing: 0) {
                    DashboardSection(title: "本地服务", symbol: "server.rack") {
                        StatusRow(
                            title: "后台服务",
                            detail: daemonDetail,
                            symbol: model.serviceStatus.symbol,
                            tint: model.serviceStatus.tint
                        )
                        if let lifecycle = model.lifecycle {
                            RowDivider()
                            StatusRow(
                                title: "运行时",
                                detail: "v\(lifecycle.runtime.productVersion) · \(managementMode(lifecycle.management))",
                                symbol: "shippingbox",
                                tint: lifecycle.management.canControl ? .positive : .secondary
                            )
                            RowDivider()
                            StatusRow(
                                title: "受保护任务",
                                detail: protectedWorkDetail(lifecycle.protectedWorkItems),
                                symbol: "pause.circle",
                                tint: lifecycle.protectedWorkItems.total == 0 ? .positive : .caution
                            )
                        }
                        HStack(spacing: 12) {
                            Button("复制诊断信息") {
                                copyDiagnostics()
                            }
                            Button("打开日志") {
                                Task { await openLogDirectory() }
                            }
                        }
                        .buttonStyle(.link)
                        .padding(.horizontal, 14)
                        .padding(.bottom, 12)
                    }
                    SectionDivider()
                    DashboardSection(title: "AI 网关", symbol: "point.3.connected.trianglepath.dotted") {
                        StatusRow(
                            title: "网关",
                            detail: dashboardDetail(model.dashboard?.aiGatewayEnabled),
                            symbol: "point.3.connected.trianglepath.dotted",
                            tint: boolTint(model.dashboard?.aiGatewayEnabled)
                        )
                        RowDivider()
                        StatusRow(
                            title: "供应商",
                            detail: providerDetail,
                            symbol: "server.rack",
                            tint: model.dashboard == nil ? .secondary : .positive
                        )
                    }
                }
            }
            .frame(maxWidth: 720, alignment: .leading)
            .padding(28)
        }
        .background(Color(nsColor: .windowBackgroundColor))
    }

    private var remoteDetail: String {
        guard let dashboard = model.dashboard else { return unavailableDetail }
        return dashboard.remoteControlHealthy ? "状态正常" : dashboard.remoteControlConnected ? "已连接" : "离线"
    }

    private var daemonDetail: String {
        guard let lifecycle = model.lifecycle else { return model.serviceStatus.title }
        return lifecycle.management.canControl ? "已托管 · \(runtimeState(lifecycle.runtime.state))" : "运行正常 · 只读"
    }

    private func runtimeState(_ state: String) -> String {
        switch state {
        case "active": "运行中"
        default: "未知状态"
        }
    }

    private func managementMode(_ management: ManageLifecycle.Management) -> String {
        if management.canControl { return "可管理" }
        return management.mode == "readOnly" ? "只读" : "未知模式"
    }

    private func protectedWorkDetail(_ items: ManageLifecycle.ProtectedWorkItems) -> String {
        guard items.total > 0 else { return "无" }
        return "\(items.total) 项进行中"
    }

    private var providerDetail: String {
        guard let dashboard = model.dashboard else { return unavailableDetail }
        return "已配置 \(dashboard.aiGatewayProviderCount) 个"
    }

    private var unavailableDetail: String {
        switch model.dashboardState {
        case .legacy: "需要更新"
        case .unauthorized: "需要授权"
        case .unavailable: "不可用"
        case .offline: "不可用"
        case .stale: "上次状态"
        case .starting: "正在启动"
        default: "检查中"
        }
    }

    private func dashboardDetail(_ value: Bool?) -> String {
        guard let value else { return unavailableDetail }
        return value ? "已就绪" : "未配置"
    }

    private func boolTint(_ value: Bool?) -> StatusTint {
        guard let value else { return .secondary }
        return value ? .positive : .caution
    }

    private func copyDiagnostics() {
        let dashboard = model.dashboard
        let lines = [
            "ThreadRelay 状态：\(model.serviceStatus.title)",
            "仪表盘状态：\(model.dashboardState.title)",
            "服务 API：\(dashboard?.service.apiMajor.description ?? "未知")",
            "服务就绪：\(readyDescription(dashboard?.service.ready))",
            "远程控制：\(remoteDetail)",
            "AI 网关：\(dashboardDetail(dashboard?.aiGatewayEnabled))",
        ]
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(lines.joined(separator: "\n"), forType: .string)
    }

    private func readyDescription(_ ready: Bool?) -> String {
        guard let ready else { return "未知" }
        return ready ? "是" : "否"
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
                Text("上次检查：\(lastCheckedAt.formatted(date: .omitted, time: .shortened))")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }
        }
    }

    private var notice: (text: String, symbol: String, color: Color)? {
        switch dashboardState {
        case .loading:
            ("正在加载服务状态…", "arrow.clockwise", .secondary)
        case .refreshing:
            ("正在刷新…", "arrow.clockwise", .secondary)
        case .starting:
            ("本地服务仍在启动。", "clock", .orange)
        case .legacy:
            ("请更新后台服务以查看完整仪表盘。", "arrow.triangle.2.circlepath", .orange)
        case .unauthorized:
            ("管理凭据已变化，请在控制文件可用后刷新。", "lock.trianglebadge.exclamationmark", .red)
        case .unavailable:
            ("本地服务已就绪，但仪表盘无法加载。", "exclamationmark.triangle", .red)
        case .offline:
            ("无法连接本地服务。", "network.slash", .red)
        case .stale:
            ("刷新失败，当前显示上次获取的状态。", "clock.badge.exclamationmark", .orange)
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
            message: "该功能将在后续迁移阶段接入。",
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
