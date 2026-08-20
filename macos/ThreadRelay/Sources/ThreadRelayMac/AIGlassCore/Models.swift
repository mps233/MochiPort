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

public struct TokenEvent: Equatable, Sendable {
    public let service: ServiceID
    public let timestamp: Date
    public let model: String
    public let inputTokens: Int
    public let outputTokens: Int
    public let cacheReadTokens: Int
    public let cacheCreationTokens: Int
    /// 이벤트가 발생한 프로젝트 (cwd lastPathComponent). nil = 미파악.
    public let project: String?
    public init(service: ServiceID, timestamp: Date, model: String,
                inputTokens: Int, outputTokens: Int, cacheReadTokens: Int, cacheCreationTokens: Int,
                project: String? = nil) {
        self.service = service
        self.timestamp = timestamp
        self.model = model
        self.inputTokens = inputTokens
        self.outputTokens = outputTokens
        self.cacheReadTokens = cacheReadTokens
        self.cacheCreationTokens = cacheCreationTokens
        self.project = project
    }
    public var totalTokens: Int { inputTokens + outputTokens + cacheReadTokens + cacheCreationTokens }
}
