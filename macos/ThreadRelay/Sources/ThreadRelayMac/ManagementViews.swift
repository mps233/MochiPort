import AppKit
import SwiftUI

private struct ManagementPageInsets: ViewModifier {
    let topPadding: CGFloat

    func body(content: Content) -> some View {
        if #available(macOS 14.0, *) {
            content
                .contentMargins(
                    .horizontal,
                    ThreadRelayPageLayout.horizontalPadding,
                    for: .scrollContent
                )
                .contentMargins(
                    .top,
                    topPadding,
                    for: .scrollContent
                )
        } else {
            content
                .padding(.horizontal, ThreadRelayPageLayout.horizontalPadding)
                .padding(.top, topPadding)
        }
    }
}

private extension View {
    func managementPageInsets(
        topPadding: CGFloat = ThreadRelayPageLayout.topPadding
    ) -> some View {
        modifier(ManagementPageInsets(topPadding: topPadding))
    }
}

struct CodexAccessView: View {
    @EnvironmentObject private var model: AppModel
    @State private var confirmsRestore = false
    @State private var showsEnhancedDetails = false
    @State private var showsTechnicalDetails = false

    private var gatewayServiceEnabled: Bool? {
        model.gateway?.enabled ?? model.dashboard?.aiGatewayEnabled
    }

    var body: some View {
        Form {
            if let error = model.sectionErrors[.codex] {
                InlineManagementError(
                    message: error,
                    retry: { Task { await model.loadSection(.codex, force: true) } }
                )
            }

            if let status = model.codexStatus {
                Section("Codex") {
                    CodexStatusFormRow(
                        status: status,
                        gatewayEnabled: gatewayServiceEnabled
                    )
                }
                requestPathSection(status, gatewayEnabled: gatewayServiceEnabled)
                providerSection(status)
                environmentSection(status)
                diagnosticsSection(status)
                enhancedLaunchSection
            } else if !model.isLoading(.codex), model.sectionErrors[.codex] == nil {
                Section("Codex") {
                    ManagementEmptyState(
                        title: "还没连接 Codex",
                        message: "打开开关后，这里会显示连接状态。",
                        symbol: "app.badge.checkmark"
                    )
                }
            }
        }
        .formStyle(.grouped)
        .scrollContentBackground(.hidden)
        .managementPageInsets()
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .overlay {
            if model.isLoading(.codex) {
                CodexLoadingSurface()
            }
        }
        .background(Color(nsColor: .windowBackgroundColor))
        .task { await model.loadSection(.codex) }
        .alert("恢复原来的设置？", isPresented: $confirmsRestore) {
            Button("取消", role: .cancel) {}
            Button("恢复", role: .destructive) {
                Task { await model.uninstallCodex() }
            }
        } message: {
            Text("会恢复 Codex 原来的连接，并关闭 MochiPort。不会删除会话记录。")
        }
        .sheet(
            isPresented: Binding(
                get: { model.codexEnhancedWaitingForAppExit },
                set: { presented in
                    if !presented {
                        Task { await model.cancelCodexEnhancedLaunch() }
                    }
                }
            )
        ) {
            EnhancedLaunchWaitSheet(
                onCancel: { Task { await model.cancelCodexEnhancedLaunch() } }
            )
        }
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                if let status = model.codexStatus, status.providerMode != "direct-api" {
                    Button {
                        Task {
                            if status.configured {
                                await model.beginCodexEnhancedLaunch()
                            } else {
                                await model.configureCodex()
                            }
                        }
                    } label: {
                        if model.codexEnhancedLaunchInProgress {
                            ProgressView()
                                .controlSize(.small)
                        } else {
                            Label(
                                status.configured
                                    ? (model.codexEnhancedLaunchError == nil ? "启动 Codex" : "重新尝试")
                                    : "连接 Codex",
                                systemImage: status.configured ? "play.fill" : "link.badge.plus"
                            )
                        }
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(status.configured && model.codexEnhancedLaunchInProgress)
                    .accessibilityLabel(status.configured ? "启动 Codex" : "连接 Codex")
                }
            }
            ToolbarItemGroup(placement: .automatic) {
                if let status = model.codexStatus {
                    if status.providerMode != "direct-api" {
                        Button {
                            Task { await model.repairCodex() }
                        } label: {
                            Image(systemName: "wrench.and.screwdriver")
                        }
                        .disabled(!(status.configOk && status.authOk))
                        .help("检查设置")
                        .accessibilityLabel("检查设置")
                    }

                    Button {
                        Task { await model.refreshCodexModels() }
                    } label: {
                        Image(systemName: "arrow.triangle.2.circlepath")
                    }
                    .help("刷新模型列表")
                    .accessibilityLabel("刷新模型列表")

                    Menu {
                        if status.providerMode != "direct-api" {
                            Button(status.configured ? "重新连接" : "连接 Codex") {
                                Task { await model.configureCodex() }
                            }
                            Divider()
                        } else {
                            Button("连接 MochiPort") {
                                Task { await model.configureCodex() }
                            }
                            Divider()
                        }
                        Button("恢复原来的设置", role: .destructive) {
                            confirmsRestore = true
                        }
                        .disabled(!(status.configured || status.configOk || status.authOk))
                    } label: {
                        Image(systemName: "ellipsis")
                    }
                    .menuStyle(.borderlessButton)
                    .help("更多操作")
                    .accessibilityLabel("更多操作")
                }
            }
        }
    }

    private func requestPathSection(
        _ status: ManageCodexStatus,
        gatewayEnabled: Bool?
    ) -> some View {
        return Section {
            CodexRequestPathRow(
                mode: status.providerMode,
                gatewayEnabled: gatewayEnabled,
                isUpdating: model.isLoading(.codex),
                setGatewayEnabled: { enabled in
                    enabled
                        ? await model.configureCodex()
                        : await model.uninstallCodex()
                }
            )
        } header: {
            Text("连接 Codex")
        }
    }

    private func providerSection(_ status: ManageCodexStatus) -> some View {
        let activeProvider = status.providers.first { provider in
            provider.name == status.activeProvider
        }
        let otherProviders = status.providers.filter { provider in
            provider.name != activeProvider?.name
        }

        return Section {
            if let activeProvider {
                CodexProviderRow(provider: activeProvider)
            } else if status.providers.isEmpty {
                Text("还没有模型服务。")
                    .foregroundStyle(.secondary)
            } else {
                Text("还没有选择模型服务。")
                    .foregroundStyle(.secondary)
            }

            if !otherProviders.isEmpty {
                DisclosureGroup {
                    ForEach(otherProviders) { provider in
                        CodexProviderRow(provider: provider)
                    }
                } label: {
                    HStack {
                        Text("其他已配置服务")
                        Spacer()
                        Text("\(otherProviders.count) 个")
                            .foregroundStyle(.secondary)
                    }
                }
            }
        } header: {
            Text("当前模型服务")
        } footer: {
            Text(otherProviders.isEmpty
                ? "Codex 当前会使用这里的服务。"
                : "Codex 当前使用上面的服务，其他服务不会参与请求。")
        }
    }

    private func environmentSection(_ status: ManageCodexStatus) -> some View {
        Section("本机设置") {
            LabeledContent("Codex 文件夹") {
                Text(status.codexHome)
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .textSelection(.enabled)
            }

            LabeledContent("图片功能") {
                Text(status.imageGenerationEnabled ? "已启用" : "未启用")
                    .foregroundStyle(.secondary)
            }
        }
    }

    private func diagnosticsSection(_ status: ManageCodexStatus) -> some View {
        Section("检查设置") {
            DisclosureGroup(isExpanded: $showsTechnicalDetails) {
                CodexDiagnosticList(status: status)
                    .padding(.top, 4)
            } label: {
                Label("查看详细信息", systemImage: "stethoscope")
            }
        }
    }

    @ViewBuilder
    private var enhancedLaunchSection: some View {
        Section("启动 Codex") {
            if let operation = model.codexEnhancedOperation {
                EnhancedLaunchProgressRow(
                    operation: operation,
                    legacyFallback: model.codexEnhancedUsesLegacyFallback,
                    error: model.codexEnhancedLaunchError,
                    canCancel: model.canCancelCodexEnhancedLaunch,
                    cancel: { Task { await model.cancelCodexEnhancedLaunch() } }
                )
                .padding(.vertical, 4)
            } else if let error = model.codexEnhancedLaunchError {
                Label(error, systemImage: "exclamationmark.triangle.fill")
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }

            DisclosureGroup(isExpanded: $showsEnhancedDetails) {
                VStack(alignment: .leading, spacing: 9) {
                    EnhancedLaunchFeatureRow(
                        symbol: "checkmark.seal",
                        text: "启动前检查并保存设置，然后连接 MochiPort。"
                    )
                    EnhancedLaunchFeatureRow(
                        symbol: "bolt",
                        text: "启动后确认连接是否正常。"
                    )
                    EnhancedLaunchFeatureRow(
                        symbol: "eye",
                        text: "让自定义模型出现在 Codex 里。"
                    )
                    EnhancedLaunchFeatureRow(
                        symbol: "puzzlepiece.extension",
                        text: "兼容不同语言和安装方式。"
                    )
                }
                .padding(.top, 4)
            } label: {
                Label("启动前会做什么", systemImage: "sparkles")
            }
        }
    }

}

private struct CodexStatusFormRow: View {
    let status: ManageCodexStatus
    let gatewayEnabled: Bool?
    private var isDirectApiMode: Bool { status.providerMode == "direct-api" }
    private var isUnknownMode: Bool {
        status.providerMode == nil || status.providerMode == "unknown"
    }
    private var gatewayUnavailable: Bool {
        status.providerMode == "threadrelay" && gatewayEnabled == false
    }
    private var remoteControlReady: Bool {
        return !status.remoteControlSupported || status.remoteControlConfigured
    }
    private var needsAttention: Bool {
        guard !isDirectApiMode else { return false }
        return isUnknownMode || gatewayUnavailable || !status.configOk || !status.authOk || !status.providerOk
            || !status.guiConfigured || !remoteControlReady
    }
    private var title: String {
        if isDirectApiMode { return "Codex 已准备好" }
        if isUnknownMode { return "还没连接 Codex" }
        if gatewayUnavailable { return "MochiPort 未开启" }
        return needsAttention ? "还需要处理" : "Codex 已连接"
    }
    private var subtitle: String {
        if isDirectApiMode {
            return "正在使用 Codex 原来的设置。"
        }
        if isUnknownMode {
            return "打开下面的开关即可连接。"
        }
        if gatewayUnavailable {
            return "请打开 MochiPort。"
        }
        if needsAttention {
            return "请检查下面的提示。"
        }
        return "已连接，可以使用。"
    }
    private var stateSymbol: String {
        return needsAttention ? "exclamationmark.circle.fill" : "checkmark.circle.fill"
    }
    private var stateColor: Color {
        return needsAttention ? .orange : .green
    }
    private var stateTitle: String {
        if isDirectApiMode { return "原来的设置" }
        if isUnknownMode { return "未连接" }
        if gatewayUnavailable { return "未开启" }
        return needsAttention ? "需处理" : "已连接"
    }

    var body: some View {
        HStack(alignment: .center, spacing: 12) {
            VStack(alignment: .leading, spacing: 4) {
                Text(title)
                    .font(.body.weight(.semibold))
                Text(subtitle)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
            }

            Spacer(minLength: 16)

            Label(stateTitle, systemImage: stateSymbol)
                .font(.callout.weight(.medium))
                .foregroundStyle(stateColor)
                .fixedSize()
        }
        .accessibilityElement(children: .combine)
    }
}

private struct CodexRequestPathRow: View {
    let mode: String?
    let gatewayEnabled: Bool?
    let isUpdating: Bool
    let setGatewayEnabled: (Bool) async -> Bool

    @State private var pendingEnabled: Bool?
    @State private var togglePending = false

    private var isDirectApi: Bool { mode == "direct-api" }
    private var isThreadRelay: Bool { mode == "threadrelay" }
    private var isKnownMode: Bool { isDirectApi || isThreadRelay }
    private var gatewayReady: Bool {
        isThreadRelay && gatewayEnabled != false
    }
    private var toggleValue: Bool {
        pendingEnabled ?? gatewayReady
    }
    private var toggleDisabled: Bool {
        isUpdating || togglePending || !isKnownMode
    }
    private var detail: String {
        if isDirectApi {
            return "已关闭，Codex 使用原来的设置。"
        }
        if isThreadRelay, gatewayEnabled == false {
            return "MochiPort 已关闭，打开开关即可使用。"
        }
        if isThreadRelay {
            return "Codex 已连接，可以使用。"
        }
        return "打开开关连接 Codex。"
    }

    var body: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 3) {
                Text("使用 MochiPort")
                    .font(.body.weight(.semibold))
                Text(detail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
            }

            Spacer(minLength: 12)

            if isKnownMode {
                Toggle("使用 MochiPort", isOn: Binding(
                    get: { toggleValue },
                    set: { newValue in
                        guard !toggleDisabled, newValue != toggleValue else { return }
                        pendingEnabled = newValue
                        togglePending = true
                        Task { @MainActor in
                            _ = await setGatewayEnabled(newValue)
                            pendingEnabled = nil
                            togglePending = false
                        }
                    }
                ))
                .labelsHidden()
                .toggleStyle(.switch)
                .controlSize(.small)
                .accessibilityLabel("使用 MochiPort")
                .help(toggleValue ? "关闭后恢复原来的设置" : "打开后连接 MochiPort")
                .disabled(toggleDisabled)
            } else {
                Button {
                    Task { _ = await setGatewayEnabled(true) }
                } label: {
                    Label("连接", systemImage: "link.badge.plus")
                }
                .buttonStyle(.bordered)
                .disabled(isUpdating)
            }
        }
        .accessibilityElement(children: .contain)
        .onChange(of: mode) { _ in
            pendingEnabled = nil
            togglePending = false
        }
    }
}

private struct CodexProviderRow: View {
    let provider: ManageCodexStatus.Provider

    var body: some View {
        HStack(alignment: .center, spacing: 12) {
            Image(systemName: provider.secretSet ? "key.fill" : "key.slash")
                .foregroundStyle(provider.secretSet ? Color.secondary : Color.orange)
                .frame(width: 18)

            VStack(alignment: .leading, spacing: 3) {
                Text(provider.name)
                    .font(.body.weight(.medium))
                Text(provider.baseUrl ?? "未设置地址")
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .textSelection(.enabled)
            }

            Spacer(minLength: 12)

            VStack(alignment: .trailing, spacing: 3) {
                Text(provider.secretSet ? "已设置 Key" : "还没设置 Key")
                    .font(.caption)
                    .foregroundStyle(provider.secretSet ? Color.secondary : Color.orange)
                if provider.supportsWebsockets {
                    Text("支持实时回复")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
            }
        }
        .accessibilityElement(children: .combine)
    }
}

private struct CodexDiagnosticList: View {
    let status: ManageCodexStatus

    private var isDirectApiMode: Bool { status.providerMode == "direct-api" }
    private var remoteControlReady: Bool {
        !status.remoteControlSupported || status.remoteControlConfigured
    }

    var body: some View {
        VStack(spacing: 0) {
            CodexDiagnosticRow(
                title: "配置文件",
                detail: status.configOk
                    ? "已找到 Codex 配置"
                    : (status.configError ?? "没有找到有效配置"),
                ready: status.configOk
            )
            Divider()
            CodexDiagnosticRow(
                title: "登录状态",
                detail: isDirectApiMode
                    ? "直连时使用 Provider 的 API 认证"
                    : (status.authOk ? "已登录" : (status.authError ?? "需要登录 Codex")),
                ready: isDirectApiMode || status.authOk
            )
            Divider()
            CodexDiagnosticRow(
                title: "桌面控制",
                detail: isDirectApiMode
                    ? "直连时不参与请求"
                    : (status.guiConfigured ? "可以管理 Codex App" : (status.guiError ?? "需要修复")),
                ready: isDirectApiMode || status.guiConfigured
            )
            Divider()
            CodexDiagnosticRow(
                title: "远程控制",
                detail: isDirectApiMode
                    ? "直连时不需要"
                    : (!status.remoteControlSupported
                        ? "当前 Codex 版本不支持"
                        : (status.remoteControlConfigured
                            ? "已开启"
                            : (status.remoteControlError ?? "尚未开启"))),
                ready: isDirectApiMode || remoteControlReady
            )
        }
    }
}

private struct CodexDiagnosticRow: View {
    let title: String
    let detail: String
    let ready: Bool

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: ready ? "checkmark.circle.fill" : "exclamationmark.circle.fill")
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(ready ? Color.green : Color.orange)
                .frame(width: 16)
            Text(title)
                .font(.callout.weight(.medium))
            Spacer(minLength: 12)
            Text(detail)
                .font(.caption)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.trailing)
                .lineLimit(2)
        }
        .padding(.vertical, 7)
        .accessibilityElement(children: .combine)
    }
}

private struct CodexLoadingSurface: View {
    var body: some View {
        Group {
            if #available(macOS 26.0, *) {
                content
                    .glassEffect(.regular, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            } else {
                content
                    .background(
                        .regularMaterial,
                        in: RoundedRectangle(cornerRadius: 8, style: .continuous)
                    )
            }
        }
    }

    private var content: some View {
        ProgressView()
            .controlSize(.large)
            .padding(18)
    }
}

/// Modal shown while the AppModel performs a fresh preflight and waits for
/// Codex App to exit. The polling task survives view reconstruction.
private struct EnhancedLaunchWaitSheet: View {
    let onCancel: () -> Void

    var body: some View {
        VStack(spacing: 16) {
            ProgressView()
                .controlSize(.large)
            VStack(spacing: 6) {
                Text("等待 Codex App 退出")
                    .font(.headline)
                Text("增强启动需要在 Codex App 关闭后进行。请手动退出 Codex App，检测到退出后会自动继续。")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Button("取消", role: .cancel, action: onCancel)
                .keyboardShortcut(.cancelAction)
        }
        .padding(28)
        .frame(width: 380)
        .accessibilityIdentifier("codex.enhanced-wait-sheet")
    }
}

private struct EnhancedLaunchProgressRow: View {
    let operation: ManageEnhancedLaunchOperation
    let legacyFallback: Bool
    let error: String?
    let canCancel: Bool
    let cancel: () -> Void

