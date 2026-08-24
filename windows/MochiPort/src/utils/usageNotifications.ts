import {
  resolveNotificationTitle,
  type CustomNotificationMessages,
  type NotificationMessageContext,
  type NotificationMessageKey,
} from "./notificationMessages";
import { evaluateUsageBriefing } from "./usageBriefings";

export interface UsageQuotaWindow {
  kind: "session5h" | "weekly" | string;
  usedPercent: number;
  resetsAtMs?: number | null;
  depletionEtaMs?: number | null;
}

export interface UsageNotificationSnapshot {
  updatedAtMs: number;
  tokensPerMinute: number;
  burnRateTokensPerMinute?: number;
  activeBaselineTokensPerMinute: number;
  quotaWindows: UsageQuotaWindow[];
  todayTokens?: number;
  estimatedCostUsd?: number;
  yesterdayTokens?: number;
  yesterdayCostUsd?: number;
  yesterdayTopProject?: string | null;
  dailyUsage?: Array<{ day: string; tokens: number }>;
  streakDays?: number;
  previousBestDailyTokens?: number | null;
  weeklyReport?: {
    lastWeekTokens: number;
    lastWeekCostUsd: number;
    previousWeekTokens: number;
    lastWeekTopProject?: string | null;
  } | null;
  lastActivityAtMs?: number | null;
}

export interface UsageNotificationSettings {
  warnThreshold: number;
  criticalThreshold: number;
  notifyLimitThreshold: boolean;
  notifyDepletion: boolean;
  notifyWindowReset: boolean;
  notifyBurnSpike: boolean;
  notifyComeback?: boolean;
  notifyBriefing?: boolean;
  includeStreak?: boolean;
  includeWeeklyReport?: boolean;
  notifyMilestone?: boolean;
  notifyRecord?: boolean;
  realMode?: boolean;
  customMessages?: CustomNotificationMessages;
}

export interface UsageNotificationEvent {
  id: string;
  kind: "limit" | "depletion" | "reset" | "burnSpike" | "comeback" | "briefing" | "milestone" | "record";
  title: string;
  body: string;
  cooldownMs: number;
}

const MILESTONE_THRESHOLDS_DESCENDING = [
  5_000_000_000,
  2_000_000_000,
  1_000_000_000,
  500_000_000,
  250_000_000,
  100_000_000,
] as const;
const WEEKLY_DEPLETION_RATIO = 0.6;
const WEEKLY_DEPLETION_MIN_RESET_LEAD_MS = 24 * 60 * 60_000;

const quotaLabel = (kind: string) => kind === "session5h" ? "5 小时" : kind === "weekly" ? "每周" : kind;

function notificationTitle(
  key: NotificationMessageKey,
  defaultTitle: string,
  settings: UsageNotificationSettings,
  context: NotificationMessageContext = {},
): string {
  return resolveNotificationTitle(key, defaultTitle, {
    realMode: settings.realMode ?? false,
    customMessages: settings.customMessages ?? {},
  }, context);
}

