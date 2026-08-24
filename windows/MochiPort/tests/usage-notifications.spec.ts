import { expect, test } from "@playwright/test";
import {
  evaluateUsageNotifications,
  type UsageNotificationSettings,
  type UsageNotificationSnapshot,
} from "../src/utils/usageNotifications";
import { resolveNotificationTitle } from "../src/utils/notificationMessages";

const settings: UsageNotificationSettings = {
  warnThreshold: 70,
  criticalThreshold: 90,
  notifyLimitThreshold: true,
  notifyDepletion: true,
  notifyWindowReset: true,
  notifyBurnSpike: true,
};

function snapshot(
  updatedAtMs: number,
  usedPercent: number,
  resetsAtMs = updatedAtMs + 4 * 60 * 60_000,
): UsageNotificationSnapshot {
  return {
    updatedAtMs,
    tokensPerMinute: 100,
    burnRateTokensPerMinute: 100,
    activeBaselineTokensPerMinute: 100,
    quotaWindows: [{ kind: "session5h", usedPercent, resetsAtMs }],
  };
}

test("quota notifications prioritize the highest crossed threshold", () => {
  const now = 1_700_000_000_000;
  const events = evaluateUsageNotifications(snapshot(now, 93), snapshot(now - 30_000, 68), settings, now);
  expect(events[0].id).toBe("limit:session5h:90");
  expect(events[0].body).toContain("93%");
  expect(events.filter((event) => event.kind === "limit")).toHaveLength(1);
});

test("quota notifications detect resets and depletion before reset", () => {
  const now = 1_700_000_000_000;
  const reset = evaluateUsageNotifications(snapshot(now, 4), snapshot(now - 30_000, 45), settings, now);
  expect(reset[0]?.id).toBe("reset:session5h");

  const current = snapshot(now, 52, now + 60 * 60_000);
  current.quotaWindows[0].depletionEtaMs = now + 20 * 60_000;
  const depletion = evaluateUsageNotifications(current, undefined, settings, now);
  expect(depletion[0]?.id).toBe("depletion:session5h");
});

test("burn spikes use the ten-minute burn rate and require a three-times active baseline", () => {
  const current = snapshot(1_700_000_000_000, 20);
  current.quotaWindows = [];
  current.tokensPerMinute = 9_000;
  current.burnRateTokensPerMinute = 3_010;
  current.activeBaselineTokensPerMinute = 1_000;
  expect(evaluateUsageNotifications(current, undefined, settings, current.updatedAtMs)[0]?.id).toBe("burn-spike");

  current.burnRateTokensPerMinute = 3_000;
  expect(evaluateUsageNotifications(current, undefined, settings, current.updatedAtMs)).toEqual([]);
});

test("depletion ignores one-refresh deltas and consumes only the Rust regression ETA", () => {
  const now = 1_700_000_000_000;
  const previous = snapshot(now - 30_000, 20, now + 60 * 60_000);
  const current = snapshot(now, 80, now + 60 * 60_000);
  expect(evaluateUsageNotifications(current, previous, settings, now)
    .filter((event) => event.kind === "depletion")).toEqual([]);

  current.quotaWindows[0].depletionEtaMs = now + 15 * 60_000;
  expect(evaluateUsageNotifications(current, previous, settings, now)
    .find((event) => event.kind === "depletion")).toEqual(
    expect.objectContaining({ id: "depletion:session5h" }),
  );
});

test("weekly depletion matches the Mac urgency ratio and minimum reset lead", () => {
  const now = 1_700_000_000_000;
  const weekly = snapshot(now, 72, now + 5 * 24 * 60 * 60_000);
  weekly.quotaWindows[0] = {
    kind: "weekly",
    usedPercent: 72,
    resetsAtMs: now + 5 * 24 * 60 * 60_000,
    depletionEtaMs: now + 2 * 24 * 60 * 60_000,
  };
  expect(evaluateUsageNotifications(weekly, undefined, settings, now)
    .find((event) => event.kind === "depletion")).toEqual(expect.objectContaining({
    id: "depletion:weekly",
    body: "照这个趋势，约 2 天后耗尽每周额度",
    cooldownMs: 12 * 60 * 60_000,
  }));

  weekly.quotaWindows[0].depletionEtaMs = now + 4 * 24 * 60 * 60_000;
  expect(evaluateUsageNotifications(weekly, undefined, settings, now)
    .filter((event) => event.kind === "depletion")).toEqual([]);

  weekly.quotaWindows[0].resetsAtMs = now + 12 * 60 * 60_000;
  weekly.quotaWindows[0].depletionEtaMs = now + 2 * 60 * 60_000;
  expect(evaluateUsageNotifications(weekly, undefined, settings, now)
    .filter((event) => event.kind === "depletion")).toEqual([]);
});

