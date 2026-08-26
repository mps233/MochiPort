import AppKit
import SwiftUI

/// A UI-facing snapshot of one configured messaging account.
///
/// The management API is intentionally kept out of this view.  The parent
/// model can map its response into this value and receive mutations through
/// the optional callbacks on `MessagingAccountsView`.
struct MessagingAccountSummary: Identifiable, Equatable {
    enum Platform: String, CaseIterable, Identifiable {
        case feishu
        case telegram
        case wechat
        case wecom

        var id: String { rawValue }

        var title: String {
            switch self {
            case .feishu: "飞书"
            case .telegram: "Telegram"
            case .wechat: "微信"
            case .wecom: "企业微信"
            }
        }

        var symbol: String {
            switch self {
            case .feishu: "bubble.left.and.bubble.right"
            case .telegram: "paperplane"
            case .wechat: "message"
            case .wecom: "person.2"
            }
        }

    }

    let platform: Platform
    let accountID: String
    let displayName: String?
    let avatarData: String?
    let enabled: Bool
    let configured: Bool
    let secretSet: Bool
    let connecting: Bool
    let polling: Bool
    let connected: Bool
    let lastError: String?
    let lastEventAt: Date?
    let lastInboundAt: Date?

    var id: String { "\(platform.rawValue):\(accountID)" }

    init(
        platform: Platform,
        accountID: String,
        displayName: String? = nil,
        avatarData: String? = nil,
        enabled: Bool = true,
        configured: Bool = true,
        secretSet: Bool = true,
        connecting: Bool = false,
        polling: Bool = false,
        connected: Bool = false,
        lastError: String? = nil,
        lastEventAt: Date? = nil,
        lastInboundAt: Date? = nil
    ) {
        self.platform = platform
        self.accountID = accountID
        self.displayName = displayName
        self.avatarData = avatarData
        self.enabled = enabled
        self.configured = configured
        self.secretSet = secretSet
        self.connecting = connecting
        self.polling = polling
        self.connected = connected
        self.lastError = lastError
        self.lastEventAt = lastEventAt
        self.lastInboundAt = lastInboundAt
    }

    init?(_ account: ManageIMAccount) {
        guard let platform = Platform(rawValue: account.platform) else {
            return nil
        }
        self.init(
            platform: platform,
            accountID: account.accountId,
            displayName: account.displayName,
            avatarData: account.avatarData,
            enabled: account.enabled,
            configured: account.configured,
            secretSet: account.secretSet,
            connecting: account.connecting,
            polling: account.polling,
            connected: account.connected,
            lastError: account.lastError,
            lastEventAt: account.lastEventAtMs.map { Date(timeIntervalSince1970: TimeInterval($0) / 1_000) },
            lastInboundAt: account.lastInboundAtMs.map { Date(timeIntervalSince1970: TimeInterval($0) / 1_000) }
        )
    }

    /// Safe, deterministic data for SwiftUI previews and visual review.
    /// No real credentials or user identifiers are included.
    static let previewAccounts: [Self] = [
        Self(
            platform: .telegram,
            accountID: "telegram-main",
            displayName: "主 Telegram",
            polling: true,
            connected: true,
            lastEventAt: Date(timeIntervalSince1970: 1_754_000_120),
            lastInboundAt: Date(timeIntervalSince1970: 1_754_000_080)
        ),
        Self(
            platform: .feishu,
            accountID: "feishu-work",
            displayName: "工作空间",
            connected: true,
            lastEventAt: Date(timeIntervalSince1970: 1_754_000_060),
            lastInboundAt: Date(timeIntervalSince1970: 1_753_999_990)
        ),
        Self(
            platform: .wechat,
            accountID: "wechat-support",
            displayName: "客服微信",
            connecting: true,
            lastEventAt: Date(timeIntervalSince1970: 1_753_999_800)
        ),
        Self(
            platform: .wecom,
            accountID: "wecom-ops",
            displayName: "运营群机器人",
            enabled: false,
            configured: false,
            secretSet: false,
            lastError: "尚未完成凭据配置"
        ),
    ]
}

