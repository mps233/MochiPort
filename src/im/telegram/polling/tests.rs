//! Tests for the Telegram polling and Topic lifecycle modules.

use reqwest::StatusCode;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::auto_topic::{
    cwd_is_within_project, find_auto_topic_target, keep_auto_created_topic_if_current,
    retry_auto_topic_resume, should_retry_auto_topic_resume, started_thread_field,
    started_thread_id, telegram_topic_creation_is_current,
};
use super::topic_cleanup::{
    TelegramTopicCleanupTarget, TelegramTopicDeleteOutcome, finish_telegram_topic_cleanup,
    finish_telegram_topic_cleanup_worker_iteration, mark_telegram_topic_bindings_for_cleanup,
    retry_telegram_topic_delete, schedule_reconciled_telegram_topic_cleanup,
    telegram_topic_cleanup_is_required, telegram_topic_delete_retry_delay,
    telegram_topic_delete_should_retry, wait_for_telegram_topic_cleanup_retry_deadline,
    wait_for_telegram_topic_delete_retry,
};
use super::topic_reconcile::{
    TelegramTopicLifecycle, apply_binding_lifecycle,
    commit_telegram_topic_name_to_codex_if_current, persist_telegram_topic_binding_state,
    record_telegram_topic_name_for_codex_sync, should_edit_telegram_topic_name,
    update_telegram_topic_lifecycle,
};
use super::*;
use crate::im::telegram::api::{
    TelegramChat, TelegramForumTopicEdited, TelegramMediaFile, TelegramPhotoSize,
    TelegramStickerFile, TelegramUser,
};

async fn blocking_successful_topic_api(
    account_id: &str,
    bot_token: &str,
) -> (
    TelegramApi,
    tokio::sync::oneshot::Receiver<String>,
    tokio::sync::oneshot::Sender<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock Telegram server");
    let address = listener.local_addr().expect("mock Telegram address");
    let (request_tx, request_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept Topic request");
        let mut request = vec![0_u8; 4_096];
        let count = stream.read(&mut request).await.expect("read Topic request");
        let first_line = String::from_utf8_lossy(&request[..count])
            .lines()
            .next()
            .unwrap_or_default()
            .to_string();
        let _ = request_tx.send(first_line);
        let _ = release_rx.await;
        let body = r#"{"ok":true,"result":true}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .await
            .expect("write Topic response");
    });
    let api = TelegramApi::new(TelegramSettings {
        account_id: account_id.to_string(),
        bot_token: bot_token.to_string(),
        ..Default::default()
    })
    .with_test_api_base(format!("http://{address}"));
    (api, request_rx, release_tx)
}

#[test]
fn archived_topics_delete_immediately_while_missing_topics_keep_the_grace_period() {
    let mut binding = crate::store::TelegramTopicBindingState::default();
    let active_ids = HashSet::new();
    let archived_ids = HashSet::from(["thread-archived".to_string()]);
    assert_eq!(
        update_telegram_topic_lifecycle(
            &mut binding,
            "thread-archived",
            &active_ids,
            &archived_ids,
            1,
            1_000,
        ),
        TelegramTopicLifecycle::Archived
    );
    assert_eq!(binding.codex_state, "archived");
    assert_eq!(binding.archived_at_ms, Some(1_000));

    let mut missing = crate::store::TelegramTopicBindingState::default();
    assert_eq!(
        update_telegram_topic_lifecycle(
            &mut missing,
            "thread-missing",
            &active_ids,
            &HashSet::new(),
            1,
            2_000,
        ),
        TelegramTopicLifecycle::MissingGrace
    );
    assert_eq!(
        update_telegram_topic_lifecycle(
            &mut missing,
            "thread-missing",
            &active_ids,
            &HashSet::new(),
            1,
            2_000 + TELEGRAM_TOPIC_STATE_GRACE.as_millis(),
        ),
        TelegramTopicLifecycle::MissingDelete
    );
}

#[test]
fn binding_lifecycle_fields_are_applied_without_changing_revision_policy() {
    let mut binding = crate::store::TelegramTopicBindingState {
        archived_at_ms: Some(100),
        missing_at_ms: Some(200),
        lifecycle_revision: 7,
        ..Default::default()
    };

    assert!(apply_binding_lifecycle(
        &mut binding,
        TelegramTopicLifecycle::MissingGrace,
        3,
        300,
    ));
    assert_eq!(binding.codex_state, "missing");
    assert_eq!(binding.archived_at_ms, None);
    assert_eq!(binding.missing_at_ms, Some(200));
    assert_eq!(binding.lifecycle_generation, 3);
    assert_eq!(binding.lifecycle_revision, 7);
    assert_eq!(binding.last_checked_at_ms, 300);

    assert!(apply_binding_lifecycle(
        &mut binding,
        TelegramTopicLifecycle::Archived,
        3,
        400,
    ));
    assert_eq!(binding.codex_state, "archived");
    assert_eq!(binding.archived_at_ms, Some(400));
    assert_eq!(binding.missing_at_ms, None);
    assert_eq!(binding.lifecycle_revision, 7);

    assert!(apply_binding_lifecycle(
        &mut binding,
        TelegramTopicLifecycle::Active,
        3,
        500,
    ));
    assert_eq!(binding.codex_state, "active");
    assert_eq!(binding.archived_at_ms, None);
    assert_eq!(binding.missing_at_ms, None);
    assert_eq!(binding.lifecycle_revision, 7);
}

#[test]
fn unchanged_active_topic_names_do_not_consume_mutation_rate_limits() {
    assert!(!should_edit_telegram_topic_name(
        false,
        "Same title",
        "Same title"
    ));
    assert!(should_edit_telegram_topic_name(
        false,
        "Old title",
        "New title"
    ));
    assert!(!should_edit_telegram_topic_name(
        true,
        "Telegram title",
        "Codex title"
    ));
}

#[tokio::test]
async fn delayed_bot_name_echo_consumes_only_its_marker_without_regressing_newer_state() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let mut config = crate::config::AppConfig::default();
    config.state_path = temp_dir.path().join("state.json");
    let state =
        crate::app_state::AppState::new(temp_dir.path().join("config.toml"), config, None, None);
    let api = TelegramApi::new(TelegramSettings {
        account_id: "bot".to_string(),
        bot_token: "token".to_string(),
        ..Default::default()
    });
    let conversation_key = "telegram:bot:-100|topic=42";
    let expected_binding = crate::store::TelegramTopicBindingState {
        thread_id: "thread-name".to_string(),
        codex_title: "A".to_string(),
        topic_name: "A".to_string(),
        last_synced_codex_title: "A".to_string(),
        last_synced_topic_name: "A".to_string(),
        last_checked_at_ms: 123,
        ..Default::default()
    };
    {
        let mut persisted = state.persisted.lock().await;
        persisted
            .im_thread_bindings
            .insert(conversation_key.to_string(), "thread-name".to_string());
        persisted
            .telegram_topic_binding_states
            .insert(conversation_key.to_string(), expected_binding.clone());
    }
    let old_b_token = install_telegram_topic_name_marker(&state, conversation_key, "B").await;
    let new_a_token = install_telegram_topic_name_marker(&state, conversation_key, "A").await;

    assert!(
        handle_forum_topic_service_message(
            &state,
            &api,
            &TelegramMessage {
                message_id: 1,
                message_thread_id: Some(42),
                chat: TelegramChat {
                    id: -100,
                    kind: "supergroup".to_string(),
                    ..Default::default()
                },
                forum_topic_edited: Some(TelegramForumTopicEdited {
                    name: Some("B".to_string()),
                    icon_custom_emoji_id: None,
                }),
                ..Default::default()
            },
        )
        .await
    );

    let persisted = state.persisted.lock().await;
    let binding = &persisted.telegram_topic_binding_states[conversation_key];
    assert_eq!(binding, &expected_binding);
    drop(persisted);
    let markers = state.telegram_topic_name_sync_ops.lock().await;
    assert_eq!(markers[conversation_key].len(), 1);
    assert_eq!(markers[conversation_key][0].name, "A");
    assert_eq!(markers[conversation_key][0].token, new_a_token);
    assert_ne!(markers[conversation_key][0].token, old_b_token);
    drop(markers);
    assert!(
        !state
            .events
            .lock()
            .await
            .iter()
            .any(|event| event.kind == "telegram_topic_name_synced_to_codex")
    );
}

#[tokio::test]
async fn delayed_telegram_to_codex_rpc_cannot_overwrite_a_newer_realtime_name() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let mut config = crate::config::AppConfig::default();
    config.state_path = temp_dir.path().join("state.json");
    let state =
        crate::app_state::AppState::new(temp_dir.path().join("config.toml"), config, None, None);
    let conversation_key = "telegram:bot:-100|topic=45";
    {
        let mut persisted = state.persisted.lock().await;
        persisted
            .im_thread_bindings
            .insert(conversation_key.to_string(), "thread-name".to_string());
        persisted.telegram_topic_binding_states.insert(
            conversation_key.to_string(),
            crate::store::TelegramTopicBindingState {
                thread_id: "thread-name".to_string(),
                codex_title: "A".to_string(),
                topic_name: "A".to_string(),
                last_synced_codex_title: "A".to_string(),
                last_synced_topic_name: "A".to_string(),
                ..Default::default()
            },
        );
    }

    let telegram_b_token = state
        .begin_telegram_topic_name_update(conversation_key)
        .await;
    assert_eq!(
        record_telegram_topic_name_for_codex_sync(
            &state,
            conversation_key,
            "open",
            "B",
            telegram_b_token,
        )
        .await
        .as_deref(),
        Some("thread-name")
    );

    let realtime_c_token = state
        .begin_telegram_topic_name_update(conversation_key)
        .await;
    {
        let mut persisted = state.persisted.lock().await;
        let binding = persisted
            .telegram_topic_binding_states
            .get_mut(conversation_key)
            .expect("binding");
        binding.codex_title = "C".to_string();
        binding.topic_name = "C".to_string();
        binding.last_synced_codex_title = "C".to_string();
        binding.last_synced_topic_name = "C".to_string();
    }
    state
        .finish_telegram_topic_name_update(conversation_key, realtime_c_token)
        .await;

    assert!(
        !commit_telegram_topic_name_to_codex_if_current(
            &state,
            conversation_key,
            "thread-name",
            "B",
            telegram_b_token,
        )
        .await
    );
    let persisted = state.persisted.lock().await;
    let binding = &persisted.telegram_topic_binding_states[conversation_key];
    assert_eq!(binding.codex_title, "C");
    assert_eq!(binding.topic_name, "C");
    assert_eq!(binding.last_synced_codex_title, "C");
    assert_eq!(binding.last_synced_topic_name, "C");
}

