//! Management API contract catalogue.
//!
//! The macOS and Windows clients are generated from the JSON Schema this module
//! reflects, so the daemon stays the single source of truth for the shape of the
//! management API. Listing a type here is what publishes it to the generated
//! Swift and TypeScript contracts; no other file needs editing to add one.
//!
//! The catalogue is never constructed at runtime. It exists so `schemars` can
//! walk every type reachable from it, and so the exported schema has a single
//! stable entry point.

use schemars::JsonSchema;

use crate::codex::app_config::CodexAppConfigStatus;
use crate::config::AppConfig;

use super::{
    InstanceShutdownRequest, ManageDashboardResponse, ManageLogDirectoryResponse,
    RemoteControlBackendStatusResponse, StatusResponse, codex_app, im_api, manage,
    manage_workspace, onboarding, usage_api,
};

/// Every request and response shape the desktop clients consume.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub(crate) struct ManageContractCatalogue {
    app_config: AppConfig,
    codex_app_config_status: CodexAppConfigStatus,

    // Daemon status and lifecycle.
    manage_status: manage::ManageStatusResponse,
    health: manage::HealthResponse,
    lifecycle: manage::LifecycleResponse,
    lifecycle_control: manage::LifecycleControlRequest,
    lifecycle_lease: manage::LifecycleLeaseRequest,
    lifecycle_lease_takeover: manage::LifecycleLeaseTakeoverRequest,
    lifecycle_credential_rotate: manage::LifecycleCredentialRotateRequest,
    instance_shutdown: InstanceShutdownRequest,
    status: StatusResponse,

    // Dashboard and workspace.
    dashboard: ManageDashboardResponse,
    log_directory: ManageLogDirectoryResponse,
    remote_control_backend_status: RemoteControlBackendStatusResponse,
    update_gateway: manage_workspace::UpdateGatewayRequest,
    upsert_provider: manage_workspace::UpsertProviderRequest,
    delete_provider: manage_workspace::DeleteProviderRequest,
    fetch_provider_models: manage_workspace::FetchProviderModelsRequest,
    fetch_provider_usage: manage_workspace::FetchProviderUsageRequest,
    update_settings: manage_workspace::UpdateSettingsRequest,
    clear_old_request_logs: manage_workspace::ClearOldRequestLogsRequest,
    fetch_sub2api_accounts: manage_workspace::FetchSub2ApiAccountsRequest,
    set_sub2api_account_schedulable: manage_workspace::SetSub2ApiAccountSchedulableRequest,
    update_sub2api_admin: manage_workspace::UpdateSub2ApiAdminRequest,

    // Local Codex usage.
    usage_summary: usage_api::UsageSummaryResponse,
    usage_totals: usage_api::UsageTotals,
    usage_day_entry: usage_api::UsageDayEntry,
    usage_hour_entry: usage_api::UsageHourEntry,
    usage_project_entry: usage_api::UsageProjectEntry,
    usage_model_entry: usage_api::UsageModelEntry,
    usage_provider_entry: usage_api::UsageProviderEntry,
    usage_breakdown_row: usage_api::UsageBreakdownRow,
    usage_quota_snapshot: usage_api::UsageQuotaSnapshot,
    usage_quota_window: usage_api::UsageQuotaWindow,
    usage_minute_bucket: usage_api::UsageMinuteBucket,
    usage_quota_history_point: usage_api::UsageQuotaHistoryPoint,
    usage_quota_point: usage_api::UsageQuotaPoint,

    // Codex desktop integration.
    codex_app_status: codex_app::ManageCodexAppStatus,
    configure_codex_app: codex_app::ConfigureCodexAppRequest,
    delete_codex_app_provider: codex_app::DeleteCodexAppProviderRequest,
    set_codex_app_provider_websocket: codex_app::SetCodexAppProviderWebSocketRequest,
    enhanced_launch_operation: codex_app::EnhancedLaunchOperationRequest,

    // Messaging channels.
    manage_im_accounts: im_api::ManageImAccountsResponse,
    im_accounts: im_api::ImAccountsResponse,
    configure_telegram_bot: im_api::ConfigureTelegramBotRequest,
    rotate_telegram_pairing_code: im_api::RotateTelegramPairingCodeRequest,
    sync_telegram_topics: im_api::SyncTelegramTopicsRequest,
    set_telegram_reply_granularity: im_api::SetTelegramReplyGranularityRequest,
    telegram_project_groups: im_api::TelegramProjectGroupsResponse,
    update_telegram_project_groups: im_api::UpdateTelegramProjectGroupsRequest,
    delete_im_account: im_api::DeleteImAccountRequest,
    set_im_account_enabled: im_api::SetImAccountEnabledRequest,
    set_im_channel_enabled: im_api::SetImChannelEnabledRequest,
    telegram_bot_status: im_api::TelegramBotStatus,
    feishu_bot_status: im_api::FeishuBotStatus,
    wechat_bot_status: im_api::WechatBotStatus,
    wecom_bot_status: im_api::WecomBotStatus,

    // Onboarding.
    configure_feishu_account: onboarding::ConfigureFeishuAccountRequest,
    wechat_onboard_poll: onboarding::WechatOnboardPollRequest,
    wecom_onboard_poll: onboarding::WecomOnboardPollRequest,
}

/// Reflects the catalogue into the JSON Schema the client generator consumes.
///
/// Test-only: exporting the schema is a build step, not a runtime capability.
#[cfg(test)]
pub(crate) fn contract_schema() -> schemars::Schema {
    schemars::schema_for!(ManageContractCatalogue)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// Writes the schema that `scripts/generate-client-contracts.py` reads.
    ///
    /// Run this after changing any contract type, then regenerate the clients:
    ///
    /// ```text
    /// cargo test --lib write_management_contract_schema
    /// python3 scripts/generate-client-contracts.py contracts/manage-contracts.schema.json \
    ///     --swift macos/MochiPort/Sources/MochiPortMac/Generated/ManageContracts.swift \
    ///     --typescript windows/MochiPort/src/api/generated/manageContracts.ts
    /// ```
    #[test]
    fn write_management_contract_schema() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let directory = root.join("contracts");
        std::fs::create_dir_all(&directory).expect("create contracts directory");

        let mut rendered =
            serde_json::to_string_pretty(&contract_schema()).expect("serialize contract schema");
        rendered.push('\n');
        std::fs::write(directory.join("manage-contracts.schema.json"), rendered)
            .expect("write contract schema");
    }

    /// The generated clients depend on these names, so a rename has to be a
    /// deliberate change rather than a silent one.
    #[test]
    fn catalogue_publishes_the_expected_contract_types() {
        let schema = serde_json::to_value(contract_schema()).expect("serialize contract schema");
        let definitions = schema
            .get("$defs")
            .and_then(|value| value.as_object())
            .expect("schema has $defs");
        for expected in [
            "ManageStatusResponse",
            "ManageDashboardResponse",
            "UpdateGatewayRequest",
            "TelegramProjectGroupsResponse",
        ] {
            assert!(
                definitions.contains_key(expected),
                "contract type {expected} is missing from the exported schema"
            );
        }
    }
}
