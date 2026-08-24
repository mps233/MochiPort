import { expect, test } from "@playwright/test";
import { fixtureAccounts, fixtureDashboard } from "../src/api/fixtures";

interface DaemonRecoveryTestState {
  startInvocations: number;
  healthInvocations: number;
  ready: boolean;
}

test("a failed automatic daemon launch keeps an explicit recovery path", async ({ page }) => {
  await page.addInitScript(({ dashboard, accounts }) => {
    const state: DaemonRecoveryTestState = {
      startInvocations: 0,
      healthInvocations: 0,
      ready: false,
    };
    const testWindow = window as typeof window & {
      __MOCHIPORT_DAEMON_RECOVERY_TEST__: DaemonRecoveryTestState;
      __TAURI_INTERNALS__: Record<string, unknown>;
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => void };
    };
    testWindow.__MOCHIPORT_DAEMON_RECOVERY_TEST__ = state;

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
      async invoke(command: string, args?: Record<string, unknown>) {
        if (command === "start_daemon") {
          state.startInvocations += 1;
          if (state.startInvocations === 1) throw new Error("首次自动启动失败");
          state.ready = true;
          return { started: true, executable: "C:\\Program Files\\MochiPort\\mochiport-daemon.exe", message: "后台服务启动中" };
        }
        if (command === "management_request") {
          const path = String(args?.path ?? "");
          if (path === "healthz") {
            state.healthInvocations += 1;
            if (!state.ready) throw new Error("connection refused");
            return { status: 200, body: JSON.stringify({ service: "threadrelay", apiMajor: 1, ready: true }) };
          }
          if (!state.ready) throw new Error("connection refused");
          if (path === "api/v1/manage/dashboard") return { status: 200, body: JSON.stringify(dashboard) };
          if (path === "api/v1/manage/im/accounts") return { status: 200, body: JSON.stringify({ accounts }) };
          if (path === "api/v1/manage/lifecycle") return { status: 404, body: JSON.stringify({ error: "not available" }) };
          if (path === "api/v1/manage/gateway/sub2api") {
            return { status: 200, body: JSON.stringify({ configured: false, baseUrl: "", secretSet: false }) };
          }
          return { status: 404, body: JSON.stringify({ error: `未处理的测试请求：${path}` }) };
        }
        if (command === "codex_usage_snapshot") {
          return {
            available: false,
            sourceDirectory: "C:\\Users\\Test\\.codex\\sessions",
            scannedFiles: 0,
            todayTokens: 0,
            todayRequests: 0,
            tokensPerMinute: 0,
            burnRateTokensPerMinute: 0,
            activeBaselineTokensPerMinute: 0,
            estimatedCostUsd: 0,
            quotaWindows: [],
            sevenDay: [],
            updatedAtMs: Date.now(),
          };
        }
        if (command === "plugin:window|is_maximized") return false;
        if (command === "plugin:event|listen") return 1;
        return null;
      },
    };
    testWindow.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => undefined };
  }, { dashboard: fixtureDashboard, accounts: fixtureAccounts });

  await page.goto("/");

  const recoverButton = page.getByRole("button", { name: "启动本地服务" });
  await expect(recoverButton).toBeVisible({ timeout: 10_000 });
  await expect.poll(() => page.evaluate(() => (
    window as typeof window & { __MOCHIPORT_DAEMON_RECOVERY_TEST__: DaemonRecoveryTestState }
  ).__MOCHIPORT_DAEMON_RECOVERY_TEST__.startInvocations)).toBe(1);

  await recoverButton.click();

  await expect(page.getByText("运行正常", { exact: true }).first()).toBeVisible();
  await expect(recoverButton).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => (
    window as typeof window & { __MOCHIPORT_DAEMON_RECOVERY_TEST__: DaemonRecoveryTestState }
  ).__MOCHIPORT_DAEMON_RECOVERY_TEST__.startInvocations)).toBe(2);
});
