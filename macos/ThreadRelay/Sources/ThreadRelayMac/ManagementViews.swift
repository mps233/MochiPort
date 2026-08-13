import AppKit
import SwiftUI

struct CodexAccessView: View {
    @EnvironmentObject private var model: AppModel
    @State private var confirmsUninstall = false
    @State private var showsEnhancedWait = false
    @State private var enhancedLaunching = false

    var body: some View {
        ManagementScrollPage(
            title: "Codex 接入",
            subtitle: "检查并维护 Codex App 与本地 ThreadRelay 的连接。",
            symbol: "chevron.left.forwardslash.chevron.right",
            loading: model.isLoading(.codex),
            error: model.sectionErrors[.codex],
            retry: { Task { await model.loadSection(.codex, force: true) } }
        ) {
            if let status = model.codexStatus {
                statusContent(status)
            } else if !model.isLoading(.codex), model.sectionErrors[.codex] == nil {
                ManagementEmptyState(
                    title: "尚未读取 Codex 状态",
                    message: "刷新后会显示配置、授权和远程控制状态。",
                    symbol: "app.badge.checkmark"
                )
            }
        }
        .task { await model.loadSection(.codex) }
        .alert("卸载 Codex 接入？", isPresented: $confirmsUninstall) {
            Button("取消", role: .cancel) {}
            Button("卸载", role: .destructive) {
                Task { await model.uninstallCodex() }
            }
        } message: {
            Text("只移除 ThreadRelay 管理的 Codex 配置，并保留其他用户配置。")
        }
        .sheet(isPresented: $showsEnhancedWait) {
            EnhancedLaunchWaitSheet(
                checkRunning: { await model.checkCodexAppRunning() },
                onReady: {
                    showsEnhancedWait = false
                    startEnhancedLaunch()
                },
                onCancel: { showsEnhancedWait = false }
            )
        }
    }

    /// Enhanced launch requires Codex App to be closed first; when it is
    /// still running, show the waiting sheet that polls the preflight until
    /// the app exits, then continues automatically.
    private func startEnhancedLaunchFlow() {
        guard !enhancedLaunching else { return }
        if model.codexPreflight?.status.running == true {
            showsEnhancedWait = true
        } else {
            startEnhancedLaunch()
        }
    }

    private func startEnhancedLaunch() {
        guard !enhancedLaunching else { return }
        enhancedLaunching = true
        Task {
            _ = await model.launchCodexEnhanced()
            enhancedLaunching = false
        }
    }