test("records compare against the durable all-time best instead of the bounded chart tail", () => {
  const now = new Date(2026, 7, 24, 15, 0).getTime();
  const previous = { ...snapshot(now - 30_000, 20), todayTokens: 79_000 };
  const current = {
    ...snapshot(now, 20),
    todayTokens: 80_000,
    previousBestDailyTokens: 90_000,
    dailyUsage: [{ day: "2026-08-23", tokens: 70_000 }],
  };
  const recordSettings = { ...settings, notifyRecord: true };
  expect(evaluateUsageNotifications(current, previous, recordSettings, now)
    .filter((event) => event.kind === "record")).toEqual([]);

  current.todayTokens = 90_001;
  expect(evaluateUsageNotifications(current, previous, recordSettings, now)
    .find((event) => event.kind === "record")).toEqual(expect.objectContaining({
    body: "超过此前 90,000 Token",
  }));
});

test("activity notifications cover comeback, milestone, record, and briefing events", () => {
  const now = new Date(2026, 7, 24, 9, 15).getTime();
  const activitySettings: UsageNotificationSettings = {
    ...settings,
    notifyComeback: true,
    notifyBriefing: true,
    notifyMilestone: true,
    notifyRecord: true,
  };

  const comebackCurrent = { ...snapshot(now, 20), lastActivityAtMs: now };
  const comebackPrevious = { ...snapshot(now - 30_000, 20), lastActivityAtMs: now - 4 * 60 * 60_000 };
  expect(evaluateUsageNotifications(comebackCurrent, comebackPrevious, activitySettings, now)[0]?.kind).toBe("comeback");

  const milestoneCurrent = { ...snapshot(now, 20), todayTokens: 100_000_200 };
  const milestonePrevious = { ...snapshot(now - 30_000, 20), todayTokens: 99_999_900 };
  expect(evaluateUsageNotifications(milestoneCurrent, milestonePrevious, activitySettings, now)[0]?.kind).toBe("milestone");

  const recordCurrent = {
    ...snapshot(now, 20),
    todayTokens: 80_000,
    previousBestDailyTokens: 75_000,
  };
  const recordPrevious = { ...snapshot(now - 30_000, 20), todayTokens: 74_000 };
  expect(evaluateUsageNotifications(recordCurrent, recordPrevious, { ...activitySettings, notifyMilestone: false }, now)[0]?.kind).toBe("record");

  const briefingCurrent = {
    ...snapshot(now, 20),
    todayTokens: 42_000,
    estimatedCostUsd: 0.42,
    yesterdayTokens: 36_000,
    yesterdayCostUsd: 0.36,
    yesterdayTopProject: "MochiPort",
  };
  expect(evaluateUsageNotifications(briefingCurrent, undefined, { ...activitySettings, notifyMilestone: false, notifyRecord: false }, now)[0]?.kind).toBe("briefing");
});

test("Mac milestone thresholds emit only the highest threshold crossed in one refresh", () => {
  const now = new Date(2026, 7, 24, 9, 15).getTime();
  const events = evaluateUsageNotifications(
    { ...snapshot(now, 20), todayTokens: 2_100_000_000 },
    { ...snapshot(now - 30_000, 20), todayTokens: 90_000_000 },
    { ...settings, notifyMilestone: true },
    now,
  );

  expect(events.filter((event) => event.kind === "milestone")).toEqual([
    expect.objectContaining({
      id: "milestone:2026-08-24:2000000000",
    }),
  ]);
});