#[tokio::test]
async fn clearing_one_failed_marker_preserves_another_marker_with_the_same_name() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let mut config = crate::config::AppConfig::default();
    config.state_path = temp_dir.path().join("state.json");
    let state =
        crate::app_state::AppState::new(temp_dir.path().join("config.toml"), config, None, None);
    let conversation_key = "telegram:bot:-100|topic=43";
    let first = install_telegram_topic_name_marker(&state, conversation_key, "Same").await;
    let second = install_telegram_topic_name_marker(&state, conversation_key, "Same").await;

    clear_telegram_topic_name_marker(&state, conversation_key, second).await;

    let markers = state.telegram_topic_name_sync_ops.lock().await;
    assert_eq!(markers[conversation_key].len(), 1);
    assert_eq!(markers[conversation_key][0].token, first);
}

#[tokio::test]
async fn topic_not_modified_clears_its_echo_marker_immediately() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let mut config = crate::config::AppConfig::default();
    config.state_path = temp_dir.path().join("state.json");
    let state =
        crate::app_state::AppState::new(temp_dir.path().join("config.toml"), config, None, None);
    let conversation_key = "telegram:bot:-100|topic=44";

    assert!(
        run_telegram_topic_edit_with_marker(&state, conversation_key, "Same", || async {
            Ok(TelegramForumTopicEditOutcome::NotModified)
        })
        .await
        .expect("unchanged topic is a confirmed probe")
    );
    assert!(
        !state
            .telegram_topic_name_sync_ops
            .lock()
            .await
            .contains_key(conversation_key)
    );
}

#[test]
fn topic_name_marker_ttl_outlasts_the_polling_conflict_backoff() {
    assert!(
        TELEGRAM_TOPIC_NAME_MARKER_TTL > Duration::from_secs(TELEGRAM_CONFLICT_BACKOFF_SECONDS)
    );
}

#[test]
fn topic_delete_retry_honors_server_retry_after_and_caps_generic_backoff() {
    let server_error = anyhow::Error::new(TelegramApiError {
        method: "deleteForumTopic".to_string(),
        status: StatusCode::TOO_MANY_REQUESTS,
        error_code: Some(429),
        description: "Too Many Requests".to_string(),
        retry_after: Some(43),
    });
    assert_eq!(
        telegram_topic_delete_retry_delay(Some(&server_error), 1),
        Duration::from_secs(43)
    );
    let zero_retry = anyhow::Error::new(TelegramApiError {
        method: "deleteForumTopic".to_string(),
        status: StatusCode::TOO_MANY_REQUESTS,
        error_code: Some(429),
        description: "Too Many Requests".to_string(),
        retry_after: Some(0),
    });
    assert_eq!(
        telegram_topic_delete_retry_delay(Some(&zero_retry), 1),
        Duration::from_secs(1)
    );
    assert_eq!(
        telegram_topic_delete_retry_delay(None, 1),
        Duration::from_secs(5)
    );
    assert_eq!(
        telegram_topic_delete_retry_delay(None, 10),
        Duration::from_secs(60)
    );

    let forbidden = anyhow::Error::new(TelegramApiError {
        method: "deleteForumTopic".to_string(),
        status: StatusCode::FORBIDDEN,
        error_code: Some(403),
        description: "not enough rights".to_string(),
        retry_after: None,
    });
    assert!(!telegram_topic_delete_should_retry(&forbidden));
    let bad_request = anyhow::Error::new(TelegramApiError {
        method: "deleteForumTopic".to_string(),
        status: StatusCode::BAD_REQUEST,
        error_code: Some(400),
        description: "chat not found".to_string(),
        retry_after: None,
    });
    assert!(!telegram_topic_delete_should_retry(&bad_request));
    let server_failure = anyhow::Error::new(TelegramApiError {
        method: "deleteForumTopic".to_string(),
        status: StatusCode::BAD_GATEWAY,
        error_code: Some(502),
        description: "bad gateway".to_string(),
        retry_after: None,
    });
    assert!(telegram_topic_delete_should_retry(&server_failure));
    let http_timeout = anyhow::Error::new(TelegramApiError {
        method: "deleteForumTopic".to_string(),
        status: StatusCode::REQUEST_TIMEOUT,
        error_code: Some(400),
        description: "request timeout".to_string(),
        retry_after: None,
    });
    assert!(telegram_topic_delete_should_retry(&http_timeout));
    let api_timeout = anyhow::Error::new(TelegramApiError {
        method: "deleteForumTopic".to_string(),
        status: StatusCode::OK,
        error_code: Some(408),
        description: "request timeout".to_string(),
        retry_after: None,
    });
    assert!(telegram_topic_delete_should_retry(&api_timeout));
    let api_server_failure = anyhow::Error::new(TelegramApiError {
        method: "deleteForumTopic".to_string(),
        status: StatusCode::OK,
        error_code: Some(500),
        description: "internal error".to_string(),
        retry_after: None,
    });
    assert!(telegram_topic_delete_should_retry(&api_server_failure));
    assert!(telegram_topic_delete_should_retry(&anyhow!(
        "network unavailable"
    )));
}

#[tokio::test]
async fn every_topic_mutation_records_retry_after_before_releasing_the_gate() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let mut config = crate::config::AppConfig::default();
    config.state_path = temp_dir.path().join("state.json");
    let state =
        crate::app_state::AppState::new(temp_dir.path().join("config.toml"), config, None, None);
    let observed_at = Instant::now();

    let error = run_telegram_topic_mutation(&state, "account", || async {
        Err::<(), _>(anyhow::Error::new(TelegramApiError {
            method: "editForumTopic".to_string(),
            status: StatusCode::TOO_MANY_REQUESTS,
            error_code: Some(429),
            description: "Too Many Requests".to_string(),
            retry_after: Some(43),
        }))
    })
    .await
    .expect_err("rate limit is returned to the caller");
    assert!(error.to_string().contains("Too Many Requests"));
    assert!(
        state
            .telegram_topic_cleanup_retry_deadline("account")
            .await
            .is_some_and(|deadline| deadline >= observed_at + Duration::from_secs(43))
    );

    let fallback_at = Instant::now();
    let _ = run_telegram_topic_mutation(&state, "fallback", || async {
        Err::<(), _>(anyhow::Error::new(TelegramApiError {
            method: "createForumTopic".to_string(),
            status: StatusCode::TOO_MANY_REQUESTS,
            error_code: Some(429),
            description: "Too Many Requests".to_string(),
            retry_after: None,
        }))
    })
    .await;
    assert!(
        state
            .telegram_topic_cleanup_retry_deadline("fallback")
            .await
            .is_some_and(|deadline| {
                deadline >= fallback_at + Duration::from_secs(TELEGRAM_TOPIC_DELETE_RETRY_SECONDS)
            })
    );
}

#[tokio::test]
async fn final_delete_attempt_still_records_its_account_cooldown() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let mut config = crate::config::AppConfig::default();
    config.state_path = temp_dir.path().join("state.json");
    let state =
        crate::app_state::AppState::new(temp_dir.path().join("config.toml"), config, None, None);
    let observed_at = Instant::now();

    let error = retry_telegram_topic_delete(
        1,
        || {
            run_telegram_topic_mutation_while(
                &state,
                "account",
                None,
                || std::future::ready(true),
                || async {
                    Err::<bool, _>(anyhow::Error::new(TelegramApiError {
                        method: "deleteForumTopic".to_string(),
                        status: StatusCode::TOO_MANY_REQUESTS,
                        error_code: Some(429),
                        description: "Too Many Requests".to_string(),
                        retry_after: Some(17),
                    }))
                },
            )
        },
        |_| std::future::ready(()),
    )
    .await
    .expect_err("final rate-limited attempt is returned");

    assert!(error.to_string().contains("Too Many Requests"));
    assert!(
        state
            .telegram_topic_cleanup_retry_deadline("account")
            .await
            .is_some_and(|deadline| deadline >= observed_at + Duration::from_secs(17))
    );
}

#[tokio::test]
async fn generic_delete_retry_backoff_does_not_hold_the_account_gate() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let mut config = crate::config::AppConfig::default();
    config.state_path = temp_dir.path().join("state.json");
    let state =
        crate::app_state::AppState::new(temp_dir.path().join("config.toml"), config, None, None);
    let attempts = std::cell::Cell::new(0_usize);

    let outcome = retry_telegram_topic_delete(
        2,
        || {
            let attempt = attempts.get() + 1;
            attempts.set(attempt);
            run_telegram_topic_mutation_while(
                &state,
                "account",
                None,
                || std::future::ready(true),
                move || async move { Ok(attempt > 1) },
            )
        },
        |_| async {
            let gate = state.telegram_topic_mutation_gate("account").await;
            assert!(gate.try_lock().is_ok());
        },
    )
    .await
    .expect("second attempt confirms deletion");

    assert_eq!(outcome, TelegramTopicDeleteOutcome::Deleted);
    assert_eq!(attempts.get(), 2);
}

#[tokio::test]
async fn topic_mutation_waits_for_cooldown_without_holding_the_account_gate() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let mut config = crate::config::AppConfig::default();
    config.state_path = temp_dir.path().join("state.json");
    let state =
        crate::app_state::AppState::new(temp_dir.path().join("config.toml"), config, None, None);
    state
        .extend_telegram_topic_cleanup_retry_deadline("account", Duration::from_millis(80))
        .await;
    let executed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let task = tokio::spawn({
        let state = state.clone();
        let executed = executed.clone();
        async move {
            run_telegram_topic_mutation(&state, "account", || async move {
                executed.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })
            .await
        }
    });
    tokio::task::yield_now().await;

    let gate = state.telegram_topic_mutation_gate("account").await;
    let guard = tokio::time::timeout(Duration::from_millis(30), gate.lock())
        .await
        .expect("cooldown waiter does not hold mutation gate");
    tokio::time::sleep(Duration::from_millis(90)).await;
    state
        .extend_telegram_topic_cleanup_retry_deadline("account", Duration::from_millis(80))
        .await;
    drop(guard);

    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!executed.load(std::sync::atomic::Ordering::SeqCst));
    tokio::time::timeout(Duration::from_millis(250), task)
        .await
        .expect("mutation observes the extended deadline")
        .expect("mutation task")
        .expect("mutation succeeds");
    assert!(executed.load(std::sync::atomic::Ordering::SeqCst));
}

