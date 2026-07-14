use std::time::{SystemTime, UNIX_EPOCH};

use axum::{Json, extract::State, http::StatusCode};
use serde_json::{Map, Value, json};

use crate::{ai_gateway::catalog::configured_models_response_with_etag, app_state::SharedState};

pub(super) async fn accounts_check() -> Json<Value> {
    Json(json!({
        "account_ordering": ["acct_codexhub_local"],
        "current_account_id": "acct_codexhub_local",
        "accounts": [{
            "id": "acct_codexhub_local",
            "account_id": "acct_codexhub_local",
            "account_user_id": "user_codexhub_local__acct_codexhub_local",
            "user_id": "user_codexhub_local",
            "name": "CodexHub Local",
            "title": "CodexHub Local",
            "email": "codexhub-local@example.local",
            "plan_type": "pro",
            "structure": "personal",
            "role": "owner",
            "is_default": true,
            "is_deactivated": false,
            "is_paid": true,
        }],
    }))
}

pub(super) async fn statsig_bootstrap(State(state): State<SharedState>) -> Json<Value> {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    let model_slugs = {
        let config = state.config.lock().await;
        let (models, _) = configured_models_response_with_etag(&config.ai_gateway);
        models["models"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|model| model.get("slug").and_then(Value::as_str))
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    let payload = statsig_bootstrap_payload(now_ms, &model_slugs);
    Json(json!({
        "statsigPayload": payload.to_string(),
    }))
}

fn statsig_bootstrap_payload(now_ms: u64, model_slugs: &[String]) -> Value {
    let mut feature_gates = Map::new();
    for gate in [
        "1834314516",
        "1714131075",
        "72045066",
        "2982604767",
        "2177625257",
        "3657624089",
        "3245360288",
        "3646210497",
        "1186680773",
        "1042620455",
        "4114442250",
        "824038554",
        "410065390",
        "2296472986",
    ] {
        feature_gates.insert(
            gate.to_string(),
            json!({
                "v": true,
                "r": "codexhub-local",
                "s": [],
                "i": "userID",
            }),
        );
    }

    let model_list_config = json!({
        "available_models": model_slugs,
        "use_hidden_models": false,
        "default_model": model_slugs.first().map(String::as_str).unwrap_or("gpt-5.5"),
    });

    json!({
        "response_format": "init-v2",
        "feature_gates": feature_gates,
        "dynamic_configs": {
            "107580212": {
                "v": model_list_config.clone(),
                "r": "codexhub-local",
                "s": [],
                "i": "userID",
                "ue": false,
                "p": true
            }
        },
        "layer_configs": {
            "2096615506": {
                "v": "codexhub_primary_runtime_config",
                "r": "codexhub-local",
                "s": [],
                "i": "userID",
                "ue": false,
                "p": true
            },
            "72216192": {
                "v": "codexhub_i18n_layer_config",
                "r": "codexhub-local",
                "s": [],
                "i": "userID",
                "ue": false,
                "p": true
            }
        },
        "param_stores": {},
        "values": {
            "codexhub_model_list_config": model_list_config,
            "codexhub_primary_runtime_config": {},
            "codexhub_i18n_layer_config": {
                "enable_i18n": true,
                "locale_source": "FIRST_AVAILABLE"
            }
        },
        "exposures": {},
        "sdkParams": {},
        "sdk_flags": {},
        "has_updates": true,
        "time": now_ms,
        "user": {
            "userID": "user_codexhub_local",
            "email": "codexhub-local@example.local",
            "customIDs": {
                "account_id": "acct_codexhub_local"
            },
            "custom": {
                "auth_status": "logged_in",
                "auth_method": "chatgpt",
                "plan_type": "pro",
                "brand_name": "codex"
            }
        }
    })
}

pub(super) async fn onboarding_context() -> Json<Value> {
    Json(json!({
        "account_id": "acct_codexhub_local",
        "account_user_id": "user_codexhub_local__acct_codexhub_local",
        "completed": true,
        "requires_onboarding": false,
        "items": [],
    }))
}

pub(super) async fn usage() -> Json<Value> {
    Json(json!({
        "plan_type": "pro",
        "rate_limit": {
            "allowed": true,
            "limit_reached": false,
        },
        "credits": {
            "has_credits": true,
            "unlimited": true,
        },
    }))
}

pub(super) async fn beacons_home() -> Json<Value> {
    Json(json!({ "beacon_ui_response": Value::Null }))
}

pub(super) async fn beacons_event() -> Json<Value> {
    Json(json!({ "ok": true }))
}

pub(super) async fn tasks_list() -> Json<Value> {
    Json(json!({
        "items": [],
        "cursor": Value::Null,
    }))
}

pub(super) async fn wham_environments() -> Json<Value> {
    Json(json!({
        "items": [],
        "cursor": Value::Null,
    }))
}

pub(super) async fn wham_apps() -> Json<Value> {
    Json(json!({
        "items": [],
        "cursor": Value::Null,
    }))
}

pub(super) async fn connectors_directory_list() -> Json<Value> {
    Json(json!({
        "apps": [],
        "nextToken": Value::Null,
    }))
}

pub(super) async fn analytics_events() -> StatusCode {
    StatusCode::NO_CONTENT
}

pub(super) async fn accounts_mfa_info() -> Json<Value> {
    Json(json!({ "mfa_enabled_v2": true }))
}

pub(super) async fn remote_control_mfa_requirement() -> Json<Value> {
    Json(json!({ "requirement": "not_required" }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statsig_bootstrap_payload_matches_codex_app_sync_bootstrap_shape() {
        let response = statsig_bootstrap_payload(
            1234,
            &[
                "gpt-5.6-sol".to_string(),
                "grok-4.5".to_string(),
                "Opus-4.8".to_string(),
            ],
        );

        assert_eq!(response["has_updates"], true);
        assert_eq!(response["response_format"], "init-v2");
        assert_eq!(response["time"], 1234);
        assert_eq!(response["user"]["userID"], "user_codexhub_local");
        let gates = response["feature_gates"].as_object().unwrap();
        assert!(gates["1834314516"].is_object());
        for gate in [
            "1042620455",
            "4114442250",
            "824038554",
            "410065390",
            "2296472986",
        ] {
            assert_eq!(gates[gate]["v"], true);
        }
        for gate in ["2055603567", "3936985709"] {
            assert_ne!(
                gates
                    .get(gate)
                    .and_then(|gate| gate.get("v"))
                    .and_then(Value::as_bool),
                Some(true)
            );
        }
        assert!(response["dynamic_configs"]["107580212"].is_object());
        assert_eq!(
            response["dynamic_configs"]["107580212"]["v"]["available_models"],
            json!(["gpt-5.6-sol", "grok-4.5", "Opus-4.8"])
        );
        assert_eq!(
            response["dynamic_configs"]["107580212"]["v"]["default_model"],
            "gpt-5.6-sol"
        );
        assert_eq!(
            response["values"]["codexhub_model_list_config"],
            response["dynamic_configs"]["107580212"]["v"]
        );
        assert!(response["layer_configs"]["2096615506"].is_object());
        assert!(response["layer_configs"]["72216192"].is_object());
        assert_eq!(
            response["values"]["codexhub_i18n_layer_config"]["enable_i18n"],
            true
        );
        assert_eq!(
            response["values"]["codexhub_i18n_layer_config"]["locale_source"],
            "FIRST_AVAILABLE"
        );
    }
}
