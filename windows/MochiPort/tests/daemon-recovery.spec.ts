import { expect, test } from "@playwright/test";
import { fixtureAccounts, fixtureDashboard, fixtureLifecycle } from "../src/api/fixtures";

interface DaemonRecoveryTestState {
  startInvocations: number;
  healthInvocations: number;
  ready: boolean;
}

interface SafeRestartRecoveryTestState extends DaemonRecoveryTestState {
  safeRestartInvocations: number;
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
            return { status: 200, body: JSON.stringify({ service: "mochiport", apiMajor: 1, ready: true }) };
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

test("a failed safe restart keeps ordinary refresh observation-only", async ({ page }) => {
  await page.addInitScript(({ dashboard, accounts, lifecycle }) => {
    const ownedLifecycle = {
      ...lifecycle,
      management: {
        ...lifecycle.management,
        leaseExpiresAtMs: Date.now() + 60_000,
      },
    };
    const state: SafeRestartRecoveryTestState = {
      startInvocations: 0,
      healthInvocations: 0,
      safeRestartInvocations: 0,
      ready: true,
    };
    const testWindow = window as typeof window & {
      __MOCHIPORT_SAFE_RESTART_RECOVERY_TEST__: SafeRestartRecoveryTestState;
      __TAURI_INTERNALS__: Record<string, unknown>;
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => void };
    };
    testWindow.__MOCHIPORT_SAFE_RESTART_RECOVERY_TEST__ = state;
    localStorage.setItem("mochiport.section", "settings");

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
          state.ready = true;
          return { started: true, executable: ownedLifecycle.executable, message: "后台服务启动中" };
        }
        if (command === "lifecycle_installation_id") return ownedLifecycle.management.installationId;
        if (command === "lifecycle_lease") return ownedLifecycle;
        if (command === "lifecycle_safe_restart") {
          state.safeRestartInvocations += 1;
          state.ready = false;
          throw new Error("同路径后台服务未恢复");
        }
        if (command === "management_request") {
          const path = String(args?.path ?? "");
          if (path === "healthz") {
            state.healthInvocations += 1;
            if (!state.ready) throw new Error("connection refused");
            return { status: 200, body: JSON.stringify({ service: "mochiport", apiMajor: 1, ready: true }) };
          }
          if (!state.ready) throw new Error("connection refused");
          if (path === "api/v1/manage/dashboard") return { status: 200, body: JSON.stringify(dashboard) };
          if (path === "api/v1/manage/im/accounts") return { status: 200, body: JSON.stringify({ accounts }) };
          if (path === "api/v1/manage/lifecycle") return { status: 200, body: JSON.stringify(ownedLifecycle) };
          if (path === "api/v1/manage/settings") {
            return {
              status: 200,
              body: JSON.stringify({
                language: "zh-CN",
                theme: "system",
                localConnectionMode: "standard",
                bind: "127.0.0.1:3847",
                outboundProxy: { mode: "system", url: "<none>", credentialSet: false },
              }),
            };
          }
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
  }, { dashboard: fixtureDashboard, accounts: fixtureAccounts, lifecycle: fixtureLifecycle });

  await page.goto("/");
  await page.getByRole("button", { name: "更新与诊断" }).click();
  await page.getByRole("button", { name: "安全重启后台服务" }).click();
  const confirmation = page.getByRole("dialog", { name: "重启后台服务？" });
  await confirmation.getByRole("button", { name: "确认安全重启" }).click();
  await expect(confirmation.getByRole("alert")).toContainText("同路径后台服务未恢复");
  await confirmation.getByRole("button", { name: "取消" }).click();

  await page.getByRole("button", { name: "概览", exact: true }).click();
  const refreshButton = page.locator(".page-header").getByRole("button", { name: "刷新", exact: true });
  const healthBeforeRefresh = await page.evaluate(() => (
    window as typeof window & { __MOCHIPORT_SAFE_RESTART_RECOVERY_TEST__: SafeRestartRecoveryTestState }
  ).__MOCHIPORT_SAFE_RESTART_RECOVERY_TEST__.healthInvocations);

  for (let attempt = 1; attempt <= 3; attempt += 1) {
    await refreshButton.click();
    await expect.poll(() => page.evaluate(() => (
      window as typeof window & { __MOCHIPORT_SAFE_RESTART_RECOVERY_TEST__: SafeRestartRecoveryTestState }
    ).__MOCHIPORT_SAFE_RESTART_RECOVERY_TEST__.healthInvocations)).toBeGreaterThanOrEqual(healthBeforeRefresh + attempt);
  }

  const finalState = await page.evaluate(() => (
    window as typeof window & { __MOCHIPORT_SAFE_RESTART_RECOVERY_TEST__: SafeRestartRecoveryTestState }
  ).__MOCHIPORT_SAFE_RESTART_RECOVERY_TEST__);
  expect(finalState.safeRestartInvocations).toBe(1);
  expect(finalState.startInvocations).toBe(0);
});
