use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

static JITTER_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Returns `duration` shifted by up to ±`fraction` of its length.
///
/// Periodic timers and retry loops use this so multiple accounts,
/// connections, and daemon restarts do not reconnect in lockstep.
pub fn jittered(duration: Duration, fraction: f64) -> Duration {
    if duration.is_zero() || !(fraction > 0.0) {
        return duration;
    }
    let spread = duration.as_secs_f64() * fraction.clamp(0.0, 1.0);
    let offset = (next_unit() * 2.0 - 1.0) * spread;
    Duration::from_secs_f64((duration.as_secs_f64() + offset).max(0.0))
}

fn next_unit() -> f64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.subsec_nanos() as u64)
        .unwrap_or(0);
    let mut seed = nanos
        ^ JITTER_COUNTER
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15);
    seed ^= seed >> 12;
    seed ^= seed << 25;
    seed ^= seed >> 27;
    let value = seed.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11;
    value as f64 / (1u64 << 53) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jittered_zero_duration_is_unchanged() {
        assert_eq!(jittered(Duration::ZERO, 0.2), Duration::ZERO);
    }

    #[test]
    fn jittered_stays_within_fraction_of_base() {
        let base = Duration::from_secs(10);
        for _ in 0..256 {
            let value = jittered(base, 0.2);
            assert!(value >= Duration::from_secs(8));
            assert!(value <= Duration::from_secs(12));
        }
    }

    #[test]
    fn jittered_covers_both_sides_of_base() {
        let mut above = false;
        let mut below = false;
        for _ in 0..256 {
            if jittered(Duration::from_secs(10), 0.2) > Duration::from_secs(10) {
                above = true;
            } else {
                below = true;
            }
        }
        assert!(above && below);
    }

    #[test]
    fn jittered_negative_fraction_is_ignored() {
        assert_eq!(
            jittered(Duration::from_secs(10), -0.5),
            Duration::from_secs(10)
        );
    }
}
