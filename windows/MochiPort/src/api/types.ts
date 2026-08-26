export type ServiceStatus = "checking" | "available" | "bridgeAvailable" | "unavailable";

export interface HealthResponse {
  service: string;
  apiMajor: number;
  ready: boolean;
}

export type ServiceProbe =
  | { kind: "versioned"; health: HealthResponse }
  | { kind: "legacy" };

export type AppSection =
  | "overview"
  | "codex"
  | "gateway"
  | "messaging"
  | "sessions"
  | "requestLogs"
  | "settings";

export interface ServiceIdentity {
  service: string;
  apiMajor: number;
  ready: boolean;
  instanceId: string;
  pid: number;
  startedAtMs: number;
}

export interface EndpointStatus {
  configured: boolean;
  connected: boolean;
}

export interface MessageChannel {
  accountCount: number;
  connectedAccountCount: number;
}

export interface Dashboard {
  service: ServiceIdentity;
  bridgeRunning: boolean;
  remoteControlConnected: boolean;
  remoteControlHealthy: boolean;
  executionClients: {
    codexApp: EndpointStatus;
    vscode: EndpointStatus;
    cli: EndpointStatus;
  };
  messageChannels: {
    telegram: MessageChannel;
    feishu: MessageChannel;
    wechat: MessageChannel;
    wecom: MessageChannel;
    /** Counts reported by v1 daemons before per-channel status was added. */
    legacyUnattributed?: MessageChannel;
  };
  aiGatewayEnabled: boolean;
  aiGatewayProviderCount: number;
  requestLoggingEnabled: boolean;
}

export interface Lifecycle {
  service: ServiceIdentity;
  executable: string;
  executableSha256?: string | null;
  configPath: string;
  bind: string;
  runtime: {
    state: string;
    productVersion: string;
    buildNumber?: number | null;
    apiMajor: number;
  };
  protectedWorkItems: {
    aiGatewayRequests: number;
    codexTurns: number;
    imStreams: number;
    pendingApprovals: number;
    remoteControlRequests: number;
    total: number;
  };
  management: {
    state: string;
    mode: string;
    canControl: boolean;
    installationId?: string | null;
    leaseGeneration?: number | null;
    leaseExpiresAtMs?: number | null;
    managementTokenGeneration?: number | null;
  };
}

export interface LifecycleCredentialMutationResponse {
  ok: boolean;
  rotated: boolean;
  requestId: string;
  managementTokenGeneration: number;
}

export interface IMAccount {
  platform: string;
  accountId: string;
  displayName?: string | null;
  avatarData?: string | null;
  enabled: boolean;
  configured: boolean;
  secretSet: boolean;
  connecting: boolean;
  polling: boolean;
  connected: boolean;
  lastError?: string | null;
  lastEventAtMs?: number | null;
  lastInboundAtMs?: number | null;
}

export interface TelegramProjectGroup {
  chatId: string;
  projectName: string;
  cwd: string;
}

export interface TelegramProjectGroupAccount {
  accountId: string;
  projectGroups: TelegramProjectGroup[];
}

export interface TelegramProjectGroupsResponse {
  accounts: TelegramProjectGroupAccount[];
}

export interface TelegramProjectGroupsMutationResponse {
  ok: boolean;
  accountId: string;
  projectGroups: TelegramProjectGroup[];
  restartRequired: boolean;
}

export interface CodexProvider {
  name: string;
  baseUrl?: string | null;
  secretSet: boolean;
  supportsWebsockets: boolean;
}

export interface CodexStatus {
  codexHome: string;
  configured: boolean;
  configOk: boolean;
  authOk: boolean;
  providerOk: boolean;
  configError?: string | null;
  authError?: string | null;
  guiConfigured: boolean;
  guiError?: string | null;
  remoteControlSupported: boolean;
  remoteControlConfigured: boolean;
  remoteControlError?: string | null;
  providers: CodexProvider[];
  imageGenerationEnabled: boolean;
  connectionMode: string;
  providerMode?: "threadrelay" | "direct-api" | "unknown" | null;
  providerModeMessage?: string | null;
  activeProvider?: string | null;
}

export interface CodexEnhancedPreflight {
  ok: boolean;
  status: {
    running: boolean;
  };
}

export type CodexEnhancedOperationPhase =
  | "preparing"
  | "launching"
  | "waitingForApp"
  | "injecting"
  | "ready"
  | "failed"
  | "cancelled";

export interface CodexEnhancedOperation {
  requestId: string;
  phase: CodexEnhancedOperationPhase;
  startedAtMs: number;
  updatedAtMs: number;
  canCancel: boolean;
  message: string;
  error?: string | null;
  recovery?: string | null;
  report?: unknown;
}

export interface CodexEnhancedOperationResponse {
  ok: boolean;
  operation?: CodexEnhancedOperation | null;
}

