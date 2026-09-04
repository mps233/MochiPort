import { invoke } from "@tauri-apps/api/core";
import type {
  Gateway,
  GatewayProvider,
  Lifecycle,
  ServiceProbe,
  TelegramProjectGroup,
} from "./types";
import {
  isActionResponse,
  isCodexEnhancedOperationResponse,
  isCodexEnhancedPreflight,
  isCodexModelCatalogResponse,
  isCodexSessionsResponse,
  isCodexStatus,
  isDashboard,
  isFeishuOnboardingPollResponse,
  isFeishuOnboardingStartResponse,
  isGateway,
  isGatewayMutationResponse,
  isGatewayProviderModelsResponse,
  isGatewayProviderRecentAccountResponse,
  isGatewayProviderUsageResponse,
  isHealthResponse,
  isIMAccountsResponse,
  isLifecycle,
  isLifecycleCredentialMutationResponse,
  isOkResponse,
  isProviderTemplatesResponse,
  isRequestLogDetailResponse,
  isRequestLogsResponse,
  isSettings,
  isSettingsMutationResponse,
  isSub2ApiAdmin,
  isSub2ApiAdminMutationResponse,
  isSub2ApiPoolResponse,
  isTelegramProjectGroupsMutationResponse,
  isTelegramProjectGroupsResponse,
  isWechatOnboardingPollResponse,
  isWechatOnboardingStartResponse,
  isWecomOnboardingPollResponse,
  isWecomOnboardingStartResponse,
  type Validator,
} from "./validators";

interface NativeResponse {
  status: number;
  body: string;
}

export class ManagementError extends Error {
  constructor(
    message: string,
    readonly status?: number,
    readonly connectionFailure = false,
  ) {
    super(message);
    this.name = "ManagementError";
  }
}

function isTauri(): boolean {
  return typeof window !== "undefined" &&
    ("__TAURI_INTERNALS__" in window || "__TAURI__" in window);
}

async function invokeRequest(path: string, method = "GET", body?: unknown): Promise<NativeResponse> {
  const payload = body === undefined ? undefined : JSON.stringify(body);
  if (isTauri()) {
    return invoke<NativeResponse>("management_request", {
      path,
      method,
      body: payload ?? null,
    });
  }

  const baseUrl = import.meta.env.VITE_MANAGEMENT_URL || "http://127.0.0.1:3847";
  const response = await fetch(`${baseUrl.replace(/\/$/, "")}/${path.replace(/^\//, "")}`, {
    method,
    headers: payload ? { "content-type": "application/json" } : undefined,
    body: payload,
  });
  return { status: response.status, body: await response.text() };
}

function invalidResponse(path: string, method: string, status: number): ManagementError {
  const normalizedPath = `/${path.replace(/^\//, "")}`;
  return new ManagementError(
    `本地服务返回的响应格式错误（${method} ${normalizedPath}）`,
    status,
  );
}

async function request<T>(
  path: string,
  validator: Validator<T>,
  method = "GET",
  body?: unknown,
): Promise<T> {
  let response: NativeResponse;
  try {
    response = await invokeRequest(path, method, body);
  } catch (error) {
    const message = error instanceof Error
      ? error.message
      : typeof error === "string" && error.trim()
        ? error
        : "无法连接本地服务";
    throw new ManagementError(
      message,
      undefined,
      true,
    );
  }
  const succeeded = response.status >= 200 && response.status < 300;
  let parsed: unknown;
  try {
    if (typeof response.body !== "string" || response.body.trim().length === 0) {
      throw new SyntaxError("empty response");
    }
    parsed = JSON.parse(response.body);
  } catch {
    if (succeeded) throw invalidResponse(path, method, response.status);
    parsed = undefined;
  }
  if (!succeeded) {
    const message = typeof parsed === "object" && parsed && "error" in parsed
      ? String((parsed as { error?: unknown }).error)
      : `本地服务返回 HTTP ${response.status}`;
    throw new ManagementError(message, response.status);
  }
  if (!validator(parsed)) throw invalidResponse(path, method, response.status);
  return parsed;
}