#[tokio::test]
async fn cleanup_cancels_while_queued_for_the_account_mutation_gate() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let mut config = crate::config::AppConfig::default();
    config.state_path = temp_dir.path().join("state.json");
    let state =
        crate::app_state::AppState::new(temp_dir.path().join("config.toml"), config, None, None);
    let gate = state.telegram_topic_mutation_gate("account").await;
    let guard = gate.lock().await;
    let should_continue = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let executed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let notifier = Arc::new(tokio::sync::Notify::new());
    let task = tokio::spawn({
        let state = state.clone();
        let should_continue = should_continue.clone();
        let executed = executed.clone();
        let notifier = notifier.clone();
        async move {
            run_telegram_topic_mutation_while(
                &state,
                "account",
                Some(&notifier),
                || std::future::ready(should_continue.load(std::sync::atomic::Ordering::SeqCst)),
                || async move {
                    executed.store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok(())
                },
            )
            .await
        }
    });
    tokio::task::yield_now().await;
    should_continue.store(false, std::sync::atomic::Ordering::SeqCst);
    notifier.notify_one();

    let result = tokio::time::timeout(Duration::from_millis(100), task)
        .await
        .expect("lifecycle notification cancels gate wait")
        .expect("mutation task");
    assert!(result.is_none());
    assert!(!executed.load(std::sync::atomic::Ordering::SeqCst));
    drop(guard);
}

#[tokio::test]
async fn topic_delete_retry_loop_waits_full_retry_after_before_success() {
    let attempts = std::cell::Cell::new(0_usize);
    let delays = std::cell::RefCell::new(Vec::new());

    let outcome = retry_telegram_topic_delete(
        3,
        || {
            let attempt = attempts.get() + 1;
            attempts.set(attempt);
            std::future::ready(Some(if attempt == 1 {
                Err(anyhow::Error::new(TelegramApiError {
                    method: "deleteForumTopic".to_string(),
                    status: StatusCode::TOO_MANY_REQUESTS,
                    error_code: Some(429),
                    description: "Too Many Requests".to_string(),
                    retry_after: Some(43),
                }))
            } else {
                Ok(true)
            }))
        },
        |retry| {
            delays.borrow_mut().push(retry.delay);
            std::future::ready(())
        },
    )
    .await
    .expect("delete succeeds after retry");

    assert_eq!(outcome, TelegramTopicDeleteOutcome::Deleted);
    assert_eq!(attempts.get(), 2);
    assert_eq!(*delays.borrow(), vec![Duration::from_secs(43)]);
}

#[tokio::test]
async fn topic_delete_retry_loop_is_bounded_and_stops_on_permanent_error() {
    let false_attempts = std::cell::Cell::new(0_usize);
    let false_waits = std::cell::Cell::new(0_usize);
    let error = retry_telegram_topic_delete(
        3,
        || {
            false_attempts.set(false_attempts.get() + 1);
            std::future::ready(Some(Ok(false)))
        },
        |_| {
            false_waits.set(false_waits.get() + 1);
            std::future::ready(())
        },
    )
    .await
    .expect_err("false result remains unconfirmed");
    assert!(error.to_string().contains("after 3 attempts"));
    assert_eq!(false_attempts.get(), 3);
    assert_eq!(false_waits.get(), 2);

    let permanent_attempts = std::cell::Cell::new(0_usize);
    let permanent_waits = std::cell::Cell::new(0_usize);
    let error = retry_telegram_topic_delete(
        3,
        || {
            permanent_attempts.set(permanent_attempts.get() + 1);
            std::future::ready(Some(Err(anyhow::Error::new(TelegramApiError {
                method: "deleteForumTopic".to_string(),
                status: StatusCode::FORBIDDEN,
                error_code: Some(403),
                description: "not enough rights".to_string(),
                retry_after: None,
            }))))
        },
        |_| {
            permanent_waits.set(permanent_waits.get() + 1);
            std::future::ready(())
        },
    )
    .await
    .expect_err("permanent error is returned immediately");
    assert!(error.to_string().contains("not enough rights"));
    assert_eq!(permanent_attempts.get(), 1);
    assert_eq!(permanent_waits.get(), 0);
}

#[tokio::test]
async fn unarchive_notification_interrupts_a_topic_delete_retry_delay() {
    let notifier = tokio::sync::Notify::new();
    notifier.notify_one();

    tokio::time::timeout(
        Duration::from_millis(100),
        wait_for_telegram_topic_delete_retry(Duration::from_secs(60), Some(&notifier)),
    )
    .await
    .expect("stored notification cancels retry wait");
}

#[tokio::test]
async fn cleanup_worker_replays_a_newer_archive_revision_without_teardown_aba() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let mut config = crate::config::AppConfig::default();
    config.state_path = temp_dir.path().join("state.json");
    let state =
        crate::app_state::AppState::new(temp_dir.path().join("config.toml"), config, None, None);
    let generation = state.runtime.lock().await.start_bridge_generation();
    let route = RouteTarget {
        platform: ImPlatformKind::Telegram,
        conversation_key: "telegram:telegram:-100|topic=47".to_string(),
        account_id: "telegram".to_string(),
        chat_id: "-100|topic=47".to_string(),
        remote_client_key: "im:telegram:test".to_string(),
    };
    {
        let mut persisted = state.persisted.lock().await;
        persisted.im_thread_bindings.insert(
            route.conversation_key.clone(),
            "thread-rearchive".to_string(),
        );
        persisted.telegram_topic_binding_states.insert(
            route.conversation_key.clone(),
            crate::store::TelegramTopicBindingState {
                thread_id: "thread-rearchive".to_string(),
                codex_state: "archived".to_string(),
                lifecycle_generation: generation,
                lifecycle_revision: 2,
                ..Default::default()
            },
        );
    }
    let notifier = Arc::new(tokio::sync::Notify::new());
    state
        .telegram_topic_cleanup_registrations
        .lock()
        .await
        .insert(
            route.conversation_key.clone(),
            TelegramTopicCleanupRegistration {
                token: 41,
                lifecycle_generation: generation,
                lifecycle_revision: 1,
                notifier: notifier.clone(),
            },
        );
    let target = TelegramTopicCleanupTarget {
        conversation_key: route.conversation_key.clone(),
        route,
        thread_id: "thread-rearchive".to_string(),
        chat_id: "-100".to_string(),
        topic_id: 47,
        lifecycle_revision: 1,
    };

    assert_eq!(
        finish_telegram_topic_cleanup_worker_iteration(&state, &target, 41, generation, 1,).await,
        Some((generation, 2))
    );
    {
        let registrations = state.telegram_topic_cleanup_registrations.lock().await;
        let registration = &registrations[&target.conversation_key];
        assert_eq!(registration.token, 41);
        assert_eq!(registration.lifecycle_revision, 2);
    }

    state
        .telegram_topic_cleanup_registrations
        .lock()
        .await
        .insert(
            target.conversation_key.clone(),
            TelegramTopicCleanupRegistration {
                token: 42,
                lifecycle_generation: generation,
                lifecycle_revision: 3,
                notifier,
            },
        );
    assert_eq!(
        finish_telegram_topic_cleanup_worker_iteration(&state, &target, 41, generation, 2,).await,
        None
    );
    assert_eq!(
        state.telegram_topic_cleanup_registrations.lock().await[&target.conversation_key].token,
        42
    );
}

#[tokio::test]
async fn rearchive_notification_does_not_bypass_an_account_retry_after_deadline() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let mut config = crate::config::AppConfig::default();
    config.state_path = temp_dir.path().join("state.json");
    let state =
        crate::app_state::AppState::new(temp_dir.path().join("config.toml"), config, None, None);
    let generation = state.runtime.lock().await.start_bridge_generation();
    let route = RouteTarget {
        platform: ImPlatformKind::Telegram,
        conversation_key: "telegram:telegram:-100|topic=48".to_string(),
        account_id: "telegram".to_string(),
        chat_id: "-100|topic=48".to_string(),
        remote_client_key: "im:telegram:test".to_string(),
    };
    {
        let mut persisted = state.persisted.lock().await;
        persisted.im_thread_bindings.insert(
            route.conversation_key.clone(),
            "thread-retry-after".to_string(),
        );
        persisted.telegram_topic_binding_states.insert(
            route.conversation_key.clone(),
            crate::store::TelegramTopicBindingState {
                thread_id: "thread-retry-after".to_string(),
                codex_state: "archived".to_string(),
                lifecycle_generation: generation,
                lifecycle_revision: 3,
                ..Default::default()
            },
        );
    }
    state
        .extend_telegram_topic_cleanup_retry_deadline("telegram", Duration::from_secs(60))
        .await;
    let notifier = tokio::sync::Notify::new();
    let target = TelegramTopicCleanupTarget {
        conversation_key: route.conversation_key.clone(),
        route,
        thread_id: "thread-retry-after".to_string(),
        chat_id: "-100".to_string(),
        topic_id: 48,
        lifecycle_revision: 3,
    };

    notifier.notify_one();
    let mut wait = Box::pin(wait_for_telegram_topic_cleanup_retry_deadline(
        &state, &target, generation, 3, &notifier,
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(25), &mut wait)
            .await
            .is_err(),
        "a rearchive wake must keep waiting for the Retry-After deadline"
    );

    {
        let mut persisted = state.persisted.lock().await;
        let binding = persisted
            .telegram_topic_binding_states
            .get_mut(&target.conversation_key)
            .expect("binding");
        binding.codex_state = "active".to_string();
        binding.lifecycle_revision = 4;
    }
    notifier.notify_one();
    assert!(
        !tokio::time::timeout(Duration::from_millis(250), &mut wait)
            .await
            .expect("active lifecycle wakes the cooldown wait")
    );

    {
        let mut persisted = state.persisted.lock().await;
        let binding = persisted
            .telegram_topic_binding_states
            .get_mut(&target.conversation_key)
            .expect("binding");
        binding.codex_state = "archived".to_string();
        binding.lifecycle_revision = 5;
    }
    let mut rearchive_target = target.clone();
    rearchive_target.lifecycle_revision = 5;
    notifier.notify_one();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(25),
            wait_for_telegram_topic_cleanup_retry_deadline(
                &state,
                &rearchive_target,
                generation,
                5,
                &notifier,
            ),
        )
        .await
        .is_err(),
        "the replacement worker must honor the remaining account deadline"
    );
}

