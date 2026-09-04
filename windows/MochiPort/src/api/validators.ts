import type {
  CodexEnhancedOperation,
  CodexEnhancedOperationResponse,
  CodexEnhancedPreflight,
  CodexCatalogModel,
  CodexSession,
  CodexStatus,
  Dashboard,
  Gateway,
  GatewayProviderModelsResponse,
  GatewayProviderRecentAccountResponse,
  GatewayProviderTemplate,
  GatewayProviderUsageResponse,
  HealthResponse,
  IMAccount,
  Lifecycle,
  LifecycleCredentialMutationResponse,
  RequestLog,
  RequestLogDetail,
  RequestLogsResponse,
  Settings,
  Sub2ApiAdmin,
  Sub2ApiPool,
  TelegramProjectGroup,
  TelegramProjectGroupAccount,
  TelegramProjectGroupsMutationResponse,
  TelegramProjectGroupsResponse,
} from "./types";

export type Validator<T> = (value: unknown) => value is T;

type JsonObject = Record<string, unknown>;

const isObject = (value: unknown): value is JsonObject =>
  typeof value === "object" && value !== null && !Array.isArray(value);
const isString = (value: unknown): value is string => typeof value === "string";
const isBoolean = (value: unknown): value is boolean => typeof value === "boolean";
const isNumber = (value: unknown): value is number => typeof value === "number" && Number.isFinite(value);
const isInteger = (value: unknown): value is number => isNumber(value) && Number.isInteger(value);
const isNullableString = (value: unknown): value is string | null | undefined =>
  value === undefined || value === null || isString(value);
const isNullableNumber = (value: unknown): value is number | null | undefined =>
  value === undefined || value === null || isNumber(value);
const isNullableBoolean = (value: unknown): value is boolean | null | undefined =>
  value === undefined || value === null || isBoolean(value);
const isStringArray = (value: unknown): value is string[] => Array.isArray(value) && value.every(isString);

function hasServiceIdentity(value: unknown): boolean {
  return isObject(value)
    && isString(value.service)
    && isInteger(value.apiMajor)
    && isBoolean(value.ready)
    && isString(value.instanceId)
    && value.instanceId.length > 0
    && isInteger(value.pid)
    && isNumber(value.startedAtMs);
}

function hasEndpointStatus(value: unknown): boolean {
  return isObject(value) && isBoolean(value.configured) && isBoolean(value.connected);
}

function hasMessageChannel(value: unknown): boolean {
  return isObject(value) && isInteger(value.accountCount) && isInteger(value.connectedAccountCount);
}

export const isHealthResponse: Validator<HealthResponse> = (value): value is HealthResponse =>
  isObject(value)
  && isString(value.service)
  && isInteger(value.apiMajor)
  && isBoolean(value.ready);

export interface LogDirectoryResponse {
  directory: string;
  instanceId: string;
}

export const isLogDirectoryResponse: Validator<LogDirectoryResponse> =
  (value): value is LogDirectoryResponse => isObject(value)
    && isString(value.directory)
    && isString(value.instanceId);

export const isDashboard: Validator<Dashboard> = (value): value is Dashboard => {
  if (!isObject(value) || !hasServiceIdentity(value.service)) return false;
  if (!isBoolean(value.bridgeRunning)
    || !isBoolean(value.remoteControlConnected)
    || !isBoolean(value.remoteControlHealthy)
    || !isBoolean(value.aiGatewayEnabled)
    || !isInteger(value.aiGatewayProviderCount)
    || !isBoolean(value.requestLoggingEnabled)) return false;
  // Daemons released before the execution-client split exposed only
  // `codexAppConfigured`. Normalize that response just like the macOS decoder
  // does so a supported v1 daemon does not become unusable on Windows.
  if (!isObject(value.executionClients) && isBoolean(value.codexAppConfigured)) {
    value.executionClients = {
      codexApp: { configured: value.codexAppConfigured, connected: false },
      vscode: { configured: false, connected: false },
      cli: { configured: false, connected: false },
    };
  }
  if (!isObject(value.executionClients)
    || !hasEndpointStatus(value.executionClients.codexApp)
    || !hasEndpointStatus(value.executionClients.vscode)
    || !hasEndpointStatus(value.executionClients.cli)) return false;

  // Older v1 responses reported aggregate IM counts. Preserve those totals in
  // an unattributed bucket instead of pretending that every channel is empty.
  if (!isObject(value.messageChannels)
    && isInteger(value.imAccountCount)
    && isInteger(value.connectedImAccountCount)) {
    value.messageChannels = {
      telegram: { accountCount: 0, connectedAccountCount: 0 },
      feishu: { accountCount: 0, connectedAccountCount: 0 },
      wechat: { accountCount: 0, connectedAccountCount: 0 },
      wecom: { accountCount: 0, connectedAccountCount: 0 },
      legacyUnattributed: {
        accountCount: value.imAccountCount,
        connectedAccountCount: value.connectedImAccountCount,
      },
    };
  }
  return isObject(value.messageChannels)
    && hasMessageChannel(value.messageChannels.telegram)
    && hasMessageChannel(value.messageChannels.feishu)
    && hasMessageChannel(value.messageChannels.wechat)
    && hasMessageChannel(value.messageChannels.wecom)
    && (value.messageChannels.legacyUnattributed === undefined
      || hasMessageChannel(value.messageChannels.legacyUnattributed));
};

