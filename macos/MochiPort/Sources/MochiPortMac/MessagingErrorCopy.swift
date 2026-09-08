import Foundation

/// Translates a messaging account's raw runtime error into a short title the
/// user can act on at a glance, plus an optional hint line.  The raw string is
/// never altered by this layer's fallback and stays available through
/// tooltips and the daemon log, so support workflows keep working.
enum MessagingErrorCopy {
    struct Notice: Equatable {
        let title: String
        let hint: String?
    }

    static func notice(
        for rawError: String,
        platform: MessagingAccountSummary.Platform
    ) -> Notice {
        let raw = rawError.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !raw.isEmpty else { return Notice(title: rawError, hint: nil) }
        let status = httpStatus(in: raw)

        // Telegram 409: another getUpdates consumer holds the same bot token.
        if status == 409
            || (platform == .telegram && raw.contains("terminated by other getUpdates"))
        {
            return Notice(
                title: "机器人正在另一处使用（409 冲突）",
                hint: "同一个机器人同时被另一台设备或另一个实例接收消息，或设置过 webhook。"
                    + "让每人使用自己的机器人；停掉另一端后会自动恢复。"
            )
        }

        if status == 429 {
            return Notice(
                title: "请求过于频繁（\(platform.title) 限流）",
                hint: "已触发\(platform.title)限流，服务会按官方要求的间隔自动重试，无需处理。"
            )
        }

        if let status, (500...599).contains(status) {
            return Notice(
                title: "\(platform.title)服务暂时不可用（\(status)）",
                hint: "官方接口偶发异常，稍后会自动恢复。"
            )
        }

        // Transport-level failure: the request never reached the server.
        // This is the signature users hit when the outbound proxy is
        // missing or dead, so the hint points straight at the setting.
        if raw.contains("error sending request") {
            return Notice(
                title: "无法连接\(platform.title)服务器",
                hint: "通常是网络不通或代理未生效。检查「设置 → 出站代理」是否指向可用的本地代理。"
            )
        }

        if raw.contains("timed out") || raw.contains("timeout") {
            return Notice(
                title: "连接\(platform.title)超时",
                hint: "网络或代理不稳定，稍后会自动重试；频繁出现请检查「设置 → 出站代理」。"
            )
        }

        return Notice(title: raw, hint: nil)
    }

    /// 管理接口（保存设置、添加账号、额度刷新等）返回的原始错误串 →
    /// 用户可读消息。识别不了的错误原样返回，不做破坏；daemon 日志
    /// 始终保留原始串供排查。
    static func managementMessage(for rawError: String) -> String {
        let raw = rawError.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !raw.isEmpty else { return rawError }

        let lower = raw.lowercased()
        let platform: MessagingAccountSummary.Platform
        if lower.contains("feishu") {
            platform = .feishu
        } else if lower.contains("wecom") {
            platform = .wecom
        } else if lower.contains("wechat") {
            platform = .wechat
        } else if lower.contains("telegram") {
            platform = .telegram
        } else if raw.contains("error sending request") {
            return "无法连接对应服务：通常是网络不通或代理未生效。请检查「设置 → 出站代理」是否指向可用的本地代理。"
        } else {
            return raw
        }

        let notice = notice(for: raw, platform: platform)
        guard let hint = notice.hint else { return notice.title }
        return "\(notice.title)。\(hint)"
    }

    /// Extracts the leading integer after `status=` — the daemon formats
    /// upstream HTTP failures as `... failed: status=409 Conflict ...`.
    private static func httpStatus(in raw: String) -> Int? {
        guard let range = raw.range(of: "status=") else { return nil }
        var digits = ""
        for character in raw[range.upperBound...] {
            guard character.isNumber else { break }
            digits.append(character)
        }
        guard let first = digits.first, first != "0", let status = Int(digits) else { return nil }
        return status
    }
}
