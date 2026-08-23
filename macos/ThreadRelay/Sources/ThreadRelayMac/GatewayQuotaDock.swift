import AppKit
import Foundation
import SwiftUI

private let gatewayQuotaDefaultProgressReference = 20.0
private let gatewayQuotaDefaultWarningThreshold = 3.0

enum GatewayQuotaTone: Equatable {
    case normal
    case warning
    case critical
    case unavailable
}

struct GatewayQuotaMeterPresentation: Equatable {
    let fraction: Double?
    let statusText: String
    let tone: GatewayQuotaTone
    let warningThreshold: Double?
}

enum GatewayQuotaCachedState: Equatable {
    case fresh
    case refreshing
    case refreshFailed
}

func gatewayQuotaCachedPresentation(
    _ presentation: GatewayQuotaMeterPresentation,
    state: GatewayQuotaCachedState
) -> GatewayQuotaMeterPresentation {
    guard state != .fresh else { return presentation }
    return GatewayQuotaMeterPresentation(
        fraction: presentation.fraction,
        statusText: state == .refreshing
            ? "正在刷新 · 上次数据"
            : "刷新失败 · 上次数据",
        tone: state == .refreshing
            ? .unavailable
            : (presentation.tone == .critical ? .critical : .warning),
        warningThreshold: presentation.warningThreshold
    )
}

struct GatewayQuotaRequestID: Hashable {
    let providerName: String?
    let gatewayGeneration: Int
    let refreshGeneration: Int
}

func gatewayQuotaMeterPresentation(
    _ usage: ManageProviderUsageResponse.Usage
) -> GatewayQuotaMeterPresentation {
    if usage.accountValid == false {
        return GatewayQuotaMeterPresentation(
            fraction: 0,
            statusText: providerUsageAccountWarning(usage.accountStatus) ?? "账户不可用",
            tone: .critical,
            warningThreshold: nil
        )
    }
    if usage.unlimited {
        return GatewayQuotaMeterPresentation(
            fraction: 1,
            statusText: "无限额度",
            tone: .normal,
            warningThreshold: nil
        )
    }
    guard let remaining = usage.remaining, remaining.isFinite else {
        return GatewayQuotaMeterPresentation(
            fraction: nil,
            statusText: usage.balanceStatus == "available"
                ? "额度可用"
                : providerUsageStatusText(usage.balanceStatus),
            tone: usage.balanceStatus == "available" ? .normal : .unavailable,
            warningThreshold: nil
        )
    }

    let progressReference = gatewayQuotaProgressReference(unit: usage.unit)
    let warningThreshold = gatewayQuotaWarningThreshold(unit: usage.unit)
    let fraction = max(0, min(remaining / progressReference, 1))
    if remaining <= 0 {
        return GatewayQuotaMeterPresentation(
            fraction: 0,
            statusText: "额度耗尽",
            tone: .critical,
            warningThreshold: warningThreshold
        )
    }
    if remaining < warningThreshold {
        return GatewayQuotaMeterPresentation(
            fraction: fraction,
            statusText: "余额偏低",
            tone: .warning,
            warningThreshold: warningThreshold
        )
    }
    return GatewayQuotaMeterPresentation(
        fraction: fraction,
        statusText: "余额充足",
        tone: .normal,
        warningThreshold: warningThreshold
    )
}

func gatewayQuotaProgressReference(unit _: String?) -> Double {
    gatewayQuotaDefaultProgressReference
}

func gatewayQuotaWarningThreshold(unit _: String?) -> Double {
    gatewayQuotaDefaultWarningThreshold
}

