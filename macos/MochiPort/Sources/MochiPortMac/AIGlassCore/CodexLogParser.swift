import Foundation

public struct CodexParsed: Equatable, Sendable {
    public let timestamp: Date
    public let event: TokenEvent?       // info가 null이면 nil
    public let limits: [LimitWindow]    // primary→session5h, secondary→weekly
}

public struct CodexSessionMeta: Equatable, Sendable {
    public let project: String?
    public let source: String
    public let sessionID: String?

    public var provider: String { source }

    public init(project: String?, source: String = "legacy", sessionID: String? = nil) {
        self.project = project
        let normalized = source.trimmingCharacters(in: .whitespacesAndNewlines)
        self.source = normalized.isEmpty ? "legacy" : normalized
        let normalizedID = sessionID?.trimmingCharacters(in: .whitespacesAndNewlines)
        self.sessionID = normalizedID.flatMap { $0.isEmpty ? nil : $0 }
    }
}

public enum CodexLogParser {
    /// 홈 디렉토리 경로 주입 (테스트용). nil이면 `NSHomeDirectory()`.
    nonisolated(unsafe) public static var homeDirectoryOverride: String?

    /// session_meta 라인에서 cwd → 프로젝트명을 반환.
    /// cwd가 홈 디렉토리와 정확히 일치하면 "~", 아니면 lastPathComponent.
    /// type != "session_meta" 이거나 파싱 실패 시 nil.
    public static func parseSessionMeta(line: String) -> String? {
        parseSessionMetaDetails(line: line)?.project
    }

    /// Parses project and provider/source from a session_meta line. The
    /// provider is stored by Codex under `payload.model_provider`; a few older
    /// clients put it at the top level, so both locations are accepted.
    public static func parseSessionMetaDetails(line: String) -> CodexSessionMeta? {
        guard !line.isEmpty,
              let data = line.data(using: .utf8),
              let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              obj["type"] as? String == "session_meta",
              let payload = obj["payload"] as? [String: Any]
        else { return nil }
        let home = homeDirectoryOverride ?? NSHomeDirectory()
        let project: String?
        if let cwd = payload["cwd"] as? String {
            if cwd == home {
                project = "~"
            } else {
                let last = (cwd as NSString).lastPathComponent
                project = last.isEmpty ? nil : last
            }
        } else {
            project = nil
        }
        let source = (payload["model_provider"] as? String)
            ?? (obj["model_provider"] as? String)
            ?? (payload["provider"] as? String)
            ?? "legacy"
        return CodexSessionMeta(
            project: project,
            source: source,
            sessionID: payload["id"] as? String)
    }

    /// Returns the active model from a `turn_context` record.
    public static func parseTurnContextModel(line: String) -> String? {
        guard !line.isEmpty,
              let data = line.data(using: .utf8),
              let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              obj["type"] as? String == "turn_context",
              let payload = obj["payload"] as? [String: Any],
              let model = payload["model"] as? String,
              !model.isEmpty
        else { return nil }
        return CodexModel.normalize(model)
    }

    public static func parse(
        line: String,
        model: String = "codex",
        fallbackTimestamp: Date? = nil
    ) -> CodexParsed? {
        guard !line.isEmpty,
              let data = line.data(using: .utf8),
              let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let payload = obj["payload"] as? [String: Any],
              payload["type"] as? String == "token_count",
              let timestamp = timestamp(from: obj["timestamp"] as? String)
                ?? fallbackTimestamp
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
           let usage = (info["last_token_usage"] as? [String: Any])
            ?? (info["total_token_usage"] as? [String: Any]) {
            func intValue(_ object: [String: Any], _ key: String) -> Int {
                max(0, (object[key] as? NSNumber)?.intValue ?? 0)
            }
            let reportedInput = intValue(usage, "input_tokens")
            let output = intValue(usage, "output_tokens")
            let cached = intValue(usage, "cached_input_tokens")
            let reportedTotal = (usage["total_tokens"] as? NSNumber)
                .map { max(0, $0.intValue) }
                ?? (reportedInput + output)
            let cumulative = (info["total_token_usage"] as? [String: Any]).map { total in
                CodexCumulativeUsage(
                    inputTokens: intValue(total, "input_tokens"),
                    cachedInputTokens: intValue(total, "cached_input_tokens"),
                    outputTokens: intValue(total, "output_tokens"),
                    reasoningOutputTokens: intValue(total, "reasoning_output_tokens"),
                    totalTokens: intValue(total, "total_tokens"))
            }
            event = TokenEvent(
                service: .codex,
                timestamp: timestamp,
                model: model.isEmpty ? "codex" : model,
                inputTokens: max(0, reportedInput - cached),
                outputTokens: output,
                cacheReadTokens: cached,
                cacheCreationTokens: max(
                    0,
                    intValue(usage, "cache_creation_input_tokens"),
                    intValue(usage, "cache_write_input_tokens"),
                    intValue(usage, "cache_creation_tokens")),
                reportedInputTokens: reportedInput,
                reportedTotalTokens: reportedTotal,
                cumulativeUsage: cumulative
            )
        }
        return CodexParsed(timestamp: timestamp, event: event, limits: limits)
    }

    /// ai-token-monitor first attempts a full timestamp parse, then preserves
    /// the first YYYY-MM-DD substring as a local date. A path-derived date is
    /// supplied by the collector only when both forms are unavailable.
    private static func timestamp(from raw: String?) -> Date? {
        guard let raw else { return nil }
        if let exact = ISO8601.date(raw) { return exact }
        guard raw.count >= 10 else { return nil }
        let prefix = String(raw.prefix(10))
        let formatter = DateFormatter()
        formatter.calendar = Calendar(identifier: .gregorian)
        formatter.timeZone = .current
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyy-MM-dd"
        return formatter.date(from: prefix)
    }
}
