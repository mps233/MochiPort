import SwiftUI

/// 账号池页面：连接 Sub2API 管理接口，并只读展示账号、状态与上游余额。
/// 池数据完全来自 daemon 的只读快照；本页不存储凭据，也不修改上游账号。
struct AccountPoolView: View {
    @EnvironmentObject private var model: AppModel

    @State private var sub2ApiBaseURL = ""
    @State private var sub2ApiAdminKey = ""
    @State private var sub2ApiFormInitialized = false
    @State private var sub2ApiSaving = false
    @State private var sub2ApiEditing = false
    @State private var confirmSub2ApiDisconnect = false

    private var configured: Bool { model.sub2ApiAdmin?.configured == true }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: MochiPortPageLayout.sectionSpacing) {
                if let error = model.sectionErrors[.accountPool] {
                    AccountPoolInlineError(message: error) {
                        model.dismissSectionError(.accountPool)
                    }
                }

                AccountPoolConnectionCard(
                    admin: model.sub2ApiAdmin,
                    baseURL: $sub2ApiBaseURL,
                    adminKey: $sub2ApiAdminKey,
                    isEditing: $sub2ApiEditing,
                    isSaving: sub2ApiSaving,
                    onSave: { Task { await saveSub2ApiConnection() } },
                    onDisconnect: { confirmSub2ApiDisconnect = true }
                )

                if configured {
                    AccountPoolContentSection(
                        pool: model.sub2ApiAccountPool,
                        isLoading: model.sub2ApiAccountPoolLoading,
                        loadError: model.sub2ApiAccountPoolError,
                        onRetry: {
                            Task { await model.refreshSub2ApiAccountPool(forceBillingRefresh: true) }
                        }
                    )
                }
            }
            .frame(maxWidth: MochiPortPageLayout.maxContentWidth, alignment: .leading)
            .padding(.top, MochiPortPageLayout.topPadding)
            .padding(.bottom, MochiPortPageLayout.bottomPadding)
        }
        .contentMargins(
            .horizontal,
            MochiPortPageLayout.horizontalPadding,
            for: .scrollContent
        )
        .scrollIndicators(.never)
        .task {
            await model.loadSection(.accountPool)
            synchronizeSub2ApiAdmin(model.sub2ApiAdmin, gateway: model.gateway)
        }
        .onChange(of: model.sub2ApiAdmin) { _, admin in
            synchronizeSub2ApiAdmin(admin, gateway: model.gateway)
        }
        .onDisappear {
            model.cancelSub2ApiAccountPoolRefresh()
        }
        .confirmationDialog(
            "断开 Sub2API 账号池？",
            isPresented: $confirmSub2ApiDisconnect
        ) {
            Button("断开连接", role: .destructive) {
                Task { await disconnectSub2ApiAccountPool() }
            }
            Button("取消", role: .cancel) {}
        } message: {
            Text("只会删除本机保存的管理连接，不会修改 Sub2API 中的账号。")
        }
    }

    @MainActor
    private func saveSub2ApiConnection() async {
        guard !sub2ApiSaving else { return }
        sub2ApiSaving = true
        defer { sub2ApiSaving = false }
        let key = sub2ApiAdminKey.trimmingCharacters(in: .whitespacesAndNewlines)
        let saved = await model.saveSub2ApiAdmin(
            baseUrl: sub2ApiBaseURL,
            adminApiKey: key.isEmpty ? nil : key
        )
        guard saved else { return }
        sub2ApiAdminKey = ""
        sub2ApiEditing = false
        await model.refreshSub2ApiAccountPool()
    }

    @MainActor
    private func disconnectSub2ApiAccountPool() async {
        guard !sub2ApiSaving else { return }
        sub2ApiSaving = true
        defer { sub2ApiSaving = false }
        guard await model.disconnectSub2ApiAdmin() else { return }
        sub2ApiAdminKey = ""
        sub2ApiEditing = false
        sub2ApiBaseURL = suggestedSub2ApiBaseURL(model.gateway)
    }

    private func synchronizeSub2ApiAdmin(
        _ admin: ManageSub2ApiAdmin?,
        gateway: ManageGateway?
    ) {
        guard let admin else { return }
        if admin.configured || !admin.baseUrl.isEmpty {
            sub2ApiBaseURL = admin.baseUrl
        } else if !sub2ApiFormInitialized, sub2ApiBaseURL.isEmpty {
            sub2ApiBaseURL = suggestedSub2ApiBaseURL(gateway)
        }
        sub2ApiFormInitialized = true
    }

    private func suggestedSub2ApiBaseURL(_ gateway: ManageGateway?) -> String {
        gateway?.providers.first(where: { provider in
            provider.name.localizedCaseInsensitiveContains("sub2api")
                || provider.baseUrl.localizedCaseInsensitiveContains("sub2api")
        })?.baseUrl ?? ""
    }
}

