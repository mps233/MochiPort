import AppKit
import SwiftUI

struct SettingsView: View {
    @EnvironmentObject private var model: AppModel
    @EnvironmentObject private var glass: AIGlassCoordinator
    @Environment(\.openURL) private var openURL
    @AppStorage("closeBehavior") private var closeBehavior = "menuBar"
    @State private var language = "zh-CN"
    @State private var theme = "system"
    @State private var connectionMode = "standard"
    @State private var proxyMode = "system"
    @State private var proxyURL = ""
    @State private var originalProxyURL = ""
    @State private var clearProxyCredentials = false
    @State private var saving = false
    @State private var settingsReady = false
    @State private var checkingUpdate = false
    @State private var updateMessage = "尚未检查更新"
    @State private var releaseNotes = ""
    @State private var latestReleaseURL: URL?
    @State private var confirmsRestart = false
    @State private var daemonUpdateConfirmation: DaemonUpdateConfirmation?
    @State private var daemonTakeoverConfirmation: DaemonManagementConfirmation?
    @State private var credentialRotationConfirmation: DaemonManagementConfirmation?

    var body: some View {
        TabView {
            generalSettings
            .tabItem { Label("通用", systemImage: "gearshape") }

            networkSettings
                .tabItem { Label("网络", systemImage: "network") }

            AIGlassSettingsView(settings: glass.settings)
                .tabItem { Label("使用量", systemImage: "chart.bar.xaxis") }

            diagnosticsSettings
                .tabItem { Label("更新与诊断", systemImage: "stethoscope") }
        }
        .frame(width: 680, height: 460)
        .task {
            await model.loadSettings()
            synchronizeSettings(model.settings)
            await model.refresh()
        }
        .onChange(of: model.settings) { settings in
            synchronizeSettings(settings)
        }
        .confirmationDialog(
            "重启本地服务？",
            isPresented: $confirmsRestart,
            titleVisibility: .visible
        ) {
            Button("重启", role: .destructive) {
                Task { await model.restartDaemon() }
            }
            Button("取消", role: .cancel) {}
        } message: {
            Text("MochiPort 会先确认没有进行中的受保护任务，再请求后台服务安全重启。")
        }
        .confirmationDialog(
            "更新后台服务？",
            isPresented: Binding(
                get: { daemonUpdateConfirmation != nil },
                set: { if !$0 { daemonUpdateConfirmation = nil } }
            ),
            titleVisibility: .visible
        ) {
            Button("更新后台服务", role: .destructive) {
                guard let confirmation = daemonUpdateConfirmation else { return }
                daemonUpdateConfirmation = nil
                Task { await model.activateDaemonUpdate(confirming: confirmation) }
            }
            Button("取消", role: .cancel) {}
        } message: {
            if let confirmation = daemonUpdateConfirmation {
                Text(
                    "将后台服务更新至 \(confirmation.candidate.version)（构建 \(confirmation.candidate.build)）。"
                        + " MochiPort 会先等待受保护任务完成，再切换版本并安全退出后台服务。"
                        + (confirmation.protectedWorkItemCount == 0
                            ? " 当前没有受保护任务。"
                            : " 当前有 \(confirmation.protectedWorkItemCount) 项受保护任务。")
                )
            }
        }
        .confirmationDialog(
            "接管后台服务？",
            isPresented: Binding(
                get: { daemonTakeoverConfirmation != nil },
                set: { if !$0 { daemonTakeoverConfirmation = nil } }
            ),
            titleVisibility: .visible
        ) {
            Button("接管后台服务", role: .destructive) {
                guard let confirmation = daemonTakeoverConfirmation else { return }
                daemonTakeoverConfirmation = nil
                Task { await model.takeOverDaemonManagement(confirming: confirmation) }
            }
            Button("取消", role: .cancel) {}
        } message: {
            Text("这会重新核验正在运行的后台服务，接替其他安装的管理租约，并立即更换共享管理凭据。其他安装将失去管理权。")
        }
        .confirmationDialog(
            "重新生成管理凭据？",
            isPresented: Binding(
                get: { credentialRotationConfirmation != nil },
                set: { if !$0 { credentialRotationConfirmation = nil } }
            ),
            titleVisibility: .visible
        ) {
            Button("重新生成", role: .destructive) {
                guard let confirmation = credentialRotationConfirmation else { return }
                credentialRotationConfirmation = nil
                Task { await model.rotateManagementCredential(confirming: confirmation) }
            }
            Button("取消", role: .cancel) {}
        } message: {
            Text("新的管理凭据会立即生效，旧凭据不再可用。当前安装会继续管理后台服务。")
        }
    }