#[tokio::test]
async fn archive_unarchive_and_delete_notifications_update_bound_topic_state() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let mut config = crate::config::AppConfig::default();
    config.state_path = temp_dir.path().join("state.json");
    let state =
        crate::app_state::AppState::new(temp_dir.path().join("config.toml"), config, None, None);
    let generation = state.runtime.lock().await.start_bridge_generation();
    let route = RouteTarget {
        platform: ImPlatformKind::Telegram,
        conversation_key: "telegram:telegram:-100|topic=42".to_string(),
        account_id: "telegram".to_string(),
        chat_id: "-100|topic=42".to_string(),
        remote_client_key: "im:telegram:test".to_string(),
    };
    state
        .runtime
        .lock()
        .await
        .bind_route("thread-42", route.clone());
    {
        let mut persisted = state.persisted.lock().await;
        persisted
            .im_thread_bindings
            .insert(route.conversation_key.clone(), "thread-42".to_string());
        persisted.telegram_topic_binding_states.insert(
            route.conversation_key.clone(),
            crate::store::TelegramTopicBindingState {
                thread_id: "thread-42".to_string(),
                topic_name: "Topic 42".to_string(),
                ..Default::default()
            },
        );
    }

    archive_telegram_topic_for_codex_thread(
        &state,
        &ImApiRegistry::default(),
        "thread-42",
        generation,
    )
    .await;
    assert_eq!(
        state.persisted.lock().await.telegram_topic_binding_states[&route.conversation_key]
            .codex_state,
        "archived"
    );

    unarchive_telegram_topic_for_codex_thread(&state, "thread-42", generation).await;
    assert_eq!(
        state.persisted.lock().await.telegram_topic_binding_states[&route.conversation_key]
            .codex_state,
        "active"
    );

    delete_telegram_topic_for_codex_thread(
        &state,
        &ImApiRegistry::default(),
        "thread-42",
        generation,
    )
    .await;
    assert_eq!(
        state.persisted.lock().await.telegram_topic_binding_states[&route.conversation_key]
            .codex_state,
        "deleted"
    );
    unarchive_telegram_topic_for_codex_thread(&state, "thread-42", generation).await;
    assert_eq!(
        state.persisted.lock().await.telegram_topic_binding_states[&route.conversation_key]
            .codex_state,
        "deleted"
    );

    archive_telegram_topic_for_codex_thread(
        &state,
        &ImApiRegistry::default(),
        "thread-42",
        generation,
    )
    .await;
    assert_eq!(
        state.persisted.lock().await.telegram_topic_binding_states[&route.conversation_key]
            .codex_state,
        "deleted"
    );
}

#[tokio::test]
async fn stale_generation_cannot_commit_or_overwrite_topic_lifecycle() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let mut config = crate::config::AppConfig::default();
    config.state_path = temp_dir.path().join("state.json");
    let state =
        crate::app_state::AppState::new(temp_dir.path().join("config.toml"), config, None, None);
    let first_generation = state.runtime.lock().await.start_bridge_generation();
    let route = RouteTarget {
        platform: ImPlatformKind::Telegram,
        conversation_key: "telegram:telegram:-100|topic=45".to_string(),
        account_id: "telegram".to_string(),
        chat_id: "-100|topic=45".to_string(),
        remote_client_key: "im:telegram:test".to_string(),
    };
    state
        .runtime
        .lock()
        .await
        .bind_route("thread-generation", route.clone());
    {
        let mut persisted = state.persisted.lock().await;
        persisted.im_thread_bindings.insert(
            route.conversation_key.clone(),
            "thread-generation".to_string(),
        );
        persisted.telegram_topic_binding_states.insert(
            route.conversation_key.clone(),
            crate::store::TelegramTopicBindingState {
                thread_id: "thread-generation".to_string(),
                ..Default::default()
            },
        );
    }
    let current_generation = state.runtime.lock().await.start_bridge_generation();

    archive_telegram_topic_for_codex_thread(
        &state,
        &ImApiRegistry::default(),
        "thread-generation",
        first_generation,
    )
    .await;
    assert_eq!(
        state.persisted.lock().await.telegram_topic_binding_states[&route.conversation_key]
            .codex_state,
        "active"
    );

    archive_telegram_topic_for_codex_thread(
        &state,
        &ImApiRegistry::default(),
        "thread-generation",
        current_generation,
    )
    .await;
    unarchive_telegram_topic_for_codex_thread(&state, "thread-generation", current_generation)
        .await;
    archive_telegram_topic_for_codex_thread(
        &state,
        &ImApiRegistry::default(),
        "thread-generation",
        first_generation,
    )
    .await;

    let persisted = state.persisted.lock().await;
    let binding = &persisted.telegram_topic_binding_states[&route.conversation_key];
    assert_eq!(binding.codex_state, "active");
    assert_eq!(binding.lifecycle_generation, current_generation);
}

#[test]
fn deleted_topic_state_is_not_revived_by_a_stale_active_snapshot() {
    let mut binding = crate::store::TelegramTopicBindingState {
        thread_id: "thread-deleted".to_string(),
        codex_state: "deleted".to_string(),
        ..Default::default()
    };
    assert_eq!(
        update_telegram_topic_lifecycle(
            &mut binding,
            "thread-deleted",
            &HashSet::from(["thread-deleted".to_string()]),
            &HashSet::new(),
            0,
            4_000,
        ),
        TelegramTopicLifecycle::Deleted
    );
    assert_eq!(binding.codex_state, "deleted");
}

#[tokio::test]
async fn reconciliation_cannot_overwrite_a_newer_archive_marker() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let mut config = crate::config::AppConfig::default();
    config.state_path = temp_dir.path().join("state.json");
    let state =
        crate::app_state::AppState::new(temp_dir.path().join("config.toml"), config, None, None);
    let conversation_key = "telegram:telegram:-100|topic=43";
    let expected = crate::store::TelegramTopicBindingState {
        thread_id: "thread-43".to_string(),
        topic_name: "Topic 43".to_string(),
        ..Default::default()
    };
    let archived = crate::store::TelegramTopicBindingState {
        codex_state: "archived".to_string(),
        archived_at_ms: Some(2_000),
        last_checked_at_ms: 2_000,
        ..expected.clone()
    };
    {
        let mut persisted = state.persisted.lock().await;
        persisted
            .im_thread_bindings
            .insert(conversation_key.to_string(), "thread-43".to_string());
        persisted
            .telegram_topic_binding_states
            .insert(conversation_key.to_string(), archived.clone());
    }

    let wrote = persist_telegram_topic_binding_state(
        &state,
        conversation_key,
        "thread-43",
        Some(&expected),
        expected.clone(),
        0,
        0,
    )
    .await;

    assert!(wrote.is_none());
    assert_eq!(
        state.persisted.lock().await.telegram_topic_binding_states[conversation_key],
        archived
    );

    let stale_archived = archived.clone();
    let restored_active = crate::store::TelegramTopicBindingState {
        codex_state: "active".to_string(),
        archived_at_ms: None,
        last_checked_at_ms: 3_000,
        ..expected.clone()
    };
    state
        .persisted
        .lock()
        .await
        .telegram_topic_binding_states
        .insert(conversation_key.to_string(), restored_active.clone());
    let wrote = persist_telegram_topic_binding_state(
        &state,
        conversation_key,
        "thread-43",
        Some(&stale_archived),
        stale_archived.clone(),
        0,
        0,
    )
    .await;
    assert!(wrote.is_none());
    assert_eq!(
        state.persisted.lock().await.telegram_topic_binding_states[conversation_key],
        restored_active
    );
}

#[tokio::test]
async fn archived_reconciliation_commits_a_thread_lifecycle_tombstone() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let mut config = crate::config::AppConfig::default();
    config.state_path = temp_dir.path().join("state.json");
    let state =
        crate::app_state::AppState::new(temp_dir.path().join("config.toml"), config, None, None);
    let generation = state.runtime.lock().await.start_bridge_generation();
    let conversation_key = "telegram:telegram:-100|topic=49";
    let active = crate::store::TelegramTopicBindingState {
        thread_id: "thread-reconcile-archive".to_string(),
        lifecycle_generation: generation,
        ..Default::default()
    };
    let archived = crate::store::TelegramTopicBindingState {
        codex_state: "archived".to_string(),
        archived_at_ms: Some(now_ms()),
        lifecycle_revision: 1,
        ..active.clone()
    };
    {
        let mut persisted = state.persisted.lock().await;
        persisted.im_thread_bindings.insert(
            conversation_key.to_string(),
            "thread-reconcile-archive".to_string(),
        );
        persisted
            .telegram_topic_binding_states
            .insert(conversation_key.to_string(), active.clone());
    }

    assert!(
        persist_telegram_topic_binding_state(
            &state,
            conversation_key,
            "thread-reconcile-archive",
            Some(&active),
            archived,
            generation,
            0,
        )
        .await
        .is_some()
    );
    assert!(
        !state
            .telegram_thread_allows_topic_binding("thread-reconcile-archive", generation)
            .await
    );
    state
        .persisted
        .lock()
        .await
        .im_thread_bindings
        .remove(conversation_key);
    assert!(
        !state
            .observe_telegram_thread_started("thread-reconcile-archive", generation)
            .await
    );
}