export const isLifecycle: Validator<Lifecycle> = (value): value is Lifecycle => {
  if (!isObject(value)
    || !hasServiceIdentity(value.service)
    || !isString(value.executable)
    || !isNullableString(value.executableSha256)
    || !isString(value.configPath)
    || !isString(value.bind)) return false;
  if (!isObject(value.runtime)
    || !isString(value.runtime.state)
    || !isString(value.runtime.productVersion)
    || !isNullableNumber(value.runtime.buildNumber)
    || !isInteger(value.runtime.apiMajor)) return false;
  if (!isObject(value.protectedWorkItems)
    || !isInteger(value.protectedWorkItems.aiGatewayRequests)
    || !isInteger(value.protectedWorkItems.codexTurns)
    || !isInteger(value.protectedWorkItems.imStreams)
    || !isInteger(value.protectedWorkItems.pendingApprovals)
    || !isInteger(value.protectedWorkItems.remoteControlRequests)
    || !isInteger(value.protectedWorkItems.total)) return false;
  return isObject(value.management)
    && isString(value.management.state)
    && isString(value.management.mode)
    && isBoolean(value.management.canControl)
    && isNullableString(value.management.installationId)
    && isNullableNumber(value.management.leaseGeneration)
    && isNullableNumber(value.management.leaseExpiresAtMs)
    && isNullableNumber(value.management.managementTokenGeneration);
};

export const isLifecycleCredentialMutationResponse: Validator<LifecycleCredentialMutationResponse> =
  (value): value is LifecycleCredentialMutationResponse => isObject(value)
    && value.ok === true
    && isBoolean(value.rotated)
    && isString(value.requestId)
    && value.requestId.length > 0
    && isInteger(value.managementTokenGeneration)
    && value.managementTokenGeneration > 0;

function isCodexProvider(value: unknown): boolean {
  return isObject(value)
    && isString(value.name)
    && isNullableString(value.baseUrl)
    && isBoolean(value.secretSet)
    && isBoolean(value.supportsWebsockets);
}

export const isCodexStatus: Validator<CodexStatus> = (value): value is CodexStatus => {
  if (!isObject(value)
    || !isString(value.codexHome)
    || !isBoolean(value.configured)
    || !isBoolean(value.configOk)
    || !isBoolean(value.authOk)
    || !isBoolean(value.providerOk)
    || !isNullableString(value.configError)
    || !isNullableString(value.authError)
    || !isBoolean(value.guiConfigured)
    || !isNullableString(value.guiError)
    || !isBoolean(value.remoteControlSupported)
    || !isBoolean(value.remoteControlConfigured)
    || !isNullableString(value.remoteControlError)
    || !Array.isArray(value.providers)
    || !value.providers.every(isCodexProvider)
    || !isBoolean(value.imageGenerationEnabled)
    || !isString(value.connectionMode)
    || !isNullableString(value.providerMode)
    || !isNullableString(value.providerModeMessage)
    || !isNullableString(value.activeProvider)) return false;
  return value.providerMode === undefined
    || value.providerMode === null
    || value.providerMode === "mochiport"
    || value.providerMode === "threadrelay"
    || value.providerMode === "direct-api"
    || value.providerMode === "unknown";
};

function isCodexSession(value: unknown): value is CodexSession {
  if (!isObject(value) || !isString(value.id) || value.id.length === 0) return false;
  // Keep parity with the macOS compatibility decoder. These fields were
  // absent in early session-history responses but have stable safe defaults.
  if (value.preview === undefined || value.preview === null) value.preview = "";
  if (value.modelProvider === undefined || value.modelProvider === null) value.modelProvider = "openai";
  if (value.updatedAt === undefined || value.updatedAt === null) value.updatedAt = 0;
  return isObject(value)
    && isString(value.preview)
    && isString(value.modelProvider)
    && isNumber(value.updatedAt)
    && isNullableString(value.path)
    && isNullableString(value.name)
    && isNullableString(value.cwd);
}