private struct AccountPoolInlineError: View {
    let message: String
    let onDismiss: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(.orange)
            Text(message)
                .font(.callout)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 8)
            Button {
                onDismiss()
            } label: {
                Image(systemName: "xmark")
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .help("关闭提示")
            .accessibilityLabel("关闭错误提示")
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.orange.opacity(0.1), in: RoundedRectangle(cornerRadius: 10))
        .accessibilityIdentifier("accountpool.inline-error")
    }
}

private struct AccountPoolConnectionCard: View {
    let admin: ManageSub2ApiAdmin?
    @Binding var baseURL: String
    @Binding var adminKey: String
    @Binding var isEditing: Bool
    let isSaving: Bool
    let onSave: () -> Void
    let onDisconnect: () -> Void

    private var configured: Bool { admin?.configured == true }
    private var hasSavedKey: Bool { admin?.secretSet == true }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 12) {
                Image(systemName: configured ? "checkmark.circle.fill" : "link.badge.plus")
                    .font(.system(size: 17, weight: .medium))
                    .foregroundStyle(configured ? Color.green : Color.accentColor)
                    .frame(width: 36, height: 36)
                    .background(
                        (configured ? Color.green : Color.accentColor).opacity(0.12),
                        in: RoundedRectangle(cornerRadius: 10, style: .continuous)
                    )
                    .accessibilityHidden(true)

                VStack(alignment: .leading, spacing: 2) {
                    Text(configured ? "管理连接已就绪" : "连接 Sub2API 账号池")
                        .font(.callout.weight(.semibold))
                    Text(statusDetail)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }

                Spacer(minLength: 12)

                if configured {
                    Button(isEditing ? "取消" : "修改") {
                        withAnimation(.easeOut(duration: 0.16)) {
                            isEditing.toggle()
                        }
                    }
                    .controlSize(.small)
                    .disabled(isSaving)

                    if !isEditing {
                        Button("断开", role: .destructive) {
                            onDisconnect()
                        }
                        .controlSize(.small)
                        .disabled(isSaving)
                    }
                }
            }

            if !configured || isEditing {
                Divider()
                    .opacity(0.55)

                LabeledContent("管理地址") {
                    TextField("https://sub2api.example.com", text: $baseURL)
                        .textFieldStyle(.roundedBorder)
                        .frame(maxWidth: 560)
                        .accessibilityLabel("Sub2API 管理地址")
                }
                LabeledContent("Admin API Key") {
                    SecureField(
                        hasSavedKey ? "留空以继续使用已保存的密钥" : "输入管理密钥",
                        text: $adminKey
                    )
                    .textFieldStyle(.roundedBorder)
                    .frame(maxWidth: 560)
                    .accessibilityLabel("Sub2API Admin API Key")
                }
                HStack {
                    Label(
                        hasSavedKey ? "管理密钥已保存在本机，界面不会回显。" : "需要管理权限的 API Key。",
                        systemImage: hasSavedKey ? "key.fill" : "key"
                    )
                    .font(.caption)
                    .foregroundStyle(.secondary)

                    Spacer(minLength: 12)

                    Button {
                        onSave()
                    } label: {
                        if isSaving {
                            ProgressView()
                                .controlSize(.small)
                        } else {
                            Label(configured ? "更新连接" : "连接", systemImage: "link")
                        }
                    }
                    .buttonStyle(.borderedProminent)
                    .buttonBorderShape(.capsule)
                    .controlSize(.small)
                    .disabled(!canSave(isSaving))
                }
            }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .accountPoolCardSurface()
        .animation(.easeOut(duration: 0.18), value: isEditing)
        .animation(.easeOut(duration: 0.18), value: configured)
        .accessibilityIdentifier("accountpool.connection")
    }

    private func canSave(_ saving: Bool) -> Bool {
        !saving
            && !baseURL.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && (hasSavedKey || !adminKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
    }

    private var statusDetail: String {
        if configured {
            return "账号状态、倍率与上游余额展示在下方。"
        }
        return "输入管理地址和 Admin API Key，连接后在这里查看账号与余额。"
    }
}

