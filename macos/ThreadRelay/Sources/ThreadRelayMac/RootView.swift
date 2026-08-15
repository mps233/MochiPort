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
    @State private var manualSub2ApiRefreshTask: Task<Void, Never>?

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
                    Divider()
                    OverviewSub2ApiAccountPoolSummary(
                        admin: model.sub2ApiAdmin,
                        pool: model.sub2ApiAccountPool,
                        isLoading: model.sub2ApiAccountPoolLoading,
                        loadError: model.sub2ApiAccountPoolError,
                        onRefresh: {
                            manualSub2ApiRefreshTask?.cancel()
                            manualSub2ApiRefreshTask = Task {
                                await model.refreshSub2ApiAccountPool(
                                    forceBillingRefresh: true
                                )
                            }
                        },
                        onConnect: { model.selection = .gateway }
                    )
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
                            detail: runtimeDetail(lifecycle),
                            symbol: "shippingbox",
                            tint: lifecycle.management.canControl ? .positive : .secondary
                        )
                        if model.daemonBuildMismatch {
                            Divider()
                            OverviewStatusRow(
                                title: model.daemonUpgradePending ? "后台升级" : "版本一致性",
                                detail: model.daemonUpgradePending
                                    ? model.daemonUpgradeDetail
                                    : "界面与后台服务构建不一致",
                                symbol: "exclamationmark.triangle",
                                tint: .caution
                            )
                        }
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

            }
            .frame(maxWidth: 860, alignment: .leading)
            .padding(ThreadRelaySpacing.page)
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
            "后台升级：\(model.daemonUpgradePending ? model.daemonUpgradeDetail : "无")",
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

private struct OverviewSub2ApiAccountPoolSummary: View {
    let admin: ManageSub2ApiAdmin?
    let pool: ManageSub2ApiAccountPoolResponse.Pool?
    let isLoading: Bool
    let loadError: String?
    let onRefresh: () -> Void
    let onConnect: () -> Void
    @State private var showsAllAccounts = false

    private var configured: Bool { admin?.configured == true }
    private let collapsedAccountLimit = 6

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 10) {
                Text("Sub2API 账号池")
                    .font(.subheadline.weight(.medium))
                if let accounts = pool?.accounts, !accounts.isEmpty {
                    Text("\(accounts.count)")
                        .font(.caption.monospacedDigit().weight(.medium))
                        .foregroundStyle(.secondary)
                }
                Spacer(minLength: ThreadRelaySpacing.standard)
                if configured {
                    Button(action: onRefresh) {
                        ZStack {
                            Image(systemName: "arrow.clockwise")
                                .opacity(isLoading ? 0 : 1)
                            ProgressView()
                                .controlSize(.small)
                                .opacity(isLoading ? 1 : 0)
                        }
                        .frame(width: 18, height: 18)
                    }
                    .buttonStyle(.plain)
                    .disabled(isLoading)
                    .help("刷新账号余额与倍率")
                    .accessibilityLabel("刷新 Sub2API 账号池")
                }
            }

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
                    accounts: showsAllAccounts
                        ? pool.accounts
                        : Array(pool.accounts.prefix(collapsedAccountLimit))
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
                                : "显示另外 \(pool.accounts.count - collapsedAccountLimit) 个账号",
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
                    Text("余额按账号展示，多个账号可能共享同一钱包")
                }
                .font(.caption2)
                .foregroundStyle(.tertiary)
                .lineLimit(1)
            }
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

private enum OverviewSub2ApiColumns {
    static let status: CGFloat = 86
    static let localRate: CGFloat = 72
    static let upstreamRate: CGFloat = 82
    static let balance: CGFloat = 118
    static let spacing: CGFloat = 12
}

