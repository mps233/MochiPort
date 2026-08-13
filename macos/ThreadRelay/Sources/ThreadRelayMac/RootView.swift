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
                case .codex:
                    CodexAccessView()
                case .sessions:
                    SessionsView()
                case .requestLogs:
                    RequestLogsView()
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
                case .gateway:
                    GatewayView()
                }
            }
            .navigationTitle((model.selection ?? .overview).title)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(Color(nsColor: .windowBackgroundColor))
            .overlay(alignment: .bottom) {
                if let feedback = model.actionFeedback {
                    ActionFeedbackCapsule(feedback: feedback) {
                        model.actionFeedback = nil
                    }
                    .padding(.bottom, 14)
                    .transition(.opacity.combined(with: .move(edge: .bottom)))
                }
            }
            .animation(.easeInOut(duration: 0.18), value: model.actionFeedback)
            // Let the title bar draw no background of its own so the content
            // color shows through. An opaque toolbar color would span the
            // whole window width and cover the top of the sidebar.
            .toolbarBackground(.hidden, for: .windowToolbar)
        }
        .task {
            await model.refresh()
            model.startAutoRefresh()
            // Silent one-shot update check; delayed inside so it never
            // competes with the startup refresh burst.
            model.scheduleStartupUpdateCheck()
        }
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button {
                    Task {
                        await model.refresh()
                        if let selection = model.selection,
                           selection != .overview,
                           selection != .messaging {
                            await model.loadSection(selection, force: true)
                        }
                    }
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
    @Environment(\.openURL) private var openURL

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                OverviewHeader(
                    status: model.serviceStatus,
                    dashboardState: model.dashboardState,
                    lastCheckedAt: model.lastCheckedAt,
                    hasDashboard: model.dashboard != nil,
                    recoveryInProgress: model.daemonRecoveryInProgress,
                    recoveryError: model.daemonRecoveryError,
                    onRecover: { Task { await model.startDaemonManually() } }
                )

                if let update = model.availableUpdate, !model.updateNoticeDismissed {
                    UpdateNoticeCapsule(
                        version: update.version,
                        onOpen: { openURL(update.url) },
                        onDismiss: { model.updateNoticeDismissed = true }
                    )
                }

                OverviewCardSurface {
                    ConnectionTopologyView()
                }

                ManagementCard(title: "本地服务", symbol: "server.rack") {
                    OverviewStatusRow(
                        title: "后台服务",
                        detail: daemonDetail,
                        symbol: model.serviceStatus.symbol,
                        tint: model.serviceStatus.tint
                    )
                    if let lifecycle = model.lifecycle {
                        Divider()
                        OverviewStatusRow(
                            title: "运行时",
                            detail: "v\(lifecycle.runtime.productVersion) · \(managementMode(lifecycle.management))",
                            symbol: "shippingbox",
                            tint: lifecycle.management.canControl ? .positive : .secondary
                        )
                        Divider()
                        OverviewStatusRow(
                            title: "受保护任务",
                            detail: protectedWorkDetail(lifecycle.protectedWorkItems),
                            symbol: "pause.circle",
                            tint: lifecycle.protectedWorkItems.total == 0 ? .positive : .caution
                        )
                    }
                    Divider()
                    HStack(spacing: 16) {
                        Button("复制诊断信息") {
                            copyDiagnostics()
                        }
                        Button("打开日志") {
                            Task { await openLogDirectory() }
                        }
                    }
                    .buttonStyle(.link)
                }

                ManagementCard(title: "AI 网关", symbol: "point.3.connected.trianglepath.dotted") {
                    OverviewStatusRow(
                        title: "网关",
                        detail: dashboardDetail(model.dashboard?.aiGatewayEnabled),
                        symbol: "point.3.connected.trianglepath.dotted",
                        tint: boolTint(model.dashboard?.aiGatewayEnabled)
                    )
                    Divider()
                    OverviewStatusRow(
                        title: "供应商",
                        detail: providerDetail,
                        symbol: "server.rack",
                        tint: model.dashboard == nil ? .secondary : .positive
                    )
                }
            }
            .frame(maxWidth: 860, alignment: .leading)
            .padding(ThreadRelaySpacing.page)
        }
        .scrollIndicators(.never)
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

/// Dismissible informational capsule shown at the top of the overview when
/// a newer release is available. Dismissal only lasts for this app session.
private struct UpdateNoticeCapsule: View {
    let version: String
    let onOpen: () -> Void
    let onDismiss: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            Button {
                onOpen()
            } label: {
                Label("发现新版本 \(version) — 打开下载页", systemImage: "arrow.down.circle")
            }
            .buttonStyle(.plain)
            .foregroundStyle(.blue)
            .help("打开发布下载页")
            .accessibilityLabel("发现新版本 \(version)，打开下载页")
            Button {
                onDismiss()
            } label: {
                Image(systemName: "xmark")
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .help("本次会话不再提示")
            .accessibilityLabel("关闭新版本提示")
        }
        .font(.callout)
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .background(Color.blue.opacity(0.09), in: Capsule())
        .accessibilityIdentifier("overview.update-notice")
    }
}