#[tokio::test]
async fn archived_reconciliation_hands_cross_account_rebinding_to_the_current_api() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let mut config = crate::config::AppConfig::default();
    config.state_path = temp_dir.path().join("state.json");
    let state =
        crate::app_state::AppState::new(temp_dir.path().join("config.toml"), config, None, None);
    let generation = state.runtime.lock().await.start_bridge_generation();
    let thread_id = "thread-reconcile-rebound";
    assert!(
        state
            .observe_telegram_thread_started(thread_id, generation)
            .await
    );
    let old_route = RouteTarget {
        platform: ImPlatformKind::Telegram,
        conversation_key: "telegram:account-a:-100|topic=61".to_string(),
        account_id: "account-a".to_string(),
        chat_id: "-100|topic=61".to_string(),
        remote_client_key: "im:telegram:account-a".to_string(),
    };
    let new_route = RouteTarget {
        platform: ImPlatformKind::Telegram,
        conversation_key: "telegram:account-b:-100|topic=62".to_string(),
        account_id: "account-b".to_string(),
        chat_id: "-100|topic=62".to_string(),
        remote_client_key: "im:telegram:account-b".to_string(),
    };
    bind_thread_to_route_for_generation(
        &state,
        &old_route,
        thread_id,
        None,
        old_route.remote_client_key.clone(),
        Some(generation),
    )
    .await
    .expect("bind old Topic");
    let expected = state.persisted.lock().await.telegram_topic_binding_states
        [&old_route.conversation_key]
        .clone();
    let expected_lifecycle_revision = state
        .telegram_thread_lifecycle_revision(thread_id, generation)
        .await
        .expect("lifecycle snapshot");
    let mut archived = expected.clone();
    assert_eq!(
        update_telegram_topic_lifecycle(
            &mut archived,
            thread_id,
            &HashSet::new(),
            &HashSet::from([thread_id.to_string()]),
            generation,
            4_000,
        ),
        TelegramTopicLifecycle::Archived
    );

    let (query_started_tx, query_started_rx) = tokio::sync::oneshot::channel();
    let (release_query_tx, release_query_rx) = tokio::sync::oneshot::channel();
    let commit_task = tokio::spawn({
        let state = state.clone();
        let old_key = old_route.conversation_key.clone();
        let expected = expected.clone();
        async move {
            let _ = query_started_tx.send(());
            let _ = release_query_rx.await;
            persist_telegram_topic_binding_state(
                &state,
                &old_key,
                thread_id,
                Some(&expected),
                archived,
                generation,
                expected_lifecycle_revision,
            )
            .await
        }
    });
    query_started_rx.await.expect("query started");
    bind_thread_to_route_for_generation(
        &state,
        &new_route,
        thread_id,
        None,
        new_route.remote_client_key.clone(),
        Some(generation),
    )
    .await
    .expect("rebind while query is in flight");
    {
        let mut persisted = state.persisted.lock().await;
        let rebound = persisted
            .telegram_topic_binding_states
            .get_mut(&new_route.conversation_key)
            .expect("new Topic binding");
        rebound.topic_name = "New Topic Name".to_string();
        rebound.last_synced_topic_name = "New Topic Name".to_string();
    }
    release_query_tx.send(()).expect("release query");
    let commit = commit_task
        .await
        .expect("commit task")
        .expect("unchanged lifecycle token commits");

    assert_eq!(
        commit.conversation_key.as_deref(),
        Some(new_route.conversation_key.as_str())
    );
    let persisted = state.persisted.lock().await;
    assert!(
        !persisted
            .im_thread_bindings
            .contains_key(&old_route.conversation_key)
    );
    let rebound = &persisted.telegram_topic_binding_states[&new_route.conversation_key];
    assert_eq!(rebound.codex_state, "archived");
    assert_eq!(rebound.topic_name, "New Topic Name");
    assert_eq!(rebound.last_synced_topic_name, "New Topic Name");
    drop(persisted);
    assert!(
        !state
            .telegram_thread_allows_topic_binding(thread_id, generation)
            .await
    );

    let (api_a, request_a, release_a) =
        blocking_successful_topic_api("account-a", "account-a-token").await;
    let (api_b, request_b, release_b) =
        blocking_successful_topic_api("account-b", "account-b-token").await;
    let mut api_registry = ImApiRegistry::default();
    api_registry.telegram.insert("account-a".to_string(), api_a);
    api_registry.telegram.insert("account-b".to_string(), api_b);
    assert!(
        schedule_reconciled_telegram_topic_cleanup(
            &state,
            &api_registry,
            commit,
            thread_id.to_string(),
            generation,
        )
        .await
    );
    let (selected_account, request_line) =
        tokio::time::timeout(Duration::from_secs(1), async move {
            tokio::select! {
                request = request_a => ("account-a", request.expect("account A request")),
                request = request_b => ("account-b", request.expect("account B request")),
            }
        })
        .await
        .expect("one account receives the delete request");
    assert_eq!(selected_account, "account-b");
    assert!(request_line.contains("/botaccount-b-token/deleteForumTopic"));
    assert!(
        state
            .telegram_topic_cleanup_registrations
            .lock()
            .await
            .contains_key(&new_route.conversation_key),
        "the cross-account cleanup remains registered until B responds"
    );
    release_b.send(()).expect("release account B response");
    let _ = release_a.send(());
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let binding_exists = state
                .persisted
                .lock()
                .await
                .im_thread_bindings
                .contains_key(&new_route.conversation_key);
            let cleanup_registered = state
                .telegram_topic_cleanup_registrations
                .lock()
                .await
                .contains_key(&new_route.conversation_key);
            if !binding_exists && !cleanup_registered {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("account B cleanup finishes");
}

#[tokio::test]
async fn archived_reconciliation_cannot_overwrite_unarchive_during_the_query() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let mut config = crate::config::AppConfig::default();
    config.state_path = temp_dir.path().join("state.json");
    let state =
        crate::app_state::AppState::new(temp_dir.path().join("config.toml"), config, None, None);
    let generation = state.runtime.lock().await.start_bridge_generation();
    let thread_id = "thread-reconcile-unarchived";
    assert!(
        state
            .observe_telegram_thread_started(thread_id, generation)
            .await
    );
    let route = RouteTarget {
        platform: ImPlatformKind::Telegram,
        conversation_key: "telegram:telegram:-100|topic=63".to_string(),
        account_id: "telegram".to_string(),
        chat_id: "-100|topic=63".to_string(),
        remote_client_key: "im:telegram:test".to_string(),
    };
    bind_thread_to_route_for_generation(
        &state,
        &route,
        thread_id,
        None,
        route.remote_client_key.clone(),
        Some(generation),
    )
    .await
    .expect("bind Topic");
    assert_eq!(
        mark_telegram_topic_bindings_for_cleanup(
            &state,
            thread_id,
            TelegramTopicLifecycle::Archived,
            generation,
        )
        .await
        .len(),
        1
    );
    let expected =
        state.persisted.lock().await.telegram_topic_binding_states[&route.conversation_key].clone();
    let expected_lifecycle_revision = state
        .telegram_thread_lifecycle_revision(thread_id, generation)
        .await
        .expect("lifecycle snapshot");
    let archived = expected.clone();

    let (query_started_tx, query_started_rx) = tokio::sync::oneshot::channel();
    let (release_query_tx, release_query_rx) = tokio::sync::oneshot::channel();
    let commit_task = tokio::spawn({
        let state = state.clone();
        let conversation_key = route.conversation_key.clone();
        async move {
            let _ = query_started_tx.send(());
            let _ = release_query_rx.await;
            persist_telegram_topic_binding_state(
                &state,
                &conversation_key,
                thread_id,
                Some(&expected),
                archived,
                generation,
                expected_lifecycle_revision,
            )
            .await
        }
    });
    query_started_rx.await.expect("query started");
    unarchive_telegram_topic_for_codex_thread(&state, thread_id, generation).await;
    release_query_tx.send(()).expect("release query");
    assert!(
        commit_task.await.expect("commit task").is_none(),
        "stale archive snapshot must fail its lifecycle token check"
    );

    let persisted = state.persisted.lock().await;
    assert_eq!(
        persisted.telegram_topic_binding_states[&route.conversation_key].codex_state,
        "active"
    );
    drop(persisted);
    assert!(
        state
            .telegram_thread_allows_topic_binding(thread_id, generation)
            .await
    );
}

#[tokio::test]
async fn active_reconciliation_cancels_a_queued_archive_cleanup() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let mut config = crate::config::AppConfig::default();
    config.state_path = temp_dir.path().join("state.json");
    let state =
        crate::app_state::AppState::new(temp_dir.path().join("config.toml"), config, None, None);
    let generation = state.runtime.lock().await.start_bridge_generation();
    let route = RouteTarget {
        platform: ImPlatformKind::Telegram,
        conversation_key: "telegram:telegram:-100|topic=46".to_string(),
        account_id: "telegram".to_string(),
        chat_id: "-100|topic=46".to_string(),
        remote_client_key: "im:telegram:test".to_string(),
    };
    let archived = crate::store::TelegramTopicBindingState {
        thread_id: "thread-active-again".to_string(),
        codex_state: "archived".to_string(),
        archived_at_ms: Some(2_000),
        lifecycle_generation: generation,
        lifecycle_revision: 1,
        last_checked_at_ms: 2_000,
        ..Default::default()
    };
    {
        let mut persisted = state.persisted.lock().await;
        persisted.im_thread_bindings.insert(
            route.conversation_key.clone(),
            "thread-active-again".to_string(),
        );
        persisted
            .telegram_topic_binding_states
            .insert(route.conversation_key.clone(), archived.clone());
    }
    let cleanup_notifier = Arc::new(tokio::sync::Notify::new());
    state
        .telegram_topic_cleanup_registrations
        .lock()
        .await
        .insert(
            route.conversation_key.clone(),
            TelegramTopicCleanupRegistration {
                token: 51,
                lifecycle_generation: generation,
                lifecycle_revision: archived.lifecycle_revision,
                notifier: cleanup_notifier.clone(),
            },
        );
    let mut active = archived.clone();
    assert_eq!(
        update_telegram_topic_lifecycle(
            &mut active,
            "thread-active-again",
            &HashSet::from(["thread-active-again".to_string()]),
            &HashSet::new(),
            generation,
            3_000,
        ),
        TelegramTopicLifecycle::Active
    );
    let active_revision = active.lifecycle_revision;
    assert!(
        persist_telegram_topic_binding_state(
            &state,
            &route.conversation_key,
            "thread-active-again",
            Some(&archived),
            active,
            generation,
            0,
        )
        .await
        .is_some()
    );
    tokio::time::timeout(Duration::from_millis(250), cleanup_notifier.notified())
        .await
        .expect("active reconciliation wakes the cleanup worker");
    let target = TelegramTopicCleanupTarget {
        conversation_key: route.conversation_key.clone(),
        route,
        thread_id: "thread-active-again".to_string(),
        chat_id: "-100".to_string(),
        topic_id: 46,
        lifecycle_revision: active_revision,
    };
    assert!(
        !telegram_topic_cleanup_is_required(
            &state,
            &target,
            generation,
            target.lifecycle_revision,
        )
        .await
    );
    assert_eq!(
        finish_telegram_topic_cleanup_worker_iteration(
            &state,
            &target,
            51,
            generation,
            archived.lifecycle_revision,
        )
        .await,
        None
    );
    assert!(
        !state
            .telegram_topic_cleanup_registrations
            .lock()
            .await
            .contains_key(&target.conversation_key)
    );
}

#[tokio::test]
async fn successful_remote_delete_clears_a_binding_even_after_unarchive() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let mut config = crate::config::AppConfig::default();
    config.state_path = temp_dir.path().join("state.json");
    let state =
        crate::app_state::AppState::new(temp_dir.path().join("config.toml"), config, None, None);
    let route = RouteTarget {
        platform: ImPlatformKind::Telegram,
        conversation_key: "telegram:telegram:-100|topic=44".to_string(),
        account_id: "telegram".to_string(),
        chat_id: "-100|topic=44".to_string(),
        remote_client_key: "im:telegram:test".to_string(),
    };
    state
        .runtime
        .lock()
        .await
        .bind_route("thread-44", route.clone());
    {
        let mut persisted = state.persisted.lock().await;
        persisted
            .im_thread_bindings
            .insert(route.conversation_key.clone(), "thread-44".to_string());
        persisted.telegram_topic_binding_states.insert(
            route.conversation_key.clone(),
            crate::store::TelegramTopicBindingState {
                thread_id: "thread-44".to_string(),
                codex_state: "active".to_string(),
                ..Default::default()
            },
        );
    }
    let target = TelegramTopicCleanupTarget {
        conversation_key: route.conversation_key.clone(),
        route,
        thread_id: "thread-44".to_string(),
        chat_id: "-100".to_string(),
        topic_id: 44,
        lifecycle_revision: 0,
    };

    finish_telegram_topic_cleanup(&state, &target, "remote_delete_succeeded", false).await;

    assert!(
        !state
            .persisted
            .lock()
            .await
            .im_thread_bindings
            .contains_key(&target.conversation_key)
    );
}