const lifecycleInstallationStorageKey = "mochiport.lifecycle.installation-id";

function browserLifecycleInstallationId(): string {
  const existing = localStorage.getItem(lifecycleInstallationStorageKey)?.trim();
  if (existing) return existing;
  const generated = typeof crypto.randomUUID === "function"
    ? crypto.randomUUID()
    : `windows-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
  localStorage.setItem(lifecycleInstallationStorageKey, generated);
  return generated;
}

function lifecycleIdentity(lifecycle: Lifecycle) {
  if (!lifecycle.executableSha256) {
    throw new ManagementError("后台服务未提供可执行文件 SHA-256，不能申请管理权");
  }
  return {
    pid: lifecycle.service.pid,
    startedAtMs: lifecycle.service.startedAtMs,
    executable: lifecycle.executable,
    executableSha256: lifecycle.executableSha256,
    bind: lifecycle.bind,
  };
}

async function browserLifecycleLease(
  operation: "claim" | "renew",
  lifecycle: Lifecycle,
): Promise<Lifecycle> {
  if (!import.meta.env.DEV) {
    throw new ManagementError("后台服务生命周期操作必须由 Windows 原生桥执行");
  }
  return request(
    `api/v1/manage/lifecycle/lease/${operation}`,
    isLifecycle,
    "POST",
    {
      installationId: browserLifecycleInstallationId(),
      daemonInstanceId: lifecycle.service.instanceId,
      daemonIdentity: lifecycleIdentity(lifecycle),
    },
  );
}

function lifecycleManagementGenerations(lifecycle: Lifecycle) {
  const leaseGeneration = lifecycle.management.leaseGeneration;
  const managementTokenGeneration = lifecycle.management.managementTokenGeneration;
  if (leaseGeneration == null || managementTokenGeneration == null) {
    throw new ManagementError("后台服务管理状态缺少 generation，请刷新后重试");
  }
  return { leaseGeneration, managementTokenGeneration };
}

function lifecycleRequestId(): string {
  return typeof crypto.randomUUID === "function"
    ? crypto.randomUUID()
    : `windows-lifecycle-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
}

async function browserLifecycleCredentialMutation(
  operation: "takeover" | "rotate",
  lifecycle: Lifecycle,
): Promise<Lifecycle> {
  if (!import.meta.env.DEV) {
    throw new ManagementError("后台服务生命周期操作必须由 Windows 原生桥执行");
  }
  const installationId = browserLifecycleInstallationId();
  const { leaseGeneration, managementTokenGeneration } = lifecycleManagementGenerations(lifecycle);
  const requestId = lifecycleRequestId();
  const path = operation === "takeover"
    ? "api/v1/manage/lifecycle/lease/takeover"
    : "api/v1/manage/lifecycle/credential/rotate";
  const body = operation === "takeover"
    ? {
        installationId,
        daemonInstanceId: lifecycle.service.instanceId,
        expectedLeaseGeneration: leaseGeneration,
        expectedManagementTokenGeneration: managementTokenGeneration,
        requestId,
        force: true,
        daemonIdentity: lifecycleIdentity(lifecycle),
      }
    : {
        installationId,
        daemonInstanceId: lifecycle.service.instanceId,
        leaseGeneration,
        expectedManagementTokenGeneration: managementTokenGeneration,
        requestId,
        reason: "leakRecovery",
      };
  const mutation = await request(
    path,
    isLifecycleCredentialMutationResponse,
    "POST",
    body,
  );
  if (mutation.requestId !== requestId) {
    throw new ManagementError("后台服务返回了不匹配的管理操作确认");
  }
  const refreshed = await request("api/v1/manage/lifecycle", isLifecycle);
  if (refreshed.service.instanceId !== lifecycle.service.instanceId
    || !refreshed.management.canControl
    || refreshed.management.installationId !== installationId
    || refreshed.management.managementTokenGeneration !== mutation.managementTokenGeneration) {
    throw new ManagementError("后台服务管理状态校验失败，请刷新后重试");
  }
  return refreshed;
}