func gatewayQuotaAmountText(_ value: Double, unit: String?) -> String {
    guard value.isFinite else { return "—" }
    let formatter = NumberFormatter()
    formatter.locale = Locale(identifier: "en_US_POSIX")
    formatter.numberStyle = .decimal
    formatter.usesGroupingSeparator = true
    formatter.minimumFractionDigits = 2
    formatter.maximumFractionDigits = 2
    let amount = formatter.string(from: NSNumber(value: abs(value))) ?? String(format: "%.2f", abs(value))
    let sign = value < 0 ? "-" : ""
    switch unit?.trimmingCharacters(in: .whitespacesAndNewlines).uppercased() {
    case "USD": return "\(sign)$\(amount)"
    case "CNY", "RMB": return "\(sign)¥\(amount)"
    case let unit? where !unit.isEmpty: return "\(sign)\(amount) \(unit)"
    default: return "\(sign)\(amount)"
    }
}

func gatewayQuotaBalanceText(_ usage: ManageProviderUsageResponse.Usage) -> String {
    if usage.unlimited { return "无限额度" }
    if let remaining = usage.remaining, remaining.isFinite {
        return gatewayQuotaAmountText(remaining, unit: usage.unit)
    }
    return providerUsageBalanceText(usage)
}

func gatewayQuotaSub2ApiRateText(
    _ account: ManageSub2ApiAccountPoolResponse.Account,
    fallbackUsage: ManageProviderUsageResponse.Usage?
) -> String? {
    let accountRate = account.upstreamBilling.effectiveRateMultiplier
        ?? account.upstreamBilling.resolvedRateMultiplier
        ?? account.localRateMultiplier
    if let accountRate {
        return providerUsageMultiplierText(accountRate)
    }
    return providerUsageMultiplierText(
        fallbackUsage?.effectiveRateMultiplier ?? fallbackUsage?.resolvedRateMultiplier
    )
}

func gatewayQuotaMeterPresentation(
    _ balance: ManageSub2ApiAccountPoolResponse.Account.Balance
) -> GatewayQuotaMeterPresentation {
    if balance.accountValid == false {
        return GatewayQuotaMeterPresentation(
            fraction: 0,
            statusText: providerUsageAccountWarning(balance.accountStatus) ?? "账户不可用",
            tone: .critical,
            warningThreshold: nil
        )
    }
    if balance.unlimited {
        return GatewayQuotaMeterPresentation(
            fraction: 1,
            statusText: "无限额度",
            tone: .normal,
            warningThreshold: nil
        )
    }
    guard let remaining = balance.remaining, remaining.isFinite else {
        return GatewayQuotaMeterPresentation(
            fraction: nil,
            statusText: sub2ApiCapabilityStateText(balance.state),
            tone: balance.state == "available" ? .normal : .unavailable,
            warningThreshold: nil
        )
    }

    let progressReference = gatewayQuotaProgressReference(unit: balance.unit)
    let warningThreshold = gatewayQuotaWarningThreshold(unit: balance.unit)
    let fraction = max(0, min(remaining / progressReference, 1))
    if remaining <= 0 {
        return GatewayQuotaMeterPresentation(
            fraction: 0,
            statusText: "额度耗尽",
            tone: .critical,
            warningThreshold: warningThreshold
        )
    }
    if remaining < warningThreshold {
        return GatewayQuotaMeterPresentation(
            fraction: fraction,
            statusText: "余额偏低",
            tone: .warning,
            warningThreshold: warningThreshold
        )
    }
    return GatewayQuotaMeterPresentation(
        fraction: fraction,
        statusText: "余额充足",
        tone: .normal,
        warningThreshold: warningThreshold
    )
}

func gatewayQuotaSiteDisplayName(_ siteURL: String?) -> String? {
    guard let siteURL,
          let host = URLComponents(string: siteURL)?.host?.lowercased()
    else { return nil }

    let labels = host.split(separator: ".").map(String.init)
    guard labels.count >= 2 else { return labels.first }
    let infrastructureLabels: Set<String> = ["api", "openai", "vip", "www"]
    let registrableLabels = labels.dropLast()
    return registrableLabels.first(where: { !infrastructureLabels.contains($0) })
        ?? registrableLabels.last
}

