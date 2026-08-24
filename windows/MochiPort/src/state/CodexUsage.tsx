import { invoke } from "@tauri-apps/api/core";
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
import { showNativeNotification } from "../native/windowsIntegration";
import {
  loadNotificationMessageStyle,
  NOTIFICATION_SOUND_STORAGE_KEY,
} from "../utils/notificationMessages";
import {
  evaluateUsageNotifications,
  type UsageNotificationEvent,
  type UsageQuotaWindow,
} from "../utils/usageNotifications";
import type { UsageWeeklyReport } from "../utils/usageBriefings";

export interface UsageDay {
  day: string;
  tokens: number;
}

export interface UsageProject {
  project: string;
  tokens: number;
}

export interface CodexUsageSnapshot {
  available: boolean;
  sourceDirectory: string;
  scannedFiles: number;
  todayTokens: number;
  todayRequests: number;
  yesterdayTokens?: number;
  yesterdayCostUsd?: number;
  yesterdayTopProject?: string | null;
  tokensPerMinute: number;
  burnRateTokensPerMinute?: number;
  activeBaselineTokensPerMinute: number;
  estimatedCostUsd: number;
  quotaWindows: UsageQuotaWindow[];
  sevenDay: UsageDay[];
  dailyUsage?: UsageDay[];
  sevenDayProjects?: UsageProject[];
  streakDays?: number;
  previousBestDailyTokens?: number | null;
  weeklyReport?: UsageWeeklyReport | null;
  topProject?: string | null;
  lastActivityAtMs?: number | null;
  updatedAtMs: number;
}

interface CodexUsageContextValue {
  snapshot: CodexUsageSnapshot | undefined;
  loading: boolean;
  error: string | undefined;
  history: UsageHistoryEvent[];
  refresh: () => Promise<void>;
  clearHistory: () => void;
}

const REFRESH_INTERVAL_MS = 30_000;
const HISTORY_STORAGE_KEY = "mochiport.usage-event-history";
const MAX_HISTORY_EVENTS = 50;

export interface UsageHistoryEvent extends UsageNotificationEvent {
  occurredAtMs: number;
}

const CodexUsageContext = createContext<CodexUsageContextValue | undefined>(undefined);

function isTauri(): boolean {
  return "__TAURI_INTERNALS__" in window || "__TAURI__" in window;
}

function isSnapshot(value: unknown): value is CodexUsageSnapshot {
  if (typeof value !== "object" || value === null) return false;
  const snapshot = value as Partial<CodexUsageSnapshot>;
  const weeklyReport = snapshot.weeklyReport;
  const validWeeklyReport = weeklyReport === undefined
    || weeklyReport === null
    || (typeof weeklyReport.lastWeekTokens === "number"
      && typeof weeklyReport.lastWeekCostUsd === "number"
      && typeof weeklyReport.previousWeekTokens === "number"
      && (weeklyReport.lastWeekTopProject === undefined
        || weeklyReport.lastWeekTopProject === null
        || typeof weeklyReport.lastWeekTopProject === "string"));
  return typeof snapshot.available === "boolean"
    && typeof snapshot.sourceDirectory === "string"
    && typeof snapshot.scannedFiles === "number"
    && typeof snapshot.todayTokens === "number"
    && typeof snapshot.todayRequests === "number"
    && (snapshot.yesterdayTokens === undefined || typeof snapshot.yesterdayTokens === "number")
    && (snapshot.yesterdayCostUsd === undefined || typeof snapshot.yesterdayCostUsd === "number")
    && (snapshot.yesterdayTopProject === undefined
      || snapshot.yesterdayTopProject === null
      || typeof snapshot.yesterdayTopProject === "string")
    && typeof snapshot.tokensPerMinute === "number"
    && (snapshot.burnRateTokensPerMinute === undefined
      || typeof snapshot.burnRateTokensPerMinute === "number")
    && typeof snapshot.activeBaselineTokensPerMinute === "number"
    && typeof snapshot.estimatedCostUsd === "number"
    && typeof snapshot.updatedAtMs === "number"
    && Array.isArray(snapshot.quotaWindows)
    && snapshot.quotaWindows.every((window) => typeof window.kind === "string"
      && typeof window.usedPercent === "number"
      && (window.resetsAtMs === undefined || window.resetsAtMs === null || typeof window.resetsAtMs === "number")
      && (window.depletionEtaMs === undefined || window.depletionEtaMs === null || typeof window.depletionEtaMs === "number"))
    && Array.isArray(snapshot.sevenDay)
    && snapshot.sevenDay.every((entry) => typeof entry.day === "string" && typeof entry.tokens === "number")
    && (snapshot.dailyUsage === undefined
      || (Array.isArray(snapshot.dailyUsage)
        && snapshot.dailyUsage.every((entry) => typeof entry.day === "string" && typeof entry.tokens === "number")))
    && (snapshot.sevenDayProjects === undefined
      || (Array.isArray(snapshot.sevenDayProjects)
        && snapshot.sevenDayProjects.every((entry) => typeof entry.project === "string" && typeof entry.tokens === "number")))
    && (snapshot.streakDays === undefined || typeof snapshot.streakDays === "number")
    && (snapshot.previousBestDailyTokens === undefined
      || snapshot.previousBestDailyTokens === null
      || typeof snapshot.previousBestDailyTokens === "number")
    && validWeeklyReport;
}

