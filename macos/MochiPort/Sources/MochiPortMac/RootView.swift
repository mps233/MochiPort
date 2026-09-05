import AppKit
import SwiftUI

struct RootView: View {
    @EnvironmentObject private var model: AppModel
    @State private var showsAccountOnboarding = false
    @State private var opensGatewayProviders = false

    var body: some View {
        let liveAccounts = model.imAccounts
        let projectGroupAccounts = model.telegramProjectGroupAccounts
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
            .listStyle(.sidebar)
            .scrollContentBackground(.hidden)
            .safeAreaInset(edge: .bottom, spacing: 0) {
                MochiPortSidebarFooter()
            }
            .navigationTitle("MochiPort")
            .navigationSplitViewColumnWidth(min: 190, ideal: 220, max: 260)
        } detail: {
            Group {
                switch model.selection ?? .overview {
                case .overview:
                    OverviewView(
                        onOpenGateway: {
                            opensGatewayProviders = true
                            model.selection = .gateway
                        },
                        onOpenMessaging: { model.selection = .messaging },
                        onOpenCodex: { model.selection = .codex },
                        onOpenSettings: {
                            NSApp.sendAction(
                                Selector(("showSettingsWindow:")),
                                to: nil,
                                from: nil
                            )
                        }
                    )
                case .codex:
                    CodexAccessView(onOpenGateway: { model.selection = .gateway })
                case .sessions:
                    SessionsView()
                case .requestLogs:
                    RequestLogsView()
                case .messaging:
                    MessagingAccountsView(
                        accounts: liveAccounts.compactMap(MessagingAccountSummary.init),
                        telegramProjectGroupAccounts: projectGroupAccounts,
                        availability: model.imAccountsAvailability,
                        onAdd: { showsAccountOnboarding = true },
                        onToggle: { account, enabled in
                            let live = matchingLiveAccount(account, in: liveAccounts)
                            guard let live else { return false }
                            return await model.setIMAccountEnabled(live, enabled: enabled)
                        },
                        onDelete: { account in
                            let live = matchingLiveAccount(account, in: liveAccounts)
                            guard let live else { return }
                            Task { await model.deleteIMAccount(live) }
                        },
                        onSaveTelegramProjectGroups: { accountId, groups in
                            await model.saveTelegramProjectGroups(accountId: accountId, projectGroups: groups)
                        },
                        onSyncTelegramTopics: { accountId, chatId in
                            await model.syncTelegramTopics(accountId: accountId, chatId: chatId)
                        },
                        onReplyGranularity: { account, granularity in
                            await model.setTelegramReplyGranularity(
                                accountId: account.accountID,
                                granularity: granularity
                            )
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
                    GatewayView(
                        startAtProviders: opensGatewayProviders,
                        startAddingProvider: opensGatewayProviders
                    )
                case .accountPool:
                    AccountPoolView()
                }
            }
            .navigationTitle((model.selection ?? .overview).title)
            // Let macOS 26 provide the title-bar material and scroll-edge
            // treatment. A hand-painted background or gradient here prevents
            // the toolbar from sampling the live content beneath it.
            .toolbarBackgroundVisibility(.hidden, for: .automatic)
            .scrollEdgeEffectStyle(.soft, for: .top)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
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
        }
        .task {
            await model.startAtAppLaunch()
        }
        .toolbar {
            if model.selection == .overview {
                ToolbarItem(placement: .primaryAction) {
                    OverviewStatusToolbarButton(
                        status: model.serviceStatus,
                        dashboardState: model.dashboardState,
                        dashboard: model.dashboard,
                        lastCheckedAt: model.lastCheckedAt,
                        recoveryInProgress: model.daemonRecoveryInProgress,
                        recoveryError: model.daemonRecoveryError,
                        onRecover: { Task { await model.startDaemonManually() } }
                    )
                }
            }
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
        .onChange(of: model.selection) { _, selection in
            if selection != .gateway {
                opensGatewayProviders = false
            }
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

    private func matchingLiveAccount(
        _ account: MessagingAccountSummary,
        in accounts: [ManageIMAccount]
    ) -> ManageIMAccount? {
        accounts.first(where: { live in
            live.platform == account.platform.rawValue && live.accountId == account.accountID
        })
    }
}

private struct MochiPortSidebarFooter: View {
    private var appName: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleDisplayName") as? String
            ?? Bundle.main.object(forInfoDictionaryKey: "CFBundleName") as? String
            ?? "MochiPort"
    }

    private var appVersion: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String
            ?? "开发版"
    }

    var body: some View {
        VStack(spacing: 0) {
            Divider()

            HStack(spacing: 10) {
                ZStack {
                    Circle()
                        .fill(Color(nsColor: .controlBackgroundColor))

                    Image(nsImage: NSApp.applicationIconImage)
                        .resizable()
                        .interpolation(.high)
                        .scaledToFill()
                        .frame(width: 46, height: 46)
                        .frame(width: 34, height: 34)
                        .mask(Circle())
                }
                .frame(width: 40, height: 40)
                .overlay {
                    Circle()
                        .stroke(Color.white.opacity(0.45), lineWidth: 1)
                }

                VStack(alignment: .leading, spacing: 3) {
                    Text(appName)
                        .font(.callout.weight(.semibold))
                        .lineLimit(1)

                    Text("Version \(appVersion)")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }

                Spacer(minLength: 0)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityElement(children: .combine)
            .accessibilityIdentifier("sidebar.appIdentity")
        }
    }
}

private struct OverviewView: View {
    @EnvironmentObject private var model: AppModel
    @EnvironmentObject private var glass: AIGlassCoordinator
    let onOpenGateway: () -> Void
    let onOpenMessaging: () -> Void
    let onOpenCodex: () -> Void
    let onOpenSettings: () -> Void

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: MochiPortPageLayout.sectionSpacing) {
                if model.hasAvailableUnifiedUpdate,
                   !model.unifiedUpdateNoticeDismissed
                {
                    UnifiedUpdateEntry(
                        context: .overview,
                        onDismiss: { model.unifiedUpdateNoticeDismissed = true }
                    )
                }

                OverviewStartHereView(
                    onOpenGateway: onOpenGateway,
                    onOpenMessaging: onOpenMessaging,
                    onOpenCodex: onOpenCodex
                )

                OverviewUsageInsightsView(
                    store: glass.store,
                    statsStore: glass.statsStore,
                    providerUsage: model.gatewayProviderUsage
                )
                .task {
                    while !Task.isCancelled {
                        await model.refreshGatewayProviderUsage()
                        try? await Task.sleep(for: .seconds(60))
                    }
                }

                ConnectionTopologyView()

                OverviewAccountPoolSummaryRow(
                    admin: model.sub2ApiAdmin,
                    pool: model.sub2ApiAccountPool,
                    isLoading: model.sub2ApiAccountPoolLoading,
                    loadError: model.sub2ApiAccountPoolError,
                    onOpen: { model.selection = .accountPool }
                )
            }
            .frame(maxWidth: MochiPortPageLayout.maxContentWidth, alignment: .leading)
            .padding(.top, MochiPortPageLayout.topPadding)
            .padding(.bottom, MochiPortPageLayout.bottomPadding)
        }
        // Keep the scroll view edge-to-edge so the system sidebar glass can
        // sample live detail content underneath it; the resting inset still
        // keeps cards and headings clear of the sidebar via scroll margins.
        .contentMargins(
            .horizontal,
            MochiPortPageLayout.horizontalPadding,
            for: .scrollContent
        )
        .scrollIndicators(.never)
        .task {
            await model.refreshSub2ApiAccountPool()
        }
        .onDisappear {
            model.cancelSub2ApiAccountPoolRefresh()
        }
    }
}