private struct AccountPoolContentSection: View {
    let pool: ManageSub2ApiAccountPoolResponse.Pool?
    let isLoading: Bool
    let loadError: String?
    let onRetry: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if pool?.accounts.isEmpty != false {
                AccountPoolPlaceholder(
                    isLoading: isLoading,
                    loadError: loadError,
                    onRetry: onRetry
                )
            } else if let pool {
                if let loadError {
                    Label("显示上次结果，刷新失败", systemImage: "clock.badge.exclamationmark")
                        .font(.caption)
                        .foregroundStyle(.orange)
                        .help(loadError)
                }
                if let warnings = pool.warnings, !warnings.isEmpty {
                    Label(sub2ApiWarningsText(warnings), systemImage: "exclamationmark.triangle")
                        .font(.caption)
                        .foregroundStyle(.orange)
                }

                AccountPoolStatsStrip(summary: sub2ApiPoolSummary(pool.accounts))

                AccountPoolAccountTable(accounts: pool.accounts)

                Text("更新于 \(sub2ApiFetchedTime(pool.fetchedAtMs))")
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
                    .lineLimit(1)
                    .frame(maxWidth: .infinity, alignment: .trailing)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityIdentifier("accountpool.content")
    }
}

private struct AccountPoolPlaceholder: View {
    let isLoading: Bool
    let loadError: String?
    let onRetry: () -> Void

    var body: some View {
        VStack(spacing: 9) {
            if isLoading {
                ProgressView()
                    .controlSize(.small)
                Text("正在读取账号池…")
                    .font(.callout)
            } else if let loadError {
                Image(systemName: "exclamationmark.triangle")
                    .font(.system(size: 22))
                    .foregroundStyle(.orange)
                Text("暂时无法读取账号池")
                    .font(.callout.weight(.medium))
                Text(loadError)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .lineLimit(3)
                    .frame(maxWidth: 420)
                Button("重试", action: onRetry)
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .padding(.top, 2)
            } else {
                Image(systemName: "person.3")
                    .font(.system(size: 22))
                    .foregroundStyle(.secondary)
                Text("账号池中还没有账号")
                    .font(.callout)
            }
        }
        .padding(28)
        .frame(maxWidth: .infinity, minHeight: 120)
        .accountPoolCardSurface()
    }
}

private struct AccountPoolStatsStrip: View {
    let summary: Sub2ApiPoolSummary

