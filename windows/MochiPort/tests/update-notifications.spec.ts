import { expect, test } from "@playwright/test";
import { STARTUP_UPDATE_CHECK_DELAY_MS } from "../src/state/useUpdateNotifications";

interface UpdateNotificationTestState {
  updateInvocations: number;
  notifications: Array<{ title: string; body?: string; sound?: string }>;
}

async function installTauriUpdateMock(
  page: import("@playwright/test").Page,
  notifyUpdate: boolean,
  failFirstUpdate = false,
  releaseUrl = "https://github.com/mps233/mochiport/releases/tag/v0.5.4",
): Promise<void> {
  await page.clock.install({ time: new Date("2026-08-24T08:00:00Z") });
  await page.addInitScript(({ enabled, failFirst, releaseUrl: mockedReleaseUrl }) => {
    localStorage.setItem("mochiport.notify-update", enabled ? "on" : "off");
    localStorage.setItem("mochiport.notification-real-mode", "on");
    localStorage.setItem("mochiport.notification-sound", "on");
    localStorage.setItem("mochiport.notification-custom-messages", JSON.stringify({
      update: ["升级后的 {AGENT} 来了"],
    }));

    const state: UpdateNotificationTestState = {
      updateInvocations: 0,
      notifications: [],
    };
    const testWindow = window as typeof window & {
      __MOCHIPORT_UPDATE_TEST__: UpdateNotificationTestState;
      __TAURI_INTERNALS__: Record<string, unknown>;
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => void };
    };
    testWindow.__MOCHIPORT_UPDATE_TEST__ = state;

    class MockNotification {
      static permission = "granted";
      static requestPermission = async () => "granted";

      constructor(title: string, options?: NotificationOptions & { sound?: string }) {
        state.notifications.push({ title, body: options?.body, sound: options?.sound });
      }
    }
    Object.defineProperty(window, "Notification", { configurable: true, value: MockNotification });

    let callbackId = 0;
    let updateAttempts = 0;
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
        if (command === "check_for_updates") {
          state.updateInvocations += 1;
          updateAttempts += 1;
          if (failFirst && updateAttempts === 1) throw new Error("更新服务暂时不可用");
          return {
            currentVersion: "0.5.3",
            latestVersion: "vv0.5.4",
            updateAvailable: true,
            releaseUrl: mockedReleaseUrl,
          };
        }
        if (command === "plugin:window|is_maximized") return false;
        if (command === "plugin:event|listen") return 1;
        return null;
      },
    };
    testWindow.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => undefined };
  }, { enabled: notifyUpdate, failFirst: failFirstUpdate, releaseUrl });
}

test("startup waits 15 seconds then sends the customized update notification with sound", async ({ page }) => {
  expect(STARTUP_UPDATE_CHECK_DELAY_MS).toBe(15_000);
  await installTauriUpdateMock(page, true);
  await page.goto("/?fixture=1");

  await page.clock.fastForward(14_000);
  expect(await page.evaluate(() => (
    window as typeof window & { __MOCHIPORT_UPDATE_TEST__: UpdateNotificationTestState }
  ).__MOCHIPORT_UPDATE_TEST__)).toEqual({ updateInvocations: 0, notifications: [] });

  await page.clock.fastForward(1_000);
  await expect.poll(() => page.evaluate(() => (
    window as typeof window & { __MOCHIPORT_UPDATE_TEST__: UpdateNotificationTestState }
  ).__MOCHIPORT_UPDATE_TEST__.updateInvocations)).toBe(1);
  await expect.poll(() => page.evaluate(() => (
    window as typeof window & { __MOCHIPORT_UPDATE_TEST__: UpdateNotificationTestState }
  ).__MOCHIPORT_UPDATE_TEST__.notifications)).toEqual([{
    title: "升级后的 来了",
    body: "v0.5.4",
    sound: "Default",
  }]);
});

test("disabling update notifications still checks for updates without showing a toast", async ({ page }) => {
  await installTauriUpdateMock(page, false);
  await page.goto("/?fixture=1");

  await page.clock.fastForward(15_000);
  await expect.poll(() => page.evaluate(() => (
    window as typeof window & { __MOCHIPORT_UPDATE_TEST__: UpdateNotificationTestState }
  ).__MOCHIPORT_UPDATE_TEST__.updateInvocations)).toBe(1);
  expect(await page.evaluate(() => (
    window as typeof window & { __MOCHIPORT_UPDATE_TEST__: UpdateNotificationTestState }
  ).__MOCHIPORT_UPDATE_TEST__.notifications)).toEqual([]);
});

