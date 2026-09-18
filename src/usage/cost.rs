//! Cost estimation for Codex usage.
//!
//! Prices are per million tokens and match the table the Windows client used
//! (`AI Token Monitor v0.20.5`), so moving the calculation here does not change
//! any displayed figure.
//!
//! Matching is intentionally ordered: the first canonicalized substring wins, so
//! specific variants must precede their broader families (`gpt-5.6-sol` before
//! `gpt-5.6`, `gpt-5.1-codex-mini` before `gpt-5.1-codex`).

use serde::Deserialize;

use super::log::TokenUsage;

/// Per-million-token prices for one model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Pricing {
    pub input: f64,
    pub output: f64,
    pub cached_input: f64,
}

impl Pricing {
    const fn new(input: f64, output: f64, cached_input: f64) -> Self {
        Self {
            input,
            output,
            cached_input,
        }
    }
}

/// Fallback when no known model matches.
const DEFAULT_PRICING: Pricing = Pricing::new(2.50, 15.00, 0.25);

/// Ordered price table; the first canonicalized substring match wins.
const PRICING: &[(&str, Pricing)] = &[
    ("gpt-5.6-sol", Pricing::new(5.00, 30.00, 0.50)),
    ("gpt-5.6-terra", Pricing::new(2.50, 15.00, 0.25)),
    ("gpt-5.6-luna", Pricing::new(1.00, 6.00, 0.10)),
    ("gpt-5.6", Pricing::new(5.00, 30.00, 0.50)),
    ("gpt-5.5-pro", Pricing::new(30.00, 180.00, 0.0)),
    ("gpt-5.5", Pricing::new(5.00, 30.00, 0.50)),
    ("gpt-5.4-pro", Pricing::new(30.00, 180.00, 0.0)),
    ("gpt-5.4-nano", Pricing::new(0.20, 1.25, 0.02)),
    ("gpt-5.4-mini", Pricing::new(0.75, 4.50, 0.075)),
    ("gpt-5.4", Pricing::new(2.50, 15.00, 0.25)),
    ("gpt-5.3-codex", Pricing::new(1.75, 14.00, 0.175)),
    ("gpt-5.3", Pricing::new(1.75, 14.00, 0.175)),
    ("gpt-5.2-codex", Pricing::new(1.75, 14.00, 0.175)),
    ("gpt-5.2", Pricing::new(1.25, 10.00, 0.125)),
    ("gpt-5.1-codex-max", Pricing::new(1.25, 10.00, 0.125)),
    ("gpt-5.1-codex-mini", Pricing::new(0.25, 2.00, 0.025)),
    ("gpt-5.1-codex", Pricing::new(1.25, 10.00, 0.125)),
    ("gpt-5.1", Pricing::new(0.625, 5.00, 0.125)),
    ("gpt-5-codex", Pricing::new(1.25, 10.00, 0.125)),
    ("gpt-5-mini", Pricing::new(0.125, 1.00, 0.025)),
    ("gpt-5-nano", Pricing::new(0.05, 0.40, 0.005)),
    ("gpt-5", Pricing::new(1.25, 10.00, 0.125)),
    ("gpt-4.1-mini", Pricing::new(0.40, 1.60, 0.10)),
    ("gpt-4.1", Pricing::new(2.00, 8.00, 0.50)),
    ("o4-mini", Pricing::new(1.10, 4.40, 0.55)),
    ("o3", Pricing::new(0.40, 1.60, 0.20)),
    ("codex-mini", Pricing::new(1.50, 6.00, 0.025)),
];

/// Vendor prefixes that may prefix a model name in a log (`openai/gpt-5.1`).
const VENDOR_PREFIXES: &[&str] = &[
    "anthropic",
    "openai",
    "azure",
    "bedrock",
    "vertex",
    "google",
    "xai",
    "moonshot",
    "zhipu",
    "kiro",
    "litellm",
    "omniroute",
    "openrouter",
];

/// Lowercases a model name and strips a leading vendor prefix, so both
/// `openai/gpt-5.1` and `gpt-5.1` resolve to the same entry.
pub(crate) fn canonical_model(model: &str) -> String {
    let lowered = model.trim().to_ascii_lowercase();
    let without_prefix = match lowered.split_once('/') {
        Some((vendor, rest)) if VENDOR_PREFIXES.contains(&vendor) => rest.to_owned(),
        _ => lowered,
    };
    without_prefix
}

/// User-supplied price overrides, loaded once.
///
/// Both clients read `~/.claude/pricing.json` and prefer it over the built-in
/// table, so the daemon has to honour the same file; otherwise a machine with
/// custom prices would show one figure locally and another from the daemon.
static USER_PRICING: std::sync::OnceLock<Option<UserPricing>> = std::sync::OnceLock::new();

#[derive(Debug, Deserialize)]
struct UserPricing {
    codex: UserPricingProvider,
}

#[derive(Debug, Deserialize)]
struct UserPricingProvider {
    models: Vec<UserPricingModel>,
}