export interface CodexSessionsResponse {
  ok: boolean;
  threads: CodexSession[];
  providers: string[];
  total?: number;
}

export const isCodexSessionsResponse: Validator<CodexSessionsResponse> = (value): value is CodexSessionsResponse =>
  isObject(value)
  && value.ok === true
  && Array.isArray(value.threads)
  && value.threads.every(isCodexSession)
  && isStringArray(value.providers)
  && (value.total === undefined || isInteger(value.total));

const enhancedPhases = new Set([
  "preparing",
  "launching",
  "waitingForApp",
  "injecting",
  "ready",
  "failed",
  "cancelled",
]);

function isCodexEnhancedOperation(value: unknown): value is CodexEnhancedOperation {
  return isObject(value)
    && isString(value.requestId)
    && value.requestId.length > 0
    && isString(value.phase)
    && enhancedPhases.has(value.phase)
    && isNumber(value.startedAtMs)
    && isNumber(value.updatedAtMs)
    && isBoolean(value.canCancel)
    && isString(value.message)
    && isNullableString(value.error)
    && isNullableString(value.recovery);
}

export const isCodexEnhancedPreflight: Validator<CodexEnhancedPreflight> = (value): value is CodexEnhancedPreflight =>
  isObject(value)
  && value.ok === true
  && isObject(value.status)
  && isBoolean(value.status.running);

export const isCodexEnhancedOperationResponse: Validator<CodexEnhancedOperationResponse> =
  (value): value is CodexEnhancedOperationResponse => isObject(value)
    && value.ok === true
    && (value.operation === undefined || value.operation === null || isCodexEnhancedOperation(value.operation));

function isGatewayProvider(value: unknown): boolean {
  return isObject(value)
    && isString(value.name)
    && isBoolean(value.enabled)
    && isString(value.providerType)
    && isNullableString(value.compatibility)
    && isString(value.baseUrl)
    && isNullableString(value.modelsUrl)
    && isStringArray(value.models)
    && isObject(value.modelAliases)
    && Object.values(value.modelAliases).every(isString)
    && isNullableString(value.promptCacheRetention)
    && isNumber(value.weight)
    && isNumber(value.timeoutSecs)
    && isBoolean(value.secretSet);
}

export const isGateway: Validator<Gateway> = (value): value is Gateway =>
  isObject(value)
  && isBoolean(value.enabled)
  && isBoolean(value.filterImageGenerationTool)
  && isBoolean(value.requestLoggingEnabled)
  && isBoolean(value.requestLogDetailsEnabled)
  && isStringArray(value.codexVisibleModels)
  && Array.isArray(value.providers)
  && value.providers.every(isGatewayProvider);

export const isSettings: Validator<Settings> = (value): value is Settings =>
  isObject(value)
  && isNullableString(value.language)
  && isNullableString(value.theme)
  && isString(value.localConnectionMode)
  && isString(value.bind)
  && isObject(value.outboundProxy)
  && isString(value.outboundProxy.mode)
  && isString(value.outboundProxy.url)
  && isBoolean(value.outboundProxy.credentialSet);

export interface SettingsMutationResponse {
  ok: true;
  settings: Settings;
}

export const isSettingsMutationResponse: Validator<SettingsMutationResponse> =
  (value): value is SettingsMutationResponse => isObject(value)
    && value.ok === true
    && isSettings(value.settings);

function isIMAccount(value: unknown): value is IMAccount {
  return isObject(value)
    && isString(value.platform)
    && isString(value.accountId)
    && isNullableString(value.displayName)
    && isNullableString(value.avatarData)
    && isBoolean(value.enabled)
    && isBoolean(value.configured)
    && isBoolean(value.secretSet)
    && isBoolean(value.connecting)
    && isBoolean(value.polling)
    && isBoolean(value.connected)
    && isNullableString(value.lastError)
    && isNullableNumber(value.lastEventAtMs)
    && isNullableNumber(value.lastInboundAtMs);
}

export interface IMAccountsResponse {
  accounts: IMAccount[];
}

export const isIMAccountsResponse: Validator<IMAccountsResponse> = (value): value is IMAccountsResponse =>
  isObject(value) && Array.isArray(value.accounts) && value.accounts.every(isIMAccount);

