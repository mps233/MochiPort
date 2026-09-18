//! Codex usage accounting.
//!
//! One implementation of "read the local Codex rollout logs and work out token
//! usage". Both desktop clients consume this through the management API instead
//! of parsing the logs themselves; see `docs/codex-usage-unification.zh-CN.md`
//! for what each side owns.
//!
//! Layout:
//!
//! - `log` parses individual rollout records;
//! - `scan` walks the sessions tree and accumulates per-session totals;
//! - `cost` prices usage from the built-in table plus user overrides;
//! - `service` rolls sessions up into the summaries the API returns.

pub(crate) mod cost;
pub(crate) mod log;
pub(crate) mod scan;
pub(crate) mod service;