struct GatewayQuotaDock: View {
    private static let fullProviderWidth: CGFloat = 160
    private static let compactProviderWidth: CGFloat = 116
    private static let compactProviderBreakpoint: CGFloat = 516

    @EnvironmentObject private var model: AppModel
    @State private var selectedProviderName: String?
    @State private var providerUsage: ManageProviderUsageResponse?
    @State private var providerUsageError: String?
    @State private var providerUsageLoading = false
    @State private var recentAccountResponse: ManageProviderRecentAccountResponse?
    @State private var recentAccountError: String?
    @State private var recentAccountLoading = false
    @State private var gatewayGeneration = 0
    @State private var usageRefreshGeneration = 0
    @State private var showsUsageDetails = false

    private var providers: [ManageGatewayProvider] {
        (model.gateway?.providers ?? []).sorted { lhs, rhs in
            if lhs.enabled != rhs.enabled { return lhs.enabled && !rhs.enabled }
            if lhs.weight != rhs.weight { return lhs.weight > rhs.weight }
            return lhs.name.localizedStandardCompare(rhs.name) == .orderedAscending
        }
    }

    private var selectedProvider: ManageGatewayProvider? {
        guard let selectedProviderName else { return nil }
        return providers.first { $0.name == selectedProviderName }
    }

    private var recentAccount: ManageSub2ApiAccountPoolResponse.Account? {
        // The provider usage endpoint can report an aggregate wallet balance.
        // Once the daemon identifies the latest Sub2API account used by this
        // Provider, that account is the value the user is actually routing
        // through and must take precedence over the aggregate snapshot.
        guard let accountID = recentAccountResponse?.account?.accountId else { return nil }
        return model.sub2ApiAccountPool?.accounts.first { $0.id == accountID }
    }

    private var usageTaskID: GatewayQuotaRequestID {
        GatewayQuotaRequestID(
            providerName: selectedProviderName,
            gatewayGeneration: gatewayGeneration,
            refreshGeneration: usageRefreshGeneration
        )
    }

    var body: some View {
        let requestID = usageTaskID
        GatewayQuotaDockSurface {
            GeometryReader { proxy in
                let usesCompactProvider = proxy.size.width <= Self.compactProviderBreakpoint

                HStack(spacing: 10) {
                    providerPicker(usesCompactDetails: usesCompactProvider)
                        .padding(.leading, 14)
                        .frame(
                            width: usesCompactProvider
                                ? Self.compactProviderWidth
                                : Self.fullProviderWidth,
                            alignment: .leading
                        )

                    quotaSummary
                        .frame(minWidth: 150, maxWidth: .infinity)

                    actionButtons
                        .padding(.trailing, 12)
                        .frame(width: 142, alignment: .trailing)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .center)
            }
        }
        .frame(maxWidth: .infinity)
        .frame(height: 52)
        .background(alignment: .bottom) {
            LinearGradient(
                colors: [
                    Color(nsColor: .windowBackgroundColor).opacity(0),
                    Color(nsColor: .windowBackgroundColor).opacity(0.22),
                ],
                startPoint: .top,
                endPoint: .bottom
            )
            .frame(height: 44)
            .frame(maxWidth: .infinity)
            .padding(.bottom, -8)
            .allowsHitTesting(false)
            .accessibilityHidden(true)
        }
        .zIndex(1)
        .accessibilityIdentifier("gateway.quotaDock")
        .task {
            if model.gateway == nil {
                _ = await model.loadSection(.gateway)
            }
            synchronizeSelectedProvider()
        }
        .onReceive(model.$gateway) { _ in
            providerUsage = nil
            providerUsageError = nil
            providerUsageLoading = false
            recentAccountResponse = nil
            recentAccountError = nil
            recentAccountLoading = false
            gatewayGeneration &+= 1
            synchronizeSelectedProvider()
        }
        .onChange(of: selectedProviderName) { _ in
            providerUsage = nil
            providerUsageError = nil
            recentAccountResponse = nil
            recentAccountError = nil
        }
        .task(id: requestID) {
            async let providerLoad: Void = loadSelectedProviderUsage(requestID: requestID)
            async let accountLoad: Void = loadRecentAccountContext(
                requestID: requestID,
                forceAccountRefresh: requestID.refreshGeneration > 0
            )
            _ = await (providerLoad, accountLoad)

            while !Task.isCancelled, requestID == usageTaskID {
                do {
                    try await Task.sleep(for: .seconds(8))
                } catch {
                    return
                }
                await loadRecentAccount(requestID: requestID, showsLoading: false)
            }
        }
    }