private struct OverviewSub2ApiAccountTable: View {
    let accounts: [ManageSub2ApiAccountPoolResponse.Account]

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: OverviewSub2ApiColumns.spacing) {
                OverviewSub2ApiHeader("账号")
                    .frame(maxWidth: .infinity, alignment: .leading)
                OverviewSub2ApiHeader("状态")
                    .frame(width: OverviewSub2ApiColumns.status, alignment: .leading)
                OverviewSub2ApiHeader("本地倍率")
                    .frame(width: OverviewSub2ApiColumns.localRate, alignment: .trailing)
                OverviewSub2ApiHeader("上游倍率")
                    .frame(width: OverviewSub2ApiColumns.upstreamRate, alignment: .trailing)
                OverviewSub2ApiHeader("余额 / 额度")
                    .frame(width: OverviewSub2ApiColumns.balance, alignment: .trailing)
            }
            .padding(.vertical, 7)

            Divider()

            ForEach(Array(accounts.enumerated()), id: \.element.id) { index, account in
                OverviewSub2ApiAccountRow(account: account)
                if index < accounts.count - 1 {
                    Divider()
                }
            }
        }
    }
}

private struct OverviewSub2ApiHeader: View {
    let title: String

    init(_ title: String) {
        self.title = title
    }

    var body: some View {
        Text(title)
            .font(.caption.weight(.medium))
            .foregroundStyle(.secondary)
            .lineLimit(1)
    }
}

private struct OverviewSub2ApiAccountRow: View {
    let account: ManageSub2ApiAccountPoolResponse.Account

    private var accountTint: Color {
        account.schedulable && account.status.lowercased() == "active" ? .green : .orange
    }

    private var balanceTint: Color {
        guard account.upstreamBalance.state == "available" else { return .secondary }
        if account.upstreamBalance.accountValid == false { return .orange }
        if let remaining = account.upstreamBalance.remaining, remaining < 0 { return .red }
        return .primary
    }

    var body: some View {
        HStack(spacing: OverviewSub2ApiColumns.spacing) {
            VStack(alignment: .leading, spacing: 2) {
                Text(account.name)
                    .font(.callout.weight(.medium))
                    .lineLimit(1)
                Text(sub2ApiAccountKindText(account))
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
                    .lineLimit(1)
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            HStack(spacing: 5) {
                Circle()
                    .fill(accountTint)
                    .frame(width: 6, height: 6)
                Text(sub2ApiAccountStatusText(account))
                    .lineLimit(1)
            }
            .font(.caption)
            .foregroundStyle(account.schedulable ? Color.secondary : Color.orange)
            .frame(width: OverviewSub2ApiColumns.status, alignment: .leading)

            Text(sub2ApiMultiplierText(account.localRateMultiplier))
                .font(.callout.monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(width: OverviewSub2ApiColumns.localRate, alignment: .trailing)

            HStack(spacing: 4) {
                if account.upstreamBilling.stale {
                    Image(systemName: "clock.badge.exclamationmark")
                        .font(.caption2)
                        .foregroundStyle(.orange)
                }
                Text(sub2ApiUpstreamRateText(account.upstreamBilling))
                    .font(.callout.monospacedDigit())
                    .lineLimit(1)
            }
            .frame(width: OverviewSub2ApiColumns.upstreamRate, alignment: .trailing)
            .help(sub2ApiCapabilityStateText(account.upstreamBilling.state))

            Text(sub2ApiBalanceText(account.upstreamBalance))
                .font(.callout.monospacedDigit().weight(.medium))
                .foregroundStyle(balanceTint)
                .lineLimit(1)
                .minimumScaleFactor(0.78)
                .frame(width: OverviewSub2ApiColumns.balance, alignment: .trailing)
                .help(sub2ApiBalanceHelp(account.upstreamBalance))
        }
        .padding(.vertical, 8)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(
            "\(account.name)，\(sub2ApiAccountStatusText(account))，本地倍率 \(sub2ApiMultiplierText(account.localRateMultiplier))，上游倍率 \(sub2ApiUpstreamRateText(account.upstreamBilling))，余额 \(sub2ApiBalanceText(account.upstreamBalance))"
        )
    }
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
