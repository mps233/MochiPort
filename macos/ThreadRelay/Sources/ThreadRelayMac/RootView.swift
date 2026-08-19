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
                        accounts: model.imAccounts.compactMap(MessagingAccountSummary.init),
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
            .safeAreaInset(edge: .bottom, spacing: 0) {
                GatewayQuotaDock()
                .padding(.horizontal, 16)
                .padding(.bottom, 8)
            }
            .overlay(alignment: .bottom) {
                if let feedback = model.actionFeedback {
                    ActionFeedbackCapsule(feedback: feedback) {
                        model.actionFeedback = nil
                    }
                    .padding(.horizontal, 16)
                    .padding(.bottom, 68)
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
    @State private var manualSub2ApiRefreshTask: Task<Void, Never>?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: ThreadRelayPageLayout.sectionSpacing) {
                OverviewMasthead(
                    status: model.serviceStatus,
                    dashboardState: model.dashboardState,
                    lastCheckedAt: model.lastCheckedAt,
                    hasDashboard: model.dashboard != nil,
                    recoveryInProgress: model.daemonRecoveryInProgress,
                    recoveryError: model.daemonRecoveryError,
                    onRecover: { Task { await model.startDaemonManually() } }
                )

                if let update = model.availableUpdate, !model.updateNoticeDismissed {
                    OverviewUpdateNotice(
                        version: update.version,
                        onOpen: { openURL(update.url) },
                        onDismiss: { model.updateNoticeDismissed = true }
                    )
                }

                OverviewSignalStrip(
                    dashboard: model.dashboard,
                    dashboardState: model.dashboardState,
                    serviceStatus: model.serviceStatus,
                    remoteDetail: remoteDetail,
                    bridgeDetail: bridgeDetail
                )

                OverviewSectionHeading(
                    title: "连接拓扑",
                    subtitle: "客户端和消息渠道通过本地服务的实时连接"
                )
                ConnectionTopologyView(showsHeader: false)

                LazyVGrid(
                    columns: [GridItem(.adaptive(minimum: 350), spacing: 28)],
                    spacing: 28
                ) {
                    OverviewDetailSection(
                        title: "AI 网关",
                        subtitle: "请求路由与供应商"
                    ) {
                        OverviewKeyValueRow(
                            title: "网关",
                            detail: dashboardDetail(model.dashboard?.aiGatewayEnabled),
                            symbol: "point.3.connected.trianglepath.dotted",
                            tint: boolTint(model.dashboard?.aiGatewayEnabled)
                        )
                        Divider()
                        OverviewKeyValueRow(
                            title: "供应商",
                            detail: providerDetail,
                            symbol: "server.rack",
                            tint: model.dashboard == nil ? .secondary : .positive
                        )
                        Divider()
                        Button {
                            model.selection = .gateway
                        } label: {
                            Label("打开网关设置", systemImage: "arrow.up.right")
                        }
                        .buttonStyle(.link)
                    }

                    OverviewDetailSection(
                        title: "本地服务",
                        subtitle: "进程、运行时与任务保护"
                    ) {
                        OverviewKeyValueRow(
                            title: "后台服务",
                            detail: daemonDetail,
                            symbol: model.serviceStatus.symbol,
                            tint: model.serviceStatus.tint
                        )
                        if let lifecycle = model.lifecycle {
                            Divider()
                            OverviewKeyValueRow(
                                title: "运行时",
                                detail: runtimeDetail(lifecycle),
                                symbol: "shippingbox",
                                tint: lifecycle.management.canControl ? .positive : .secondary
                            )
                            if model.daemonBuildMismatch {
                                Divider()
                                OverviewKeyValueRow(
                                    title: "版本不一致",
                                    detail: model.daemonUpgradePending
                                        ? model.daemonUpgradeDetail
                                        : "界面与后台服务构建不一致",
                                    symbol: "exclamationmark.triangle",
                                    tint: .caution
                                )
                            }
                            Divider()
                            OverviewKeyValueRow(
                                title: "受保护任务",
                                detail: protectedWorkDetail(lifecycle.protectedWorkItems),
                                symbol: "pause.circle",
                                tint: lifecycle.protectedWorkItems.total == 0 ? .positive : .caution
                            )
                        }
                        Divider()
                        HStack(spacing: 16) {
                            Button {
                                copyDiagnostics()
                            } label: {
                                Label("复制诊断", systemImage: "doc.on.doc")
                            }
                            Button {
                                Task { await openLogDirectory() }
                            } label: {
                                Label("打开日志", systemImage: "folder")
                            }
                        }
                        .buttonStyle(.link)
                    }
                }

                OverviewSectionHeading(
                    title: "Sub2API 账号池",
                    subtitle: "按站点合并渠道；展开站点查看账号余额与倍率",
                    trailing: sub2ApiAccountCount,
                    isRefreshing: model.sub2ApiAccountPoolLoading,
                    onRefresh: sub2ApiRefreshAction
                )
                OverviewSub2ApiAccountPoolSummary(
                    admin: model.sub2ApiAdmin,
                    pool: model.sub2ApiAccountPool,
                    isLoading: model.sub2ApiAccountPoolLoading,
                    loadError: model.sub2ApiAccountPoolError,
                    onConnect: { model.selection = .gateway }
                )
            }
            .frame(maxWidth: ThreadRelayPageLayout.maxContentWidth, alignment: .leading)
            .padding(.horizontal, ThreadRelayPageLayout.horizontalPadding)
            .padding(.top, ThreadRelayPageLayout.topPadding)
            .padding(.bottom, ThreadRelayPageLayout.bottomPadding)
        }
        .scrollIndicators(.never)
        .background(Color(nsColor: .windowBackgroundColor))
        .task {
            await model.refreshSub2ApiAccountPool()
        }
        .onDisappear {
            manualSub2ApiRefreshTask?.cancel()
            manualSub2ApiRefreshTask = nil
            model.cancelSub2ApiAccountPoolRefresh()
        }
    }

    private var bridgeDetail: String {
        guard let dashboard = model.dashboard else { return unavailableDetail }
        return dashboard.bridgeRunning ? "运行中" : "未运行"
    }

    private func refreshSub2Api() {
        manualSub2ApiRefreshTask?.cancel()
        manualSub2ApiRefreshTask = Task {
            await model.refreshSub2ApiAccountPool(forceBillingRefresh: true)
        }
    }

    private var sub2ApiRefreshAction: (() -> Void)? {
        guard model.sub2ApiAdmin?.configured == true else { return nil }
        return { refreshSub2Api() }
    }

    private var remoteDetail: String {
        guard let dashboard = model.dashboard else { return unavailableDetail }
        return dashboard.remoteControlHealthy ? "状态正常" : dashboard.remoteControlConnected ? "已连接" : "离线"
    }

    private var daemonDetail: String {
        guard let lifecycle = model.lifecycle else { return model.serviceStatus.title }
        if model.ownsDaemonLease {
            return "已托管 · \(runtimeState(lifecycle.runtime.state))"
        }
        if model.daemonLeaseConflict {
            return "运行正常 · 其他安装管理"
        }
        return "运行正常 · 仅查看"
    }

    private func runtimeState(_ state: String) -> String {
        switch state {
        case "active": "运行中"
        case "draining": "排空中"
        case "shutdownCommitted": "正在切换"
        default: "未知状态"
        }
    }

    private func managementMode(_ management: ManageLifecycle.Management) -> String {
        if model.ownsDaemonLease { return "当前安装已托管" }
        if model.daemonLeaseConflict { return "其他安装已托管" }
        return management.mode == "readOnly" ? "仅查看" : "未知模式"
    }

    private func runtimeDetail(_ lifecycle: ManageLifecycle) -> String {
        var components = ["v\(lifecycle.runtime.productVersion)"]
        if let buildNumber = lifecycle.runtime.buildNumber {
            components.append("构建 \(buildNumber)")
        }
        components.append(managementMode(lifecycle.management))
        return components.joined(separator: " · ")
    }

    private func protectedWorkDetail(_ items: ManageLifecycle.ProtectedWorkItems) -> String {
        guard items.total > 0 else { return "无" }
        return "\(items.total) 项进行中"
    }

    private var providerDetail: String {
        guard let dashboard = model.dashboard else { return unavailableDetail }
        return "已配置 \(dashboard.aiGatewayProviderCount) 个"
    }

    private var sub2ApiAccountCount: String? {
        guard let accounts = model.sub2ApiAccountPool?.accounts else { return nil }
        return "\(accounts.count) 个账号"
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
            "后台构建：\(model.lifecycle?.runtime.buildNumber.map(String.init) ?? "旧版/未知")",
            "构建一致性：\(model.daemonBuildMismatch ? "不一致" : "未发现差异")",
            "后台版本：\(model.daemonUpgradePending ? model.daemonUpgradeDetail : "无差异")",
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

private struct OverviewSectionHeading: View {
    let title: String
    let subtitle: String?
    let trailing: String?
    let isRefreshing: Bool
    let onRefresh: (() -> Void)?

    init(
        title: String,
        subtitle: String? = nil,
        trailing: String? = nil,
        isRefreshing: Bool = false,
        onRefresh: (() -> Void)? = nil
    ) {
        self.title = title
        self.subtitle = subtitle
        self.trailing = trailing
        self.isRefreshing = isRefreshing
        self.onRefresh = onRefresh
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            VStack(alignment: .leading, spacing: 3) {
                Text(title)
                    .font(.headline)
                if let subtitle {
                    Text(subtitle)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                }
            }
            Spacer(minLength: 12)
            if let trailing {
                Text(trailing)
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
            }
            if let onRefresh {
                Button(action: onRefresh) {
                    ZStack {
                        Image(systemName: "arrow.clockwise")
                            .opacity(isRefreshing ? 0 : 1)
                        ProgressView()
                            .controlSize(.small)
                            .opacity(isRefreshing ? 1 : 0)
                    }
                    .frame(width: 18, height: 18)
                }
                .buttonStyle(.plain)
                .disabled(isRefreshing)
                .help("刷新账号余额与倍率")
                .accessibilityLabel("刷新 Sub2API 账号池")
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct OverviewDetailSection<Content: View>: View {
    let title: String
    let subtitle: String?
    @ViewBuilder let content: Content

    init(
        title: String,
        subtitle: String? = nil,
        @ViewBuilder content: () -> Content
    ) {
        self.title = title
        self.subtitle = subtitle
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            OverviewSectionHeading(title: title, subtitle: subtitle)
            VStack(alignment: .leading, spacing: 12) {
                content
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct OverviewUpdateNotice: View {
    let version: String
    let onOpen: () -> Void
    let onDismiss: () -> Void

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: "arrow.down.circle.fill")
                .symbolRenderingMode(.hierarchical)
                .foregroundStyle(.blue)
            Button {
                onOpen()
            } label: {
                VStack(alignment: .leading, spacing: 2) {
                    Text("ThreadRelay \(version) 可用")
                        .font(.callout.weight(.medium))
                    Text("打开下载页查看新版")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            .buttonStyle(.plain)
            .help("打开发布下载页")
            .accessibilityLabel("发现新版本 \(version)，打开下载页")
            Spacer(minLength: 12)
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
        .padding(.vertical, 2)
        .accessibilityIdentifier("overview.update-notice")
    }
}

private struct OverviewMasthead: View {
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
        VStack(alignment: .leading, spacing: 14) {
            HStack(alignment: .center, spacing: 12) {
                Circle()
                    .fill(status.tint.color)
                    .frame(width: 10, height: 10)
                    .shadow(color: status.tint.color.opacity(0.35), radius: 4)
                VStack(alignment: .leading, spacing: 2) {
                    Text(status.title)
                        .font(.title3.weight(.semibold))
                    Text(status.detail)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }
                Spacer(minLength: 16)
                refreshStamp
            }
            if let notice {
                Label(notice.text, systemImage: notice.symbol)
                    .font(.callout)
                    .foregroundStyle(notice.color)
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

    @ViewBuilder
    private var refreshStamp: some View {
        if let lastCheckedAt {
            HStack(spacing: 6) {
                ProgressView()
                    .controlSize(.small)
                    .opacity(dashboardState.isRefreshing ? 1 : 0)
                    .accessibilityHidden(!dashboardState.isRefreshing)
                Text("更新于 \(lastCheckedAt.formatted(date: .omitted, time: .shortened))")
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
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

private struct OverviewSignalStrip: View {
    let dashboard: ManageDashboard?
    let dashboardState: DashboardState
    let serviceStatus: ServiceStatus
    let remoteDetail: String
    let bridgeDetail: String

    private var clientCount: Int? {
        guard let clients = dashboard?.executionClients else { return nil }
        return [clients.codexApp, clients.vscode, clients.cli].count(where: \.connected)
    }

    private var channelCount: Int? {
        guard let channels = dashboard?.messageChannels else { return nil }
        return [channels.telegram, channels.feishu, channels.wechat, channels.wecom]
            .count(where: { $0.connectedAccountCount > 0 })
    }

    private var unavailableText: String {
        switch dashboardState {
        case .stale: "上次状态"
        case .offline, .unavailable: "不可用"
        case .legacy: "需更新"
        default: "检查中"
        }
    }

    var body: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 0) {
                signalCells
            }
            VStack(spacing: 0) {
                signalCells
            }
        }
        .padding(.vertical, 14)
        .overlay(alignment: .top) { Divider() }
        .overlay(alignment: .bottom) { Divider() }
        .accessibilityIdentifier("overview.signal-strip")
    }

    @ViewBuilder
    private var signalCells: some View {
        OverviewSignalCell(
            title: "服务",
            value: serviceStatus.title,
            symbol: "server.rack",
            tint: serviceStatus.tint
        )
        OverviewSignalDivider()
        OverviewSignalCell(
            title: "客户端",
            value: clientCount.map { "\($0) 个在线" } ?? unavailableText,
            symbol: "laptopcomputer.and.iphone",
            tint: clientCount.map { $0 > 0 ? .positive : .secondary } ?? .secondary
        )
        OverviewSignalDivider()
        OverviewSignalCell(
            title: "消息渠道",
            value: channelCount.map { "\($0) 个在线" } ?? unavailableText,
            symbol: "bubble.left.and.bubble.right",
            tint: channelCount.map { $0 > 0 ? .positive : .secondary } ?? .secondary
        )
        OverviewSignalDivider()
        OverviewSignalCell(
            title: "远程控制",
            value: remoteDetail,
            symbol: "network",
            tint: dashboard?.remoteControlHealthy == true ? .positive : .secondary
        )
        OverviewSignalDivider()
        OverviewSignalCell(
            title: "消息桥接",
            value: bridgeDetail,
            symbol: "arrow.left.arrow.right",
            tint: dashboard?.bridgeRunning == true ? .positive : .secondary
        )
    }
}

private struct OverviewSignalCell: View {
    let title: String
    let value: String
    let symbol: String
    let tint: StatusTint

    var body: some View {
        HStack(spacing: 9) {
            Image(systemName: symbol)
                .font(.system(size: 14, weight: .medium))
                .foregroundStyle(tint.color)
                .frame(width: 18)
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Text(value)
                    .font(.callout.weight(.medium))
                    .lineLimit(1)
            }
            Spacer(minLength: 8)
        }
        .frame(maxWidth: .infinity, minHeight: 38, alignment: .leading)
        .padding(.horizontal, 12)
        .accessibilityElement(children: .combine)
    }
}

private struct OverviewSignalDivider: View {
    var body: some View {
        Divider()
            .frame(maxHeight: 34)
    }
}

private struct OverviewKeyValueRow: View {
    let title: String
    let detail: String
    let symbol: String
    var tint: StatusTint = .secondary

    var body: some View {
        HStack(spacing: ThreadRelaySpacing.standard) {
            Image(systemName: symbol)
                .font(.system(size: 14, weight: .medium))
                .symbolRenderingMode(.hierarchical)
                .foregroundStyle(tint.color)
                .frame(width: 20)
            Text(title)
                .font(.callout)
            Spacer(minLength: ThreadRelaySpacing.standard)
            Text(detail)
                .font(.callout)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.trailing)
        }
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("overview.status.\(title.lowercased().replacingOccurrences(of: " ", with: "-"))")
    }
}

private struct OverviewSub2ApiAccountPoolSummary: View {
    let admin: ManageSub2ApiAdmin?
    let pool: ManageSub2ApiAccountPoolResponse.Pool?
    let isLoading: Bool
    let loadError: String?
    let onConnect: () -> Void
    @State private var showsAllAccounts = false

    private var configured: Bool { admin?.configured == true }
    private let collapsedAccountLimit = 6

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            VStack(alignment: .leading, spacing: 12) {
                if !configured {
                    HStack(spacing: 10) {
                        Label("尚未连接账号管理接口", systemImage: "link.badge.plus")
                            .font(.callout)
                            .foregroundStyle(.secondary)
                        Spacer(minLength: 12)
                        Button("前往连接", action: onConnect)
                    }
                    .frame(minHeight: 36)
                } else if pool?.accounts.isEmpty != false {
                    OverviewSub2ApiAccountPoolEmptyState(
                        isLoading: isLoading,
                        loadError: loadError
                    )
                } else if let pool {
                    if let loadError {
                        Label("刷新失败，正在显示上次结果", systemImage: "clock.badge.exclamationmark")
                            .font(.caption)
                            .foregroundStyle(.orange)
                            .help(loadError)
                    }
                    if let warnings = pool.warnings, !warnings.isEmpty {
                        Label(
                            sub2ApiWarningsText(warnings),
                            systemImage: "exclamationmark.triangle"
                        )
                        .font(.caption)
                        .foregroundStyle(.orange)
                    }

                    OverviewSub2ApiAccountTable(
                        accounts: pool.accounts,
                        accountLimit: showsAllAccounts ? nil : collapsedAccountLimit
                    )

                    if pool.accounts.count > collapsedAccountLimit {
                        Button {
                            withAnimation(.spring(response: 0.3, dampingFraction: 1)) {
                                showsAllAccounts.toggle()
                            }
                        } label: {
                            Label(
                                showsAllAccounts
                                    ? "收起账号"
                                    : "展开全部 \(pool.accounts.count) 个账号",
                                systemImage: showsAllAccounts ? "chevron.up" : "chevron.down"
                            )
                        }
                        .buttonStyle(.plain)
                        .font(.caption.weight(.medium))
                        .foregroundStyle(.secondary)
                    }

                    HStack(spacing: 6) {
                        Text("更新于 \(sub2ApiFetchedTime(pool.fetchedAtMs))")
                        Text("·")
                        Text("同站点渠道已合并，展开查看账号明细")
                    }
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
                    .lineLimit(1)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .accessibilityIdentifier("overview.sub2api-account-pool")
    }
}

private struct OverviewSub2ApiAccountPoolEmptyState: View {
    let isLoading: Bool
    let loadError: String?

    var body: some View {
        HStack(spacing: 9) {
            if isLoading {
                ProgressView()
                    .controlSize(.small)
                Text("正在读取账号池…")
            } else if loadError != nil {
                Image(systemName: "exclamationmark.triangle")
                Text("暂时无法读取账号池")
            } else {
                Image(systemName: "person.3")
                Text("账号池中还没有账号")
            }
        }
        .font(.callout)
        .foregroundStyle(.secondary)
        .frame(minHeight: 44, alignment: .leading)
        .help(loadError ?? "")
    }
}

private struct OverviewSub2ApiAccountGroup: Identifiable {
    let key: String
    let siteUrl: String?
    var accounts: [ManageSub2ApiAccountPoolResponse.Account]

    var id: String { key }
}

private struct OverviewSub2ApiAccountTable: View {
    let accounts: [ManageSub2ApiAccountPoolResponse.Account]
    let accountLimit: Int?
    @State private var expandedGroupKeys = Set<String>()

    private var groups: [OverviewSub2ApiAccountGroup] {
        var result: [OverviewSub2ApiAccountGroup] = []
        var indexByKey: [String: Int] = [:]
        for account in accounts {
            let key = sub2ApiAccountGroupKey(account)
            if let index = indexByKey[key] {
                result[index].accounts.append(account)
            } else {
                indexByKey[key] = result.count
                result.append(
                    OverviewSub2ApiAccountGroup(
                        key: key,
                        siteUrl: account.siteUrl,
                        accounts: [account]
                    )
                )
            }
        }
        return result
    }

    private var visibleGroups: [OverviewSub2ApiAccountGroup] {
        guard let accountLimit else { return groups }
        var result: [OverviewSub2ApiAccountGroup] = []
        var visibleAccountCount = 0
        for group in groups {
            guard visibleAccountCount < accountLimit else { break }
            result.append(group)
            visibleAccountCount += group.accounts.count
        }
        return result
    }

    var body: some View {
        VStack(spacing: 0) {
            ForEach(Array(visibleGroups.enumerated()), id: \.element.id) { index, group in
                if group.accounts.count == 1, let account = group.accounts.first {
                    OverviewSub2ApiAccountRow(account: account)
                } else {
                    DisclosureGroup(isExpanded: expansionBinding(for: group.key)) {
                        VStack(spacing: 0) {
                            ForEach(Array(group.accounts.enumerated()), id: \.element.id) { childIndex, account in
                                OverviewSub2ApiAccountRow(
                                    account: account,
                                    nested: true,
                                    showsBalance: childBalancesDiffer(in: group)
                                )
                                if childIndex < group.accounts.count - 1 {
                                    Divider()
                                }
                            }
                        }
                        .padding(.leading, 24)
                        .padding(.bottom, 2)
                    } label: {
                        OverviewSub2ApiAccountGroupSummary(group: group)
                            .contentShape(Rectangle())
                            .onTapGesture {
                                toggleExpansion(for: group.key)
                            }
                    }
                    .tint(.secondary)
                }

                if index < visibleGroups.count - 1 {
                    Divider()
                }
            }
        }
    }

    private func expansionBinding(for key: String) -> Binding<Bool> {
        Binding(
            get: { expandedGroupKeys.contains(key) },
            set: { expanded in
                if expanded {
                    expandedGroupKeys.insert(key)
                } else {
                    expandedGroupKeys.remove(key)
                }
            }
        )
    }

    private func toggleExpansion(for key: String) {
        withAnimation(.spring(response: 0.25, dampingFraction: 1)) {
            if expandedGroupKeys.contains(key) {
                expandedGroupKeys.remove(key)
            } else {
                expandedGroupKeys.insert(key)
            }
        }
    }

    private func childBalancesDiffer(in group: OverviewSub2ApiAccountGroup) -> Bool {
        Set(group.accounts.map { sub2ApiBalanceText($0.upstreamBalance) }).count > 1
    }
}

private struct OverviewSub2ApiAccountGroupSummary: View {
    let group: OverviewSub2ApiAccountGroup

    private var availableCount: Int {
        group.accounts.count(where: { account in
            account.schedulable && account.status.lowercased() == "active"
        })
    }

    private var statusText: String {
        if availableCount == group.accounts.count { return "全部可用" }
        if availableCount == 0 { return "均不可用" }
        return "\(availableCount)/\(group.accounts.count) 可用"
    }

    private var statusTint: Color {
        if availableCount == group.accounts.count { return .green }
        if availableCount == 0 { return .red }
        return .orange
    }

    private var balanceSummaryText: String {
        let values = group.accounts.map { sub2ApiBalanceText($0.upstreamBalance) }
        var uniqueValues: [String] = []
        for value in values where !uniqueValues.contains(value) {
            uniqueValues.append(value)
        }
        return uniqueValues.count > 1 ? "各渠道不同" : (uniqueValues.first ?? "—")
    }

    private var balanceTint: Color {
        guard Set(group.accounts.map { sub2ApiBalanceText($0.upstreamBalance) }).count == 1,
              let balance = group.accounts.first?.upstreamBalance,
              balance.state == "available"
        else { return .secondary }
        if balance.accountValid == false { return .orange }
        if let remaining = balance.remaining, remaining < 0 { return .red }
        return .primary
    }

    var body: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 3) {
                Text(sub2ApiSiteLabel(group.siteUrl))
                    .font(.callout.weight(.semibold))
                    .lineLimit(1)
                Text("\(group.accounts.count) 个渠道")
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
                    .lineLimit(1)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .help(group.siteUrl ?? "")

            VStack(alignment: .trailing, spacing: 3) {
                HStack(spacing: 5) {
                    Circle()
                        .fill(statusTint)
                        .frame(width: 6, height: 6)
                    Text(statusText)
                        .lineLimit(1)
                }
                .font(.caption)
                .foregroundStyle(.secondary)

                Text("余额 \(balanceSummaryText)")
                    .font(.caption.monospacedDigit().weight(.medium))
                    .foregroundStyle(balanceTint)
                    .lineLimit(1)
                    .minimumScaleFactor(0.78)
            }
            .frame(minWidth: 116, alignment: .trailing)
        }
        .padding(.vertical, 9)
        .frame(minHeight: 50)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(
            "\(sub2ApiSiteLabel(group.siteUrl))，\(group.accounts.count) 个渠道，\(statusText)，余额 \(balanceSummaryText)"
        )
    }
}

private struct OverviewSub2ApiAccountRow: View {
    let account: ManageSub2ApiAccountPoolResponse.Account
    let nested: Bool
    let showsBalance: Bool

    init(
        account: ManageSub2ApiAccountPoolResponse.Account,
        nested: Bool = false,
        showsBalance: Bool = true
    ) {
        self.account = account
        self.nested = nested
        self.showsBalance = showsBalance
    }

    private var accountTint: Color {
        guard account.schedulable else { return .red }
        switch account.status.lowercased() {
        case "active": return .green
        case "cooldown": return .orange
        default: return .red
        }
    }

    private var balanceTint: Color {
        guard account.upstreamBalance.state == "available" else { return .secondary }
        if account.upstreamBalance.accountValid == false { return .orange }
        if let remaining = account.upstreamBalance.remaining, remaining < 0 { return .red }
        return .primary
    }

    var body: some View {
        HStack(alignment: .center, spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(account.name)
                    .font(nested ? .callout : .callout.weight(.medium))
                    .lineLimit(1)
                Text(sub2ApiAccountKindText(account))
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
                    .lineLimit(1)
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            VStack(alignment: .trailing, spacing: 4) {
                HStack(spacing: 10) {
                    HStack(spacing: 5) {
                        Circle()
                            .fill(accountTint)
                            .frame(width: 6, height: 6)
                        Text(sub2ApiAccountStatusText(account))
                            .lineLimit(1)
                    }
                    .foregroundStyle(.secondary)

                    OverviewSub2ApiMetric(
                        title: "本地",
                        value: sub2ApiMultiplierText(account.localRateMultiplier)
                    )
                    OverviewSub2ApiMetric(
                        title: "上游",
                        value: sub2ApiUpstreamRateText(account.upstreamBilling),
                        symbol: account.upstreamBilling.stale ? "clock.badge.exclamationmark" : nil,
                        tint: account.upstreamBilling.stale ? .orange : nil
                    )
                    .help(sub2ApiCapabilityStateText(account.upstreamBilling.state))
                }
                .font(.caption)

                if showsBalance {
                    HStack(spacing: 4) {
                        Text("余额")
                            .foregroundStyle(.tertiary)
                        Text(sub2ApiBalanceText(account.upstreamBalance))
                            .font(.caption.monospacedDigit().weight(.medium))
                            .foregroundStyle(balanceTint)
                            .lineLimit(1)
                            .minimumScaleFactor(0.78)
                    }
                    .help(sub2ApiBalanceHelp(account.upstreamBalance))
                }
            }
            .frame(minWidth: 270, alignment: .trailing)
        }
        .padding(.vertical, nested ? 8 : 9)
        .frame(minHeight: nested ? (showsBalance ? 52 : 46) : 56)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(
            "\(account.name)，\(sub2ApiAccountStatusText(account))，本地倍率 \(sub2ApiMultiplierText(account.localRateMultiplier))，上游倍率 \(sub2ApiUpstreamRateText(account.upstreamBilling))，余额 \(sub2ApiBalanceText(account.upstreamBalance))"
        )
    }
}

private struct OverviewSub2ApiMetric: View {
    let title: String
    let value: String
    var symbol: String?
    var tint: Color?

    var body: some View {
        HStack(spacing: 3) {
            Text(title)
                .foregroundStyle(.tertiary)
            if let symbol {
                Image(systemName: symbol)
                    .font(.caption2)
                    .foregroundStyle(tint ?? .secondary)
            }
            Text(value)
                .font(.caption.monospacedDigit())
                .foregroundStyle(tint ?? .secondary)
                .lineLimit(1)
                .minimumScaleFactor(0.78)
        }
    }
}

private func sub2ApiAccountGroupKey(
    _ account: ManageSub2ApiAccountPoolResponse.Account
) -> String {
    let platform = account.platform.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    let accountType = account.accountType.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()

    guard let siteUrl = account.siteUrl,
          let components = sub2ApiSiteComponents(siteUrl),
          let normalizedUrl = components.string,
          !normalizedUrl.isEmpty
    else {
        // Accounts without a site URL must remain separate rather than being
        // grouped under a shared placeholder.
        return "account:\(account.id)"
    }

    return "site:\(normalizedUrl)|platform:\(platform)|type:\(accountType)"
}

private func sub2ApiSiteLabel(_ siteUrl: String?) -> String {
    guard let siteUrl,
          let components = sub2ApiSiteComponents(siteUrl),
          let host = components.host,
          !host.isEmpty
    else {
        return "未标注站点"
    }

    let path = components.path.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
    let port = components.port.map { ":\($0)" } ?? ""
    return path.isEmpty ? "\(host)\(port)" : "\(host)\(port)/\(path)"
}

private func sub2ApiSiteComponents(_ siteUrl: String) -> URLComponents? {
    guard var components = URLComponents(string: siteUrl),
          let scheme = components.scheme?.lowercased(),
          (scheme == "http" || scheme == "https"),
          let host = components.host?.lowercased(),
          !host.isEmpty
    else {
        return nil
    }

    components.scheme = scheme
    components.host = host
    components.user = nil
    components.password = nil
    components.query = nil
    components.fragment = nil
    if (scheme == "http" && components.port == 80)
        || (scheme == "https" && components.port == 443)
    {
        components.port = nil
    }
    components.path = components.path.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
    if !components.path.isEmpty {
        components.path = "/" + components.path
    } else {
        components.path = ""
    }
    return components
}

func sub2ApiCapabilityStateText(_ state: String) -> String {
    switch state {
    case "available": "可用"
    case "not_applicable": "不适用"
    case "not_exposed": "未提供"
    case "unsupported": "不支持"
    case "unauthorized": "未授权"
    case "forbidden": "无权限"
    case "temporarily_unavailable": "暂不可用"
    case "invalid_response": "响应异常"
    default: "未知"
    }
}

func sub2ApiMultiplierText(_ value: Double?) -> String {
    guard let value, value.isFinite else { return "—" }
    return "×\(sub2ApiCompactDecimal(value, maximumFractionDigits: 4))"
}

func sub2ApiUpstreamRateText(
    _ billing: ManageSub2ApiAccountPoolResponse.Account.Billing
) -> String {
    if let value = billing.effectiveRateMultiplier ?? billing.resolvedRateMultiplier {
        return sub2ApiMultiplierText(value)
    }
    return sub2ApiCapabilityStateText(billing.state)
}

func sub2ApiBalanceText(
    _ balance: ManageSub2ApiAccountPoolResponse.Account.Balance
) -> String {
    if balance.unlimited { return "无限" }
    if let remaining = balance.remaining, remaining.isFinite {
        let normalized = abs(remaining) < 0.005 ? 0 : remaining
        let amount = sub2ApiCompactDecimal(normalized, maximumFractionDigits: 2, minimumFractionDigits: 2)
        switch balance.unit?.uppercased() {
        case "USD": return normalized < 0 ? "-$\(amount.dropFirst())" : "$\(amount)"
        case let unit? where !unit.isEmpty: return "\(amount) \(unit)"
        default: return amount
        }
    }
    return sub2ApiCapabilityStateText(balance.state)
}

private func sub2ApiCompactDecimal(
    _ value: Double,
    maximumFractionDigits: Int,
    minimumFractionDigits: Int = 0
) -> String {
    let formatter = NumberFormatter()
    formatter.locale = Locale(identifier: "en_US_POSIX")
    formatter.numberStyle = .decimal
    formatter.usesGroupingSeparator = true
    formatter.minimumFractionDigits = minimumFractionDigits
    formatter.maximumFractionDigits = maximumFractionDigits
    formatter.roundingMode = .halfUp
    return formatter.string(from: NSNumber(value: value))
        ?? String(format: "%.*f", locale: formatter.locale, maximumFractionDigits, value)
}

private func sub2ApiAccountStatusText(
    _ account: ManageSub2ApiAccountPoolResponse.Account
) -> String {
    guard account.schedulable else { return "不可调度" }
    switch account.status.lowercased() {
    case "active": return "可用"
    case "inactive", "disabled": return "已停用"
    case "error": return "异常"
    case "cooldown": return "冷却中"
    default: return account.status.isEmpty ? "未知" : account.status
    }
}

private func sub2ApiAccountKindText(
    _ account: ManageSub2ApiAccountPoolResponse.Account
) -> String {
    let parts = [account.platform, account.accountType]
        .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
        .filter { !$0.isEmpty }
    return parts.isEmpty ? "账号 \(account.id)" : parts.joined(separator: " · ")
}

private func sub2ApiBalanceHelp(
    _ balance: ManageSub2ApiAccountPoolResponse.Account.Balance
) -> String {
    var parts = [sub2ApiCapabilityStateText(balance.state)]
    if let plan = balance.planName, !plan.isEmpty { parts.append(plan) }
    if let status = balance.accountStatus, !status.isEmpty { parts.append(status) }
    return parts.joined(separator: " · ")
}

private func sub2ApiWarningsText(_ warnings: [String]) -> String {
    warnings.map { warning in
        switch warning {
        case "billing_refresh_failed": "上游倍率刷新失败"
        case "usage_probe_not_exposed": "当前 Sub2API 未提供余额探测"
        case "usage_probe_failed": "上游余额刷新失败"
        default: "部分账号数据不可用"
        }
    }
    .reduce(into: [String]()) { result, value in
        if !result.contains(value) { result.append(value) }
    }
    .joined(separator: " · ")
}

private func sub2ApiFetchedTime(_ milliseconds: Int64) -> String {
    Date(timeIntervalSince1970: Double(milliseconds) / 1_000)
        .formatted(date: .omitted, time: .shortened)
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