function fixtureSnapshot(): CodexUsageSnapshot {
  const formatter = new Intl.DateTimeFormat("sv-SE", { year: "numeric", month: "2-digit", day: "2-digit" });
  const dailyUsage = Array.from({ length: 105 }, (_, index) => {
    const day = new Date();
    day.setHours(0, 0, 0, 0);
    day.setDate(day.getDate() - (104 - index));
    const weekday = day.getDay();
    const wave = 72_000 + ((index * 47_311) % 154_000);
    const tokens = weekday === 0 ? Math.round(wave * 0.2) : weekday === 6 ? Math.round(wave * 0.48) : wave;
    return { day: formatter.format(day), tokens };
  });
  const sevenDay = dailyUsage.slice(-7);
  return {
    available: true,
    sourceDirectory: "C:\\Users\\Mia\\.codex\\sessions",
    scannedFiles: 23,
    todayTokens: sevenDay.at(-1)?.tokens ?? 0,
    todayRequests: 17,
    yesterdayTokens: dailyUsage.at(-2)?.tokens ?? 0,
    yesterdayCostUsd: 1.32,
    yesterdayTopProject: "MochiPort",
    tokensPerMinute: 982.3,
    burnRateTokensPerMinute: 734.8,
    activeBaselineTokensPerMinute: 410.2,
    estimatedCostUsd: 1.84,
    quotaWindows: [
      {
        kind: "session5h",
        usedPercent: 38,
        resetsAtMs: Date.now() + 2.4 * 60 * 60_000,
        depletionEtaMs: null,
      },
      {
        kind: "weekly",
        usedPercent: 61,
        resetsAtMs: Date.now() + 3.2 * 24 * 60 * 60_000,
        depletionEtaMs: null,
      },
    ],
    sevenDay,
    dailyUsage,
    sevenDayProjects: [
      { project: "MochiPort", tokens: 648_400 },
      { project: "CellularBridge", tokens: 284_600 },
      { project: "agent-reach", tokens: 132_900 },
      { project: "notes", tokens: 74_300 },
    ],
    streakDays: 5,
    previousBestDailyTokens: Math.max(...dailyUsage.slice(0, -1).map((entry) => entry.tokens)),
    weeklyReport: {
      lastWeekTokens: 1_250_000,
      lastWeekCostUsd: 12.35,
      previousWeekTokens: 1_000_000,
      lastWeekTopProject: "MochiPort",
    },
    topProject: "MochiPort",
    lastActivityAtMs: Date.now() - 38_000,
    updatedAtMs: Date.now(),
  };
}

function browserSnapshot(): CodexUsageSnapshot {
  return {
    available: false,
    sourceDirectory: "%USERPROFILE%\\.codex\\sessions",
    scannedFiles: 0,
    todayTokens: 0,
    todayRequests: 0,
    yesterdayTokens: 0,
    yesterdayCostUsd: 0,
    yesterdayTopProject: null,
    tokensPerMinute: 0,
    burnRateTokensPerMinute: 0,
    activeBaselineTokensPerMinute: 0,
    estimatedCostUsd: 0,
    quotaWindows: [],
    sevenDay: [],
    dailyUsage: [],
    sevenDayProjects: [],
    streakDays: 0,
    previousBestDailyTokens: null,
    weeklyReport: null,
    updatedAtMs: Date.now(),
  };
}

function loadHistory(): UsageHistoryEvent[] {
  try {
    const value = JSON.parse(localStorage.getItem(HISTORY_STORAGE_KEY) ?? "[]") as unknown;
    if (!Array.isArray(value)) return [];
    return value.filter((entry): entry is UsageHistoryEvent => typeof entry === "object" && entry !== null
      && typeof (entry as Partial<UsageHistoryEvent>).id === "string"
      && typeof (entry as Partial<UsageHistoryEvent>).kind === "string"
      && typeof (entry as Partial<UsageHistoryEvent>).title === "string"
      && typeof (entry as Partial<UsageHistoryEvent>).body === "string"
      && typeof (entry as Partial<UsageHistoryEvent>).cooldownMs === "number"
      && typeof (entry as Partial<UsageHistoryEvent>).occurredAtMs === "number").slice(0, MAX_HISTORY_EVENTS);
  } catch {
    return [];
  }
}

