import Foundation

public enum ServiceID: String, CaseIterable, Codable, Sendable, Identifiable {
    case codex
    public var id: String { rawValue }
    public var displayName: String { "Codex" }
}

public struct LimitWindow: Equatable, Sendable {
    public enum Kind: String, Sendable, CaseIterable {
        case session5h, weekly, daily
        public var label: String {
            switch self {
            case .session5h: return "5h"
            case .weekly: return "每周"
            case .daily: return "每日"
            }
        }
    }
    public let kind: Kind
    public let usedPercent: Double // 0...100
    public let resetsAt: Date?
    public init(kind: Kind, usedPercent: Double, resetsAt: Date?) {
        self.kind = kind
        self.usedPercent = usedPercent
        self.resetsAt = resetsAt
    }
}

/// Cumulative Codex usage reported in `total_token_usage`.
///
/// The snapshot is never summed. It only distinguishes a verbatim turn replay
/// from a genuinely separate turn that happens to have the same per-turn
/// usage.
public struct CodexCumulativeUsage: Equatable, Hashable, Sendable {
    public let inputTokens: Int
    public let cachedInputTokens: Int
    public let outputTokens: Int
    public let reasoningOutputTokens: Int
    public let totalTokens: Int

    public init(inputTokens: Int, cachedInputTokens: Int, outputTokens: Int,
                reasoningOutputTokens: Int, totalTokens: Int) {
        self.inputTokens = inputTokens
        self.cachedInputTokens = cachedInputTokens
        self.outputTokens = outputTokens
        self.reasoningOutputTokens = reasoningOutputTokens
        self.totalTokens = totalTokens
    }
}

/// Model identity normalization shared by parsing, replay de-duplication and
/// pricing. This mirrors ai-token-monitor's spelling and gateway-prefix rules.
public enum CodexModel {
    private static let vendorPrefixes = [
        "anthropic", "openai", "azure", "bedrock", "vertex", "google", "xai",
        "moonshot", "zhipu", "kiro", "litellm", "omniroute", "openrouter",
    ]
    private static let recognizableFamilies = [
        "claude", "opus", "sonnet", "haiku", "fable", "mythos",
        "gpt", "codex", "o3-", "o4-", "grok", "kimi", "moonshot", "glm",
        "gemini", "deepseek", "qwen", "llama", "mistral",
    ]

    static func canonical(_ model: String) -> String {
        String(model.lowercased().map { character in
            character == "." || character == "_" ? "-" : character
        })
    }

    public static func normalize(_ model: String) -> String {
        let canonical = canonical(model)
        let withoutPath = canonical.split(separator: "/", omittingEmptySubsequences: false)
            .last.map(String.init) ?? canonical
        for vendor in vendorPrefixes {
            let prefix = vendor + "-"
            guard withoutPath.hasPrefix(prefix) else { continue }
            let remainder = String(withoutPath.dropFirst(prefix.count))
            if recognizableFamilies.contains(where: remainder.contains) {
                return remainder
            }
        }
        return withoutPath
    }
}

public struct TokenEvent: Equatable, Sendable {
    public let service: ServiceID
    public let timestamp: Date
    public let model: String
    public let inputTokens: Int
    public let outputTokens: Int
    public let cacheReadTokens: Int
    public let cacheCreationTokens: Int
    /// Raw `input_tokens` as Codex reported it, including cached input.
    public let reportedInputTokens: Int
    /// Raw per-turn `total_tokens`. Compaction can make this differ from a
    /// reconstruction based on the component columns, so it must be retained.
    public let reportedTotalTokens: Int
    /// `session_meta.payload.id`, used only for replay de-duplication.
    public let sessionID: String?
    /// Cumulative session usage used only as a replay discriminator.
    public let cumulativeUsage: CodexCumulativeUsage?
    /// 이벤트가 발생한 프로젝트 (cwd lastPathComponent). nil = 미파악.
    public let project: String?
    /// Session provider/source (for example `ai-gateway`, `custom`, or
    /// `sub2api`). This is a Codex configuration name, not a billing
    /// identity; older logs do not carry it and use `legacy`.
    public let source: String
    public init(service: ServiceID, timestamp: Date, model: String,
                inputTokens: Int, outputTokens: Int, cacheReadTokens: Int, cacheCreationTokens: Int,
                project: String? = nil, source: String = "legacy",
                reportedInputTokens: Int? = nil, reportedTotalTokens: Int? = nil,
                sessionID: String? = nil, cumulativeUsage: CodexCumulativeUsage? = nil) {
        self.service = service
        self.timestamp = timestamp
        self.model = CodexModel.normalize(model)
        self.inputTokens = inputTokens
        self.outputTokens = outputTokens
        self.cacheReadTokens = cacheReadTokens
        self.cacheCreationTokens = cacheCreationTokens
        self.reportedInputTokens = reportedInputTokens ?? (inputTokens + cacheReadTokens)
        // Legacy/synthetic callers did not carry the reported total. Keep
        // their previous request-token meaning while real Codex events always
        // provide the explicit log value.
        self.reportedTotalTokens = reportedTotalTokens ?? (inputTokens + outputTokens)
        self.sessionID = sessionID
        self.cumulativeUsage = cumulativeUsage
        self.project = project
        let normalizedSource = source.trimmingCharacters(in: .whitespacesAndNewlines)
        self.source = normalizedSource.isEmpty ? "legacy" : normalizedSource
    }

    /// Compatibility spelling for callers that call the field a provider.
    public var provider: String { source }
    /// Primary usage value shown by ai-token-monitor: Codex's reported
    /// per-turn `total_tokens`, including compaction totals exactly as logged.
    public var requestTokens: Int { reportedTotalTokens }

    /// All tokens processed by the model, including cache reads/writes.
    /// This is intentionally kept for callers that need a context-volume
    /// metric; it is not the default usage number shown by AIGlass.
    public var contextTokens: Int {
        inputTokens + outputTokens + cacheReadTokens + cacheCreationTokens
    }

    /// Primary usage metric used by the dashboard.
    ///
    /// Keep the historical property name for source compatibility.
    public var totalTokens: Int { requestTokens }
}