test("recurring risk candidates keep one-shot activity and Monday briefing candidates eligible", () => {
  const now = new Date(2026, 7, 24, 9, 15).getTime();
  const resetAt = now + 60 * 60_000;
  const current: UsageNotificationSnapshot = {
    ...snapshot(now, 52, resetAt),
    tokensPerMinute: 401,
    burnRateTokensPerMinute: 401,
    activeBaselineTokensPerMinute: 100,
    todayTokens: 5_100_000_000,
    estimatedCostUsd: 1.2,
    yesterdayTokens: 75_000_000,
    yesterdayCostUsd: 0.9,
    lastActivityAtMs: now,
    dailyUsage: [
      { day: "2026-08-23", tokens: 4_500_000_000 },
      { day: "2026-08-24", tokens: 5_100_000_000 },
    ],
    previousBestDailyTokens: 4_500_000_000,
    weeklyReport: {
      lastWeekTokens: 700_000_000,
      lastWeekCostUsd: 8.4,
      previousWeekTokens: 650_000_000,
      lastWeekTopProject: "MochiPort",
    },
  };
  const previous: UsageNotificationSnapshot = {
    ...snapshot(now - 60_000, 50, resetAt),
    todayTokens: 90_000_000,
    lastActivityAtMs: now - 4 * 60 * 60_000,
  };
  current.quotaWindows[0].depletionEtaMs = now + 20 * 60_000;
  const events = evaluateUsageNotifications(current, previous, {
    ...settings,
    notifyComeback: true,
    notifyBriefing: true,
    includeWeeklyReport: true,
    notifyMilestone: true,
    notifyRecord: true,
  }, now);

  expect(events.map((event) => event.kind)).toEqual([
    "depletion",
    "burnSpike",
    "comeback",
    "milestone",
    "record",
    "briefing",
  ]);
  expect(events.find((event) => event.kind === "milestone")?.id).toBe("milestone:2026-08-24:5000000000");
  expect(events.find((event) => event.kind === "briefing")).toEqual(expect.objectContaining({
    title: "每周报告 📊",
    body: expect.stringContaining("上周 700.0M"),
  }));
});

test("custom notification titles override REAL Mode and substitute every supported placeholder", () => {
  const title = resolveNotificationTitle(
    "limitThreshold",
    "Codex 额度接近上限",
    {
      realMode: true,
      customMessages: {
        limitThreshold: ["{AGENT} 已用 {USAGE}，{RESET} 后重置，共 {TOKENS}"],
      },
    },
    { agent: "Codex", usage: 92.4, reset: "18 分钟", tokens: 1_250_000 },
    () => 0.75,
  );
  expect(title).toBe("Codex 已用 92%，18 分钟 后重置，共 1M");
});

test("REAL Mode changes only the event title and preserves the informative body", () => {
  const now = 1_700_000_000_000;
  const event = evaluateUsageNotifications(
    snapshot(now, 93),
    snapshot(now - 30_000, 68),
    { ...settings, realMode: true },
    now,
  )[0];
  expect(event.title).not.toBe("Codex 额度接近上限");
  expect(event.body).toBe("5 小时窗口已使用 93% · 4 小时 0 分钟后重置");
});

