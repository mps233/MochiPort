import { expect, test, type Route } from "@playwright/test";
import {
  fixtureAccounts,
  fixtureDashboard,
  fixtureLifecycle,
  fixtureLogs,
} from "../src/api/fixtures";

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

test("a slow initial request-log load cannot overwrite newer filtered results", async ({ page }) => {
  let initialLogQueryStarted = false;
  let filteredLogQueryStarted = false;
  let releaseInitial!: () => void;
  const initialGate = new Promise<void>((resolve) => {
    releaseInitial = resolve;
  });
  const slowInitialLog = { ...fixtureLogs[0], id: 9_001, requestId: "slow-initial-result", modelId: "stale-model" };
  const filteredLog = { ...fixtureLogs[2], id: 9_002, requestId: "filtered-current-result", modelId: "filtered-model", status: "failed" };

  await page.addInitScript(() => localStorage.setItem("mochiport.section", "requestLogs"));
  await page.route(`${managementOrigin}/**`, async (route) => {
    const request = route.request();
    if (request.method() === "OPTIONS") {
      await route.fulfill({ status: 204, headers: corsHeaders });
      return;
    }
    const url = new URL(request.url());
    const path = url.pathname.replace(/^\//, "");
    if (path === "healthz") return fulfillJson(route, { service: "mochiport", apiMajor: 1, ready: true });
    if (path === "api/v1/manage/dashboard") return fulfillJson(route, fixtureDashboard);
    if (path === "api/v1/manage/im/accounts") return fulfillJson(route, { accounts: fixtureAccounts });
    if (path === "api/v1/manage/lifecycle") return fulfillJson(route, fixtureLifecycle);
    if (path === "api/v1/manage/request-logs") {
      if (url.searchParams.get("status") === "failed") {
        filteredLogQueryStarted = true;
        return fulfillJson(route, { logs: [filteredLog], nextCursor: null, hasMore: false });
      }
      initialLogQueryStarted = true;
      await initialGate;
      return fulfillJson(route, { logs: [slowInitialLog], nextCursor: "stale-cursor", hasMore: true });
    }
    return fulfillJson(route, { error: `未处理的测试请求：${path}` }, 404);
  });

  await page.goto("/");
  await expect(page.getByRole("heading", { name: "请求日志", exact: true })).toBeVisible();
  await expect.poll(() => initialLogQueryStarted).toBe(true);
  await page.getByRole("combobox", { name: "状态" }).selectOption("failed");
  await expect.poll(() => filteredLogQueryStarted).toBe(true);
  await expect(page.locator(".log-row")).toHaveCount(1);
  await expect(page.locator(".log-row")).toContainText("filtered-current-result");

  releaseInitial();
  await page.waitForTimeout(150);
  await expect(page.locator(".log-row")).toHaveCount(1);
  await expect(page.locator(".log-row")).toContainText("filtered-current-result");
  await expect(page.locator(".log-row")).not.toContainText("slow-initial-result");
  await expect(page.getByRole("button", { name: "加载更多" })).toHaveCount(0);
});

test("a stale initial request-log failure cannot clear a newer query's loading state", async ({ page }) => {
  let initialStarted = false;
  let filteredStarted = false;
  let releaseInitial!: () => void;
  let releaseFiltered!: () => void;
  const initialGate = new Promise<void>((resolve) => { releaseInitial = resolve; });
  const filteredGate = new Promise<void>((resolve) => { releaseFiltered = resolve; });
  const filteredLog = { ...fixtureLogs[2], id: 9_003, requestId: "current-after-stale-error", status: "failed" };

  await page.addInitScript(() => localStorage.setItem("mochiport.section", "requestLogs"));
  await page.route(`${managementOrigin}/**`, async (route) => {
    const request = route.request();
    if (request.method() === "OPTIONS") {
      await route.fulfill({ status: 204, headers: corsHeaders });
      return;
    }
    const url = new URL(request.url());
    const path = url.pathname.replace(/^\//, "");
    if (path === "healthz") return fulfillJson(route, { service: "mochiport", apiMajor: 1, ready: true });
    if (path === "api/v1/manage/dashboard") return fulfillJson(route, fixtureDashboard);
    if (path === "api/v1/manage/im/accounts") return fulfillJson(route, { accounts: fixtureAccounts });
    if (path === "api/v1/manage/lifecycle") return fulfillJson(route, fixtureLifecycle);
    if (path === "api/v1/manage/request-logs") {
      if (url.searchParams.get("status") === "failed") {
        filteredStarted = true;
        await filteredGate;
        return fulfillJson(route, { logs: [filteredLog], nextCursor: null, hasMore: false });
      }
      initialStarted = true;
      await initialGate;
      return fulfillJson(route, { error: "迟到的首屏错误（不应显示）" }, 500);
    }
    return fulfillJson(route, { error: `未处理的测试请求：${path}` }, 404);
  });

  await page.goto("/");
  await expect.poll(() => initialStarted).toBe(true);
  await page.getByRole("combobox", { name: "状态" }).selectOption("failed");
  await expect.poll(() => filteredStarted).toBe(true);
  const refreshButton = page.getByRole("button", { name: "刷新", exact: true }).last();
  await expect(refreshButton).toBeDisabled();

  releaseInitial();
  await page.waitForTimeout(150);
  await expect(page.getByRole("alert")).toHaveCount(0);
  await expect(refreshButton).toBeDisabled();

  releaseFiltered();
  await expect(refreshButton).toBeEnabled();
  await expect(page.locator(".log-row")).toContainText("current-after-stale-error");
  await expect(page.getByRole("alert")).toHaveCount(0);
});