test("a startup result is shared with the overview banner even when notifications are disabled", async ({ page }) => {
  await installTauriUpdateMock(page, false);
  await page.goto("/?fixture=1");

  await page.clock.fastForward(15_000);
  await expect(page.getByRole("status").filter({ hasText: "MochiPort 有新版本" })).toBeVisible();
  expect(await page.evaluate(() => (
    window as typeof window & { __MOCHIPORT_UPDATE_TEST__: UpdateNotificationTestState }
  ).__MOCHIPORT_UPDATE_TEST__.notifications)).toEqual([]);
});

test("dismissing the overview update entry lasts for this session and settings uses the same result", async ({ page }) => {
  await installTauriUpdateMock(page, true);
  await page.goto("/?fixture=1");

  await page.clock.fastForward(15_000);
  const notice = page.getByRole("status").filter({ hasText: "MochiPort 有新版本" });
  await expect(notice).toBeVisible();
  await notice.getByRole("button", { name: "关闭更新提示" }).click();
  await expect(notice).toHaveCount(0);

  await page.getByRole("button", { name: "设置", exact: true }).click();
  await page.getByRole("button", { name: /更新与诊断/ }).click();
  await expect(page.getByRole("status").filter({ hasText: "发现 vv0.5.4" })).toBeVisible();
  await page.getByRole("button", { name: "概览", exact: true }).click();
  await expect(page.getByRole("status").filter({ hasText: "MochiPort 有新版本" })).toHaveCount(0);
});

test("a manual retry replaces a failed startup check in the shared state", async ({ page }) => {
  await installTauriUpdateMock(page, false, true);
  await page.goto("/?fixture=1");

  await page.clock.fastForward(15_000);
  await page.getByRole("button", { name: "设置", exact: true }).click();
  await page.getByRole("button", { name: /更新与诊断/ }).click();
  await expect(page.getByRole("status").filter({ hasText: "检查失败：更新服务暂时不可用" })).toBeVisible();

  await page.getByRole("button", { name: "检查更新" }).click();
  await expect(page.getByRole("status").filter({ hasText: "发现 vv0.5.4" })).toBeVisible();
  await page.getByRole("button", { name: "概览", exact: true }).click();
  await expect(page.getByRole("status").filter({ hasText: "MochiPort 有新版本" })).toBeVisible();
  expect(await page.evaluate(() => (
    window as typeof window & { __MOCHIPORT_UPDATE_TEST__: UpdateNotificationTestState }
  ).__MOCHIPORT_UPDATE_TEST__.updateInvocations)).toBe(2);
});

test("manual update checks in a non-Tauri fixture do not invoke native commands", async ({ page }) => {
  await page.addInitScript(() => {
    const testWindow = window as typeof window & { __MOCHIPORT_NATIVE_INVOKES__?: number };
    testWindow.__MOCHIPORT_NATIVE_INVOKES__ = 0;
    const original = testWindow.__TAURI_INTERNALS__;
    if (original) {
      const invoke = (original as { invoke?: unknown }).invoke;
      if (typeof invoke === "function") {
        (original as { invoke: (...args: unknown[]) => unknown }).invoke = (...args) => {
          testWindow.__MOCHIPORT_NATIVE_INVOKES__ = (testWindow.__MOCHIPORT_NATIVE_INVOKES__ ?? 0) + 1;
          return (invoke as (...innerArgs: unknown[]) => unknown)(...args);
        };
      }
    }
  });
  await page.goto("/?fixture=1");
  await page.getByRole("button", { name: "设置", exact: true }).click();
  await page.getByRole("button", { name: /更新与诊断/ }).click();
  await page.getByRole("button", { name: "检查更新" }).click();

  await expect(page.getByRole("status").filter({ hasText: "预览模式不检查更新" })).toBeVisible();
  expect(await page.evaluate(() => (window as typeof window & { __MOCHIPORT_NATIVE_INVOKES__?: number }).__MOCHIPORT_NATIVE_INVOKES__ ?? 0)).toBe(0);
});

test("an update response with an untrusted release URL is rejected", async ({ page }) => {
  await installTauriUpdateMock(page, false, false, "https://updates.example.invalid/mochiport");
  await page.goto("/?fixture=1");
  await page.getByRole("button", { name: "设置", exact: true }).click();
  await page.getByRole("button", { name: /更新与诊断/ }).click();
  await page.getByRole("button", { name: "检查更新" }).click();

  await expect(page.getByRole("status").filter({ hasText: "检查失败：更新检查响应格式无效" })).toBeVisible();
});

test("the overview update notice remains usable at a narrow Windows width", async ({ page }) => {
  await page.setViewportSize({ width: 540, height: 760 });
  await installTauriUpdateMock(page, false);
  await page.goto("/?fixture=1");
  await page.clock.fastForward(15_000);

  const notice = page.getByRole("status").filter({ hasText: "MochiPort 有新版本" });
  await expect(notice).toBeVisible();
  await expect(notice.getByRole("button", { name: "打开发布页" })).toBeVisible();
  await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth)).toBeLessThanOrEqual(1);
});