    private var generalSettings: some View {
        settingsForm {
            Section("窗口") {
                Picker("关闭主窗口时", selection: $closeBehavior) {
                    Text("隐藏到菜单栏").tag("menuBar")
                    Text("退出界面").tag("quitGUI")
                }
                Text("无论选择哪种方式，本地服务都会继续运行。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            Section("显示") {
                Picker("服务消息语言", selection: $language) {
                    Text("简体中文").tag("zh-CN")
                    Text("English").tag("en-US")
                }
                Picker("外观", selection: $theme) {
                    Text("跟随系统").tag("system")
                    Text("浅色").tag("light")
                    Text("深色").tag("dark")
                }
                Text("消息语言用于后台服务回复；外观会立即应用到 MochiPort 窗口。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            saveSection
        }
    }

    private var networkSettings: some View {
        settingsForm {
            Section("本地连接") {
                Picker("连接模式", selection: $connectionMode) {
                    Text("标准（127.0.0.1）").tag("standard")
                    Text("VPN 兼容（localhost）").tag("vpnCompatible")
                }
                if let bind = model.settings?.bind {
                    LabeledContent("监听地址") {
                        Text(bind)
                            .font(.body.monospaced())
                            .textSelection(.enabled)
                    }
                }
            }

            Section("出站代理") {
                Picker("代理模式", selection: $proxyMode) {
                    Text("跟随系统").tag("system")
                    Text("直连").tag("direct")
                    Text("自定义").tag("custom")
                }
                if proxyMode == "custom" {
                    TextField("例如 socks5://127.0.0.1:1080", text: $proxyURL)
                    if model.settings?.outboundProxy.credentialSet == true {
                        Label("已保存代理凭据；不修改 URL 会继续保留。", systemImage: "key.fill")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                        Toggle("清除已保存的代理凭据", isOn: $clearProxyCredentials)
                    }
                }
            }

            saveSection
        }
    }

    @ViewBuilder
    private var serviceSettingsSections: some View {
        Section("本地服务") {
            if let lifecycle = model.lifecycle {
                LabeledContent("状态", value: model.serviceStatus.title)
                LabeledContent("版本", value: lifecycle.runtime.productVersion)
                LabeledContent(
                    "构建",
                    value: lifecycle.runtime.buildNumber.map(String.init) ?? "旧版后台服务"
                )
                if model.daemonBuildMismatch {
                    Label(
                        model.daemonUpgradePending
                            ? model.daemonUpgradeDetail
                            : "界面与后台服务构建不一致",
                        systemImage: "exclamationmark.triangle"
                    )
                        .foregroundStyle(.orange)
                }
                Label(
                    model.ownsDaemonLease
                        ? "当前界面已获得后台服务管理权"
                        : model.daemonLeaseConflict
                            ? "后台服务由其他安装管理，当前界面仅能查看"
                            : "当前界面仅能查看后台服务状态",
                    systemImage: model.ownsDaemonLease ? "lock.open" : "eye"
                )
                .font(.caption)
                .foregroundStyle(model.daemonLeaseConflict ? .orange : .secondary)
                LabeledContent("进程", value: String(lifecycle.service.pid))
                LabeledContent("运行文件") {
                    Text(lifecycle.executable)
                        .font(.caption.monospaced())
                        .textSelection(.enabled)
                }
                LabeledContent("配置文件") {
                    Text(lifecycle.configPath)
                        .font(.caption.monospaced())
                        .textSelection(.enabled)
                }
                LabeledContent("受保护任务", value: String(lifecycle.protectedWorkItems.total))
            } else {
                LabeledContent("状态", value: model.serviceStatus.title)
                Text("尚未读取到本地服务信息。")
                    .foregroundStyle(.secondary)
            }
        }

        Section("本地服务操作") {
            HStack {
                Button("刷新状态") {
                    Task { await model.refresh() }
                }
                Button("打开日志目录") {
                    Task {
                        guard let url = await model.logDirectory() else { return }
                        NSWorkspace.shared.open(url)
                    }
                }
            }
            if model.ownsDaemonLease {
                Button {
                    confirmsRestart = true
                } label: {
                    Label(
                        model.daemonTransitionInProgress
                            ? "正在重启后台服务"
                            : "安全重启后台服务",
                        systemImage: "arrow.clockwise.circle"
                    )
                }
                .disabled(
                    model.daemonTransitionInProgress
                        || model.daemonLeaseTakeoverInProgress
                        || model.managementCredentialRotationInProgress
                )
            }
            if model.daemonLeaseConflict {
                Button {
                    daemonTakeoverConfirmation = model.daemonLeaseTakeoverConfirmation
                } label: {
                    Label(
                        model.daemonLeaseTakeoverInProgress
                            ? "正在接管后台服务"
                            : "接管后台服务",
                        systemImage: "lock.open"
                    )
                }
                .disabled(!model.canTakeOverDaemonLease)
            }
            if model.ownsDaemonLease {
                Button {
                    credentialRotationConfirmation = model.managementCredentialRotationConfirmation
                } label: {
                    Label(
                        model.managementCredentialRotationInProgress
                            ? "正在重新生成管理凭据"
                            : "重新生成管理凭据",
                        systemImage: "key"
                    )
                }
                .disabled(!model.canRotateManagementCredential)
            }
            if model.daemonLeaseTakeoverInProgress
                || model.managementCredentialRotationInProgress
            {
                HStack(spacing: 8) {
                    ProgressView()
                        .controlSize(.small)
                    Text(model.daemonManagementFeedback ?? "正在处理…")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            } else if let error = model.managementOperationError {
                Label(error, systemImage: "exclamationmark.triangle")
                    .font(.caption)
                    .foregroundStyle(.red)
            } else if let feedback = model.daemonManagementFeedback {
                Label(feedback, systemImage: "checkmark.circle")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }

    private var diagnosticsSettings: some View {
        settingsForm {
            serviceSettingsSections

            Section("版本") {
                LabeledContent(
                    "MochiPort",
                    value: Bundle.main.object(
                        forInfoDictionaryKey: "CFBundleShortVersionString"
                    ) as? String ?? "开发版"
                )
                LabeledContent(
                    "构建",
                    value: Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String
                        ?? "本地"
                )
            }

            Section("诊断") {
                Button("复制当前诊断摘要") {
                    copyDiagnostics()
                }
                Button("打开日志目录") {
                    Task {
                        guard let url = await model.logDirectory() else { return }
                        NSWorkspace.shared.open(url)
                    }
                }
            }

            Section("更新") {
                HStack {
                    Button {
                        checkForUpdates()
                    } label: {
                        Label("检查更新", systemImage: "arrow.clockwise")
                    }
                    .disabled(checkingUpdate || model.updateCheckState == .checking)
                    if checkingUpdate || model.updateCheckState == .checking {
                        ProgressView().controlSize(.small)
                    }
                }
                Text(updateMessage)
                    .font(.caption)
                    .foregroundStyle(.secondary)

                UnifiedUpdateEntry(
                    context: .settings,
                    onConfirmDaemon: {
                        daemonUpdateConfirmation = model.daemonUpdateConfirmation
                    }
                )

                DisclosureGroup("组件版本详情") {
                    componentVersionRow(
                        title: "MochiPort",
                        current: "当前 \(currentVersion)",
                        update: model.availableUIUpdate.map { "可更新至 \($0.version)" }
                    )
                    componentVersionRow(
                        title: "后台服务",
                        current: daemonCurrentVersion,
                        update: daemonUpdateText
                    )
                    if let compatibility = model.daemonUpdateCompatibilityDescription {
                        Label(
                            compatibility,
                            systemImage: model.daemonUpdateCompatibility == .compatible
                                ? "checkmark.circle"
                                : "exclamationmark.triangle"
                        )
                        .font(.caption)
                        .foregroundStyle(
                            model.daemonUpdateCompatibility == .compatible
                                ? Color.secondary
                                : Color.orange
                        )
                    }
                    if case let .failed(message) = model.daemonUpdateOperation {
                        Label(message, systemImage: "exclamationmark.triangle")
                            .font(.caption)
                            .foregroundStyle(.red)
                    }
                }
                if !releaseNotes.isEmpty {
                    DisclosureGroup("发布说明") {
                        ScrollView {
                            Text(releaseNotes)
                                .font(.caption)
                                .textSelection(.enabled)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(.vertical, 6)
                        }
                        .scrollIndicators(.never)
                        .frame(maxHeight: 110)
                    }
                }
                Button(latestReleaseURL == nil ? "打开 GitHub 发布页" : "打开最新版本下载页") {
                    let fallback = URL(
                        string: "https://github.com/mps233/mochiport/releases/latest"
                    )
                    if let url = latestReleaseURL ?? fallback {
                        openURL(url)
                    }
                }
                Text("MochiPort 会统一展示更新；界面和后台服务仍独立发布。后台服务只有在你明确确认后，才会在受保护任务排空后切换版本。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }

    @ViewBuilder
    private var saveSection: some View {
        Section {
            HStack {
                if let error = model.managementOperationError {
                    Text(error)
                        .font(.caption)
                        .foregroundStyle(.red)
                        .lineLimit(2)
                }
                Spacer()
                Button("保存设置") {
                    save()
                }
                .buttonStyle(.borderedProminent)
                .disabled(
                    !settingsReady
                        || saving
                        || (proxyMode == "custom" && proxyURL.isEmpty)
                )
            }
        }
    }

    private func settingsForm<Content: View>(
        @ViewBuilder content: () -> Content
    ) -> some View {
        Form(content: content)
            .formStyle(.grouped)
            .scrollIndicators(.never)
            .padding(12)
    }

    private func save() {
        saving = true
        Task {
            _ = await model.saveSettings(
                language: language,
                theme: theme,
                localConnectionMode: connectionMode,
                outboundProxyMode: proxyMode,
                outboundProxyURL: proxyMode == "custom"
                    && (proxyURL != originalProxyURL || clearProxyCredentials)
                    ? proxyURL
                    : nil
            )
            saving = false
        }
    }

    private func synchronizeSettings(_ settings: ManageSettings?) {
        guard let settings else {
            settingsReady = false
            return
        }
        language = settings.language ?? "zh-CN"
        theme = settings.theme ?? "system"
        connectionMode = settings.localConnectionMode
        proxyMode = settings.outboundProxy.mode
        proxyURL = settings.outboundProxy.url == "<none>" ? "" : settings.outboundProxy.url
        originalProxyURL = proxyURL
        clearProxyCredentials = false
        settingsReady = true
    }

    private func copyDiagnostics() {
        var lines = [
            "MochiPort \(Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "开发版")",
            "服务：\(model.serviceStatus.title)",
            "Dashboard：\(model.dashboardState.title)",
        ]
        if let lifecycle = model.lifecycle {
            lines.append("Daemon：\(lifecycle.executable)")
            lines.append("配置：\(lifecycle.configPath)")
            lines.append("受保护任务：\(lifecycle.protectedWorkItems.total)")
        }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(lines.joined(separator: "\n"), forType: .string)
    }

    private func checkForUpdates() {
        checkingUpdate = true
        updateMessage = "正在检查…"
        releaseNotes = ""
        latestReleaseURL = nil
        Task {
            defer { checkingUpdate = false }
            await model.checkForUpdates()
            latestReleaseURL = model.availableUIUpdate?.validatedReleaseURL
            releaseNotes = combinedReleaseNotes()
            switch model.unifiedUpdateState {
            case .notChecked:
                updateMessage = "尚未检查更新"
            case .upToDate:
                updateMessage = "MochiPort 和后台服务均已是最新版本"
            case .ui:
                updateMessage = "发现 MochiPort 新版本"
            case .daemon:
                updateMessage = "发现后台服务更新"
            case .both:
                updateMessage = "发现 MochiPort 和后台服务更新"
            case .checking:
                updateMessage = "正在检查更新…"
            case let .failed(message):
                updateMessage = "检查失败：\(message)"
            }
        }
    }

    @ViewBuilder
    private func componentVersionRow(
        title: String,
        current: String,
        update: String?
    ) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text(title).font(.callout.weight(.medium))
                    Text(update ?? current)
                        .font(.caption)
                        .foregroundStyle(update == nil ? .secondary : .primary)
                }
                Spacer()
            }
            Divider()
        }
    }

    private var daemonCurrentVersion: String {
        guard let lifecycle = model.lifecycle else { return "当前版本未知" }
        let build = lifecycle.runtime.buildNumber.map { "构建 \($0)" } ?? "旧版后台服务"
        return "\(lifecycle.runtime.productVersion)（\(build)）"
    }

    private var daemonUpdateText: String? {
        guard let update = model.availableDaemonUpdate else { return nil }
        let build = update.build.map { "构建 \($0)" } ?? ""
        return "可更新至 \(update.version)（\(build)）"
    }

    private func combinedReleaseNotes() -> String {
        var sections: [String] = []
        if let notes = model.availableUIUpdate?.notes?
            .trimmingCharacters(in: .whitespacesAndNewlines), !notes.isEmpty
        {
            sections.append("界面更新\n\(notes)")
        }
        if let notes = model.availableDaemonUpdate?.notes?
            .trimmingCharacters(in: .whitespacesAndNewlines), !notes.isEmpty
        {
            sections.append("后台服务更新\n\(notes)")
        }
        return sections.joined(separator: "\n\n")
    }

    private var currentVersion: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0.0.0"
    }
}
