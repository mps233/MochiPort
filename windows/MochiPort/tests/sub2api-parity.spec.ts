import { expect, test, type Page, type Route } from "@playwright/test";
import {
  fixtureAccounts,
  fixtureDashboard,
  fixtureGateway,
  fixtureLifecycle,
  fixtureSub2ApiPool,
} from "../src/api/fixtures";
import type { Sub2ApiPool } from "../src/api/types";

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

interface Sub2ApiMockState {
  baseUrl: string;
  accountRequests: Array<Record<string, unknown>>;
  schedulableRequests: Array<Record<string, unknown>>;
  recentAccountRequests: Array<Record<string, unknown>>;
  pool: Sub2ApiPool;
  failSchedulableMutation?: boolean;
}

async function installManagementMock(page: Page, state: Sub2ApiMockState) {
  await page.route(`${managementOrigin}/**`, async (route) => {
    const request = route.request();
    if (request.method() === "OPTIONS") {
      await route.fulfill({ status: 204, headers: corsHeaders });
      return;
    }
    const path = new URL(request.url()).pathname.replace(/^\//, "");
    const body = request.postDataJSON() as Record<string, unknown> | null;
    if (path === "healthz") return fulfillJson(route, { service: "mochiport", apiMajor: 1, ready: true });
    if (path === "api/v1/manage/dashboard") return fulfillJson(route, fixtureDashboard);
    if (path === "api/v1/manage/im/accounts") return fulfillJson(route, { accounts: fixtureAccounts });
    if (path === "api/v1/manage/lifecycle") return fulfillJson(route, fixtureLifecycle);
    if (path === "api/v1/manage/gateway" && request.method() === "GET") return fulfillJson(route, fixtureGateway);
    if (path === "api/v1/manage/gateway/provider-templates") return fulfillJson(route, { templates: [] });
    if (path === "api/v1/manage/gateway/sub2api" && request.method() === "GET") {
      return fulfillJson(route, { configured: true, baseUrl: state.baseUrl, secretSet: true });
    }
    if (path === "api/v1/manage/gateway/sub2api/config") {
      state.baseUrl = String(body?.baseUrl ?? state.baseUrl);
      return fulfillJson(route, { ok: true, sub2api: { configured: true, baseUrl: state.baseUrl, secretSet: true } });
    }
    if (path === "api/v1/manage/gateway/sub2api/accounts") {
      state.accountRequests.push(body ?? {});
      return fulfillJson(route, { ok: true, pool: { ...state.pool, fetchedAtMs: Date.now() } });
    }
    const schedulableMatch = path.match(/^api\/v1\/manage\/gateway\/sub2api\/accounts\/(\d+)\/schedulable$/);
    if (schedulableMatch) {
      state.schedulableRequests.push(body ?? {});
      if (state.failSchedulableMutation) {
        return fulfillJson(route, { error: "账号调度设置失败" }, 502);
      }
      const accountId = Number(schedulableMatch[1]);
      const schedulable = body?.schedulable === true;
      state.pool = {
        ...state.pool,
        accounts: state.pool.accounts.map((account) => account.id === accountId ? { ...account, schedulable } : account),
      };
      return fulfillJson(route, { ok: true, accountId, schedulable });
    }
    if (path === "api/v1/manage/gateway/provider/usage") {
      return fulfillJson(route, {
        ok: true,
        providerName: String(body?.providerName ?? ""),
        usage: {
          source: "sub2api",
          balanceStatus: "available",
          billingStatus: "available",
          remaining: 999,
          unlimited: false,
          unit: "USD",
          todayActualCost: 1.25,
          effectiveRateMultiplier: 1,
        },
      });
    }
    if (path === "api/v1/manage/gateway/provider/recent-account") {
      state.recentAccountRequests.push(body ?? {});
      return fulfillJson(route, {
        ok: true,
        providerName: String(body?.providerName ?? ""),
        account: { accountId: state.pool.accounts[0].id, accountName: state.pool.accounts[0].name, createdAt: new Date().toISOString() },
      });
    }
    return fulfillJson(route, { error: `未处理的测试请求：${path}` }, 404);
  });
}

test("Sub2API pool uses its five-minute cache, force refresh body, warnings, and recent account", async ({ page }) => {
  const state: Sub2ApiMockState = {
    baseUrl: "https://sub2api.example.com",
    accountRequests: [],
    schedulableRequests: [],
    recentAccountRequests: [],
    pool: { ...structuredClone(fixtureSub2ApiPool), warnings: ["部分上游倍率仍在刷新"] },
  };
  await page.addInitScript(() => localStorage.setItem("mochiport.section", "gateway"));
  await installManagementMock(page, state);
  await page.goto("/");

  await expect(page.getByRole("tab", { name: "常规", exact: true })).toBeVisible();
  await expect.poll(() => state.accountRequests.length).toBe(1);
  expect(state.accountRequests[0]).toEqual({ forceBillingRefresh: false });

  await page.getByRole("tab", { name: "账号池", exact: true }).click();
  await expect(page.getByText("部分上游倍率仍在刷新")).toBeVisible();
  await expect(page.getByText(state.pool.accounts[0].name, { exact: true })).toBeVisible();
  expect(state.accountRequests).toHaveLength(1);

  await page.locator(".account-pool-page").getByRole("button", { name: "刷新", exact: true }).click();
  await expect.poll(() => state.accountRequests.length).toBe(2);
  expect(state.accountRequests[1]).toEqual({ forceBillingRefresh: true });

  await page.getByRole("tab", { name: "模型服务", exact: true }).click();
  const providerCard = page.locator(".provider-card", { hasText: "OpenAI" });
  await providerCard.getByRole("button", { name: "用量" }).click();
  const dialog = page.getByRole("dialog", { name: /OpenAI · 余额与计费/ });
  await expect(dialog.getByText("最近使用账号", { exact: true })).toBeVisible();
  await expect(dialog.getByText(state.pool.accounts[0].name, { exact: true })).toBeVisible();
  await expect(dialog.locator(".provider-usage__hero strong")).toHaveText("42.86");
  expect(state.recentAccountRequests.at(-1)).toEqual({ providerName: "OpenAI" });
});

test("Sub2API account schedule switch persists per-account state", async ({ page }) => {
  const state: Sub2ApiMockState = {
    baseUrl: "https://sub2api.example.com",
    accountRequests: [],
    schedulableRequests: [],
    recentAccountRequests: [],
    pool: structuredClone(fixtureSub2ApiPool),
  };
  await page.addInitScript(() => localStorage.setItem("mochiport.section", "gateway"));
  await installManagementMock(page, state);
  await page.goto("/");
  await page.getByRole("tab", { name: "账号池", exact: true }).click();

  const scheduleSwitch = page.getByRole("switch", { name: "OpenAI 主账号调度" });
  await expect(scheduleSwitch).toHaveAttribute("aria-checked", "true");
  await scheduleSwitch.click();
  await expect.poll(() => state.schedulableRequests.length).toBe(1);
  expect(state.schedulableRequests[0]).toEqual({ schedulable: false });
  await expect(scheduleSwitch).toHaveAttribute("aria-checked", "false");

  await page.locator(".account-pool-page").getByRole("button", { name: "刷新", exact: true }).click();
  await expect(scheduleSwitch).toHaveAttribute("aria-checked", "false");
});

test("Sub2API account schedule switch rolls back when the daemon rejects it", async ({ page }) => {
  const state: Sub2ApiMockState = {
    baseUrl: "https://sub2api.example.com",
    accountRequests: [],
    schedulableRequests: [],
    recentAccountRequests: [],
    pool: structuredClone(fixtureSub2ApiPool),
    failSchedulableMutation: true,
  };
  await page.addInitScript(() => localStorage.setItem("mochiport.section", "gateway"));
  await installManagementMock(page, state);
  await page.goto("/");
  await page.getByRole("tab", { name: "账号池", exact: true }).click();

  const scheduleSwitch = page.getByRole("switch", { name: "OpenAI 主账号调度" });
  await scheduleSwitch.click();
  await expect.poll(() => state.schedulableRequests.length).toBe(1);
  await expect(scheduleSwitch).toHaveAttribute("aria-checked", "true");
  await expect(page.getByText("账号调度设置失败", { exact: true }).first()).toBeVisible();
});

test("a late pool refresh cannot restore the previous connection's accounts", async ({ page }) => {
  const stalePool = structuredClone(fixtureSub2ApiPool);
  stalePool.accounts[0].name = "迟到的旧账号";
  const currentPool = structuredClone(fixtureSub2ApiPool);
  currentPool.accounts[0].name = "新连接账号";
  const state: Sub2ApiMockState = {
    baseUrl: "https://old-sub2api.example.com",
    accountRequests: [],
    schedulableRequests: [],
    recentAccountRequests: [],
    pool: structuredClone(fixtureSub2ApiPool),
  };
  let releaseStale!: () => void;
  const staleGate = new Promise<void>((resolve) => { releaseStale = resolve; });
  let forceRequestCount = 0;

  await page.addInitScript(() => localStorage.setItem("mochiport.section", "gateway"));
  await installManagementMock(page, state);
  await page.route(`${managementOrigin}/api/v1/manage/gateway/sub2api/accounts`, async (route) => {
    const body = route.request().postDataJSON() as Record<string, unknown>;
    state.accountRequests.push(body);
    if (body.forceBillingRefresh === true) {
      forceRequestCount += 1;
      if (forceRequestCount === 1) {
        await staleGate;
        return fulfillJson(route, { ok: true, pool: { ...stalePool, fetchedAtMs: Date.now() } });
      }
      return fulfillJson(route, { ok: true, pool: { ...currentPool, fetchedAtMs: Date.now() } });
    }
    return fulfillJson(route, { ok: true, pool: { ...state.pool, fetchedAtMs: Date.now() } });
  });
  await page.goto("/");
  await page.getByRole("tab", { name: "账号池", exact: true }).click();
  await expect(page.getByText("OpenAI 主账号", { exact: true })).toBeVisible();

  await page.locator(".account-pool-page").getByRole("button", { name: "刷新", exact: true }).click();
  await expect.poll(() => forceRequestCount).toBe(1);
  await page.getByRole("button", { name: "编辑", exact: true }).click();
  await page.getByLabel("Sub2API 地址").fill("https://new-sub2api.example.com");
  await page.getByRole("button", { name: "保存并连接" }).click();
  await expect.poll(() => forceRequestCount).toBe(2);
  await expect(page.getByText("新连接账号", { exact: true })).toBeVisible();

  releaseStale();
  await page.waitForTimeout(150);
  await expect(page.getByText("新连接账号", { exact: true })).toBeVisible();
  await expect(page.getByText("迟到的旧账号", { exact: true })).toHaveCount(0);
});