#[test]
fn converts_private_message_to_inbound() {
    let settings = TelegramSettings::default();
    let inbound = inbound_from_message(
        &settings,
        &["42".to_string()],
        &TelegramMessage {
            message_id: 9,
            from: Some(TelegramUser {
                id: 42,
                is_bot: false,
                username: Some("ada".to_string()),
                first_name: Some("Ada".to_string()),
                last_name: None,
            }),
            chat: TelegramChat {
                id: 42,
                kind: "private".to_string(),
                title: None,
                username: Some("ada".to_string()),
                first_name: Some("Ada".to_string()),
                last_name: None,
            },
            text: Some("/status".to_string()),
            ..TelegramMessage::default()
        },
    )
    .expect("inbound message");

    assert_eq!(inbound.platform, ImPlatformKind::Telegram);
    assert_eq!(inbound.conversation_key(), "telegram:telegram:42");
    assert_eq!(inbound.chat_type, ChatType::Direct);
    assert_eq!(inbound.text, "/status");
}

#[test]
fn accepts_photo_without_text_and_selects_largest_size() {
    let settings = TelegramSettings::default();
    let message = TelegramMessage {
        message_id: 12,
        chat: TelegramChat {
            id: 42,
            kind: "private".to_string(),
            ..TelegramChat::default()
        },
        photo: Some(vec![
            TelegramPhotoSize {
                file_id: "wide".to_string(),
                file_unique_id: "wide-unique".to_string(),
                width: 1280,
                height: 720,
                file_size: Some(1_000),
            },
            TelegramPhotoSize {
                file_id: "large".to_string(),
                file_unique_id: "large-unique".to_string(),
                width: 1024,
                height: 1024,
                file_size: Some(10_000),
            },
        ]),
        ..TelegramMessage::default()
    };

    let inbound = inbound_from_message(&settings, &["42".to_string()], &message)
        .expect("photo message should be accepted");
    let specs = attachment_specs(&message);

    assert!(inbound.text.is_empty());
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].file_id, "large");
    assert_eq!(specs[0].kind, "image");
}

#[test]
fn treats_image_documents_as_visual_input() {
    let message = TelegramMessage {
        message_id: 14,
        chat: TelegramChat {
            id: 42,
            kind: "private".to_string(),
            ..TelegramChat::default()
        },
        document: Some(TelegramMediaFile {
            file_id: "image-document".to_string(),
            file_unique_id: "image-document-unique".to_string(),
            file_size: Some(8_192),
            file_name: Some("diagram.png".to_string()),
            mime_type: Some("image/png".to_string()),
        }),
        ..TelegramMessage::default()
    };

    let specs = attachment_specs(&message);

    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].kind, "image");
    assert_eq!(specs[0].directory, "images");
}

#[test]
fn deduplicates_animation_compatibility_document_by_file_id() {
    let message = TelegramMessage {
        message_id: 15,
        chat: TelegramChat {
            id: 42,
            kind: "private".to_string(),
            ..TelegramChat::default()
        },
        animation: Some(TelegramMediaFile {
            file_id: "shared-animation-file".to_string(),
            file_unique_id: "shared-animation-unique".to_string(),
            file_size: Some(8_192),
            file_name: Some("demo.mp4".to_string()),
            mime_type: Some("video/mp4".to_string()),
        }),
        document: Some(TelegramMediaFile {
            file_id: "shared-animation-file".to_string(),
            file_unique_id: "shared-animation-unique".to_string(),
            file_size: Some(8_192),
            file_name: Some("compatibility.mp4".to_string()),
            mime_type: Some("video/mp4".to_string()),
        }),
        ..TelegramMessage::default()
    };

    let specs = attachment_specs(&message);

    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].file_id, "shared-animation-file");
    assert_eq!(specs[0].kind, "video");
    assert_eq!(specs[0].directory, "videos");
    assert_eq!(specs[0].name, "demo.mp4");
}

#[test]
fn preserves_each_telegram_sticker_format() {
    let cases = [
        (false, false, "image", "images", ".webp", "image/webp"),
        (
            true,
            false,
            "file",
            "files",
            ".tgs",
            "application/x-tgsticker",
        ),
        (false, true, "video", "videos", ".webm", "video/webm"),
    ];

    for (is_animated, is_video, kind, directory, extension, mime_type) in cases {
        let message = TelegramMessage {
            message_id: 18,
            chat: TelegramChat {
                id: 42,
                kind: "private".to_string(),
                ..TelegramChat::default()
            },
            sticker: Some(TelegramStickerFile {
                file_id: format!("sticker-{extension}"),
                file_unique_id: format!("sticker-unique-{extension}"),
                file_size: Some(4_096),
                is_animated,
                is_video,
            }),
            ..TelegramMessage::default()
        };

        let specs = attachment_specs(&message);

        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].kind, kind);
        assert_eq!(specs[0].directory, directory);
        assert!(specs[0].name.ends_with(extension));
        assert_eq!(specs[0].mime_type.as_deref(), Some(mime_type));
    }
}

#[tokio::test]
async fn oversized_attachment_returns_user_visible_failure_without_network() {
    let api = TelegramApi::new(TelegramSettings {
        account_id: "test".to_string(),
        bot_token: String::new(),
        allowed_chat_ids: vec!["42".to_string()],
        ..TelegramSettings::default()
    });
    let message = TelegramMessage {
        message_id: 16,
        chat: TelegramChat {
            id: 42,
            kind: "private".to_string(),
            ..TelegramChat::default()
        },
        caption: Some("review this archive".to_string()),
        document: Some(TelegramMediaFile {
            file_id: "oversized-file".to_string(),
            file_unique_id: "oversized-unique".to_string(),
            file_size: Some(TELEGRAM_MAX_FILE_BYTES + 1),
            file_name: Some("archive.zip".to_string()),
            mime_type: Some("application/zip".to_string()),
        }),
        ..TelegramMessage::default()
    };

    let collection =
        collect_telegram_attachments(&api, Path::new("/unused-test-root"), &message).await;
    let notice = attachment_failure_notice(&collection.failures, true);

    assert!(collection.attachments.is_empty());
    assert_eq!(collection.failures.len(), 1);
    assert_eq!(
        collection.failures[0].reason,
        TelegramAttachmentFailureReason::TooLarge {
            bytes: Some(TELEGRAM_MAX_FILE_BYTES + 1),
        }
    );
    assert!(notice.contains("archive.zip"));
    assert!(notice.contains("20 MB"));
    assert!(notice.contains("说明文字也没有提交"));
}

#[test]
fn failure_notice_explains_metadata_download_and_persist_errors() {
    let failures = [
        TelegramAttachmentFailure {
            name: "metadata.bin".to_string(),
            reason: TelegramAttachmentFailureReason::MetadataUnavailable,
        },
        TelegramAttachmentFailure {
            name: "download.bin".to_string(),
            reason: TelegramAttachmentFailureReason::DownloadFailed,
        },
        TelegramAttachmentFailure {
            name: "persist.bin".to_string(),
            reason: TelegramAttachmentFailureReason::PersistFailed,
        },
    ];

    let notice = attachment_failure_notice(&failures, false);

    assert!(notice.contains("metadata.bin：无法读取 Telegram 文件信息"));
    assert!(notice.contains("download.bin：从 Telegram 下载失败"));
    assert!(notice.contains("persist.bin：下载后无法保存到本机"));
    assert!(!notice.contains("说明文字也没有提交"));
}

#[tokio::test]
async fn malformed_media_returns_metadata_failure_without_network() {
    let api = TelegramApi::new(TelegramSettings::default());
    let message = TelegramMessage {
        message_id: 17,
        chat: TelegramChat {
            id: 42,
            kind: "private".to_string(),
            ..TelegramChat::default()
        },
        photo: Some(Vec::new()),
        ..TelegramMessage::default()
    };

    let collection =
        collect_telegram_attachments(&api, Path::new("/unused-test-root"), &message).await;

    assert!(collection.attachments.is_empty());
    assert_eq!(
        collection.failures,
        vec![TelegramAttachmentFailure {
            name: "Telegram 附件".to_string(),
            reason: TelegramAttachmentFailureReason::MetadataUnavailable,
        }]
    );
}

#[test]
fn attachment_names_hash_the_complete_telegram_file_id() {
    let first = unique_attachment_name("report.pdf", "shared-prefix-file-a");
    let second = unique_attachment_name("report.pdf", "shared-prefix-file-b");

    assert_ne!(first, second);
    assert!(first.starts_with("report-"));
    assert!(first.ends_with(".pdf"));
    assert_eq!(first.len(), "report-".len() + 64 + ".pdf".len());
}

#[test]
fn uses_document_caption_as_turn_text() {
    let settings = TelegramSettings::default();
    let message = TelegramMessage {
        message_id: 13,
        chat: TelegramChat {
            id: 42,
            kind: "private".to_string(),
            ..TelegramChat::default()
        },
        caption: Some("review this file".to_string()),
        document: Some(TelegramMediaFile {
            file_id: "document".to_string(),
            file_unique_id: "document-unique".to_string(),
            file_size: Some(4_096),
            file_name: Some("report.pdf".to_string()),
            mime_type: Some("application/pdf".to_string()),
        }),
        ..TelegramMessage::default()
    };

    let inbound = inbound_from_message(&settings, &["42".to_string()], &message)
        .expect("document message should be accepted");
    let specs = attachment_specs(&message);

    assert_eq!(inbound.text, "review this file");
    assert_eq!(specs[0].name, "report.pdf");
    assert_eq!(specs[0].kind, "file");
}

#[test]
fn empty_allowed_chat_ids_do_not_pass_message_conversion() {
    let settings = TelegramSettings::default();
    let inbound = inbound_from_message(
        &settings,
        &[],
        &TelegramMessage {
            message_id: 9,
            from: Some(TelegramUser {
                id: 42,
                is_bot: false,
                username: Some("ada".to_string()),
                first_name: Some("Ada".to_string()),
                last_name: None,
            }),
            chat: TelegramChat {
                id: 42,
                kind: "private".to_string(),
                title: None,
                username: Some("ada".to_string()),
                first_name: Some("Ada".to_string()),
                last_name: None,
            },
            text: Some("/status".to_string()),
            ..TelegramMessage::default()
        },
    );

    assert!(inbound.is_none());
}

#[test]
fn rejects_private_message_from_unlisted_chat() {
    let settings = TelegramSettings::default();
    let inbound = inbound_from_message(
        &settings,
        &["99".to_string()],
        &TelegramMessage {
            message_id: 9,
            from: Some(TelegramUser {
                id: 42,
                is_bot: false,
                username: Some("ada".to_string()),
                first_name: Some("Ada".to_string()),
                last_name: None,
            }),
            chat: TelegramChat {
                id: 42,
                kind: "private".to_string(),
                title: None,
                username: Some("ada".to_string()),
                first_name: Some("Ada".to_string()),
                last_name: None,
            },
            text: Some("/status".to_string()),
            ..TelegramMessage::default()
        },
    );

    assert!(inbound.is_none());
}

