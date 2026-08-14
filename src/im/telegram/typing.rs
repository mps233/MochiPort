use std::time::Duration;

use crate::{
    app_state::SharedState,
    im::telegram::{
        adapter::TelegramAdapter,
        api::{TelegramApi, TelegramApiError},
    },
    im_runtime::{RouteTarget, TelegramTypingSendAction},
};

const TELEGRAM_TYPING_RETRY_THROTTLE_MS: u128 = 300;
const TELEGRAM_TYPING_RENEWAL_MS: u128 = 4_000;
const TELEGRAM_TYPING_RETRY_MAX_SECONDS: u64 = 30;
const TELEGRAM_TYPING_SEND_TIMEOUT_SECONDS: u64 = 5;
const TELEGRAM_TYPING_FINISH_TIMEOUT_SECONDS: u64 = 6;
const TURN_TYPING_ITEM_PREFIX: &str = "turn:";

pub(crate) async fn start_turn(
    state: &SharedState,
    api: TelegramApi,
    thread_id: &str,
    turn_id: &str,
    route: &RouteTarget,
) {
    start(state, api, thread_id, &turn_item_id(turn_id), route).await;
}

pub(crate) async fn finish_turn(
    state: &SharedState,
    api: TelegramApi,
    thread_id: &str,
    turn_id: &str,
    route: &RouteTarget,
) {
    finish(state, api, thread_id, &turn_item_id(turn_id), route).await;
}

pub(crate) async fn turn_is_active(state: &SharedState, thread_id: &str, turn_id: &str) -> bool {
    state
        .runtime
        .lock()
        .await
        .telegram_typing_item_is_active(thread_id, &turn_item_id(turn_id))
}

pub(crate) async fn start(
    state: &SharedState,
    api: TelegramApi,
    thread_id: &str,
    item_id: &str,
    route: &RouteTarget,
) {
    let generation = state
        .runtime
        .lock()
        .await
        .start_telegram_typing(thread_id, item_id);
    if let Some(generation) = generation {
        spawn_driver(state, api, thread_id, item_id, route, generation);
    }
}

pub(crate) async fn finish(
    state: &SharedState,
    api: TelegramApi,
    thread_id: &str,
    item_id: &str,
    route: &RouteTarget,
) {
    let typing = state
        .runtime
        .lock()
        .await
        .finish_telegram_typing(thread_id, item_id);
    let Some((generation, should_start, completed)) = typing else {
        return;
    };
    if should_start {
        spawn_driver(state, api, thread_id, item_id, route, generation);
    }
    let completed_in_time = tokio::time::timeout(
        Duration::from_secs(TELEGRAM_TYPING_FINISH_TIMEOUT_SECONDS),
        async {
            loop {
                if !state
                    .runtime
                    .lock()
                    .await
                    .telegram_typing_is_active(thread_id, item_id, generation)
                {
                    return;
                }
                tokio::select! {
                    _ = completed.notified() => {}
                    _ = tokio::time::sleep(Duration::from_millis(50)) => {}
                }
            }
        },
    )
    .await
    .is_ok();
    if !completed_in_time {
        let cancelled = state
            .runtime
            .lock()
            .await
            .cancel_telegram_typing_generation(thread_id, item_id, generation);
        if cancelled {
            state
                .push_event(
                    "warn",
                    "telegram_typing_finish_timeout",
                    format!(
                        "thread={thread_id} item={item_id} chat={} generation={generation}",
                        route.chat_id
                    ),
                )
                .await;
        }
    }
}

fn spawn_driver(
    state: &SharedState,
    api: TelegramApi,
    thread_id: &str,
    item_id: &str,
    route: &RouteTarget,
    generation: i64,
) {
    let state = state.clone();
    let thread_id = thread_id.to_string();
    let item_id = item_id.to_string();
    let route = route.clone();
    tokio::spawn(async move {
        typing_driver(state, api, thread_id, item_id, route, generation).await;
    });
}