#[derive(Debug, Deserialize)]
struct UserPricingModel {
    #[serde(rename = "match")]
    pattern: String,
    input: f64,
    output: f64,
    #[serde(default, rename = "cached_input")]
    cached_input: f64,
}

/// Returns the path of the optional user price file.
fn user_pricing_path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        std::path::PathBuf::from(home)
            .join(".claude")
            .join("pricing.json"),
    )
}

/// Loads the user overrides, caching the result for the process lifetime.
///
/// A missing or malformed file is not an error: the built-in table is the
/// documented fallback, and a user editing the file should not see usage break.
fn user_pricing() -> Option<&'static UserPricing> {
    USER_PRICING
        .get_or_init(|| {
            let path = user_pricing_path()?;
            let contents = std::fs::read_to_string(path).ok()?;
            serde_json::from_str(&contents).ok()
        })
        .as_ref()
}

/// Returns the price entry for a model, or the fallback when nothing matches.
///
/// The user table is consulted first, matching the clients' precedence.
pub(crate) fn pricing_for(model: &str) -> Pricing {
    let canonical = canonical_model(model);
    if let Some(user) = user_pricing()
        && let Some(entry) = user
            .codex
            .models
            .iter()
            .find(|entry| canonical.contains(&canonical_model(&entry.pattern)))
    {
        return Pricing {
            input: entry.input,
            output: entry.output,
            cached_input: entry.cached_input,
        };
    }
    PRICING
        .iter()
        .find(|(name, _)| canonical.contains(name))
        .map(|(_, pricing)| *pricing)
        .unwrap_or(DEFAULT_PRICING)
}