/// Chrome-only card wrapper for overview content that carries its own title
/// row (for example the connection topology), matching `ManagementCard`.
private struct OverviewCardSurface<Content: View>: View {
    @ViewBuilder let content: Content

    init(@ViewBuilder content: () -> Content) {
        self.content = content()
    }

    var body: some View {
        content
            .padding(18)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                Color(nsColor: .controlBackgroundColor),
                in: RoundedRectangle(cornerRadius: ThreadRelayRadius.content)
            )
            .overlay {
                RoundedRectangle(cornerRadius: ThreadRelayRadius.content)
                    .stroke(Color.primary.opacity(0.07), lineWidth: 1)
            }
    }
}

private struct OverviewHeader: View {
    let status: ServiceStatus
    let dashboardState: DashboardState
    let lastCheckedAt: Date?
    let hasDashboard: Bool
    var recoveryInProgress = false
    var recoveryError: String?
    var onRecover: (() -> Void)?

    private var showsRecovery: Bool {
        if case .unavailable = status { return onRecover != nil }
        return false
    }

    var body: some View {
        VStack(alignment: .leading, spacing: ThreadRelaySpacing.standard) {
            HStack(alignment: .center, spacing: 14) {
                Image(systemName: status.symbol)
                    .font(.system(size: 23, weight: .medium))
                    .symbolRenderingMode(.hierarchical)
                    .foregroundStyle(status.tint.color)
                    .frame(width: 42, height: 42)
                    .background(status.tint.color.opacity(0.11), in: RoundedRectangle(cornerRadius: 11))
                VStack(alignment: .leading, spacing: 3) {
                    Text(status.title)
                        .font(.title2.weight(.semibold))
                    Text(status.detail)
                        .foregroundStyle(.secondary)
                }
                Spacer(minLength: ThreadRelaySpacing.standard)
                if let lastCheckedAt {
                    VStack(alignment: .trailing, spacing: 3) {
                        Text("上次检查")
                            .font(.caption2)
                            .foregroundStyle(.tertiary)
                        HStack(spacing: 6) {
                            // Kept in the layout at all times so periodic
                            // refreshes never shift the page height.
                            ProgressView()
                                .controlSize(.small)
                                .opacity(dashboardState.isRefreshing ? 1 : 0)
                                .accessibilityHidden(!dashboardState.isRefreshing)
                            Text(lastCheckedAt.formatted(date: .omitted, time: .shortened))
                                .font(.callout.weight(.medium))
                                .monospacedDigit()
                                .foregroundStyle(.secondary)
                        }
                    }
                }
            }
            if let notice {
                Label(notice.text, systemImage: notice.symbol)
                    .font(.callout)
                    .foregroundStyle(notice.color)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 6)
                    .background(notice.color.opacity(0.09), in: Capsule())
            }
            if showsRecovery {
                HStack(spacing: 12) {
                    Button {
                        onRecover?()
                    } label: {
                        if recoveryInProgress {
                            HStack(spacing: 7) {
                                ProgressView()
                                    .controlSize(.small)
                                Text("正在启动…")
                            }
                        } else {
                            Label("启动本地服务", systemImage: "play.circle.fill")
                        }
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(recoveryInProgress)
                    .accessibilityIdentifier("overview.recover-daemon")
                    if let recoveryError {
                        Text(recoveryError)
                            .font(.caption)
                            .foregroundStyle(.red)
                            .lineLimit(2)
                    }
                }
            }
        }
    }

    /// Routine refreshes must not add or remove a banner row: `.refreshing`
    /// never shows one, and `.loading` only does before any data exists.
    private var notice: (text: String, symbol: String, color: Color)? {
        switch dashboardState {
        case .loading:
            hasDashboard ? nil : ("正在加载服务状态…", "arrow.clockwise", .secondary)
        case .refreshing:
            nil
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
}

private struct OverviewStatusRow: View {
    let title: String
    let detail: String
    let symbol: String
    var tint: StatusTint = .secondary

    var body: some View {
        HStack(spacing: ThreadRelaySpacing.standard) {
            Image(systemName: symbol)
                .font(.system(size: 13, weight: .semibold))
                .symbolRenderingMode(.hierarchical)
                .foregroundStyle(tint.color)
                .frame(width: 28, height: 28)
                .background(tint.color.opacity(0.1), in: RoundedRectangle(cornerRadius: 8))
            Text(title)
            Spacer(minLength: ThreadRelaySpacing.standard)
            Text(detail)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.trailing)
        }
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("overview.status.\(title.lowercased().replacingOccurrences(of: " ", with: "-"))")
    }
}

private extension StatusTint {
    var color: Color {
        switch self {
        case .secondary: .secondary
        case .positive: .green
        case .caution: .orange
        case .negative: .red
        }
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
