import AppKit
import CoreImage.CIFilterBuiltins
import SwiftUI

/// Callbacks the onboarding sheet uses to reach the daemon. Grouping them in
/// one value keeps the view decoupled from the model and previewable with
/// deterministic fixtures.
struct MessagingOnboardingActions {
    var configureTelegram:
        @MainActor (_ botToken: String, _ mentionOnly: Bool) async throws
            -> ManageIMAccountConfigureResponse
    var configureFeishu:
        @MainActor (_ appId: String, _ appSecret: String) async throws
            -> ManageIMAccountConfigureResponse
    var startFeishuScan: @MainActor () async throws -> ManageFeishuOnboardingStart
    var pollFeishuScan:
        @MainActor (_ deviceCode: String) async throws -> ManageFeishuOnboardingPoll
    var startWechatScan: @MainActor () async throws -> ManageWechatOnboardingStart
    var pollWechatScan:
        @MainActor (_ sessionKey: String, _ verifyCode: String?) async throws
            -> ManageWechatOnboardingPoll
    var startWecomScan: @MainActor () async throws -> ManageWecomOnboardingStart
    var pollWecomScan:
        @MainActor (_ sessionKey: String) async throws -> ManageWecomOnboardingPoll
}

/// Four-step onboarding sheet for adding a messaging account:
/// choose a platform, provide credentials or scan, wait for verification,
/// done. Every step can go back or cancel, failures stay on the current step
/// for a retry, expired QR codes refresh in place, and success closes the
/// sheet from an explicit confirmation step.
struct MessagingOnboardingView: View {
    private enum Step: Equatable {
        case platform
        case telegramCredentials
        case feishuSetup
        case wechatScan
        case wecomScan
        case verifying
        case done
    }

    private enum FeishuMethod: String, CaseIterable, Identifiable {
        case scan
        case manual

        var id: String { rawValue }

        var title: String {
            switch self {
            case .scan: "扫码授权"
            case .manual: "手动填写凭据"
            }
        }
    }

    private enum ScanPhase: Equatable {
        case idle
        case loading
        case waiting(qrContent: String, hint: String)
        case awaitingVerifyCode(qrContent: String)
        case expired(message: String)
        case failed(message: String)
    }

    let actions: MessagingOnboardingActions

    @Environment(\.dismiss) private var dismiss
    @State private var step: Step = .platform
    @State private var selectedPlatform: MessagingAccountSummary.Platform = .telegram

    @State private var botToken = ""
    @State private var mentionOnly = false

    @State private var feishuMethod: FeishuMethod = .scan
    @State private var feishuAppID = ""
    @State private var feishuAppSecret = ""

    @State private var wechatSessionKey = ""
    @State private var wechatVerifyCode = ""

    @State private var scanPhase: ScanPhase = .idle
    @State private var scanTask: Task<Void, Never>?
    @State private var verifyTask: Task<Void, Never>?
    @State private var verifyingReturnStep: Step = .telegramCredentials
    @State private var errorMessage: String?
    @State private var completedAccountName = ""