    var body: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 10) {
                cells
            }
            VStack(spacing: 10) {
                cells
            }
        }
        .accessibilityElement(children: .contain)
    }

    @ViewBuilder
    private var cells: some View {
        AccountPoolStatsCell(
            title: "账号",
            value: "\(summary.total)",
            symbol: "person.3.sequence",
            tint: .secondary
        )
        AccountPoolStatsCell(
            title: "可用",
            value: "\(summary.available)",
            symbol: "checkmark.circle",
            tint: availabilityTint
        )
        AccountPoolStatsCell(
            title: "异常",
            value: "\(summary.attention)",
            symbol: "exclamationmark.triangle",
            tint: summary.attention > 0 ? .red : .secondary
        )
        AccountPoolStatsCell(
            title: "余额",
            value: summary.balanceText,
            symbol: "creditcard",
            tint: .primary,
            isEmphasized: true
        )
    }

    private var availabilityTint: Color {
        if summary.available == summary.total { return Theme.safeGreen }
        if summary.available == 0 { return .red }
        return .orange
    }
}

private struct AccountPoolStatsCell: View {
    let title: String
    let value: String
    let symbol: String
    let tint: Color
    var isEmphasized = false

    var body: some View {
        HStack(spacing: 11) {
            Image(systemName: symbol)
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(isEmphasized ? Color.accentColor : tint)
                .frame(width: 30, height: 30)
                .background(
                    (isEmphasized ? Color.accentColor : tint).opacity(0.12),
                    in: RoundedRectangle(cornerRadius: 9, style: .continuous)
                )
                .accessibilityHidden(true)

            VStack(alignment: .leading, spacing: 1) {
                Text(title)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Text(value)
                    .font(isEmphasized ? .title3.monospacedDigit().weight(.semibold) : .callout.monospacedDigit().weight(.semibold))
                    .foregroundStyle(isEmphasized ? Color.primary : Color.primary.opacity(0.85))
                    .lineLimit(1)
                    .minimumScaleFactor(0.6)
            }
            Spacer(minLength: 6)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 11)
        .frame(maxWidth: .infinity, minHeight: 58, alignment: .leading)
        .accountPoolCardSurface()
        .accessibilityElement(children: .combine)
    }
}

// MARK: - 账号表格（自概览页整体迁入）

private struct AccountPoolAccountGroup: Identifiable {
    let key: String
    let siteUrl: String?
    var accounts: [ManageSub2ApiAccountPoolResponse.Account]

    var id: String { key }
}

private enum AccountPoolTableLayout {
    static let columnSpacing: CGFloat = 12
    static let statusWidth: CGFloat = 104
    static let balanceWidth: CGFloat = 96
    static let horizontalPadding: CGFloat = 16
    static let nestedContentInset: CGFloat = 36
    static let routeGuideInset: CGFloat = 30
}

private struct AccountPoolAccountTable: View {
    let accounts: [ManageSub2ApiAccountPoolResponse.Account]
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var expandedGroupKeys: Set<String> = []

    private var groups: [AccountPoolAccountGroup] {
        var result: [AccountPoolAccountGroup] = []
        var indexByKey: [String: Int] = [:]
        for account in accounts {
            let key = sub2ApiAccountGroupKey(account)
            if let index = indexByKey[key] {
                result[index].accounts.append(account)
            } else {
                indexByKey[key] = result.count
                result.append(
                    AccountPoolAccountGroup(
                        key: key,
                        siteUrl: account.siteUrl,
                        accounts: [account]
                    )
                )
            }
        }
        return result
    }

    private var expandableGroupKeys: Set<String> {
        Set(groups.filter { $0.accounts.count > 1 }.map(\.id))
    }

