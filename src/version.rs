pub const PRODUCT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Release builds inject the macOS/Windows/Linux package build number. Local
/// developer builds remain identifiable without pretending to be a release.
pub const BUILD_NUMBER: &str = match option_env!("THREADRELAY_BUILD_NUMBER") {
    Some(value) if !value.is_empty() => value,
    _ => "dev",
};

pub fn build_number() -> Option<u64> {
    BUILD_NUMBER.parse().ok().filter(|value| *value > 0)
}