private enum MessagingAccountState: Equatable {
    case disabled
    case connected
    case connecting
    case error
    case incomplete
    case offline

    var title: String {
        switch self {
        case .disabled: "已停用"
        case .connected: "已连接"
        case .connecting: "连接中"
        case .error: "连接异常"
        case .incomplete: "待配置"
        case .offline: "未连接"
        }
    }

    var tint: Color {
        switch self {
        case .disabled, .offline, .incomplete: .secondary
        case .connected: .green
        case .connecting: .orange
        case .error: .red
        }
    }
}

/// Displays a daemon-fetched account avatar and falls back to the bundled
/// platform symbol when the remote profile is unavailable.
struct MessagingAccountAvatar: View {
    let account: MessagingAccountSummary
    var size: CGFloat = 28

    var body: some View {
        Group {
            if let image = image {
                Image(nsImage: image)
                    .resizable()
                    .scaledToFill()
            } else {
                Image(systemName: account.platform.symbol)
                    .font(.system(size: size * 0.52, weight: .semibold))
                    .foregroundStyle(.primary.opacity(0.72))
                    .frame(width: size, height: size)
                    .background(.quaternary)
            }
        }
        .frame(width: size, height: size)
        .clipShape(Circle())
        .overlay {
            Circle().strokeBorder(Color.primary.opacity(0.08), lineWidth: 0.5)
        }
        .accessibilityHidden(true)
    }

    private var image: NSImage? {
        guard let avatarData = account.avatarData,
              let separator = avatarData.firstIndex(of: ","),
              let data = Data(base64Encoded: String(avatarData[avatarData.index(after: separator)...]))
        else { return nil }
        return NSImage(data: data)
    }
}

private enum MessagingAccountFilter: String, CaseIterable, Identifiable, ThreadRelaySegmentItem {
    case all
    case feishu
    case telegram
    case wechat
    case wecom

    var id: String { rawValue }

    var title: String {
        switch self {
        case .all: "全部"
        case .feishu: "飞书"
        case .telegram: "Telegram"
        case .wechat: "微信"
        case .wecom: "企业微信"
        }
    }

    var symbol: String {
        switch self {
        case .all: "rectangle.stack.fill"
        case .feishu: "bubble.left.and.bubble.right"
        case .telegram: "paperplane.fill"
        case .wechat: "message.fill"
        case .wecom: "person.2.fill"
        }
    }

    func matches(_ account: MessagingAccountSummary) -> Bool {
        self == .all || rawValue == account.platform.rawValue
    }
}

/// Account list surface for the macOS client.
///
/// The view is intentionally usable before the live management endpoint is
/// wired: pass `MessagingAccountSummary.previewAccounts` for a deterministic
/// preview, or pass live snapshots and implement the optional callbacks.
struct MessagingAccountsView: View {
    let accounts: [MessagingAccountSummary]
    var telegramProjectGroupAccounts: [ManageTelegramProjectGroupAccount] = []
    var availability: MessagingAccountsAvailability = .available
    var onAdd: (() -> Void)?
    /// Returns whether the backend acknowledged the change. On `false` the
    /// optimistic switch state is rolled back.
    var onToggle: ((MessagingAccountSummary, Bool) async -> Bool)?
    var onDelete: ((MessagingAccountSummary) -> Void)?
    var onSaveTelegramProjectGroups: ((String, [ManageTelegramProjectGroup]) async -> Bool)?
    var onSyncTelegramTopics: ((String, String) async -> Bool)?

    @State private var searchText = ""
    @State private var filter: MessagingAccountFilter = .all
    @State private var enabledOverrides: [String: Bool] = [:]
    @State private var pendingToggleIDs: Set<String> = []
    @State private var expandedIDs: Set<String> = []
    @State private var pendingDeletion: MessagingAccountSummary?
    @State private var hoveredAccountID: String?
    @State private var projectGroupAccount: ManageTelegramProjectGroupAccount?
    @State private var editingProjectGroups: [ManageTelegramProjectGroup] = []

