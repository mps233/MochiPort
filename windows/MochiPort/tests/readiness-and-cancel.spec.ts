import { expect, test, type Page, type Route } from "@playwright/test";
import {
  fixtureAccounts,
  fixtureCodexStatus,
  fixtureDashboard,
  fixtureGateway,
  fixtureLifecycle,
  fixtureSessions,
} from "../src/api/fixtures";
import type { CodexEnhancedOperation } from "../src/api/types";

const managementOrigin = "http://127.0.0.1:3847";
const corsHeaders = {
  "access-control-allow-origin": "*",
  "access-control-allow-methods": "GET, POST, OPTIONS",
  "access-control-allow-headers": "content-type",
  "content-type": "application/json",
};

async function fulfillJson(route: Route, body: unknown, status = 200) {
  await route.fulfill({ status, headers: corsHeaders, body: JSON.stringify(body) });
}

async function handlePreflight(route: Route): Promise<boolean> {
  if (route.request().method() !== "OPTIONS") return false;
  await route.fulfill({ status: 204, headers: corsHeaders });
  return true;
}

async function setInitialSection(page: Page, section: "codex" | "sessions") {
  await page.addInitScript((nextSection) => {
    localStorage.setItem("mochiport.section", nextSection);
  }, section);
}

async function fulfillReadyBaseRead(route: Route, path: string): Promise<boolean> {
  if (path === "healthz") {
    await fulfillJson(route, { service: "mochiport", apiMajor: 1, ready: true });
    return true;
  }
  if (path === "api/v1/manage/dashboard") {
    await fulfillJson(route, fixtureDashboard);
    return true;
  }
  if (path === "api/v1/manage/im/accounts") {
    await fulfillJson(route, { accounts: fixtureAccounts });
    return true;
  }
  if (path === "api/v1/manage/lifecycle") {
    await fulfillJson(route, fixtureLifecycle);
    return true;
  }
  if (path === "api/v1/manage/codex/status") {
    await fulfillJson(route, fixtureCodexStatus);
    return true;
  }
  if (path === "api/v1/manage/gateway") {
    await fulfillJson(route, fixtureGateway);
    return true;
  }
  return false;
}