function isTelegramProjectGroup(value: unknown): value is TelegramProjectGroup {
  return isObject(value)
    && isString(value.chatId)
    && isString(value.projectName)
    && isString(value.cwd);
}

function isTelegramProjectGroupAccount(value: unknown): value is TelegramProjectGroupAccount {
  return isObject(value)
    && isString(value.accountId)
    && Array.isArray(value.projectGroups)
    && value.projectGroups.every(isTelegramProjectGroup);
}

export const isTelegramProjectGroupsResponse: Validator<TelegramProjectGroupsResponse> =
  (value): value is TelegramProjectGroupsResponse => isObject(value)
    && Array.isArray(value.accounts)
    && value.accounts.every(isTelegramProjectGroupAccount);

export const isTelegramProjectGroupsMutationResponse: Validator<TelegramProjectGroupsMutationResponse> =
  (value): value is TelegramProjectGroupsMutationResponse => isObject(value)
    && value.ok === true
    && isString(value.accountId)
    && Array.isArray(value.projectGroups)
    && value.projectGroups.every(isTelegramProjectGroup)
    && isBoolean(value.restartRequired);

function isRequestLog(value: unknown): value is RequestLog {
  return isObject(value)
    && isNumber(value.id)
    && isString(value.requestId)
    && isString(value.modelId)
    && isBoolean(value.stream)
    && isString(value.channel)
    && isString(value.providerType)
    && isString(value.status)
    && isNullableNumber(value.inputTokens)
    && isNullableNumber(value.outputTokens)
    && isNullableNumber(value.totalTokens)
    && isNullableNumber(value.readCacheTokens)
    && isNullableNumber(value.readCacheHitRate)
    && isNullableNumber(value.writeCacheTokens)
    && isNullableNumber(value.writeCache5mTokens)
    && isNullableNumber(value.writeCache1hTokens)
    && isNullableNumber(value.costUsd)
    && isNullableNumber(value.latencyMs)
    && isNullableNumber(value.ttftMs)
    && isNumber(value.createdAtMs)
    && isString(value.createdAt)
    && isNullableString(value.errorMessage)
    && isNullableNumber(value.upstreamRequestBodyBytes);
}

export const isRequestLogsResponse: Validator<RequestLogsResponse> = (value): value is RequestLogsResponse =>
  isObject(value)
  && Array.isArray(value.logs)
  && value.logs.every(isRequestLog)
  && isNullableString(value.nextCursor)
  && (value.hasMore === undefined || isBoolean(value.hasMore));

function isRequestLogDetail(value: unknown): value is RequestLogDetail {
  if (!isRequestLog(value)) return false;
  const detail = value as unknown as JsonObject;
  return isNullableString(detail.requestHeadersJson)
    && isNullableString(detail.requestJson)
    && isNullableString(detail.upstreamRequestHeadersJson)
    && isNullableString(detail.upstreamRequestJson)
    && isNullableString(detail.upstreamResponseSse)
    && isNullableString(detail.responseJson);
}

export interface RequestLogDetailResponse {
  log: RequestLogDetail;
}

export const isRequestLogDetailResponse: Validator<RequestLogDetailResponse> =
  (value): value is RequestLogDetailResponse => isObject(value) && isRequestLogDetail(value.log);

export interface ActionResponse {
  ok: true;
  deleted?: number;
}

export const isActionResponse: Validator<ActionResponse> = (value): value is ActionResponse =>
  isObject(value)
  && value.ok === true
  && (value.deleted === undefined || isInteger(value.deleted));

export const isOkResponse: Validator<{ ok: true }> = (value): value is { ok: true } =>
  isActionResponse(value);

export interface GatewayMutationResponse {
  ok: true;
  gateway: Gateway;
}

export const isGatewayMutationResponse: Validator<GatewayMutationResponse> =
  (value): value is GatewayMutationResponse => isObject(value)
    && value.ok === true
    && isGateway(value.gateway);

function isGatewayProviderTemplate(value: unknown): value is GatewayProviderTemplate {
  return isObject(value)
    && isString(value.id)
    && isString(value.displayName)
    && isString(value.providerType)
    && isNullableString(value.compatibility)
    && isString(value.baseUrl)
    && isNullableString(value.modelsUrl)
    && isStringArray(value.models);
}

export interface ProviderTemplatesResponse {
  templates: GatewayProviderTemplate[];
}