#[test]
fn ignores_group_messages() {
    let settings = TelegramSettings {
        mention_only: true,
        ..TelegramSettings::default()
    };
    let message = TelegramMessage {
        message_id: 10,
        from: Some(TelegramUser {
            id: 42,
            is_bot: false,
            username: Some("ada".to_string()),
            first_name: Some("Ada".to_string()),
            last_name: None,
        }),
        chat: TelegramChat {
            id: -100,
            kind: "group".to_string(),
            title: Some("Codex".to_string()),
            username: None,
            first_name: None,
            last_name: None,
        },
        text: Some("hello".to_string()),
        ..TelegramMessage::default()
    };

    assert!(inbound_from_message(&settings, &["42".to_string()], &message).is_none());
}

#[test]
fn ignores_group_messages_even_when_mentioned() {
    let settings = TelegramSettings {
        mention_only: true,
        ..TelegramSettings::default()
    };
    let inbound = inbound_from_message(
        &settings,
        &["42".to_string()],
        &TelegramMessage {
            message_id: 11,
            from: Some(TelegramUser {
                id: 42,
                is_bot: false,
                username: Some("ada".to_string()),
                first_name: Some("Ada".to_string()),
                last_name: None,
            }),
            chat: TelegramChat {
                id: -100,
                kind: "group".to_string(),
                title: Some("Codex".to_string()),
                username: None,
                first_name: None,
                last_name: None,
            },
            text: Some("@codex_bot hello".to_string()),
            ..TelegramMessage::default()
        },
    );

    assert!(inbound.is_none());
}

#[test]
fn routes_configured_forum_topic_as_its_own_conversation() {
    let settings = TelegramSettings {
        project_groups: vec![crate::config::TelegramProjectGroupConfig {
            chat_id: "-100".to_string(),
            project_name: "MochiPort".to_string(),
            cwd: "/tmp/mochiport".to_string(),
        }],
        ..TelegramSettings::default()
    };
    let message = TelegramMessage {
        message_id: 12,
        from: Some(TelegramUser {
            id: 42,
            is_bot: false,
            username: Some("ada".to_string()),
            first_name: Some("Ada".to_string()),
            last_name: None,
        }),
        chat: TelegramChat {
            id: -100,
            kind: "supergroup".to_string(),
            title: Some("MochiPort".to_string()),
            ..TelegramChat::default()
        },
        message_thread_id: Some(17),
        text: Some("修一下路由".to_string()),
        ..TelegramMessage::default()
    };

    let inbound = inbound_from_message(&settings, &[], &message).expect("topic inbound");
    assert_eq!(inbound.chat_type, ChatType::Group);
    assert_eq!(inbound.chat_id, "-100|topic=17");
    assert_eq!(
        inbound.conversation_key(),
        "telegram:telegram:-100|topic=17"
    );
}

#[test]
fn forum_topic_name_uses_first_line_and_is_bounded() {
    assert_eq!(
        forum_topic_name("MochiPort", "修一下启动流程\n补充日志"),
        "修一下启动流程"
    );
    assert_eq!(forum_topic_name("MochiPort", "   "), "MochiPort");
    let long = "a".repeat(100);
    assert_eq!(forum_topic_name("MochiPort", &long).chars().count(), 64);
}

#[test]
fn started_thread_metadata_accepts_nested_fields_and_fallback_keys() {
    let params = serde_json::json!({
        "threadId": "",
        "thread": {
            "id": "thread-1",
            "cwd": "/tmp/project",
            "name": null,
            "title": "会话标题"
        }
    });

    assert_eq!(started_thread_id(&params).as_deref(), Some("thread-1"));
    let thread = params.get("thread").expect("nested thread");
    assert_eq!(
        started_thread_field(&params, thread, &["name", "title"]).as_deref(),
        Some("会话标题")
    );
    assert_eq!(
        started_thread_field(&params, thread, &["cwd"]).as_deref(),
        Some("/tmp/project")
    );
}

#[tokio::test]
async fn auto_topic_resume_retries_until_new_rollout_is_visible() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let mut config = crate::config::AppConfig::default();
    config.state_path = temp_dir.path().join("state.json");
    let state =
        crate::app_state::AppState::new(temp_dir.path().join("config.toml"), config, None, None);
    let generation = state.runtime.lock().await.start_bridge_generation();
    let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let result = retry_auto_topic_resume(&state, "thread-new", generation, Duration::ZERO, || {
        let attempts = attempts.clone();
        async move {
            let attempt = attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if attempt == 0 {
                Err(anyhow!(
                    "remote-control request failed: no rollout found for thread id thread-new"
                ))
            } else {
                Ok(serde_json::json!({"thread": {"id": "thread-new"}}))
            }
        }
    })
    .await
    .expect("second resume should succeed");

    assert_eq!(result["thread"]["id"], "thread-new");
    assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[tokio::test]
async fn auto_topic_resume_stops_when_bridge_generation_changes() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let mut config = crate::config::AppConfig::default();
    config.state_path = temp_dir.path().join("state.json");
    let state =
        crate::app_state::AppState::new(temp_dir.path().join("config.toml"), config, None, None);
    let generation = state.runtime.lock().await.start_bridge_generation();
    let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let error = retry_auto_topic_resume(&state, "thread-stale", generation, Duration::ZERO, || {
        let state = state.clone();
        let attempts = attempts.clone();
        async move {
            attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            state.runtime.lock().await.invalidate_bridge_generation();
            Err(anyhow!(
                "remote-control request failed: no rollout found for thread id thread-stale"
            ))
        }
    })
    .await
    .expect_err("stale generation should cancel the retry");

    assert!(error.to_string().contains("bridge generation changed"));
    assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn auto_topic_resume_retries_only_missing_rollouts_within_the_attempt_budget() {
    let missing_rollout = anyhow!("no rollout found for thread id thread-new");
    assert!(should_retry_auto_topic_resume(&missing_rollout, 1));
    assert!(should_retry_auto_topic_resume(
        &missing_rollout,
        AUTO_TOPIC_RESUME_MAX_ATTEMPTS - 1
    ));
    assert!(!should_retry_auto_topic_resume(
        &missing_rollout,
        AUTO_TOPIC_RESUME_MAX_ATTEMPTS
    ));
    assert!(!should_retry_auto_topic_resume(
        &anyhow!("permission denied"),
        1
    ));
}

#[test]
fn cwd_matching_is_component_aware() {
    assert!(cwd_is_within_project("/tmp/project", "/tmp/project"));
    assert!(cwd_is_within_project("/tmp/project/src", "/tmp/project"));
    assert!(!cwd_is_within_project("/tmp/project-old", "/tmp/project"));
    assert!(!cwd_is_within_project("", "/tmp/project"));
}

#[tokio::test]
async fn auto_topic_target_prefers_the_most_specific_project_group() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let mut config = crate::config::AppConfig::default();
    config.state_path = temp_dir.path().join("state.json");
    config.telegram_accounts = vec![crate::config::TelegramConfig {
        enabled: true,
        account_id: "telegram".to_string(),
        bot_token: "token".to_string(),
        project_groups: vec![
            crate::config::TelegramProjectGroupConfig {
                chat_id: "-100-parent".to_string(),
                project_name: "Parent".to_string(),
                cwd: "/tmp/project".to_string(),
            },
            crate::config::TelegramProjectGroupConfig {
                chat_id: "-100-child".to_string(),
                project_name: "Child".to_string(),
                cwd: "/tmp/project/frontend".to_string(),
            },
        ],
        ..Default::default()
    }];
    let state =
        crate::app_state::AppState::new(temp_dir.path().join("config.toml"), config, None, None);
    let mut registry = ImApiRegistry::default();
    registry.telegram.insert(
        "telegram".to_string(),
        TelegramApi::new(TelegramSettings {
            account_id: "telegram".to_string(),
            bot_token: "token".to_string(),
            ..Default::default()
        }),
    );

    let target = find_auto_topic_target(&state, &registry, "/tmp/project/frontend/src")
        .await
        .expect("matching project group");
    assert_eq!(target.chat_id, "-100-child");
}

#[tokio::test]
async fn auto_topic_creation_stops_when_bridge_generation_is_invalidated() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let mut config = crate::config::AppConfig::default();
    config.state_path = temp_dir.path().join("state.json");
    let state =
        crate::app_state::AppState::new(temp_dir.path().join("config.toml"), config, None, None);
    let generation = state.runtime.lock().await.start_bridge_generation();
    state.runtime.lock().await.invalidate_bridge_generation();

    auto_create_topic_for_codex_thread_for_generation(
        &state,
        &ImApiRegistry::default(),
        "default:codex_app",
        serde_json::json!({
            "threadId": "thread-stale",
            "cwd": "/tmp/project",
            "name": "stale session"
        }),
        generation,
        None,
    )
    .await;

    assert!(state.events.lock().await.is_empty());
}

#[tokio::test]
async fn auto_topic_created_after_archive_is_cleaned_instead_of_bound() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let mut config = crate::config::AppConfig::default();
    config.state_path = temp_dir.path().join("state.json");
    let state =
        crate::app_state::AppState::new(temp_dir.path().join("config.toml"), config, None, None);
    let generation = state.runtime.lock().await.start_bridge_generation();
    let thread_id = "thread-create-archive";
    assert!(
        state
            .observe_telegram_thread_started(thread_id, generation)
            .await
    );
    let (create_started_tx, create_started_rx) = tokio::sync::oneshot::channel();
    let (release_create_tx, release_create_rx) = tokio::sync::oneshot::channel();
    let create_task = tokio::spawn({
        let state = state.clone();
        async move {
            run_telegram_topic_mutation_while(
                &state,
                "bot",
                None,
                || telegram_topic_creation_is_current(&state, thread_id, generation),
                || async move {
                    let _ = create_started_tx.send(());
                    let _ = release_create_rx.await;
                    Ok::<_, anyhow::Error>(77_i64)
                },
            )
            .await
        }
    });
    create_started_rx.await.expect("create API started");
    state
        .observe_telegram_thread_lifecycle(
            thread_id,
            generation,
            TelegramThreadLifecycleState::Archived,
        )
        .await;
    release_create_tx.send(()).expect("release create API");
    let topic_id = create_task
        .await
        .expect("create task")
        .expect("request began")
        .expect("create response");
    let cleaned = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let retained = keep_auto_created_topic_if_current(&state, thread_id, generation, topic_id, {
        let cleaned = cleaned.clone();
        move |topic_id| async move {
            assert_eq!(topic_id, 77);
            cleaned.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    })
    .await;

    assert!(retained.is_none());
    assert!(cleaned.load(std::sync::atomic::Ordering::SeqCst));
    assert!(state.runtime.lock().await.route_by_thread.is_empty());
    assert!(state.persisted.lock().await.im_thread_bindings.is_empty());
}