export function CodexUsageProvider({ children }: PropsWithChildren) {
  const fixtureMode = useMemo(() => {
    const params = new URLSearchParams(window.location.search);
    return params.has("fixture") || import.meta.env.VITE_FIXTURE_MODE === "true";
  }, []);
  const [snapshot, setSnapshot] = useState<CodexUsageSnapshot | undefined>(() => fixtureMode ? fixtureSnapshot() : undefined);
  const [loading, setLoading] = useState(!fixtureMode);
  const [error, setError] = useState<string>();
  const [history, setHistory] = useState<UsageHistoryEvent[]>(loadHistory);
  const previousSnapshot = useRef<CodexUsageSnapshot | undefined>(undefined);
  const refreshInFlight = useRef<Promise<void> | null>(null);
  const notificationInFlight = useRef(new Set<string>());

  const evaluateNotifications = useCallback((next: CodexUsageSnapshot) => {
    const previous = previousSnapshot.current;
    previousSnapshot.current = next;
    const candidates = evaluateUsageNotifications(next, previous, {
      warnThreshold: Number(localStorage.getItem("mochiport.warn-threshold") ?? 70),
      criticalThreshold: Number(localStorage.getItem("mochiport.critical-threshold") ?? 90),
      notifyLimitThreshold: localStorage.getItem("mochiport.notify-limit-threshold") !== "off",
      notifyDepletion: localStorage.getItem("mochiport.notify-depletion") !== "off",
      notifyWindowReset: localStorage.getItem("mochiport.notify-window-reset") !== "off",
      notifyBurnSpike: localStorage.getItem("mochiport.notify-burn-spike") !== "off",
      notifyComeback: localStorage.getItem("mochiport.notify-comeback") !== "off",
      notifyBriefing: localStorage.getItem("mochiport.notify-briefing") !== "off",
      includeStreak: localStorage.getItem("mochiport.fun-streak") !== "off",
      includeWeeklyReport: localStorage.getItem("mochiport.fun-weekly-report") !== "off",
      notifyMilestone: localStorage.getItem("mochiport.notify-milestone-record") !== "off",
      notifyRecord: localStorage.getItem("mochiport.notify-milestone-record") !== "off",
      ...loadNotificationMessageStyle(localStorage),
    });
    if (!candidates.length) return;

    const nowMs = Date.now();
    const storedTimestamp = (key: string) => {
      const timestamp = Number(localStorage.getItem(key) ?? 0);
      return Number.isFinite(timestamp) ? timestamp : 0;
    };
    const historyEvent = candidates.find((event) => (
      nowMs - storedTimestamp(`mochiport.event-cooldown.${event.id}`) >= event.cooldownMs
    ));
    if (historyEvent) {
      const recorded = { ...historyEvent, occurredAtMs: nowMs };
      setHistory((current) => {
        const nextHistory = [recorded, ...current].slice(0, MAX_HISTORY_EVENTS);
        localStorage.setItem(HISTORY_STORAGE_KEY, JSON.stringify(nextHistory));
        return nextHistory;
      });
      localStorage.setItem(`mochiport.event-cooldown.${historyEvent.id}`, String(nowMs));
    }
    if (localStorage.getItem("mochiport.notifications") !== "on") return;
    const notificationEvent = candidates.find((event) => (
      !notificationInFlight.current.has(event.id)
      && nowMs - storedTimestamp(`mochiport.notification-cooldown.${event.id}`) >= event.cooldownMs
    ));
    if (!notificationEvent) return;
    const cooldownKey = `mochiport.notification-cooldown.${notificationEvent.id}`;
    notificationInFlight.current.add(notificationEvent.id);
    void showNativeNotification(
      notificationEvent.title,
      notificationEvent.body,
      localStorage.getItem(NOTIFICATION_SOUND_STORAGE_KEY) === "on",
    ).then((sent) => {
      if (sent) localStorage.setItem(cooldownKey, String(Date.now()));
    }).catch(() => undefined).finally(() => notificationInFlight.current.delete(notificationEvent.id));
  }, []);

  const clearHistory = useCallback(() => {
    localStorage.removeItem(HISTORY_STORAGE_KEY);
    setHistory([]);
  }, []);

  const runRefresh = useCallback(async () => {
    if (fixtureMode) {
      setSnapshot(fixtureSnapshot());
      setError(undefined);
      setLoading(false);
      return;
    }
    if (!isTauri()) {
      setSnapshot(browserSnapshot());
      setError(undefined);
      setLoading(false);
      return;
    }
    setLoading(true);
    try {
      const result = await invoke<unknown>("codex_usage_snapshot");
      if (!isSnapshot(result)) throw new Error("本机用量响应格式无效");
      evaluateNotifications(result);
      setSnapshot(result);
      setError(undefined);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setLoading(false);
    }
  }, [evaluateNotifications, fixtureMode]);

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

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(), REFRESH_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, [refresh]);

  const value = useMemo<CodexUsageContextValue>(
    () => ({ snapshot, loading, error, history, refresh, clearHistory }),
    [clearHistory, error, history, loading, refresh, snapshot],
  );
  return <CodexUsageContext.Provider value={value}>{children}</CodexUsageContext.Provider>;
}

export function useCodexUsage(): CodexUsageContextValue {
  const value = useContext(CodexUsageContext);
  if (!value) throw new Error("useCodexUsage must be used inside CodexUsageProvider");
  return value;
}