export const isProviderTemplatesResponse: Validator<ProviderTemplatesResponse> =
  (value): value is ProviderTemplatesResponse => isObject(value)
    && Array.isArray(value.templates)
    && value.templates.every(isGatewayProviderTemplate);

function isCodexCatalogModel(value: unknown): value is CodexCatalogModel {
  return isObject(value)
    && isString(value.id)
    && value.id.length > 0
    && isString(value.displayName)
    && value.displayName.length > 0;
}

export interface CodexModelCatalogResponse {
  models: CodexCatalogModel[];
}

export const isCodexModelCatalogResponse: Validator<CodexModelCatalogResponse> =
  (value): value is CodexModelCatalogResponse => isObject(value)
    && Array.isArray(value.models)
    && value.models.every(isCodexCatalogModel);

function isProviderModelAttempt(value: unknown): boolean {
  return isObject(value)
    && isString(value.url)
    && isNullableNumber(value.status)
    && isNullableString(value.error)
    && isNullableString(value.preview);
}

export const isGatewayProviderModelsResponse: Validator<GatewayProviderModelsResponse> =
  (value): value is GatewayProviderModelsResponse => isObject(value)
    && isBoolean(value.ok)
    && isStringArray(value.models)
    && Array.isArray(value.attempts)
    && value.attempts.every(isProviderModelAttempt);

function isProviderUsage(value: unknown): boolean {
  return isObject(value)
    && isString(value.source)
    && isString(value.balanceStatus)
    && isString(value.billingStatus)
    && isNullableNumber(value.remaining)
    && isBoolean(value.unlimited)
    && isNullableString(value.unit)
    && isNullableString(value.balanceMode)
    && isNullableString(value.planName)
    && isNullableBoolean(value.accountValid)
    && isNullableString(value.accountStatus)
    && isNullableNumber(value.todayCost)
    && isNullableNumber(value.todayActualCost)
    && isNullableNumber(value.groupRateMultiplier)
    && isNullableNumber(value.userRateMultiplier)
    && isNullableNumber(value.resolvedRateMultiplier)
    && isNullableNumber(value.effectiveRateMultiplier)
    && isNullableBoolean(value.peakRateEnabled)
    && isNullableString(value.peakStart)
    && isNullableString(value.peakEnd)
    && isNullableNumber(value.peakRateMultiplier)
    && isNullableNumber(value.appliedPeakMultiplier)
    && isNullableString(value.timezone)
    && isNullableString(value.observedAt);
}

export const isGatewayProviderUsageResponse: Validator<GatewayProviderUsageResponse> =
  (value): value is GatewayProviderUsageResponse => isObject(value)
    && isBoolean(value.ok)
    && isString(value.providerName)
    && isProviderUsage(value.usage);

export const isSub2ApiAdmin: Validator<Sub2ApiAdmin> = (value): value is Sub2ApiAdmin =>
  isObject(value)
  && isBoolean(value.configured)
  && isString(value.baseUrl)
  && isBoolean(value.secretSet);

export interface Sub2ApiAdminMutationResponse {
  ok: true;
  sub2api: Sub2ApiAdmin;
}

export const isSub2ApiAdminMutationResponse: Validator<Sub2ApiAdminMutationResponse> =
  (value): value is Sub2ApiAdminMutationResponse => isObject(value)
    && value.ok === true
    && isSub2ApiAdmin(value.sub2api);

function isSub2ApiAccount(value: unknown): boolean {
  return isObject(value)
    && isInteger(value.id)
    && isString(value.name)
    && isNullableString(value.siteUrl)
    && isString(value.platform)
    && isString(value.accountType)
    && isString(value.status)
    && isBoolean(value.schedulable)
    && isNullableNumber(value.localRateMultiplier)
    && isObject(value.upstreamBilling)
    && isString(value.upstreamBilling.state)
    && isNullableNumber(value.upstreamBilling.resolvedRateMultiplier)
    && isNullableNumber(value.upstreamBilling.effectiveRateMultiplier)
    && isNullableString(value.upstreamBilling.observedAt)
    && isNullableString(value.upstreamBilling.freshUntil)
    && isBoolean(value.upstreamBilling.stale)
    && isObject(value.upstreamBalance)
    && isString(value.upstreamBalance.state)
    && isNullableNumber(value.upstreamBalance.remaining)
    && isBoolean(value.upstreamBalance.unlimited)
    && isNullableString(value.upstreamBalance.unit)
    && isNullableString(value.upstreamBalance.mode)
    && isNullableString(value.upstreamBalance.planName)
    && isNullableBoolean(value.upstreamBalance.accountValid)
    && isNullableString(value.upstreamBalance.accountStatus)
    && isNullableString(value.upstreamBalance.observedAt);
}

