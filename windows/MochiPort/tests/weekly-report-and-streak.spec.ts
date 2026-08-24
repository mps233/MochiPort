import { expect, test } from "@playwright/test";
import { evaluateUsageBriefing, type UsageBriefingSnapshot } from "../src/utils/usageBriefings";

const weeklySnapshot: UsageBriefingSnapshot = {
  todayTokens: 42_000,
  estimatedCostUsd: 0.42,
  yesterdayTokens: 36_000,
  yesterdayCostUsd: 0.36,
  yesterdayTopProject: "MochiPort",
  streakDays: 5,
  weeklyReport: {
    lastWeekTokens: 1_250_000,
    lastWeekCostUsd: 12.345,
    previousWeekTokens: 1_000_000,
    lastWeekTopProject: "MochiPort",
  },
};

test("Monday morning uses the Mac-compatible weekly report special edition", () => {
  const mondayMorning = new Date(2026, 7, 24, 8, 30).getTime();
  const briefing = evaluateUsageBriefing(weeklySnapshot, {
    notifyBriefing: true,
    includeStreak: true,
    includeWeeklyReport: true,
  }, mondayMorning);

  expect(briefing).toEqual({
    id: "briefing:2026-08-24:morning",
    period: "morning",
    defaultTitle: "每周报告 📊",
    body: "上周 1.3M (~$12.35) · 主要项目 MochiPort · 较前一周 +25%",
    fixedTitle: true,
  });
  expect(briefing?.body).not.toContain("连续使用");
});

test("weekly and streak content switches are independent under the briefing master switch", () => {
  const mondayMorning = new Date(2026, 7, 24, 10, 15).getTime();
  const regularMorning = evaluateUsageBriefing(weeklySnapshot, {
    notifyBriefing: true,
    includeStreak: true,
    includeWeeklyReport: false,
  }, mondayMorning);
  expect(regularMorning?.defaultTitle).toBe("昨日使用摘要");
  expect(regularMorning?.body).toBe("昨日：36.0K tokens · 约 $0.36 · 主要项目 MochiPort · 连续使用 5 天 🔥");

  const withoutStreak = evaluateUsageBriefing(weeklySnapshot, {
    notifyBriefing: true,
    includeStreak: false,
    includeWeeklyReport: false,
  }, mondayMorning);
  expect(withoutStreak?.body).toBe("昨日：36.0K tokens · 约 $0.36 · 主要项目 MochiPort");

  expect(evaluateUsageBriefing(weeklySnapshot, {
    notifyBriefing: false,
    includeStreak: true,
    includeWeeklyReport: true,
  }, mondayMorning)).toBeUndefined();
});

test("streak appears only in a regular morning summary at two days or more", () => {
  const tuesdayMorning = new Date(2026, 7, 25, 8, 0).getTime();
  const twoDay = evaluateUsageBriefing({ ...weeklySnapshot, streakDays: 2 }, {
    notifyBriefing: true,
    includeStreak: true,
    includeWeeklyReport: true,
  }, tuesdayMorning);
  expect(twoDay?.body).toContain("连续使用 2 天");

  const oneDay = evaluateUsageBriefing({ ...weeklySnapshot, streakDays: 1 }, {
    notifyBriefing: true,
    includeStreak: true,
    includeWeeklyReport: true,
  }, tuesdayMorning);
  expect(oneDay?.body).not.toContain("连续使用");

  const evening = evaluateUsageBriefing(weeklySnapshot, {
    notifyBriefing: true,
    includeStreak: true,
    includeWeeklyReport: true,
  }, new Date(2026, 7, 25, 21, 59).getTime());
  expect(evening?.period).toBe("evening");
  expect(evening?.body).not.toContain("连续使用");
  expect(evening?.defaultTitle).toBe("今日使用总结");
  expect(evening?.body).toBe("今日：42.0K tokens · 约 $0.42 · Codex 占比 100%");
});

test("ordinary briefings match the Mac morning, lunch, and zero-data rules", () => {
  const lunch = evaluateUsageBriefing(weeklySnapshot, {
    notifyBriefing: true,
  }, new Date(2026, 7, 25, 12, 0).getTime());
  expect(lunch).toMatchObject({
    period: "lunch",
    defaultTitle: "今日进度",
    body: "照这个进度，午夜前约 84.0K（约 $0.84）",
    fixedTitle: false,
  });

  expect(evaluateUsageBriefing({
    todayTokens: 0,
    estimatedCostUsd: 0,
    yesterdayTokens: 0,
    yesterdayCostUsd: 0,
  }, {
    notifyBriefing: true,
  }, new Date(2026, 7, 25, 8, 0).getTime())).toBeUndefined();
});

test("weekly report omits unavailable comparison and project fields", () => {
  const briefing = evaluateUsageBriefing({
    todayTokens: 10,
    streakDays: 1,
    weeklyReport: {
      lastWeekTokens: 900,
      lastWeekCostUsd: 0,
      previousWeekTokens: 0,
      lastWeekTopProject: null,
    },
  }, {
    notifyBriefing: true,
    includeWeeklyReport: true,
  }, new Date(2026, 7, 24, 8, 0).getTime());
  expect(briefing?.body).toBe("上周 900 (~$0.00)");
});

test("fixture settings persist the independent streak and weekly-report switches", async ({ page }) => {
  await page.addInitScript(() => {
    if (sessionStorage.getItem("mochiport.weekly-streak-test-reset") === "1") return;
    sessionStorage.setItem("mochiport.weekly-streak-test-reset", "1");
    localStorage.setItem("mochiport.section", "settings");
    localStorage.removeItem("mochiport.fun-streak");
    localStorage.removeItem("mochiport.fun-weekly-report");
  });
  await page.goto("/?fixture=1");
  await page.getByRole("button", { name: /使用量/ }).click();

  const streak = page.getByRole("switch", { name: "时段摘要包含连续使用天数" });
  const weekly = page.getByRole("switch", { name: "周一显示上周报告" });
  await expect(streak).toHaveAttribute("aria-checked", "true");
  await expect(weekly).toHaveAttribute("aria-checked", "true");
  await streak.click();
  await weekly.click();
  await page.getByRole("button", { name: "保存提醒设置" }).click();
  await expect(page.getByText("通知设置已保存", { exact: true })).toBeVisible();
  expect(await page.evaluate(() => ({
    streak: localStorage.getItem("mochiport.fun-streak"),
    weekly: localStorage.getItem("mochiport.fun-weekly-report"),
  }))).toEqual({ streak: "off", weekly: "off" });

  await page.reload();
  await page.getByRole("button", { name: /使用量/ }).click();
  await expect(page.getByRole("switch", { name: "时段摘要包含连续使用天数" })).toHaveAttribute("aria-checked", "false");
  await expect(page.getByRole("switch", { name: "周一显示上周报告" })).toHaveAttribute("aria-checked", "false");
});
