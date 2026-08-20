import Foundation

/// 토큰 이벤트의 추정 비용(USD)을 계산한다.
///
/// 주의: 여기 단가는 **API 환산 추정치, 2026-06 기준**이며 실제 구독 실비가 아니다.
/// Codex 구독은 정액이므로, 이 값은 "API로 같은 양을 썼다면 얼마였을까"를
/// 가늠하는 참고치일 뿐이다.
public enum CostEstimator {
    /// USD per million tokens (MTok).
    struct Rate {
        let input: Double
        let output: Double
        let cacheRead: Double
        let cacheCreation: Double
        let longContextThreshold: Int?
        let longContextInputMultiplier: Double
        let longContextOutputMultiplier: Double
    }

    // Sub2API fallback pricing, USD per million tokens. Keep the known Codex
    // families aligned with its current backend table; the generic fallback
    // remains only for model names that cannot be identified locally.
    private static let rates: [(prefix: String, rate: Rate)] = [
        ("gpt-5.6-sol", Rate(input: 5.0, output: 30.0, cacheRead: 0.5, cacheCreation: 6.25,
                              longContextThreshold: 272_000, longContextInputMultiplier: 2.0,
                              longContextOutputMultiplier: 1.5)),
        ("gpt-5.6-terra", Rate(input: 2.0, output: 12.0, cacheRead: 0.2, cacheCreation: 2.5,
                                longContextThreshold: 272_000, longContextInputMultiplier: 2.0,
                                longContextOutputMultiplier: 1.5)),
        ("gpt-5.6-luna", Rate(input: 0.2, output: 1.2, cacheRead: 0.02, cacheCreation: 0.25,
                               longContextThreshold: 272_000, longContextInputMultiplier: 2.0,
                               longContextOutputMultiplier: 1.5)),
        ("gpt-5.4-mini", Rate(input: 0.75, output: 4.5, cacheRead: 0.075, cacheCreation: 0.75,
                               longContextThreshold: nil, longContextInputMultiplier: 1.0,
                               longContextOutputMultiplier: 1.0)),
        ("gpt-5.4-nano", Rate(input: 0.2, output: 1.25, cacheRead: 0.02, cacheCreation: 0.2,
                               longContextThreshold: nil, longContextInputMultiplier: 1.0,
                               longContextOutputMultiplier: 1.0)),
        ("gpt-5.4", Rate(input: 2.5, output: 15.0, cacheRead: 0.25, cacheCreation: 2.5,
                          longContextThreshold: 272_000, longContextInputMultiplier: 2.0,
                          longContextOutputMultiplier: 1.5)),
        ("gpt-5.5", Rate(input: 2.5, output: 15.0, cacheRead: 0.25, cacheCreation: 2.5,
                          longContextThreshold: 272_000, longContextInputMultiplier: 2.0,
                          longContextOutputMultiplier: 1.5)),
        ("gpt-5.2", Rate(input: 1.75, output: 14.0, cacheRead: 0.175, cacheCreation: 1.75,
                          longContextThreshold: nil, longContextInputMultiplier: 1.0,
                          longContextOutputMultiplier: 1.0)),
        ("gpt-5.3-codex", Rate(input: 1.5, output: 12.0, cacheRead: 0.15, cacheCreation: 1.5,
                                longContextThreshold: nil, longContextInputMultiplier: 1.0,
                                longContextOutputMultiplier: 1.0)),
        ("codex", Rate(input: 1.5, output: 12.0, cacheRead: 0.15, cacheCreation: 1.5,
                        longContextThreshold: nil, longContextInputMultiplier: 1.0,
                        longContextOutputMultiplier: 1.0)),
        ("gpt", Rate(input: 1.25, output: 10.0, cacheRead: 0.125, cacheCreation: 1.5625,
                      longContextThreshold: nil, longContextInputMultiplier: 1.0,
                      longContextOutputMultiplier: 1.0)),
    ]

    private static let fallback = Rate(input: 3.0, output: 15.0, cacheRead: 0.3, cacheCreation: 3.75,
                                       longContextThreshold: nil, longContextInputMultiplier: 1.0,
                                       longContextOutputMultiplier: 1.0)

    private static func rate(for model: String) -> Rate {
        let m = model.lowercased()
        // hasPrefix 우선
        for entry in rates where m.hasPrefix(entry.prefix) {
            return entry.rate
        }
        if m.contains("gpt") || m.contains("codex") {
            return Rate(input: 1.25, output: 10.0, cacheRead: 0.125, cacheCreation: 1.5625,
                        longContextThreshold: nil, longContextInputMultiplier: 1.0,
                        longContextOutputMultiplier: 1.0)
        }
        return fallback
    }

    /// 단일 이벤트의 추정 비용(USD). The multiplier is the effective
    /// Sub2API token multiplier when a live billing snapshot is available.
    public static func cost(of event: TokenEvent, multiplier: Double = 1.0) -> Double {
        let r = rate(for: event.model)
        let perToken = 1.0 / 1_000_000.0
        var inputRate = r.input
        var outputRate = r.output
        var cacheReadRate = r.cacheRead
        var cacheCreationRate = r.cacheCreation
        let contextTokens = event.inputTokens + event.cacheReadTokens + event.cacheCreationTokens
        if let threshold = r.longContextThreshold, contextTokens > threshold {
            inputRate *= r.longContextInputMultiplier
            outputRate *= r.longContextOutputMultiplier
            cacheReadRate *= r.longContextInputMultiplier
            cacheCreationRate *= r.longContextInputMultiplier
        }
        let input = Double(event.inputTokens) * inputRate * perToken
        let output = Double(event.outputTokens) * outputRate * perToken
        let cacheRead = Double(event.cacheReadTokens) * cacheReadRate * perToken
        let cacheCreate = Double(event.cacheCreationTokens) * cacheCreationRate * perToken
        let safeMultiplier = multiplier.isFinite ? max(0, multiplier) : 1.0
        return (input + output + cacheRead + cacheCreate) * safeMultiplier
    }

    /// 이벤트 배열의 추정 비용 합계(USD).
    public static func cost(of events: [TokenEvent], multiplier: Double = 1.0) -> Double {
        events.reduce(0) { $0 + cost(of: $1, multiplier: multiplier) }
    }
}