function isSub2ApiPool(value: unknown): value is Sub2ApiPool {
  return isObject(value)
    && isString(value.source)
    && isNumber(value.fetchedAtMs)
    && Array.isArray(value.accounts)
    && value.accounts.every(isSub2ApiAccount)
    && (value.warnings === undefined || value.warnings === null || isStringArray(value.warnings));
}

export interface Sub2ApiPoolResponse {
  ok: true;
  pool: Sub2ApiPool;
}

export const isSub2ApiPoolResponse: Validator<Sub2ApiPoolResponse> =
  (value): value is Sub2ApiPoolResponse => isObject(value)
    && value.ok === true
    && isSub2ApiPool(value.pool);

export const isGatewayProviderRecentAccountResponse: Validator<GatewayProviderRecentAccountResponse> =
  (value): value is GatewayProviderRecentAccountResponse => isObject(value)
    && isBoolean(value.ok)
    && isString(value.providerName)
    && (value.account === undefined
      || value.account === null
      || (isObject(value.account)
        && isInteger(value.account.accountId)
        && isString(value.account.accountName)
        && isString(value.account.createdAt)));

export interface FeishuOnboardingStartResponse {
  verificationUri: string;
  verificationUriComplete: string;
  deviceCode: string;
  expiresIn: number;
  interval: number;
  qrSvg: string;
}

export const isFeishuOnboardingStartResponse: Validator<FeishuOnboardingStartResponse> =
  (value): value is FeishuOnboardingStartResponse => isObject(value)
    && isString(value.verificationUri)
    && isString(value.verificationUriComplete)
    && isString(value.deviceCode)
    && isNumber(value.expiresIn)
    && isNumber(value.interval)
    && isString(value.qrSvg);

export interface FeishuOnboardingPollResponse {
  done: boolean;
  appId?: string | null;
  displayName?: string | null;
  error?: string | null;
  errorDescription?: string | null;
}

export const isFeishuOnboardingPollResponse: Validator<FeishuOnboardingPollResponse> =
  (value): value is FeishuOnboardingPollResponse => isObject(value)
    && isBoolean(value.done)
    && isNullableString(value.appId)
    && isNullableString(value.displayName)
    && isNullableString(value.error)
    && isNullableString(value.errorDescription);

export interface WechatOnboardingStartResponse {
  sessionKey: string;
  qrcodeUrl: string;
  qrSvg: string;
  expiresIn: number;
}

export const isWechatOnboardingStartResponse: Validator<WechatOnboardingStartResponse> =
  (value): value is WechatOnboardingStartResponse => isObject(value)
    && isString(value.sessionKey)
    && isString(value.qrcodeUrl)
    && isString(value.qrSvg)
    && isNumber(value.expiresIn);

export interface WechatOnboardingPollResponse {
  done: boolean;
  status?: string | null;
  needVerifyCode?: boolean | null;
  accountId?: string | null;
  alreadyConnected?: boolean | null;
  error?: string | null;
}

export const isWechatOnboardingPollResponse: Validator<WechatOnboardingPollResponse> =
  (value): value is WechatOnboardingPollResponse => isObject(value)
    && isBoolean(value.done)
    && isNullableString(value.status)
    && isNullableBoolean(value.needVerifyCode)
    && isNullableString(value.accountId)
    && isNullableBoolean(value.alreadyConnected)
    && isNullableString(value.error);

export interface WecomOnboardingStartResponse {
  sessionKey: string;
  qrcodeUrl: string;
  qrSvg: string;
  expiresIn: number;
  interval: number;
}

export const isWecomOnboardingStartResponse: Validator<WecomOnboardingStartResponse> =
  (value): value is WecomOnboardingStartResponse => isObject(value)
    && isString(value.sessionKey)
    && isString(value.qrcodeUrl)
    && isString(value.qrSvg)
    && isNumber(value.expiresIn)
    && isNumber(value.interval);

export interface WecomOnboardingPollResponse {
  done: boolean;
  status?: string | null;
  accountId?: string | null;
  error?: string | null;
}

export const isWecomOnboardingPollResponse: Validator<WecomOnboardingPollResponse> =
  (value): value is WecomOnboardingPollResponse => isObject(value)
    && isBoolean(value.done)
    && isNullableString(value.status)
    && isNullableString(value.accountId)
    && isNullableString(value.error);