#[test]
fn parses_thread_route_callback_data() {
    let action = action_from_callback_data("trs:thread-route-7:2:3").expect("resume action");
    match action {
        InboundAction::ThreadRouteResumeIndex {
            request_id,
            page,
            index,
        } => {
            assert_eq!(request_id, "thread-route-7");
            assert_eq!(page, 2);
            assert_eq!(index, 3);
        }
        other => panic!("unexpected action: {other:?}"),
    }

    let action = action_from_callback_data("ap:abc123:2").expect("approval action");
    match action {
        InboundAction::ApprovalDecision {
            request_fingerprint,
            option_index,
        } => {
            assert_eq!(request_fingerprint, "abc123");
            assert_eq!(option_index, 2);
        }
        other => panic!("unexpected action: {other:?}"),
    }

    let action = action_from_callback_data("tlp:thread-route-7:next").expect("page action");
    match action {
        InboundAction::ThreadRouteListPage {
            request_id,
            direction,
        } => {
            assert_eq!(request_id, "thread-route-7");
            assert_eq!(direction, ThreadRouteDirection::Next);
        }
        other => panic!("unexpected action: {other:?}"),
    }
}

#[test]
fn parses_thread_settings_callback_data() {
    let action =
        action_from_callback_data("tmo:thread-model-7:12:model").expect("open model action");
    match action {
        InboundAction::ThreadSettingsOpenField {
            request_id,
            revision,
            field,
        } => {
            assert_eq!(request_id, "thread-model-7");
            assert_eq!(revision, 12);
            assert_eq!(field, ThreadSettingsField::Model);
        }
        other => panic!("unexpected action: {other:?}"),
    }

    let action =
        action_from_callback_data("tmp:thread-model-7:12:prev").expect("model page action");
    match action {
        InboundAction::ThreadSettingsModelPage {
            request_id,
            revision,
            direction,
        } => {
            assert_eq!(request_id, "thread-model-7");
            assert_eq!(revision, 12);
            assert_eq!(direction, ThreadRouteDirection::Prev);
        }
        other => panic!("unexpected action: {other:?}"),
    }

    let action =
        action_from_callback_data("tms:thread-model-7:12:3:2").expect("model select action");
    match action {
        InboundAction::ThreadSettingsChooseModel {
            request_id,
            revision,
            page,
            index,
        } => {
            assert_eq!(request_id, "thread-model-7");
            assert_eq!(revision, 12);
            assert_eq!(page, 3);
            assert_eq!(index, 2);
        }
        other => panic!("unexpected action: {other:?}"),
    }

    let action = action_from_callback_data("tmv:thread-model-7:12:fast").expect("speed action");
    assert!(matches!(
        action,
        InboundAction::ThreadSettingsChooseSpeed {
            revision: 12,
            fast: true,
            ..
        }
    ));
    assert!(matches!(
        action_from_callback_data("tmq:thread-model-7:12:yes"),
        Some(InboundAction::ThreadSettingsCompatibilityConfirm { accept: true, .. })
    ));
    assert!(action_from_callback_data("tmp:thread-model-7:12:sideways").is_none());
    assert!(action_from_callback_data("tms:thread-model-7:not-a-revision:3:2").is_none());
    let set_index =
        action_from_callback_data("tcs:thread-7:permission:1:2").expect("create option set");
    assert!(matches!(
        set_index,
        InboundAction::ThreadRouteCreateSetIndex { .. }
    ));
    assert!(action_from_callback_data("tmo:thread-model-7:12:other").is_none());
}

#[test]
fn pairing_text_matches_code_or_start_payload() {
    assert!(pairing_text_matches(Some("012345"), "012345"));
    assert!(pairing_text_matches(Some(" 012345 "), "012345"));
    assert!(pairing_text_matches(Some("/start 012345"), "012345"));
    assert!(pairing_text_matches(Some("/start@MyBot 012345"), "012345"));
    assert!(!pairing_text_matches(Some("/start"), "012345"));
    assert!(!pairing_text_matches(Some("0123456"), "012345"));
    assert!(!pairing_text_matches(Some("543210"), "012345"));
    assert!(!pairing_text_matches(None, "012345"));
}

async fn pairing_test_state(
    temp_dir: &tempfile::TempDir,
    pairing_code: &str,
    allowed_chat_ids: &[&str],
) -> SharedState {
    let mut config = crate::config::AppConfig::default();
    config
        .telegram_accounts
        .push(crate::config::TelegramConfig {
            account_id: "tg_1".to_string(),
            allowed_chat_ids: allowed_chat_ids
                .iter()
                .map(|chat_id| (*chat_id).to_string())
                .collect(),
            pairing_code: pairing_code.to_string(),
            ..Default::default()
        });
    crate::app_state::AppState::new(temp_dir.path().join("config.toml"), config, None, None)
}

async fn mock_telegram_api(account_id: &str) -> TelegramApi {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock Telegram server");
    let address = listener.local_addr().expect("mock Telegram address");
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut request = vec![0_u8; 4_096];
            let _ = stream.read(&mut request).await;
            let body = r#"{"ok":true,"result":true}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });
    TelegramApi::new(TelegramSettings {
        account_id: account_id.to_string(),
        bot_token: "test-token".to_string(),
        ..Default::default()
    })
    .with_test_api_base(format!("http://{address}"))
}

fn private_chat(chat_id: i64) -> TelegramChat {
    TelegramChat {
        id: chat_id,
        kind: "private".to_string(),
        ..Default::default()
    }
}

#[tokio::test]
async fn pairing_gate_binds_chat_with_correct_code_and_consumes_message() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let state = pairing_test_state(&temp_dir, "012345", &[]).await;
    let api = mock_telegram_api("tg_1").await;
    let mut access = TelegramChatAccess::new(Vec::new());
    let mut attempts = PairingAttempts::default();
    let chat = private_chat(42);

    let decision = ensure_chat_allowed(
        &state,
        &api,
        &mut access,
        &chat,
        Some("012345"),
        &mut attempts,
    )
    .await;

    assert_eq!(decision, TelegramChatAccessDecision::Consumed);
    assert!(access.is_allowed("42"));
    let config = state.config.lock().await;
    assert_eq!(
        config.telegram_accounts[0].allowed_chat_ids,
        vec!["42".to_string()]
    );
    assert_eq!(config.telegram_accounts[0].pairing_code, "012345");
    drop(config);
    let events = state.events.lock().await;
    assert!(
        events
            .iter()
            .any(|event| event.kind == "telegram_chat_bound")
    );
}

#[tokio::test]
async fn pairing_gate_accepts_start_command_payload() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let state = pairing_test_state(&temp_dir, "012345", &[]).await;
    let api = mock_telegram_api("tg_1").await;
    let mut access = TelegramChatAccess::new(Vec::new());
    let mut attempts = PairingAttempts::default();
    let chat = private_chat(77);

    let decision = ensure_chat_allowed(
        &state,
        &api,
        &mut access,
        &chat,
        Some("/start@SomeBot 012345"),
        &mut attempts,
    )
    .await;

    assert_eq!(decision, TelegramChatAccessDecision::Consumed);
    assert!(access.is_allowed("77"));
    // 已绑定后再来任意消息直接放行，不再走配对分支。
    let decision = ensure_chat_allowed(
        &state,
        &api,
        &mut access,
        &chat,
        Some("随便聊聊"),
        &mut attempts,
    )
    .await;
    assert_eq!(decision, TelegramChatAccessDecision::Allowed);
}

#[tokio::test]
async fn pairing_gate_rejects_wrong_code_and_locks_after_repeated_failures() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let state = pairing_test_state(&temp_dir, "012345", &[]).await;
    let api = mock_telegram_api("tg_1").await;
    let mut access = TelegramChatAccess::new(Vec::new());
    let mut attempts = PairingAttempts::default();
    let chat = private_chat(99);

    for _ in 0..PAIRING_MAX_FAILURES - 1 {
        let decision = ensure_chat_allowed(
            &state,
            &api,
            &mut access,
            &chat,
            Some("999999"),
            &mut attempts,
        )
        .await;
        assert_eq!(
            decision,
            TelegramChatAccessDecision::DeniedWith(PAIRING_HINT_TEXT)
        );
    }
    let decision = ensure_chat_allowed(
        &state,
        &api,
        &mut access,
        &chat,
        Some("999999"),
        &mut attempts,
    )
    .await;
    assert_eq!(
        decision,
        TelegramChatAccessDecision::DeniedWith(PAIRING_LOCKOUT_TEXT)
    );
    // 冷却期内即使提交正确配对码也静默忽略，且不写入白名单。
    let decision = ensure_chat_allowed(
        &state,
        &api,
        &mut access,
        &chat,
        Some("012345"),
        &mut attempts,
    )
    .await;
    assert_eq!(decision, TelegramChatAccessDecision::Ignored);
    assert!(!access.is_allowed("99"));
    assert!(
        state.config.lock().await.telegram_accounts[0]
            .allowed_chat_ids
            .is_empty()
    );
    let events = state.events.lock().await;
    assert!(
        events
            .iter()
            .any(|event| event.kind == "telegram_pairing_failed")
    );
}

#[tokio::test]
async fn pairing_mode_off_keeps_legacy_auto_bind_and_denial() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    // 无配对码 + 空白名单：保持旧的“首聊自动绑定”。
    let state = pairing_test_state(&temp_dir, "", &[]).await;
    let api = mock_telegram_api("tg_1").await;
    let mut access = TelegramChatAccess::new(Vec::new());
    let mut attempts = PairingAttempts::default();
    let chat = private_chat(42);

    let decision = ensure_chat_allowed(
        &state,
        &api,
        &mut access,
        &chat,
        Some("任务描述"),
        &mut attempts,
    )
    .await;
    assert_eq!(decision, TelegramChatAccessDecision::Allowed);
    assert!(access.is_allowed("42"));
    assert_eq!(
        state.config.lock().await.telegram_accounts[0].allowed_chat_ids,
        vec!["42".to_string()]
    );

    // 无配对码 + 非空白名单：其他私聊仍然拒绝。
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let state = pairing_test_state(&temp_dir, "", &["100"]).await;
    let mut access = TelegramChatAccess::new(vec!["100".to_string()]);
    let chat = private_chat(200);

    let decision = ensure_chat_allowed(
        &state,
        &api,
        &mut access,
        &chat,
        Some("012345"),
        &mut attempts,
    )
    .await;
    assert_eq!(decision, TelegramChatAccessDecision::Denied);
    assert!(!access.is_allowed("200"));
}
