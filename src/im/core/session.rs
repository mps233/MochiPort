use std::collections::HashMap;

use tauri::{AppHandle, Manager};
use tokio::sync::Mutex;

use super::types::{ImChannelKind, InboundMessage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImSessionBinding {
    pub conversation_key: String,
    pub channel: ImChannelKind,
    pub account_id: String,
    pub sender_id: String,
    pub chat_id: String,
    pub last_message_id: Option<String>,
    pub thread_id: Option<String>,
}

impl ImSessionBinding {
    pub fn from_inbound(message: &InboundMessage) -> Self {
        Self {
            conversation_key: message.conversation_key(),
            channel: message.channel,
            account_id: message.account_id.clone(),
            sender_id: message.sender_id.clone(),
            chat_id: message.chat_id.clone(),
            last_message_id: Some(message.message_id.clone()),
            thread_id: None,
        }
    }
}

#[derive(Default)]
pub struct ImSessionStore {
    bindings: HashMap<String, ImSessionBinding>,
}

#[derive(Default)]
pub struct ImSessionState {
    pub(crate) inner: Mutex<ImSessionStore>,
}

impl ImSessionStore {
    pub fn upsert_from_inbound(&mut self, message: &InboundMessage) -> &ImSessionBinding {
        let key = message.conversation_key();
        self.bindings
            .entry(key)
            .and_modify(|binding| {
                binding.channel = message.channel;
                binding.account_id = message.account_id.clone();
                binding.sender_id = message.sender_id.clone();
                binding.chat_id = message.chat_id.clone();
                binding.last_message_id = Some(message.message_id.clone());
            })
            .or_insert_with(|| ImSessionBinding::from_inbound(message))
    }

    pub fn bind_thread(&mut self, conversation_key: &str, thread_id: Option<String>) {
        if let Some(binding) = self.bindings.get_mut(conversation_key) {
            binding.thread_id = thread_id;
        }
    }

    pub fn upsert_route_binding(
        &mut self,
        channel: ImChannelKind,
        account_id: &str,
        sender_id: &str,
        chat_id: &str,
    ) -> &mut ImSessionBinding {
        let conversation_key = format!("{}:{}:{}", channel.as_str(), account_id, chat_id);
        self.bindings
            .entry(conversation_key.clone())
            .and_modify(|binding| {
                binding.channel = channel;
                binding.account_id = account_id.to_string();
                binding.sender_id = sender_id.to_string();
                binding.chat_id = chat_id.to_string();
            })
            .or_insert_with(|| ImSessionBinding {
                conversation_key,
                channel,
                account_id: account_id.to_string(),
                sender_id: sender_id.to_string(),
                chat_id: chat_id.to_string(),
                last_message_id: None,
                thread_id: None,
            })
    }

    pub fn get_by_thread_id(&self, thread_id: &str) -> Option<&ImSessionBinding> {
        self.bindings
            .values()
            .find(|binding| binding.thread_id.as_deref() == Some(thread_id))
    }

    pub fn find_binding(
        &self,
        channel: ImChannelKind,
        account_id: &str,
        chat_id: &str,
    ) -> Option<&ImSessionBinding> {
        self.bindings.values().find(|binding| {
            binding.channel == channel
                && binding.account_id == account_id
                && binding.chat_id == chat_id
        })
    }
}

pub fn build_conversation_key(channel: ImChannelKind, account_id: &str, chat_id: &str) -> String {
    format!("{}:{}:{}", channel.as_str(), account_id, chat_id)
}

pub async fn resolve_thread_id_for_chat<R: tauri::Runtime>(
    app: &AppHandle<R>,
    channel: ImChannelKind,
    account_id: &str,
    chat_id: &str,
) -> Option<String> {
    let Some(state) = app.try_state::<ImSessionState>() else {
        return None;
    };
    let store = state.inner.lock().await;
    store
        .find_binding(channel, account_id, chat_id)
        .and_then(|binding| binding.thread_id.clone())
        .filter(|id| !id.trim().is_empty())
}

pub async fn resolve_binding_by_thread<R: tauri::Runtime>(
    app: &AppHandle<R>,
    channel: ImChannelKind,
    thread_id: &str,
) -> Option<ImSessionBinding> {
    let Some(state) = app.try_state::<ImSessionState>() else {
        return None;
    };
    let store = state.inner.lock().await;
    store
        .get_by_thread_id(thread_id)
        .filter(|binding| binding.channel == channel)
        .cloned()
}

pub async fn resolve_conversation_key_by_thread<R: tauri::Runtime>(
    app: &AppHandle<R>,
    channel: ImChannelKind,
    thread_id: &str,
) -> Option<String> {
    resolve_binding_by_thread(app, channel, thread_id)
        .await
        .map(|binding| binding.conversation_key)
}

pub async fn bind_route_to_thread<R: tauri::Runtime>(
    app: &AppHandle<R>,
    channel: ImChannelKind,
    account_id: &str,
    sender_id: &str,
    chat_id: &str,
    thread_id: &str,
) {
    let Some(state) = app.try_state::<ImSessionState>() else {
        return;
    };
    let mut store = state.inner.lock().await;
    let binding = store.upsert_route_binding(channel, account_id, sender_id, chat_id);
    binding.thread_id = Some(thread_id.to_string());
}