function sameRestartTarget(previous: Lifecycle, candidate: Lifecycle): boolean {
  return candidate.service.ready
    && candidate.service.instanceId !== previous.service.instanceId
    && previous.runtime.buildNumber != null
    && candidate.runtime.buildNumber === previous.runtime.buildNumber
    && candidate.runtime.productVersion === previous.runtime.productVersion
    && candidate.executable.toLocaleLowerCase() === previous.executable.toLocaleLowerCase()
    && candidate.bind === previous.bind
    && Boolean(candidate.executableSha256)
    && candidate.executableSha256?.toLocaleLowerCase() === previous.executableSha256?.toLocaleLowerCase();
}

const lifecycleDelay = (milliseconds: number) =>
  new Promise((resolve) => window.setTimeout(resolve, milliseconds));

async function browserSafeRestartLifecycle(lifecycle: Lifecycle): Promise<Lifecycle> {
  if (!import.meta.env.DEV) {
    throw new ManagementError("后台服务生命周期操作必须由 Windows 原生桥执行");
  }
  if (lifecycle.runtime.buildNumber == null) {
    throw new ManagementError("后台服务未提供构建号，不能验证同构建重启");
  }
  if (!lifecycle.management.leaseGeneration) {
    throw new ManagementError("后台服务管理租约缺少 generation");
  }
  await request("api/v1/manage/lifecycle/restart", isActionResponse, "POST", {
    installationId: browserLifecycleInstallationId(),
    daemonInstanceId: lifecycle.service.instanceId,
    leaseGeneration: lifecycle.management.leaseGeneration,
    force: false,
  });

  let stableIdentity = "";
  let stableCount = 0;
  for (let attempt = 0; attempt < 80; attempt += 1) {
    await lifecycleDelay(100);
    try {
      const candidate = await request("api/v1/manage/lifecycle", isLifecycle);
      if (!sameRestartTarget(lifecycle, candidate)) {
        stableIdentity = "";
        stableCount = 0;
        continue;
      }
      const identity = `${candidate.service.instanceId}:${candidate.service.pid}:${candidate.service.startedAtMs}`;
      if (identity === stableIdentity) stableCount += 1;
      else {
        stableIdentity = identity;
        stableCount = 1;
      }
      if (stableCount >= 2) return browserLifecycleLease("claim", candidate);
    } catch {
      stableIdentity = "";
      stableCount = 0;
    }
  }
  throw new ManagementError("后台服务未能以相同路径和构建在预期时间内恢复");
}

async function invokeValidated<T>(
  command: string,
  args: Record<string, unknown>,
  validator: Validator<T>,
): Promise<T> {
  try {
    const value: unknown = await invoke(command, args);
    if (!validator(value)) throw new ManagementError("Windows 原生服务返回的响应格式错误");
    return value;
  } catch (error) {
    if (error instanceof ManagementError) throw error;
    throw new ManagementError(error instanceof Error ? error.message : String(error));
  }
}