    private func providerPicker(usesCompactDetails: Bool) -> some View {
        Menu {
            if providers.isEmpty {
                Text("尚未配置 Provider")
            } else {
                ForEach(providers) { provider in
                    Button {
                        selectedProviderName = provider.name
                    } label: {
                        if provider.name == selectedProviderName {
                            Label(provider.name, systemImage: "checkmark")
                        } else {
                            Text(provider.name)
                        }
                    }
                }
            }
        } label: {
            HStack(spacing: 10) {
                if let provider = selectedProvider {
                    ProviderLogoView(
                        providerType: provider.providerType,
                        compatibility: provider.compatibility,
                        providerName: provider.name,
                        size: 28
                    )
                } else {
                    Image(systemName: "point.3.connected.trianglepath.dotted")
                        .font(.system(size: 17, weight: .medium))
                        .foregroundStyle(.secondary)
                        .frame(width: 28, height: 28)
                }

                if usesCompactDetails {
                    Text(accountTitle)
                        .font(.callout.weight(.semibold))
                        .lineLimit(1)
                } else {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(accountTitle)
                            .font(.callout.weight(.semibold))
                            .lineLimit(1)
                        Text(accountSubtitle)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                }
                Spacer(minLength: 2)
                Image(systemName: "chevron.up.chevron.down")
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(.tertiary)
            }
            .contentShape(Rectangle())
        }
        .menuStyle(.borderlessButton)
        .disabled(providers.isEmpty)
        .help(recentAccount == nil ? "选择要查看额度的 Provider" : "最近使用账号；点此切换 Provider")
        .accessibilityLabel("选择额度 Provider")
        .accessibilityValue(accountTitle)
    }

    private var accountTitle: String {
        gatewayQuotaSiteDisplayName(recentAccount?.siteUrl)
            ?? recentAccount?.name
            ?? selectedProvider?.name
            ?? "AI Gateway"
    }

    private var accountSubtitle: String {
        if let recentAccount { return recentAccount.name }
        if recentAccountLoading { return "正在识别最近使用账号" }
        if recentAccountError != nil { return providerSubtitle }
        return providerSubtitle
    }

    private var providerSubtitle: String {
        guard let provider = selectedProvider else { return "尚未配置 Provider" }
        if !provider.enabled { return "已停用" }
        let protocolName = gatewayProtocolDisplayName(
            provider.providerType,
            compatibility: provider.compatibility
        )
        return provider.models.isEmpty ? protocolName : "\(protocolName) · \(provider.models.count) 个模型"
    }