    var body: some View {
        HStack(alignment: .top, spacing: 11) {
            statusSymbol
                .frame(width: 18, height: 18)

            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 7) {
                    Text(phaseTitle)
                        .font(.callout.weight(.semibold))
                    if legacyFallback {
                        Text("兼容模式")
                            .font(.caption2.weight(.medium))
                            .foregroundStyle(.secondary)
                    }
                }
                Text(error ?? operation.message)
                    .font(.caption)
                    .foregroundStyle(error == nil ? AnyShapeStyle(.secondary) : AnyShapeStyle(.red))
                    .fixedSize(horizontal: false, vertical: true)
                if let recovery = operation.recovery, !recovery.isEmpty {
                    Text(recovery)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }

            Spacer(minLength: 12)
            if canCancel {
                Button("取消", role: .cancel, action: cancel)
                    .controlSize(.small)
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("codex.enhanced-operation")
    }

    @ViewBuilder
    private var statusSymbol: some View {
        switch operation.phase {
        case "ready":
            Image(systemName: "checkmark.circle.fill")
                .foregroundStyle(.green)
        case "failed":
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(.red)
        case "cancelled":
            Image(systemName: "xmark.circle")
                .foregroundStyle(.secondary)
        default:
            ProgressView()
                .controlSize(.small)
        }
    }

    private var phaseTitle: String {
        switch operation.phase {
        case "preparing": "正在准备"
        case "launching": "正在启动 Codex App"
        case "waitingForApp", "waiting_for_app": "等待 Codex App 就绪"
        case "injecting": "正在应用增强配置"
        case "ready": "增强启动已完成"
        case "failed": "增强启动失败"
        case "cancelled": "增强启动已取消"
        default: "正在增强启动"
        }
    }
}

private struct EnhancedLaunchFeatureRow: View {
    let symbol: String
    let text: String

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 7) {
            Image(systemName: symbol)
                .font(.caption)
                .foregroundStyle(Color.accentColor)
                .frame(width: 14)
            Text(text)
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

private struct SessionProjectGroup: Identifiable {
    let id: String
    let title: String
    let path: String?
    let sessions: [ManageCodexSession]

    var latestUpdatedAt: Int64 {
        sessions.map(\.updatedAt).max() ?? 0
    }
}

private enum SessionRouteFilter: String, CaseIterable, Identifiable {
    case all
    case gateway
    case direct

    var id: Self { self }

    var title: String {
        switch self {
        case .all: "全部"
        case .gateway: "AI Gateway"
        case .direct: "直连"
        }
    }

    var symbol: String {
        switch self {
        case .all: "rectangle.stack.fill"
        case .gateway: "server.rack"
        case .direct: "arrow.up.right"
        }
    }

    func matches(_ session: ManageCodexSession) -> Bool {
        switch self {
        case .all: true
        case .gateway: session.modelProvider == "ai-gateway"
        case .direct: session.modelProvider != "ai-gateway"
        }
    }
}

private struct SessionRouteFilterControl: View {
    @Binding var selection: SessionRouteFilter
    @Namespace private var selectionNamespace

    var body: some View {
        Group {
            if #available(macOS 26.0, *) {
                segments
                    .glassEffect(.regular.interactive(), in: .capsule)
            } else {
                segments
                    .background(.quaternary.opacity(0.5), in: Capsule())
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("会话范围")
    }

    private var segments: some View {
        HStack(spacing: 2) {
            ForEach(SessionRouteFilter.allCases) { filter in
                segment(filter)
            }
        }
        .padding(3)
    }

    private func segment(_ filter: SessionRouteFilter) -> some View {
        let isSelected = selection == filter

        return Button {
            guard selection != filter else { return }
            withAnimation(.easeInOut(duration: 0.2)) {
                selection = filter
            }
        } label: {
            HStack(spacing: 5) {
                Image(systemName: filter.symbol)
                    .font(.system(size: 10, weight: .semibold))
                Text(filter.title)
                    .font(.system(size: 11, weight: .semibold))
                    .lineLimit(1)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 5)
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .foregroundStyle(isSelected ? AnyShapeStyle(.white) : AnyShapeStyle(.secondary))
        .background {
            if isSelected {
                Capsule()
                    .fill(Color.accentColor)
                    .matchedGeometryEffect(id: "selectedSessionRoute", in: selectionNamespace)
            }
        }
        .accessibilityLabel(filter.title)
        .accessibilityAddTraits(isSelected ? [.isSelected] : [])
        .help("只显示\(filter.title)会话")
    }
}

private enum SessionSourceState {
    case waiting
    case connected
    case offline
    case unavailable

    var title: String {
        switch self {
        case .waiting: "等待连接"
        case .connected: "已连接"
        case .offline: "未连接"
        case .unavailable: "不可用"
        }
    }

    var detail: String {
        switch self {
        case .waiting: "正在等待本地服务和 Codex App 就绪。"
        case .connected: "可读取当前 Codex App 可见的本机会话。"
        case .offline: "打开 Codex App，确认远程控制已启用，然后刷新。"
        case .unavailable: "尚未配置 Codex App 的远程控制。"
        }
    }

    var symbol: String {
        switch self {
        case .waiting: "hourglass"
        case .connected: "checkmark.circle.fill"
        case .offline: "link"
        case .unavailable: "exclamationmark.circle"
        }
    }

    var tint: Color {
        switch self {
        case .waiting: .secondary
        case .connected: .green
        case .offline: .orange
        case .unavailable: .red
        }
    }

    var emptyTitle: String {
        switch self {
        case .waiting: "正在等待 Codex App"
        case .connected: "当前没有可见会话"
        case .offline: "Codex App 尚未连接"
        case .unavailable: "Codex App 尚未配置"
        }
    }

    var emptyMessage: String {
        switch self {
        case .waiting: "连接建立后，会话会自动显示在这里。"
        case .connected: "在 Codex App 中创建或打开会话后，刷新即可看到。"
        case .offline: "请打开 Codex App，确认远程控制已启用，然后刷新。"
        case .unavailable: "请先完成 Codex App 接入，再读取本机会话。"
        }
    }
}

struct SessionsView: View {
    @EnvironmentObject private var model: AppModel
    @State private var query = ""
    @State private var routeFilter: SessionRouteFilter = .all
    @State private var selectedIDs = Set<String>()
    @State private var moveInFlight = false

    private let unknownProjectKey = "__threadrelay_unknown_project__"

    private var filteredSessions: [ManageCodexSession] {
        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return model.codexSessions.filter { session in
            guard routeFilter.matches(session) else { return false }
            guard !needle.isEmpty else { return true }
            return session.displayName.lowercased().contains(needle)
                || session.modelProvider.lowercased().contains(needle)
                || session.id.lowercased().contains(needle)
                || (session.cwd?.lowercased().contains(needle) ?? false)
        }
    }

    private var sessionGroups: [SessionProjectGroup] {
        Dictionary(grouping: filteredSessions, by: projectKey(for:))
            .map { key, sessions in
                let sortedSessions = sessions.sorted {
                    if $0.updatedAt != $1.updatedAt {
                        return $0.updatedAt > $1.updatedAt
                    }
                    return $0.id < $1.id
                }
                let path = key == unknownProjectKey ? nil : key
                return SessionProjectGroup(
                    id: key,
                    title: projectTitle(for: path),
                    path: path,
                    sessions: sortedSessions
                )
            }
            .sorted {
                if $0.latestUpdatedAt != $1.latestUpdatedAt {
                    return $0.latestUpdatedAt > $1.latestUpdatedAt
                }
                return $0.title.localizedStandardCompare($1.title) == .orderedAscending
            }
    }

    private var selectedSessions: [ManageCodexSession] {
        model.codexSessions.filter { selectedIDs.contains($0.id) }
    }

    private var sessionSourceState: SessionSourceState {
        guard let dashboard = model.dashboard else {
            return model.isLoading(.sessions)
                || model.dashboardState.isRefreshing
                || model.dashboardState == .starting
                ? .waiting
                : .unavailable
        }

        let codexApp = dashboard.executionClients.codexApp
        if codexApp.connected { return .connected }
        if codexApp.configured { return .offline }
        return .unavailable
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            sessionToolbar
                .padding(.horizontal, ThreadRelayPageLayout.horizontalPadding)
                .padding(.top, ThreadRelayPageLayout.topPadding)
                .padding(.bottom, 12)

            List(selection: $selectedIDs) {
                if let error = model.sectionErrors[.sessions] {
                    InlineManagementError(
                        message: error,
                        retry: { Task { await model.loadSection(.sessions, force: true) } },
                        dismiss: { model.dismissSectionError(.sessions) }
                    )
                    .padding(.bottom, 12)
                    .listRowInsets(EdgeInsets())
                    .listRowSeparator(.hidden)
                    .listRowBackground(Color.clear)
                }

                if filteredSessions.isEmpty,
                   !model.isLoading(.sessions),
                   model.sectionErrors[.sessions] == nil {
                    ManagementEmptyState(
                        title: query.isEmpty ? sessionSourceState.emptyTitle : "没有匹配的会话",
                        message: query.isEmpty
                            ? sessionSourceState.emptyMessage
                            : "调整搜索词后重试。",
                        symbol: query.isEmpty ? sessionSourceState.symbol : "magnifyingglass"
                    )
                    .frame(maxWidth: .infinity, minHeight: 240)
                    .listRowSeparator(.hidden)
                } else {
                    sessionTableHeader
                        .textCase(nil)
                        .listRowInsets(EdgeInsets())
                        .listRowSeparator(.hidden)
                        .listRowBackground(Color(nsColor: .windowBackgroundColor))

                    ForEach(Array(tableSessions.enumerated()), id: \.element.id) { index, session in
                        sessionListRow(session)
                            .tag(session.id)
                            .listRowInsets(EdgeInsets())
                            .listRowSeparator(.hidden)
                            .contextMenu {
                                moveMenuEntries(for: session)
                                Divider()
                                Button("复制会话 ID") {
                                    NSPasteboard.general.clearContents()
                                    NSPasteboard.general.setString(session.id, forType: .string)
                                }
                            }
                    }
                }
            }
            .listStyle(.inset(alternatesRowBackgrounds: true))
            .scrollContentBackground(.hidden)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .overlay {
            if model.isLoading(.sessions), model.codexSessions.isEmpty {
                ProgressView("正在读取会话…")
            }
        }
        .task { await model.loadSection(.sessions) }
        .searchable(text: $query, placement: .toolbar, prompt: "搜索会话")
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                sessionActions
            }
        }
    }

    private var sessionToolbar: some View {
        VStack(alignment: .leading, spacing: 12) {
            sessionSourceBanner

            HStack(spacing: 12) {
                SessionRouteFilterControl(selection: $routeFilter)
                    .frame(width: 320)
                Spacer(minLength: 0)

                if !selectedIDs.isEmpty {
                    Text("已选 \(selectedIDs.count) 项")
                        .font(.caption.weight(.medium))
                        .foregroundStyle(.secondary)
                        .monospacedDigit()
                        .fixedSize()
                        .accessibilityIdentifier("sessions.selection-count")
                }
            }
        }
    }

    private var sessionSourceBanner: some View {
        let state = sessionSourceState

        return HStack(alignment: .top, spacing: 10) {
            Image(systemName: state.symbol)
                .font(.system(size: 15, weight: .medium))
                .foregroundStyle(state.tint)
                .frame(width: 20, height: 20)

            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 8) {
                    Text("读取来源：当前 Codex App")
                        .font(.callout.weight(.semibold))
                    Text(state.title)
                        .font(.caption.weight(.medium))
                        .foregroundStyle(state.tint)
                }
                Text(state.detail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            Spacer(minLength: 12)

            if model.isLoading(.sessions) {
                ProgressView()
                    .controlSize(.small)
                    .accessibilityLabel("正在刷新会话")
            } else {
                Text("共 \(model.codexSessions.count) 个")
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
                    .fixedSize()
            }

            Button {
                Task { await model.loadSection(.sessions, force: true) }
            } label: {
                Image(systemName: "arrow.clockwise")
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
            .disabled(model.isLoading(.sessions))
            .help("刷新本机会话")
            .accessibilityLabel("刷新本机会话")
        }
        .padding(.bottom, 10)
        .overlay(alignment: .bottom) {
            Divider()
        }
        .accessibilityIdentifier("sessions.source-status")
    }

    private var sessionTableHeader: some View {
        HStack(spacing: 12) {
            Text("名称")
                .frame(minWidth: 180, maxWidth: .infinity, alignment: .leading)
            Text("项目")
                .frame(minWidth: 170, maxWidth: .infinity, alignment: .leading)
            Text("Provider")
                .frame(width: 120, alignment: .leading)
            Text("最近活动")
                .frame(width: 90, alignment: .trailing)
        }
        .font(.caption.weight(.semibold))
        .foregroundStyle(.secondary)
        .padding(.horizontal, 12)
        .padding(.vertical, 5)
    }

    private func sessionListRow(_ session: ManageCodexSession) -> some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 3) {
                Text(session.displayName)
                    .font(.body.weight(.medium))
                    .lineLimit(1)
                Text(session.id)
                    .font(.caption2.monospaced())
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            .frame(minWidth: 180, maxWidth: .infinity, alignment: .leading)

            VStack(alignment: .leading, spacing: 3) {
                Text(projectTitle(for: projectPath(for: session)))
                    .font(.callout.weight(.medium))
                    .lineLimit(1)
                Text(projectPath(for: session) ?? "没有工作目录")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            .frame(minWidth: 170, maxWidth: .infinity, alignment: .leading)

            Text(session.modelProvider == "ai-gateway" ? "AI Gateway" : session.modelProvider)
                .font(.callout)
                .foregroundStyle(session.modelProvider == "ai-gateway" ? Color.accentColor : .secondary)
                .lineLimit(1)
                .frame(width: 120, alignment: .leading)

            Text(relativeDate(seconds: session.updatedAt))
                .font(.callout.monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(width: 90, alignment: .trailing)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .contentShape(Rectangle())
        .help("会话 ID：\(session.id)")
        .accessibilityIdentifier("sessions.row.\(session.id)")
    }

    private var tableSessions: [ManageCodexSession] {
        sessionGroups.flatMap(\.sessions)
    }

    private func projectKey(for session: ManageCodexSession) -> String {
        projectPath(for: session) ?? unknownProjectKey
    }

    private func projectPath(for session: ManageCodexSession) -> String? {
        guard let cwd = session.cwd?.trimmingCharacters(in: .whitespacesAndNewlines), !cwd.isEmpty else {
            return nil
        }

        if let url = URL(string: cwd), url.isFileURL {
            return url.standardizedFileURL.path
        }

        let expanded = NSString(string: cwd).expandingTildeInPath
        return URL(fileURLWithPath: expanded).standardizedFileURL.path
    }

    private func projectTitle(for path: String?) -> String {
        guard let path, !path.isEmpty else { return "未指定项目" }
        let name = URL(fileURLWithPath: path).lastPathComponent
        return name.isEmpty ? path : name
    }

    /// Context-menu entries act on the whole selection when the clicked row
    /// is part of it, otherwise only on the clicked row.
    @ViewBuilder
    private func moveMenuEntries(for session: ManageCodexSession) -> some View {
        let targets = selectedIDs.contains(session.id) ? selectedSessions : [session]
        Button(menuTitle("AI Gateway", count: targets.count)) {
            moveSessions(targets, to: nil)
        }
        .disabled(moveInFlight || targets.allSatisfy { $0.modelProvider == "ai-gateway" })
        let providers = model.codexSessionProviders.filter { $0 != "ai-gateway" }
        ForEach(providers, id: \.self) { provider in
            Button(menuTitle(provider, count: targets.count)) {
                moveSessions(targets, to: provider)
            }
            .disabled(moveInFlight || targets.allSatisfy { $0.modelProvider == provider })
        }
    }

    private func menuTitle(_ target: String, count: Int) -> String {
        count > 1 ? "切换 \(count) 项到 \(target)" : "切换到 \(target)"
    }

    private var sessionActions: some View {
        Menu {
            Section("切换 Provider") {
                Button("AI Gateway") {
                    moveSessions(selectedSessions, to: nil)
                }
                let providers = model.codexSessionProviders.filter { $0 != "ai-gateway" }
                if !providers.isEmpty {
                    ForEach(providers, id: \.self) { provider in
                        Button(provider) {
                            moveSessions(selectedSessions, to: provider)
                        }
                    }
                }
            }
        } label: {
            Image(systemName: "ellipsis")
        }
        .disabled(selectedIDs.isEmpty || moveInFlight)
        .menuStyle(.borderlessButton)
        .accessibilityLabel("更多会话操作")
        .help("更多会话操作")
    }

    /// Skips sessions already on the target, then deselects the updated ones so
    /// only failed sessions stay selected for a retry.
    private func moveSessions(_ sessions: [ManageCodexSession], to provider: String?) {
        let target = provider ?? "ai-gateway"
        let ids = sessions.filter { $0.modelProvider != target }.map(\.id)
        guard !ids.isEmpty, !moveInFlight else { return }
        moveInFlight = true
        Task {
            let result = await model.moveCodexSessions(ids: ids, to: provider)
            let skipped = sessions.filter { $0.modelProvider == target }.map(\.id)
            selectedIDs.subtract(result.movedIds)
            selectedIDs.subtract(skipped)
            moveInFlight = false
        }
    }
}

private struct SessionProjectHeader: View {
    let group: SessionProjectGroup

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: group.path == nil ? "folder.badge.questionmark" : "folder")
                .font(.system(size: 14, weight: .medium))
                .foregroundStyle(.secondary)

            VStack(alignment: .leading, spacing: 3) {
                Text(group.title)
                    .font(.callout.weight(.semibold))
                    .foregroundStyle(.primary)
                    .lineLimit(1)
                if let path = group.path {
                    Text(path)
                        .font(.caption.monospaced())
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .textSelection(.enabled)
                } else {
                    Text("没有可用的工作目录")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            Spacer(minLength: 12)
            Text("\(group.sessions.count) 个会话")
                .font(.caption.weight(.medium).monospacedDigit())
                .foregroundStyle(.secondary)
        }
        .textCase(nil)
    }
}

private struct SessionRow: View {
    let session: ManageCodexSession

    var body: some View {
        if #available(macOS 14.0, *) {
            SessionRowSelectionReader(session: session)
        } else {
            SessionRowContent(session: session, emphasized: false)
        }
    }
}

/// `backgroundProminence` only exists on macOS 14+; it keeps secondary row
/// metadata legible when the native list selection surface is active.
@available(macOS 14.0, *)
private struct SessionRowSelectionReader: View {
    @Environment(\.backgroundProminence) private var backgroundProminence
    let session: ManageCodexSession

    var body: some View {
        SessionRowContent(session: session, emphasized: backgroundProminence == .increased)
    }
}

private struct SessionRowContent: View {
    let session: ManageCodexSession
    let emphasized: Bool

    private var isGateway: Bool { session.modelProvider == "ai-gateway" }

    private var iconColor: Color {
        if emphasized { return .white.opacity(0.88) }
        return isGateway ? .accentColor : .secondary
    }

    private var providerTextColor: Color {
        emphasized
            ? Color.white.opacity(0.82)
            : .secondary
    }

    var body: some View {
        HStack(spacing: ThreadRelaySpacing.standard) {
            Image(systemName: isGateway ? "point.3.connected.trianglepath.dotted" : "bubble.left.and.text.bubble.right")
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(iconColor)
                .frame(width: 22)
            VStack(alignment: .leading, spacing: 3) {
                Text(session.displayName)
                    .font(.body.weight(.medium))
                    .lineLimit(1)
                Text(isGateway ? "AI Gateway" : session.modelProvider)
                    .font(.caption)
                    .foregroundStyle(providerTextColor)
                    .lineLimit(1)
            }
            Spacer(minLength: ThreadRelaySpacing.standard)
            Text(relativeDate(seconds: session.updatedAt))
                .font(.caption)
                .foregroundStyle(emphasized ? Color.white.opacity(0.78) : .secondary)
                .monospacedDigit()
        }
        .padding(.vertical, 10)
        .help("会话 ID：\(session.id)")
    }
}

private enum GatewaySection: String, CaseIterable, Identifiable {
    case general
    case providers
    case accountPool

    var id: Self { self }

    var title: String {
        switch self {
        case .general: "概览"
        case .providers: "模型服务"
        case .accountPool: "账号"
        }
    }

    var symbol: String {
        switch self {
        case .general: "switch.2"
        case .providers: "server.rack"
        case .accountPool: "person.2"
        }
    }
}

private struct GatewaySectionControl: View {
    @Binding var selection: GatewaySection
    @Namespace private var selectionNamespace

    var body: some View {
        Group {
            if #available(macOS 26.0, *) {
                segments
                    .glassEffect(.regular.interactive(), in: .capsule)
            } else {
                segments
                    .background(.quaternary.opacity(0.5), in: Capsule())
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("AI 网关设置区域")
    }

    private var segments: some View {
        HStack(spacing: 2) {
            ForEach(GatewaySection.allCases) { section in
                segment(section)
            }
        }
        .padding(3)
    }

    private func segment(_ section: GatewaySection) -> some View {
        let isSelected = selection == section

        return Button {
            guard selection != section else { return }
            withAnimation(.easeInOut(duration: 0.2)) {
                selection = section
            }
        } label: {
            HStack(spacing: 5) {
                Image(systemName: section.symbol)
                    .font(.system(size: 10, weight: .semibold))
                Text(section.title)
                    .font(.system(size: 11, weight: .semibold))
                    .lineLimit(1)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 5)
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .foregroundStyle(isSelected ? AnyShapeStyle(.white) : AnyShapeStyle(.secondary))
        .background {
            if isSelected {
                Capsule()
                    .fill(Color.accentColor)
                    .matchedGeometryEffect(id: "selectedGatewaySection", in: selectionNamespace)
            }
        }
        .accessibilityLabel(section.title)
        .accessibilityAddTraits(isSelected ? [.isSelected] : [])
        .help("显示\(section.title)")
    }
}

struct GatewayView: View {
    @EnvironmentObject private var model: AppModel
    private enum GatewayProviderFilter: String, CaseIterable, Identifiable {
        case all
        case enabled
        case disabled

        var id: Self { self }

        var title: String {
            switch self {
            case .all: "全部"
            case .enabled: "已启用"
            case .disabled: "已停用"
            }
        }
    }

    @State private var enabled = false
    @State private var filterImages = false
    @State private var requestLogging = false
    @State private var requestDetails = false
    @State private var visibleModels = ""
    @State private var settingsReady = false
    @State private var modelCatalog: [ManageCodexCatalogModel] = []
    @State private var selectedCatalogModels = Set<String>()
    @State private var customVisibleModelInput = ""
    @State private var manualVisibleModelsExpanded = false
    @State private var editor: GatewayProviderEditorState?
    @State private var providerToDelete: ManageGatewayProvider?
    @State private var providerQuery = ""
    @State private var providerFilter: GatewayProviderFilter = .all
    @State private var sub2ApiBaseURL = ""
    @State private var sub2ApiAdminKey = ""
    @State private var sub2ApiFormInitialized = false
    @State private var sub2ApiSaving = false
    @State private var confirmSub2ApiDisconnect = false
    @State private var section: GatewaySection = .general
    @State private var preferencesSaving = false
    @State private var sub2ApiEditing = false

    init(startAtProviders: Bool = false, startAddingProvider: Bool = false) {
        _section = State(initialValue: startAtProviders ? .providers : .general)
        _editor = State(
            initialValue: startAddingProvider
                ? GatewayProviderEditorState(provider: nil)
                : nil
        )
    }

    var body: some View {
        Group {
            switch section {
            case .providers:
                gatewayRoot
                    .searchable(text: $providerQuery, placement: .toolbar, prompt: "搜索模型服务")
            case .general, .accountPool:
                gatewayRoot
            }
        }
        .toolbar {
            if section == .providers {
                ToolbarItem(placement: .primaryAction) {
                    Button {
                        editor = GatewayProviderEditorState(provider: nil)
                    } label: {
                        Label("添加服务", systemImage: "plus")
                    }
                    .help("添加模型服务")
                }
                if modelsDirty {
                    ToolbarItem(placement: .primaryAction) {
                        Button {
                            saveVisibleModels()
                        } label: {
                            Label("保存模型", systemImage: "checkmark")
                        }
                        .disabled(!settingsReady || model.isLoading(.gateway))
                        .help("保存 Codex 可见模型")
                    }
                }
            }
        }
        .task {
            await model.loadSection(.gateway)
            // The catalog endpoint may not exist on an older daemon; an empty
            // catalog keeps the plain text editor as the only input.
            modelCatalog = await model.loadCodexModelCatalog() ?? []
            synchronizeGateway(model.gateway)
            synchronizeSub2ApiAdmin(model.sub2ApiAdmin, gateway: model.gateway)
        }
        .onChange(of: model.gateway) { gateway in
            synchronizeGateway(gateway)
            synchronizeSub2ApiAdmin(model.sub2ApiAdmin, gateway: gateway)
        }
        .onChange(of: model.sub2ApiAdmin) { admin in
            synchronizeSub2ApiAdmin(admin, gateway: model.gateway)
        }
        .sheet(item: $editor) { state in
            GatewayProviderEditor(state: state) { originalName, provider, apiKey, clearAPIKey in
                let saved = await model.saveGatewayProvider(
                    originalName: originalName,
                    provider: provider,
                    apiKey: apiKey,
                    clearAPIKey: clearAPIKey
                )
                if saved { editor = nil }
                return saved
            }
        }
        .alert(
            "删除 Provider？",
            isPresented: Binding(
                get: { providerToDelete != nil },
                set: { if !$0 { providerToDelete = nil } }
            )
        ) {
            Button("取消", role: .cancel) { providerToDelete = nil }
            Button("删除", role: .destructive) {
                guard let provider = providerToDelete else { return }
                Task {
                    _ = await model.deleteGatewayProvider(provider)
                    providerToDelete = nil
                }
            }
        } message: {
            Text("删除后，该 Provider 将立即停止参与模型路由。")
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

    private var gatewayRoot: some View {
        VStack(alignment: .leading, spacing: 0) {
            GatewaySectionControl(selection: $section)
                .frame(width: 440)
                .padding(.horizontal, ThreadRelayPageLayout.horizontalPadding)
                .padding(.top, ThreadRelayPageLayout.topPadding)
                .padding(.bottom, 12)

            if let error = model.sectionErrors[.gateway] {
                InlineManagementError(
                    message: error,
                    retry: { Task { await model.loadSection(.gateway, force: true) } },
                    dismiss: { model.dismissSectionError(.gateway) }
                )
                .padding(.horizontal, ThreadRelayPageLayout.horizontalPadding)
                .padding(.bottom, 12)
            }

            if let gateway = model.gateway {
                switch section {
                case .general:
                    generalPage(gateway)
                case .providers:
                    modelsAndProvidersPage(gateway.providers)
                case .accountPool:
                    accountPoolPage
                }
            } else {
                Spacer(minLength: 0)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .overlay {
            if model.isLoading(.gateway), model.gateway == nil {
                ProgressView("正在读取 AI 网关…")
            }
        }
        .background(Color(nsColor: .windowBackgroundColor))
    }

    private func generalPage(_ gateway: ManageGateway) -> some View {
        let enabledProviders = gateway.providers.filter(\.enabled).count
        let modelCount = Set(gateway.providers.flatMap(\.models)).count

        return Form {
            Section("状态") {
                HStack(spacing: 12) {
                    Label(
                        gateway.enabled ? "已开启" : "已关闭",
                        systemImage: gateway.enabled ? "checkmark.circle.fill" : "pause.circle"
                    )
                    .foregroundStyle(gateway.enabled ? Color.green : .secondary)
                    Spacer(minLength: 12)
                    Text("\(enabledProviders) 个模型服务 · \(modelCount) 个模型")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .monospacedDigit()
                        .lineLimit(1)
                }
            }

            Section("选项") {
                GatewayPreferenceRow(
                    title: "关闭图片功能",
                    detail: "部分模型不支持图片功能时可以打开。"
                ) {
                    Toggle("关闭图片功能", isOn: filterImagesBinding)
                        .labelsHidden()
                        .toggleStyle(.switch)
                        .disabled(preferencesSaving)
                }
            }

            Section("使用记录") {
                GatewayPreferenceRow(
                    title: "保存使用记录",
                    detail: "保存简单记录，方便查看使用情况。"
                ) {
                    Toggle("保存使用记录", isOn: requestLoggingBinding)
                        .labelsHidden()
                        .toggleStyle(.switch)
                        .disabled(preferencesSaving)
                }
                GatewayPreferenceRow(
                    title: "保存详细记录",
                    detail: requestLogging ? "保存更多内容，方便排查问题。" : "请先打开使用记录。"
                ) {
                    Toggle("保存详细记录", isOn: requestDetailsBinding)
                        .labelsHidden()
                        .toggleStyle(.switch)
                        .disabled(!requestLogging || preferencesSaving)
                }
            }
        }
        .formStyle(.grouped)
        .scrollContentBackground(.hidden)
        .managementPageInsets(topPadding: 0)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func modelsAndProvidersPage(_ providers: [ManageGatewayProvider]) -> some View {
        List {
            Section {
                HStack(alignment: .center, spacing: 12) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("模型服务")
                            .font(.headline)
                        Text("\(providers.count) 个服务")
                            .font(.caption.monospacedDigit())
                            .foregroundStyle(.secondary)
                    }
                    Spacer(minLength: 12)
                    Picker("服务筛选", selection: $providerFilter) {
                        ForEach(GatewayProviderFilter.allCases) { filter in
                            Text(filter.title).tag(filter)
                        }
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                    .frame(width: 300)
                }
                .padding(.vertical, 4)
                .listRowSeparator(.hidden)

                if providers.isEmpty {
                    ManagementEmptyState(
                        title: "还没有模型服务",
                        message: "点击右上角“添加服务”开始设置。",
                        symbol: "server.rack"
                    )
                    .frame(maxWidth: .infinity, minHeight: 150)
                    .listRowSeparator(.hidden)
                } else if filteredProviders(providers).isEmpty {
                    ManagementEmptyState(
                        title: "没有匹配的模型服务",
                        message: "换一个名称或筛选条件试试。",
                        symbol: "magnifyingglass"
                    )
                    .frame(maxWidth: .infinity, minHeight: 120)
                    .listRowSeparator(.hidden)
                } else {
                    providerTableHeader
                        .listRowInsets(EdgeInsets())
                        .listRowSeparator(.hidden)
                        .listRowBackground(Color(nsColor: .windowBackgroundColor))
                    ForEach(filteredProviders(providers)) { provider in
                        GatewayProviderListRow(
                            provider: provider,
                            onEdit: { editor = GatewayProviderEditorState(provider: provider) },
                            onDelete: { providerToDelete = provider }
                        )
                        .listRowInsets(EdgeInsets())
                        .listRowSeparator(.hidden)
                    }
                }
            }

            Section {
                if modelCatalog.isEmpty {
                    Text("暂时没有目录模型，可在下方添加自定义模型。")
                        .foregroundStyle(.secondary)
                } else if filteredVisibleCatalogModels.isEmpty {
                    Text("没有匹配的目录模型。")
                        .foregroundStyle(.secondary)
                } else {
                    ForEach(filteredVisibleCatalogModels) { entry in
                        let selected = selectedCatalogModels.contains(entry.id)
                        Button {
                            catalogBinding(entry.id).wrappedValue.toggle()
                        } label: {
                            HStack(spacing: 10) {
                                Image(systemName: selected ? "checkmark.circle.fill" : "circle")
                                    .foregroundStyle(selected ? Color.accentColor : .secondary)
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(entry.displayName)
                                        .font(.body.weight(.medium))
                                    Text(entry.id)
                                        .font(.caption2.monospaced())
                                        .foregroundStyle(.secondary)
                                }
                                Spacer(minLength: 0)
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("可见模型 \(entry.displayName)")
                        .accessibilityValue(selected ? "已选择" : "未选择")
                    }
                }
            } header: {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text("Codex 可用模型")
                    Spacer(minLength: 12)
                    Text("已选 \(mergedVisibleModels.count) 个")
                        .font(.caption.monospacedDigit())
                        .foregroundStyle(.secondary)
                }
            }

            Section("自定义模型") {
                VStack(alignment: .leading, spacing: 0) {
                    if customVisibleModels.isEmpty {
                        Text("暂无自定义模型，在下方输入名称添加。")
                            .font(.callout)
                            .foregroundStyle(.secondary)
                            .padding(.vertical, 6)
                    } else {
                        ForEach(Array(customVisibleModels.enumerated()), id: \.offset) { index, modelID in
                            HStack(spacing: 9) {
                                Image(systemName: "cube")
                                    .font(.callout)
                                    .foregroundStyle(.secondary)
                                    .frame(width: 18)
                                Text(modelID)
                                    .font(.body.monospaced())
                                    .lineLimit(1)
                                    .truncationMode(.middle)
                                Spacer(minLength: 8)
                                Button {
                                    removeVisibleModel(modelID)
                                } label: {
                                    Image(systemName: "xmark.circle")
                                }
                                .buttonStyle(.plain)
                                .foregroundStyle(.secondary)
                                .help("移除模型")
                                .accessibilityLabel("移除模型 \(modelID)")
                            }
                            .padding(.vertical, 5)

                            if index < customVisibleModels.count - 1 {
                                Divider()
                                    .padding(.leading, 27)
                            }
                        }
                    }

                    if !customVisibleModels.isEmpty {
                        Divider()
                            .padding(.vertical, 10)
                    }

                    HStack(spacing: 8) {
                        TextField("输入模型名称，按回车添加", text: $customVisibleModelInput)
                            .textFieldStyle(.plain)
                            .onSubmit { addVisibleModel() }
                        Button {
                            addVisibleModel()
                        } label: {
                            Image(systemName: "plus")
                                .font(.system(size: 12, weight: .semibold))
                        }
                        .buttonStyle(.bordered)
                        .controlSize(.small)
                        .disabled(customVisibleModelInput.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                        .help("添加自定义模型")
                        .accessibilityLabel("添加自定义模型")
                    }
                    .padding(.horizontal, 10)
                    .padding(.vertical, 7)
                    .background(
                        Color(nsColor: .textBackgroundColor),
                        in: RoundedRectangle(cornerRadius: 8, style: .continuous)
                    )
                    .overlay {
                        RoundedRectangle(cornerRadius: 8, style: .continuous)
                            .stroke(Color.primary.opacity(0.10), lineWidth: 1)
                    }
                }
                .padding(.vertical, 4)
                .listRowSeparator(.hidden)
                .listRowBackground(Color.clear)

                DisclosureGroup(isExpanded: $manualVisibleModelsExpanded) {
                    TextEditor(text: $visibleModels)
                        .font(.body.monospaced())
                        .frame(minHeight: 90)
                        .padding(6)
                        .scrollContentBackground(.hidden)
                        .background(
                            Color(nsColor: .textBackgroundColor),
                            in: RoundedRectangle(cornerRadius: 8, style: .continuous)
                        )
                        .overlay {
                            RoundedRectangle(cornerRadius: 8, style: .continuous)
                                .stroke(Color.primary.opacity(0.10), lineWidth: 1)
                        }
                        .padding(.top, 8)
                } label: {
                    HStack(spacing: 6) {
                        Image(systemName: "pencil.line")
                        Text("批量编辑")
                        Spacer()
                        Text("每行一个")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                .font(.subheadline.weight(.medium))
                .padding(.vertical, 6)
                .listRowSeparator(.hidden)
                .listRowBackground(Color.clear)
            }
        }
        .listStyle(.inset)
        .scrollContentBackground(.hidden)
        .managementPageInsets(topPadding: 0)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func providersPage(_ providers: [ManageGatewayProvider]) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("\(filteredProviders(providers).count) / \(providers.count) 个供应商")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
                Spacer(minLength: 12)
                Picker("供应商筛选", selection: $providerFilter) {
                    ForEach(GatewayProviderFilter.allCases) { filter in
                        Text(filter.title).tag(filter)
                    }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .frame(width: 300)
            }
            .padding(.horizontal, ThreadRelayPageLayout.horizontalPadding)
            .padding(.bottom, 8)

            List {
                if providers.isEmpty {
                    ManagementEmptyState(
                        title: "尚未添加供应商",
                        message: "添加协议、Base URL、模型和 API Key 后即可开始路由。",
                        symbol: "server.rack"
                    )
                    .frame(maxWidth: .infinity, minHeight: 220)
                    .listRowSeparator(.hidden)
                } else if filteredProviders(providers).isEmpty {
                    ManagementEmptyState(
                        title: "没有匹配的供应商",
                        message: "换一个名称、协议或筛选条件试试。",
                        symbol: "magnifyingglass"
                    )
                    .frame(maxWidth: .infinity, minHeight: 180)
                    .listRowSeparator(.hidden)
                } else {
                    providerTableHeader
                        .listRowInsets(EdgeInsets())
                        .listRowSeparator(.hidden)
                        .listRowBackground(Color(nsColor: .windowBackgroundColor))
                    ForEach(filteredProviders(providers)) { provider in
                        GatewayProviderListRow(
                            provider: provider,
                            onEdit: { editor = GatewayProviderEditorState(provider: provider) },
                            onDelete: { providerToDelete = provider }
                        )
                        .listRowInsets(EdgeInsets())
                        .listRowSeparator(.hidden)
                    }
                }
            }
            .listStyle(.inset(alternatesRowBackgrounds: true))
            .scrollContentBackground(.hidden)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var accountPoolPage: some View {
        Form {
            Section("Sub2API 账号池") {
                sub2ApiAccountPoolContent
            }
        }
        .formStyle(.grouped)
        .scrollContentBackground(.hidden)
        .managementPageInsets(topPadding: 0)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var providerTableHeader: some View {
        HStack(spacing: 12) {
            Text("供应商")
                .frame(minWidth: 180, maxWidth: .infinity, alignment: .leading)
            Text("协议")
                .frame(width: 140, alignment: .leading)
            Text("模型")
                .frame(width: 70, alignment: .trailing)
            Text("权重")
                .frame(width: 70, alignment: .trailing)
            Text("状态")
                .frame(width: 90, alignment: .leading)
            Text("")
                .frame(width: 70)
        }
        .font(.caption.weight(.semibold))
        .foregroundStyle(.secondary)
        .padding(.horizontal, 12)
        .padding(.vertical, 5)
    }

    private var sub2ApiAccountPoolContent: some View {
        let admin = model.sub2ApiAdmin
        let configured = admin?.configured == true
        let hasSavedKey = admin?.secretSet == true
        let trimmedURL = sub2ApiBaseURL.trimmingCharacters(in: .whitespacesAndNewlines)
        let trimmedKey = sub2ApiAdminKey.trimmingCharacters(in: .whitespacesAndNewlines)
        let canSave = !trimmedURL.isEmpty
            && (hasSavedKey || !trimmedKey.isEmpty)
            && !sub2ApiSaving

        return Group {
            HStack(spacing: 10) {
                Image(systemName: configured ? "checkmark.circle.fill" : "circle.dashed")
                    .foregroundStyle(configured ? Color.green : .secondary)
                VStack(alignment: .leading, spacing: 2) {
                    Text(configured ? "管理连接已就绪" : "尚未连接账号池")
                        .font(.body.weight(.medium))
                    Text(configured ? "概览会显示账号状态、倍率和上游余额。" : "连接后可以在概览查看账号池状态。")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer(minLength: 12)
                if configured {
                    Button(sub2ApiEditing ? "取消" : "修改") {
                        sub2ApiEditing.toggle()
                    }
                    if !sub2ApiEditing {
                        Button("断开", role: .destructive) {
                            confirmSub2ApiDisconnect = true
                        }
                        .disabled(sub2ApiSaving)
                    }
                }
            }

            if !configured || sub2ApiEditing {
                Divider()
                LabeledContent("管理地址") {
                    TextField("https://sub2api.example.com", text: $sub2ApiBaseURL)
                        .textFieldStyle(.roundedBorder)
                        .frame(maxWidth: 560)
                        .accessibilityLabel("Sub2API 管理地址")
                }
                LabeledContent("Admin API Key") {
                    SecureField(
                        hasSavedKey ? "留空以继续使用已保存的密钥" : "输入管理密钥",
                        text: $sub2ApiAdminKey
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
                        Task { await saveSub2ApiAccountPool() }
                    } label: {
                        if sub2ApiSaving {
                            ProgressView()
                                .controlSize(.small)
                        } else {
                            Label(configured ? "更新连接" : "连接", systemImage: "link")
                        }
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(!canSave)
                }
            }
        }
    }

    private var settingsCard: some View {
            ManagementCard(title: "选项") {
                GatewayPreferenceRow(
                title: "MochiPort",
                detail: "服务状态由“连接 Codex”页面的开关管理。"
            ) {
                Label(
                    enabled ? "已启用" : "已停用",
                    systemImage: enabled ? "checkmark.circle.fill" : "pause.circle"
                )
                .foregroundStyle(enabled ? Color.green : .secondary)
            }

            Divider()

            VStack(alignment: .leading, spacing: 10) {
                Text("图片功能")
                    .font(.headline)
                GatewayPreferenceRow(
                    title: "关闭图片功能",
                    detail: "部分模型不支持图片功能时可以打开。"
                ) {
                    Toggle("关闭图片功能", isOn: $filterImages)
                        .labelsHidden()
                        .toggleStyle(.switch)
                }
            }

            Divider()

            VStack(alignment: .leading, spacing: 10) {
                Text("使用记录")
                    .font(.headline)
                GatewayPreferenceRow(
                    title: "保存使用记录",
                    detail: "保存简单记录，方便查看使用情况。"
                ) {
                    Toggle("保存使用记录", isOn: $requestLogging)
                        .labelsHidden()
                        .toggleStyle(.switch)
                }
                GatewayPreferenceRow(
                    title: "保存详细记录",
                    detail: requestLogging ? "保存更多内容，方便排查问题。" : "请先打开使用记录。"
                ) {
                    Toggle("保存详细记录", isOn: $requestDetails)
                        .labelsHidden()
                        .toggleStyle(.switch)
                        .disabled(!requestLogging)
                }
            }

            Divider()

            VStack(alignment: .leading, spacing: 6) {
                HStack(alignment: .firstTextBaseline) {
                    VStack(alignment: .leading, spacing: 4) {
                        Text("Codex 里的模型")
                            .font(.headline)
                        Text("只决定 Codex 里显示哪些模型。")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    Spacer()
                    Text("已选 \(mergedVisibleModels.count) 个")
                        .font(.caption.monospacedDigit().weight(.medium))
                        .foregroundStyle(.secondary)
                }
                if !modelCatalog.isEmpty {
                    VStack(alignment: .leading, spacing: 9) {
                        HStack(alignment: .firstTextBaseline) {
                            Label("目录模型", systemImage: "checklist")
                                .font(.subheadline.weight(.medium))
                            Spacer()
                            Text("已选 \(selectedCatalogModels.count) / \(modelCatalog.count)")
                                .font(.caption.monospacedDigit())
                                .foregroundStyle(.secondary)
                        }

                        if filteredVisibleCatalogModels.isEmpty {
                            Text("没有匹配的目录模型。")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(.vertical, 7)
                        } else {
                            LazyVGrid(
                                columns: [GridItem(.adaptive(minimum: 230), alignment: .leading)],
                                alignment: .leading,
                                spacing: 7
                            ) {
                                ForEach(filteredVisibleCatalogModels) { entry in
                                    let selected = selectedCatalogModels.contains(entry.id)
                                    Button {
                                        catalogBinding(entry.id).wrappedValue.toggle()
                                    } label: {
                                        HStack(spacing: 8) {
                                            Image(systemName: selected ? "checkmark.circle.fill" : "circle")
                                                .foregroundStyle(selected ? Color.accentColor : .secondary)
                                            VStack(alignment: .leading, spacing: 2) {
                                                Text(entry.displayName)
                                                    .font(.caption.weight(.medium))
                                                    .lineLimit(1)
                                                Text(entry.id)
                                                    .font(.caption2.monospaced())
                                                    .foregroundStyle(.secondary)
                                                    .lineLimit(1)
                                                    .truncationMode(.middle)
                                            }
                                            Spacer(minLength: 0)
                                        }
                                        .contentShape(Rectangle())
                                    }
                                    .buttonStyle(.plain)
                                    .padding(.horizontal, 9)
                                    .padding(.vertical, 7)
                                    .background(
                                        selected
                                            ? Color.accentColor.opacity(0.10)
                                            : Color(nsColor: .textBackgroundColor),
                                        in: RoundedRectangle(cornerRadius: 7)
                                    )
                                    .overlay {
                                        RoundedRectangle(cornerRadius: 7)
                                            .stroke(
                                                selected
                                                    ? Color.accentColor.opacity(0.28)
                                                    : Color.primary.opacity(0.09),
                                                lineWidth: 1
                                            )
                                    }
                                    .help(entry.id)
                                    .accessibilityLabel("可见模型 \(entry.displayName)")
                                    .accessibilityValue(selected ? "已选择" : "未选择")
                                }
                            }
                        }
                    }
                }

                HStack(alignment: .firstTextBaseline) {
                    Label("自定义模型", systemImage: "square.stack.3d.up")
                        .font(.subheadline.weight(.medium))
                    Spacer()
                    if !customVisibleModels.isEmpty {
                        Button("清空", role: .destructive) {
                            visibleModels = ""
                        }
                        .buttonStyle(.borderless)
                    }
                }

                if customVisibleModels.isEmpty {
                    Text("还没有自定义模型。可在这里添加目录之外的模型名称。")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(11)
                        .background(Color(nsColor: .textBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
                } else {
                    LazyVGrid(
                        columns: [GridItem(.adaptive(minimum: 230), alignment: .leading)],
                        alignment: .leading,
                        spacing: 7
                    ) {
                        ForEach(customVisibleModels, id: \.self) { model in
                            GatewayModelToken(model: model) {
                                removeVisibleModel(model)
                            }
                        }
                    }
                }

                HStack(spacing: 8) {
                    TextField("输入模型名称后添加", text: $customVisibleModelInput)
                        .textFieldStyle(.roundedBorder)
                        .onSubmit { addVisibleModel() }
                    Button {
                        addVisibleModel()
                    } label: {
                        Image(systemName: "plus")
                    }
                    .buttonStyle(.bordered)
                    .disabled(customVisibleModelInput.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    .help("添加自定义模型")
                    .accessibilityLabel("添加自定义模型")
                }

                DisclosureGroup(isExpanded: $manualVisibleModelsExpanded) {
                    TextEditor(text: $visibleModels)
                        .font(.body.monospaced())
                        .frame(minHeight: 72)
                        .padding(5)
                        .background(Color(nsColor: .textBackgroundColor), in: RoundedRectangle(cornerRadius: 7))
                        .padding(.top, 6)
                } label: {
                    HStack {
                        Label("批量编辑自定义模型", systemImage: "pencil.line")
                        Spacer()
                        Text("每行或逗号分隔")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                .font(.subheadline.weight(.medium))
            }

            HStack {
                if settingsDirty {
                    Label("有未保存更改", systemImage: "circle.fill")
                        .font(.caption)
                        .foregroundStyle(.orange)
                }
                Spacer()
                Button {
                    Task {
                        await model.saveGatewaySettings(
                            enabled: enabled,
                            filterImageGenerationTool: filterImages,
                            requestLoggingEnabled: requestLogging,
                            requestLogDetailsEnabled: requestLogging && requestDetails,
                            codexVisibleModels: mergedVisibleModels
                        )
                    }
                } label: {
                    if model.isLoading(.gateway) {
                        HStack(spacing: 7) {
                            ProgressView()
                                .controlSize(.small)
                            Text("保存中…")
                        }
                    } else {
                        Label(settingsDirty ? "保存更改" : "已保存", systemImage: settingsDirty ? "checkmark" : "checkmark.circle")
                    }
                }
                .buttonStyle(.borderedProminent)
                .disabled(!settingsReady || !settingsDirty || model.isLoading(.gateway))
            }
        }
    }

    private func sub2ApiAccountPoolCard(_ gateway: ManageGateway) -> some View {
        let admin = model.sub2ApiAdmin
        let configured = admin?.configured == true
        let hasSavedKey = admin?.secretSet == true
        let trimmedURL = sub2ApiBaseURL.trimmingCharacters(in: .whitespacesAndNewlines)
        let trimmedKey = sub2ApiAdminKey.trimmingCharacters(in: .whitespacesAndNewlines)
        let canSave = !trimmedURL.isEmpty
            && (hasSavedKey || !trimmedKey.isEmpty)
            && !sub2ApiSaving

        return ManagementCard(title: "Sub2API 账号池") {
            HStack(spacing: 10) {
                Image(systemName: configured ? "checkmark.circle.fill" : "circle.dashed")
                    .foregroundStyle(configured ? Color.green : Color.secondary)
                VStack(alignment: .leading, spacing: 2) {
                    Text(configured ? "管理连接已就绪" : "连接账号管理接口")
                        .font(.body.weight(.medium))
                    Text(configured ? "概览会显示账号状态、倍率和上游余额。" : "使用 Sub2API Admin API Key 读取账号池。")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer(minLength: 12)
                if configured {
                    Button("断开", role: .destructive) {
                        confirmSub2ApiDisconnect = true
                    }
                    .disabled(sub2ApiSaving)
                }
            }

            Divider()

            LabeledContent("管理地址") {
                TextField("https://sub2api.example.com", text: $sub2ApiBaseURL)
                    .textFieldStyle(.roundedBorder)
                    .frame(maxWidth: 560)
                    .accessibilityLabel("Sub2API 管理地址")
            }

            LabeledContent("Admin API Key") {
                SecureField(
                    hasSavedKey ? "留空以继续使用已保存的密钥" : "输入管理密钥",
                    text: $sub2ApiAdminKey
                )
                .textFieldStyle(.roundedBorder)
                .frame(maxWidth: 560)
                .accessibilityLabel("Sub2API Admin API Key")
            }

            HStack(alignment: .center, spacing: 12) {
                Label(
                    hasSavedKey ? "管理密钥已保存在本机，界面不会回显。" : "需要只读账号权限的管理密钥。",
                    systemImage: hasSavedKey ? "key.fill" : "key"
                )
                .font(.caption)
                .foregroundStyle(.secondary)
                Spacer(minLength: 12)
                Button {
                    Task { await saveSub2ApiAccountPool() }
                } label: {
                    if sub2ApiSaving {
                        HStack(spacing: 7) {
                            ProgressView()
                                .controlSize(.small)
                            Text("验证中…")
                        }
                    } else {
                        Label(configured ? "更新连接" : "连接", systemImage: "link")
                    }
                }
                .buttonStyle(.borderedProminent)
                .disabled(!canSave)
            }
        }
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

    @MainActor
    private func saveSub2ApiAccountPool() async {
        guard !sub2ApiSaving else { return }
        sub2ApiSaving = true
        defer { sub2ApiSaving = false }
        let key = sub2ApiAdminKey.trimmingCharacters(in: .whitespacesAndNewlines)
        let saved = await model.saveSub2ApiAdmin(
            baseUrl: sub2ApiBaseURL,
            adminApiKey: key.isEmpty ? nil : key
        )
        if saved {
            sub2ApiAdminKey = ""
        }
    }

    @MainActor
    private func disconnectSub2ApiAccountPool() async {
        guard !sub2ApiSaving else { return }
        sub2ApiSaving = true
        defer { sub2ApiSaving = false }
        guard await model.disconnectSub2ApiAdmin() else { return }
        sub2ApiAdminKey = ""
        sub2ApiBaseURL = suggestedSub2ApiBaseURL(model.gateway)
    }

    private func catalogBinding(_ id: String) -> Binding<Bool> {
        Binding(
            get: { selectedCatalogModels.contains(id) },
            set: { included in
                if included {
                    selectedCatalogModels.insert(id)
                } else {
                    selectedCatalogModels.remove(id)
                }
            }
        )
    }

    private var filteredVisibleCatalogModels: [ManageCodexCatalogModel] {
        let query = providerQuery.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !query.isEmpty else { return modelCatalog }
        return modelCatalog.filter {
            $0.id.lowercased().contains(query) || $0.displayName.lowercased().contains(query)
        }
    }

    private var customVisibleModels: [String] {
        splitValues(visibleModels)
    }

    private func addVisibleModel() {
        let value = customVisibleModelInput.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !value.isEmpty else { return }
        if let catalogEntry = modelCatalog.first(where: { $0.id == value }) {
            selectedCatalogModels.insert(catalogEntry.id)
        } else {
            visibleModels = mergedModelLines(existing: visibleModels, fetched: [value])
        }
        customVisibleModelInput = ""
    }

    private func removeVisibleModel(_ model: String) {
        visibleModels = splitValues(visibleModels)
            .filter { $0 != model }
            .joined(separator: "\n")
    }

    /// Catalog picks (in catalog order) plus the free-form entries, deduped.
    private var mergedVisibleModels: [String] {
        var seen = Set<String>()
        var merged: [String] = []
        for id in modelCatalog.map(\.id) where selectedCatalogModels.contains(id) {
            if seen.insert(id).inserted { merged.append(id) }
        }
        for custom in splitValues(visibleModels) {
            if seen.insert(custom).inserted { merged.append(custom) }
        }
        return merged
    }

    private var filterImagesBinding: Binding<Bool> {
        Binding(
            get: { filterImages },
            set: { newValue in
                guard newValue != filterImages, !preferencesSaving else { return }
                filterImages = newValue
                saveGatewayPreferences()
            }
        )
    }

    private var requestLoggingBinding: Binding<Bool> {
        Binding(
            get: { requestLogging },
            set: { newValue in
                guard newValue != requestLogging, !preferencesSaving else { return }
                requestLogging = newValue
                if !newValue { requestDetails = false }
                saveGatewayPreferences()
            }
        )
    }

    private var requestDetailsBinding: Binding<Bool> {
        Binding(
            get: { requestDetails },
            set: { newValue in
                guard newValue != requestDetails, !preferencesSaving else { return }
                requestDetails = newValue
                saveGatewayPreferences()
            }
        )
    }

    private var modelsDirty: Bool {
        guard let gateway = model.gateway else { return false }
        return Set(mergedVisibleModels) != Set(gateway.codexVisibleModels)
    }

    private func saveGatewayPreferences() {
        guard settingsReady, !preferencesSaving else { return }
        preferencesSaving = true
        let snapshot = (
            enabled: enabled,
            filterImages: filterImages,
            requestLogging: requestLogging,
            requestDetails: requestLogging && requestDetails,
            models: mergedVisibleModels
        )
        Task {
            let saved = await model.saveGatewaySettings(
                enabled: snapshot.enabled,
                filterImageGenerationTool: snapshot.filterImages,
                requestLoggingEnabled: snapshot.requestLogging,
                requestLogDetailsEnabled: snapshot.requestDetails,
                codexVisibleModels: snapshot.models
            )
            if !saved {
                synchronizeGateway(model.gateway)
            }
            preferencesSaving = false
        }
    }

    private func saveVisibleModels() {
        guard settingsReady, modelsDirty, !model.isLoading(.gateway) else { return }
        Task {
            _ = await model.saveGatewaySettings(
                enabled: enabled,
                filterImageGenerationTool: filterImages,
                requestLoggingEnabled: requestLogging,
                requestLogDetailsEnabled: requestLogging && requestDetails,
                codexVisibleModels: mergedVisibleModels
            )
        }
    }

    private var settingsDirty: Bool {
        guard let gateway = model.gateway else { return false }
        return enabled != gateway.enabled
            || filterImages != gateway.filterImageGenerationTool
            || requestLogging != gateway.requestLoggingEnabled
            || (requestLogging && requestDetails) != gateway.requestLogDetailsEnabled
            || Set(mergedVisibleModels) != Set(gateway.codexVisibleModels)
    }

    private func synchronizeGateway(_ gateway: ManageGateway?) {
        guard let gateway else {
            settingsReady = false
            return
        }
        enabled = gateway.enabled
        filterImages = gateway.filterImageGenerationTool
        requestLogging = gateway.requestLoggingEnabled
        requestDetails = gateway.requestLogDetailsEnabled
        let visible = gateway.codexVisibleModels
        if modelCatalog.isEmpty {
            selectedCatalogModels = []
            visibleModels = visible.joined(separator: "\n")
        } else {
            let catalogIds = Set(modelCatalog.map(\.id))
            selectedCatalogModels = Set(visible.filter { catalogIds.contains($0) })
            visibleModels = visible.filter { !catalogIds.contains($0) }.joined(separator: "\n")
        }
        settingsReady = true
    }

    private func providersCard(_ providers: [ManageGatewayProvider]) -> some View {
        ManagementCard(title: "Provider") {
            HStack {
                VStack(alignment: .leading, spacing: 3) {
                    Text("\(filteredProviders(providers).count) / \(providers.count) 个上游")
                        .font(.headline)
                    Text("按名称、协议或 Base URL 快速定位上游。")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer()
                Button {
                    editor = GatewayProviderEditorState(provider: nil)
                } label: {
                    Label("添加 Provider", systemImage: "plus")
                }
            }

            HStack(spacing: 10) {
                TextField("搜索 Provider…", text: $providerQuery)
                    .textFieldStyle(.roundedBorder)
                    .frame(minWidth: 190)
                Picker("Provider 筛选", selection: $providerFilter) {
                    ForEach(GatewayProviderFilter.allCases) { filter in
                        Text(filter.title).tag(filter)
                    }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .frame(maxWidth: 300)
            }

            if providers.isEmpty {
                ManagementEmptyState(
                    title: "尚未添加 Provider",
                    message: "添加上游协议、Base URL、模型和只写 API Key 后即可开始路由。",
                    symbol: "server.rack"
                )
                .frame(minHeight: 150)
                Button {
                    editor = GatewayProviderEditorState(provider: nil)
                } label: {
                    Label("添加第一个 Provider", systemImage: "plus")
                }
                .buttonStyle(.borderedProminent)
                .frame(maxWidth: .infinity)
            } else if filteredProviders(providers).isEmpty {
                ManagementEmptyState(
                    title: "没有匹配的 Provider",
                    message: "换一个名称或筛选条件试试。",
                    symbol: "line.3.horizontal.decrease.circle"
                )
                .frame(minHeight: 120)
            } else {
                ForEach(Array(filteredProviders(providers).enumerated()), id: \.element.id) { index, provider in
                    if index > 0 { Divider() }
                    GatewayProviderRow(
                        provider: provider,
                        onEdit: { editor = GatewayProviderEditorState(provider: provider) },
                        onDelete: { providerToDelete = provider }
                    )
                }
            }
        }
    }

    private func filteredProviders(_ providers: [ManageGatewayProvider]) -> [ManageGatewayProvider] {
        let query = providerQuery.trimmingCharacters(in: .whitespacesAndNewlines)
        return providers.filter { provider in
            let matchesFilter: Bool = switch providerFilter {
            case .all: true
            case .enabled: provider.enabled
            case .disabled: !provider.enabled
            }
            guard matchesFilter else { return false }
            guard !query.isEmpty else { return true }
            return provider.name.localizedCaseInsensitiveContains(query)
                || provider.baseUrl.localizedCaseInsensitiveContains(query)
                || gatewayProtocolDisplayName(provider.providerType, compatibility: provider.compatibility)
                    .localizedCaseInsensitiveContains(query)
                || provider.models.contains { $0.localizedCaseInsensitiveContains(query) }
        }
    }
}

/// Maps stable provider-type identifiers to the display names used by the
/// legacy GUI (see `provider_protocol_display` in `src/gui/ai_gateway.rs`).
func gatewayProtocolDisplayName(_ providerType: String, compatibility: String?) -> String {
    switch providerType {
    case "open_ai_responses": return "OpenAI Responses"
    case "deepseek_responses": return "DeepSeek Responses"
    case "grok_responses": return "Grok Responses"
    case "chat_completions": return "Chat Completions"
    case "anthropic_messages":
        if compatibility == "glm_anthropic" || compatibility == "zhipu_anthropic" {
            return "GLM Anthropic Messages"
        }
        return "Anthropic Messages"
    default: return providerType
    }
}

private struct GatewaySummaryMetric: View {
    let title: String
    let value: String
    let symbol: String
    let tint: Color

    var body: some View {
        HStack(spacing: 9) {
            Image(systemName: symbol)
                .font(.system(size: 15, weight: .semibold))
                .foregroundStyle(tint)
                .frame(width: 22)
            VStack(alignment: .leading, spacing: 2) {
                Text(value)
                    .font(.headline.weight(.semibold))
                    .monospacedDigit()
                    .lineLimit(1)
                    .minimumScaleFactor(0.8)
                Text(title)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct GatewayPreferenceRow<Control: View>: View {
    let title: String
    let detail: String
    @ViewBuilder let control: Control

    init(
        title: String,
        detail: String,
        @ViewBuilder control: () -> Control
    ) {
        self.title = title
        self.detail = detail
        self.control = control()
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 16) {
            VStack(alignment: .leading, spacing: 3) {
                Text(title)
                    .font(.body.weight(.medium))
                Text(detail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 12)
            control
        }
    }
}

private struct GatewayProviderRow: View {
    @EnvironmentObject private var model: AppModel
    let provider: ManageGatewayProvider
    let onEdit: () -> Void
    let onDelete: () -> Void

    @State private var localEnabled: Bool
    @State private var toggleInFlight = false

    init(
        provider: ManageGatewayProvider,
        onEdit: @escaping () -> Void,
        onDelete: @escaping () -> Void
    ) {
        self.provider = provider
        self.onEdit = onEdit
        self.onDelete = onDelete
        _localEnabled = State(initialValue: provider.enabled)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 10) {
                Circle()
                    .fill(localEnabled ? Color.green : Color.secondary.opacity(0.4))
                    .frame(width: 8, height: 8)
                ProviderLogoView(
                    providerType: provider.providerType,
                    compatibility: provider.compatibility,
                    providerName: provider.name,
                    size: 24
                )
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 8) {
                        Text(provider.name)
                            .font(.body.weight(.semibold))
                            .lineLimit(1)
                        Text(gatewayProtocolDisplayName(provider.providerType, compatibility: provider.compatibility))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                    StatusCapsule(
                        text: localEnabled ? "参与路由" : "已停用",
                        positive: localEnabled
                    )
                }
                Spacer(minLength: 10)
                Toggle("启用 Provider", isOn: toggleBinding)
                    .toggleStyle(.switch)
                    .controlSize(.small)
                    .labelsHidden()
                    .disabled(toggleInFlight)
                    .help(localEnabled ? "停用 Provider" : "启用 Provider")
                    .accessibilityLabel("启用 Provider \(provider.name)")
                Menu {
                    Button("编辑", action: onEdit)
                    Button("复制 Base URL") {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(provider.baseUrl, forType: .string)
                    }
                    Divider()
                    Button("删除", role: .destructive, action: onDelete)
                } label: {
                    Image(systemName: "ellipsis.circle")
                }
                .menuStyle(.borderlessButton)
                .help("Provider 操作")
                .accessibilityLabel("Provider 操作")
            }

            HStack(spacing: 12) {
                Label("\(provider.models.count) 个模型", systemImage: "cube")
                Label("权重 \(provider.weight)", systemImage: "slider.horizontal.3")
                Label("超时 \(provider.timeoutSecs) 秒", systemImage: "clock")
                Label(
                    provider.secretSet ? "API Key 已设置" : "未设置 API Key",
                    systemImage: provider.secretSet ? "key.fill" : "key.slash"
                )
                .foregroundStyle(provider.secretSet ? Color.secondary : Color.orange)
            }
            .font(.caption)
            .foregroundStyle(.secondary)
            .lineLimit(1)

            Text(provider.baseUrl)
                .font(.caption.monospaced())
                .foregroundStyle(.tertiary)
                .lineLimit(1)
                .truncationMode(.middle)
                .textSelection(.enabled)
        }
        .padding(.vertical, 7)
        .onChange(of: provider.enabled) { enabled in
            localEnabled = enabled
        }
    }

    /// Optimistic switch: flip locally right away, submit through the same
    /// upsert route (API key untouched), and roll back if the daemon refuses.
    private var toggleBinding: Binding<Bool> {
        Binding(
            get: { localEnabled },
            set: { newValue in
                guard newValue != localEnabled, !toggleInFlight else { return }
                localEnabled = newValue
                toggleInFlight = true
                let updated = ManageGatewayProvider(
                    name: provider.name,
                    enabled: newValue,
                    providerType: provider.providerType,
                    compatibility: provider.compatibility,
                    baseUrl: provider.baseUrl,
                    modelsUrl: provider.modelsUrl,
                    models: provider.models,
                    modelAliases: provider.modelAliases,
                    promptCacheRetention: provider.promptCacheRetention,
                    weight: provider.weight,
                    timeoutSecs: provider.timeoutSecs,
                    secretSet: provider.secretSet
                )
                Task {
                    let acknowledged = await model.saveGatewayProvider(
                        originalName: provider.name,
                        provider: updated,
                        apiKey: nil,
                        clearAPIKey: false
                    )
                    if !acknowledged {
                        localEnabled = !newValue
                    }
                    toggleInFlight = false
                }
            }
        )
    }
}

private struct GatewayProviderListRow: View {
    @EnvironmentObject private var model: AppModel
    let provider: ManageGatewayProvider
    let onEdit: () -> Void
    let onDelete: () -> Void

    @State private var localEnabled: Bool
    @State private var toggleInFlight = false

    init(
        provider: ManageGatewayProvider,
        onEdit: @escaping () -> Void,
        onDelete: @escaping () -> Void
    ) {
        self.provider = provider
        self.onEdit = onEdit
        self.onDelete = onDelete
        _localEnabled = State(initialValue: provider.enabled)
    }

    var body: some View {
        HStack(spacing: 12) {
            HStack(spacing: 9) {
                Circle()
                    .fill(localEnabled ? Color.green : Color.secondary.opacity(0.4))
                    .frame(width: 7, height: 7)
                ProviderLogoView(
                    providerType: provider.providerType,
                    compatibility: provider.compatibility,
                    providerName: provider.name,
                    size: 24
                )
                VStack(alignment: .leading, spacing: 2) {
                    Text(provider.name)
                        .font(.body.weight(.medium))
                        .lineLimit(1)
                    Text(provider.baseUrl)
                        .font(.caption2.monospaced())
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
            }
            .frame(minWidth: 180, maxWidth: .infinity, alignment: .leading)

            Text(gatewayProtocolDisplayName(provider.providerType, compatibility: provider.compatibility))
                .font(.callout)
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .frame(width: 140, alignment: .leading)

            Text("\(provider.models.count)")
                .font(.callout.monospacedDigit())
                .frame(width: 70, alignment: .trailing)

            Text("\(provider.weight)")
                .font(.callout.monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(width: 70, alignment: .trailing)

            Toggle("启用 Provider", isOn: toggleBinding)
                .toggleStyle(.switch)
                .controlSize(.small)
                .labelsHidden()
                .frame(width: 90, alignment: .leading)
                .disabled(toggleInFlight)
                .help(localEnabled ? "停用 Provider" : "启用 Provider")
                .accessibilityLabel("启用 Provider \(provider.name)")

            Menu {
                Button("编辑", action: onEdit)
                Button("复制 Base URL") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(provider.baseUrl, forType: .string)
                }
                Divider()
                Button("删除", role: .destructive, action: onDelete)
            } label: {
                Image(systemName: "ellipsis.circle")
            }
            .menuStyle(.borderlessButton)
            .frame(width: 70, alignment: .trailing)
            .help("供应商操作")
            .accessibilityLabel("供应商操作 \(provider.name)")
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 7)
        .contentShape(Rectangle())
        .onChange(of: provider.enabled) { enabled in
            localEnabled = enabled
        }
    }

    private var toggleBinding: Binding<Bool> {
        Binding(
            get: { localEnabled },
            set: { newValue in
                guard newValue != localEnabled, !toggleInFlight else { return }
                localEnabled = newValue
                toggleInFlight = true
                let updated = ManageGatewayProvider(
                    name: provider.name,
                    enabled: newValue,
                    providerType: provider.providerType,
                    compatibility: provider.compatibility,
                    baseUrl: provider.baseUrl,
                    modelsUrl: provider.modelsUrl,
                    models: provider.models,
                    modelAliases: provider.modelAliases,
                    promptCacheRetention: provider.promptCacheRetention,
                    weight: provider.weight,
                    timeoutSecs: provider.timeoutSecs,
                    secretSet: provider.secretSet
                )
                Task {
                    let acknowledged = await model.saveGatewayProvider(
                        originalName: provider.name,
                        provider: updated,
                        apiKey: nil,
                        clearAPIKey: false
                    )
                    if !acknowledged { localEnabled = !newValue }
                    toggleInFlight = false
                }
            }
        )
    }
}

private struct GatewayProviderEditorState: Identifiable {
    let id = UUID()
    let originalName: String?
    let provider: ManageGatewayProvider?

    init(provider: ManageGatewayProvider?) {
        originalName = provider?.name
        self.provider = provider
    }
}

private struct GatewayProviderEditor: View {
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var model: AppModel
    let state: GatewayProviderEditorState
    let onSave: (String?, ManageGatewayProvider, String?, Bool) async -> Bool

    @State private var name: String
    @State private var enabled: Bool
    @State private var providerType: String
    @State private var compatibility: String
    @State private var baseURL: String
    @State private var modelsURL: String
    @State private var models: String
    @State private var promptCacheRetention: String
    @State private var weight: Int
    @State private var timeoutSecs: Int
    @State private var apiKey = ""
    @State private var clearAPIKey = false
    @State private var saving = false
    @State private var aliasEntries: [ModelAliasEntry]
    @State private var templates: [ManageProviderTemplate] = []
    @State private var selectedTemplateID = ""
    @State private var fetchingModels = false
    @State private var fetchModelsNotice: String?
    @State private var fetchModelsNoticeIsPositive = true
    @State private var fetchModelsFailureLines: [String] = []
    @State private var fetchModelsAttemptDetailsExpanded = false
    @State private var providerUsage: ManageProviderUsageResponse?
    @State private var providerUsageError: String?
    @State private var fetchingProviderUsage = false
    @State private var providerUsageRequestGeneration = 0
    @State private var providerUsageTask: Task<Void, Never>?
    @State private var manualModelsExpanded = false
    @State private var modelQuery = ""
    @State private var customModelInput = ""

    private struct ModelAliasEntry: Identifiable {
        let id = UUID()
        var alias: String
        var target: String
    }

    private let providerTypes = [
        "open_ai_responses",
        "deepseek_responses",
        "grok_responses",
        "chat_completions",
        "anthropic_messages",
    ]

    init(
        state: GatewayProviderEditorState,
        onSave: @escaping (String?, ManageGatewayProvider, String?, Bool) async -> Bool
    ) {
        self.state = state
        self.onSave = onSave
        let provider = state.provider
        _name = State(initialValue: provider?.name ?? "")
        _enabled = State(initialValue: provider?.enabled ?? true)
        _providerType = State(initialValue: provider?.providerType ?? "open_ai_responses")
        _compatibility = State(initialValue: provider?.compatibility ?? "")
        _baseURL = State(initialValue: provider?.baseUrl ?? "")
        _modelsURL = State(initialValue: provider?.modelsUrl ?? "")
        _models = State(initialValue: provider?.models.joined(separator: "\n") ?? "")
        _promptCacheRetention = State(initialValue: provider?.promptCacheRetention ?? "")
        _weight = State(initialValue: provider?.weight ?? 100)
        _timeoutSecs = State(initialValue: provider?.timeoutSecs ?? 600)
        _aliasEntries = State(initialValue: (provider?.modelAliases ?? [:])
            .sorted { $0.key < $1.key }
            .map { ModelAliasEntry(alias: $0.key, target: $0.value) })
    }

    private var trimmedAliases: [(alias: String, target: String)] {
        aliasEntries.compactMap { entry in
            let alias = entry.alias.trimmingCharacters(in: .whitespacesAndNewlines)
            let target = entry.target.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !alias.isEmpty, !target.isEmpty else { return nil }
            return (alias, target)
        }
    }

    private var hasDuplicateAliases: Bool {
        let aliases = trimmedAliases.map(\.alias)
        return Set(aliases).count != aliases.count
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                VStack(alignment: .leading, spacing: 3) {
                    Text(state.provider == nil ? "添加 Provider" : "编辑 Provider")
                        .font(.title2.weight(.semibold))
                    Text("API Key 只写不回显；留空会保留已经保存的值。")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer()
            }
            .padding(24)

            Divider()

            Form {
                if state.provider == nil, !templates.isEmpty {
                    Picker("服务商模板", selection: $selectedTemplateID) {
                        Text("自定义").tag("")
                        ForEach(templates) { template in
                            Text(template.displayName).tag(template.id)
                        }
                    }
                    .accessibilityLabel("选择服务商模板")
                    .onChange(of: selectedTemplateID) { id in
                        guard let template = templates.first(where: { $0.id == id }) else { return }
                        applyTemplate(template)
                    }
                }
                TextField("名称", text: $name)
                Toggle("启用", isOn: $enabled)
                Picker("协议", selection: $providerType) {
                    ForEach(providerTypes, id: \.self) { type in
                        Text(gatewayProtocolDisplayName(type, compatibility: nil)).tag(type)
                    }
                }
                TextField("兼容配置（可选）", text: $compatibility)
                TextField("Base URL", text: $baseURL)
                TextField("Models URL（可选）", text: $modelsURL)
                SecureField(
                    state.provider?.secretSet == true ? "API Key（已设置）" : "API Key",
                    text: $apiKey
                )
                .disabled(clearAPIKey)
                if state.provider?.secretSet == true {
                    Toggle("清除已保存的 API Key", isOn: $clearAPIKey)
                }
                if state.provider != nil {
                    providerUsageSection
                }
                TextField("Prompt Cache Retention（可选）", text: $promptCacheRetention)
                Stepper("权重：\(weight)", value: $weight, in: 1...10_000)
                Stepper("超时：\(timeoutSecs) 秒", value: $timeoutSecs, in: 1...3_600)
                modelDiscoverySection
                VStack(alignment: .leading, spacing: 8) {
                    Text("模型映射（对外别名 → 上游模型）")
                    Text("Codex 侧使用别名调用时，网关会替换为对应的上游模型标识。")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    Text("检测到 Claude 系列模型时会自动补充对应的 Codex 别名，手动映射优先。")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    ForEach($aliasEntries) { $entry in
                        HStack(spacing: 8) {
                            TextField("对外别名", text: $entry.alias)
                                .textFieldStyle(.roundedBorder)
                            Image(systemName: "arrow.right")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            TextField("上游模型", text: $entry.target)
                                .textFieldStyle(.roundedBorder)
                            Button {
                                aliasEntries.removeAll { $0.id == entry.id }
                            } label: {
                                Image(systemName: "trash")
                            }
                            .buttonStyle(.plain)
                            .foregroundStyle(.red)
                            .help("删除映射")
                            .accessibilityLabel("删除映射 \(entry.alias)")
                        }
                    }
                    if hasDuplicateAliases {
                        Text("存在重复的对外别名，请先去重再保存。")
                            .font(.caption)
                            .foregroundStyle(.red)
                    }
                    Button {
                        aliasEntries.append(ModelAliasEntry(alias: "", target: ""))
                    } label: {
                        Label("添加映射", systemImage: "plus")
                    }
                    .accessibilityLabel("添加模型映射")
                }
            }
            .formStyle(.grouped)
            .padding(.horizontal, 16)

            Divider()

            HStack {
                if let error = model.managementOperationError {
                    Text(error)
                        .font(.caption)
                        .foregroundStyle(.red)
                        .lineLimit(2)
                }
                Spacer()
                Button("取消", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button("保存") {
                    save()
                }
                .keyboardShortcut(.defaultAction)
                .buttonStyle(.borderedProminent)
                .disabled(
                    saving
                        || name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                        || baseURL.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                        || hasDuplicateAliases
                )
            }
            .padding(18)
        }
        .frame(minWidth: 700, idealWidth: 760, minHeight: 700, idealHeight: 760)
        .task {
            // Templates only make sense when creating a provider; failures
            // (including older daemons without the endpoint) silently keep
            // the plain form.
            guard state.provider == nil else { return }
            templates = await model.loadGatewayProviderTemplates() ?? []
        }
        .onChange(of: name) { _ in invalidateProviderUsage() }
        .onChange(of: enabled) { _ in invalidateProviderUsage() }
        .onChange(of: providerType) { _ in invalidateProviderUsage() }
        .onChange(of: compatibility) { _ in invalidateProviderUsage() }
        .onChange(of: baseURL) { _ in invalidateProviderUsage() }
        .onChange(of: modelsURL) { _ in invalidateProviderUsage() }
        .onChange(of: models) { _ in invalidateProviderUsage() }
        .onChange(of: promptCacheRetention) { _ in invalidateProviderUsage() }
        .onChange(of: weight) { _ in invalidateProviderUsage() }
        .onChange(of: timeoutSecs) { _ in invalidateProviderUsage() }
        .onChange(of: apiKey) { _ in invalidateProviderUsage() }
        .onChange(of: clearAPIKey) { _ in invalidateProviderUsage() }
        .onChange(of: aliasEntries.map { "\($0.alias)\u{0}\($0.target)" }) { _ in
            invalidateProviderUsage()
        }
        .onDisappear {
            invalidateProviderUsage()
        }
    }

    private var canFetchProviderUsage: Bool {
        state.provider?.secretSet == true && !clearAPIKey && !fetchingProviderUsage
    }

    @ViewBuilder
    private var providerUsageSection: some View {
        Section("余额与倍率") {
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                VStack(alignment: .leading, spacing: 3) {
                    Text("查询已保存 API Key 的余额和计费倍率。")
                        .font(.body)
                    Text("输入框中尚未保存的新 Key 不会参与查询。")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer(minLength: 12)
                Button {
                    fetchProviderUsage()
                } label: {
                    if fetchingProviderUsage {
                        HStack(spacing: 6) {
                            ProgressView()
                                .controlSize(.small)
                            Text("查询中…")
                        }
                    } else {
                        Label(providerUsage == nil ? "查询" : "刷新", systemImage: "arrow.clockwise")
                    }
                }
                .disabled(!canFetchProviderUsage)
                .accessibilityLabel("查询 Provider 余额与倍率")
            }

            if state.provider?.secretSet != true {
                Label("请先保存 API Key，再查询余额与倍率。", systemImage: "key.slash")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            } else if clearAPIKey {
                Label("已选择清除 API Key，无法查询。", systemImage: "exclamationmark.circle")
                    .font(.caption)
                    .foregroundStyle(.orange)
            }

            if let response = providerUsage {
                providerUsageRows(response.usage)
            }

            if let providerUsageError {
                Label(providerUsageError, systemImage: "exclamationmark.triangle.fill")
                    .font(.caption)
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    @ViewBuilder
    private func providerUsageRows(_ usage: ManageProviderUsageResponse.Usage) -> some View {
        LabeledContent("余额") {
            Text(providerUsageBalanceText(usage))
                .font(.body.monospacedDigit().weight(.medium))
        }

        LabeledContent("当前倍率") {
            Text(
                providerUsageMultiplierText(
                    usage.effectiveRateMultiplier ?? usage.resolvedRateMultiplier
                ) ?? providerUsageBillingText(usage)
            )
            .font(.body.monospacedDigit().weight(.medium))
        }

        if let resolvedRateMultiplier = usage.resolvedRateMultiplier {
            LabeledContent("基础倍率") {
                Text(providerUsageMultiplierText(resolvedRateMultiplier) ?? "—")
                    .font(.body.monospacedDigit())
            }
        }

        if let components = providerUsageRateComponentsText(usage) {
            LabeledContent("倍率构成", value: components)
        }

        if let planName = usage.planName.flatMap(nilIfEmpty) {
            LabeledContent("套餐", value: planName)
        }

        if let accountWarning = providerUsageAccountWarning(usage.accountStatus) {
            Label(accountWarning, systemImage: "exclamationmark.circle.fill")
                .font(.caption)
                .foregroundStyle(.orange)
        } else if usage.accountValid == false {
            Label("API Key 当前不可用。", systemImage: "exclamationmark.circle.fill")
                .font(.caption)
                .foregroundStyle(.orange)
        }

        if usage.peakRateEnabled == true {
            LabeledContent("峰时计费") {
                Text(providerUsagePeakText(usage))
                    .font(.body.monospacedDigit())
                    .multilineTextAlignment(.trailing)
            }
        }

        HStack(spacing: 5) {
            Text("来源：\(usage.source)")
            if let observedAt = usage.observedAt.flatMap(nilIfEmpty) {
                Text("·")
                Text(observedAt)
            }
        }
        .font(.caption)
        .foregroundStyle(.secondary)
        .lineLimit(1)
        .truncationMode(.middle)
    }

    /// Fills the form with a template's defaults; every field stays editable
    /// afterwards. Switching back to "自定义" keeps the current values.
    private func applyTemplate(_ template: ManageProviderTemplate) {
        name = template.id
        providerType = template.providerType
        compatibility = template.compatibility ?? ""
        baseURL = template.baseUrl
        modelsURL = template.modelsUrl ?? ""
        models = template.models.joined(separator: "\n")
    }

    private var configuredModels: [String] {
        var seen = Set<String>()
        return splitValues(models).filter { seen.insert($0).inserted }
    }

    private var filteredConfiguredModels: [String] {
        let query = modelQuery.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !query.isEmpty else { return configuredModels }
        return configuredModels.filter { $0.lowercased().contains(query) }
    }

    private var modelFetchSource: String {
        let source = nilIfEmpty(modelsURL) ?? nilIfEmpty(baseURL)
        return source ?? "未设置上游地址"
    }

    @ViewBuilder
    private var modelDiscoverySection: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .firstTextBaseline) {
                VStack(alignment: .leading, spacing: 3) {
                    Text("模型")
                        .font(.headline)
                    Text("从上游同步可用模型，也可以手动添加或移除。")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer()
                Text("已配置 \(configuredModels.count) 个")
                    .font(.caption.monospacedDigit().weight(.medium))
                    .foregroundStyle(.secondary)
            }

            HStack(spacing: 10) {
                Image(systemName: "arrow.triangle.2.circlepath")
                    .font(.title3)
                    .foregroundStyle(Color.accentColor)
                    .frame(width: 30, height: 30)
                VStack(alignment: .leading, spacing: 3) {
                    Text("从上游获取模型")
                        .font(.subheadline.weight(.medium))
                    Text(modelFetchSource)
                        .font(.caption.monospaced())
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .textSelection(.enabled)
                }
                Spacer(minLength: 12)
                Button {
                    fetchModelsFromUpstream()
                } label: {
                    if fetchingModels {
                        HStack(spacing: 6) {
                            ProgressView()
                                .controlSize(.small)
                            Text("获取中…")
                        }
                    } else {
                        Label("获取模型", systemImage: "arrow.down.circle")
                    }
                }
                .buttonStyle(.borderedProminent)
                .disabled(
                    fetchingModels
                        || baseURL.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                )
                .accessibilityLabel("从上游获取模型列表")
            }
            .padding(12)
            .background(Color.accentColor.opacity(0.08), in: RoundedRectangle(cornerRadius: 10))
            .overlay {
                RoundedRectangle(cornerRadius: 10)
                    .stroke(Color.accentColor.opacity(0.16), lineWidth: 1)
            }

            if let fetchModelsNotice {
                HStack(spacing: 7) {
                    Image(systemName: fetchModelsNoticeIsPositive ? "checkmark.circle.fill" : "exclamationmark.triangle.fill")
                    Text(fetchModelsNotice)
                        .lineLimit(2)
                    Spacer(minLength: 4)
                }
                .font(.caption)
                .foregroundStyle(fetchModelsNoticeIsPositive ? Color.green : Color.orange)
                .padding(.horizontal, 10)
                .padding(.vertical, 8)
                .background(
                    (fetchModelsNoticeIsPositive ? Color.green : Color.orange).opacity(0.09),
                    in: RoundedRectangle(cornerRadius: 8)
                )
            }

            if !fetchModelsFailureLines.isEmpty {
                DisclosureGroup(isExpanded: $fetchModelsAttemptDetailsExpanded) {
                    VStack(alignment: .leading, spacing: 5) {
                        ForEach(Array(fetchModelsFailureLines.enumerated()), id: \.offset) { _, line in
                            Text(line)
                                .font(.caption.monospaced())
                                .foregroundStyle(.secondary)
                                .lineLimit(3)
                                .textSelection(.enabled)
                        }
                    }
                    .padding(.top, 7)
                } label: {
                    HStack(spacing: 7) {
                        Image(systemName: "exclamationmark.octagon")
                            .foregroundStyle(.orange)
                        Text("查看获取详情")
                        Spacer()
                        Text("\(fetchModelsFailureLines.count) 次尝试")
                            .font(.caption.monospacedDigit())
                            .foregroundStyle(.secondary)
                    }
                }
                .font(.caption.weight(.medium))
                .padding(10)
                .background(Color.orange.opacity(0.08), in: RoundedRectangle(cornerRadius: 8))
            }

            HStack(alignment: .firstTextBaseline) {
                Label("已添加模型", systemImage: "checklist")
                    .font(.subheadline.weight(.medium))
                Spacer()
                if !configuredModels.isEmpty {
                    Button("清空", role: .destructive) {
                        models = ""
                        modelQuery = ""
                    }
                    .buttonStyle(.borderless)
                }
            }

            if !configuredModels.isEmpty {
                HStack(spacing: 7) {
                    Image(systemName: "magnifyingglass")
                        .foregroundStyle(.secondary)
                    TextField("搜索已添加模型…", text: $modelQuery)
                        .textFieldStyle(.plain)
                }
                .padding(.horizontal, 9)
                .padding(.vertical, 7)
                .background(Color(nsColor: .textBackgroundColor), in: RoundedRectangle(cornerRadius: 7))
            }

            if configuredModels.isEmpty {
                HStack(spacing: 8) {
                    Image(systemName: "cube.transparent")
                        .foregroundStyle(.secondary)
                    Text("还没有模型。点击“获取模型”或在下方手动添加。")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(12)
                .background(Color(nsColor: .textBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
            } else if filteredConfiguredModels.isEmpty {
                Text("没有匹配的模型。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(12)
            } else {
                ScrollView {
                    LazyVGrid(
                        columns: [GridItem(.adaptive(minimum: 190), alignment: .leading)],
                        alignment: .leading,
                        spacing: 7
                    ) {
                        ForEach(filteredConfiguredModels, id: \.self) { model in
                            GatewayModelToken(model: model) {
                                removeModel(model)
                            }
                        }
                    }
                    .padding(1)
                }
                .frame(maxHeight: 180)
            }

            HStack(spacing: 8) {
                TextField("输入模型名称后添加", text: $customModelInput)
                    .textFieldStyle(.roundedBorder)
                    .onSubmit { addCustomModel() }
                Button {
                    addCustomModel()
                } label: {
                    Image(systemName: "plus")
                }
                .buttonStyle(.bordered)
                .disabled(customModelInput.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                .help("添加模型")
                .accessibilityLabel("添加自定义模型")
            }

            DisclosureGroup(isExpanded: $manualModelsExpanded) {
                TextEditor(text: $models)
                    .font(.body.monospaced())
                    .frame(minHeight: 78)
                    .padding(5)
                    .background(Color(nsColor: .textBackgroundColor), in: RoundedRectangle(cornerRadius: 7))
                    .padding(.top, 6)
            } label: {
                HStack {
                    Label("手动编辑模型列表", systemImage: "pencil.line")
                    Spacer()
                    Text("每行一个")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            .font(.subheadline.weight(.medium))
        }
    }

    private func addCustomModel() {
        let value = customModelInput.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !value.isEmpty else { return }
        models = mergedModelLines(existing: models, fetched: [value])
        customModelInput = ""
    }

    private func removeModel(_ model: String) {
        models = configuredModels.filter { $0 != model }.joined(separator: "\n")
    }

    /// Asks the daemon to list the upstream's models with the form's current
    /// values. When editing an existing provider the stored API key is reused
    /// unless the user typed a new one in this session.
    private func fetchModelsFromUpstream() {
        guard !fetchingModels else { return }
        fetchingModels = true
        fetchModelsNotice = nil
        fetchModelsNoticeIsPositive = true
        fetchModelsFailureLines = []
        fetchModelsAttemptDetailsExpanded = false
        Task {
            do {
                let response = try await model.fetchGatewayProviderModels(
                    providerName: state.originalName,
                    baseUrl: baseURL.trimmingCharacters(in: .whitespacesAndNewlines),
                    modelsUrl: nilIfEmpty(modelsURL),
                    providerType: providerType,
                    apiKey: nilIfEmpty(apiKey)
                )
                if response.ok {
                    let existingModels = Set(configuredModels)
                    models = mergedModelLines(existing: models, fetched: response.models)
                    let addedCount = splitValues(models).filter { !existingModels.contains($0) }.count
                    if response.models.isEmpty {
                        fetchModelsNotice = "上游返回空列表"
                        fetchModelsNoticeIsPositive = false
                    } else if addedCount > 0 {
                        fetchModelsNotice = "已获取 \(response.models.count) 个模型，新增 \(addedCount) 个"
                        fetchModelsNoticeIsPositive = true
                    } else {
                        fetchModelsNotice = "已获取 \(response.models.count) 个模型，没有新增条目"
                        fetchModelsNoticeIsPositive = true
                    }
                } else if response.attempts.isEmpty {
                    fetchModelsNotice = "上游未返回模型列表"
                    fetchModelsNoticeIsPositive = false
                    fetchModelsFailureLines = ["上游未返回模型列表。"]
                } else {
                    fetchModelsNotice = "获取模型失败"
                    fetchModelsNoticeIsPositive = false
                    fetchModelsFailureLines = providerFetchAttemptLines(response.attempts)
                }
            } catch let error as APIClientError {
                fetchModelsNotice = "获取模型失败"
                fetchModelsNoticeIsPositive = false
                fetchModelsFailureLines = [error.localizedDescription]
            } catch {
                fetchModelsNotice = "获取模型失败"
                fetchModelsNoticeIsPositive = false
                fetchModelsFailureLines = ["无法连接本地服务。"]
            }
            fetchingModels = false
        }
    }

    private func fetchProviderUsage() {
        guard canFetchProviderUsage, let providerName = state.originalName else { return }
        providerUsageRequestGeneration += 1
        let generation = providerUsageRequestGeneration
        fetchingProviderUsage = true
        providerUsage = nil
        providerUsageError = nil

        providerUsageTask = Task {
            do {
                let response = try await model.fetchGatewayProviderUsage(
                    providerName: providerName
                )
                guard !Task.isCancelled,
                      generation == providerUsageRequestGeneration
                else { return }
                providerUsage = response
            } catch let error as APIClientError {
                guard !Task.isCancelled,
                      generation == providerUsageRequestGeneration
                else { return }
                providerUsageError = error.localizedDescription
            } catch {
                guard !Task.isCancelled,
                      generation == providerUsageRequestGeneration
                else { return }
                providerUsageError = "无法连接本地服务。"
            }

            guard !Task.isCancelled,
                  generation == providerUsageRequestGeneration
            else { return }
            fetchingProviderUsage = false
            providerUsageTask = nil
        }
    }

    /// Invalidates both finished and in-flight results. The generation check
    /// prevents a response started for an older form revision from reappearing.
    private func invalidateProviderUsage() {
        guard providerUsage != nil
                || providerUsageError != nil
                || fetchingProviderUsage
                || providerUsageTask != nil
        else {
            return
        }
        providerUsageTask?.cancel()
        providerUsageTask = nil
        providerUsageRequestGeneration += 1
        providerUsage = nil
        providerUsageError = nil
        fetchingProviderUsage = false
    }

    private func save() {
        saving = true
        let modelList = configuredModels
        let provider = ManageGatewayProvider(
            name: name.trimmingCharacters(in: .whitespacesAndNewlines),
            enabled: enabled,
            providerType: providerType,
            compatibility: nilIfEmpty(compatibility),
            baseUrl: baseURL.trimmingCharacters(in: .whitespacesAndNewlines),
            modelsUrl: nilIfEmpty(modelsURL),
            models: modelList,
            // Claude-series models gain their Codex aliases automatically;
            // explicit rows always win over inferred entries.
            modelAliases: mergedModelAliases(
                models: modelList,
                explicit: Dictionary(
                    trimmedAliases.map { ($0.alias, $0.target) },
                    uniquingKeysWith: { _, last in last }
                )
            ),
            promptCacheRetention: nilIfEmpty(promptCacheRetention),
            weight: weight,
            timeoutSecs: timeoutSecs,
            secretSet: clearAPIKey ? false : (state.provider?.secretSet ?? !apiKey.isEmpty)
        )
        Task {
            let saved = await onSave(
                state.originalName,
                provider,
                clearAPIKey ? nil : nilIfEmpty(apiKey),
                clearAPIKey
            )
            saving = false
            if saved { dismiss() }
        }
    }
}

private struct GatewayModelToken: View {
    let model: String
    let onRemove: () -> Void

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: "cube")
                .font(.caption)
                .foregroundStyle(Color.accentColor)
            Text(model)
                .font(.caption.monospaced())
                .lineLimit(1)
                .truncationMode(.middle)
                .frame(maxWidth: .infinity, alignment: .leading)
            Button(action: onRemove) {
                Image(systemName: "xmark")
                    .font(.caption2.weight(.semibold))
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .help("移除模型")
            .accessibilityLabel("移除模型 \(model)")
        }
        .help(model)
        .padding(.horizontal, 9)
        .padding(.vertical, 7)
        .background(Color(nsColor: .textBackgroundColor), in: RoundedRectangle(cornerRadius: 7))
        .overlay {
            RoundedRectangle(cornerRadius: 7)
                .stroke(Color.primary.opacity(0.09), lineWidth: 1)
        }
    }
}

struct RequestLogsView: View {
    @EnvironmentObject private var model: AppModel
    @State private var query = ""
    @State private var statusFilter: String?
    @State private var channelFilter: String?
    @State private var modelFilter: String?
    @State private var sort: RequestLogSort = .newest
    @State private var knownChannels: Set<String> = []
    @State private var knownModels: Set<String> = []
    @State private var selectedID: Int64?
    @State private var activeDetailID: Int64?
    @State private var confirmsClear = false
    @State private var confirmsClearOld = false
    @State private var clearing = false
    @State private var detailTask: Task<Void, Never>?

    private var filters: RequestLogFilters {
        RequestLogFilters(
            query: query,
            status: statusFilter,
            channel: channelFilter,
            modelId: modelFilter,
            sort: sort
        )
    }

    private var hasActiveFilters: Bool {
        !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            || statusFilter != nil
            || channelFilter != nil
            || modelFilter != nil
    }

    private var activeStructuredFilterCount: Int {
        [statusFilter, channelFilter, modelFilter].compactMap { $0 }.count
    }

    var body: some View {
        ZStack {
            VStack(alignment: .leading, spacing: 0) {
                filterBar
                    .pickerStyle(.menu)
                    .controlSize(.regular)
                    .padding(.horizontal, ThreadRelayPageLayout.horizontalPadding)
                    .padding(.top, ThreadRelayPageLayout.topPadding)
                    .padding(.bottom, 12)

                List(selection: $selectedID) {
                    if let error = model.sectionErrors[.requestLogs] {
                        InlineManagementError(
                            message: error,
                            retry: { Task { await model.loadSection(.requestLogs, force: true) } },
                            dismiss: { model.dismissSectionError(.requestLogs) }
                        )
                        .padding(.bottom, 12)
                        .listRowInsets(EdgeInsets())
                        .listRowSeparator(.hidden)
                        .listRowBackground(Color.clear)
                    }

                    if model.requestLogs.isEmpty, !model.isLoading(.requestLogs) {
                        ManagementEmptyState(
                            title: hasActiveFilters ? "没有匹配的请求" : "没有请求日志",
                            message: hasActiveFilters
                                ? "调整搜索或筛选条件后重试。"
                                : "在 AI 网关中开启请求日志后，新请求会显示在这里。",
                            symbol: "list.bullet.rectangle"
                        )
                        .frame(maxWidth: .infinity, minHeight: 240)
                        .listRowSeparator(.hidden)
                    } else {
                        requestLogTableHeader
                            .textCase(nil)
                            .listRowInsets(EdgeInsets())
                            .listRowSeparator(.hidden)
                            .listRowBackground(Color(nsColor: .windowBackgroundColor))

                        ForEach(model.requestLogs) { log in
                            RequestLogRow(log: log)
                                .tag(log.id)
                                .listRowInsets(EdgeInsets())
                                .listRowSeparator(.hidden)
                                .contentShape(Rectangle())
                                .onTapGesture {
                                    showDetail(id: log.id)
                                }
                        }

                        if model.requestLogHasMore {
                            HStack {
                                Spacer()
                                Button {
                                    Task { _ = await model.loadMoreRequestLogs() }
                                } label: {
                                    if model.requestLogLoadingMore {
                                        HStack(spacing: 7) {
                                            ProgressView()
                                                .controlSize(.small)
                                            Text("正在加载…")
                                        }
                                    } else {
                                        Label("加载更多", systemImage: "chevron.down")
                                    }
                                }
                                .disabled(model.requestLogLoadingMore)
                                Spacer()
                            }
                            .padding(.vertical, 8)
                            .listRowBackground(Color.clear)
                            .listRowSeparator(.hidden)
                        }
                    }
                }
                .listStyle(.inset(alternatesRowBackgrounds: true))
                .scrollContentBackground(.hidden)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            .opacity(activeDetailID == nil ? 1 : 0)
            .allowsHitTesting(activeDetailID == nil)
            .accessibilityHidden(activeDetailID != nil)

            if let activeDetailID {
                detailPage(logID: activeDetailID)
                    .transition(.opacity)
            }
        }
        .animation(.easeInOut(duration: 0.16), value: activeDetailID)
        .overlay {
            if model.isLoading(.requestLogs), model.requestLogs.isEmpty {
                ProgressView("正在读取请求日志…")
            }
        }
        .background(Color(nsColor: .windowBackgroundColor))
        .searchable(
            text: $query,
            placement: .toolbar,
            prompt: "搜索请求 ID、模型、渠道、Provider 或状态"
        )
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                cleanupMenu
            }
        }
        .task(id: filters) {
            if filters.query != model.requestLogFilters.query {
                try? await Task.sleep(for: .milliseconds(300))
                guard !Task.isCancelled else { return }
            }
            await model.setRequestLogFilters(filters)
        }
        // Mirrors the legacy GUI's 5-second list auto-refresh. Pauses while
        // the window is hidden or a clear/reload is already running; the
        // section is only mounted while the page is visible, so switching
        // away cancels the loop. Selection and the loaded detail survive
        // because the list reload never touches them.
        .task {
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(5))
                guard !Task.isCancelled else { return }
                guard model.isWindowVisible, !clearing, !model.isLoading(.requestLogs) else {
                    continue
                }
                _ = await model.loadSection(.requestLogs)
            }
        }
        .onChange(of: model.requestLogs) { logs in
            rememberFilterOptions(from: logs)
            if let id = activeDetailID, !logs.contains(where: { $0.id == id }) {
                closeDetail()
            }
            if let id = selectedID, !logs.contains(where: { $0.id == id }) {
                selectedID = nil
            }
        }
        .onChange(of: selectedID) { id in
            if let id, activeDetailID == nil {
                showDetail(id: id)
            }
        }
        .onAppear { rememberFilterOptions(from: model.requestLogs) }
        .onDisappear { detailTask?.cancel() }
        .alert("清空全部请求日志？", isPresented: $confirmsClear) {
            Button("取消", role: .cancel) {}
            Button("清空", role: .destructive) {
                Task {
                    clearing = true
                    if await model.clearRequestLogs() {
                        selectedID = nil
                        closeDetail()
                    }
                    clearing = false
                }
            }
        } message: {
            Text("该操作不可撤销。清空大量日志可能需要几分钟，请耐心等待。Provider 配置和网关状态不会受到影响。")
        }
        .alert("清理 3 天前的日志？", isPresented: $confirmsClearOld) {
            Button("取消", role: .cancel) {}
            Button("清理", role: .destructive) {
                Task {
                    clearing = true
                    _ = await model.clearOldRequestLogs()
                    clearing = false
                }
            }
        } message: {
            Text("将保留最近 3 天的请求日志，删除更早的记录。清理大量日志可能需要几分钟，请耐心等待。")
        }
    }

    private func detailPage(logID: Int64) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Button {
                    closeDetail()
                } label: {
                    Label("请求日志", systemImage: "chevron.left")
                }
                .help("返回请求日志")
                Spacer()
            }
            .padding(.horizontal, ThreadRelaySpacing.page)
            .padding(.vertical, 14)

            Divider()

            if let detail = model.requestLogDetail, detail.id == logID {
                RequestLogDetailContent(detail: detail)
            } else if let error = model.sectionErrors[.requestLogs] {
                InlineManagementError(
                    message: error,
                    retry: { loadDetail(id: logID) }
                )
                .padding(ThreadRelaySpacing.page)
                Spacer()
            } else {
                ProgressView("正在读取详情…")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color(nsColor: .windowBackgroundColor))
    }

    private func showDetail(id: Int64) {
        guard activeDetailID != id else { return }
        activeDetailID = id
        selectedID = id
        loadDetail(id: id)
    }

    private func loadDetail(id: Int64) {
        detailTask?.cancel()
        model.clearRequestLogDetail()
        model.dismissSectionError(.requestLogs)
        detailTask = Task { await model.loadRequestLogDetail(id: id) }
    }

    private func closeDetail() {
        detailTask?.cancel()
        detailTask = nil
        activeDetailID = nil
        model.clearRequestLogDetail()
        model.dismissSectionError(.requestLogs)
    }

    private func rememberFilterOptions(from logs: [ManageRequestLog]) {
        knownChannels.formUnion(logs.map(\.channel).filter { !$0.isEmpty })
        knownModels.formUnion(logs.map(\.modelId).filter { !$0.isEmpty })
    }

    private var filterBar: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 8) {
                statusPicker.frame(width: 118)
                channelPicker.frame(width: 140)
                modelPicker.frame(width: 180)
                sortPicker.frame(width: 118)
                Spacer(minLength: 8)
                loadedCount
            }

            HStack(spacing: 8) {
                Menu {
                    statusPicker
                    channelPicker
                    modelPicker
                    Divider()
                    Button("清除筛选") {
                        statusFilter = nil
                        channelFilter = nil
                        modelFilter = nil
                    }
                    .disabled(activeStructuredFilterCount == 0)
                } label: {
                    Label(
                        activeStructuredFilterCount == 0
                            ? "筛选"
                            : "筛选 \(activeStructuredFilterCount)",
                        systemImage: "line.3.horizontal.decrease.circle"
                    )
                }
                sortPicker.frame(width: 118)
                Spacer(minLength: 8)
                loadedCount
            }
        }
    }

    private var requestLogTableHeader: some View {
        HStack(spacing: 12) {
            Text("模型")
                .frame(minWidth: 180, maxWidth: .infinity, alignment: .leading)
            Text("渠道")
                .frame(width: 110, alignment: .leading)
            Text("状态")
                .frame(width: 90, alignment: .leading)
            Text("耗时 / 用量")
                .frame(width: 150, alignment: .trailing)
            Text("时间")
                .frame(width: 90, alignment: .trailing)
        }
        .font(.caption.weight(.semibold))
        .foregroundStyle(.secondary)
        .padding(.horizontal, 12)
        .padding(.vertical, 5)
    }

    private var cleanupMenu: some View {
        Menu {
            Button("清理 3 天前的日志…") {
                confirmsClearOld = true
            }
            Button("清空全部日志…", role: .destructive) {
                confirmsClear = true
            }
        } label: {
            if clearing {
                HStack(spacing: 6) {
                    ProgressView()
                        .controlSize(.small)
                    Text("正在清理…")
                }
            } else {
                Label("清理", systemImage: "trash")
            }
        }
        .disabled(clearing)
        .help("清理请求日志")
        .accessibilityLabel("清理请求日志")
    }

    private var statusPicker: some View {
        Picker("状态", selection: $statusFilter) {
            Text("全部状态").tag(String?.none)
            Text("进行中").tag(String?.some("running"))
            Text("已完成").tag(String?.some("completed"))
            Text("失败").tag(String?.some("failed"))
            Text("已取消").tag(String?.some("cancelled"))
            Text("成功（兼容）").tag(String?.some("success"))
        }
    }

    private var channelPicker: some View {
        Picker("渠道", selection: $channelFilter) {
            Text("全部渠道").tag(String?.none)
            ForEach(knownChannels.sorted(), id: \.self) { channel in
                Text(channel).tag(String?.some(channel))
            }
        }
    }

    private var modelPicker: some View {
        Picker("模型", selection: $modelFilter) {
            Text("全部模型").tag(String?.none)
            ForEach(knownModels.sorted(), id: \.self) { modelID in
                Text(modelID).tag(String?.some(modelID))
            }
        }
    }

    private var sortPicker: some View {
        Picker("排序", selection: $sort) {
            ForEach(RequestLogSort.allCases, id: \.self) { option in
                Text(option.label).tag(option)
            }
        }
    }

    private var loadedCount: some View {
        Text("\(model.requestLogs.count) 条")
            .font(.caption)
            .foregroundStyle(.secondary)
            .monospacedDigit()
    }
}

private struct RequestLogRow: View {
    let log: ManageRequestLog

    var body: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(log.modelId)
                        .font(.body.weight(.medium))
                        .lineLimit(1)
                    if log.stream {
                        Image(systemName: "bolt.horizontal")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .help("流式")
                            .accessibilityLabel("流式")
                    }
                }
                Text(log.requestId)
                    .font(.caption2.monospaced())
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            .frame(minWidth: 180, maxWidth: .infinity, alignment: .leading)

            VStack(alignment: .leading, spacing: 3) {
                Text(log.channel)
                    .font(.callout.weight(.medium))
                    .lineLimit(1)
                Text(log.providerType)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            .frame(width: 110, alignment: .leading)

            StatusCapsule(text: log.status, positive: isPositiveStatus(log.status))
                .frame(width: 90, alignment: .leading)

            VStack(alignment: .trailing, spacing: 3) {
                Text(log.latencyMs.map { "\($0) ms" } ?? "等待耗时")
                    .font(.callout.monospacedDigit())
                HStack(spacing: 5) {
                    if let tokens = log.totalTokens {
                        Text("\(tokens) tokens")
                    }
                    if let cost = log.costUsd {
                        Text(String(format: "$%.6f", cost))
                    }
                }
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
                .lineLimit(1)
            }
            .frame(width: 150, alignment: .trailing)

            Text(relativeDate(milliseconds: log.createdAtMs))
                .font(.callout.monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(width: 90, alignment: .trailing)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .contentShape(Rectangle())
        .help("请求 ID：\(log.requestId)")
    }
}

struct RequestLogDetailContent: View {
    let detail: ManageRequestLogDetail
    @State private var section = DetailSection.summary

    private enum DetailSection: String, CaseIterable, Identifiable {
        case summary = "摘要"
        case request = "Codex 请求"
        case upstream = "上游请求"
        case stream = "SSE"
        case response = "响应"
        case error = "错误"

        var id: String { rawValue }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .top) {
                VStack(alignment: .leading, spacing: 5) {
                    Text(detail.modelId)
                        .font(.title3.weight(.semibold))
                    Text(detail.requestId)
                        .font(.caption.monospaced())
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                    HStack {
                        StatusCapsule(text: detail.status, positive: isPositiveStatus(detail.status))
                        Text(detail.channel)
                        if let latency = detail.latencyMs {
                            Text("· \(latency) ms")
                        }
                    }
                    .font(.caption)
                }
                Spacer()
            }

            Picker("详情", selection: $section) {
                ForEach(DetailSection.allCases) { section in
                    Text(section.rawValue).tag(section)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()

            if section == .summary {
                summaryList
            } else {
                // SSE streams and error text stay plain; only the JSON-like
                // sections pay for per-line syntax coloring.
                SearchableDetailTextView(
                    text: selectedText,
                    syntaxHighlighting: section == .request
                        || section == .upstream
                        || section == .response
                )
            }
        }
        .padding(18)
    }

    private var summaryList: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                ForEach(Array(summaryRows.enumerated()), id: \.offset) { index, row in
                    if index > 0 { Divider() }
                    HStack(alignment: .firstTextBaseline, spacing: 12) {
                        Text(row.0)
                            .foregroundStyle(.secondary)
                            .frame(width: 110, alignment: .leading)
                        Text(row.1)
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .font(.callout)
                    .padding(.vertical, 7)
                }
            }
            .padding(12)
        }
        .scrollIndicators(.never)
        .background(Color(nsColor: .textBackgroundColor), in: RoundedRectangle(cornerRadius: 9))
    }

    private var summaryRows: [(String, String)] {
        var rows: [(String, String)] = [
            ("模型", detail.modelId),
            ("请求 ID", detail.requestId),
            ("状态", detail.status),
            ("渠道", detail.channel),
            ("协议", gatewayProtocolDisplayName(detail.providerType, compatibility: nil)),
            ("流式", detail.stream ? "是" : "否"),
        ]
        rows.append(("输入 tokens", detail.inputTokens.map(formatGroupedInt) ?? "未记录"))
        rows.append(("输出 tokens", detail.outputTokens.map(formatGroupedInt) ?? "未记录"))
        rows.append(("总 tokens", detail.totalTokens.map(formatGroupedInt) ?? "未记录"))
        rows.append((
            "读缓存",
            readCacheSummary(tokens: detail.readCacheTokens, hitRate: detail.readCacheHitRate)
        ))
        rows.append((
            "写缓存",
            writeCacheSummary(
                tokens: detail.writeCacheTokens,
                fiveMinuteTokens: detail.writeCache5mTokens,
                oneHourTokens: detail.writeCache1hTokens
            )
        ))
        if let cost = detail.costUsd {
            rows.append(("费用", String(format: "$%.6f", cost)))
        }
        rows.append(("耗时", detail.latencyMs.map { "\($0) ms" } ?? "未记录"))
        rows.append(("TTFT", detail.ttftMs.map { "\($0) ms" } ?? "未记录"))
        rows.append(("创建时间", detail.createdAt))
        rows.append(("请求大小", detail.upstreamRequestBodyBytes.map(formatByteCount) ?? "未记录"))
        return rows
    }

    private var selectedText: String {
        let parts: [String?]
        switch section {
        case .summary:
            parts = []
        case .request:
            parts = [detail.requestHeadersJson, detail.requestJson]
        case .upstream:
            parts = [detail.upstreamRequestHeadersJson, detail.upstreamRequestJson]
        case .stream:
            parts = [detail.upstreamResponseSse]
        case .response:
            parts = [detail.responseJson]
        case .error:
            let error = detail.errorMessage?.trimmingCharacters(in: .whitespacesAndNewlines)
            return (error?.isEmpty == false) ? error! : "该请求没有记录错误信息。"
        }
        let text = parts.compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: "\n\n")
        return text.isEmpty ? "当前请求没有记录这一部分。请确认“记录请求与响应详情”已开启。" : text
    }
}

/// Monospaced detail text with an in-pane find bar, a line-number gutter,
/// and optional per-line JSON syntax coloring. Lines render separately in a
/// lazy stack so only visible lines pay the attributed-string cost, and
/// `ScrollViewReader` can jump straight to the current match.
private struct SearchableDetailTextView: View {
    /// Lines longer than this render without syntax coloring so a single
    /// compact multi-hundred-KB JSON line cannot stall the scanner.
    static let maxHighlightableLineLength = 2_000
    /// Above this total size the whole document falls back to plain text
    /// (the line-number gutter stays).
    static let maxHighlightableTextBytes = 2 * 1_024 * 1_024

    let text: String
    var syntaxHighlighting = false
    @State private var lines: [String] = []
    @State private var withinHighlightBudget = true
    @State private var query = ""
    @State private var matchCursor = 0

    private var trimmedQuery: String {
        query.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var matchLines: [Int] {
        guard !trimmedQuery.isEmpty else { return [] }
        return lines.indices.filter {
            lines[$0].range(of: trimmedQuery, options: .caseInsensitive) != nil
        }
    }

    private var colorizesJSON: Bool {
        syntaxHighlighting && withinHighlightBudget
    }

    private var lineNumberDigits: Int {
        max(String(lines.count).count, 2)
    }

    var body: some View {
        let matches = matchLines
        let currentLine = matches.isEmpty ? nil : matches[min(matchCursor, matches.count - 1)]

        VStack(spacing: 6) {
            HStack(spacing: 8) {
                TextField("在详情中查找", text: $query)
                    .textFieldStyle(.roundedBorder)
                    .accessibilityLabel("在详情中查找")
                if !trimmedQuery.isEmpty {
                    Text(matches.isEmpty ? "无匹配" : "\(min(matchCursor, matches.count - 1) + 1)/\(matches.count)")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .monospacedDigit()
                }
                Button {
                    step(-1, count: matches.count)
                } label: {
                    Image(systemName: "chevron.up")
                }
                .disabled(matches.count < 2)
                .help("上一个匹配")
                .accessibilityLabel("上一个匹配")
                Button {
                    step(1, count: matches.count)
                } label: {
                    Image(systemName: "chevron.down")
                }
                .disabled(matches.count < 2)
                .help("下一个匹配")
                .accessibilityLabel("下一个匹配")
                Spacer()
                Button {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(text, forType: .string)
                } label: {
                    Label("复制", systemImage: "doc.on.doc")
                }
                .buttonStyle(.link)
            }

            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(lines.indices, id: \.self) { index in
                            lineView(index, isCurrent: index == currentLine)
                                .id(index)
                        }
                    }
                    .padding(12)
                    .frame(maxWidth: .infinity, alignment: .topLeading)
                }
                .scrollIndicators(.never)
                .background(Color(nsColor: .textBackgroundColor), in: RoundedRectangle(cornerRadius: 9))
                .onChange(of: currentLine) { line in
                    guard let line else { return }
                    proxy.scrollTo(line, anchor: .center)
                }
            }
        }
        .onAppear { refreshLines() }
        .onChange(of: text) { _ in
            refreshLines()
            matchCursor = 0
        }
        .onChange(of: query) { _ in matchCursor = 0 }
    }

    private func refreshLines() {
        lines = text.components(separatedBy: "\n")
        withinHighlightBudget = text.utf8.count <= Self.maxHighlightableTextBytes
    }

    private func step(_ delta: Int, count: Int) {
        guard count > 0 else { return }
        matchCursor = (min(matchCursor, count - 1) + delta + count) % count
    }

    /// The gutter is a separate non-selectable text column, so selecting and
    /// copying content lines never picks up line numbers.
    @ViewBuilder
    private func lineView(_ index: Int, isCurrent: Bool) -> some View {
        let line = lines[index]
        HStack(alignment: .top, spacing: 10) {
            Text(paddedLineNumber(index))
                .foregroundStyle(.tertiary)
                .textSelection(.disabled)
                .accessibilityHidden(true)
            contentText(line, isCurrent: isCurrent)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .font(.system(.caption, design: .monospaced))
    }

    /// Space-padded within the monospaced font, which right-aligns the
    /// numbers without per-row width measurement.
    private func paddedLineNumber(_ index: Int) -> String {
        let number = String(index + 1)
        return String(repeating: " ", count: max(lineNumberDigits - number.count, 0)) + number
    }

    @ViewBuilder
    private func contentText(_ line: String, isCurrent: Bool) -> some View {
        let matchesQuery = !trimmedQuery.isEmpty
            && line.range(of: trimmedQuery, options: .caseInsensitive) != nil
        let colorized = colorizesJSON && line.count <= Self.maxHighlightableLineLength
        if colorized || matchesQuery {
            Text(attributedLine(line, colorized: colorized, matchesQuery: matchesQuery, isCurrent: isCurrent))
        } else {
            Text(line.isEmpty ? " " : line)
        }
    }

    private func attributedLine(
        _ line: String,
        colorized: Bool,
        matchesQuery: Bool,
        isCurrent: Bool
    ) -> AttributedString {
        var attributed = colorized
            ? jsonColoredLine(line)
            : AttributedString(line.isEmpty ? " " : line)
        // The find highlight is applied after syntax coloring so its
        // background always wins over token foreground colors.
        if matchesQuery {
            var searchRange = line.startIndex..<line.endIndex
            while let found = line.range(of: trimmedQuery, options: .caseInsensitive, range: searchRange) {
                if let attributedRange = Range(found, in: attributed) {
                    attributed[attributedRange].backgroundColor = isCurrent
                        ? Color.orange.opacity(0.55)
                        : Color.yellow.opacity(0.35)
                }
                guard found.upperBound < line.endIndex else { break }
                searchRange = found.upperBound..<line.endIndex
            }
        }
        return attributed
    }
}

/// Lightweight single-line JSON colorizer. A hand-rolled scanner (no regex)
/// keeps per-line cost linear; system semantic colors stay readable in both
/// light and dark appearance.
func jsonColoredLine(_ line: String) -> AttributedString {
    guard !line.isEmpty else { return AttributedString(" ") }
    var attributed = AttributedString(line)
    let keyColor = Color(nsColor: .systemBlue)
    let stringColor = Color(nsColor: .systemRed)
    let numberColor = Color(nsColor: .systemPurple)
    let keywordColor = Color(nsColor: .systemOrange)
    let punctuationColor = Color(nsColor: .secondaryLabelColor)

    func colorize(_ range: Range<String.Index>, _ color: Color) {
        if let attributedRange = Range(range, in: attributed) {
            attributed[attributedRange].foregroundColor = color
        }
    }

    var index = line.startIndex
    while index < line.endIndex {
        let character = line[index]
        if character == "\"" {
            // Scan the string literal, honoring backslash escapes.
            var cursor = line.index(after: index)
            var closed = false
            while cursor < line.endIndex {
                let inner = line[cursor]
                if inner == "\\" {
                    cursor = line.index(after: cursor)
                    if cursor < line.endIndex {
                        cursor = line.index(after: cursor)
                    }
                    continue
                }
                if inner == "\"" {
                    closed = true
                    break
                }
                cursor = line.index(after: cursor)
            }
            let literalEnd = closed ? line.index(after: cursor) : line.endIndex
            // A literal directly followed by (optional space and) a colon is
            // an object key.
            var probe = literalEnd
            while probe < line.endIndex, line[probe] == " " || line[probe] == "\t" {
                probe = line.index(after: probe)
            }
            let isKey = probe < line.endIndex && line[probe] == ":"
            colorize(index..<literalEnd, isKey ? keyColor : stringColor)
            index = literalEnd
            continue
        }
        if character.isNumber || (character == "-" && hasDigit(line, after: index)) {
            var cursor = line.index(after: index)
            while cursor < line.endIndex, isNumberBody(line[cursor]) {
                cursor = line.index(after: cursor)
            }
            colorize(index..<cursor, numberColor)
            index = cursor
            continue
        }
        if character.isLetter {
            var cursor = line.index(after: index)
            while cursor < line.endIndex, line[cursor].isLetter {
                cursor = line.index(after: cursor)
            }
            let word = line[index..<cursor]
            if word == "true" || word == "false" || word == "null" {
                colorize(index..<cursor, keywordColor)
            }
            index = cursor
            continue
        }
        if "{}[],:".contains(character) {
            colorize(index..<line.index(after: index), punctuationColor)
        }
        index = line.index(after: index)
    }
    return attributed
}

private func hasDigit(_ line: String, after index: String.Index) -> Bool {
    let next = line.index(after: index)
    return next < line.endIndex && line[next].isNumber
}

private func isNumberBody(_ character: Character) -> Bool {
    character.isNumber || character == "." || character == "e" || character == "E"
        || character == "+" || character == "-"
}

private struct ManagementScrollPage<Content: View>: View {
    let maxContentWidth: CGFloat
    let loading: Bool
    let error: String?
    let retry: () -> Void
    @ViewBuilder let content: Content

    init(
        maxContentWidth: CGFloat = ThreadRelayPageLayout.maxContentWidth,
        loading: Bool,
        error: String?,
        retry: @escaping () -> Void,
        @ViewBuilder content: () -> Content
    ) {
        self.maxContentWidth = maxContentWidth
        self.loading = loading
        self.error = error
        self.retry = retry
        self.content = content()
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: ThreadRelayPageLayout.sectionSpacing) {
                if let error {
                    InlineManagementError(message: error, retry: retry)
                }
                content
            }
            .frame(maxWidth: maxContentWidth, alignment: .leading)
            .padding(.horizontal, ThreadRelayPageLayout.horizontalPadding)
            .padding(.top, ThreadRelayPageLayout.topPadding)
            .padding(.bottom, ThreadRelayPageLayout.bottomPadding)
        }
        .overlay {
            if loading {
                ProgressView()
                    .controlSize(.large)
                    .padding(18)
                    .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12))
            }
        }
    }
}

struct ManagementCard<Content: View>: View {
    let title: String
    @ViewBuilder let content: Content

    init(title: String, @ViewBuilder content: () -> Content) {
        self.title = title
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title)
                .font(.headline)

            SettingsGroupSurface {
                content
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct InlineManagementError: View {
    let message: String
    let retry: () -> Void
    var dismiss: (() -> Void)?

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(.orange)
            Text(message)
                .font(.callout)
            Spacer()
            Button("重试", action: retry)
            if let dismiss {
                Button {
                    dismiss()
                } label: {
                    Image(systemName: "xmark")
                }
                .buttonStyle(.plain)
                .foregroundStyle(.secondary)
                .help("关闭提示")
                .accessibilityLabel("关闭错误提示")
            }
        }
        .padding(12)
        .background(Color.orange.opacity(0.1), in: RoundedRectangle(cornerRadius: 10))
    }
}

private struct ManagementEmptyState: View {
    let title: String
    let message: String
    let symbol: String

    var body: some View {
        VStack(spacing: 9) {
            Image(systemName: symbol)
                .font(.system(size: 28))
                .foregroundStyle(.secondary)
            Text(title)
                .font(.headline)
            Text(message)
                .font(.callout)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 420)
        }
        .padding(28)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

private struct StatusCapsule: View {
    let text: String
    let positive: Bool

    var body: some View {
        Text(text)
            .font(.caption.weight(.medium))
            .foregroundStyle(positive ? Color.green : Color.orange)
            .padding(.horizontal, 7)
            .padding(.vertical, 3)
            .background(
                (positive ? Color.green : Color.orange).opacity(0.11),
                in: Capsule()
            )
    }
}

func splitValues(_ value: String) -> [String] {
    value
        .components(separatedBy: CharacterSet(charactersIn: ",\n"))
        .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
        .filter { !$0.isEmpty }
}

private func nilIfEmpty(_ value: String) -> String? {
    let value = value.trimmingCharacters(in: .whitespacesAndNewlines)
    return value.isEmpty ? nil : value
}

/// User-facing copy for the normalized balance and billing statuses returned
/// by the daemon. Balance and billing render independently for partial results.
func providerUsageStatusText(_ status: String) -> String {
    switch status {
    case "available": "可用"
    case "unsupported": "服务商不支持查询"
    case "unauthorized": "API Key 未授权"
    case "forbidden": "无权查询"
    case "temporarily_unavailable": "暂时不可用"
    case "invalid_response": "响应格式异常"
    default: "状态未知"
    }
}

func providerUsageBillingText(_ usage: ManageProviderUsageResponse.Usage) -> String {
    if usage.source == "new_api", usage.billingStatus == "unsupported" {
        return "未提供"
    }
    return providerUsageStatusText(usage.billingStatus)
}

func providerUsageBalanceText(_ usage: ManageProviderUsageResponse.Usage) -> String {
    if usage.unlimited { return "无限" }
    if let remaining = usage.remaining, remaining.isFinite {
        let value = providerUsageDecimalText(remaining)
        guard let unit = usage.unit.flatMap(nilIfEmpty) else { return value }
        return "\(value) \(unit)"
    }
    if usage.balanceStatus == "available" {
        return "可用（未返回余额）"
    }
    return providerUsageStatusText(usage.balanceStatus)
}

func providerUsageAccountWarning(_ status: String?) -> String? {
    switch status {
    case "disabled": "API Key 已停用。"
    case "inactive": "API Key 已停用。"
    case "quota_exhausted": "API Key 额度已耗尽。"
    case "expired": "API Key 已过期。"
    default: nil
    }
}

func providerUsageMultiplierText(_ value: Double?) -> String? {
    guard let value, value.isFinite else { return nil }
    return "×\(providerUsageDecimalText(value))"
}

func providerUsageRateComponentsText(_ usage: ManageProviderUsageResponse.Usage) -> String? {
    var parts: [String] = []
    if let group = providerUsageMultiplierText(usage.groupRateMultiplier) {
        parts.append("分组 \(group)")
    }
    if let user = providerUsageMultiplierText(usage.userRateMultiplier) {
        parts.append("用户 \(user)")
    }
    return parts.isEmpty ? nil : parts.joined(separator: " · ")
}

func providerUsagePeakText(_ usage: ManageProviderUsageResponse.Usage) -> String {
    var parts: [String] = []
    if let peak = providerUsageMultiplierText(usage.peakRateMultiplier) {
        parts.append("峰时 \(peak)")
    }
    if let applied = providerUsageMultiplierText(usage.appliedPeakMultiplier) {
        parts.append("当前 \(applied)")
    }
    if let start = usage.peakStart.flatMap(nilIfEmpty),
       let end = usage.peakEnd.flatMap(nilIfEmpty)
    {
        parts.append("\(start)–\(end)")
    }
    if let timezone = usage.timezone.flatMap(nilIfEmpty) {
        parts.append(timezone)
    }
    return parts.isEmpty ? "已启用" : parts.joined(separator: " · ")
}

private func providerUsageDecimalText(_ value: Double) -> String {
    var text = String(format: "%.4f", value)
    while text.last == "0" { text.removeLast() }
    if text.last == "." { text.removeLast() }
    return text
}

/// Thousands grouping aligned with the legacy GUI's `format_int`.
func formatGroupedInt(_ value: Int64) -> String {
    let digits = String(value.magnitude)
    var grouped = ""
    for (index, character) in digits.reversed().enumerated() {
        if index > 0, index % 3 == 0 { grouped.append(",") }
        grouped.append(character)
    }
    var result = String(grouped.reversed())
    if value < 0 { result.insert("-", at: result.startIndex) }
    return result
}

/// Byte formatting aligned with the legacy GUI's `format_bytes`: two
/// decimals from 1 MB, one decimal from 1 KB, raw bytes below.
func formatByteCount(_ bytes: Int64) -> String {
    if bytes >= 1_048_576 {
        return String(format: "%.2f MB", Double(bytes) / 1_048_576)
    }
    if bytes >= 1024 {
        return String(format: "%.1f KB", Double(bytes) / 1024)
    }
    return "\(bytes) B"
}

/// Read-cache summary row: `N tokens(X.X%)`; the hit rate arrives as a
/// 0...1 fraction and is only appended when recorded.
func readCacheSummary(tokens: Int64?, hitRate: Double?) -> String {
    guard let tokens else { return "未记录" }
    var text = "\(formatGroupedInt(tokens)) tokens"
    if let hitRate {
        text += String(format: "(%.1f%%)", hitRate * 100)
    }
    return text
}

/// Write-cache summary row: `N tokens [5m N, 1h N]`. The TTL split is
/// annotated only when the upstream reported either tier, mirroring the
/// legacy `format_write_cache`.
func writeCacheSummary(
    tokens: Int64?,
    fiveMinuteTokens: Int64?,
    oneHourTokens: Int64?
) -> String {
    guard let tokens else { return "未记录" }
    var text = "\(formatGroupedInt(tokens)) tokens"
    let fiveMinute = fiveMinuteTokens ?? 0
    let oneHour = oneHourTokens ?? 0
    if fiveMinute > 0 || oneHour > 0 {
        text += " [5m \(formatGroupedInt(fiveMinute)), 1h \(formatGroupedInt(oneHour))]"
    }
    return text
}

/// Canonical Codex alias for a Claude-series upstream model, mirroring the
/// legacy GUI's `inferred_model_alias_key` (`src/gui.rs`).
func inferredModelAliasKey(_ model: String) -> String? {
    switch model.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() {
    case "claude-opus-4-8": "opus-4.8"
    case "claude-sonnet-4-6": "sonnet-4.6"
    default: nil
    }
}

/// Aliases inferred from the model list. An alias is only generated when no
/// model in the list already uses the alias name itself.
func inferredModelAliases(models: [String]) -> [String: String] {
    var aliases: [String: String] = [:]
    for model in models {
        guard let canonical = inferredModelAliasKey(model) else { continue }
        if models.allSatisfy({ $0 != canonical }) {
            aliases[canonical] = model
        }
    }
    return aliases
}

/// Explicit user mappings win over inferred ones, mirroring the legacy
/// `build_model_aliases_for_save`.
func mergedModelAliases(
    models: [String],
    explicit: [String: String]
) -> [String: String] {
    var merged = explicit
    for (alias, target) in inferredModelAliases(models: models) where merged[alias] == nil {
        merged[alias] = target
    }
    return merged
}

/// Merges fetched upstream models into the free-form model text: existing
/// entries keep their order, duplicates collapse, and new models append.
func mergedModelLines(existing: String, fetched: [String]) -> String {
    var seen = Set<String>()
    var lines: [String] = []
    for entry in splitValues(existing) where seen.insert(entry).inserted {
        lines.append(entry)
    }
    for entry in fetched {
        let trimmed = entry.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { continue }
        if seen.insert(trimmed).inserted { lines.append(trimmed) }
    }
    return lines.joined(separator: "\n")
}

/// One display line per fetch attempt: `URL — 状态码或错误 — preview`, with
/// the preview truncated to 120 characters and at most four attempts shown.
func providerFetchAttemptLines(
    _ attempts: [ManageProviderModelsFetchResponse.Attempt],
    limit: Int = 4
) -> [String] {
    attempts.prefix(limit).map { attempt in
        var parts = [attempt.url]
        if let status = attempt.status {
            parts.append("HTTP \(status)")
        } else if let error = attempt.error, !error.isEmpty {
            parts.append(error)
        } else {
            parts.append("无响应")
        }
        if let preview = attempt.preview?.trimmingCharacters(in: .whitespacesAndNewlines),
           !preview.isEmpty {
            parts.append(String(preview.prefix(120)))
        }
        return parts.joined(separator: " — ")
    }
}

private func relativeDate(seconds: Int64) -> String {
    relativeDate(Date(timeIntervalSince1970: TimeInterval(seconds)))
}

private func relativeDate(milliseconds: Int64) -> String {
    relativeDate(Date(timeIntervalSince1970: TimeInterval(milliseconds) / 1_000))
}

private func relativeDate(_ date: Date) -> String {
    return RelativeDateTimeFormatter().localizedString(for: date, relativeTo: Date())
}

private func isPositiveStatus(_ status: String) -> Bool {
    let normalized = status.lowercased()
    return normalized == "success"
        || normalized == "completed"
        || normalized == "ok"
        || normalized.hasPrefix("2")
}
