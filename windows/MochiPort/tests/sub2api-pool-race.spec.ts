import { expect, test, type Page, type Route } from "@playwright/test";
import {
  fixtureAccounts,
  fixtureDashboard,
  fixtureGateway,
  fixtureLifecycle,
  fixtureSub2ApiPool,
} from "../src/api/fixtures";
import type { Sub2ApiAdmin, Sub2ApiPool } from "../src/api/types";

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

function poolNamed(name: string): Sub2ApiPool {
  return {
    ...structuredClone(fixtureSub2ApiPool),
    fetchedAtMs: Date.now(),
    accounts: fixtureSub2ApiPool.accounts.map((account, index) => ({
      ...structuredClone(account),
      name: index === 0 ? name : account.name,
    })),
  };
}

test("a stale Sub2API pool request cannot overwrite a newly saved connection", async ({ page }) => {
  const pageErrors: string[] = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));
  let admin: Sub2ApiAdmin = {
    configured: true,
    baseUrl: "https://old-sub2api.example.com",
    secretSet: true,
  };
  let poolRequests = 0;
  let releaseOldPool!: () => void;
  const oldPoolGate = new Promise<void>((resolve) => {
    releaseOldPool = resolve;
  });
  let submittedConfig: Record<string, unknown> | undefined;

  await page.addInitScript(() => localStorage.setItem("mochiport.section", "gateway"));
  await installManagementMock(page, async (route, path) => {
    const request = route.request();
    const body = request.postDataJSON() as Record<string, unknown> | null;
    if (path === "healthz") {
      return fulfillJson(route, { service: "mochiport", apiMajor: 1, ready: true });
    }
    if (path === "api/v1/manage/dashboard") return fulfillJson(route, fixtureDashboard);
    if (path === "api/v1/manage/im/accounts") return fulfillJson(route, { accounts: fixtureAccounts });
    if (path === "api/v1/manage/lifecycle") return fulfillJson(route, fixtureLifecycle);
    if (path === "api/v1/manage/gateway" && request.method() === "GET") {
      return fulfillJson(route, fixtureGateway);
    }
    if (path === "api/v1/manage/gateway/sub2api" && request.method() === "GET") {
      return fulfillJson(route, admin);
    }
    if (path === "api/v1/manage/gateway/sub2api/config") {
      submittedConfig = body ?? undefined;
      admin = {
        configured: true,
        baseUrl: String(body?.baseUrl ?? ""),
        secretSet: true,
      };
      return fulfillJson(route, { ok: true, sub2api: admin });
    }
    if (path === "api/v1/manage/gateway/sub2api/accounts") {
      poolRequests += 1;
      if (poolRequests === 2) {
        await oldPoolGate;
        return fulfillJson(route, { ok: true, pool: poolNamed("迟到的旧账号") });
      }
      return fulfillJson(route, {
        ok: true,
        pool: poolNamed(poolRequests === 1 ? "初始账号" : "新连接账号"),
      });
    }
    return fulfillJson(route, { error: `未处理的测试请求：${path}` }, 404);
  });

  await page.goto("/");
  await page.waitForTimeout(100);
  expect(pageErrors).toEqual([]);
  await page.getByRole("tab", { name: "账号池", exact: true }).click();
  await expect(page.getByText("初始账号", { exact: true })).toBeVisible();
  await expect.poll(() => poolRequests).toBe(1);

  await page.locator(".account-pool-page").getByRole("button", { name: "刷新", exact: true }).click();
  await expect.poll(() => poolRequests).toBe(2);

  await page.getByRole("button", { name: "编辑", exact: true }).click();
  await page.getByLabel("Sub2API 地址").fill("https://new-sub2api.example.com");
  await page.getByLabel("管理 API Key").fill("replacement-admin-key");
  await page.getByRole("button", { name: "保存并连接" }).click();

  await expect.poll(() => poolRequests).toBe(3);
  await expect(page.getByText("新连接账号", { exact: true })).toBeVisible();
  expect(submittedConfig).toMatchObject({
    baseUrl: "https://new-sub2api.example.com",
    adminApiKey: "replacement-admin-key",
    clearAdminApiKey: false,
  });

  releaseOldPool();
  await page.waitForTimeout(150);
  await expect(page.getByText("新连接账号", { exact: true })).toBeVisible();
  await expect(page.getByText("迟到的旧账号", { exact: true })).toHaveCount(0);
  await expect(page.getByText("https://new-sub2api.example.com", { exact: true })).toBeVisible();
});

async function installManagementMock(
  page: Page,
  handler: (route: Route, path: string) => Promise<void>,
) {
  await page.route(`${managementOrigin}/**`, async (route) => {
    if (route.request().method() === "OPTIONS") {
      await route.fulfill({ status: 204, headers: corsHeaders });
      return;
    }
    const path = new URL(route.request().url()).pathname.replace(/^\//, "");
    await handler(route, path);
  });
}
