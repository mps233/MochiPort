export interface UsageWeeklyReport {
  lastWeekTokens: number;
  lastWeekCostUsd: number;
  previousWeekTokens: number;
  lastWeekTopProject?: string | null;
}

export interface UsageBriefingSnapshot {
  todayTokens?: number;
  estimatedCostUsd?: number;
  yesterdayTokens?: number;
  yesterdayCostUsd?: number;
  yesterdayTopProject?: string | null;
  streakDays?: number;
  weeklyReport?: UsageWeeklyReport | null;
}

export interface UsageBriefingSettings {
  notifyBriefing?: boolean;
  includeStreak?: boolean;
  includeWeeklyReport?: boolean;
}

export type UsageBriefingPeriod = "morning" | "lunch" | "evening";

export interface UsageBriefing {
  id: string;
  period: UsageBriefingPeriod;
  defaultTitle: string;
  body: string;
  fixedTitle: boolean;
}

function localDayKey(milliseconds: number): string {
  const date = new Date(milliseconds);
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function briefingPeriod(hour: number): UsageBriefingPeriod | undefined {
  if (hour >= 8 && hour < 11) return "morning";
  if (hour >= 12 && hour < 14) return "lunch";
  if (hour >= 18 && hour < 22) return "evening";
  return undefined;
}

function formatTokens(tokens: number): string {
  if (tokens >= 1_000_000) return `${(tokens / 1_000_000).toFixed(1)}M`;
  if (tokens >= 1_000) return `${(tokens / 1_000).toFixed(1)}K`;
  return String(tokens);
}

function formatCost(cost: number): string {
  return `$${cost.toFixed(2)}`;
}

function roundedPercent(value: number): number {
  return value < 0 ? -Math.round(-value) : Math.round(value);
}

function weeklyReportBody(report: UsageWeeklyReport): string {
  let body = `上周 ${formatTokens(report.lastWeekTokens)} (~$${report.lastWeekCostUsd.toFixed(2)})`;
  if (report.lastWeekTopProject) body += ` · 主要项目 ${report.lastWeekTopProject}`;
  if (report.previousWeekTokens > 0) {
    const delta = (report.lastWeekTokens - report.previousWeekTokens) / report.previousWeekTokens * 100;
    const rounded = roundedPercent(delta);
    body += ` · 较前一周 ${rounded >= 0 ? "+" : ""}${rounded}%`;
  }
  return body;
}

export function evaluateUsageBriefing(
  snapshot: UsageBriefingSnapshot,
  settings: UsageBriefingSettings,
  nowMs = Date.now(),
): UsageBriefing | undefined {
  if (!settings.notifyBriefing) return undefined;
  const now = new Date(nowMs);
  const period = briefingPeriod(now.getHours());
  if (!period) return undefined;
  const id = `briefing:${localDayKey(nowMs)}:${period}`;

  if (period === "morning"
    && now.getDay() === 1
    && settings.includeWeeklyReport !== false
    && snapshot.weeklyReport
    && snapshot.weeklyReport.lastWeekTokens > 0) {
    return {
      id,
      period,
      defaultTitle: "每周报告 📊",
      body: weeklyReportBody(snapshot.weeklyReport),
      fixedTitle: true,
    };
  }

  if (period === "morning") {
    const yesterdayTokens = snapshot.yesterdayTokens ?? 0;
    const yesterdayCost = snapshot.yesterdayCostUsd ?? 0;
    if (yesterdayTokens <= 0 && yesterdayCost <= 0) return undefined;
    let body = `昨日：${formatTokens(yesterdayTokens)} tokens · 约 ${formatCost(yesterdayCost)}`;
    if (snapshot.yesterdayTopProject) body += ` · 主要项目 ${snapshot.yesterdayTopProject}`;
    if (settings.includeStreak !== false && (snapshot.streakDays ?? 0) >= 2) {
      body += ` · 连续使用 ${snapshot.streakDays} 天 🔥`;
    }
    return { id, period, defaultTitle: "昨日使用摘要", body, fixedTitle: false };
  }

  const todayTokens = snapshot.todayTokens ?? 0;
  const todayCost = snapshot.estimatedCostUsd ?? 0;
  if (todayTokens <= 0 && todayCost <= 0) return undefined;

  if (period === "lunch") {
    const midnight = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
    const elapsedMs = nowMs - midnight;
    if (elapsedMs < 4 * 60 * 60_000) return undefined;
    const dayFraction = elapsedMs / (24 * 60 * 60_000);
    const projectedTokens = Math.trunc(todayTokens / dayFraction);
    const projectedCost = todayCost / dayFraction;
    return {
      id,
      period,
      defaultTitle: "今日进度",
      body: `照这个进度，午夜前约 ${formatTokens(projectedTokens)}（约 ${formatCost(projectedCost)}）`,
      fixedTitle: false,
    };
  }

  let body = `今日：${formatTokens(todayTokens)} tokens · 约 ${formatCost(todayCost)}`;
  if (todayTokens > 0) body += " · Codex 占比 100%";
  return { id, period, defaultTitle: "今日使用总结", body, fixedTitle: false };
}