    private func toggleGroup(_ key: String) {
        let animation: Animation = reduceMotion
            ? .easeOut(duration: 0.14)
            : .easeInOut(duration: 0.2)
        withAnimation(animation) {
            if expandedGroupKeys.contains(key) {
                expandedGroupKeys.remove(key)
            } else {
                expandedGroupKeys.insert(key)
            }
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: AccountPoolTableLayout.columnSpacing) {
                Text("站点 / 账号")
                    .frame(maxWidth: .infinity, alignment: .leading)
                Text("状态")
                    .frame(width: AccountPoolTableLayout.statusWidth, alignment: .leading)
                Text("余额")
                    .frame(width: AccountPoolTableLayout.balanceWidth, alignment: .trailing)
            }
            .font(.caption.weight(.medium))
            .tracking(0.3)
            .foregroundStyle(.tertiary)
            .padding(.horizontal, AccountPoolTableLayout.horizontalPadding)
            .padding(.vertical, 9)
            .background(Color.primary.opacity(0.03))
            .overlay(alignment: .bottom) {
                Divider()
                    .opacity(0.65)
            }

            ForEach(Array(groups.enumerated()), id: \.element.id) { index, group in
                if group.accounts.count == 1, let account = group.accounts.first {
                    AccountPoolAccountRow(account: account)
                } else {
                    AccountPoolAccountGroupRow(
                        group: group,
                        isExpanded: expandedGroupKeys.contains(group.id),
                        onToggle: { toggleGroup(group.id) }
                    )
                    if expandedGroupKeys.contains(group.id) {
                        VStack(spacing: 0) {
                            ForEach(Array(group.accounts.enumerated()), id: \.element.id) { childIndex, account in
                                AccountPoolAccountRow(
                                    account: account,
                                    nested: true
                                )
                                if childIndex < group.accounts.count - 1 {
                                    Divider()
                                        .opacity(0.55)
                                        .padding(.leading, 52)
                                        .padding(.trailing, AccountPoolTableLayout.horizontalPadding)
                                }
                            }
                        }
                        .background {
                            ZStack(alignment: .leading) {
                                Color.primary.opacity(0.012)
                                Rectangle()
                                    .fill(Color.accentColor.opacity(0.24))
                                    .frame(width: 1)
                                    .padding(.leading, AccountPoolTableLayout.routeGuideInset)
                                    .padding(.vertical, 12)
                            }
                        }
                        .transition(
                            reduceMotion
                                ? .opacity
                                : .opacity.combined(with: .move(edge: .top))
                        )
                    }
                }

                if index < groups.count - 1 {
                    Divider()
                        .opacity(0.65)
                        .padding(.horizontal, AccountPoolTableLayout.horizontalPadding)
                }
            }
        }
        .accountPoolCardSurface()
        .onChange(of: expandableGroupKeys) { _, keys in
            expandedGroupKeys.formIntersection(keys)
        }
    }
}

private struct AccountPoolAccountGroupRow: View {
    let group: AccountPoolAccountGroup
    let isExpanded: Bool
    let onToggle: () -> Void
    @State private var isHovering = false

    private var availableCount: Int {
        group.accounts.count(where: { account in
            account.schedulable && account.status.lowercased() == "active"
        })
    }