    var body: some View {
        VStack(alignment: .leading, spacing: MochiPortSpacing.section) {
            header
            content
            Spacer(minLength: 0)
            footer
        }
        .padding(MochiPortSpacing.page)
        .frame(width: 520, height: 500)
        .onDisappear {
            scanTask?.cancel()
            verifyTask?.cancel()
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("messaging-onboarding.sheet")
    }

    // MARK: - Header

    private var header: some View {
        VStack(alignment: .leading, spacing: MochiPortSpacing.compact) {
            Image(systemName: headerSymbol)
                .font(.system(size: 28))
                .foregroundStyle(step == .done ? AnyShapeStyle(.green) : AnyShapeStyle(.tint))
            Text(headerTitle)
                .font(.title2.weight(.semibold))
            Text(headerSubtitle)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var headerSymbol: String {
        switch step {
        case .platform: "message.badge.waveform"
        case .telegramCredentials: MessagingAccountSummary.Platform.telegram.symbol
        case .feishuSetup: MessagingAccountSummary.Platform.feishu.symbol
        case .wechatScan: MessagingAccountSummary.Platform.wechat.symbol
        case .wecomScan: MessagingAccountSummary.Platform.wecom.symbol
        case .verifying: "clock"
        case .done: "checkmark.circle.fill"
        }
    }

    private var headerTitle: String {
        switch step {
        case .platform: "连接消息渠道"
        case .telegramCredentials: "连接 Telegram"
        case .feishuSetup: "连接飞书"
        case .wechatScan: "连接微信"
        case .wecomScan: "连接企业微信"
        case .verifying: "正在验证"
        case .done: "已连接"
        }
    }

    private var headerSubtitle: String {
        switch step {
        case .platform: "选择要接入的平台。"
        case .telegramCredentials: "输入从 @BotFather 获取的机器人 Token。"
        case .feishuSetup: "扫码授权，或手动填写应用凭据。"
        case .wechatScan: "使用微信扫码登录机器人。"
        case .wecomScan: "使用企业微信扫码授权。"
        case .verifying: "正在验证凭据…"
        case .done: completedAccountName
        }
    }

    // MARK: - Step content

    @ViewBuilder
    private var content: some View {
        switch step {
        case .platform:
            platformList
        case .telegramCredentials:
            telegramCredentialsForm
        case .feishuSetup:
            feishuSetup
        case .wechatScan, .wecomScan:
            scanArea
        case .verifying:
            HStack(spacing: MochiPortSpacing.standard) {
                ProgressView()
                    .controlSize(.small)
                Text("验证通常在几秒内完成。")
                    .foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        case .done:
            Text("账号已出现在消息渠道列表中，可随时启停或删除。")
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var platformList: some View {
        VStack(spacing: 0) {
            platformRow(.telegram, detail: "使用机器人 Token 接入")
            Divider().padding(.leading, 46)
            platformRow(.feishu, detail: "扫码授权或手动填写应用凭据")
            Divider().padding(.leading, 46)
            platformRow(.wechat, detail: "扫码登录，必要时输入验证码")
            Divider().padding(.leading, 46)
            platformRow(.wecom, detail: "扫码授权企业微信机器人")
        }
        .settingsGroupedBackground()
        .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
    }

    private func platformRow(
        _ platform: MessagingAccountSummary.Platform,
        detail: String
    ) -> some View {
        Button {
            selectedPlatform = platform
        } label: {
            HStack(spacing: MochiPortSpacing.standard) {
                Image(systemName: platform.symbol)
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(.tint)
                    .frame(width: 26)
                VStack(alignment: .leading, spacing: 2) {
                    Text(platform.title)
                        .font(.body.weight(.medium))
                        .foregroundStyle(.primary)
                    Text(detail)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer(minLength: MochiPortSpacing.compact)
                Image(systemName: selectedPlatform == platform ? "checkmark.circle.fill" : "circle")
                    .foregroundStyle(.tint)
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("onboarding.platform.\(platform.rawValue)")
    }

    private var telegramCredentialsForm: some View {
        VStack(alignment: .leading, spacing: MochiPortSpacing.standard) {
            SecureField("Bot Token", text: $botToken)
                .textFieldStyle(.roundedBorder)
                .accessibilityIdentifier("onboarding.telegram.token")
            Toggle("仅响应 @ 提及的消息", isOn: $mentionOnly)
                .accessibilityIdentifier("onboarding.telegram.mention-only")
            Text("在 Telegram 中与 @BotFather 对话并使用 /newbot 创建机器人即可获得 Token。凭据只写入本地服务，界面不会回显。")
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            errorLabel
        }
    }

    private var feishuSetup: some View {
        VStack(alignment: .leading, spacing: MochiPortSpacing.standard) {
            Picker("接入方式", selection: $feishuMethod) {
                ForEach(FeishuMethod.allCases) { method in
                    Text(method.title).tag(method)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .onChange(of: feishuMethod) { _, method in
                switch method {
                case .scan:
                    startFeishuScanFlow()
                case .manual:
                    cancelScan()
                }
            }

            switch feishuMethod {
            case .scan:
                scanArea
            case .manual:
                TextField("App ID", text: $feishuAppID)
                    .textFieldStyle(.roundedBorder)
                    .accessibilityIdentifier("onboarding.feishu.app-id")
                SecureField("App Secret", text: $feishuAppSecret)
                    .textFieldStyle(.roundedBorder)
                    .accessibilityIdentifier("onboarding.feishu.app-secret")
                Text("在飞书开放平台的应用详情中获取凭据。凭据只写入本地服务，界面不会回显。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                errorLabel
            }
        }
    }

    @ViewBuilder
    private var errorLabel: some View {
        if let errorMessage {
            Label(errorMessage, systemImage: "exclamationmark.triangle")
                .font(.callout)
                .foregroundStyle(.red)
                .fixedSize(horizontal: false, vertical: true)
                .accessibilityIdentifier("onboarding.error")
        }
    }

    @ViewBuilder
    private var scanArea: some View {
        switch scanPhase {
        case .idle, .loading:
            HStack(spacing: MochiPortSpacing.standard) {
                ProgressView()
                    .controlSize(.small)
                Text("正在获取二维码…")
                    .foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, minHeight: 190)
        case let .waiting(qrContent, hint):
            VStack(spacing: MochiPortSpacing.standard) {
                QRCodeView(content: qrContent)
                    .frame(width: 164, height: 164)
                HStack(spacing: 6) {
                    ProgressView()
                        .controlSize(.mini)
                    Text(hint)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            .frame(maxWidth: .infinity)
        case let .awaitingVerifyCode(qrContent):
            VStack(spacing: MochiPortSpacing.standard) {
                QRCodeView(content: qrContent)
                    .frame(width: 108, height: 108)
                    .opacity(0.3)
                Text("微信要求输入验证码以完成连接。")
                    .font(.callout)
                errorLabel
                HStack(spacing: MochiPortSpacing.compact) {
                    TextField("验证码", text: $wechatVerifyCode)
                        .textFieldStyle(.roundedBorder)
                        .frame(width: 150)
                        .accessibilityIdentifier("onboarding.wechat.verify-code")
                    Button("提交验证码") {
                        submitWechatVerifyCode()
                    }
                    .keyboardShortcut(.defaultAction)
                    .disabled(
                        wechatVerifyCode.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                    )
                }
            }
            .frame(maxWidth: .infinity)
        case let .expired(message), let .failed(message):
            VStack(spacing: MochiPortSpacing.standard) {
                Image(systemName: "exclamationmark.triangle")
                    .font(.system(size: 24))
                    .foregroundStyle(.orange)
                Text(message)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
                Button("重新获取二维码") {
                    restartCurrentScan()
                }
            }
            .frame(maxWidth: .infinity, minHeight: 190)
        }
    }

    // MARK: - Footer

    @ViewBuilder
    private var footer: some View {
        HStack {
            switch step {
            case .platform:
                Spacer()
                Button("取消", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button("继续") { advanceFromPlatform() }
                    .keyboardShortcut(.defaultAction)
            case .telegramCredentials:
                Button("返回") { goBackToPlatform() }
                Spacer()
                Button("取消", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button("验证并添加") { startTelegramVerification() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(trimmedBotToken.isEmpty)
            case .feishuSetup:
                Button("返回") { goBackToPlatform() }
                Spacer()
                Button("取消", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                if feishuMethod == .manual {
                    Button("验证并添加") { startFeishuCredentialVerification() }
                        .keyboardShortcut(.defaultAction)
                        .disabled(trimmedFeishuAppID.isEmpty || trimmedFeishuAppSecret.isEmpty)
                }
            case .wechatScan, .wecomScan:
                Button("返回") { goBackToPlatform() }
                Spacer()
                Button("取消", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction)
            case .verifying:
                Spacer()
                Button("取消验证", role: .cancel) {
                    verifyTask?.cancel()
                }
                .keyboardShortcut(.cancelAction)
            case .done:
                Spacer()
                Button("完成") { dismiss() }
                    .keyboardShortcut(.defaultAction)
            }
        }
    }

    // MARK: - Navigation

    private func advanceFromPlatform() {
        errorMessage = nil
        switch selectedPlatform {
        case .telegram:
            step = .telegramCredentials
        case .feishu:
            step = .feishuSetup
            if feishuMethod == .scan {
                startFeishuScanFlow()
            }
        case .wechat:
            step = .wechatScan
            startWechatScanFlow()
        case .wecom:
            step = .wecomScan
            startWecomScanFlow()
        }
    }

    private func goBackToPlatform() {
        cancelScan()
        errorMessage = nil
        step = .platform
    }

    private func cancelScan() {
        scanTask?.cancel()
        scanTask = nil
        scanPhase = .idle
    }

    private func restartCurrentScan() {
        switch step {
        case .feishuSetup:
            startFeishuScanFlow()
        case .wechatScan:
            startWechatScanFlow()
        case .wecomScan:
            startWecomScanFlow()
        default:
            break
        }
    }

    private func finishScan(name: String) {
        scanPhase = .idle
        completedAccountName = name
        step = .done
    }

    // MARK: - Telegram

    private var trimmedBotToken: String {
        botToken.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private func startTelegramVerification() {
        errorMessage = nil
        verifyingReturnStep = .telegramCredentials
        step = .verifying
        verifyTask = Task {
            do {
                let response = try await actions.configureTelegram(trimmedBotToken, mentionOnly)
                completedAccountName = displayName(from: response)
                step = .done
            } catch {
                handleVerificationFailure(error)
            }
        }
    }

    // MARK: - Feishu

    private var trimmedFeishuAppID: String {
        feishuAppID.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var trimmedFeishuAppSecret: String {
        feishuAppSecret.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private func startFeishuCredentialVerification() {
        errorMessage = nil
        verifyingReturnStep = .feishuSetup
        step = .verifying
        verifyTask = Task {
            do {
                let response = try await actions.configureFeishu(
                    trimmedFeishuAppID,
                    trimmedFeishuAppSecret
                )
                completedAccountName = displayName(from: response)
                step = .done
            } catch {
                handleVerificationFailure(error)
            }
        }
    }

    private func startFeishuScanFlow() {
        scanTask?.cancel()
        errorMessage = nil
        scanPhase = .loading
        scanTask = Task {
            do {
                let session = try await actions.startFeishuScan()
                let interval = max(2, session.interval)
                let deadline = Date().addingTimeInterval(TimeInterval(max(60, session.expiresIn)))
                scanPhase = .waiting(
                    qrContent: session.verificationUriComplete,
                    hint: "使用飞书 App 扫码并确认授权。"
                )
                while !Task.isCancelled {
                    try await Task.sleep(for: .seconds(interval))
                    let poll = try await actions.pollFeishuScan(session.deviceCode)
                    if poll.done {
                        finishScan(name: poll.displayName ?? poll.appId ?? "飞书应用")
                        return
                    }
                    if let code = poll.error, !code.isEmpty,
                       code != "authorization_pending", code != "slow_down" {
                        if code == "expired_token" {
                            scanPhase = .expired(message: "二维码已过期。")
                        } else {
                            scanPhase = .failed(message: poll.errorDescription ?? code)
                        }
                        return
                    }
                    if Date() > deadline {
                        scanPhase = .expired(message: "二维码已过期。")
                        return
                    }
                }
            } catch {
                handleScanFailure(error)
            }
        }
    }

    // MARK: - WeChat

    private func startWechatScanFlow() {
        scanTask?.cancel()
        errorMessage = nil
        wechatVerifyCode = ""
        scanPhase = .loading
        scanTask = Task {
            do {
                let session = try await actions.startWechatScan()
                wechatSessionKey = session.sessionKey
                let deadline = Date().addingTimeInterval(TimeInterval(max(60, session.expiresIn)))
                scanPhase = .waiting(
                    qrContent: session.qrcodeUrl,
                    hint: "使用微信扫码并在手机上确认。"
                )
                try await runWechatPollLoop(
                    qrContent: session.qrcodeUrl,
                    deadline: deadline,
                    initialVerifyCode: nil
                )
            } catch {
                handleScanFailure(error)
            }
        }
    }

    private func submitWechatVerifyCode() {
        let code = wechatVerifyCode.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !code.isEmpty, case let .awaitingVerifyCode(qrContent) = scanPhase else { return }
        scanTask?.cancel()
        errorMessage = nil
        wechatVerifyCode = ""
        scanPhase = .waiting(qrContent: qrContent, hint: "正在提交验证码…")
        scanTask = Task {
            do {
                try await runWechatPollLoop(
                    qrContent: qrContent,
                    deadline: Date().addingTimeInterval(300),
                    initialVerifyCode: code
                )
            } catch {
                handleScanFailure(error)
            }
        }
    }

    private func runWechatPollLoop(
        qrContent: String,
        deadline: Date,
        initialVerifyCode: String?
    ) async throws {
        var pendingCode = initialVerifyCode
        while !Task.isCancelled {
            if pendingCode == nil {
                try await Task.sleep(for: .seconds(3))
            }
            let poll = try await actions.pollWechatScan(wechatSessionKey, pendingCode)
            let submittedCode = pendingCode != nil
            pendingCode = nil
            if poll.done {
                if poll.alreadyConnected == true {
                    finishScan(name: "该微信此前已连接")
                } else {
                    finishScan(name: poll.accountId ?? "微信机器人")
                }
                return
            }
            if poll.needVerifyCode == true {
                if submittedCode {
                    errorMessage = "验证码不正确，请重试。"
                }
                scanPhase = .awaitingVerifyCode(qrContent: qrContent)
                return
            }
            if let error = poll.error, !error.isEmpty {
                switch error {
                case "expired":
                    scanPhase = .expired(message: "二维码已过期。")
                case "verify_code_blocked":
                    scanPhase = .failed(message: "验证码尝试次数过多，请稍后重新扫码。")
                default:
                    scanPhase = .failed(message: error)
                }
                return
            }
            if Date() > deadline {
                scanPhase = .expired(message: "二维码已过期。")
                return
            }
        }
    }

    // MARK: - WeCom

    private func startWecomScanFlow() {
        scanTask?.cancel()
        errorMessage = nil
        scanPhase = .loading
        scanTask = Task {
            do {
                let session = try await actions.startWecomScan()
                let interval = max(2, session.interval)
                let deadline = Date().addingTimeInterval(TimeInterval(max(60, session.expiresIn)))
                scanPhase = .waiting(
                    qrContent: session.qrcodeUrl,
                    hint: "使用企业微信扫码并确认。"
                )
                while !Task.isCancelled {
                    try await Task.sleep(for: .seconds(interval))
                    let poll = try await actions.pollWecomScan(session.sessionKey)
                    if poll.done {
                        finishScan(name: poll.accountId ?? "企业微信机器人")
                        return
                    }
                    if let error = poll.error, !error.isEmpty {
                        if error == "expired" {
                            scanPhase = .expired(message: "二维码已过期。")
                        } else {
                            scanPhase = .failed(message: error)
                        }
                        return
                    }
                    if Date() > deadline {
                        scanPhase = .expired(message: "二维码已过期。")
                        return
                    }
                }
            } catch {
                handleScanFailure(error)
            }
        }
    }

    // MARK: - Shared failure handling

    private func displayName(from response: ManageIMAccountConfigureResponse) -> String {
        let name = response.displayName?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return name.isEmpty ? response.accountId : name
    }

    private func handleVerificationFailure(_ error: Error) {
        if error is CancellationError || (error as? URLError)?.code == .cancelled {
            step = verifyingReturnStep
            return
        }
        errorMessage = failureMessage(for: error)
        step = verifyingReturnStep
    }

    private func handleScanFailure(_ error: Error) {
        if error is CancellationError || (error as? URLError)?.code == .cancelled {
            return
        }
        scanPhase = .failed(message: failureMessage(for: error))
    }

    private func failureMessage(for error: Error) -> String {
        if let apiError = error as? APIClientError {
            return apiError.localizedDescription
        }
        return "无法连接本地服务，请稍后重试。"
    }
}

private struct QRCodeView: View {
    let content: String

    var body: some View {
        if let image = Self.image(for: content) {
            Image(nsImage: image)
                .interpolation(.none)
                .resizable()
                .scaledToFit()
                .accessibilityLabel("二维码")
        } else {
            RoundedRectangle(cornerRadius: 8)
                .fill(Color.primary.opacity(0.06))
                .overlay {
                    Image(systemName: "qrcode")
                        .font(.system(size: 30))
                        .foregroundStyle(.secondary)
                }
        }
    }

    private static func image(for content: String) -> NSImage? {
        let filter = CIFilter.qrCodeGenerator()
        filter.message = Data(content.utf8)
        filter.correctionLevel = "M"
        guard let output = filter.outputImage else { return nil }
        let scaled = output.transformed(by: CGAffineTransform(scaleX: 8, y: 8))
        let representation = NSCIImageRep(ciImage: scaled)
        let image = NSImage(size: representation.size)
        image.addRepresentation(representation)
        return image
    }
}

#if DEBUG
extension MessagingOnboardingActions {
    /// Deterministic preview wiring that never contacts a daemon.
    static let preview = MessagingOnboardingActions(
        configureTelegram: { _, _ in
            try? await Task.sleep(for: .seconds(1))
            return ManageIMAccountConfigureResponse(
                ok: true,
                platform: "telegram",
                accountId: "tg_1000001",
                displayName: "预览机器人 (@preview_bot)"
            )
        },
        configureFeishu: { appId, _ in
            try? await Task.sleep(for: .seconds(1))
            return ManageIMAccountConfigureResponse(
                ok: true,
                platform: "feishu",
                accountId: appId,
                displayName: "预览飞书应用"
            )
        },
        startFeishuScan: {
            ManageFeishuOnboardingStart(
                verificationUri: "https://example.invalid/feishu",
                verificationUriComplete: "https://example.invalid/feishu?code=preview",
                deviceCode: "preview-device-code",
                expiresIn: 600,
                interval: 2,
                qrSvg: ""
            )
        },
        pollFeishuScan: { _ in
            ManageFeishuOnboardingPoll(
                done: false,
                appId: nil,
                displayName: nil,
                error: "authorization_pending",
                errorDescription: nil
            )
        },
        startWechatScan: {
            ManageWechatOnboardingStart(
                sessionKey: "preview-session",
                qrcodeUrl: "https://example.invalid/wechat-qr",
                qrSvg: "",
                expiresIn: 300
            )
        },
        pollWechatScan: { _, _ in
            ManageWechatOnboardingPoll(
                done: false,
                status: "pending",
                needVerifyCode: false,
                accountId: nil,
                alreadyConnected: nil,
                error: nil
            )
        },
        startWecomScan: {
            ManageWecomOnboardingStart(
                sessionKey: "preview-session",
                qrcodeUrl: "https://example.invalid/wecom-qr",
                qrSvg: "",
                expiresIn: 300,
                interval: 2
            )
        },
        pollWecomScan: { _ in
            ManageWecomOnboardingPoll(
                done: false,
                status: "pending",
                accountId: nil,
                error: nil
            )
        }
    )
}

#Preview("消息渠道接入") {
    MessagingOnboardingView(actions: .preview)
}
#endif