export interface CodexSession {
  id: string;
  preview: string;
  modelProvider: string;
  updatedAt: number;
  path?: string | null;
  name?: string | null;
  cwd?: string | null;
}

export interface GatewayProvider {
  name: string;
  enabled: boolean;
  providerType: string;
  compatibility?: string | null;
  baseUrl: string;
  modelsUrl?: string | null;
  models: string[];
  modelAliases: Record<string, string>;
  promptCacheRetention?: string | null;
  weight: number;
  timeoutSecs: number;
  secretSet: boolean;
}

export interface GatewayProviderTemplate {
  id: string;
  displayName: string;
  providerType: string;
  compatibility?: string | null;
  baseUrl: string;
  modelsUrl?: string | null;
  models: string[];
}

export interface CodexCatalogModel {
  id: string;
  displayName: string;
}

export interface GatewayProviderModelAttempt {
  url: string;
  status?: number | null;
  error?: string | null;
  preview?: string | null;
}

export interface GatewayProviderModelsResponse {
  ok: boolean;
  models: string[];
  attempts: GatewayProviderModelAttempt[];
}

export interface GatewayProviderUsage {
  source: string;
  balanceStatus: string;
  billingStatus: string;
  remaining?: number | null;
  unlimited: boolean;
  unit?: string | null;
  balanceMode?: string | null;
  planName?: string | null;
  accountValid?: boolean | null;
  accountStatus?: string | null;
  todayCost?: number | null;
  todayActualCost?: number | null;
  groupRateMultiplier?: number | null;
  userRateMultiplier?: number | null;
  resolvedRateMultiplier?: number | null;
  effectiveRateMultiplier?: number | null;
  peakRateEnabled?: boolean | null;
  peakStart?: string | null;
  peakEnd?: string | null;
  peakRateMultiplier?: number | null;
  appliedPeakMultiplier?: number | null;
  timezone?: string | null;
  observedAt?: string | null;
}

export interface GatewayProviderUsageResponse {
  ok: boolean;
  providerName: string;
  usage: GatewayProviderUsage;
}

export interface Gateway {
  enabled: boolean;
  filterImageGenerationTool: boolean;
  requestLoggingEnabled: boolean;
  requestLogDetailsEnabled: boolean;
  codexVisibleModels: string[];
  providers: GatewayProvider[];
}

export interface RequestLog {
  id: number;
  requestId: string;
  modelId: string;
  stream: boolean;
  channel: string;
  providerType: string;
  status: string;
  inputTokens?: number | null;
  outputTokens?: number | null;
  totalTokens?: number | null;
  readCacheTokens?: number | null;
  readCacheHitRate?: number | null;
  writeCacheTokens?: number | null;
  writeCache5mTokens?: number | null;
  writeCache1hTokens?: number | null;
  costUsd?: number | null;
  latencyMs?: number | null;
  ttftMs?: number | null;
  createdAtMs: number;
  createdAt: string;
  errorMessage?: string | null;
  upstreamRequestBodyBytes?: number | null;
}

export interface RequestLogDetail extends RequestLog {
  requestHeadersJson?: string | null;
  requestJson?: string | null;
  upstreamRequestHeadersJson?: string | null;
  upstreamRequestJson?: string | null;
  upstreamResponseSse?: string | null;
  responseJson?: string | null;
}

export interface Settings {
  language?: string | null;
  theme?: string | null;
  localConnectionMode: string;
  bind: string;
  outboundProxy: {
    mode: string;
    url: string;
    credentialSet: boolean;
  };
}

export interface Sub2ApiAdmin {
  configured: boolean;
  baseUrl: string;
  secretSet: boolean;
}

export interface Sub2ApiAccount {
  id: number;
  name: string;
  siteUrl?: string | null;
  platform: string;
  accountType: string;
  status: string;
  schedulable: boolean;
  localRateMultiplier?: number | null;
  upstreamBilling: {
    state: string;
    resolvedRateMultiplier?: number | null;
    effectiveRateMultiplier?: number | null;
    observedAt?: string | null;
    freshUntil?: string | null;
    stale: boolean;
  };
  upstreamBalance: {
    state: string;
    remaining?: number | null;
    unlimited: boolean;
    unit?: string | null;
    mode?: string | null;
    planName?: string | null;
    accountValid?: boolean | null;
    accountStatus?: string | null;
    observedAt?: string | null;
  };
}

export interface Sub2ApiPool {
  source: string;
  fetchedAtMs: number;
  accounts: Sub2ApiAccount[];
  warnings?: string[] | null;
}

export interface GatewayProviderRecentAccount {
  accountId: number;
  accountName: string;
  createdAt: string;
}

export interface GatewayProviderRecentAccountResponse {
  ok: boolean;
  providerName: string;
  account?: GatewayProviderRecentAccount | null;
}

export interface RequestLogsResponse {
  logs: RequestLog[];
  nextCursor?: string | null;
  hasMore?: boolean;
}