    private var balanceSummaryText: String {
        let values = group.accounts.map { sub2ApiBalanceText($0.upstreamBalance) }
        var uniqueValues: [String] = []
        for value in values where !uniqueValues.contains(value) {
            uniqueValues.append(value)
        }
        return uniqueValues.count > 1 ? "各账号不同" : (uniqueValues.first ?? "—")
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

    private var statusTint: Color {
        if availableCount == group.accounts.count { return Theme.safeGreen }
        if availableCount == 0 { return .red }
        return .orange
    }

    private var statusText: String {
        availableCount == group.accounts.count
            ? "全部可用"
            : "\(availableCount)/\(group.accounts.count) 可用"
    }

    var body: some View {
        Button(action: onToggle) {
            HStack(spacing: AccountPoolTableLayout.columnSpacing) {
                HStack(spacing: 9) {
                    Image(systemName: isExpanded ? "chevron.down" : "chevron.right")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(isHovering ? Color.accentColor : .secondary)
                        .frame(width: 24, height: 24)
                        .background(
                            Color.accentColor.opacity(isHovering ? 0.12 : 0.065),
                            in: RoundedRectangle(cornerRadius: 7, style: .continuous)
                        )
                        .accessibilityHidden(true)

                    VStack(alignment: .leading, spacing: 3) {
                        Text(sub2ApiSiteLabel(group.siteUrl))
                            .font(.callout.weight(.semibold))
                            .lineLimit(1)
                        Text("\(group.accounts.count) 个账号 · \(availableCount) 个可用")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .help(group.siteUrl ?? "")

                AccountPoolStatusCapsule(text: statusText, tint: statusTint)
                    .frame(width: AccountPoolTableLayout.statusWidth, alignment: .leading)

                AccountPoolBalanceValue(text: balanceSummaryText, tint: balanceTint)
                    .frame(width: AccountPoolTableLayout.balanceWidth, alignment: .trailing)
            }
            .padding(.horizontal, AccountPoolTableLayout.horizontalPadding)
            .padding(.vertical, 10)
            .frame(maxWidth: .infinity, minHeight: 64, alignment: .leading)
            .background(Color.accentColor.opacity(isHovering ? 0.055 : 0.022))
        }
        .buttonStyle(.plain)
        .contentShape(Rectangle())
        .onHover { isHovering = $0 }
        .animation(.easeOut(duration: 0.14), value: isHovering)
        .help(isExpanded ? "收起子账号" : "展开子账号")
        .accessibilityLabel(
            "\(sub2ApiSiteLabel(group.siteUrl))，\(group.accounts.count) 个账号，\(availableCount) 个可用，余额 \(balanceSummaryText)"
        )
        .accessibilityValue(isExpanded ? "子账号已展开" : "子账号已收起")
        .accessibilityHint("主账号汇总始终显示，仅切换下面的子账号列表")
    }
}

private struct AccountPoolAccountRow: View {
    let account: ManageSub2ApiAccountPoolResponse.Account
    let nested: Bool
    @State private var isHovering = false

    init(
        account: ManageSub2ApiAccountPoolResponse.Account,
        nested: Bool = false
    ) {
        self.account = account
        self.nested = nested
    }

    private var accountTint: Color {
        guard account.schedulable else { return .red }
        switch account.status.lowercased() {
        case "active": return Theme.safeGreen
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
        HStack(spacing: AccountPoolTableLayout.columnSpacing) {
            VStack(alignment: .leading, spacing: 2) {
                Text(account.name)
                    .font(.callout.weight(.medium))
                    .lineLimit(1)
                HStack(spacing: 5) {
                    Text(sub2ApiAccountKindText(account))
                    Text("·")
                    Text("倍率 \(sub2ApiMultiplierText(account.localRateMultiplier)) / 上游 \(sub2ApiUpstreamRateText(account.upstreamBilling))")
                        .help(sub2ApiCapabilityStateText(account.upstreamBilling.state))
                    if account.upstreamBilling.stale {
                        Image(systemName: "clock.badge.exclamationmark")
                            .font(.caption2)
                            .foregroundStyle(.orange)
                            .help("上游倍率数据已过期")
                            .accessibilityLabel("上游倍率数据已过期")
                    }
                }
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .minimumScaleFactor(0.78)
            }
            .padding(.leading, nested ? AccountPoolTableLayout.nestedContentInset : 0)
            .frame(maxWidth: .infinity, alignment: .leading)

            AccountPoolStatusCapsule(
                text: sub2ApiAccountStatusText(account),
                tint: accountTint
            )
            .frame(width: AccountPoolTableLayout.statusWidth, alignment: .leading)

            AccountPoolBalanceValue(
                text: sub2ApiBalanceText(account.upstreamBalance),
                tint: balanceTint
            )
            .frame(width: AccountPoolTableLayout.balanceWidth, alignment: .trailing)
            .help(sub2ApiBalanceHelp(account.upstreamBalance))

        }
        .padding(.horizontal, AccountPoolTableLayout.horizontalPadding)
        .padding(.vertical, nested ? 9 : 10)
        .frame(minHeight: 58)
        .background(Color.primary.opacity(isHovering ? 0.035 : 0))
        .overlay(alignment: .leading) {
            if nested {
                Circle()
                    .fill(accountTint)
                    .frame(width: 5, height: 5)
                    .padding(.leading, AccountPoolTableLayout.routeGuideInset - 2)
                    .accessibilityHidden(true)
            }
        }
        .onHover { isHovering = $0 }
        .animation(.easeOut(duration: 0.12), value: isHovering)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(
            "\(account.name)，\(sub2ApiAccountStatusText(account))，本地倍率 \(sub2ApiMultiplierText(account.localRateMultiplier))，上游倍率 \(sub2ApiUpstreamRateText(account.upstreamBilling))，余额 \(sub2ApiBalanceText(account.upstreamBalance))"
        )
    }
}

/// 余额列统一排版：金额用等距半粗体，"无限"不是数字，弱化成次要色。
private struct AccountPoolBalanceValue: View {
    let text: String
    let tint: Color

    private var isUnlimited: Bool {
        text == "无限" || text == "无限额度"
    }

    var body: some View {
        Text(text)
            .font(.callout.monospacedDigit().weight(isUnlimited ? .medium : .semibold))
            .foregroundStyle(isUnlimited ? Color.secondary : tint)
            .lineLimit(1)
            .minimumScaleFactor(0.78)
    }
}

private struct AccountPoolStatusCapsule: View {
    let text: String
    let tint: Color

    var body: some View {
        HStack(spacing: 5) {
            Circle()
                .fill(tint)
                .frame(width: 5, height: 5)
            Text(text)
                .lineLimit(1)
        }
        .font(.caption.weight(.medium))
        .foregroundStyle(tint)
        .padding(.horizontal, 8)
        .padding(.vertical, 4)
        .background(tint.opacity(0.10), in: Capsule())
        .overlay {
            Capsule()
                .strokeBorder(tint.opacity(0.18), lineWidth: 0.5)
        }
    }
}

// MARK: - 池汇总（概览摘要行与统计条共用）

struct Sub2ApiPoolSummary: Equatable {
    let total: Int
    let available: Int
    let attention: Int
    let balanceText: String
}

/// 可用 = 可调度且状态 active；异常 = 不可调度或状态 error。
/// 停用/冷却等中间态不计入异常，只在表格中展示。
func sub2ApiPoolSummary(
    _ accounts: [ManageSub2ApiAccountPoolResponse.Account]
) -> Sub2ApiPoolSummary {
    Sub2ApiPoolSummary(
        total: accounts.count,
        available: accounts.count(where: { $0.schedulable && $0.status.lowercased() == "active" }),
        attention: accounts.count(where: { !$0.schedulable || $0.status.lowercased() == "error" }),
        balanceText: sub2ApiPoolBalanceSummaryText(accounts)
    )
}

/// 余额按币种分别求和；无法读取余额或无限额度的账号不参与合计。
func sub2ApiPoolBalanceSummaryText(
    _ accounts: [ManageSub2ApiAccountPoolResponse.Account]
) -> String {
    var totals: [String: Double] = [:]
    var units: [String] = []
    for account in accounts {
        let balance = account.upstreamBalance
        guard balance.state == "available",
              !balance.unlimited,
              let remaining = balance.remaining,
              remaining.isFinite
        else { continue }
        let unit = balance.unit?
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .uppercased() ?? ""
        if totals[unit] == nil {
            units.append(unit)
        }
        totals[unit, default: 0] += remaining
    }
    guard !units.isEmpty else { return "—" }
    return units.map { unit -> String in
        let amount = sub2ApiCompactDecimal(
            totals[unit] ?? 0,
            maximumFractionDigits: 2,
            minimumFractionDigits: 2
        )
        switch unit {
        case "USD": return "$\(amount)"
        case "": return amount
        case let other: return "\(amount) \(other)"
        }
    }
    .joined(separator: " · ")
}

// MARK: - Sub2API 展示格式化（概览摘要、配额 Dock 与仪表盘共用）

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
        case "balance_export_unavailable": "当前 Sub2API 版本不提供账号备份导出"
        case "balance_export_forbidden": "Sub2API 已开启两步验证，无法读取上游余额"
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

private extension View {
    @ViewBuilder
    func accountPoolCardSurface() -> some View {
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