    init(
        accounts: [MessagingAccountSummary],
        telegramProjectGroupAccounts: [ManageTelegramProjectGroupAccount] = [],
        availability: MessagingAccountsAvailability = .available,
        onAdd: (() -> Void)? = nil,
        onToggle: ((MessagingAccountSummary, Bool) async -> Bool)? = nil,
        onDelete: ((MessagingAccountSummary) -> Void)? = nil,
        onSaveTelegramProjectGroups: ((String, [ManageTelegramProjectGroup]) async -> Bool)? = nil,
        onSyncTelegramTopics: ((String, String) async -> Bool)? = nil
    ) {
        self.accounts = accounts
        self.telegramProjectGroupAccounts = telegramProjectGroupAccounts
        self.availability = availability
        self.onAdd = onAdd
        self.onToggle = onToggle
        self.onDelete = onDelete
        self.onSaveTelegramProjectGroups = onSaveTelegramProjectGroups
        self.onSyncTelegramTopics = onSyncTelegramTopics
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: ThreadRelayPageLayout.sectionSpacing) {
                availabilityNotice
                filterBar
                accountSummaryBar
                accountPanel
            }
            .frame(maxWidth: ThreadRelayPageLayout.maxContentWidth, alignment: .leading)
            .padding(.horizontal, ThreadRelayPageLayout.horizontalPadding)
            .padding(.top, ThreadRelayPageLayout.topPadding)
            .padding(.bottom, ThreadRelayPageLayout.bottomPadding)
        }
        .scrollIndicators(.never)
        .searchable(text: $searchText, prompt: "搜索账号")
        .onChange(of: accounts) { _, newAccounts in
            // Drop optimistic values once the parent has acknowledged them.
            enabledOverrides = enabledOverrides.filter { id, value in
                guard let account = newAccounts.first(where: { $0.id == id }) else { return false }
                return account.enabled != value
            }
            expandedIDs = expandedIDs.intersection(Set(newAccounts.map(\.id)))
        }
        .confirmationDialog(
            "删除账号？",
            isPresented: Binding(
                get: { pendingDeletion != nil },
                set: { isPresented in
                    if !isPresented { pendingDeletion = nil }
                }
            ),
            titleVisibility: .visible,
            presenting: pendingDeletion
        ) { account in
            Button("删除账号", role: .destructive) {
                onDelete?(account)
                pendingDeletion = nil
            }
            Button("取消", role: .cancel) {
                pendingDeletion = nil
            }
        } message: { account in
            Text("将删除“\(account.displayName?.trimmedNonEmpty ?? account.platform.title)”及其连接配置，此操作不可撤销。")
        }
        .sheet(item: $projectGroupAccount) { account in
            TelegramProjectGroupsView(
                accountID: account.accountId,
                projectGroups: $editingProjectGroups,
                onSave: { groups in
                    guard let onSaveTelegramProjectGroups else { return false }
                    return await onSaveTelegramProjectGroups(account.accountId, groups)
                },
                onSyncTopics: { chatId in
                    guard let onSyncTelegramTopics else { return false }
                    return await onSyncTelegramTopics(account.accountId, chatId)
                }
            )
        }
        .accessibilityIdentifier("messaging-accounts.view")
    }

    @ViewBuilder
    private var availabilityNotice: some View {
        switch availability {
        case .loading:
            Label("正在读取消息渠道账号…", systemImage: "arrow.clockwise")
                .font(.callout)
                .foregroundStyle(.secondary)
        case .available:
            EmptyView()
        case .needsUpdate:
            updateNotice(
                title: "后台服务需要更新",
                message: "当前后台服务不支持账号管理，请更新 MochiPort 后再试。",
                symbol: "arrow.triangle.2.circlepath"
            )
        case .unauthorized:
            updateNotice(
                title: "需要重新授权",
                message: "管理凭据已失效，请刷新后台服务后再试。",
                symbol: "lock.trianglebadge.exclamationmark"
            )
        case let .unavailable(message):
            updateNotice(
                title: "暂时无法读取账号",
                message: message,
                symbol: "exclamationmark.triangle"
            )
        }
    }

    private func updateNotice(title: String, message: String, symbol: String) -> some View {
        HStack(alignment: .top, spacing: ThreadRelaySpacing.standard) {
            Image(systemName: symbol)
                .foregroundStyle(.orange)
                .frame(width: 20)
            VStack(alignment: .leading, spacing: 3) {
                Text(title)
                    .font(.callout.weight(.semibold))
                Text(message)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 0)
        }
        .padding(12)
        .background(Color.orange.opacity(0.08), in: RoundedRectangle(cornerRadius: ThreadRelayRadius.content))
        .overlay {
            RoundedRectangle(cornerRadius: ThreadRelayRadius.content)
                .strokeBorder(Color.orange.opacity(0.18), lineWidth: 0.5)
        }
    }

    private var accountSummaryBar: some View {
        HStack(alignment: .center, spacing: ThreadRelaySpacing.standard) {
            Text(summaryText)
                .font(.callout)
                .foregroundStyle(.secondary)
                .lineLimit(1)
            Spacer(minLength: 0)
            if let onAdd {
                Button(action: onAdd) {
                    Label("添加账号", systemImage: "plus")
                }
                .buttonStyle(.borderedProminent)
                .disabled(availability != .available)
                .help("添加消息渠道账号")
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var filterBar: some View {
        GlassSegmentedControl(
            selection: $filter,
            accessibilityLabel: "消息渠道",
            help: { "只显示\($0.title)账号" }
        )
        .frame(width: 440)
        .accessibilityIdentifier("messaging-accounts.channel-filter")
    }

    private var accountPanel: some View {
        let visible = filteredAccounts
        return VStack(spacing: 0) {
            if visible.isEmpty {
                emptyState
            } else {
                ForEach(Array(visible.enumerated()), id: \.element.id) { index, account in
                    accountRow(account)
                    if index < visible.count - 1 {
                        Divider()
                            .padding(.leading, 64)
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var emptyState: some View {
        VStack(spacing: ThreadRelaySpacing.compact) {
            Image(systemName: emptyStateSymbol)
                .font(.system(size: 26, weight: .regular))
                .foregroundStyle(.secondary)
            Text(emptyStateTitle)
                .font(.headline)
            Text(emptyStateMessage)
                .font(.callout)
                .foregroundStyle(.secondary)
        }
        .multilineTextAlignment(.center)
        .frame(maxWidth: .infinity)
        .padding(.vertical, 54)
    }

    private var emptyStateSymbol: String {
        if !searchText.isEmpty { return "magnifyingglass" }
        switch availability {
        case .needsUpdate: return "arrow.triangle.2.circlepath"
        case .unauthorized: return "lock"
        case .unavailable: return "exclamationmark.triangle"
        default: return "bubble.left.and.bubble.right"
        }
    }

    private var emptyStateTitle: String {
        if !searchText.isEmpty { return "没有匹配的账号" }
        switch availability {
        case .needsUpdate: return "账号管理暂不可用"
        case .unauthorized: return "无法读取账号"
        case .unavailable: return "账号状态暂不可用"
        default: return "还没有消息渠道账号"
        }
    }

    private var emptyStateMessage: String {
        if !searchText.isEmpty { return "尝试更换关键词或渠道筛选。" }
        switch availability {
        case .needsUpdate: return "更新后台服务后，已配置的账号会显示在这里。"
        case .unauthorized: return "请刷新后台服务并重新建立管理授权。"
        case let .unavailable(message): return message
        default: return "添加一个账号后，连接状态会显示在这里。"
        }
    }

    private func accountRow(_ account: MessagingAccountSummary) -> some View {
        let isExpanded = expandedIDs.contains(account.id)
        let mutationsEnabled = availability == .available
        let togglePending = pendingToggleIDs.contains(account.id)
        return VStack(spacing: 0) {
            HStack(spacing: ThreadRelaySpacing.standard) {
                Button {
                    withAnimation(.easeInOut(duration: 0.18)) {
                        if isExpanded {
                            expandedIDs.remove(account.id)
                        } else {
                            expandedIDs.insert(account.id)
                        }
                    }
                } label: {
                    HStack(spacing: ThreadRelaySpacing.standard) {
                        Image(systemName: isExpanded ? "chevron.down" : "chevron.right")
                            .font(.caption2.weight(.semibold))
                            .foregroundStyle(.tertiary)
                            .frame(width: 10)

                        MessagingAccountAvatar(account: account)

                        VStack(alignment: .leading, spacing: 3) {
                            Text(account.displayName?.trimmedNonEmpty ?? account.platform.title)
                                .font(.body.weight(.medium))
                                .lineLimit(1)
                            HStack(spacing: 6) {
                                Text(account.platform.title)
                                Text("·")
                                Text(account.accountID)
                                    .font(.caption2.monospaced())
                            }
                            .font(.caption)
                                .foregroundStyle(.secondary)
                                .lineLimit(1)
                                .truncationMode(.middle)
                        }
                        Spacer(minLength: 4)
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help(isExpanded ? "收起账号详情" : "展开账号详情")

                accountStatus(for: account)

                Toggle(
                    "",
                    isOn: Binding(
                        get: { isEnabled(account) },
                        set: { newValue in
                            guard mutationsEnabled, !togglePending else { return }
                            enabledOverrides[account.id] = newValue
                            guard let onToggle else { return }
                            pendingToggleIDs.insert(account.id)
                            Task {
                                let acknowledged = await onToggle(account, newValue)
                                if !acknowledged {
                                    withAnimation(.easeInOut(duration: 0.18)) {
                                        _ = enabledOverrides.removeValue(forKey: account.id)
                                    }
                                }
                                pendingToggleIDs.remove(account.id)
                            }
                        }
                    )
                )
                .labelsHidden()
                .toggleStyle(.switch)
                .controlSize(.small)
                .disabled(!mutationsEnabled || togglePending || onToggle == nil)
                .accessibilityLabel(isEnabled(account) ? "停用账号" : "启用账号")
                .help(isEnabled(account) ? "停用账号" : "启用账号")

            }
            .padding(.horizontal, 16)
            .frame(minHeight: 64)

            if isExpanded {
                accountDetails(account)
                    .padding(.leading, 64)
                    .padding(.trailing, 16)
                    .padding(.bottom, 14)
            }
        }
        .background(hoveredAccountID == account.id ? Color.primary.opacity(0.04) : Color.clear)
        .onHover { hovering in
            withAnimation(.easeInOut(duration: 0.12)) {
                if hovering {
                    hoveredAccountID = account.id
                } else if hoveredAccountID == account.id {
                    hoveredAccountID = nil
                }
            }
        }
        .contextMenu {
            if onDelete != nil, mutationsEnabled {
                Button("删除账号", role: .destructive) {
                    pendingDeletion = account
                }
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("messaging-account.\(account.id)")
    }

    private func accountStatus(for account: MessagingAccountSummary) -> some View {
        let state = state(for: account)
        return HStack(spacing: 6) {
            Circle()
                .fill(state.tint)
                .frame(width: 7, height: 7)
            Text(state.title)
                .font(.callout)
                .foregroundStyle(state.tint)
        }
        .fixedSize(horizontal: true, vertical: false)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("连接状态：\(state.title)")
    }

    private func accountDetails(_ account: MessagingAccountSummary) -> some View {
        VStack(alignment: .leading, spacing: ThreadRelaySpacing.compact) {
            HStack(spacing: 18) {
                detailItem("配置", value: account.configured ? "完整" : "不完整")
                detailItem("凭据", value: account.secretSet ? "已设置" : "未设置")
                if account.polling {
                    detailItem("轮询", value: "运行中")
                }
            }
            if let lastError = account.lastError?.trimmedNonEmpty {
                Label(lastError, systemImage: "exclamationmark.triangle")
                    .font(.caption)
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            } else if let lastInboundAt = account.lastInboundAt {
                Text("最近收到消息：\(Text(lastInboundAt, style: .relative))")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            } else if let lastEventAt = account.lastEventAt {
                Text("最近活动：\(Text(lastEventAt, style: .relative))")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            if account.platform == .telegram, onSaveTelegramProjectGroups != nil {
                Button {
                    let saved = telegramProjectGroupAccounts.first(where: { $0.accountId == account.accountID })
                    editingProjectGroups = saved?.projectGroups ?? []
                    projectGroupAccount = saved ?? ManageTelegramProjectGroupAccount(accountId: account.accountID, projectGroups: [])
                } label: {
                    Label("配置项目群", systemImage: "folder.badge.gearshape")
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
                .disabled(availability != .available)
                .help("为 Telegram 机器人配置项目群")
            }
            if onDelete != nil, availability == .available {
                Button("删除账号", role: .destructive) {
                    pendingDeletion = account
                }
                .buttonStyle(.link)
                .padding(.top, 2)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .transition(.opacity)
    }

    private func detailItem(_ title: String, value: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title)
                .font(.caption2)
                .foregroundStyle(.tertiary)
            Text(value)
                .font(.caption.weight(.medium))
        }
    }

    private var filteredAccounts: [MessagingAccountSummary] {
        let query = searchText.trimmingCharacters(in: .whitespacesAndNewlines)
        return accounts.filter { account in
            guard filter.matches(account) else { return false }
            guard !query.isEmpty else { return true }
            return account.displayName?.localizedCaseInsensitiveContains(query) == true
                || account.accountID.localizedCaseInsensitiveContains(query)
                || account.platform.title.localizedCaseInsensitiveContains(query)
        }
    }

    private var summaryText: String {
        let connected = accounts.filter { state(for: $0) == .connected }.count
        if accounts.isEmpty { return "管理 Telegram、飞书、微信和企业微信账号" }
        return "\(accounts.count) 个账号 · \(connected) 个在线"
    }

    private func isEnabled(_ account: MessagingAccountSummary) -> Bool {
        enabledOverrides[account.id] ?? account.enabled
    }

    private func state(for account: MessagingAccountSummary) -> MessagingAccountState {
        guard isEnabled(account) else { return .disabled }
        if account.connected { return .connected }
        if account.lastError?.trimmedNonEmpty != nil { return .error }
        if account.connecting || account.polling { return .connecting }
        if !account.configured || !account.secretSet { return .incomplete }
        return .offline
    }
}

private struct TelegramProjectGroupsView: View {
    @Environment(\.dismiss) private var dismiss
    let accountID: String
    @Binding var projectGroups: [ManageTelegramProjectGroup]
    let onSave: ([ManageTelegramProjectGroup]) async -> Bool
    let onSyncTopics: (String) async -> Bool
    @State private var saving = false
    @State private var errorMessage: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("一个群对应一个项目。机器人收到群里的第一条消息后，会自动创建一个 Topic，并把这个 Topic 绑定到对应项目目录。")
                .font(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

            if projectGroups.isEmpty {
                ContentUnavailableView("还没有项目群", systemImage: "folder", description: Text("添加一个 Telegram 群组后即可开始。"))
                    .frame(maxWidth: .infinity)
            } else {
                ScrollView {
                    VStack(spacing: 10) {
                        ForEach($projectGroups) { $group in
                            VStack(alignment: .leading, spacing: 8) {
                                HStack {
                                    TextField("项目名称", text: $group.projectName)
                                    Button {
                                        projectGroups.removeAll { $0.id == group.id }
                                    } label: {
                                        Image(systemName: "trash")
                                    }
                                    .buttonStyle(.borderless)
                                    .foregroundStyle(.red)
                                    .help("删除项目群")
                                }
                                TextField("Telegram 群组 ID，例如 -1001234567890", text: $group.chatId)
                                    .textFieldStyle(.roundedBorder)
                                    .font(.caption.monospaced())
                                TextField("项目目录", text: $group.cwd)
                                    .textFieldStyle(.roundedBorder)
                            }
                            .padding(12)
                            .background(Color.primary.opacity(0.035), in: RoundedRectangle(cornerRadius: 8))
                        }
                    }
                }
                .frame(maxHeight: 300)
            }

            Button {
                projectGroups.append(ManageTelegramProjectGroup(chatId: "", projectName: "", cwd: ""))
            } label: {
                Label("添加项目群", systemImage: "plus")
            }
            .buttonStyle(.bordered)

            if !projectGroups.isEmpty {
                Menu {
                    ForEach(projectGroups) { group in
                        Button(group.projectName.isEmpty ? group.chatId : group.projectName) {
                            Task { await syncTopics(for: group) }
                        }
                        .disabled(group.chatId.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    }
                } label: {
                    Label("同步 / 转移 Codex 会话到 Telegram Topic", systemImage: "arrow.triangle.2.circlepath")
                }
                .buttonStyle(.borderedProminent)
                .disabled(saving)
            }

            Label("需要使用 Forum 群组，并确保 Bot 有管理 Topic 的权限。群成员都可以向该项目发送消息。", systemImage: "info.circle")
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

            if let errorMessage {
                Label(errorMessage, systemImage: "exclamationmark.triangle")
                    .font(.caption)
                    .foregroundStyle(.red)
            }
        }
        .padding(22)
        .frame(width: 560)
        .navigationTitle("Telegram 项目群")
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("取消") { dismiss() }
            }
            ToolbarItem(placement: .confirmationAction) {
                Button("保存") { Task { await save() } }
                    .buttonStyle(.borderedProminent)
                    .disabled(saving)
            }
        }
    }

    private func save() async {
        let normalized = projectGroups.map {
            ManageTelegramProjectGroup(
                chatId: $0.chatId.trimmingCharacters(in: .whitespacesAndNewlines),
                projectName: $0.projectName.trimmingCharacters(in: .whitespacesAndNewlines),
                cwd: $0.cwd.trimmingCharacters(in: .whitespacesAndNewlines)
            )
        }
        guard normalized.allSatisfy({ !$0.chatId.isEmpty && !$0.projectName.isEmpty && !$0.cwd.isEmpty }) else {
            errorMessage = "每个项目群都需要填写项目名称、群组 ID 和项目目录。"
            return
        }
        guard Set(normalized.map(\.chatId)).count == normalized.count else {
            errorMessage = "群组 ID 不能重复。"
            return
        }
        saving = true
        errorMessage = nil
        if await onSave(normalized) {
            dismiss()
        } else {
            errorMessage = "保存失败，请查看账号页面底部的错误提示。"
        }
        saving = false
    }

    private func syncTopics(for group: ManageTelegramProjectGroup) async {
        guard !saving else { return }
        saving = true
        errorMessage = nil
        let ok = await onSyncTopics(group.chatId.trimmingCharacters(in: .whitespacesAndNewlines))
        if !ok {
            errorMessage = "同步失败，请查看账号页面底部的错误提示。"
        }
        saving = false
    }
}

private extension Optional where Wrapped == String {
    var trimmedNonEmpty: String? {
        guard let value = self?.trimmingCharacters(in: .whitespacesAndNewlines), !value.isEmpty else {
            return nil
        }
        return value
    }
}

private extension String {
    var trimmedNonEmpty: String? {
        let value = trimmingCharacters(in: .whitespacesAndNewlines)
        return value.isEmpty ? nil : value
    }
}

#if DEBUG
#Preview("消息渠道账号") {
    MessagingAccountsView(accounts: MessagingAccountSummary.previewAccounts)
        .frame(width: 860, height: 620)
}

#Preview("空账号列表") {
    MessagingAccountsView(accounts: [])
        .frame(width: 860, height: 620)
}
#endif
