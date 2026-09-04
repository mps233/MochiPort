import {
  createContext,
  type PropsWithChildren,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { api, ManagementError } from "../api/client";
import {
  fixtureAccounts,
  fixtureCodexStatus,
  fixtureDashboard,
  fixtureGateway,
  fixtureLifecycle,
  fixtureLogs,
  fixtureSessions,
  fixtureSettings,
  fixtureSub2ApiAdmin,
  fixtureSub2ApiPool,
} from "../api/fixtures";
import type {
  AppSection,
  CodexSession,
  CodexEnhancedOperation,
  CodexStatus,
  Dashboard,
  Gateway,
  GatewayProvider,
  IMAccount,
  Lifecycle,
  RequestLog,
  RequestLogDetail,
  RequestLogsResponse,
  ServiceStatus,
  Settings,
  Sub2ApiAdmin,
  Sub2ApiPool,
  TelegramProjectGroup,
  TelegramProjectGroupAccount,
} from "../api/types";
import { useCodexEnhancedLaunch } from "./useCodexEnhancedLaunch";

type SectionMap<T> = Partial<Record<AppSection, T>>;

interface GatewaySettingsDraft {
  enabled: boolean;
  filterImageGenerationTool: boolean;
  requestLoggingEnabled: boolean;
  requestLogDetailsEnabled: boolean;
  codexVisibleModels: string[];
}

type CodexAction = "configure" | "repair" | "uninstall" | "models/refresh" | "direct-api-mode";

const codexActionFeedback: Record<CodexAction, string> = {
  configure: "已连接 MochiPort",
  repair: "已修复 GUI 环境",
  uninstall: "已恢复原来的 Codex 设置，MochiPort 已关闭",
  "models/refresh": "已刷新模型列表",
  "direct-api-mode": "已切换到原来的连接",
};

function gatewaySettingsWithEnabled(gateway: Gateway, enabled: boolean): GatewaySettingsDraft {
  return {
    enabled,
    filterImageGenerationTool: gateway.filterImageGenerationTool,
    requestLoggingEnabled: gateway.requestLoggingEnabled,
    requestLogDetailsEnabled: gateway.requestLogDetailsEnabled,
    codexVisibleModels: gateway.codexVisibleModels,
  };
}

export interface ProviderDraft extends GatewayProvider {
  originalName?: string | null;
  apiKey?: string | null;
  clearApiKey?: boolean;
}

export interface RequestLogFilters {
  query: string;
  status: string | null;
  channel: string | null;
  modelId: string | null;
  sort: "newest" | "oldest";
}

const defaultRequestLogFilters: RequestLogFilters = {
  query: "",
  status: null,
  channel: null,
  modelId: null,
  sort: "newest",
};

function requestLogFilterKey(filters: RequestLogFilters): string {
  return JSON.stringify([
    filters.query.trim(),
    filters.status?.trim() || null,
    filters.channel?.trim() || null,
    filters.modelId?.trim() || null,
    filters.sort,
  ]);
}

function mergeRequestLogs(leading: RequestLog[], trailing: RequestLog[]): RequestLog[] {
  const seen = new Set<number>();
  return [...leading, ...trailing].filter((entry) => {
    if (seen.has(entry.id)) return false;
    seen.add(entry.id);
    return true;
  });
}

function requestLogPageHasMore(response: RequestLogsResponse, previousCursor?: string): boolean {
  const nextCursor = response.nextCursor?.trim();
  return Boolean(nextCursor && nextCursor !== previousCursor && (response.hasMore ?? true));
}

function accountRefreshMessage(error: unknown): string {
  const detail = error instanceof Error ? error.message : String(error);
  return `消息账号暂时无法刷新；继续显示上次成功读取的配置。${detail ? ` ${detail}` : ""}`;
}

interface SettingsDraft {
  language: string | null;
  theme: string | null;
  localConnectionMode: string;
  outboundProxyMode: string;
  outboundProxyUrl?: string | null;
}

interface AppModelValue {
  fixtureMode: boolean;
  selection: AppSection;
  setSelection: (section: AppSection) => void;
  status: ServiceStatus;
  statusMessage: string;
  lastCheckedAt?: number;
  dashboard?: Dashboard;
  lifecycle?: Lifecycle;
  ownsDaemonLease: boolean;
  daemonLeaseConflict: boolean;
  daemonTransitionInProgress: boolean;
  daemonLeaseTakeoverInProgress: boolean;
  managementCredentialRotationInProgress: boolean;
  lifecycleOperationError?: string;
  codexStatus?: CodexStatus;
  codexEnhancedOperation?: CodexEnhancedOperation;
  codexEnhancedWaitingForAppExit: boolean;
  codexEnhancedUsesLegacyFallback: boolean;
  codexEnhancedLaunchError?: string;
  codexEnhancedLaunchInProgress: boolean;
  canCancelCodexEnhancedLaunch: boolean;
  sessions: CodexSession[];
  sessionProviders: string[];
  gateway?: Gateway;
  accounts: IMAccount[];
  accountsRefreshError?: string;
  telegramProjectGroupAccounts: TelegramProjectGroupAccount[];
  saveTelegramProjectGroups: (accountId: string, groups: TelegramProjectGroup[]) => Promise<boolean>;
  requestLogs: RequestLog[];
  requestLogsHasMore: boolean;
  settings?: Settings;
  sub2ApiAdmin?: Sub2ApiAdmin;
  sub2ApiPool?: Sub2ApiPool;
  sub2ApiPoolLoading: boolean;
  sub2ApiPoolError?: string;
  loading: SectionMap<boolean>;
  errors: SectionMap<string>;
  busy: Record<string, boolean>;
  feedback?: string;
  refresh: () => Promise<void>;
  startDaemon: () => Promise<boolean>;
  loadSection: (section: AppSection, force?: boolean) => Promise<void>;
  dismissError: (section: AppSection) => void;
  dismissAccountsRefreshError: () => void;
  clearFeedback: () => void;
  clearLifecycleOperationError: () => void;
  saveGatewaySettings: (draft: GatewaySettingsDraft) => Promise<boolean>;
  saveProvider: (draft: ProviderDraft) => Promise<boolean>;
  deleteProvider: (name: string) => Promise<boolean>;
  saveSub2Api: (baseUrl: string, adminApiKey: string) => Promise<boolean>;
  disconnectSub2Api: () => Promise<boolean>;
  refreshSub2ApiPool: (forceBillingRefresh?: boolean) => Promise<void>;
  toggleAccount: (account: IMAccount, enabled: boolean) => Promise<boolean>;
  deleteAccount: (account: IMAccount) => Promise<boolean>;
  addTelegram: (token: string, mentionOnly: boolean) => Promise<boolean>;
  addFeishu: (appId: string, appSecret: string) => Promise<boolean>;
  runCodexAction: (action: CodexAction) => Promise<boolean>;
  beginCodexEnhancedLaunch: () => Promise<boolean>;
  cancelCodexEnhancedLaunch: () => Promise<void>;
  saveSettings: (draft: SettingsDraft) => Promise<boolean>;
  takeOverDaemonManagement: () => Promise<boolean>;
  rotateManagementCredential: () => Promise<boolean>;
  restartDaemon: () => Promise<boolean>;
  loadRequestLogDetail: (id: number) => Promise<RequestLogDetail | undefined>;
  queryRequestLogs: (filters: RequestLogFilters, append?: boolean) => Promise<void>;
  clearRequestLogs: (olderThanDays?: number) => Promise<boolean>;
  completeFixtureOnboarding: (platform: string) => void;
}

const AppModelContext = createContext<AppModelValue | null>(null);

const delay = (milliseconds: number) => new Promise((resolve) => window.setTimeout(resolve, milliseconds));
const SUB2_API_POOL_CACHE_MS = 5 * 60 * 1000;

function lifecycleLeaseOwnedBy(
  lifecycle: Lifecycle | undefined,
  installationId: string | undefined,
  now = Date.now(),
): boolean {
  return Boolean(
    lifecycle
      && installationId
      && lifecycle.management.canControl
      && lifecycle.management.installationId === installationId
      && lifecycle.management.leaseGeneration != null
      && lifecycle.management.leaseExpiresAtMs != null
      && lifecycle.management.leaseExpiresAtMs > now,
  );
}

function lifecycleHasConflictingLease(
  lifecycle: Lifecycle,
  installationId: string,
  now = Date.now(),
): boolean {
  const owner = lifecycle.management.installationId;
  if (!owner || owner === installationId) return false;
  const expiry = lifecycle.management.leaseExpiresAtMs;
  // Missing expiry is treated conservatively: this client never takes over an
  // ownership record it cannot prove has expired.
  return expiry == null || expiry > now;
}

function defaultStatusMessage(status: ServiceStatus): string {
  switch (status) {
    case "checking": return "正在连接本地服务";
    case "available": return "本地服务已就绪";
    case "bridgeAvailable": return "请更新后台服务以启用全部管理功能";
    case "unavailable": return "本地服务不可用";
  }
}

export function AppModelProvider({ children }: PropsWithChildren) {
  const fixtureMode = useMemo(() => {
    const params = new URLSearchParams(window.location.search);
    return params.has("fixture") || import.meta.env.VITE_FIXTURE_MODE === "true";
  }, []);
  const [selectionState, setSelectionState] = useState<AppSection>(() => {
    const stored = localStorage.getItem("mochiport.section") as AppSection | null;
    return stored ?? "overview";
  });
  const [status, setStatus] = useState<ServiceStatus>(fixtureMode ? "available" : "checking");
  const [statusMessage, setStatusMessage] = useState(defaultStatusMessage(fixtureMode ? "available" : "checking"));
  const [lastCheckedAt, setLastCheckedAt] = useState<number>();
  const [dashboard, setDashboard] = useState<Dashboard | undefined>(fixtureMode ? fixtureDashboard : undefined);
  const [lifecycle, setLifecycle] = useState<Lifecycle | undefined>(fixtureMode ? fixtureLifecycle : undefined);
  const [lifecycleInstallationId, setLifecycleInstallationId] = useState<string | undefined>(
    fixtureMode ? fixtureLifecycle.management.installationId ?? undefined : undefined,
  );
  const [daemonTransitionInProgress, setDaemonTransitionInProgress] = useState(false);
  const [daemonLeaseTakeoverInProgress, setDaemonLeaseTakeoverInProgress] = useState(false);
  const [managementCredentialRotationInProgress, setManagementCredentialRotationInProgress] = useState(false);
  const [lifecycleOperationError, setLifecycleOperationError] = useState<string>();
  const [codexStatus, setCodexStatus] = useState<CodexStatus | undefined>(fixtureMode ? fixtureCodexStatus : undefined);
  const [sessions, setSessions] = useState<CodexSession[]>(fixtureMode ? fixtureSessions : []);
  const [sessionProviders, setSessionProviders] = useState<string[]>(fixtureMode ? ["MochiPort", "openai"] : []);
  const [gateway, setGateway] = useState<Gateway | undefined>(fixtureMode ? fixtureGateway : undefined);
  const [accounts, setAccounts] = useState<IMAccount[]>(fixtureMode ? fixtureAccounts : []);
  const [telegramProjectGroupAccounts, setTelegramProjectGroupAccounts] = useState<TelegramProjectGroupAccount[]>([]);
  const [accountsRefreshError, setAccountsRefreshError] = useState<string>();
  const [requestLogs, setRequestLogs] = useState<RequestLog[]>(fixtureMode ? fixtureLogs : []);
  const [requestLogsHasMore, setRequestLogsHasMore] = useState(false);
  const [settings, setSettings] = useState<Settings | undefined>(fixtureMode ? fixtureSettings : undefined);
  const [sub2ApiAdmin, setSub2ApiAdmin] = useState<Sub2ApiAdmin | undefined>(fixtureMode ? fixtureSub2ApiAdmin : undefined);
  const [sub2ApiPool, setSub2ApiPool] = useState<Sub2ApiPool | undefined>(fixtureMode ? fixtureSub2ApiPool : undefined);
  const [sub2ApiPoolLoading, setSub2ApiPoolLoading] = useState(false);
  const [sub2ApiPoolError, setSub2ApiPoolError] = useState<string>();
  const [loading, setLoading] = useState<SectionMap<boolean>>({});
  const [errors, setErrors] = useState<SectionMap<string>>({});
  const [busy, setBusy] = useState<Record<string, boolean>>({});
  const [feedback, setFeedback] = useState<string>();
  const daemonStartAttempted = useRef(false);
  const daemonTransitionRef = useRef(false);
  const loadedSections = useRef(new Set<AppSection>());
  const sectionGenerations = useRef<Partial<Record<AppSection, number>>>({});
  const refreshInFlight = useRef<Promise<void> | null>(null);
  const lifecycleInstallationIdInFlight = useRef<Promise<string> | null>(null);
  const accountMutationGenerations = useRef(new Map<string, number>());
  const requestLogGeneration = useRef(0);
  const requestLogCursor = useRef<string | undefined>(undefined);
  const requestLogHasMore = useRef(false);
  const requestLogLoadedFilterKey = useRef<string | undefined>(fixtureMode ? requestLogFilterKey(defaultRequestLogFilters) : undefined);
  const requestLogLoadedPageCount = useRef(fixtureMode ? 1 : 0);
  const sub2ApiAdminRef = useRef<Sub2ApiAdmin | undefined>(fixtureMode ? fixtureSub2ApiAdmin : undefined);
  const sub2ApiPoolRef = useRef<Sub2ApiPool | undefined>(fixtureMode ? fixtureSub2ApiPool : undefined);
  const sub2ApiPoolGeneration = useRef(0);
  const sub2ApiPoolInFlight = useRef<{ generation: number; promise: Promise<void> } | null>(null);
  const sub2ApiMutationGeneration = useRef(0);

  const setSelection = useCallback((section: AppSection) => {
    setSelectionState(section);
    localStorage.setItem("mochiport.section", section);
  }, []);

  const withBusy = useCallback(async <T,>(key: string, task: () => Promise<T>): Promise<T> => {
    setBusy((current) => ({ ...current, [key]: true }));
    try {
      return await task();
    } finally {
      setBusy((current) => ({ ...current, [key]: false }));
    }
  }, []);

  const recordError = useCallback((section: AppSection, error: unknown) => {
    const message = error instanceof Error ? error.message : String(error);
    setErrors((current) => ({ ...current, [section]: message }));
    return message;
  }, []);

  const publishSub2ApiAdmin = useCallback((next: Sub2ApiAdmin | undefined) => {
    sub2ApiAdminRef.current = next;
    setSub2ApiAdmin(next);
  }, []);

  const publishSub2ApiPool = useCallback((next: Sub2ApiPool | undefined) => {
    sub2ApiPoolRef.current = next;
    setSub2ApiPool(next);
  }, []);

  const invalidateSub2ApiPool = useCallback(() => {
    sub2ApiPoolGeneration.current += 1;
    sub2ApiPoolInFlight.current = null;
    publishSub2ApiPool(undefined);
    setSub2ApiPoolLoading(false);
    setSub2ApiPoolError(undefined);
  }, [publishSub2ApiPool]);

  const refreshSub2ApiPoolWithAdmin = useCallback((
    forceBillingRefresh = false,
    knownAdmin?: Sub2ApiAdmin,
  ): Promise<void> => {
    if (knownAdmin) {
      const previousAdmin = sub2ApiAdminRef.current;
      const connectionChanged = previousAdmin?.configured !== knownAdmin.configured
        || previousAdmin?.baseUrl !== knownAdmin.baseUrl
        || previousAdmin?.secretSet !== knownAdmin.secretSet;
      publishSub2ApiAdmin(knownAdmin);
      if (!knownAdmin.configured) {
        if (connectionChanged || sub2ApiPoolRef.current) invalidateSub2ApiPool();
        return Promise.resolve();
      }
      if (connectionChanged) invalidateSub2ApiPool();
    }

    const cached = sub2ApiPoolRef.current;
    if (!forceBillingRefresh
      && cached
      && Date.now() - cached.fetchedAtMs < SUB2_API_POOL_CACHE_MS) {
      return Promise.resolve();
    }
    if (!forceBillingRefresh && sub2ApiPoolInFlight.current) {
      return sub2ApiPoolInFlight.current.promise;
    }

    const generation = sub2ApiPoolGeneration.current + 1;
    sub2ApiPoolGeneration.current = generation;
    const isCurrent = () => sub2ApiPoolGeneration.current === generation;
    setSub2ApiPoolLoading(true);
    setSub2ApiPoolError(undefined);

    const task = Promise.resolve().then(async () => {
      try {
        if (fixtureMode) {
          await delay(120);
          if (isCurrent()) {
            publishSub2ApiPool({ ...fixtureSub2ApiPool, fetchedAtMs: Date.now() });
          }
          return;
        }

        const admin = knownAdmin ?? sub2ApiAdminRef.current ?? await api.sub2ApiAdmin();
        if (!isCurrent()) return;
        publishSub2ApiAdmin(admin);
        if (!admin.configured) {
          publishSub2ApiPool(undefined);
          return;
        }

        const pool = await api.sub2ApiAccounts(forceBillingRefresh);
        if (isCurrent()) publishSub2ApiPool(pool);
      } catch (error) {
        if (isCurrent()) {
          setSub2ApiPoolError(error instanceof Error ? error.message : String(error));
        }
      } finally {
        if (isCurrent()) setSub2ApiPoolLoading(false);
        if (sub2ApiPoolInFlight.current?.generation === generation) {
          sub2ApiPoolInFlight.current = null;
        }
      }
    });
    sub2ApiPoolInFlight.current = { generation, promise: task };
    return task;
  }, [fixtureMode, invalidateSub2ApiPool, publishSub2ApiAdmin, publishSub2ApiPool]);

  const refreshSub2ApiPool = useCallback(
    (forceBillingRefresh = false) => refreshSub2ApiPoolWithAdmin(forceBillingRefresh),
    [refreshSub2ApiPoolWithAdmin],
  );

  const loadLifecycleInstallationId = useCallback(async () => {
    if (lifecycleInstallationId) return lifecycleInstallationId;
    if (!lifecycleInstallationIdInFlight.current) {
      const task = api.lifecycleInstallationId();
      lifecycleInstallationIdInFlight.current = task;
      void task.finally(() => {
        if (lifecycleInstallationIdInFlight.current === task) {
          lifecycleInstallationIdInFlight.current = null;
        }
      });
    }
    const installationId = await lifecycleInstallationIdInFlight.current;
    setLifecycleInstallationId(installationId);
    return installationId;
  }, [lifecycleInstallationId]);

  const reconcileLifecycleLease = useCallback(async (observed: Lifecycle) => {
    if (fixtureMode || daemonTransitionInProgress) return observed;
    try {
      const installationId = await loadLifecycleInstallationId();
      if (lifecycleHasConflictingLease(observed, installationId)) return observed;
      const operation = lifecycleLeaseOwnedBy(observed, installationId) ? "renew" : "claim";
      return await api.lifecycleLease(operation, observed);
    } catch {
      // Claim is opportunistic and renew is non-destructive. An old daemon,
      // another installation, or an identity mismatch leaves the view read-only.
      return observed;
    }
  }, [daemonTransitionInProgress, fixtureMode, loadLifecycleInstallationId]);

  const ownsDaemonLease = lifecycleLeaseOwnedBy(lifecycle, lifecycleInstallationId);
  const daemonLeaseConflict = Boolean(
    lifecycle
      && lifecycleInstallationId
      && lifecycleHasConflictingLease(lifecycle, lifecycleInstallationId),
  );

  const resetRequestLogPagination = useCallback((clearLogs: boolean) => {
    requestLogCursor.current = undefined;
    requestLogHasMore.current = false;
    requestLogLoadedFilterKey.current = undefined;
    requestLogLoadedPageCount.current = 0;
    setRequestLogsHasMore(false);
    if (clearLogs) setRequestLogs([]);
  }, []);

  const applyRequestLogFirstPage = useCallback((response: RequestLogsResponse, filterKey: string) => {
    const previousCursor = requestLogCursor.current;
    const previousHasMore = requestLogHasMore.current;
    const previousPageCount = requestLogLoadedPageCount.current;
    const preserveLoadedTail = requestLogLoadedFilterKey.current === filterKey && previousPageCount > 0;
    const responseHasMore = requestLogPageHasMore(response);

    setRequestLogs((current) => (
      preserveLoadedTail && responseHasMore
        ? mergeRequestLogs(response.logs, current)
        : mergeRequestLogs(response.logs, [])
    ));

    if (preserveLoadedTail && previousPageCount > 1 && responseHasMore) {
      requestLogCursor.current = previousCursor;
      requestLogHasMore.current = previousHasMore;
      setRequestLogsHasMore(previousHasMore);
    } else {
      requestLogCursor.current = response.nextCursor?.trim() || undefined;
      requestLogHasMore.current = responseHasMore;
      requestLogLoadedPageCount.current = 1;
      setRequestLogsHasMore(responseHasMore);
    }
    requestLogLoadedFilterKey.current = filterKey;
  }, []);

  const enhancedLaunch = useCodexEnhancedLaunch({
    fixtureMode,
    onError: (message) => setErrors((current) => ({ ...current, codex: message })),
    onFeedback: setFeedback,
    onReady: async () => {
      if (fixtureMode) return;
      try {
        setCodexStatus(await api.codexStatus());
      } catch {
        // The regular page refresh remains available if this best-effort refresh fails.
      }
    },
  });

  const loadSection = useCallback(async (section: AppSection, force = false) => {
    if (fixtureMode) {
      loadedSections.current.add(section);
      return;
    }
    if (!force && loadedSections.current.has(section) && section !== "requestLogs") return;
    const generation = (sectionGenerations.current[section] ?? 0) + 1;
    sectionGenerations.current[section] = generation;
    const isCurrent = () => sectionGenerations.current[section] === generation;
    let requestLogRequestGeneration: number | undefined;
    const isCurrentOperation = () => isCurrent()
      && (requestLogRequestGeneration === undefined
        || requestLogGeneration.current === requestLogRequestGeneration);
    setLoading((current) => ({ ...current, [section]: true }));
    setErrors((current) => ({ ...current, [section]: undefined }));
    try {
      switch (section) {
        case "overview": {
          const [nextDashboard, nextAdmin] = await Promise.all([
            api.dashboard(),
            api.sub2ApiAdmin().catch(() => undefined),
          ]);
          if (!isCurrent()) return;
          setDashboard(nextDashboard);
          await refreshSub2ApiPoolWithAdmin(false, nextAdmin);
          break;
        }
        case "codex": {
          const [nextCodex, nextGateway] = await Promise.all([api.codexStatus(), api.gateway()]);
          if (!isCurrent()) return;
          setCodexStatus(nextCodex);
          setGateway(nextGateway);
          await enhancedLaunch.recover();
          break;
        }
        case "sessions": {
          const response = await api.codexSessions();
          if (!isCurrent()) return;
          setSessions(response.threads);
          setSessionProviders(response.providers);
          break;
        }
        case "gateway": {
          const [nextGateway, nextAdmin] = await Promise.all([api.gateway(), api.sub2ApiAdmin().catch(() => undefined)]);
          if (!isCurrent()) return;
          setGateway(nextGateway);
          await refreshSub2ApiPoolWithAdmin(false, nextAdmin);
          break;
        }
        case "messaging": {
          const [accountsResult, groupsResult] = await Promise.allSettled([
            api.imAccounts(),
            api.telegramProjectGroups(),
          ]);
          if (!isCurrent()) return;
          if (accountsResult.status === "rejected") throw accountsResult.reason;
          setAccounts(accountsResult.value);
          setAccountsRefreshError(undefined);
          setTelegramProjectGroupAccounts(groupsResult.status === "fulfilled" ? groupsResult.value.accounts : []);
          break;
        }
        case "requestLogs": {
          const filterKey = requestLogFilterKey(defaultRequestLogFilters);
          if (requestLogLoadedFilterKey.current && requestLogLoadedFilterKey.current !== filterKey) {
            resetRequestLogPagination(true);
          }
          const requestGeneration = requestLogGeneration.current + 1;
          requestLogGeneration.current = requestGeneration;
          requestLogRequestGeneration = requestGeneration;
          const response = await api.requestLogs("limit=100&sort=newest");
          if (!isCurrentOperation()) return;
          applyRequestLogFirstPage(response, filterKey);
          break;
        }
        case "settings": {
          const [nextSettings, nextLifecycle] = await Promise.all([api.settings(), api.lifecycle().catch(() => undefined)]);
          if (!isCurrent()) return;
          setSettings(nextSettings);
          setLifecycle(nextLifecycle ? await reconcileLifecycleLease(nextLifecycle) : undefined);
          break;
        }
      }
      if (isCurrentOperation()) loadedSections.current.add(section);
    } catch (error) {
      if (isCurrentOperation()) {
        if (section === "messaging") setAccountsRefreshError(accountRefreshMessage(error));
        else recordError(section, error);
      }
    } finally {
      if (isCurrentOperation()) setLoading((current) => ({ ...current, [section]: false }));
    }
  }, [
    enhancedLaunch.recover,
    applyRequestLogFirstPage,
    fixtureMode,
    reconcileLifecycleLease,
    recordError,
    refreshSub2ApiPoolWithAdmin,
    resetRequestLogPagination,
  ]);

  const runRefresh = useCallback(async () => {
    // A safe restart is observation-only after the daemon accepts the request.
    // Never let an ordinary refresh race that wait and spawn the bundled
    // sidecar while the service manager is restoring the current executable.
    if (daemonTransitionRef.current) return;
    if (fixtureMode) {
      setLastCheckedAt(Date.now());
      setFeedback("已刷新预览数据");
      return;
    }
    if (!dashboard) setStatus("checking");
    try {
      let probe;
      try {
        probe = await api.probe();
      } catch (initialError) {
        const canStartDaemon = initialError instanceof ManagementError && initialError.connectionFailure;
        if (canStartDaemon && !daemonStartAttempted.current) {
          daemonStartAttempted.current = true;
          await api.startDaemon().catch(() => undefined);
          for (let attempt = 0; attempt < 18; attempt += 1) {
            await delay(350);
            try {
              const nextProbe = await api.probe();
              if (nextProbe.kind === "versioned" && nextProbe.health.ready) {
                probe = nextProbe;
                break;
              }
            } catch {
              // Continue the bounded readiness wait.
            }
          }
        }
        if (!probe) throw initialError;
      }
      if (probe.kind === "legacy") {
        setStatus("bridgeAvailable");
        setStatusMessage("后台服务正在运行，但需要更新后才能使用管理功能");
        setLastCheckedAt(Date.now());
        return;
      }
      const health = probe.health;
      if (health.service !== "mochiport") throw new Error("MochiPort 端口正被其他服务占用");
      if (health.apiMajor !== 1) {
        setStatus("bridgeAvailable");
        setStatusMessage(`后台管理 API v${health.apiMajor} 与当前界面不兼容`);
        return;
      }
      if (!health.ready) {
        setStatus("checking");
        setStatusMessage("后台服务正在启动");
        setLastCheckedAt(Date.now());
        return;
      }
      const [nextDashboard, nextAccounts, nextLifecycle, nextAdmin] = await Promise.all([
        api.dashboard(),
        api.imAccounts().then(
          (value) => ({ status: "fulfilled" as const, value }),
          (reason: unknown) => ({ status: "rejected" as const, reason }),
        ),
        api.lifecycle().catch(() => undefined),
        api.sub2ApiAdmin().catch(() => undefined),
      ]);
      setDashboard(nextDashboard);
      if (nextAccounts.status === "fulfilled") {
        setAccounts(nextAccounts.value);
        setAccountsRefreshError(undefined);
      } else {
        setAccountsRefreshError(accountRefreshMessage(nextAccounts.reason));
      }
      setLifecycle(nextLifecycle ? await reconcileLifecycleLease(nextLifecycle) : undefined);
      await refreshSub2ApiPoolWithAdmin(false, nextAdmin);
      loadedSections.current.add("overview");
      setStatus("available");
      setStatusMessage("本地服务已就绪");
      setLastCheckedAt(Date.now());
      setErrors((current) => ({ ...current, overview: undefined }));
    } catch (error) {
      loadedSections.current.clear();
      setStatus("unavailable");
      setStatusMessage(error instanceof Error ? error.message : "无法连接本地服务");
      setLastCheckedAt(Date.now());
      recordError("overview", error);
    }
  }, [
    dashboard,
    fixtureMode,
    reconcileLifecycleLease,
    recordError,
    refreshSub2ApiPoolWithAdmin,
  ]);

  const refresh = useCallback(async () => {
    if (refreshInFlight.current) {
      await refreshInFlight.current;
      return;
    }
    const task = runRefresh();
    refreshInFlight.current = task;
    try {
      await task;
    } finally {
      if (refreshInFlight.current === task) refreshInFlight.current = null;
    }
  }, [runRefresh]);

  const startDaemon = useCallback(async () => {
    if (fixtureMode || daemonTransitionRef.current) return false;
    daemonTransitionRef.current = true;
    setDaemonTransitionInProgress(true);
    setLifecycleOperationError(undefined);
    setErrors((current) => ({ ...current, overview: undefined }));
    setStatus("checking");
    setStatusMessage("正在启动本地服务");
    try {
      const result = await api.startDaemon();
      // A failed automatic first-run attempt must not permanently consume the
      // user's recovery path. Explicit startup always gets a fresh bounded wait.
      daemonStartAttempted.current = true;
      for (let attempt = 0; attempt < 24; attempt += 1) {
        await delay(350);
        try {
          const probe = await api.probe();
          if (probe.kind === "versioned" && probe.health.ready) {
            setFeedback(result.started ? "本地服务已启动" : result.message);
            daemonTransitionRef.current = false;
            setDaemonTransitionInProgress(false);
            await refresh();
            return true;
          }
        } catch {
          // Continue the bounded readiness wait.
        }
      }
      throw new Error("本地服务启动后未能在预期时间内就绪");
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setStatus("unavailable");
      setStatusMessage(message);
      setLastCheckedAt(Date.now());
      setErrors((current) => ({ ...current, overview: message }));
      setLifecycleOperationError(message);
      return false;
    } finally {
      daemonTransitionRef.current = false;
      setDaemonTransitionInProgress(false);
    }
  }, [fixtureMode, refresh]);

  useEffect(() => {
    void refresh();
  }, []); // Run once; manual and interval refreshes use the latest callback below.

  useEffect(() => {
    if (!fixtureMode && status !== "available") return;
    void loadSection(selectionState);
  }, [fixtureMode, loadSection, selectionState, status]);

  useEffect(() => {
    const timer = window.setInterval(() => {
      if (!daemonTransitionRef.current) void refresh();
    }, 15_000);
    return () => window.clearInterval(timer);
  }, [refresh]);

  useEffect(() => {
    if (
      fixtureMode
      || daemonTransitionInProgress
      || daemonLeaseTakeoverInProgress
      || managementCredentialRotationInProgress
      || !ownsDaemonLease
      || !lifecycle
    ) return;
    const instanceId = lifecycle.service.instanceId;
    const timer = window.setInterval(() => {
      void api.lifecycleLease("renew", lifecycle).then((renewed) => {
        setLifecycle((current) => current?.service.instanceId === instanceId ? renewed : current);
      }).catch(() => {
        // A failed non-destructive heartbeat never falls through to claim,
        // takeover, restart, or process control. The next ordinary refresh
        // reconciles ownership from a newly verified lifecycle snapshot.
      });
    }, 10_000);
    return () => window.clearInterval(timer);
  }, [daemonLeaseTakeoverInProgress, daemonTransitionInProgress, fixtureMode, lifecycle, managementCredentialRotationInProgress, ownsDaemonLease]);

  useEffect(() => {
    const theme = settings?.theme ?? "system";
    document.documentElement.dataset.theme = theme;
  }, [settings?.theme]);

  useEffect(() => {
    const stored = localStorage.getItem("mochiport.close-behavior");
    void api.setCloseBehavior(stored === "quit" ? "quit" : "tray");
  }, []);

  useEffect(() => {
    if (!feedback) return;
    const timer = window.setTimeout(() => setFeedback(undefined), 3200);
    return () => window.clearTimeout(timer);
  }, [feedback]);

  const saveGatewaySettings = useCallback(async (draft: GatewaySettingsDraft) => {
    return withBusy("gateway-settings", async () => {
      try {
        if (fixtureMode) {
          setGateway((current) => current ? { ...current, ...draft } : current);
        } else {
          const response = await api.updateGateway(draft);
          setGateway(response.gateway);
        }
        setFeedback("AI 网关设置已保存");
        return true;
      } catch (error) {
        recordError("gateway", error);
        return false;
      }
    });
  }, [fixtureMode, recordError, withBusy]);

  const saveProvider = useCallback(async (draft: ProviderDraft) => {
    return withBusy("provider-save", async () => {
      try {
        if (fixtureMode) {
          setGateway((current) => current ? {
            ...current,
            providers: [...current.providers.filter((provider) => provider.name !== (draft.originalName ?? draft.name)), draft],
          } : current);
        } else {
          const response = await api.upsertProvider({
            originalName: draft.originalName ?? null,
            name: draft.name,
            enabled: draft.enabled,
            providerType: draft.providerType,
            compatibility: draft.compatibility ?? null,
            baseUrl: draft.baseUrl,
            modelsUrl: draft.modelsUrl ?? null,
            models: draft.models,
            modelAliases: draft.modelAliases,
            promptCacheRetention: draft.promptCacheRetention ?? null,
            weight: draft.weight,
            timeoutSecs: draft.timeoutSecs,
            apiKey: draft.apiKey || null,
            clearApiKey: draft.clearApiKey ?? false,
          });
          setGateway(response.gateway);
        }
        setFeedback(draft.originalName ? "模型服务已更新" : "模型服务已添加");
        return true;
      } catch (error) {
        recordError("gateway", error);
        return false;
      }
    });
  }, [fixtureMode, recordError, withBusy]);

  const deleteProvider = useCallback(async (name: string) => {
    return withBusy(`provider-delete:${name}`, async () => {
      try {
        if (fixtureMode) {
          setGateway((current) => current ? { ...current, providers: current.providers.filter((provider) => provider.name !== name) } : current);
        } else {
          const response = await api.deleteProvider(name);
          setGateway(response.gateway);
        }
        setFeedback("模型服务已删除");
        return true;
      } catch (error) {
        recordError("gateway", error);
        return false;
      }
    });
  }, [fixtureMode, recordError, withBusy]);

  const saveSub2Api = useCallback(async (baseUrl: string, adminApiKey: string) => {
    return withBusy("sub2api-save", async () => {
      const mutationGeneration = sub2ApiMutationGeneration.current + 1;
      sub2ApiMutationGeneration.current = mutationGeneration;
      try {
        let nextAdmin: Sub2ApiAdmin;
        if (fixtureMode) {
          nextAdmin = { configured: true, baseUrl, secretSet: true };
        } else {
          const response = await api.updateSub2ApiAdmin(baseUrl, adminApiKey || null);
          nextAdmin = response.sub2api;
        }
        if (sub2ApiMutationGeneration.current !== mutationGeneration) return false;
        publishSub2ApiAdmin(nextAdmin);
        invalidateSub2ApiPool();
        await refreshSub2ApiPoolWithAdmin(true, nextAdmin);
        if (sub2ApiMutationGeneration.current !== mutationGeneration) return false;
        setFeedback("Sub2API 账号池已连接");
        return true;
      } catch (error) {
        recordError("gateway", error);
        return false;
      }
    });
  }, [
    fixtureMode,
    invalidateSub2ApiPool,
    publishSub2ApiAdmin,
    recordError,
    refreshSub2ApiPoolWithAdmin,
    withBusy,
  ]);

  const disconnectSub2Api = useCallback(async () => {
    return withBusy("sub2api-disconnect", async () => {
      const mutationGeneration = sub2ApiMutationGeneration.current + 1;
      sub2ApiMutationGeneration.current = mutationGeneration;
      try {
        const nextAdmin = fixtureMode
          ? { configured: false, baseUrl: "", secretSet: false }
          : (await api.disconnectSub2ApiAdmin()).sub2api;
        if (sub2ApiMutationGeneration.current !== mutationGeneration) return false;
        publishSub2ApiAdmin(nextAdmin);
        invalidateSub2ApiPool();
        setFeedback("已断开 Sub2API 账号池");
        return true;
      } catch (error) {
        recordError("gateway", error);
        return false;
      }
    });
  }, [fixtureMode, invalidateSub2ApiPool, publishSub2ApiAdmin, recordError, withBusy]);

  const toggleAccount = useCallback(async (account: IMAccount, enabled: boolean) => {
    const id = `${account.platform}:${account.accountId}`;
    return withBusy(`account:${id}`, async () => {
      const generation = (accountMutationGenerations.current.get(id) ?? 0) + 1;
      accountMutationGenerations.current.set(id, generation);
      setAccounts((current) => current.map((item) => item.platform === account.platform && item.accountId === account.accountId ? { ...item, enabled } : item));
      try {
        if (!fixtureMode) await api.setIMAccountEnabled(account.platform, account.accountId, enabled);
        setFeedback(enabled ? "消息账号已启用" : "消息账号已停用");
        return true;
      } catch (error) {
        if (accountMutationGenerations.current.get(id) === generation) {
          setAccounts((current) => current.map((item) =>
            item.platform === account.platform && item.accountId === account.accountId
              ? { ...item, enabled: account.enabled }
              : item));
        }
        recordError("messaging", error);
        return false;
      }
    });
  }, [fixtureMode, recordError, withBusy]);

  const deleteAccount = useCallback(async (account: IMAccount) => {
    const id = `${account.platform}:${account.accountId}`;
    return withBusy(`account-delete:${id}`, async () => {
      try {
        if (!fixtureMode) await api.deleteIMAccount(account.platform, account.accountId);
        setAccounts((current) => current.filter((item) => item.platform !== account.platform || item.accountId !== account.accountId));
        if (account.platform === "telegram") {
          setTelegramProjectGroupAccounts((current) => current.filter((item) => item.accountId !== account.accountId));
        }
        setFeedback("消息账号已删除");
        return true;
      } catch (error) {
        recordError("messaging", error);
        return false;
      }
    });
  }, [fixtureMode, recordError, withBusy]);

  const saveTelegramProjectGroups = useCallback(async (accountId: string, groups: TelegramProjectGroup[]) => {
    return withBusy("telegram-project-groups", async () => {
      try {
        const response = fixtureMode
          ? { accountId, projectGroups: groups }
          : await api.updateTelegramProjectGroups(accountId, groups);
        setTelegramProjectGroupAccounts((current) => [
          ...current.filter((item) => item.accountId !== accountId),
          { accountId: response.accountId, projectGroups: response.projectGroups },
        ]);
        setFeedback("项目群配置已保存；重启后台服务后生效");
        return true;
      } catch (error) {
        recordError("messaging", error);
        return false;
      }
    });
  }, [fixtureMode, recordError, withBusy]);

  const addTelegram = useCallback(async (token: string, mentionOnly: boolean) => {
    return withBusy("onboarding", async () => {
      try {
        if (!fixtureMode) {
          await api.configureTelegram(token, mentionOnly);
          setAccounts(await api.imAccounts());
        } else {
          setAccounts((current) => [...current, { ...fixtureAccounts[0], accountId: `fixture-${current.length + 1}`, displayName: "新 Telegram Bot" }]);
        }
        setFeedback("Telegram 账号已连接");
        return true;
      } catch (error) {
        recordError("messaging", error);
        return false;
      }
    });
  }, [fixtureMode, recordError, withBusy]);

  const addFeishu = useCallback(async (appId: string, appSecret: string) => {
    return withBusy("onboarding", async () => {
      try {
        if (!fixtureMode) {
          await api.configureFeishu(appId, appSecret);
          setAccounts(await api.imAccounts());
        } else {
          setAccounts((current) => [...current, { ...fixtureAccounts[1], accountId: `fixture-${current.length + 1}`, displayName: "新飞书机器人" }]);
        }
        setFeedback("飞书账号已连接");
        return true;
      } catch (error) {
        recordError("messaging", error);
        return false;
      }
    });
  }, [fixtureMode, recordError, withBusy]);

  const refreshCodexAndGateway = useCallback(async () => {
    const [nextCodex, nextGateway] = await Promise.allSettled([
      api.codexStatus(),
      api.gateway(),
    ]);
    if (nextCodex.status === "fulfilled") setCodexStatus(nextCodex.value);
    if (nextGateway.status === "fulfilled") setGateway(nextGateway.value);
    if (nextCodex.status === "rejected") throw nextCodex.reason;
    if (nextGateway.status === "rejected") throw nextGateway.reason;
  }, []);

  const updateGatewayEnabled = useCallback(async (current: Gateway, enabled: boolean) => {
    if (current.enabled === enabled) return current;
    const response = await api.updateGateway(gatewaySettingsWithEnabled(current, enabled));
    setGateway(response.gateway);
    return response.gateway;
  }, []);

  const runCodexAction = useCallback(async (action: CodexAction) => {
    return withBusy(`codex:${action}`, async () => {
      setErrors((current) => ({ ...current, codex: undefined }));
      try {
        if (fixtureMode) {
          if (action === "configure") {
            setGateway((current) => current ? { ...current, enabled: true } : current);
            setCodexStatus(fixtureCodexStatus);
          } else if (action === "uninstall") {
            setGateway((current) => current ? { ...current, enabled: false } : current);
            setCodexStatus((current) => current ? {
              ...current,
              configured: false,
              configOk: false,
              remoteControlConfigured: false,
              providerMode: "direct-api",
              activeProvider: null,
            } : current);
          } else if (action === "direct-api-mode") {
            setCodexStatus((current) => ({
              ...(current ?? fixtureCodexStatus),
              providerMode: "direct-api",
              providerModeMessage: "Codex 使用原来的直连 API 设置",
              activeProvider: "openai",
            }));
          } else {
            setCodexStatus(fixtureCodexStatus);
          }
        } else if (action === "configure") {
          const previousGateway = gateway ?? await api.gateway();
          let transactionGateway = previousGateway;
          let gatewayWasChanged = false;
          try {
            if (!previousGateway.enabled) {
              transactionGateway = await updateGatewayEnabled(previousGateway, true);
              gatewayWasChanged = true;
            }
            await api.codexAction(action);
          } catch (error) {
            if (gatewayWasChanged) {
              await updateGatewayEnabled(transactionGateway, previousGateway.enabled).catch(() => undefined);
            }
            await refreshCodexAndGateway().catch(() => undefined);
            throw error;
          }
          await refreshCodexAndGateway();
        } else if (action === "uninstall") {
          const previousGateway = gateway ?? await api.gateway();
          await api.codexAction(action);
          try {
            await updateGatewayEnabled(previousGateway, false);
            await refreshCodexAndGateway();
          } catch (error) {
            await refreshCodexAndGateway().catch(() => undefined);
            throw error;
          }
        } else {
          await api.codexAction(action);
          await refreshCodexAndGateway();
        }
        setFeedback(codexActionFeedback[action]);
        return true;
      } catch (error) {
        recordError("codex", error);
        return false;
      }
    });
  }, [fixtureMode, gateway, recordError, refreshCodexAndGateway, updateGatewayEnabled, withBusy]);

  const reloadSessionsFromDaemon = useCallback(async (): Promise<string | undefined> => {
    const generation = (sectionGenerations.current.sessions ?? 0) + 1;
    sectionGenerations.current.sessions = generation;
    const isCurrent = () => sectionGenerations.current.sessions === generation;
    setLoading((current) => ({ ...current, sessions: true }));
    try {
      const response = await api.codexSessions();
      if (!isCurrent()) return undefined;
      setSessions(response.threads);
      setSessionProviders(response.providers);
      loadedSections.current.add("sessions");
      return undefined;
    } catch (error) {
      if (!isCurrent()) return undefined;
      return error instanceof Error ? error.message : String(error);
    } finally {
      if (isCurrent()) setLoading((current) => ({ ...current, sessions: false }));
    }
  }, []);

  const saveSettings = useCallback(async (draft: SettingsDraft) => {
    return withBusy("settings-save", async () => {
      try {
        if (fixtureMode) {
          setSettings((current) => current ? {
            ...current,
            language: draft.language,
            theme: draft.theme,
            localConnectionMode: draft.localConnectionMode,
            outboundProxy: { ...current.outboundProxy, mode: draft.outboundProxyMode, url: draft.outboundProxyUrl ?? current.outboundProxy.url },
          } : current);
        } else {
          const response = await api.configureSettings(draft);
          setSettings(response.settings);
        }
        setFeedback("设置已保存");
        return true;
      } catch (error) {
        recordError("settings", error);
        return false;
      }
    });
  }, [fixtureMode, recordError, withBusy]);

  const takeOverDaemonManagement = useCallback(async () => {
    if (
      fixtureMode
      || daemonTransitionRef.current
      || daemonLeaseTakeoverInProgress
      || managementCredentialRotationInProgress
      || !daemonLeaseConflict
      || !lifecycle
    ) {
      setLifecycleOperationError("确认后后台服务管理租约已变化，请刷新状态并重新确认。");
      return false;
    }
    daemonTransitionRef.current = true;
    setDaemonLeaseTakeoverInProgress(true);
    setLifecycleOperationError(undefined);
    try {
      const replacement = await api.takeOverLifecycleLease(lifecycle);
      setLifecycle(replacement);
      setStatus("available");
      setStatusMessage("本地服务已就绪");
      setLastCheckedAt(Date.now());
      setFeedback("已接管后台服务");
      return true;
    } catch (error) {
      setLifecycleOperationError(error instanceof Error ? error.message : String(error));
      try {
        setLifecycle(await api.lifecycle());
      } catch {
        // Leave the last known lifecycle visible; the next refresh can recover it.
      }
      return false;
    } finally {
      daemonTransitionRef.current = false;
      setDaemonLeaseTakeoverInProgress(false);
    }
  }, [daemonLeaseConflict, daemonLeaseTakeoverInProgress, fixtureMode, lifecycle, managementCredentialRotationInProgress]);

  const rotateManagementCredential = useCallback(async () => {
    if (
      fixtureMode
      || daemonTransitionRef.current
      || daemonLeaseTakeoverInProgress
      || managementCredentialRotationInProgress
      || !ownsDaemonLease
      || !lifecycle
    ) {
      setLifecycleOperationError("确认后后台服务管理状态已变化，请刷新状态并重新确认。");
      return false;
    }
    daemonTransitionRef.current = true;
    setManagementCredentialRotationInProgress(true);
    setLifecycleOperationError(undefined);
    try {
      const replacement = await api.rotateManagementCredential(lifecycle);
      setLifecycle(replacement);
      setStatus("available");
      setStatusMessage("本地服务已就绪");
      setLastCheckedAt(Date.now());
      setFeedback("管理凭据已重新生成");
      return true;
    } catch (error) {
      setLifecycleOperationError(error instanceof Error ? error.message : String(error));
      try {
        setLifecycle(await api.lifecycle());
      } catch {
        // Leave the last known lifecycle visible; the next refresh can recover it.
      }
      return false;
    } finally {
      daemonTransitionRef.current = false;
      setManagementCredentialRotationInProgress(false);
    }
  }, [daemonLeaseTakeoverInProgress, fixtureMode, lifecycle, managementCredentialRotationInProgress, ownsDaemonLease]);

  const restartDaemon = useCallback(async () => {
    if (fixtureMode || daemonTransitionInProgress || !ownsDaemonLease || !lifecycle) {
      setLifecycleOperationError("当前 Windows 安装没有后台服务管理权。");
      return false;
    }
    // A user-requested safe restart permanently consumes this GUI session's
    // automatic bootstrap allowance; failed recovery must stay observation-only.
    daemonStartAttempted.current = true;
    daemonTransitionRef.current = true;
    setDaemonTransitionInProgress(true);
    setLifecycleOperationError(undefined);
    setFeedback("正在安全重启后台服务");
    setStatus("checking");
    setStatusMessage("正在等待后台服务以相同路径和构建恢复");
    try {
      const replacement = await api.safeRestartLifecycle(lifecycle);
      setLifecycle(replacement);
      loadedSections.current.clear();
      setStatus("available");
      setStatusMessage("本地服务已就绪");
      setLastCheckedAt(Date.now());
      setFeedback("后台服务已安全重启");
      await loadSection(selectionState, true);
      return true;
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setLifecycleOperationError(message);
      setFeedback(undefined);
      // The daemon may still be restarting after the bounded recovery wait.
      // Re-read lifecycle directly: the ordinary refresh path is intentionally
      // avoided because it includes first-run sidecar startup.
      try {
        setLifecycle(await api.lifecycle());
        setStatus("available");
        setStatusMessage("本地服务已就绪");
      } catch {
        setStatus("unavailable");
        setStatusMessage("后台服务尚未恢复；未启动或切换任何二进制");
      }
      return false;
    } finally {
      daemonTransitionRef.current = false;
      setDaemonTransitionInProgress(false);
    }
  }, [daemonTransitionInProgress, fixtureMode, lifecycle, loadSection, ownsDaemonLease, selectionState]);

  const queryRequestLogs = useCallback(async (filters: RequestLogFilters, append = false) => {
    if (fixtureMode) return;
    const filterKey = requestLogFilterKey(filters);
    if (append && requestLogLoadedFilterKey.current !== filterKey) return;
    if (!append && requestLogLoadedFilterKey.current && requestLogLoadedFilterKey.current !== filterKey) {
      resetRequestLogPagination(true);
    }
    const requestedCursor = append ? requestLogCursor.current : undefined;
    if (append && !requestedCursor) return;
    const generation = requestLogGeneration.current + 1;
    requestLogGeneration.current = generation;
    const isCurrent = () => requestLogGeneration.current === generation;
    setLoading((current) => ({ ...current, requestLogs: true }));
    setErrors((current) => ({ ...current, requestLogs: undefined }));
    try {
      const query = new URLSearchParams({ limit: "100", sort: filters.sort });
      const appendValue = (name: string, value?: string | null) => {
        const normalized = value?.trim();
        if (normalized) query.set(name, normalized);
      };
      appendValue("query", filters.query);
      appendValue("status", filters.status);
      appendValue("channel", filters.channel);
      appendValue("modelId", filters.modelId);
      if (append) appendValue("cursor", requestedCursor);

      const response = await api.requestLogs(query.toString());
      if (!isCurrent()) return;
      if (!append) {
        applyRequestLogFirstPage(response, filterKey);
      } else {
        setRequestLogs((current) => mergeRequestLogs(current, response.logs));
        requestLogCursor.current = response.nextCursor?.trim() || undefined;
        const hasMore = requestLogPageHasMore(response, requestedCursor);
        requestLogHasMore.current = hasMore;
        requestLogLoadedPageCount.current += 1;
        setRequestLogsHasMore(hasMore);
      }
    } catch (error) {
      if (isCurrent()) recordError("requestLogs", error);
    } finally {
      if (isCurrent()) setLoading((current) => ({ ...current, requestLogs: false }));
    }
  }, [applyRequestLogFirstPage, fixtureMode, recordError, resetRequestLogPagination]);

  const loadRequestLogDetail = useCallback(async (id: number) => {
    try {
      if (fixtureMode) {
        const log = requestLogs.find((item) => item.id === id);
        return log ? { ...log, requestJson: "{\n  \"model\": \"gpt-5.4\",\n  \"stream\": true\n}", responseJson: "{\n  \"status\": \"completed\"\n}" } : undefined;
      }
      return (await api.requestLogDetail(id)).log;
    } catch (error) {
      recordError("requestLogs", error);
      return undefined;
    }
  }, [fixtureMode, recordError, requestLogs]);

  const clearRequestLogs = useCallback(async (olderThanDays?: number) => {
    return withBusy("logs-clear", async () => {
      try {
        if (fixtureMode) {
          setRequestLogs(olderThanDays ? requestLogs.slice(0, 1) : []);
          resetRequestLogPagination(false);
        } else {
          const response = olderThanDays ? await api.clearOldRequestLogs(olderThanDays) : await api.clearRequestLogs();
          setFeedback(`已删除 ${response.deleted ?? 0} 条请求日志`);
          await loadSection("requestLogs", true);
          return true;
        }
        setFeedback(olderThanDays ? "已清理旧日志" : "请求日志已清空");
        return true;
      } catch (error) {
        recordError("requestLogs", error);
        return false;
      }
    });
  }, [fixtureMode, loadSection, recordError, requestLogs, withBusy]);

  const completeFixtureOnboarding = useCallback((platform: string) => {
    if (!fixtureMode) return;
    const template = platform === "telegram" ? fixtureAccounts[0] : fixtureAccounts[1];
    setAccounts((current) => [...current, { ...template, platform, accountId: `fixture-${platform}-${current.length + 1}`, displayName: `新${platform}账号` }]);
    setFeedback("消息账号已连接");
  }, [fixtureMode]);

  const value = useMemo<AppModelValue>(() => ({
    fixtureMode,
    selection: selectionState,
    setSelection,
    status,
    statusMessage,
    lastCheckedAt,
    dashboard,
    lifecycle,
    ownsDaemonLease,
    daemonLeaseConflict,
    daemonTransitionInProgress,
    daemonLeaseTakeoverInProgress,
    managementCredentialRotationInProgress,
    lifecycleOperationError,
    codexStatus,
    codexEnhancedOperation: enhancedLaunch.operation,
    codexEnhancedWaitingForAppExit: enhancedLaunch.waitingForAppExit,
    codexEnhancedUsesLegacyFallback: enhancedLaunch.usesLegacyFallback,
    codexEnhancedLaunchError: enhancedLaunch.launchError,
    codexEnhancedLaunchInProgress: enhancedLaunch.inProgress,
    canCancelCodexEnhancedLaunch: enhancedLaunch.canCancel,
    sessions,
    sessionProviders,
    gateway,
    accounts,
    accountsRefreshError,
    telegramProjectGroupAccounts,
    requestLogs,
    requestLogsHasMore,
    settings,
    sub2ApiAdmin,
    sub2ApiPool,
    sub2ApiPoolLoading,
    sub2ApiPoolError,
    loading,
    errors,
    busy,
    feedback,
    refresh,
    startDaemon,
    loadSection,
    dismissError: (section) => setErrors((current) => ({ ...current, [section]: undefined })),
    dismissAccountsRefreshError: () => setAccountsRefreshError(undefined),
    clearFeedback: () => setFeedback(undefined),
    clearLifecycleOperationError: () => setLifecycleOperationError(undefined),
    saveGatewaySettings,
    saveProvider,
    deleteProvider,
    saveSub2Api,
    disconnectSub2Api,
    refreshSub2ApiPool,
    toggleAccount,
    deleteAccount,
    saveTelegramProjectGroups,
    addTelegram,
    addFeishu,
    runCodexAction,
    beginCodexEnhancedLaunch: enhancedLaunch.begin,
    cancelCodexEnhancedLaunch: enhancedLaunch.cancel,
    saveSettings,
    takeOverDaemonManagement,
    rotateManagementCredential,
    restartDaemon,
    loadRequestLogDetail,
    queryRequestLogs,
    clearRequestLogs,
    completeFixtureOnboarding,
  }), [
    accounts, accountsRefreshError, addFeishu, addTelegram, busy, clearRequestLogs, codexStatus, dashboard, enhancedLaunch,
    deleteAccount, deleteProvider, disconnectSub2Api, errors, feedback, fixtureMode, gateway, lastCheckedAt,
    lifecycle, ownsDaemonLease, daemonLeaseConflict, daemonTransitionInProgress, daemonLeaseTakeoverInProgress,
    managementCredentialRotationInProgress, lifecycleOperationError,
    loadRequestLogDetail, loadSection, loading, queryRequestLogs, refresh, startDaemon, requestLogs,
    requestLogsHasMore,
    refreshSub2ApiPool, restartDaemon, rotateManagementCredential, runCodexAction, saveGatewaySettings, saveProvider,
    saveSettings, takeOverDaemonManagement, saveSub2Api, selectionState, sessionProviders,
    sessions, setSelection, settings, status, statusMessage, sub2ApiAdmin, sub2ApiPool, sub2ApiPoolError,
    sub2ApiPoolLoading, toggleAccount,
    saveTelegramProjectGroups, telegramProjectGroupAccounts,
    completeFixtureOnboarding,
  ]);

  return <AppModelContext.Provider value={value}>{children}</AppModelContext.Provider>;
}

export function useAppModel(): AppModelValue {
  const value = useContext(AppModelContext);
  if (!value) throw new Error("useAppModel must be used inside AppModelProvider");
  return value;
}
