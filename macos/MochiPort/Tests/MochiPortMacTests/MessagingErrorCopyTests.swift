import XCTest

#if canImport(MochiPortMac)
@testable import MochiPortMac
#elseif canImport(MochiPort)
@testable import MochiPort
#endif

/// 覆盖 daemon 原始错误串到「一眼能看懂」文案的映射。
/// 用例里的错误串必须与 daemon 实际产出保持一致：
/// - 传输层失败：`… request failed: error sending request for url (…)`
/// - HTTP 层失败：`telegram api {method} failed: status={code} … error_code=… description=…`
final class MessagingErrorCopyTests: XCTestCase {
    private typealias Platform = MessagingAccountSummary.Platform

    func testTransportFailurePointsToOutboundProxySetting() {
        let notice = MessagingErrorCopy.notice(
            for: "telegram api getUpdates request failed: error sending request for url (https://api.telegram.org/bot***/getUpdates)",
            platform: .telegram
        )
        XCTAssertEqual(notice.title, "无法连接Telegram服务器")
        XCTAssertEqual(notice.hint, "通常是网络不通或代理未生效。检查「设置 → 出站代理」是否指向可用的本地代理。")
    }

    func testFeishuTransportFailureUsesPlatformName() {
        let notice = MessagingErrorCopy.notice(
            for: "feishu tenant_access_token request failed: error sending request for url (https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal)",
            platform: .feishu
        )
        XCTAssertEqual(notice.title, "无法连接飞书服务器")
    }

    func testConflictExplainsSharedBotToken() {
        let notice = MessagingErrorCopy.notice(
            for: "telegram api getUpdates failed: status=409 Conflict error_code=Some(409) description=Conflict: terminated by other getUpdates request; make sure that only one bot instance is running",
            platform: .telegram
        )
        XCTAssertEqual(notice.title, "机器人正在另一处使用（409 冲突）")
        XCTAssertTrue(notice.hint?.contains("自己的机器人") == true)
    }

    func testConflictIsDetectedFromDescriptionWhenStatusIsMissing() {
        let notice = MessagingErrorCopy.notice(
            for: "telegram poll terminated by other getUpdates request",
            platform: .telegram
        )
        XCTAssertEqual(notice.title, "机器人正在另一处使用（409 冲突）")
    }

    func testRateLimitMentionsAutomaticRetry() {
        let notice = MessagingErrorCopy.notice(
            for: "telegram api getUpdates failed: status=429 Too Many Requests error_code=Some(429) description=Too Many Requests: retry after 7",
            platform: .telegram
        )
        XCTAssertEqual(notice.title, "请求过于频繁（Telegram 限流）")
        XCTAssertTrue(notice.hint?.contains("自动重试") == true)
    }

    func testServerErrorMarksTemporaryOutage() {
        let notice = MessagingErrorCopy.notice(
            for: "telegram api getUpdates failed: status=502 Bad Gateway error_code=None description=Bad Gateway",
            platform: .telegram
        )
        XCTAssertEqual(notice.title, "Telegram服务暂时不可用（502）")
        XCTAssertEqual(notice.hint, "官方接口偶发异常，稍后会自动恢复。")
    }

    func testTimeoutNotice() {
        let notice = MessagingErrorCopy.notice(
            for: "telegram getMe timeout",
            platform: .telegram
        )
        XCTAssertEqual(notice.title, "连接Telegram超时")
    }

    func testUnknownErrorsFallBackToRawTextUntouched() {
        for raw in [
            "尚未完成凭据配置",
            "telegram api getMe failed: status=401 Unauthorized error_code=Some(401) description=Unauthorized",
        ] {
            let notice = MessagingErrorCopy.notice(for: raw, platform: .telegram)
            XCTAssertEqual(notice.title, raw)
            XCTAssertNil(notice.hint)
        }
    }

    func testStatusParsingStopsAtFirstNonDigit() {
        // error_code 里的数字不能混进状态码。
        let notice = MessagingErrorCopy.notice(
            for: "telegram api getUpdates failed: status=429 Too Many Requests error_code=Some(429)",
            platform: .telegram
        )
        XCTAssertEqual(notice.title, "请求过于频繁（Telegram 限流）")
    }

    // MARK: - 管理接口错误（保存设置、添加账号、额度刷新）

    func testManagementTransportErrorGainsProxyHint() {
        // daemon 的令牌验证接口在代理不通时返回的原始串。
        let message = MessagingErrorCopy.managementMessage(
            for: "telegram api getMe request failed: error sending request for url (https://api.telegram.org/bot***/getMe)"
        )
        XCTAssertEqual(message, "无法连接Telegram服务器。通常是网络不通或代理未生效。检查「设置 → 出站代理」是否指向可用的本地代理。")
    }

    func testManagementConflictErrorExplainsSharedBot() {
        let message = MessagingErrorCopy.managementMessage(
            for: "telegram api getUpdates failed: status=409 Conflict error_code=Some(409) description=Conflict: terminated by other getUpdates request"
        )
        XCTAssertTrue(message.contains("机器人正在另一处使用"))
        XCTAssertTrue(message.contains("自己的机器人"))
    }

    func testManagementTimeoutErrorMapsToFriendlyText() {
        let message = MessagingErrorCopy.managementMessage(for: "telegram getMe timeout")
        XCTAssertEqual(message, "连接Telegram超时。网络或代理不稳定，稍后会自动重试；频繁出现请检查「设置 → 出站代理」。")
    }

    func testManagementGenericTransportErrorKeepsProxyHint() {
        let message = MessagingErrorCopy.managementMessage(
            for: "refresh failed: error sending request for url (https://api.example.com/v1/usage)"
        )
        XCTAssertTrue(message.contains("出站代理"))
    }

    func testManagementUnknownErrorPassesThroughUntouched() {
        let raw = "failed to persist config: invalid bot token"
        XCTAssertEqual(MessagingErrorCopy.managementMessage(for: raw), raw)
        XCTAssertEqual(MessagingErrorCopy.managementMessage(for: "  "), "  ")
    }
}