test("one app-level coordinator keeps evaluating while Overview is unmounted and the window is hidden", async ({ page }) => {
  await page.clock.install({ time: new Date("2026-08-24T08:00:00Z") });
  await page.addInitScript(() => {
    localStorage.setItem("mochiport.notifications", "on");
    localStorage.setItem("mochiport.warn-threshold", "70");
    localStorage.setItem("mochiport.critical-threshold", "90");
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      get: () => "hidden",
    });

    const testState = {
      usageInvocations: 0,
      notifications: [] as Array<{ title: string; body?: string; sound?: string }>,
    };
    const testWindow = window as typeof window & {
      __MOCHIPORT_USAGE_TEST__: typeof testState;
      __TAURI_INTERNALS__: Record<string, unknown>;
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => void };
    };
    testWindow.__MOCHIPORT_USAGE_TEST__ = testState;

    class MockNotification {
      static permission = "granted";
      static requestPermission = async () => "granted";

      constructor(title: string, options?: NotificationOptions & { sound?: string }) {
        testState.notifications.push({ title, body: options?.body, sound: options?.sound });
      }
    }
    Object.defineProperty(window, "Notification", { configurable: true, value: MockNotification });

    let callbackId = 0;
    const callbacks = new Map<number, (payload: unknown) => void>();
    testWindow.__TAURI_INTERNALS__ = {
      callbacks,
      metadata: {
        currentWindow: { label: "main" },
        currentWebview: { label: "main", windowLabel: "main" },
      },
      transformCallback(callback: (payload: unknown) => void) {
        callbackId += 1;
        callbacks.set(callbackId, callback);
        return callbackId;
      },
      unregisterCallback(id: number) {
        callbacks.delete(id);
      },
      async invoke(command: string) {
        if (command === "codex_usage_snapshot") {
          testState.usageInvocations += 1;
          const usedPercent = testState.usageInvocations % 2 === 1 ? 68 : 93;
          const now = Date.now();
          return {
            available: true,
            sourceDirectory: "C:\\Users\\Mia\\.codex\\sessions",
            scannedFiles: testState.usageInvocations,
            todayTokens: testState.usageInvocations * 1_000,
            todayRequests: testState.usageInvocations,
            tokensPerMinute: 100,
            burnRateTokensPerMinute: 100,
            activeBaselineTokensPerMinute: 100,
            estimatedCostUsd: 0.01,
            quotaWindows: [{
              kind: "session5h",
              usedPercent,
              resetsAtMs: now + 4 * 60 * 60_000,
              depletionEtaMs: testState.usageInvocations >= 4 ? now + 60_000 : null,
            }],
            sevenDay: [],
            updatedAtMs: now,
          };
        }
        if (command === "management_request") {
          return { status: 503, body: JSON.stringify({ error: "offline test fixture" }) };
        }
        if (command === "plugin:window|is_maximized") return false;
        if (command === "plugin:event|listen") return 1;
        return null;
      },
    };
    testWindow.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => undefined };
  });

  await page.goto("/");
  await expect.poll(() => page.evaluate(() => (
    window as typeof window & { __MOCHIPORT_USAGE_TEST__: { usageInvocations: number } }
  ).__MOCHIPORT_USAGE_TEST__.usageInvocations)).toBe(1);

  await page.getByRole("button", { name: "Codex 接入", exact: true }).click();
  await expect(page.locator(".usage-insights")).toHaveCount(0);

  await page.clock.fastForward(30_000);
  await expect.poll(() => page.evaluate(() => (
    window as typeof window & { __MOCHIPORT_USAGE_TEST__: { usageInvocations: number } }
  ).__MOCHIPORT_USAGE_TEST__.usageInvocations)).toBe(2);
  await expect.poll(() => page.evaluate(() => (
    window as typeof window & { __MOCHIPORT_USAGE_TEST__: { notifications: unknown[] } }
  ).__MOCHIPORT_USAGE_TEST__.notifications.length)).toBe(1);

  await page.clock.fastForward(30_000);
  await page.clock.fastForward(30_000);
  const backgroundState = await page.evaluate(() => (
    window as typeof window & {
      __MOCHIPORT_USAGE_TEST__: {
        usageInvocations: number;
        notifications: Array<{ title: string; body?: string; sound?: string }>;
      };
    }
  ).__MOCHIPORT_USAGE_TEST__);
  expect(backgroundState.usageInvocations).toBe(4);
  expect(backgroundState.notifications).toEqual([
    {
      title: "Codex 额度接近上限",
      body: "5 小时窗口已使用 93% · 4 小时 0 分钟后重置",
      sound: undefined,
    },
    {
      title: "Codex 即将耗尽",
      body: "照这个速度，1 分钟后耗尽5 小时额度",
      sound: undefined,
    },
  ]);

  await page.getByRole("button", { name: "概览", exact: true }).click();
  await expect(page.getByRole("progressbar", { name: "5 小时额度" })).toHaveAttribute("aria-valuenow", "93");
  await expect(page.locator(".usage-history")).toContainText("Codex 额度接近上限");
  await expect(page.locator(".usage-history")).toContainText("Codex 即将耗尽");
  expect(await page.evaluate(() => JSON.parse(localStorage.getItem("mochiport.usage-event-history") ?? "[]"))).toHaveLength(2);
  expect(await page.evaluate(() => (
    window as typeof window & { __MOCHIPORT_USAGE_TEST__: { usageInvocations: number } }
  ).__MOCHIPORT_USAGE_TEST__.usageInvocations)).toBe(4);
});