    @ViewBuilder
    private func statusContent(_ status: ManageCodexStatus) -> some View {
        ManagementCard(title: "接入状态", symbol: "checkmark.shield") {
            ManagementStatusRow(
                title: "整体配置",
                detail: status.configured ? "已就绪" : "需要处理",
                ready: status.configured
            )
            Divider()
            ManagementStatusRow(
                title: "配置文件",
                detail: status.configError ?? (status.configOk ? "有效" : "未配置"),
                ready: status.configOk
            )
            Divider()
            ManagementStatusRow(
                title: "本地授权",
                detail: status.authError ?? (status.authOk ? "有效" : "未配置"),
                ready: status.authOk
            )
            Divider()
            ManagementStatusRow(
                title: "GUI 环境",
                detail: status.guiError ?? (status.guiConfigured ? "已连接" : "需要修复"),
                ready: status.guiConfigured
            )
            Divider()
            ManagementStatusRow(
                title: "远程控制",
                detail: status.remoteControlError
                    ?? (status.remoteControlConfigured ? "已开启" : "尚未开启"),
                ready: status.remoteControlConfigured
            )
            Text(status.codexHome)
                .font(.caption.monospaced())
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
                .padding(.top, 4)
        }

        ManagementCard(title: "维护操作", symbol: "wrench.and.screwdriver") {
            HStack(spacing: 10) {
                Button(status.configured ? "重新写入配置" : "接入 Codex App") {
                    Task { await model.configureCodex() }
                }
                .buttonStyle(.borderedProminent)

                Button("修复 GUI 环境") {
                    Task { await model.repairCodex() }
                }
                .disabled(!status.configOk || !status.authOk)

                Button("刷新模型") {
                    Task { await model.refreshCodexModels() }
                }

                Spacer()

                Button("卸载接入", role: .destructive) {
                    confirmsUninstall = true
                }
                .disabled(!status.configured && !status.configOk && !status.authOk)
            }

            Divider()

            HStack {
                VStack(alignment: .leading, spacing: 3) {
                    Text("增强启动")
                        .font(.headline)
                    Text(
                        model.codexPreflight?.status.running == true
                            ? "Codex App 正在运行；增强启动会先完成兼容预检。"
                            : "Codex App 当前未运行，可由 ThreadRelay 启动并注入可见模型。"
                    )
                    .font(.caption)
                    .foregroundStyle(.secondary)
                }
                Spacer()
                Button {
                    startEnhancedLaunchFlow()
                } label: {
                    if enhancedLaunching {
                        HStack(spacing: 7) {
                            ProgressView()
                                .controlSize(.small)
                            Text("正在启动…")
                        }
                    } else {
                        Text("增强启动")
                    }
                }
                .disabled(!status.configured || enhancedLaunching)
                .accessibilityLabel("增强启动 Codex App")
            }

            VStack(alignment: .leading, spacing: 6) {
                Text("与普通启动相比，增强启动会：")
                    .font(.caption.weight(.medium))
                    .foregroundStyle(.secondary)
                EnhancedLaunchFeatureRow(
                    symbol: "checkmark.seal",
                    text: "启动前预检 Codex App 进程，先备份配置快照，并把请求指向本地 ThreadRelay 服务。"
                )
                EnhancedLaunchFeatureRow(
                    symbol: "bolt",
                    text: "以调试模式启动 Codex App，在界面加载前注入增强脚本，并持续校验直到全部生效。"
                )
                EnhancedLaunchFeatureRow(
                    symbol: "eye",
                    text: "让「Codex 可见模型」中保存的 AI 网关自定义模型出现在 Codex 的模型选择器里。"
                )
                EnhancedLaunchFeatureRow(
                    symbol: "puzzlepiece.extension",
                    text: "启用中文界面与插件目录兼容层，并在官方初始化不可用时切换到本地回退。"
                )
            }
        }

        ManagementCard(title: "Codex Provider", symbol: "server.rack") {
            if status.providers.isEmpty {
                ManagementEmptyState(
                    title: "尚未配置 Provider",
                    message: "接入 Codex App 后会在这里显示写入的 Provider。",
                    symbol: "server.rack"
                )
                .frame(minHeight: 120)
            } else {
                ForEach(Array(status.providers.enumerated()), id: \.element.id) { index, provider in
                    if index > 0 { Divider() }
                    HStack(spacing: 12) {
                        Image(systemName: provider.secretSet ? "key.fill" : "key.slash")
                            .foregroundStyle(provider.secretSet ? Color.green : Color.orange)
                        VStack(alignment: .leading, spacing: 3) {
                            Text(provider.name)
                                .font(.headline)
                            Text(provider.baseUrl ?? "未设置 Base URL")
                                .font(.caption.monospaced())
                                .foregroundStyle(.secondary)
                        }
                        Spacer()
                        if provider.supportsWebsockets {
                            Label("WebSocket", systemImage: "bolt.horizontal")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                }
            }
        }
    }
}

/// Modal shown when enhanced launch is requested while Codex App is still
/// running. Polls the preflight once per second; a poll failure counts as
/// "still running" so the flow never continues on stale data.
private struct EnhancedLaunchWaitSheet: View {
    let checkRunning: () async -> Bool?
    let onReady: () -> Void
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
        .task {
            while !Task.isCancelled {
                if await checkRunning() == false {
                    onReady()
                    return
                }
                try? await Task.sleep(for: .seconds(1))
            }
        }
        .accessibilityIdentifier("codex.enhanced-wait-sheet")
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

struct SessionsView: View {
    @EnvironmentObject private var model: AppModel
    @State private var query = ""
    @State private var selectedIDs = Set<String>()
    @State private var moveInFlight = false

    private var filteredSessions: [ManageCodexSession] {
        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !needle.isEmpty else { return model.codexSessions }
        return model.codexSessions.filter {
            $0.displayName.lowercased().contains(needle)
                || $0.modelProvider.lowercased().contains(needle)
                || $0.id.lowercased().contains(needle)
        }
    }

    private var selectedSessions: [ManageCodexSession] {
        model.codexSessions.filter { selectedIDs.contains($0.id) }
    }

    private var headerSubtitle: String {
        let sessions = model.codexSessions
        guard !sessions.isEmpty else {
            return "查看真实 Codex 会话并在直连 Provider 与 AI Gateway 之间移动。"
        }
        let gatewayCount = sessions.filter { $0.modelProvider == "ai-gateway" }.count
        return "\(sessions.count) 个会话 · 直连 \(sessions.count - gatewayCount) · 网关 \(gatewayCount)"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ManagementPageHeader(
                title: "会话",
                subtitle: headerSubtitle,
                symbol: "text.bubble"
            )
            .padding(.horizontal, ThreadRelaySpacing.page)
            .padding(.top, ThreadRelaySpacing.page)
            .padding(.bottom, 20)

            HStack(spacing: 10) {
                TextField("搜索标题、Provider 或会话 ID", text: $query)
                    .textFieldStyle(.roundedBorder)
                if !selectedIDs.isEmpty {
                    Text("已选 \(selectedIDs.count) 项")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .monospacedDigit()
                        .accessibilityIdentifier("sessions.selection-count")
                }
                Button {
                    Task { await model.loadSection(.sessions, force: true) }
                } label: {
                    Label("刷新", systemImage: "arrow.clockwise")
                }
                .disabled(model.isLoading(.sessions))
                sessionActions
            }
            .padding(.horizontal, ThreadRelaySpacing.page)
            .padding(.bottom, 16)

            if let error = model.sectionErrors[.sessions] {
                InlineManagementError(
                    message: error,
                    retry: { Task { await model.loadSection(.sessions, force: true) } },
                    dismiss: { model.dismissSectionError(.sessions) }
                )
                .padding(.horizontal, ThreadRelaySpacing.page)
                .padding(.bottom, 12)
            }

            Group {
                if filteredSessions.isEmpty, !model.isLoading(.sessions) {
                    ManagementEmptyState(
                        title: query.isEmpty ? "没有可见会话" : "没有匹配的会话",
                        message: query.isEmpty
                            ? "Codex App 产生会话后会显示在这里。"
                            : "调整搜索词后重试。",
                        symbol: "text.bubble"
                    )
                } else {
                    sessionList
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(Color(nsColor: .controlBackgroundColor))
            .clipShape(RoundedRectangle(cornerRadius: ThreadRelayRadius.content))
            .overlay {
                RoundedRectangle(cornerRadius: ThreadRelayRadius.content)
                    .stroke(Color.primary.opacity(0.07), lineWidth: 1)
            }
            .overlay {
                if model.isLoading(.sessions), model.codexSessions.isEmpty {
                    ProgressView("正在读取会话…")
                }
            }
            .padding(.horizontal, ThreadRelaySpacing.page)
            .padding(.bottom, ThreadRelaySpacing.page)
        }
        .task { await model.loadSection(.sessions) }
    }

    @ViewBuilder
    private var sessionList: some View {
        let list = List(selection: $selectedIDs) {
            ForEach(filteredSessions) { session in
                SessionRow(session: session)
                    .tag(session.id)
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
        if #available(macOS 14.0, *) {
            list
                .listStyle(.inset)
                .alternatingRowBackgrounds(.enabled)
                .scrollIndicators(.never)
        } else {
            list
                .listStyle(.inset(alternatesRowBackgrounds: true))
                .scrollIndicators(.never)
        }
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
        count > 1 ? "移动 \(count) 项到 \(target)" : "移动到 \(target)"
    }

    private var sessionActions: some View {
        Menu {
            Button("AI Gateway") {
                moveSessions(selectedSessions, to: nil)
            }
            let providers = model.codexSessionProviders.filter { $0 != "ai-gateway" }
            if !providers.isEmpty {
                Divider()
                ForEach(providers, id: \.self) { provider in
                    Button(provider) {
                        moveSessions(selectedSessions, to: provider)
                    }
                }
            }
        } label: {
            Label("移动到…", systemImage: "arrow.left.arrow.right")
        }
        .disabled(selectedIDs.isEmpty || moveInFlight)
        .accessibilityLabel("移动选中的会话")
    }

    /// Skips sessions already on the target, then deselects the moved ones so
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

/// `backgroundProminence` only exists on macOS 14+; it lets the accent-tinted
/// provider capsule flip to white when the row sits on the selection color.
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

    private var tintColor: Color {
        if emphasized { return .white }
        return isGateway ? .accentColor : .secondary
    }

    private var tintBackground: Color {
        if emphasized { return Color.white.opacity(0.16) }
        return isGateway ? Color.accentColor.opacity(0.12) : Color.primary.opacity(0.06)
    }

    var body: some View {
        HStack(spacing: ThreadRelaySpacing.standard) {
            Image(systemName: isGateway ? "point.3.connected.trianglepath.dotted" : "bubble.left.and.text.bubble.right")
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(tintColor)
                .frame(width: 30, height: 30)
                .background(tintBackground, in: RoundedRectangle(cornerRadius: 8))
            VStack(alignment: .leading, spacing: 3) {
                Text(session.displayName)
                    .font(.body.weight(.medium))
                    .lineLimit(1)
                Text(session.modelProvider)
                    .font(.caption2.weight(.medium))
                    .foregroundStyle(tintColor)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 2)
                    .background(tintBackground, in: Capsule())
            }
            Spacer(minLength: ThreadRelaySpacing.standard)
            Text(relativeDate(session.updatedAt))
                .font(.caption)
                .foregroundStyle(.tertiary)
                .monospacedDigit()
        }
        .padding(.vertical, 6)
        .help("会话 ID：\(session.id)")
    }
}

struct GatewayView: View {
    @EnvironmentObject private var model: AppModel
    @State private var enabled = false
    @State private var filterImages = false
    @State private var requestLogging = false
    @State private var requestDetails = false
    @State private var visibleModels = ""
    @State private var settingsReady = false
    @State private var modelCatalog: [ManageCodexCatalogModel] = []
    @State private var selectedCatalogModels = Set<String>()
    @State private var editor: GatewayProviderEditorState?
    @State private var providerToDelete: ManageGatewayProvider?

    var body: some View {
        ManagementScrollPage(
            title: "AI 网关",
            subtitle: "管理真实 Provider、路由开关、可见模型与请求日志策略。",
            symbol: "point.3.connected.trianglepath.dotted",
            loading: model.isLoading(.gateway),
            error: model.sectionErrors[.gateway],
            retry: { Task { await model.loadSection(.gateway, force: true) } }
        ) {
            if let gateway = model.gateway {
                settingsCard
                providersCard(gateway.providers)
            }
        }
        .task {
            await model.loadSection(.gateway)
            // The catalog endpoint may not exist on an older daemon; an empty
            // catalog keeps the plain text editor as the only input.
            modelCatalog = await model.loadCodexModelCatalog() ?? []
            synchronizeGateway(model.gateway)
        }
        .onChange(of: model.gateway) { gateway in
            synchronizeGateway(gateway)
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
    }

    private var settingsCard: some View {
        ManagementCard(title: "网关策略", symbol: "switch.2") {
            Toggle("启用 AI Gateway", isOn: $enabled)
            Toggle("过滤图像生成工具", isOn: $filterImages)
            Toggle("记录请求摘要", isOn: $requestLogging)
            Toggle("记录请求与响应详情", isOn: $requestDetails)
                .disabled(!requestLogging)

            Divider()

            VStack(alignment: .leading, spacing: 6) {
                Text("Codex 可见模型")
                    .font(.headline)
                Text("这里只控制 Codex 模型选择器，不改变 Provider 路由。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                if !modelCatalog.isEmpty {
                    LazyVGrid(
                        columns: [GridItem(.adaptive(minimum: 250), alignment: .leading)],
                        alignment: .leading,
                        spacing: 6
                    ) {
                        ForEach(modelCatalog) { entry in
                            Toggle(isOn: catalogBinding(entry.id)) {
                                HStack(spacing: 6) {
                                    Text(entry.displayName)
                                    Text(entry.id)
                                        .font(.caption.monospaced())
                                        .foregroundStyle(.secondary)
                                        .lineLimit(1)
                                }
                            }
                            .toggleStyle(.checkbox)
                            .accessibilityLabel("可见模型 \(entry.displayName)")
                        }
                    }
                    .padding(.vertical, 2)
                    Text("自定义模型（目录之外的条目，每行一个）")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .padding(.top, 4)
                }
                TextEditor(text: $visibleModels)
                    .font(.body.monospaced())
                    .frame(minHeight: modelCatalog.isEmpty ? 90 : 60)
                    .padding(6)
                    .background(Color(nsColor: .textBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
            }

            HStack {
                Spacer()
                Button("保存网关设置") {
                    Task {
                        await model.saveGatewaySettings(
                            enabled: enabled,
                            filterImageGenerationTool: filterImages,
                            requestLoggingEnabled: requestLogging,
                            requestLogDetailsEnabled: requestDetails,
                            codexVisibleModels: mergedVisibleModels
                        )
                    }
                }
                .buttonStyle(.borderedProminent)
                .disabled(!settingsReady || model.isLoading(.gateway))
            }
        }
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
        ManagementCard(title: "Provider", symbol: "server.rack") {
            HStack {
                Text("\(providers.count) 个上游")
                    .foregroundStyle(.secondary)
                Spacer()
                Button {
                    editor = GatewayProviderEditorState(provider: nil)
                } label: {
                    Label("添加 Provider", systemImage: "plus")
                }
            }

            if providers.isEmpty {
                ManagementEmptyState(
                    title: "尚未添加 Provider",
                    message: "添加上游协议、Base URL、模型和只写 API Key 后即可开始路由。",
                    symbol: "server.rack"
                )
                .frame(minHeight: 150)
            } else {
                ForEach(Array(providers.enumerated()), id: \.element.id) { index, provider in
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
        HStack(spacing: 12) {
            Circle()
                .fill(localEnabled ? Color.green : Color.secondary.opacity(0.4))
                .frame(width: 8, height: 8)
            ProviderLogoView(
                providerType: provider.providerType,
                compatibility: provider.compatibility,
                providerName: provider.name,
                size: 20
            )
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 8) {
                    Text(provider.name)
                        .font(.headline)
                    Text(gatewayProtocolDisplayName(provider.providerType, compatibility: provider.compatibility))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Text("\(provider.models.count) 个模型 · 权重 \(provider.weight) · 超时 \(provider.timeoutSecs) 秒")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Text(provider.baseUrl)
                    .font(.caption.monospaced())
                    .foregroundStyle(.tertiary)
                    .lineLimit(1)
            }
            Spacer()
            Image(systemName: provider.secretSet ? "key.fill" : "key.slash")
                .foregroundStyle(provider.secretSet ? Color.green : Color.orange)
                .help(provider.secretSet ? "已设置 API Key" : "未设置 API Key")
            Toggle("", isOn: toggleBinding)
                .toggleStyle(.switch)
                .controlSize(.small)
                .labelsHidden()
                .disabled(toggleInFlight)
                .help(localEnabled ? "停用 Provider" : "启用 Provider")
                .accessibilityLabel("启用 Provider \(provider.name)")
            Button("编辑", action: onEdit)
            Button {
                onDelete()
            } label: {
                Image(systemName: "trash")
            }
            .buttonStyle(.plain)
            .foregroundStyle(.red)
            .help("删除 Provider")
        }
        .padding(.vertical, 4)
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
    @State private var fetchModelsFailureLines: [String] = []

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
                TextField("Prompt Cache Retention（可选）", text: $promptCacheRetention)
                Stepper("权重：\(weight)", value: $weight, in: 1...10_000)
                Stepper("超时：\(timeoutSecs) 秒", value: $timeoutSecs, in: 1...3_600)
                VStack(alignment: .leading, spacing: 6) {
                    HStack(alignment: .firstTextBaseline) {
                        Text("模型（每行一个）")
                        Spacer()
                        if let fetchModelsNotice {
                            Text(fetchModelsNotice)
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                        Button {
                            fetchModelsFromUpstream()
                        } label: {
                            if fetchingModels {
                                HStack(spacing: 6) {
                                    ProgressView()
                                        .controlSize(.small)
                                    Text("正在获取…")
                                }
                            } else {
                                Label("从上游获取", systemImage: "arrow.down.circle")
                            }
                        }
                        .disabled(
                            fetchingModels
                                || baseURL.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                        )
                        .accessibilityLabel("从上游获取模型列表")
                    }
                    TextEditor(text: $models)
                        .font(.body.monospaced())
                        .frame(minHeight: 90)
                        .padding(5)
                        .background(Color(nsColor: .textBackgroundColor), in: RoundedRectangle(cornerRadius: 7))
                    if !fetchModelsFailureLines.isEmpty {
                        VStack(alignment: .leading, spacing: 3) {
                            Text("获取模型失败，各地址尝试结果：")
                                .font(.caption.weight(.medium))
                                .foregroundStyle(.red)
                            ForEach(Array(fetchModelsFailureLines.enumerated()), id: \.offset) { _, line in
                                Text(line)
                                    .font(.caption.monospaced())
                                    .foregroundStyle(.secondary)
                                    .lineLimit(2)
                                    .textSelection(.enabled)
                            }
                        }
                    }
                }
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
        .frame(width: 620, height: 640)
        .task {
            // Templates only make sense when creating a provider; failures
            // (including older daemons without the endpoint) silently keep
            // the plain form.
            guard state.provider == nil else { return }
            templates = await model.loadGatewayProviderTemplates() ?? []
        }
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

    /// Asks the daemon to list the upstream's models with the form's current
    /// values. When editing an existing provider the stored API key is reused
    /// unless the user typed a new one in this session.
    private func fetchModelsFromUpstream() {
        guard !fetchingModels else { return }
        fetchingModels = true
        fetchModelsNotice = nil
        fetchModelsFailureLines = []
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
                    models = mergedModelLines(existing: models, fetched: response.models)
                    fetchModelsNotice = "获取到 \(response.models.count) 个模型"
                } else if response.attempts.isEmpty {
                    fetchModelsFailureLines = ["上游未返回模型列表。"]
                } else {
                    fetchModelsFailureLines = providerFetchAttemptLines(response.attempts)
                }
            } catch let error as APIClientError {
                fetchModelsFailureLines = [error.localizedDescription]
            } catch {
                fetchModelsFailureLines = ["无法连接本地服务。"]
            }
            fetchingModels = false
        }
    }

    private func save() {
        saving = true
        let modelList = splitValues(models)
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

struct RequestLogsView: View {
    @EnvironmentObject private var model: AppModel
    @State private var query = ""
    @State private var selectedID: Int64?
    @State private var confirmsClear = false
    @State private var confirmsClearOld = false
    @State private var clearing = false
    @State private var detailTask: Task<Void, Never>?

    private var filteredLogs: [ManageRequestLog] {
        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !needle.isEmpty else { return model.requestLogs }
        return model.requestLogs.filter {
            $0.requestId.lowercased().contains(needle)
                || $0.modelId.lowercased().contains(needle)
                || $0.channel.lowercased().contains(needle)
                || $0.status.lowercased().contains(needle)
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ManagementPageHeader(
                title: "请求日志",
                subtitle: "查看 AI Gateway 的真实请求摘要和按需加载的脱敏详情。",
                symbol: "list.bullet.rectangle"
            )
            .padding(.horizontal, ThreadRelaySpacing.page)
            .padding(.top, ThreadRelaySpacing.page)
            .padding(.bottom, 20)

            HStack(spacing: 10) {
                TextField("搜索请求 ID、模型、Provider 或状态", text: $query)
                    .textFieldStyle(.roundedBorder)
                Button {
                    Task { await model.loadSection(.requestLogs, force: true) }
                } label: {
                    Label("刷新", systemImage: "arrow.clockwise")
                }
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
                        Text("清理")
                    }
                }
                .fixedSize()
                .disabled(model.requestLogs.isEmpty || clearing)
                .accessibilityLabel("清理请求日志")
            }
            .padding(.horizontal, 28)
            .padding(.bottom, 16)

            if let error = model.sectionErrors[.requestLogs] {
                InlineManagementError(
                    message: error,
                    retry: { Task { await model.loadSection(.requestLogs, force: true) } },
                    dismiss: { model.dismissSectionError(.requestLogs) }
                )
                .padding(.horizontal, 28)
                .padding(.bottom, 12)
            }

            HSplitView {
                Group {
                    if filteredLogs.isEmpty, !model.isLoading(.requestLogs) {
                        ManagementEmptyState(
                            title: query.isEmpty ? "没有请求日志" : "没有匹配的请求",
                            message: query.isEmpty
                                ? "在 AI 网关中开启请求日志后，新请求会显示在这里。"
                                : "调整搜索词后重试。",
                            symbol: "list.bullet.rectangle"
                        )
                    } else {
                        List(selection: $selectedID) {
                            ForEach(filteredLogs) { log in
                                RequestLogRow(log: log)
                                    .tag(log.id)
                            }
                        }
                        .scrollIndicators(.never)
                    }
                }
                // Keep the pane minimums small enough that the app sidebar
                // (190pt) plus both panes still fit the 760pt minimum window,
                // otherwise the split view crushes the sidebar out of view.
                .frame(minWidth: 250, idealWidth: 380)

                RequestLogDetailPane(detail: model.requestLogDetail)
                    .frame(minWidth: 290, maxWidth: .infinity)
            }
        }
        .task { await model.loadSection(.requestLogs) }
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
            // Drop the selection (and its detail) only when the selected row
            // vanished from the refreshed list; an in-flight detail load for
            // a still-existing row keeps running.
            if let id = selectedID, !logs.contains(where: { $0.id == id }) {
                selectedID = nil
            }
        }
        .onChange(of: selectedID) { id in
            detailTask?.cancel()
            model.clearRequestLogDetail()
            guard let id else {
                return
            }
            detailTask = Task { await model.loadRequestLogDetail(id: id) }
        }
        .onDisappear { detailTask?.cancel() }
        .alert("清空全部请求日志？", isPresented: $confirmsClear) {
            Button("取消", role: .cancel) {}
            Button("清空", role: .destructive) {
                Task {
                    clearing = true
                    if await model.clearRequestLogs() {
                        selectedID = nil
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
}

private struct RequestLogRow: View {
    let log: ManageRequestLog

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(spacing: 6) {
                Text(log.modelId)
                    .font(.headline)
                    .lineLimit(1)
                if log.stream {
                    Image(systemName: "bolt.horizontal")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .help("流式")
                        .accessibilityLabel("流式")
                }
                Spacer()
                StatusCapsule(text: log.status, positive: isPositiveStatus(log.status))
            }
            HStack(spacing: 7) {
                Text(log.channel)
                Text("·")
                Text(log.latencyMs.map { "\($0) ms" } ?? "等待耗时")
                if let tokens = log.totalTokens {
                    Text("·")
                    Text("\(tokens) tokens")
                }
                if let cost = log.costUsd {
                    Text("·")
                    Text(String(format: "$%.6f", cost))
                }
            }
            .font(.caption)
            .foregroundStyle(.secondary)
            HStack {
                Text(log.requestId)
                    .font(.caption2.monospaced())
                    .lineLimit(1)
                Spacer()
                Text(relativeDate(log.createdAtMs))
                    .font(.caption2)
            }
            .foregroundStyle(.tertiary)
        }
        .padding(.vertical, 5)
    }
}

private struct RequestLogDetailPane: View {
    let detail: ManageRequestLogDetail?
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        if let detail {
            RequestLogDetailContent(detail: detail) {
                openWindow(value: detail.id)
            }
        } else {
            ManagementEmptyState(
                title: "选择一条请求",
                message: "详情按需读取，并在返回界面前遮罩令牌、密钥和授权头。",
                symbol: "doc.text.magnifyingglass"
            )
        }
    }
}

/// Shared body of the request-log detail: used by the split-view pane and by
/// the standalone detail window.
struct RequestLogDetailContent: View {
    let detail: ManageRequestLogDetail
    var onOpenWindow: (() -> Void)?
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
                if let onOpenWindow {
                    Button {
                        onOpenWindow()
                    } label: {
                        Label("在独立窗口打开", systemImage: "macwindow.on.rectangle")
                    }
                    .accessibilityLabel("在独立窗口打开日志详情")
                }
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

/// Standalone detail window content: loads its own copy of the log detail so
/// it never races with the split-view pane's selection-driven state.
struct RequestLogDetailWindow: View {
    @EnvironmentObject private var model: AppModel
    let logID: Int64
    @State private var detail: ManageRequestLogDetail?
    @State private var errorMessage: String?

    var body: some View {
        Group {
            if let detail {
                RequestLogDetailContent(detail: detail)
            } else if let errorMessage {
                VStack(spacing: 12) {
                    Label(errorMessage, systemImage: "exclamationmark.triangle")
                        .foregroundStyle(.orange)
                    Button("重试") {
                        Task { await load() }
                    }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ProgressView("正在读取详情…")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .frame(minWidth: 560, minHeight: 480)
        .background(Color(nsColor: .windowBackgroundColor))
        .navigationTitle("请求日志详情 #\(logID)")
        .task { await load() }
    }

    private func load() async {
        errorMessage = nil
        do {
            detail = try await model.fetchRequestLogDetailStandalone(id: logID)
        } catch let error as APIClientError {
            errorMessage = error.localizedDescription
        } catch {
            errorMessage = "本地服务不可用。"
        }
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
    let title: String
    let subtitle: String
    let symbol: String
    let loading: Bool
    let error: String?
    let retry: () -> Void
    @ViewBuilder let content: Content

    init(
        title: String,
        subtitle: String,
        symbol: String,
        loading: Bool,
        error: String?,
        retry: @escaping () -> Void,
        @ViewBuilder content: () -> Content
    ) {
        self.title = title
        self.subtitle = subtitle
        self.symbol = symbol
        self.loading = loading
        self.error = error
        self.retry = retry
        self.content = content()
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                ManagementPageHeader(title: title, subtitle: subtitle, symbol: symbol)
                if let error {
                    InlineManagementError(message: error, retry: retry)
                }
                content
            }
            .frame(maxWidth: 860, alignment: .leading)
            .padding(28)
        }
        .scrollIndicators(.never)
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

struct ManagementPageHeader: View {
    let title: String
    let subtitle: String
    let symbol: String

    var body: some View {
        HStack(spacing: 14) {
            Image(systemName: symbol)
                .font(.system(size: 23, weight: .medium))
                .foregroundStyle(Color.accentColor)
                .frame(width: 42, height: 42)
                .background(Color.accentColor.opacity(0.11), in: RoundedRectangle(cornerRadius: 11))
            VStack(alignment: .leading, spacing: 3) {
                Text(title)
                    .font(.title2.weight(.semibold))
                Text(subtitle)
                    .foregroundStyle(.secondary)
            }
        }
    }
}

struct ManagementCard<Content: View>: View {
    let title: String
    let symbol: String
    @ViewBuilder let content: Content

    init(title: String, symbol: String, @ViewBuilder content: () -> Content) {
        self.title = title
        self.symbol = symbol
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Label(title, systemImage: symbol)
                .font(.headline)
            content
        }
        .padding(18)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 12))
        .overlay {
            RoundedRectangle(cornerRadius: 12)
                .stroke(Color.primary.opacity(0.07), lineWidth: 1)
        }
    }
}

private struct ManagementStatusRow: View {
    let title: String
    let detail: String
    let ready: Bool

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: ready ? "checkmark.circle.fill" : "exclamationmark.circle.fill")
                .foregroundStyle(ready ? Color.green : Color.orange)
            Text(title)
            Spacer()
            Text(detail)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.trailing)
                .lineLimit(2)
        }
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

private func relativeDate(_ milliseconds: Int64) -> String {
    let date = Date(timeIntervalSince1970: TimeInterval(milliseconds) / 1_000)
    return RelativeDateTimeFormatter().localizedString(for: date, relativeTo: Date())
}

private func isPositiveStatus(_ status: String) -> Bool {
    let normalized = status.lowercased()
    return normalized == "success"
        || normalized == "completed"
        || normalized == "ok"
        || normalized.hasPrefix("2")
}