async fn typing_driver(
    state: SharedState,
    api: TelegramApi,
    thread_id: String,
    item_id: String,
    route: RouteTarget,
    generation: i64,
) {
    let adapter = TelegramAdapter::new(api);
    let mut consecutive_failures = 0_u32;
    loop {
        let now_ms = crate::types::now_ms();
        let (send_delay_ms, wait_for_update) = {
            let runtime = state.runtime.lock().await;
            if let Some(delay_ms) = runtime.telegram_typing_send_delay_ms(
                &thread_id,
                &item_id,
                generation,
                now_ms,
                TELEGRAM_TYPING_RETRY_THROTTLE_MS,
            ) {
                (Some(delay_ms), None)
            } else if let Some(wait) = runtime.telegram_typing_wait_for_update(
                &thread_id,
                &item_id,
                generation,
                now_ms,
                TELEGRAM_TYPING_RENEWAL_MS,
            ) {
                (None, Some(wait))
            } else {
                return;
            }
        };

        if let Some((wake_driver, heartbeat_delay_ms)) = wait_for_update {
            tokio::select! {
                _ = wake_driver.notified() => {}
                _ = tokio::time::sleep(Duration::from_millis(heartbeat_delay_ms)) => {
                    if !state.runtime.lock().await.mark_telegram_typing_renewal_due(
                        &thread_id,
                        &item_id,
                        generation,
                    ) {
                        return;
                    }
                }
            }
            continue;
        }

        if let Some(delay_ms) = send_delay_ms
            && delay_ms > 0
        {
            let wake_driver = {
                let runtime = state.runtime.lock().await;
                runtime.telegram_typing_wake_driver(&thread_id, &item_id, generation)
            };
            if let Some(wake_driver) = wake_driver {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(delay_ms)) => {}
                    _ = wake_driver.notified() => {}
                }
            } else {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
        }

        let snapshot = state.runtime.lock().await.take_telegram_typing_snapshot(
            &thread_id,
            &item_id,
            generation,
            crate::types::now_ms(),
        );
        let Some((finished, revision)) = snapshot else {
            return;
        };
        let result = if finished {
            Ok(())
        } else {
            match tokio::time::timeout(
                Duration::from_secs(TELEGRAM_TYPING_SEND_TIMEOUT_SECONDS),
                adapter.send_typing_action(&route.chat_id),
            )
            .await
            {
                Ok(result) => result,
                Err(_) => Err(anyhow::anyhow!(
                    "telegram sendChatAction timed out after {TELEGRAM_TYPING_SEND_TIMEOUT_SECONDS}s"
                )),
            }
        };
        let retry_delay = match &result {
            Ok(()) => {
                consecutive_failures = 0;
                None
            }
            Err(error) => {
                consecutive_failures = consecutive_failures.saturating_add(1);
                Some(typing_retry_delay(error, consecutive_failures))
            }
        };
        if !finished {
            match &result {
                Ok(()) => {
                    state
                        .push_event(
                            "info",
                            "telegram_typing_sent",
                            format!(
                                "thread={thread_id} item={item_id} chat={} generation={generation}",
                                route.chat_id
                            ),
                        )
                        .await;
                }
                Err(err) => {
                    state
                        .push_event(
                            "warn",
                            "telegram_typing_failed",
                            format!(
                                "thread={thread_id} item={item_id} chat={} generation={generation} err={err}",
                                route.chat_id
                            ),
                        )
                        .await;
                }
            }
        }
        let action = state.runtime.lock().await.complete_telegram_typing_send(
            &thread_id,
            &item_id,
            generation,
            revision,
            result.is_ok(),
        );
        if action == TelegramTypingSendAction::Stop {
            if !finished
                && item_id.starts_with(TURN_TYPING_ITEM_PREFIX)
                && let Some(retry_delay) = retry_delay
            {
                tokio::time::sleep(retry_delay).await;
                let restarted = state
                    .runtime
                    .lock()
                    .await
                    .start_telegram_typing(&thread_id, &item_id);
                if restarted == Some(generation) {
                    continue;
                }
            }
            return;
        }
    }
}

fn turn_item_id(turn_id: &str) -> String {
    format!("{TURN_TYPING_ITEM_PREFIX}{turn_id}")
}

fn typing_retry_delay(error: &anyhow::Error, consecutive_failures: u32) -> Duration {
    if let Some(retry_after) = error
        .downcast_ref::<TelegramApiError>()
        .and_then(|error| error.retry_after)
    {
        return Duration::from_secs(retry_after.clamp(1, TELEGRAM_TYPING_RETRY_MAX_SECONDS));
    }
    let exponent = consecutive_failures.saturating_sub(1).min(4);
    Duration::from_secs(
        2_u64
            .saturating_mul(1_u64 << exponent)
            .min(TELEGRAM_TYPING_RETRY_MAX_SECONDS),
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use reqwest::StatusCode;

    use super::{turn_item_id, typing_retry_delay};
    use crate::im::telegram::api::TelegramApiError;
    use crate::im_runtime::RuntimeState;

    #[test]
    fn completed_turn_indicator_cannot_restart_after_an_early_output() {
        let mut runtime = RuntimeState::default();
        let item_id = turn_item_id("turn-1");

        assert!(runtime.finish_telegram_typing("thread", &item_id).is_none());
        assert!(runtime.start_telegram_typing("thread", &item_id).is_none());
        assert!(
            runtime
                .start_telegram_typing("thread", &turn_item_id("turn-2"))
                .is_some()
        );
    }

    #[test]
    fn retry_delay_honors_server_backoff_and_caps_generic_failures() {
        let server_error = anyhow::Error::new(TelegramApiError {
            method: "sendChatAction".to_string(),
            status: StatusCode::TOO_MANY_REQUESTS,
            error_code: Some(429),
            description: "retry later".to_string(),
            retry_after: Some(7),
        });
        assert_eq!(typing_retry_delay(&server_error, 1), Duration::from_secs(7));

        let generic_error = anyhow::anyhow!("network unavailable");
        assert_eq!(
            typing_retry_delay(&generic_error, 1),
            Duration::from_secs(2)
        );
        assert_eq!(
            typing_retry_delay(&generic_error, 10),
            Duration::from_secs(30)
        );
    }
}
