import { expect, test, type Route } from "@playwright/test";
import { fixtureAccounts, fixtureDashboard, fixtureLifecycle } from "../src/api/fixtures";

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

test("an account refresh failure preserves the last successful account list", async ({ page }) => {
  let failAccounts = false;
  let accountReads = 0;
  await page.addInitScript(() => localStorage.setItem("mochiport.section", "messaging"));
  await page.route(`${managementOrigin}/**`, async (route) => {
    const request = route.request();
    if (request.method() === "OPTIONS") {
      await route.fulfill({ status: 204, headers: corsHeaders });
      return;
    }
    const path = new URL(request.url()).pathname.replace(/^\//, "");
    if (path === "healthz") return fulfillJson(route, { service: "threadrelay", apiMajor: 1, ready: true });
    if (path === "api/v1/manage/dashboard") return fulfillJson(route, fixtureDashboard);
    if (path === "api/v1/manage/lifecycle") return fulfillJson(route, fixtureLifecycle);
    if (path === "api/v1/manage/gateway/sub2api") return fulfillJson(route, { configured: false, baseUrl: "", secretSet: false });
    if (path === "api/v1/manage/im/accounts") {
      accountReads += 1;
      return failAccounts
        ? fulfillJson(route, { error: "消息配置存储暂时不可用" }, 503)
        : fulfillJson(route, { accounts: fixtureAccounts });
    }
    return fulfillJson(route, { error: `未处理的测试请求：${path}` }, 404);
  });

  await page.goto("/");
  const account = page.locator(".account-card", { hasText: "MochiPort Bot" });
  await expect(account).toBeVisible();
  await expect.poll(() => accountReads).toBeGreaterThanOrEqual(2);

  failAccounts = true;
  await page.getByRole("button", { name: "概览", exact: true }).click();
  const failedRefresh = page.waitForResponse((response) => response.url().endsWith("/api/v1/manage/im/accounts") && response.status() === 503);
  await page.locator(".page-header").getByRole("button", { name: "刷新", exact: true }).click();
  await failedRefresh;

  await page.getByRole("button", { name: "消息渠道", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("继续显示上次成功读取的配置");
  await expect(account).toBeVisible();
  await expect(page.getByRole("heading", { name: "还没有消息账号" })).toHaveCount(0);

  failAccounts = false;
  const successfulRetry = page.waitForResponse((response) => response.url().endsWith("/api/v1/manage/im/accounts") && response.status() === 200);
  await page.getByRole("button", { name: "重试", exact: true }).click();
  await successfulRetry;
  await expect(page.getByRole("alert")).toHaveCount(0);
  await expect(account).toBeVisible();
});
