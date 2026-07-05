//! 网关路由的进程内可变状态：渠道熔断健康 + 会话粘性绑定。
//!
//! 选择算法本身在 `config.rs` / `router.rs`，是无状态的纯函数；这里只维护
//! 跨请求的两块状态：
//! - `health`：每个 provider（按 `route_id`）的连续失败计数与拉黑截止时间。
//! - `bindings`：每个 session 上次成功使用的 provider，用于粘性路由保 cache。
//!
//! 状态放在 `AppState` 里用 `Mutex` 保护；`route_id` 复用
//! `config::provider_route_id`（name + type + base_url）。

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// 连续失败多少次后拉黑该渠道。
pub const FAILURE_THRESHOLD: u32 = 3;
/// 拉黑冷却时长，冷却后自动恢复可选。
pub const COOLDOWN: Duration = Duration::from_secs(30);
/// 会话绑定的空闲存活时长，超时清理避免 map 无限增长。
pub const BINDING_TTL: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Default, Clone)]
struct ProviderHealth {
    consecutive_failures: u32,
    blacklisted_until: Option<Instant>,
}

#[derive(Debug, Clone)]
struct SessionBinding {
    route_id: String,
    last_used: Instant,
}

/// 网关路由的进程内状态。重启后从零开始重新学习。
#[derive(Debug, Default)]
pub struct GatewayRoutingState {
    health: HashMap<String, ProviderHealth>,
    bindings: HashMap<String, SessionBinding>,
}

impl GatewayRoutingState {
    /// 该 route 当前是否处于拉黑冷却期。
    pub fn is_blacklisted(&self, route_id: &str, now: Instant) -> bool {
        self.health
            .get(route_id)
            .and_then(|h| h.blacklisted_until)
            .is_some_and(|until| now < until)
    }

    /// 记录一次成功：清零失败计数、解除拉黑。
    pub fn record_success(&mut self, route_id: &str) {
        if let Some(h) = self.health.get_mut(route_id) {
            h.consecutive_failures = 0;
            h.blacklisted_until = None;
        }
    }

    /// 记录一次（可熔断的）失败：累加计数，达到阈值则拉黑一段时间。
    pub fn record_failure(&mut self, route_id: &str, now: Instant) {
        let entry = self.health.entry(route_id.to_string()).or_default();
        entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
        if entry.consecutive_failures >= FAILURE_THRESHOLD {
            entry.blacklisted_until = Some(now + COOLDOWN);
        }
    }

    /// 该 session 当前绑定的 route（若未过期）。
    pub fn binding_for(&self, session_id: &str, now: Instant) -> Option<&str> {
        self.bindings.get(session_id).and_then(|b| {
            if now.duration_since(b.last_used) <= BINDING_TTL {
                Some(b.route_id.as_str())
            } else {
                None
            }
        })
    }

    /// 绑定 / 刷新 session 到某个 route。
    pub fn bind(&mut self, session_id: &str, route_id: &str, now: Instant) {
        self.bindings.insert(
            session_id.to_string(),
            SessionBinding {
                route_id: route_id.to_string(),
                last_used: now,
            },
        );
    }

    /// 清理过期的会话绑定。
    pub fn evict_stale(&mut self, now: Instant) {
        self.bindings
            .retain(|_, b| now.duration_since(b.last_used) <= BINDING_TTL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blacklist_after_threshold_then_recovers() {
        let mut state = GatewayRoutingState::default();
        let now = Instant::now();
        for _ in 0..FAILURE_THRESHOLD {
            state.record_failure("r", now);
        }
        assert!(state.is_blacklisted("r", now));
        // 冷却后恢复。
        assert!(!state.is_blacklisted("r", now + COOLDOWN + Duration::from_secs(1)));
    }

    #[test]
    fn success_clears_failures() {
        let mut state = GatewayRoutingState::default();
        let now = Instant::now();
        for _ in 0..FAILURE_THRESHOLD {
            state.record_failure("r", now);
        }
        assert!(state.is_blacklisted("r", now));
        state.record_success("r");
        assert!(!state.is_blacklisted("r", now));
    }

    #[test]
    fn binding_roundtrip_and_ttl() {
        let mut state = GatewayRoutingState::default();
        let now = Instant::now();
        state.bind("sess", "route-a", now);
        assert_eq!(state.binding_for("sess", now), Some("route-a"));
        // 超 TTL 后失效。
        assert_eq!(
            state.binding_for("sess", now + BINDING_TTL + Duration::from_secs(1)),
            None
        );
    }

    #[test]
    fn evict_stale_removes_expired_bindings() {
        let mut state = GatewayRoutingState::default();
        let now = Instant::now();
        state.bind("old", "r", now);
        state.bind("fresh", "r", now + BINDING_TTL);
        state.evict_stale(now + BINDING_TTL + Duration::from_secs(1));
        assert_eq!(state.binding_for("fresh", now + BINDING_TTL), Some("r"));
        assert!(state.bindings.get("old").is_none());
    }
}
