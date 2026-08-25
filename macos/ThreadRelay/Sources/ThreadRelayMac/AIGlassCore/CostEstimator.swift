import Foundation

/// API-equivalent Codex cost using the same table and matching semantics as
/// ai-token-monitor v0.20.5. Rates are USD per million tokens.
public enum CostEstimator {
    struct Rate: Equatable {
        let input: Double
        let output: Double
        let cachedInput: Double
    }

    private struct PricingRoot: Decodable {
        let claude: ProviderPricing
        let codex: ProviderPricing
        let opencode: ProviderPricing?
        let kimi: ProviderPricing?
        let glm: ProviderPricing?
        let grok: ProviderPricing?
    }

    private struct ProviderPricing: Decodable {
        let `default`: String
        let models: [PricingEntry]
    }

    private struct PricingEntry: Decodable {
        let match: String
        let input: Double
        let output: Double
        let cachedInput: Double

        enum CodingKeys: String, CodingKey {
            case match, input, output
            case cachedInput = "cached_input"
        }

        init(from decoder: Decoder) throws {
            let container = try decoder.container(keyedBy: CodingKeys.self)
            match = try container.decode(String.self, forKey: .match)
            input = try container.decode(Double.self, forKey: .input)
            output = try container.decode(Double.self, forKey: .output)
            cachedInput = try container.decodeIfPresent(Double.self, forKey: .cachedInput) ?? 0
        }
    }

    /// Embedded v0.20.5 fallback. First canonicalized substring wins, so
    /// specific variants remain before their broader model families.
    private static let embeddedRates: [(match: String, rate: Rate)] = [
        ("gpt-5.6-sol", Rate(input: 5.00, output: 30.00, cachedInput: 0.50)),
        ("gpt-5.6-terra", Rate(input: 2.50, output: 15.00, cachedInput: 0.25)),
        ("gpt-5.6-luna", Rate(input: 1.00, output: 6.00, cachedInput: 0.10)),
        ("gpt-5.6", Rate(input: 5.00, output: 30.00, cachedInput: 0.50)),
        ("gpt-5.5-pro", Rate(input: 30.00, output: 180.00, cachedInput: 0.0)),
        ("gpt-5.5", Rate(input: 5.00, output: 30.00, cachedInput: 0.50)),
        ("gpt-5.4-pro", Rate(input: 30.00, output: 180.00, cachedInput: 0.0)),
        ("gpt-5.4-nano", Rate(input: 0.20, output: 1.25, cachedInput: 0.02)),
        ("gpt-5.4-mini", Rate(input: 0.75, output: 4.50, cachedInput: 0.075)),
        ("gpt-5.4", Rate(input: 2.50, output: 15.00, cachedInput: 0.25)),
        ("gpt-5.3-codex", Rate(input: 1.75, output: 14.00, cachedInput: 0.175)),
        ("gpt-5.3", Rate(input: 1.75, output: 14.00, cachedInput: 0.175)),
        ("gpt-5.2-codex", Rate(input: 1.75, output: 14.00, cachedInput: 0.175)),
        ("gpt-5.2", Rate(input: 1.25, output: 10.00, cachedInput: 0.125)),
        ("gpt-5.1-codex-max", Rate(input: 1.25, output: 10.00, cachedInput: 0.125)),
        ("gpt-5.1-codex-mini", Rate(input: 0.25, output: 2.00, cachedInput: 0.025)),
        ("gpt-5.1-codex", Rate(input: 1.25, output: 10.00, cachedInput: 0.125)),
        ("gpt-5.1", Rate(input: 0.625, output: 5.00, cachedInput: 0.125)),
        ("gpt-5-codex", Rate(input: 1.25, output: 10.00, cachedInput: 0.125)),
        ("gpt-5-mini", Rate(input: 0.125, output: 1.00, cachedInput: 0.025)),
        ("gpt-5-nano", Rate(input: 0.05, output: 0.40, cachedInput: 0.005)),
        ("gpt-5", Rate(input: 1.25, output: 10.00, cachedInput: 0.125)),
        ("gpt-4.1-mini", Rate(input: 0.40, output: 1.60, cachedInput: 0.10)),
        ("gpt-4.1", Rate(input: 2.00, output: 8.00, cachedInput: 0.50)),
        ("o4-mini", Rate(input: 1.10, output: 4.40, cachedInput: 0.55)),
        ("o3", Rate(input: 0.40, output: 1.60, cachedInput: 0.20)),
        ("codex-mini", Rate(input: 1.50, output: 6.00, cachedInput: 0.025)),
    ]

    /// Unknown models use the pricing.json Codex default, `gpt-5.4`.
    private static let embeddedFallback = Rate(input: 2.50, output: 15.00, cachedInput: 0.25)

    /// ai-token-monitor accepts a complete user pricing document only when it
    /// decodes as the full provider schema. A partial/malformed override never
    /// replaces the embedded table.
    private static let configuration: (rates: [(String, Rate)], fallback: Rate) = {
        let path = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".claude/pricing.json")
        guard let data = try? Data(contentsOf: path),
              let root = try? JSONDecoder().decode(PricingRoot.self, from: data),
              !root.codex.models.isEmpty else {
            return (embeddedRates, embeddedFallback)
        }
        // Touch required/optional providers through full Codable decoding;
        // unknown JSON fields intentionally remain forward-compatible.
        _ = (root.claude, root.opencode, root.kimi, root.glm, root.grok)
        let resolved = root.codex.models.map {
            ($0.match, Rate(input: $0.input, output: $0.output, cachedInput: $0.cachedInput))
        }
        let defaultCanonical = CodexModel.canonical(root.codex.default)
        let fallback = resolved.first {
            CodexModel.canonical($0.0) == defaultCanonical
        }?.1 ?? resolved[0].1
        return (resolved, fallback)
    }()

    static func rate(for model: String) -> Rate {
        let canonical = CodexModel.canonical(model)
        return configuration.rates.first {
            canonical.contains(CodexModel.canonical($0.0))
        }?.1 ?? configuration.fallback
    }

    /// Codex's `input_tokens` includes cached input. `TokenEvent.inputTokens`
    /// is already the uncached portion, so each component is billed exactly
    /// once. Cache creation is not part of the v0.20.5 Codex formula.
    ///
    /// `multiplier` remains source-compatible with older callers but is
    /// intentionally ignored: neither Sub2API rate multipliers nor provider
    /// actual-spend snapshots alter this API-equivalent estimate.
    public static func cost(of event: TokenEvent, multiplier: Double = 1.0) -> Double {
        _ = multiplier
        let rate = rate(for: event.model)
        let perMillion = 1.0 / 1_000_000.0
        return (Double(event.inputTokens) * rate.input
            + Double(event.cacheReadTokens) * rate.cachedInput
            + Double(event.outputTokens) * rate.output) * perMillion
    }

    public static func cost(of events: [TokenEvent], multiplier: Double = 1.0) -> Double {
        _ = multiplier
        return events.reduce(0) { $0 + cost(of: $1) }
    }
}
