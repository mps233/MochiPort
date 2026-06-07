use anyhow::Result;

use crate::{app_state::SharedState, types::now_ms};

pub(crate) async fn local_bot_tokens(state: &SharedState) -> Vec<String> {
    let config = state.config.lock().await;
    config
        .effective_wechat_accounts()
        .into_iter()
        .flat_map(|account| {
            account
                .bot_token
                .trim()
                .split(',')
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter_map(|value| {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_string())
        })
        .take(10)
        .collect()
}

pub(crate) async fn load_sync_buf(state: &SharedState, account_id: &str) -> String {
    state
        .persisted
        .lock()
        .await
        .wechat
        .sync_buf_by_account
        .get(account_id)
        .cloned()
        .unwrap_or_default()
}

pub(crate) async fn save_sync_buf(
    state: &SharedState,
    account_id: &str,
    sync_buf: String,
) -> Result<()> {
    let mut persisted = state.persisted.lock().await;
    persisted
        .wechat
        .sync_buf_by_account
        .insert(account_id.to_string(), sync_buf);
    let config = state.config.lock().await.clone();
    persisted.save(&config.state_path)
}

pub(crate) async fn remember_context_token(
    state: &SharedState,
    account_id: &str,
    peer_id: &str,
    context_token: &str,
) -> Result<()> {
    let context_token = context_token.trim();
    if context_token.is_empty() {
        return Ok(());
    }
    let mut persisted = state.persisted.lock().await;
    persisted.wechat.context_tokens.insert(
        context_token_key(account_id, peer_id),
        context_token.to_string(),
    );
    persisted
        .wechat
        .context_token_captured_at_ms
        .insert(context_token_key(account_id, peer_id), now_ms());
    let config = state.config.lock().await.clone();
    persisted.save(&config.state_path)
}

pub(crate) async fn context_token_record(
    state: &SharedState,
    account_id: &str,
    peer_id: &str,
) -> Option<WechatContextTokenRecord> {
    let key = context_token_key(account_id, peer_id);
    let persisted = state.persisted.lock().await;
    persisted
        .wechat
        .context_tokens
        .get(&key)
        .cloned()
        .map(|token| WechatContextTokenRecord {
            token,
            captured_at_ms: persisted
                .wechat
                .context_token_captured_at_ms
                .get(&key)
                .copied(),
        })
}

pub(crate) async fn forget_context_token(
    state: &SharedState,
    account_id: &str,
    peer_id: &str,
) -> Result<()> {
    let mut persisted = state.persisted.lock().await;
    persisted
        .wechat
        .context_tokens
        .remove(&context_token_key(account_id, peer_id));
    persisted
        .wechat
        .context_token_captured_at_ms
        .remove(&context_token_key(account_id, peer_id));
    let config = state.config.lock().await.clone();
    persisted.save(&config.state_path)
}

#[derive(Debug, Clone)]
pub(crate) struct WechatContextTokenRecord {
    pub token: String,
    pub captured_at_ms: Option<u128>,
}

impl WechatContextTokenRecord {
    pub(crate) fn age_ms(&self) -> Option<u128> {
        self.captured_at_ms
            .map(|captured_at_ms| now_ms().saturating_sub(captured_at_ms))
    }
}

fn context_token_key(account_id: &str, peer_id: &str) -> String {
    format!("{account_id}:{peer_id}")
}