/// Estimates the API-equivalent cost of one usage sample, in US dollars.
///
/// Codex reports `input_tokens` **including** the cached portion, while the
/// cached tokens are billed at their own (lower) rate. The uncached remainder is
/// therefore billed once at the input rate and the cached part once at the
/// cached rate — summing `input + cached` would double-charge the cache.
pub(crate) fn cost_of(model: &str, usage: TokenUsage) -> f64 {
    let pricing = pricing_for(model);
    let cached = usage.cached_input_tokens.min(usage.input_tokens);
    let uncached_input = usage.input_tokens.saturating_sub(cached);
    let per_million = 1.0 / 1_000_000.0;
    (uncached_input as f64 * pricing.input
        + cached as f64 * pricing.cached_input
        + usage.output_tokens as f64 * pricing.output)
        * per_million
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u64, cached: u64, output: u64) -> TokenUsage {
        TokenUsage {
            input_tokens: input,
            cached_input_tokens: cached,
            output_tokens: output,
            ..TokenUsage::default()
        }
    }

    #[test]
    fn resolves_specific_variants_before_families() {
        // `gpt-5.1-codex-mini` must not fall through to `gpt-5.1-codex`.
        assert_eq!(pricing_for("gpt-5.1-codex-mini").input, 0.25);
        assert_eq!(pricing_for("gpt-5.1-codex").input, 1.25);
        assert_eq!(pricing_for("gpt-5.6-sol").input, 5.00);
        assert_eq!(pricing_for("gpt-5.6-terra").input, 2.50);
    }

    #[test]
    fn strips_vendor_prefixes_and_case() {
        assert_eq!(canonical_model("openai/gpt-5.1"), "gpt-5.1");
        assert_eq!(canonical_model("OpenAI/GPT-5.1"), "gpt-5.1");
        assert_eq!(canonical_model("  gpt-5.1  "), "gpt-5.1");
        // An unknown vendor is part of the name, not a prefix to drop.
        assert_eq!(canonical_model("acme/gpt-5.1"), "acme/gpt-5.1");
        // The prefixed form resolves through the same table entry.
        assert_eq!(pricing_for("openai/gpt-5.1").input, 0.625);
    }

    /// The same price table is embedded in both clients, because they price usage
    /// locally for the live figures. Copies drift silently: a model added to one
    /// side makes the daemon and the clients disagree about cost, and nothing else
    /// in the build would notice. This asserts all three still match.
    ///
    /// Fixing a failure means updating every copy, not just this one.
    #[test]
    fn client_price_tables_match_the_daemon() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));

        let macos = std::fs::read_to_string(
            root.join("macos/MochiPort/Sources/MochiPortMac/UsageEngine/CostEstimator.swift"),
        )
        .expect("macOS CostEstimator is readable");
        let windows =
            std::fs::read_to_string(root.join("windows/MochiPort/src-tauri/src/codex_usage.rs"))
                .expect("Windows codex_usage is readable");

        let macos_rates = parse_swift_rates(&macos);
        let windows_rates = parse_rust_rates(&windows);
        assert!(
            !macos_rates.is_empty() && !windows_rates.is_empty(),
            "both client tables should parse"
        );

        let daemon: std::collections::BTreeMap<&str, (f64, f64, f64)> = PRICING
            .iter()
            .map(|(name, pricing)| (*name, (pricing.input, pricing.output, pricing.cached_input)))
            .collect();

        for (label, rates) in [("macOS", &macos_rates), ("Windows", &windows_rates)] {
            for (name, daemon_rate) in &daemon {
                let client_rate = rates.get(*name).unwrap_or_else(|| {
                    panic!("{label} is missing a price for `{name}`; add it there too")
                });
                assert_eq!(
                    client_rate, daemon_rate,
                    "{label} prices `{name}` differently from the daemon"
                );
            }
            for name in rates.keys() {
                assert!(
                    daemon.contains_key(name.as_str()),
                    "{label} has a price for `{name}` that the daemon lacks"
                );
            }
        }
    }

    /// Extracts `("model", Rate(input: a, output: b, cachedInput: c))` entries.
    fn parse_swift_rates(source: &str) -> std::collections::BTreeMap<String, (f64, f64, f64)> {
        let mut rates = std::collections::BTreeMap::new();
        for line in source.lines() {
            let Some(rest) = line.trim().strip_prefix("(\"") else {
                continue;
            };
            let Some((name, tail)) = rest.split_once("\", Rate(input: ") else {
                continue;
            };
            let numbers = parse_numbers(tail);
            if numbers.len() == 3 {
                rates.insert(name.to_owned(), (numbers[0], numbers[1], numbers[2]));
            }
        }
        rates
    }

    /// Extracts `("model", CodexPricing::new(a, b, c))` entries.
    fn parse_rust_rates(source: &str) -> std::collections::BTreeMap<String, (f64, f64, f64)> {
        let mut rates = std::collections::BTreeMap::new();
        for line in source.lines() {
            let Some(rest) = line.trim().strip_prefix("(\"") else {
                continue;
            };
            let Some((name, tail)) = rest.split_once("\", CodexPricing::new(") else {
                continue;
            };
            let numbers = parse_numbers(tail);
            if numbers.len() == 3 {
                rates.insert(name.to_owned(), (numbers[0], numbers[1], numbers[2]));
            }
        }
        rates
    }

    /// Pulls the leading decimal numbers out of a rate argument list.
    fn parse_numbers(source: &str) -> Vec<f64> {
        let mut numbers = Vec::new();
        let mut current = String::new();
        for character in source.chars() {
            if character.is_ascii_digit() || character == '.' {
                current.push(character);
            } else if !current.is_empty() {
                numbers.push(current.parse().unwrap_or_default());
                current.clear();
                if numbers.len() == 3 {
                    break;
                }
            }
        }
        if numbers.len() < 3 && !current.is_empty() {
            numbers.push(current.parse().unwrap_or_default());
        }
        numbers
    }

    #[test]
    fn falls_back_for_unknown_models() {
        assert_eq!(pricing_for("some-unlisted-model"), DEFAULT_PRICING);
    }

    /// Without a user file the built-in table decides, and the lookup must not
    /// depend on the file existing.
    #[test]
    fn uses_builtin_prices_when_no_user_file_is_present() {
        // The real `~/.claude/pricing.json` may or may not exist on the machine
        // running the tests; either way a known model resolves to a real price.
        let pricing = pricing_for("gpt-5.1");
        assert!(pricing.input > 0.0);
        assert!(pricing.output > 0.0);
    }

    /// The cached portion is billed at the cached rate and must not also be
    /// charged at the input rate.
    #[test]
    fn charges_cached_tokens_once_at_the_cached_rate() {
        // 1M input of which 1M cached: exactly the cached rate for `gpt-5.1`.
        let all_cached = cost_of("gpt-5.1", usage(1_000_000, 1_000_000, 0));
        assert!((all_cached - 0.125).abs() < 1e-9, "got {all_cached}");

        // Same total input, none cached: exactly the input rate.
        let none_cached = cost_of("gpt-5.1", usage(1_000_000, 0, 0));
        assert!((none_cached - 0.625).abs() < 1e-9, "got {none_cached}");

        // Half and half lands between the two, never above the uncached price.
        let half = cost_of("gpt-5.1", usage(1_000_000, 500_000, 0));
        assert!((half - (0.625 + 0.125) / 2.0).abs() < 1e-9, "got {half}");
    }

    #[test]
    fn bills_output_at_the_output_rate() {
        // 1M output for gpt-5.1 costs 5.00.
        let cost = cost_of("gpt-5.1", usage(0, 0, 1_000_000));
        assert!((cost - 5.0).abs() < 1e-9, "got {cost}");
    }

    /// A malformed record claiming more cached tokens than input must not
    /// underflow or produce a negative charge.
    #[test]
    fn clamps_cached_tokens_to_the_reported_input() {
        let cost = cost_of("gpt-5.1", usage(100, 5_000, 0));
        assert!(cost >= 0.0);
        // Only the reported 100 input tokens are billable, and since the claim
        // exceeds the input, all of them count as cached.
        assert!(
            (cost - 100.0 * 0.125 / 1_000_000.0).abs() < 1e-12,
            "got {cost}"
        );
    }
}