private struct OverviewStartHereView: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    let onOpenGateway: () -> Void
    let onOpenMessaging: () -> Void
    let onOpenCodex: () -> Void
    @AppStorage("overview.startHereExpanded") private var isExpanded = true
    @State private var isHeaderHovered = false

    private let startStepColumns = [
        GridItem(.flexible(minimum: 180), spacing: 22, alignment: .topLeading),
        GridItem(.flexible(minimum: 180), spacing: 22, alignment: .topLeading),
    ]

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Button {
                let animation: Animation = reduceMotion
                    ? .easeOut(duration: 0.16)
                    : .spring(response: 0.28, dampingFraction: 1)
                withAnimation(animation) {
                    isExpanded.toggle()
                }
            } label: {
                HStack(alignment: isExpanded ? .top : .center, spacing: 12) {
                    Image(systemName: "point.3.connected.trianglepath.dotted")
                        .font(.system(size: 19, weight: .medium))
                        .symbolRenderingMode(.hierarchical)
                        .foregroundStyle(.secondary)
                        .frame(width: 34, height: 34)
                        .accessibilityHidden(true)

                    VStack(alignment: .leading, spacing: isExpanded ? 4 : 1) {
                        HStack(spacing: 6) {
                            Text("从这里开始")
                                .font(.headline.weight(.semibold))

                            if !isExpanded {
                                Text("·")
                                    .foregroundStyle(.tertiary)
                                Text("4 步")
                                    .font(.caption2.weight(.semibold))
                                    .foregroundStyle(.tertiary)
                                    .transition(.opacity)
                            }
                        }

                        if isExpanded {
                            Text("MochiPort 把电脑上的 Codex 连接到 Telegram、飞书等消息软件。你在手机里发消息，Codex 在电脑上完成任务。")
                                .font(.callout)
                                .foregroundStyle(.secondary)
                                .fixedSize(horizontal: false, vertical: true)
                                .transition(
                                    reduceMotion
                                        ? .opacity
                                        : .opacity.combined(with: .move(edge: .top))
                                )
                        } else {
                            Text("连接模型、消息渠道与 Codex")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                                .lineLimit(1)
                                .transition(.opacity)
                        }
                    }
                    .layoutPriority(1)

                    Spacer(minLength: 12)

                    Image(systemName: "chevron.down")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(.secondary)
                        .rotationEffect(.degrees(isExpanded ? 180 : 0))
                        .frame(width: 26, height: 26)
                        .background(
                            Color.primary.opacity(isHeaderHovered ? 0.06 : 0),
                            in: Circle()
                        )
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .onHover { isHeaderHovered = $0 }
            .help(isExpanded ? "收起使用引导" : "展开使用引导")
            .accessibilityLabel(isExpanded ? "收起使用引导" : "展开使用引导")
            .accessibilityValue(isExpanded ? "已展开" : "已收起")
            .accessibilityIdentifier("overview.start-here.toggle")

            if isExpanded {
                Divider()
                    .opacity(0.55)

                Text("第一次使用只需要四步")
                    .font(.headline)

                LazyVGrid(columns: startStepColumns, alignment: .leading, spacing: 18) {
                    startStep(
                        number: "1",
                        title: "添加模型服务",
                        detail: "填写 API 地址和 Key，保存即可。",
                        action: "配置模型",
                        symbol: "server.rack",
                        isProminent: true,
                        onAction: onOpenGateway
                    )
                    startStep(
                        number: "2",
                        title: "连接消息渠道",
                        detail: "连接 Telegram、飞书、微信或企业微信中的一个。",
                        action: "连接消息渠道",
                        symbol: "message.badge.waveform",
                        isProminent: false,
                        onAction: onOpenMessaging
                    )
                    startStep(
                        number: "3",
                        title: "连接 Codex",
                        detail: "打开开关，让 Codex 连接 MochiPort。",
                        action: "连接 Codex",
                        symbol: "link",
                        isProminent: false,
                        onAction: onOpenCodex
                    )
                    startStep(
                        number: "4",
                        title: "从消息软件开始使用",
                        detail: "在手机上给机器人发一条消息。",
                        action: nil,
                        symbol: "checkmark.circle",
                        isProminent: false,
                        onAction: nil
                    )
                }

                HStack(alignment: .center, spacing: 10) {
                    Label("至少连接一个消息软件，之后就能在手机上使用 Codex。", systemImage: "info.circle")
                        .font(.footnote)
                        .foregroundStyle(.tertiary)
                        .fixedSize(horizontal: false, vertical: true)

                    Spacer(minLength: 12)
                }
            }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, isExpanded ? 20 : 9)
        .frame(maxWidth: .infinity, alignment: .leading)
        .startHereGlassSurface()
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("overview.start-here")
    }

    @ViewBuilder
    private func startStep(
        number: String,
        title: String,
        detail: String,
        action: String?,
        symbol: String,
        isProminent: Bool,
        onAction: (() -> Void)?
    ) -> some View {
        HStack(alignment: .top, spacing: 9) {
            Text(number)
                .font(.caption.weight(.semibold).monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(width: 26, height: 26)
                .background(Color.primary.opacity(0.045), in: Circle())
                .overlay {
                    Circle()
                        .strokeBorder(Color.primary.opacity(0.14), lineWidth: 0.5)
                }

            VStack(alignment: .leading, spacing: 5) {
                Text(title)
                    .font(.callout.weight(.semibold))
                Text(detail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)

                if let action, let onAction {
                    if isProminent {
                        Button {
                            onAction()
                        } label: {
                            Label(action, systemImage: symbol)
                        }
                        .buttonStyle(.borderedProminent)
                        .buttonBorderShape(.capsule)
                        .tint(Color.accentColor)
                        .controlSize(.small)
                    } else {
                        Button {
                            onAction()
                        } label: {
                            Label(action, systemImage: symbol)
                        }
                        .buttonStyle(.link)
                        .controlSize(.small)
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

}

private extension View {
    @ViewBuilder
    func startHereGlassSurface() -> some View {
        let shape = RoundedRectangle(
            cornerRadius: MochiPortRadius.overlay,
            style: .continuous
        )

        if #available(macOS 26.0, *) {
            self.glassEffect(.regular, in: shape)
        } else {
            self
                .background(.regularMaterial, in: shape)
                .overlay {
                    shape.strokeBorder(
                        Color.primary.opacity(0.11),
                        lineWidth: 0.5
                    )
                }
        }
    }
}

enum UnifiedUpdateEntryContext {
    case overview
    case settings
}

/// Shared update entry used by the overview banner and the detailed Settings
/// pane. Installing a newer GUI bundle also carries the matching daemon; the
/// next launch performs the protected-work and lease checks before switching.
struct UnifiedUpdateEntry: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.openURL) private var openURL

    let context: UnifiedUpdateEntryContext
    let onDismiss: (() -> Void)?

    init(
        context: UnifiedUpdateEntryContext,
        onDismiss: (() -> Void)? = nil
    ) {
        self.context = context
        self.onDismiss = onDismiss
    }

    private var state: UnifiedUpdateState {
        model.unifiedUpdateState
    }

    private var title: String {
        switch state {
        case .notChecked: "尚未检查更新"
        case .checking: "正在检查更新"
        case .upToDate: "MochiPort 已是最新版本"
        case .failed: "更新检查失败"
        case .ui: "MochiPort 有新版本"
        case .daemon: "后台服务有更新"
        case .both: "MochiPort 有更新"
        }
    }

    private var subtitle: String {
        switch state {
        case .notChecked:
            return "点击“检查更新”获取界面和后台服务的最新版本"
        case .checking:
            return "正在获取界面和后台服务的最新版本"
        case .upToDate:
            return "界面和后台服务均已是最新版本"
        case let .failed(message):
            return message
        case let .ui(update):
            return "界面版本 \(update.version) 可下载"
        case let .daemon(update):
            return daemonSubtitle(update: update)
        case let .both(ui, daemon):
            let daemonText = daemonSubtitle(update: daemon)
            return "界面 \(ui.version) · 后台服务 \(daemon.version) · \(daemonText)"
        }
    }

    private var symbolName: String {
        switch state {
        case .notChecked: "arrow.clockwise.circle"
        case .checking: "arrow.triangle.2.circlepath"
        case .upToDate: "checkmark.circle.fill"
        case .failed: "exclamationmark.triangle.fill"
        case .ui, .daemon, .both: "arrow.down.circle.fill"
        }
    }

    private var symbolColor: Color {
        switch state {
        case .failed: .red
        case .upToDate: .green
        default: .blue
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: context == .overview ? 10 : 8) {
            HStack(alignment: .top, spacing: 10) {
                Image(systemName: symbolName)
                    .symbolRenderingMode(.hierarchical)
                    .foregroundStyle(symbolColor)
                    .frame(width: 20, height: 20)

                VStack(alignment: .leading, spacing: 3) {
                    Text(title)
                        .font(.callout.weight(.semibold))
                    Text(subtitle)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(context == .overview ? 2 : nil)
                }

                Spacer(minLength: 8)

                if let onDismiss {
                    Button(action: onDismiss) {
                        Image(systemName: "xmark")
                    }
                    .buttonStyle(.plain)
                    .foregroundStyle(.secondary)
                    .help("本次会话不再提示")
                    .accessibilityLabel("关闭更新提示")
                }
            }

            if model.hasAvailableUnifiedUpdate {
                updateActions
            }
        }
        .padding(context == .overview ? 12 : 0)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background {
            if context == .overview {
                RoundedRectangle(cornerRadius: 10)
                    .fill(.quaternary.opacity(0.42))
            }
        }
        .overlay {
            if context == .overview {
                RoundedRectangle(cornerRadius: 10)
                    .stroke(.quaternary, lineWidth: 0.8)
            }
        }
        .accessibilityIdentifier(
            context == .overview ? "overview.unified-update-entry" : "settings.unified-update-entry"
        )
    }

    @ViewBuilder
    private var updateActions: some View {
        VStack(alignment: .leading, spacing: 6) {
            if let update = model.availableUIUpdate {
                updateActionRow(
                    label: "界面",
                    version: update.version,
                    buttonTitle: "下载界面更新",
                    action: { openReleasePage(for: update) },
                    disabled: update.validatedReleaseURL == nil
                )
            }

            if let update = model.availableDaemonUpdate {
                updateActionRow(
                    label: "后台服务",
                    version: update.version,
                    buttonTitle: "查看发布页",
                    action: { openReleasePage(for: update) },
                    disabled: update.validatedReleaseURL == nil
                )
            }
        }
    }

    @ViewBuilder
    private func updateActionRow(
        label: String,
        version: String,
        buttonTitle: String,
        action: @escaping () -> Void,
        disabled: Bool
    ) -> some View {
        HStack(spacing: 8) {
            Text(label)
                .font(.caption.weight(.medium))
                .foregroundStyle(.secondary)
                .frame(width: 58, alignment: .leading)
            Text(version)
                .font(.caption)
                .foregroundStyle(.primary)
            Spacer(minLength: 8)
            Button(buttonTitle, action: action)
                .controlSize(context == .overview ? .small : .regular)
                .disabled(disabled)
        }
    }

    private func openReleasePage(for update: UpdateComponentRelease) {
        guard let url = update.validatedReleaseURL else { return }
        openURL(url)
    }

    private func daemonSubtitle(update: UpdateComponentRelease) -> String {
        let build = update.build.map { "（构建 \($0)）" } ?? ""
        return "后台服务 \(update.version)\(build) 随新版 MochiPort 安装；启动后会自动安全切换"
    }
}

private struct OverviewStatusToolbarButton: View {
    let status: ServiceStatus
    let dashboardState: DashboardState
    let dashboard: ManageDashboard?
    let lastCheckedAt: Date?
    let recoveryInProgress: Bool
    let recoveryError: String?
    let onRecover: () -> Void
    @State private var isPresented = false

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
        default: "检查中"
        }
    }

    private var remoteDetail: String {
        guard let dashboard else { return unavailableText }
        return dashboard.remoteControlHealthy
            ? "状态正常"
            : dashboard.remoteControlConnected ? "已连接" : "离线"
    }

    private var bridgeDetail: String {
        guard let dashboard else { return unavailableText }
        return dashboard.bridgeRunning ? "运行中" : "未运行"
    }

    private var showsRecovery: Bool {
        if case .unavailable = status { return true }
        return false
    }

    private var toolbarSymbol: String {
        switch status {
        case .checking: "arrow.clockwise"
        case .available: "server.rack"
        case .unavailable: "exclamationmark.triangle"
        }
    }

    var body: some View {
        Button {
            isPresented.toggle()
        } label: {
            Label("服务状态", systemImage: toolbarSymbol)
        }
        .help("查看服务状态")
        .accessibilityLabel("服务状态：\(status.title)")
        .accessibilityIdentifier("overview.service-status-button")
        .popover(isPresented: $isPresented, attachmentAnchor: .rect(.bounds), arrowEdge: .top) {
            VStack(alignment: .leading, spacing: 12) {
                HStack(spacing: 9) {
                    Image(systemName: status.symbol)
                        .font(.system(size: 15, weight: .semibold))
                        .symbolRenderingMode(.hierarchical)
                        .foregroundStyle(status.tint.color)
                    VStack(alignment: .leading, spacing: 2) {
                        Text("服务状态")
                            .font(.headline)
                        Text(status.title)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    Spacer(minLength: 8)
                    if let lastCheckedAt {
                        Text(lastCheckedAt.formatted(date: .omitted, time: .shortened))
                            .font(.caption.monospacedDigit())
                            .foregroundStyle(.secondary)
                    }
                }

                OverviewSignalStrip(
                    dashboard: dashboard,
                    dashboardState: dashboardState,
                    serviceStatus: status,
                    remoteDetail: remoteDetail,
                    bridgeDetail: bridgeDetail
                )

                if showsRecovery {
                    Button {
                        onRecover()
                    } label: {
                        if recoveryInProgress {
                            Label("正在启动…", systemImage: "arrow.clockwise")
                        } else {
                            Label("启动本地服务", systemImage: "play.circle.fill")
                        }
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(recoveryInProgress)
                    .accessibilityIdentifier("overview.recover-daemon")
                }
                if let recoveryError {
                    Text(recoveryError)
                        .font(.caption)
                        .foregroundStyle(.red)
                        .lineLimit(3)
                }
            }
            .padding(14)
            .frame(width: 310)
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

private struct OverviewAccountPoolSummaryRow: View {
    let admin: ManageSub2ApiAdmin?
    let pool: ManageSub2ApiAccountPoolResponse.Pool?
    let isLoading: Bool
    let loadError: String?
    let onOpen: () -> Void
    @State private var isHovered = false

    var body: some View {
        Button(action: onOpen) {
            HStack(spacing: 12) {
                Image(systemName: "person.3.sequence")
                    .font(.system(size: 19, weight: .medium))
                    .symbolRenderingMode(.hierarchical)
                    .foregroundStyle(.secondary)
                    .frame(width: 34, height: 34)
                    .accessibilityHidden(true)

                VStack(alignment: .leading, spacing: 2) {
                    Text("Sub2API 账号池")
                        .font(.callout.weight(.semibold))
                        .foregroundStyle(.primary)
                    Text(detailText)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }

                Spacer(minLength: 12)

                if let dotTint {
                    Circle()
                        .fill(dotTint)
                        .frame(width: 7, height: 7)
                        .accessibilityHidden(true)
                }

                Image(systemName: "chevron.right")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.secondary)
                    .frame(width: 26, height: 26)
                    .background(
                        Color.primary.opacity(isHovered ? 0.06 : 0),
                        in: Circle()
                    )
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { isHovered = $0 }
        .animation(.easeOut(duration: 0.14), value: isHovered)
        .startHereGlassSurface()
        .help("打开账号池页面")
        .accessibilityLabel("Sub2API 账号池，\(detailText)")
        .accessibilityIdentifier("overview.sub2api-account-pool")
    }

    private var detailText: String {
        if admin?.configured != true {
            return "未连接 · 点击去连接"
        }
        if let pool, !pool.accounts.isEmpty {
            let summary = sub2ApiPoolSummary(pool.accounts)
            var parts = [
                "\(summary.total) 个账号",
                "\(summary.available)/\(summary.total) 可用",
            ]
            if summary.attention > 0 {
                parts.append("\(summary.attention) 异常")
            }
            parts.append("余额 \(summary.balanceText)")
            return parts.joined(separator: " · ")
        }
        if isLoading {
            return "正在读取账号池…"
        }
        if loadError != nil {
            return "暂时无法读取账号池"
        }
        return "账号池中还没有账号"
    }

    private var dotTint: Color? {
        guard let pool, !pool.accounts.isEmpty else { return nil }
        let summary = sub2ApiPoolSummary(pool.accounts)
        if summary.attention > 0 { return .red }
        if summary.available == summary.total { return Theme.safeGreen }
        return .orange
    }
}