test("risk cooldowns yield the single notification slot to a Monday weekly report", async ({ page }) => {
  await page.clock.install({ time: new Date(2026, 7, 24, 9, 15) });
  await page.addInitScript(() => {
    localStorage.setItem("mochiport.notifications", "on");
    localStorage.setItem("mochiport.notify-briefing", "on");
    localStorage.setItem("mochiport.fun-weekly-report", "on");
    for (const prefix of ["mochiport.event-cooldown", "mochiport.notification-cooldown"]) {
      localStorage.setItem(`${prefix}.burn-spike`, String(Date.now()));
      localStorage.setItem(`${prefix}.depletion:session5h`, String(Date.now()));
    }

    const testState = {
      usageInvocations: 0,
      notifications: [] as Array<{ title: string; body?: string }>,
    };
    const testWindow = window as typeof window & {
      __MOCHIPORT_USAGE_COOLDOWN_TEST__: typeof testState;
      __TAURI_INTERNALS__: Record<string, unknown>;
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => void };
    };
    testWindow.__MOCHIPORT_USAGE_COOLDOWN_TEST__ = testState;

    class MockNotification {
      static permission = "granted";
      static requestPermission = async () => "granted";

      constructor(title: string, options?: NotificationOptions) {
        testState.notifications.push({ title, body: options?.body });
      }
    }
    Object.defineProperty(window, "Notification", { configurable: true, value: MockNotification });

    let callbackId = 0;
    const callbacks = new Map<number, (payload: unknown) => void>();
    testWindow.__TAURI_INTERNALS__ = {
      callbacks,
      metadata: {
        currentWindow: { label: "main" },
        currentWebview: { label: "main", windowLabel: "main" },
      },
      transformCallback(callback: (payload: unknown) => void) {
        callbackId += 1;
        callbacks.set(callbackId, callback);
        return callbackId;
      },
      unregisterCallback(id: number) {
        callbacks.delete(id);
      },
      async invoke(command: string) {
        if (command === "codex_usage_snapshot") {
          testState.usageInvocations += 1;
          const now = Date.now();
          const report = testState.usageInvocations >= 2
            ? {
                lastWeekTokens: 700_000_000,
                lastWeekCostUsd: 8.4,
                previousWeekTokens: 650_000_000,
                lastWeekTopProject: "MochiPort",
              }
            : null;
          return {
            available: true,
            sourceDirectory: "C:\\Users\\Mia\\.codex\\sessions",
            scannedFiles: testState.usageInvocations,
            todayTokens: testState.usageInvocations * 1_000,
            todayRequests: testState.usageInvocations,
            yesterdayTokens: 0,
            yesterdayCostUsd: 0,
            yesterdayTopProject: null,
            tokensPerMinute: 401,
            burnRateTokensPerMinute: 401,
            activeBaselineTokensPerMinute: 100,
            estimatedCostUsd: 0.01,
            quotaWindows: [{
              kind: "session5h",
              usedPercent: testState.usageInvocations === 1 ? 50 : 52,
              resetsAtMs: now + 60 * 60_000,
              depletionEtaMs: now + 30 * 60_000,
            }],
            sevenDay: [],
            dailyUsage: [],
            streakDays: 0,
            weeklyReport: report,
            updatedAtMs: now,
          };
        }
        if (command === "management_request") {
          return { status: 503, body: JSON.stringify({ error: "offline test fixture" }) };
        }
        if (command === "plugin:window|is_maximized") return false;
        if (command === "plugin:event|listen") return 1;
        return null;
      },
    };
    testWindow.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => undefined };
  });

  await page.goto("/");
  await expect.poll(() => page.evaluate(() => (
    window as typeof window & { __MOCHIPORT_USAGE_COOLDOWN_TEST__: { usageInvocations: number } }
  ).__MOCHIPORT_USAGE_COOLDOWN_TEST__.usageInvocations)).toBe(1);
  await expect.poll(() => page.evaluate(() => (
    window as typeof window & { __MOCHIPORT_USAGE_COOLDOWN_TEST__: { notifications: unknown[] } }
  ).__MOCHIPORT_USAGE_COOLDOWN_TEST__.notifications.length)).toBe(0);

  await page.clock.fastForward(30_000);
  await expect.poll(() => page.evaluate(() => (
    window as typeof window & { __MOCHIPORT_USAGE_COOLDOWN_TEST__: { usageInvocations: number } }
  ).__MOCHIPORT_USAGE_COOLDOWN_TEST__.usageInvocations)).toBe(2);
  const state = await page.evaluate(() => (
    window as typeof window & {
      __MOCHIPORT_USAGE_COOLDOWN_TEST__: {
        notifications: Array<{ title: string; body?: string }>;
      };
    }
  ).__MOCHIPORT_USAGE_COOLDOWN_TEST__);
  expect(state.notifications).toEqual([{
    title: "每周报告 📊",
    body: "上周 700.0M (~$8.40) · 主要项目 MochiPort · 较前一周 +8%",
  }]);
  expect(await page.evaluate(() => (
    JSON.parse(localStorage.getItem("mochiport.usage-event-history") ?? "[]") as Array<{ kind: string }>
  ).map((event) => event.kind))).toEqual(["briefing"]);
});