    private var quotaSummary: some View {
        VStack(alignment: .leading, spacing: 5) {
            ViewThatFits(in: .horizontal) {
                fullQuotaSummaryLine
                compactQuotaSummaryLine
                minimalQuotaSummaryLine
            }

            GatewayQuotaProgressTrack(
                fraction: meterPresentation.fraction,
                tint: meterTint
            )
            .frame(height: 5)
            .help(progressHelp)
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel(quotaAccessibilityLabel)
    }

    private var fullQuotaSummaryLine: some View {
        HStack(alignment: .firstTextBaseline, spacing: 7) {
            Text("剩余额度")
                .font(.caption)
                .foregroundStyle(.secondary)
            quotaBalanceLabel
            quotaStatusLabel
            Spacer(minLength: 6)
            quotaRateLabel
        }
    }

    private var compactQuotaSummaryLine: some View {
        HStack(alignment: .firstTextBaseline, spacing: 7) {
            quotaBalanceLabel
            quotaStatusLabel
            Spacer(minLength: 6)
            quotaRateLabel
        }
    }

    private var minimalQuotaSummaryLine: some View {
        HStack(alignment: .firstTextBaseline, spacing: 7) {
            quotaBalanceLabel
            Spacer(minLength: 6)
            quotaRateLabel
        }
    }

    private var quotaBalanceLabel: some View {
        Text(balanceText)
            .font(.callout.monospacedDigit().weight(.semibold))
            .lineLimit(1)
    }

    private var quotaStatusLabel: some View {
        Text(meterPresentation.statusText)
            .font(.caption)
            .foregroundStyle(meterTint)
            .lineLimit(1)
    }

    @ViewBuilder
    private var quotaRateLabel: some View {
        if let rateText {
            Text("倍率 \(rateText)")
                .font(.caption.monospacedDigit().weight(.medium))
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
    }

    private var actionButtons: some View {
        HStack(spacing: 6) {
            GatewayDockIconButton(
                systemName: "arrow.clockwise",
                help: "刷新额度",
                disabled: selectedProvider?.secretSet != true || dockIsRefreshing,
                busy: dockIsRefreshing
            ) {
                usageRefreshGeneration &+= 1
            }
            GatewayDockIconButton(
                systemName: "info.circle",
                help: "额度详情",
                disabled: selectedProvider == nil
            ) {
                showsUsageDetails.toggle()
            }
            .popover(isPresented: $showsUsageDetails, arrowEdge: .bottom) {
                GatewayQuotaDetailsPopover(
                    provider: selectedProvider,
                    usageResponse: providerUsage,
                    recentAccount: recentAccount,
                    error: providerUsageLoading ? nil : providerUsageError,
                    presentation: meterPresentation
                )
            }
            GatewayDockIconButton(
                systemName: "list.bullet.rectangle",
                help: "请求日志"
            ) {
                model.selection = .requestLogs
            }
            GatewayDockIconButton(
                systemName: "slider.horizontal.3",
                help: "AI 网关设置"
            ) {
                model.selection = .gateway
            }
        }
        .accessibilityElement(children: .contain)
    }

    private var dockIsRefreshing: Bool {
        providerUsageLoading || recentAccountLoading || model.sub2ApiAccountPoolLoading
    }

    private var balanceText: String {
        if let account = recentAccount {
            return sub2ApiBalanceText(account.upstreamBalance)
        }
        if let usage = providerUsage?.usage {
            return gatewayQuotaBalanceText(usage)
        }
        if selectedProvider?.secretSet == false { return "未保存 API Key" }
        if providerUsageLoading { return "正在读取" }
        if providerUsageError != nil { return "暂不可用" }
        return selectedProvider == nil ? "未配置" : "尚未读取"
    }

    private var meterPresentation: GatewayQuotaMeterPresentation {
        if let account = recentAccount {
            let presentation = gatewayQuotaMeterPresentation(account.upstreamBalance)
            let cachedState: GatewayQuotaCachedState = if model.sub2ApiAccountPoolLoading {
                .refreshing
            } else if model.sub2ApiAccountPoolError != nil {
                .refreshFailed
            } else {
                .fresh
            }
            return gatewayQuotaCachedPresentation(presentation, state: cachedState)
        }
        if let usage = providerUsage?.usage {
            let presentation = gatewayQuotaMeterPresentation(usage)
            let cachedState: GatewayQuotaCachedState = if providerUsageLoading {
                .refreshing
            } else if providerUsageError != nil {
                .refreshFailed
            } else {
                .fresh
            }
            return gatewayQuotaCachedPresentation(presentation, state: cachedState)
        }
        if selectedProvider?.secretSet == false {
            return GatewayQuotaMeterPresentation(
                fraction: nil,
                statusText: "需要 API Key",
                tone: .unavailable,
                warningThreshold: nil
            )
        }
        if providerUsageLoading {
            return GatewayQuotaMeterPresentation(
                fraction: nil,
                statusText: "正在读取",
                tone: .unavailable,
                warningThreshold: nil
            )
        }
        if providerUsageError != nil {
            return GatewayQuotaMeterPresentation(
                fraction: nil,
                statusText: "读取失败",
                tone: .critical,
                warningThreshold: nil
            )
        }
        return GatewayQuotaMeterPresentation(
            fraction: nil,
            statusText: selectedProvider == nil ? "未配置" : "等待刷新",
            tone: .unavailable,
            warningThreshold: nil
        )
    }

    private var meterTint: Color {
        switch meterPresentation.tone {
        case .normal: .primary.opacity(0.72)
        case .warning: .orange
        case .critical: .red
        case .unavailable: .secondary.opacity(0.55)
        }
    }

    private var rateText: String? {
        if let account = recentAccount {
            return gatewayQuotaSub2ApiRateText(account, fallbackUsage: providerUsage?.usage)
        }
        guard let usage = providerUsage?.usage else { return nil }
        return providerUsageMultiplierText(
            usage.effectiveRateMultiplier ?? usage.resolvedRateMultiplier
        )
    }

    private var quotaAccessibilityLabel: String {
        var values = [
            accountTitle,
            "剩余额度 \(balanceText)",
            meterPresentation.statusText,
        ]
        if let rateText {
            values.append("倍率 \(rateText)")
        }
        return values.joined(separator: "，")
    }

    private var progressHelp: String {
        guard let warningThreshold = meterPresentation.warningThreshold else {
            return meterPresentation.statusText
        }
        let unit = recentAccount?.upstreamBalance.unit ?? providerUsage?.usage.unit
        let progressReference = gatewayQuotaProgressReference(unit: unit)
        return "上游未提供总额度；进度按 \(gatewayQuotaAmountText(progressReference, unit: unit)) 参考值显示，低于 \(gatewayQuotaAmountText(warningThreshold, unit: unit)) 时提示余额偏低。"
    }

    private func synchronizeSelectedProvider() {
        if let selectedProviderName,
           providers.contains(where: { $0.name == selectedProviderName })
        {
            return
        }
        selectedProviderName = providers.first(where: { $0.enabled && $0.secretSet })?.name
            ?? providers.first(where: { $0.enabled })?.name
            ?? providers.first?.name
    }

    private func loadSelectedProviderUsage(requestID: GatewayQuotaRequestID) async {
        guard requestID == usageTaskID else { return }
        guard let provider = selectedProvider else {
            providerUsage = nil
            providerUsageError = nil
            providerUsageLoading = false
            return
        }
        guard provider.secretSet else {
            providerUsage = nil
            providerUsageError = nil
            providerUsageLoading = false
            return
        }

        let providerName = provider.name
        if providerUsage?.providerName != providerName {
            providerUsage = nil
        }
        if providerUsage == nil {
            providerUsageError = nil
        }
        providerUsageLoading = true
        defer {
            if requestID == usageTaskID {
                providerUsageLoading = false
            }
        }
        do {
            guard !Task.isCancelled, requestID == usageTaskID else { return }
            let response = try await model.fetchGatewayProviderUsage(providerName: providerName)
            guard !Task.isCancelled, requestID == usageTaskID else { return }
            providerUsage = response
            providerUsageError = nil
        } catch is CancellationError {
            return
        } catch {
            guard !Task.isCancelled, requestID == usageTaskID else { return }
            providerUsageError = error.localizedDescription
        }
    }

    private func loadRecentAccountContext(
        requestID: GatewayQuotaRequestID,
        forceAccountRefresh: Bool
    ) async {
        guard requestID == usageTaskID else { return }
        guard model.sub2ApiAdmin?.configured == true else {
            recentAccountResponse = nil
            recentAccountError = nil
            recentAccountLoading = false
            return
        }

        recentAccountLoading = true
        async let poolLoad: Void = model.refreshSub2ApiAccountPool(
            forceBillingRefresh: forceAccountRefresh
        )
        await loadRecentAccount(requestID: requestID, showsLoading: false)
        _ = await poolLoad
        if requestID == usageTaskID {
            recentAccountLoading = false
        }
    }

    private func loadRecentAccount(
        requestID: GatewayQuotaRequestID,
        showsLoading: Bool
    ) async {
        guard requestID == usageTaskID,
              let providerName = requestID.providerName,
              model.sub2ApiAdmin?.configured == true
        else { return }

        if showsLoading { recentAccountLoading = true }
        defer {
            if showsLoading, requestID == usageTaskID {
                recentAccountLoading = false
            }
        }
        do {
            let response = try await model.fetchGatewayProviderRecentAccount(
                providerName: providerName
            )
            guard !Task.isCancelled, requestID == usageTaskID else { return }
            recentAccountResponse = response
            recentAccountError = nil
        } catch is CancellationError {
            return
        } catch {
            guard !Task.isCancelled, requestID == usageTaskID else { return }
            recentAccountError = error.localizedDescription
        }
    }
}

private struct GatewayQuotaProgressTrack: View {
    let fraction: Double?
    let tint: Color

    var body: some View {
        GeometryReader { proxy in
            let resolved = max(0, min(fraction ?? 0, 1))
            ZStack(alignment: .leading) {
                Capsule()
                    .fill(Color.primary.opacity(0.09))
                if resolved > 0 {
                    Capsule()
                        .fill(tint)
                        .frame(width: max(5, proxy.size.width * resolved))
                }
            }
        }
        .accessibilityHidden(true)
    }
}

private struct GatewayQuotaDockSurface<Content: View>: View {
    @ViewBuilder let content: Content

    init(@ViewBuilder content: () -> Content) {
        self.content = content()
    }

    var body: some View {
        Group {
            if #available(macOS 26, *) {
                GlassEffectContainer(spacing: 0) {
                    content
                        .frame(height: 52)
                        .glassEffect(.regular.interactive(), in: Capsule())
                        .glassEffectTransition(.materialize)
                }
            } else {
                content
                    .frame(height: 52)
                    .background(.ultraThinMaterial, in: Capsule())
            }
        }
    }
}

private struct GatewayDockIconButton: View {
    let systemName: String
    let help: String
    var disabled = false
    var busy = false
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            Group {
                if busy {
                    ProgressView()
                        .controlSize(.small)
                } else {
                    Image(systemName: systemName)
                        .font(.system(size: 15, weight: .regular))
                }
            }
            .frame(width: 28, height: 28)
            .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
            .background(
                Color.primary.opacity(hovering && !disabled ? 0.08 : 0),
                in: RoundedRectangle(cornerRadius: 8, style: .continuous)
            )
        }
        .buttonStyle(GatewayDockIconButtonStyle())
        .frame(width: 28, height: 28)
        .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        .disabled(disabled)
        .opacity(disabled ? 0.38 : 1)
        .help(help)
        .accessibilityLabel(help)
        .accessibilityValue(busy ? "正在刷新" : "")
        .onHover { hovering = $0 }
        .animation(.easeOut(duration: 0.12), value: hovering)
    }
}