export const api = {
  health: () => request("healthz", isHealthResponse),
  probe: async (): Promise<ServiceProbe> => {
    const health = await request("healthz", isHealthResponse);
    if (health.service !== "mochiport") {
      throw new ManagementError("MochiPort 端口正被其他服务占用");
    }
    return health.apiMajor === 0
      ? { kind: "legacy" }
      : { kind: "versioned", health };
  },
  dashboard: () => request("api/v1/manage/dashboard", isDashboard),
  lifecycle: () => request("api/v1/manage/lifecycle", isLifecycle),
  codexStatus: () => request("api/v1/manage/codex/status", isCodexStatus),
  codexModelCatalog: async () => {
    const response = await request("api/v1/manage/codex/models/catalog", isCodexModelCatalogResponse);
    return response.models;
  },
  codexSessions: async () => {
    const response = await request("api/v1/manage/sessions", isCodexSessionsResponse);
    return response;
  },
  gateway: () => request("api/v1/manage/gateway", isGateway),
  settings: () => request("api/v1/manage/settings", isSettings),
  imAccounts: async () => {
    const response = await request("api/v1/manage/im/accounts", isIMAccountsResponse);
    return response.accounts;
  },
  telegramProjectGroups: () => request("api/v1/manage/im/account/telegram/project-groups", isTelegramProjectGroupsResponse),
  updateTelegramProjectGroups: (accountId: string, projectGroups: TelegramProjectGroup[]) =>
    request(
      "api/v1/manage/im/account/telegram/project-groups",
      isTelegramProjectGroupsMutationResponse,
      "POST",
      { accountId, projectGroups },
    ),
  sub2ApiAdmin: () => request("api/v1/manage/gateway/sub2api", isSub2ApiAdmin),
  updateSub2ApiAdmin: (baseUrl: string, adminApiKey?: string | null, clearAdminApiKey = false) =>
    request("api/v1/manage/gateway/sub2api/config", isSub2ApiAdminMutationResponse, "POST", { baseUrl, adminApiKey, clearAdminApiKey }),
  disconnectSub2ApiAdmin: () =>
    request("api/v1/manage/gateway/sub2api/disconnect", isSub2ApiAdminMutationResponse, "POST", {}),
  sub2ApiAccounts: async (forceBillingRefresh = false) => {
    const response = await request(
      "api/v1/manage/gateway/sub2api/accounts",
      isSub2ApiPoolResponse,
      "POST",
      { forceBillingRefresh },
    );
    return response.pool;
  },
  requestLogs: (query = "") => request(`api/v1/manage/request-logs${query ? `?${query}` : ""}`, isRequestLogsResponse),
  requestLogDetail: (id: number) => request(`api/v1/manage/request-logs/${id}`, isRequestLogDetailResponse),
  clearRequestLogs: () => request("api/v1/manage/request-logs/clear", isActionResponse, "POST", {}),
  clearOldRequestLogs: (days = 3) => request("api/v1/manage/request-logs/clear-old", isActionResponse, "POST", { days }),
  updateGateway: (payload: Pick<Gateway, "enabled" | "filterImageGenerationTool" | "requestLoggingEnabled" | "requestLogDetailsEnabled" | "codexVisibleModels">) =>
    request("api/v1/manage/gateway/settings", isGatewayMutationResponse, "POST", payload),
  upsertProvider: (payload: Record<string, unknown>) => request("api/v1/manage/gateway/provider", isGatewayMutationResponse, "POST", payload),
  deleteProvider: (name: string) => request("api/v1/manage/gateway/provider/delete", isGatewayMutationResponse, "POST", { name }),
  providerTemplates: async () => {
    const response = await request("api/v1/manage/gateway/provider-templates", isProviderTemplatesResponse);
    return response.templates;
  },
  fetchProviderModels: (payload: {
    providerName?: string | null;
    baseUrl: string;
    modelsUrl?: string | null;
    providerType: string;
    apiKey?: string | null;
  }) => request("api/v1/manage/gateway/provider/models/fetch", isGatewayProviderModelsResponse, "POST", payload),
  providerUsage: (providerName: string) =>
    request("api/v1/manage/gateway/provider/usage", isGatewayProviderUsageResponse, "POST", { providerName }),
  providerRecentAccount: (providerName: string) =>
    request(
      "api/v1/manage/gateway/provider/recent-account",
      isGatewayProviderRecentAccountResponse,
      "POST",
      { providerName },
    ),
  setIMAccountEnabled: (platform: string, accountId: string, enabled: boolean) =>
    request("api/v1/manage/im/account/enabled", isOkResponse, "POST", { platform, accountId, enabled }),
  deleteIMAccount: (platform: string, accountId: string) =>
    request("api/v1/manage/im/account/delete", isOkResponse, "POST", { platform, accountId }),
  configureTelegram: (botToken: string, mentionOnly: boolean) =>
    request("api/v1/manage/im/account/telegram", isOkResponse, "POST", { botToken, mentionOnly }),
  configureFeishu: (appId: string, appSecret: string) =>
    request("api/v1/manage/im/account/feishu", isOkResponse, "POST", { appId, appSecret }),
  startFeishuOnboarding: () => request("api/v1/manage/im/onboarding/feishu/start", isFeishuOnboardingStartResponse, "POST", {}),
  pollFeishuOnboarding: (deviceCode: string) => request("api/v1/manage/im/onboarding/feishu/poll", isFeishuOnboardingPollResponse, "POST", { deviceCode }),
  startWechatOnboarding: () => request("api/v1/manage/im/onboarding/wechat/start", isWechatOnboardingStartResponse, "POST", {}),
  pollWechatOnboarding: (sessionKey: string, verifyCode?: string) => request("api/v1/manage/im/onboarding/wechat/poll", isWechatOnboardingPollResponse, "POST", { sessionKey, verifyCode }),
  startWecomOnboarding: () => request("api/v1/manage/im/onboarding/wecom/start", isWecomOnboardingStartResponse, "POST", {}),
  pollWecomOnboarding: (sessionKey: string) => request("api/v1/manage/im/onboarding/wecom/poll", isWecomOnboardingPollResponse, "POST", { sessionKey }),
  configureSettings: (payload: {
    language: string | null;
    theme: string | null;
    localConnectionMode: string;
    outboundProxyMode: string;
    outboundProxyUrl?: string | null;
  }) => request("api/v1/manage/settings", isSettingsMutationResponse, "POST", payload),
  codexAction: (action: "configure" | "repair" | "uninstall" | "models/refresh" | "direct-api-mode") =>
    request(`api/v1/manage/codex/${action}`, isActionResponse, "POST", {}),
  codexEnhancedPreflight: () =>
    request("api/v1/manage/codex/enhanced/preflight", isCodexEnhancedPreflight),
  startCodexEnhancedOperation: (requestId: string) =>
    request("api/v1/manage/codex/enhanced/operation/start", isCodexEnhancedOperationResponse, "POST", { requestId }),
  codexEnhancedOperation: () =>
    request("api/v1/manage/codex/enhanced/operation", isCodexEnhancedOperationResponse),
  cancelCodexEnhancedOperation: (requestId: string) =>
    request("api/v1/manage/codex/enhanced/operation/cancel", isCodexEnhancedOperationResponse, "POST", { requestId }),
  launchCodexEnhancedLegacy: () =>
    request("api/v1/manage/codex/enhanced/launch", isActionResponse, "POST", {}),
  lifecycleInstallationId: async () => {
    if (!isTauri()) return browserLifecycleInstallationId();
    const value = await invoke<unknown>("lifecycle_installation_id");
    if (typeof value !== "string" || !value.trim()) {
      throw new ManagementError("Windows 安装身份格式无效");
    }
    return value;
  },
  lifecycleLease: (operation: "claim" | "renew", lifecycle: Lifecycle) => {
    if (!isTauri()) return browserLifecycleLease(operation, lifecycle);
    return invokeValidated("lifecycle_lease", { operation, lifecycle }, isLifecycle);
  },
  takeOverLifecycleLease: (lifecycle: Lifecycle) => {
    if (!isTauri()) return browserLifecycleCredentialMutation("takeover", lifecycle);
    return invokeValidated("lifecycle_takeover", { lifecycle }, isLifecycle);
  },
  rotateManagementCredential: (lifecycle: Lifecycle) => {
    if (!isTauri()) return browserLifecycleCredentialMutation("rotate", lifecycle);
    return invokeValidated("lifecycle_rotate_credential", { lifecycle }, isLifecycle);
  },
  safeRestartLifecycle: (lifecycle: Lifecycle) => {
    if (!isTauri()) return browserSafeRestartLifecycle(lifecycle);
    return invokeValidated("lifecycle_safe_restart", { lifecycle }, isLifecycle);
  },
  startDaemon: () => invoke<{ started: boolean; executable?: string; message: string }>("start_daemon"),
  setCloseBehavior: async (behavior: "tray" | "quit") => {
    if (!isTauri()) return;
    await invoke("set_close_behavior", { behavior });
  },
};