test("an initially selected section loads after daemon readiness polling", async ({ page }) => {
  let healthCalls = 0;
  let daemonReady = false;
  let sessionsBeforeReady = 0;
  let sessionsAfterReady = 0;

  await setInitialSection(page, "sessions");
  await page.route(`${managementOrigin}/**`, async (route) => {
    if (await handlePreflight(route)) return;

    const path = new URL(route.request().url()).pathname.replace(/^\//, "");
    if (path === "healthz") {
      healthCalls += 1;
      if (healthCalls === 1) {
        await route.abort("connectionrefused");
        return;
      }
      if (healthCalls < 4) {
        await fulfillJson(route, { service: "mochiport", apiMajor: 1, ready: false });
        return;
      }
      daemonReady = true;
      await fulfillJson(route, { service: "mochiport", apiMajor: 1, ready: true });
      return;
    }
    if (path === "api/v1/manage/dashboard") {
      await fulfillJson(route, fixtureDashboard);
      return;
    }
    if (path === "api/v1/manage/im/accounts") {
      await fulfillJson(route, { accounts: fixtureAccounts });
      return;
    }
    if (path === "api/v1/manage/lifecycle") {
      await fulfillJson(route, fixtureLifecycle);
      return;
    }
    if (path === "api/v1/manage/sessions") {
      if (!daemonReady) {
        sessionsBeforeReady += 1;
        await fulfillJson(route, { error: "后台服务仍在启动" }, 503);
        return;
      }
      sessionsAfterReady += 1;
      await fulfillJson(route, {
        ok: true,
        threads: fixtureSessions,
        providers: ["ai-gateway", "openai"],
      });
      return;
    }

    await fulfillJson(route, { error: `未处理的测试请求：${path}` }, 404);
  });

  await page.goto("/");
  await expect(page.getByRole("heading", { name: "会话", exact: true, level: 1 })).toBeVisible();
  await expect(page.getByText("Windows 客户端界面复刻", { exact: true })).toBeVisible();
  await expect(page.getByText("已连接 · 共 3 个", { exact: true })).toBeVisible();
  await expect(page.locator(".data-table__row").nth(0)).toContainText("Windows 客户端界面复刻");
  await expect(page.locator(".data-table__row").nth(0)).toContainText("mochiport");
  await expect(page.locator(".data-table__row").nth(2)).toContainText("release-notes");
  await expect(page.getByRole("alert")).toHaveCount(0);

  expect(healthCalls).toBeGreaterThanOrEqual(4);
  expect(sessionsAfterReady).toBeGreaterThanOrEqual(1);
  expect(sessionsBeforeReady).toBeLessThanOrEqual(1);
});

test("session source and empty state distinguish an offline Codex App", async ({ page }) => {
  await setInitialSection(page, "sessions");
  await page.route(`${managementOrigin}/**`, async (route) => {
    if (await handlePreflight(route)) return;
    const path = new URL(route.request().url()).pathname.replace(/^\//, "");
    if (path === "healthz") return fulfillJson(route, { service: "mochiport", apiMajor: 1, ready: true });
    if (path === "api/v1/manage/dashboard") {
      return fulfillJson(route, {
        ...fixtureDashboard,
        executionClients: {
          ...fixtureDashboard.executionClients,
          codexApp: { configured: true, connected: false },
        },
      });
    }
    if (path === "api/v1/manage/im/accounts") return fulfillJson(route, { accounts: fixtureAccounts });
    if (path === "api/v1/manage/lifecycle") return fulfillJson(route, fixtureLifecycle);
    if (path === "api/v1/manage/sessions") {
      return fulfillJson(route, { ok: true, threads: [], providers: ["ai-gateway", "openai"] });
    }
    return fulfillJson(route, { error: `未处理的测试请求：${path}` }, 404);
  });

  await page.goto("/");
  await expect(page.getByText("未连接 · 共 0 个", { exact: true })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Codex App 尚未连接", exact: true })).toBeVisible();
  await expect(page.getByText("请打开 Codex App，确认远程控制已启用，然后刷新。", { exact: true })).toBeVisible();
});

test("a non-terminal enhanced cancel response is polled until cancelled", async ({ page }) => {
  const requestId = "windows-cancel-regression";
  const startedAtMs = Date.now();
  let launchStarted = false;
  let cancelRequested = false;
  let pollsAfterCancel = 0;

  const operation = (
    phase: CodexEnhancedOperation["phase"],
    message: string,
    canCancel: boolean,
  ): CodexEnhancedOperation => ({
    requestId,
    phase,
    startedAtMs,
    updatedAtMs: Date.now(),
    canCancel,
    message,
  });

  await setInitialSection(page, "codex");
  await page.route(`${managementOrigin}/**`, async (route) => {
    if (await handlePreflight(route)) return;

    const request = route.request();
    const path = new URL(request.url()).pathname.replace(/^\//, "");
    if (path === "healthz") {
      await fulfillJson(route, { service: "mochiport", apiMajor: 1, ready: true });
      return;
    }
    if (path === "api/v1/manage/dashboard") {
      await fulfillJson(route, fixtureDashboard);
      return;
    }
    if (path === "api/v1/manage/im/accounts") {
      await fulfillJson(route, { accounts: fixtureAccounts });
      return;
    }
    if (path === "api/v1/manage/lifecycle") {
      await fulfillJson(route, fixtureLifecycle);
      return;
    }
    if (path === "api/v1/manage/codex/status") {
      await fulfillJson(route, fixtureCodexStatus);
      return;
    }
    if (path === "api/v1/manage/gateway") {
      await fulfillJson(route, fixtureGateway);
      return;
    }
    if (path === "api/v1/manage/codex/enhanced/preflight") {
      await fulfillJson(route, { ok: true, status: { running: false } });
      return;
    }
    if (path === "api/v1/manage/codex/enhanced/operation/start") {
      launchStarted = true;
      await fulfillJson(route, {
        ok: true,
        operation: operation("launching", "正在启动 Codex", true),
      });
      return;
    }
    if (path === "api/v1/manage/codex/enhanced/operation/cancel") {
      cancelRequested = true;
      await fulfillJson(route, {
        ok: true,
        operation: operation("launching", "正在取消增强启动", false),
      }, 202);
      return;
    }
    if (path === "api/v1/manage/codex/enhanced/operation") {
      if (!launchStarted) {
        await fulfillJson(route, { ok: true, operation: null });
        return;
      }
      if (!cancelRequested) {
        await fulfillJson(route, {
          ok: true,
          operation: operation("launching", "正在启动 Codex", true),
        });
        return;
      }
      pollsAfterCancel += 1;
      const cancelled = pollsAfterCancel >= 2;
      await fulfillJson(route, {
        ok: true,
        operation: cancelled
          ? operation("cancelled", "增强启动已取消", false)
          : operation("launching", "正在取消增强启动", false),
      });
      return;
    }

    await fulfillJson(route, { error: `未处理的测试请求：${path}` }, 404);
  });

  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Codex 接入" })).toBeVisible();

  await page.getByRole("button", { name: "增强模式启动 Codex" }).click();
  const operationCard = page.locator(".enhanced-operation");
  await expect(operationCard).toContainText("正在启动 Codex");
  await operationCard.getByRole("button", { name: "取消启动" }).click();

  await expect(operationCard).toContainText("正在取消增强启动");
  await expect.poll(() => pollsAfterCancel).toBeGreaterThanOrEqual(2);
  await expect(operationCard).toContainText("增强启动已取消");
  await expect(operationCard).toContainText("已取消");
  await expect(page.getByRole("button", { name: "增强模式启动 Codex" })).toBeEnabled();
});

test("cancelling while the exit preflight is in flight cannot start Codex", async ({ page }) => {
  let preflightCalls = 0;
  let startCalls = 0;
  let releaseExitPoll!: () => void;
  const exitPollGate = new Promise<void>((resolve) => {
    releaseExitPoll = resolve;
  });

  await setInitialSection(page, "codex");
  await page.route(`${managementOrigin}/**`, async (route) => {
    if (await handlePreflight(route)) return;

    const path = new URL(route.request().url()).pathname.replace(/^\//, "");
    if (await fulfillReadyBaseRead(route, path)) return;
    if (path === "api/v1/manage/codex/enhanced/operation") {
      await fulfillJson(route, { ok: true, operation: null });
      return;
    }
    if (path === "api/v1/manage/codex/enhanced/preflight") {
      preflightCalls += 1;
      if (preflightCalls === 1) {
        await fulfillJson(route, { ok: true, status: { running: true } });
        return;
      }
      await exitPollGate;
      await fulfillJson(route, { ok: true, status: { running: false } });
      return;
    }
    if (path === "api/v1/manage/codex/enhanced/operation/start") {
      startCalls += 1;
      await fulfillJson(route, {
        ok: true,
        operation: {
          requestId: "unexpected-start-after-cancel",
          phase: "launching",
          startedAtMs: Date.now(),
          updatedAtMs: Date.now(),
          canCancel: true,
          message: "不应启动",
        },
      });
      return;
    }

    await fulfillJson(route, { error: `未处理的测试请求：${path}` }, 404);
  });

  await page.goto("/");
  await expect(page.locator(".mode-card > strong")).toHaveText("MochiPort AI 网关");
  await page.getByRole("button", { name: "增强模式启动 Codex" }).click();

  const exitDialog = page.getByRole("dialog", { name: "请先退出 Codex" });
  await expect(exitDialog).toBeVisible();
  const exitPollResponse = page.waitForResponse((response) =>
    response.url().endsWith("/api/v1/manage/codex/enhanced/preflight") && preflightCalls >= 2,
  );
  await expect.poll(() => preflightCalls).toBeGreaterThanOrEqual(2);

  await exitDialog.getByRole("button", { name: "取消启动" }).click();
  await expect(exitDialog).not.toBeVisible();
  releaseExitPoll();
  await exitPollResponse;
  await page.waitForTimeout(100);

  expect(startCalls).toBe(0);
  await expect(page.locator(".enhanced-operation")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "增强模式启动 Codex" })).toBeEnabled();
});

test("a stale operation poll cannot overwrite a newer enhanced launch", async ({ page }) => {
  const firstRequestId = "stale-enhanced-request";
  const secondRequestId = "current-enhanced-request";
  const startedAtMs = Date.now();
  let startCalls = 0;
  let stalePollStarted = false;
  let releaseStalePoll!: () => void;
  const stalePollGate = new Promise<void>((resolve) => {
    releaseStalePoll = resolve;
  });

  const operation = (
    requestId: string,
    phase: CodexEnhancedOperation["phase"],
    message: string,
    canCancel: boolean,
  ): CodexEnhancedOperation => ({
    requestId,
    phase,
    startedAtMs,
    updatedAtMs: Date.now(),
    canCancel,
    message,
  });

  await setInitialSection(page, "codex");
  await page.route(`${managementOrigin}/**`, async (route) => {
    if (await handlePreflight(route)) return;

    const path = new URL(route.request().url()).pathname.replace(/^\//, "");
    if (await fulfillReadyBaseRead(route, path)) return;
    if (path === "api/v1/manage/codex/enhanced/preflight") {
      await fulfillJson(route, { ok: true, status: { running: false } });
      return;
    }
    if (path === "api/v1/manage/codex/enhanced/operation/start") {
      startCalls += 1;
      await fulfillJson(route, {
        ok: true,
        operation: startCalls === 1
          ? operation(firstRequestId, "launching", "第一个增强启动正在进行", true)
          : operation(secondRequestId, "ready", "新的增强启动已就绪", false),
      });
      return;
    }
    if (path === "api/v1/manage/codex/enhanced/operation/cancel") {
      await fulfillJson(route, {
        ok: true,
        operation: operation(firstRequestId, "cancelled", "第一个增强启动已取消", false),
      });
      return;
    }
    if (path === "api/v1/manage/codex/enhanced/operation") {
      if (startCalls === 0) {
        await fulfillJson(route, { ok: true, operation: null });
        return;
      }
      stalePollStarted = true;
      await stalePollGate;
      await fulfillJson(route, {
        ok: true,
        operation: operation(firstRequestId, "ready", "旧请求完成（不应覆盖）", false),
      });
      return;
    }

    await fulfillJson(route, { error: `未处理的测试请求：${path}` }, 404);
  });

  await page.goto("/");
  await expect(page.locator(".mode-card > strong")).toHaveText("MochiPort AI 网关");
  await page.getByRole("button", { name: "增强模式启动 Codex" }).click();

  const operationCard = page.locator(".enhanced-operation");
  await expect(operationCard).toContainText("第一个增强启动正在进行");
  await expect.poll(() => stalePollStarted).toBe(true);
  await operationCard.getByRole("button", { name: "取消启动" }).click();
  await expect(operationCard).toContainText("第一个增强启动已取消");

  await page.getByRole("button", { name: "增强模式启动 Codex" }).click();
  await expect(operationCard).toContainText("新的增强启动已就绪");

  const stalePollResponse = page.waitForResponse((response) =>
    response.url().endsWith("/api/v1/manage/codex/enhanced/operation")
      && response.request().method() === "GET",
  );
  releaseStalePoll();
  await stalePollResponse;
  await page.waitForTimeout(100);

  expect(startCalls).toBe(2);
  await expect(operationCard).toContainText("新的增强启动已就绪");
  await expect(operationCard).not.toContainText("旧请求完成（不应覆盖）");
});

test("a delayed cancel response cannot overwrite a newer enhanced launch", async ({ page }) => {
  const firstRequestId = "delayed-cancel-request";
  const secondRequestId = "new-request-after-cancel";
  const startedAtMs = Date.now();
  let startCalls = 0;
  let cancelStarted = false;
  let releaseCancel!: () => void;
  const cancelGate = new Promise<void>((resolve) => {
    releaseCancel = resolve;
  });

  const operation = (
    requestId: string,
    phase: CodexEnhancedOperation["phase"],
    message: string,
    canCancel: boolean,
  ): CodexEnhancedOperation => ({
    requestId,
    phase,
    startedAtMs,
    updatedAtMs: Date.now(),
    canCancel,
    message,
  });

  await setInitialSection(page, "codex");
  await page.route(`${managementOrigin}/**`, async (route) => {
    if (await handlePreflight(route)) return;

    const path = new URL(route.request().url()).pathname.replace(/^\//, "");
    if (await fulfillReadyBaseRead(route, path)) return;
    if (path === "api/v1/manage/codex/enhanced/preflight") {
      await fulfillJson(route, { ok: true, status: { running: false } });
      return;
    }
    if (path === "api/v1/manage/codex/enhanced/operation/start") {
      startCalls += 1;
      await fulfillJson(route, {
        ok: true,
        operation: startCalls === 1
          ? operation(firstRequestId, "launching", "第一个启动正在进行", true)
          : operation(secondRequestId, "ready", "第二个启动已就绪", false),
      });
      return;
    }
    if (path === "api/v1/manage/codex/enhanced/operation/cancel") {
      cancelStarted = true;
      await cancelGate;
      await fulfillJson(route, {
        ok: true,
        operation: operation(firstRequestId, "launching", "迟到的取消响应（不应覆盖）", false),
      }, 202);
      return;
    }
    if (path === "api/v1/manage/codex/enhanced/operation") {
      if (startCalls === 0) {
        await fulfillJson(route, { ok: true, operation: null });
        return;
      }
      await fulfillJson(route, {
        ok: true,
        operation: startCalls === 1 && cancelStarted
          ? operation(firstRequestId, "cancelled", "第一个启动已取消", false)
          : startCalls === 1
            ? operation(firstRequestId, "launching", "第一个启动正在进行", true)
            : operation(secondRequestId, "ready", "第二个启动已就绪", false),
      });
      return;
    }

    await fulfillJson(route, { error: `未处理的测试请求：${path}` }, 404);
  });

  await page.goto("/");
  await expect(page.locator(".mode-card > strong")).toHaveText("MochiPort AI 网关");
  await page.getByRole("button", { name: "增强模式启动 Codex" }).click();

  const operationCard = page.locator(".enhanced-operation");
  await expect(operationCard).toContainText("第一个启动正在进行");
  await operationCard.getByRole("button", { name: "取消启动" }).click();
  await expect.poll(() => cancelStarted).toBe(true);
  await expect(operationCard).toContainText("第一个启动已取消");

  await page.getByRole("button", { name: "增强模式启动 Codex" }).click();
  await expect(operationCard).toContainText("第二个启动已就绪");

  releaseCancel();
  await page.waitForTimeout(150);
  expect(startCalls).toBe(2);
  await expect(operationCard).toContainText("第二个启动已就绪");
  await expect(operationCard).not.toContainText("迟到的取消响应（不应覆盖）");
});

test("a delayed recovery response cannot overwrite a user-started enhanced launch", async ({ page }) => {
  const startedAtMs = Date.now();
  let recoveryStarted = false;
  let startCalls = 0;
  let releaseRecovery!: () => void;
  const recoveryGate = new Promise<void>((resolve) => {
    releaseRecovery = resolve;
  });

  const operation = (
    requestId: string,
    phase: CodexEnhancedOperation["phase"],
    message: string,
    canCancel: boolean,
  ): CodexEnhancedOperation => ({
    requestId,
    phase,
    startedAtMs,
    updatedAtMs: Date.now(),
    canCancel,
    message,
  });

  await setInitialSection(page, "codex");
  await page.route(`${managementOrigin}/**`, async (route) => {
    if (await handlePreflight(route)) return;

    const path = new URL(route.request().url()).pathname.replace(/^\//, "");
    if (await fulfillReadyBaseRead(route, path)) return;
    if (path === "api/v1/manage/codex/enhanced/preflight") {
      await fulfillJson(route, { ok: true, status: { running: false } });
      return;
    }
    if (path === "api/v1/manage/codex/enhanced/operation/start") {
      startCalls += 1;
      await fulfillJson(route, {
        ok: true,
        operation: operation("current-user-request", "ready", "用户启动已就绪", false),
      });
      return;
    }
    if (path === "api/v1/manage/codex/enhanced/operation") {
      recoveryStarted = true;
      await recoveryGate;
      await fulfillJson(route, {
        ok: true,
        operation: operation("stale-recovered-request", "launching", "迟到的恢复状态（不应覆盖）", true),
      });
      return;
    }

    await fulfillJson(route, { error: `未处理的测试请求：${path}` }, 404);
  });

  await page.goto("/");
  await expect(page.locator(".mode-card > strong")).toHaveText("MochiPort AI 网关");
  await expect.poll(() => recoveryStarted).toBe(true);

  await page.getByRole("button", { name: "增强模式启动 Codex" }).click();
  const operationCard = page.locator(".enhanced-operation");
  await expect(operationCard).toContainText("用户启动已就绪");

  releaseRecovery();
  await page.waitForTimeout(150);
  expect(startCalls).toBe(1);
  await expect(operationCard).toContainText("用户启动已就绪");
  await expect(operationCard).not.toContainText("迟到的恢复状态（不应覆盖）");
});