private struct GatewayDockIconButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed ? 0.92 : 1)
            .opacity(configuration.isPressed ? 0.72 : 1)
            .animation(.easeOut(duration: 0.12), value: configuration.isPressed)
    }
}

private struct GatewayQuotaDetailsPopover: View {
    let provider: ManageGatewayProvider?
    let usageResponse: ManageProviderUsageResponse?
    let recentAccount: ManageSub2ApiAccountPoolResponse.Account?
    let error: String?
    let presentation: GatewayQuotaMeterPresentation

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 10) {
                if let provider {
                    ProviderLogoView(
                        providerType: provider.providerType,
                        compatibility: provider.compatibility,
                        providerName: provider.name,
                        size: 32
                    )
                    VStack(alignment: .leading, spacing: 2) {
                        Text(
                            gatewayQuotaSiteDisplayName(recentAccount?.siteUrl)
                                ?? recentAccount?.name
                                ?? provider.name
                        )
                            .font(.headline)
                        Text(accountSubtitle(provider))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    }
                }
                Spacer()
                Text(presentation.statusText)
                    .font(.caption.weight(.medium))
                    .foregroundStyle(statusTint)
            }

            Divider()

            if let recentAccount {
                LabeledContent("最近使用账号", value: recentAccount.name)
                LabeledContent(
                    "剩余额度",
                    value: sub2ApiBalanceText(recentAccount.upstreamBalance)
                )
                LabeledContent(
                    "当前倍率",
                    value: gatewayQuotaSub2ApiRateText(
                        recentAccount,
                        fallbackUsage: usageResponse?.usage
                    ) ?? sub2ApiUpstreamRateText(recentAccount.upstreamBilling)
                )
                if let plan = recentAccount.upstreamBalance.planName, !plan.isEmpty {
                    LabeledContent("账户方案", value: plan)
                }
                if let observedAt = recentAccount.upstreamBalance.observedAt,
                   !observedAt.isEmpty
                {
                    LabeledContent("余额观测", value: observedAt)
                }
                if let warningThreshold = presentation.warningThreshold {
                    let unit = recentAccount.upstreamBalance.unit
                    LabeledContent(
                        "进度参考值",
                        value: gatewayQuotaAmountText(
                            gatewayQuotaProgressReference(unit: unit),
                            unit: unit
                        )
                    )
                    LabeledContent(
                        "余额偏低线",
                        value: gatewayQuotaAmountText(warningThreshold, unit: unit)
                    )
                }
            } else if let usage = usageResponse?.usage {
                LabeledContent("剩余额度", value: gatewayQuotaBalanceText(usage))
                LabeledContent(
                    "当前倍率",
                    value: providerUsageMultiplierText(
                        usage.effectiveRateMultiplier ?? usage.resolvedRateMultiplier
                    ) ?? providerUsageBillingText(usage)
                )
                if let plan = usage.planName, !plan.isEmpty {
                    LabeledContent("账户方案", value: plan)
                }
                if let observedAt = usage.observedAt, !observedAt.isEmpty {
                    LabeledContent("上游观测", value: observedAt)
                }
                if let warningThreshold = presentation.warningThreshold {
                    LabeledContent(
                        "进度参考值",
                        value: gatewayQuotaAmountText(
                            gatewayQuotaProgressReference(unit: usage.unit),
                            unit: usage.unit
                        )
                    )
                    LabeledContent(
                        "余额偏低线",
                        value: gatewayQuotaAmountText(warningThreshold, unit: usage.unit)
                    )
                }
                if let error {
                    Label("刷新失败，当前显示上次成功读取的数据。", systemImage: "exclamationmark.triangle.fill")
                        .font(.callout)
                        .foregroundStyle(.orange)
                    Text(error)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            } else if let error {
                Label(error, systemImage: "exclamationmark.triangle.fill")
                    .font(.callout)
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            } else {
                Text(provider?.secretSet == false ? "请先保存 Provider API Key。" : "尚未读取额度。")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }
        }
        .padding(18)
        .frame(width: 340)
    }

    private func accountSubtitle(_ provider: ManageGatewayProvider) -> String {
        if let recentAccount { return recentAccount.name }
        return gatewayProtocolDisplayName(
            provider.providerType,
            compatibility: provider.compatibility
        )
    }

    private var statusTint: Color {
        switch presentation.tone {
        case .normal: .secondary
        case .warning: .orange
        case .critical: .red
        case .unavailable: .secondary
        }
    }
}
