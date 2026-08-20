import Foundation

public struct CodexParsed: Equatable {
    public let timestamp: Date
    public let event: TokenEvent?       // info가 null이면 nil
    public let limits: [LimitWindow]    // primary→session5h, secondary→weekly
}

public enum CodexLogParser {
    /// 홈 디렉토리 경로 주입 (테스트용). nil이면 `NSHomeDirectory()`.
    nonisolated(unsafe) public static var homeDirectoryOverride: String?

    /// session_meta 라인에서 cwd → 프로젝트명을 반환.
    /// cwd가 홈 디렉토리와 정확히 일치하면 "~", 아니면 lastPathComponent.
    /// type != "session_meta" 이거나 파싱 실패 시 nil.
    public static func parseSessionMeta(line: String) -> String? {
        guard !line.isEmpty,
              let data = line.data(using: .utf8),
              let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              obj["type"] as? String == "session_meta",
              let payload = obj["payload"] as? [String: Any],
              let cwd = payload["cwd"] as? String
        else { return nil }
        let home = homeDirectoryOverride ?? NSHomeDirectory()
        if cwd == home { return "~" }
        let last = (cwd as NSString).lastPathComponent
        return last.isEmpty ? nil : last
    }

    public static func parse(line: String) -> CodexParsed? {
        guard !line.isEmpty,
              let data = line.data(using: .utf8),
              let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let payload = obj["payload"] as? [String: Any],
              payload["type"] as? String == "token_count",
              let tsString = obj["timestamp"] as? String,
              let timestamp = ISO8601.date(tsString)
        else { return nil }

        var limits: [LimitWindow] = []
        if let rateLimits = payload["rate_limits"] as? [String: Any] {
            func window(_ key: String, kind: LimitWindow.Kind) -> LimitWindow? {
                guard let w = rateLimits[key] as? [String: Any],
                      let used = (w["used_percent"] as? NSNumber)?.doubleValue else { return nil }
                // 최신 Codex CLI는 절대 epoch 초 `resets_at`, 구버전은 상대 `resets_in_seconds`.
                let resets: Date?
                if let epoch = w["resets_at"] as? NSNumber {
                    resets = Date(timeIntervalSince1970: epoch.doubleValue)
                } else if let inSeconds = w["resets_in_seconds"] as? NSNumber {
                    resets = timestamp.addingTimeInterval(inSeconds.doubleValue)
                } else {
                    resets = nil
                }
                return LimitWindow(kind: kind, usedPercent: used, resetsAt: resets)
            }
            if let p = window("primary", kind: .session5h) { limits.append(p) }
            if let s = window("secondary", kind: .weekly) { limits.append(s) }
        }

        var event: TokenEvent?
        if let info = payload["info"] as? [String: Any],
           let last = info["last_token_usage"] as? [String: Any] {
            func intValue(_ key: String) -> Int { (last[key] as? NSNumber)?.intValue ?? 0 }
            let cached = intValue("cached_input_tokens")
            event = TokenEvent(
                service: .codex,
                timestamp: timestamp,
                model: "gpt-codex",
                inputTokens: max(0, intValue("input_tokens") - cached),
                outputTokens: intValue("output_tokens"),
                cacheReadTokens: cached,
                cacheCreationTokens: 0
            )
        }
        return CodexParsed(timestamp: timestamp, event: event, limits: limits)
    }
}
