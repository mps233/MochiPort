//! Tests for the Telegram inbound flow, thread routing and settings modules.

#[cfg(test)]
use std::{collections::VecDeque, sync::Mutex};

use anyhow::{Result, anyhow};

use crate::im::core::i18n::ImText;

use super::{
    TelegramThreadRoutingResultDelivery, TelegramThreadRoutingResultSender,
    approval_decision_fallback_text, callback_targets_current_message,
    deliver_telegram_thread_routing_result,
};

struct ScriptedThreadRoutingResultSender {
    results: Mutex<VecDeque<Result<String>>>,
    message_ids: Mutex<Vec<Option<String>>>,
}

impl ScriptedThreadRoutingResultSender {
    fn new(results: Vec<Result<String>>) -> Self {
        Self {
            results: Mutex::new(results.into()),
            message_ids: Mutex::new(Vec::new()),
        }
    }

    fn message_ids(&self) -> Vec<Option<String>> {
        self.message_ids.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl TelegramThreadRoutingResultSender for ScriptedThreadRoutingResultSender {
    async fn send_thread_routing_result(
        &self,
        _target: &str,
        _title: &str,
        _body: &str,
        message_id: Option<&str>,
    ) -> Result<String> {
        self.message_ids
            .lock()
            .unwrap()
            .push(message_id.map(str::to_string));
        self.results
            .lock()
            .unwrap()
            .pop_front()
            .expect("missing scripted Telegram result")
    }
}

#[test]
fn rejects_callback_from_replaced_routing_message() {
    assert!(!callback_targets_current_message(Some("10"), Some("9")));
}

#[test]
fn accepts_current_callback_and_text_replies() {
    assert!(callback_targets_current_message(Some("10"), Some("10")));
    assert!(callback_targets_current_message(Some("10"), None));
}

#[test]
fn approval_failure_fallback_uses_the_localized_decision_label() {
    assert_eq!(
        approval_decision_fallback_text(ImText::zh_cn(), "Yes, proceed"),
        "已提交：允许执行"
    );
}

#[test]
fn command_parser_normalizes_standard_commands_and_legacy_aliases() {
    assert_eq!(
        super::command("/STOP@MochiPort extra"),
        Some("/stop".to_string())
    );
    assert_eq!(super::command("/s"), Some("/s".to_string()));
    assert_eq!(super::command("/q"), Some("/q".to_string()));
    assert_eq!(super::command("/reply"), Some("/reply".to_string()));
    assert_eq!(
        super::command("/REPLY@MochiPort"),
        Some("/reply".to_string())
    );
    assert_eq!(super::command("/1"), Some("/1".to_string()));
    assert_eq!(super::command("status"), None);
    assert_eq!(super::command("/queue hello"), Some("/queue".to_string()));
    assert_eq!(
        super::command("/steer@MochiPort new direction"),
        Some("/steer".to_string())
    );
    assert_eq!(super::command_payload("/queue hello world"), "hello world");
    assert_eq!(super::command_payload("/queue@MochiPort hello"), "hello");
}

#[tokio::test]
async fn final_routing_result_updates_progress_message_without_fallback() {
    let sender = ScriptedThreadRoutingResultSender::new(vec![Ok("10".to_string())]);

    let delivery =
        deliver_telegram_thread_routing_result(&sender, "chat", "title", "body", "10").await;

    assert_eq!(delivery, TelegramThreadRoutingResultDelivery::Updated);
    assert_eq!(sender.message_ids(), vec![Some("10".to_string())]);
}

#[tokio::test]
async fn final_routing_result_falls_back_to_new_message_after_update_failure() {
    let sender = ScriptedThreadRoutingResultSender::new(vec![
        Err(anyhow!("edit failed")),
        Ok("11".to_string()),
    ]);

    let delivery =
        deliver_telegram_thread_routing_result(&sender, "chat", "title", "body", "10").await;

    assert_eq!(
        delivery,
        TelegramThreadRoutingResultDelivery::SentAsNew {
            message_id: "11".to_string(),
            update_error: "edit failed".to_string(),
        }
    );
    assert_eq!(sender.message_ids(), vec![Some("10".to_string()), None]);
}

#[tokio::test]
async fn final_routing_result_swallows_both_delivery_failures() {
    let sender = ScriptedThreadRoutingResultSender::new(vec![
        Err(anyhow!("edit failed")),
        Err(anyhow!("send failed")),
    ]);

    let delivery =
        deliver_telegram_thread_routing_result(&sender, "chat", "title", "body", "10").await;

    assert_eq!(
        delivery,
        TelegramThreadRoutingResultDelivery::Undelivered {
            update_error: "edit failed".to_string(),
            fallback_error: "send failed".to_string(),
        }
    );
    assert_eq!(sender.message_ids(), vec![Some("10".to_string()), None]);
}