function localDayKey(milliseconds: number): string {
  const date = new Date(milliseconds);
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function countdown(milliseconds: number): string {
  const minutes = Math.max(0, Math.ceil(milliseconds / 60_000));
  if (minutes >= 24 * 60) return `${Math.ceil(minutes / (24 * 60))} 天`;
  if (minutes >= 60) return `${Math.floor(minutes / 60)} 小时 ${minutes % 60} 分钟`;
  return `${minutes} 分钟`;
}

export function evaluateUsageNotifications(
  current: UsageNotificationSnapshot,
  previous: UsageNotificationSnapshot | undefined,
  settings: UsageNotificationSettings,
  nowMs = Date.now(),
): UsageNotificationEvent[] {
  const events: UsageNotificationEvent[] = [];
  const previousByKind = new Map(previous?.quotaWindows.map((window) => [window.kind, window]) ?? []);
  const thresholds = [settings.criticalThreshold, settings.warnThreshold]
    .filter((value, index, values) => Number.isFinite(value) && values.indexOf(value) === index)
    .sort((left, right) => right - left);

  if (settings.notifyLimitThreshold) {
    for (const window of current.quotaWindows) {
      const before = previousByKind.get(window.kind)?.usedPercent ?? 0;
      const crossed = thresholds.find((threshold) => before < threshold && window.usedPercent >= threshold);
      if (crossed != null) {
        const reset = window.resetsAtMs && window.resetsAtMs > nowMs
          ? countdown(window.resetsAtMs - nowMs)
          : undefined;
        events.push({
          id: `limit:${window.kind}:${crossed}`,
          kind: "limit",
          title: notificationTitle("limitThreshold", "Codex 额度接近上限", settings, {
            agent: "Codex",
            usage: window.usedPercent,
            reset,
          }),
          body: `${quotaLabel(window.kind)}窗口已使用 ${Math.round(window.usedPercent)}%${reset ? ` · ${reset}后重置` : ""}`,
          cooldownMs: 30 * 60_000,
        });
      }
    }
  }

  if (settings.notifyDepletion) {
    for (const window of current.quotaWindows) {
      const etaAtMs = window.depletionEtaMs;
      if (etaAtMs == null
        || !Number.isFinite(etaAtMs)
        || etaAtMs <= nowMs
        || !window.resetsAtMs
        || etaAtMs >= window.resetsAtMs) continue;
      const etaMs = etaAtMs - nowMs;
      const resetLeadMs = window.resetsAtMs - nowMs;
      if (window.kind === "weekly"
        && (resetLeadMs < WEEKLY_DEPLETION_MIN_RESET_LEAD_MS
          || etaMs > resetLeadMs * WEEKLY_DEPLETION_RATIO)) continue;
      events.push({
        id: `depletion:${window.kind}`,
        kind: "depletion",
        title: notificationTitle("depletionRisk", "Codex 即将耗尽", settings, { agent: "Codex" }),
        body: window.kind === "weekly"
          ? etaMs <= 24 * 60 * 60_000
            ? "照这个趋势，今天内会耗尽每周额度"
            : `照这个趋势，约 ${Math.max(1, Math.ceil(etaMs / (24 * 60 * 60_000)))} 天后耗尽每周额度`
          : `照这个速度，${countdown(etaMs)}后耗尽${quotaLabel(window.kind)}额度`,
        cooldownMs: window.kind === "weekly" ? 12 * 60 * 60_000 : 30 * 60_000,
      });
    }
  }

  if (settings.notifyWindowReset && previous) {
    for (const window of current.quotaWindows) {
      const before = previousByKind.get(window.kind)?.usedPercent;
      if (before != null && before >= 30 && window.usedPercent < 10) {
        events.push({
          id: `reset:${window.kind}`,
          kind: "reset",
          title: notificationTitle("windowReset", "Codex 新额度窗口", settings, { agent: "Codex" }),
          body: `${quotaLabel(window.kind)}额度已重置`,
          cooldownMs: 30 * 60_000,
        });
      }
    }
  }

  if (settings.notifyBurnSpike
    && current.activeBaselineTokensPerMinute > 0
    && (current.burnRateTokensPerMinute ?? current.tokensPerMinute) > current.activeBaselineTokensPerMinute * 3) {
    const ratio = (current.burnRateTokensPerMinute ?? current.tokensPerMinute)
      / current.activeBaselineTokensPerMinute;
    events.push({
      id: "burn-spike",
      kind: "burnSpike",
      title: notificationTitle("burnSpike", "Token 使用量突增", settings),
      body: `正在以平时 ${ratio.toFixed(1)} 倍的速度消耗`,
      cooldownMs: 30 * 60_000,
    });
  }

  if (settings.notifyComeback && previous?.lastActivityAtMs && current.lastActivityAtMs
    && current.lastActivityAtMs > previous.lastActivityAtMs
    && current.lastActivityAtMs - previous.lastActivityAtMs >= 3 * 60 * 60_000) {
    events.push({
      id: `comeback:${localDayKey(current.lastActivityAtMs)}`,
      kind: "comeback",
      title: notificationTitle("comeback", "欢迎回来", settings),
      body: `间隔 ${countdown(current.lastActivityAtMs - previous.lastActivityAtMs)}后继续工作`,
      cooldownMs: 3 * 60 * 60_000,
    });
  }

  if (settings.notifyMilestone && current.todayTokens != null) {
    const before = previous?.todayTokens ?? 0;
    const milestone = MILESTONE_THRESHOLDS_DESCENDING
      .find((value) => before < value && current.todayTokens! >= value);
    if (milestone != null) {
      events.push({
        id: `milestone:${localDayKey(current.updatedAtMs)}:${milestone}`,
        kind: "milestone",
        title: notificationTitle("milestone", "里程碑达成", settings, { tokens: milestone }),
        body: `今日累计 ${milestone.toLocaleString()} Token`,
        cooldownMs: 24 * 60 * 60_000,
      });
    }
  }

  if (settings.notifyRecord
    && current.todayTokens != null
    && current.previousBestDailyTokens != null) {
    const today = localDayKey(current.updatedAtMs);
    const previousRecord = current.previousBestDailyTokens;
    if (previousRecord > 0
      && current.todayTokens > previousRecord
      && (previous?.todayTokens ?? 0) <= previousRecord) {
      events.push({
        id: `record:${today}`,
        kind: "record",
        title: notificationTitle("record", "今日创下新纪录", settings, { tokens: current.todayTokens }),
        body: `超过此前 ${previousRecord.toLocaleString()} Token`,
        cooldownMs: 24 * 60 * 60_000,
      });
    }
  }

  if (settings.notifyBriefing && current.todayTokens != null) {
    const briefing = evaluateUsageBriefing(current, {
      notifyBriefing: settings.notifyBriefing,
      includeStreak: settings.includeStreak,
      includeWeeklyReport: settings.includeWeeklyReport,
    }, nowMs);
    if (briefing) {
      const messageKey = briefing.period === "morning"
        ? "briefingMorning"
        : briefing.period === "lunch"
          ? "briefingLunch"
          : "briefingEvening";
      events.push({
        id: briefing.id,
        kind: "briefing",
        title: briefing.fixedTitle
          ? briefing.defaultTitle
          : notificationTitle(messageKey, briefing.defaultTitle, settings, {
            tokens: briefing.period === "morning" ? current.yesterdayTokens : current.todayTokens,
          }),
        body: briefing.body,
        cooldownMs: 20 * 60 * 60_000,
      });
    }
  }

  return events;
}
