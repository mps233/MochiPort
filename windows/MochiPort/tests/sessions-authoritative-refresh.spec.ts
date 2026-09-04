import { expect, test, type Page, type Route } from "@playwright/test";
import { fixtureAccounts, fixtureDashboard, fixtureLifecycle } from "../src/api/fixtures";
import type { CodexSession } from "../src/api/types";

const managementOrigin = "http://127.0.0.1:3847";
const corsHeaders = {
  "access-control-allow-origin": "*",
  "access-control-allow-methods": "GET, OPTIONS",
  "access-control-allow-headers": "content-type",
  "content-type": "application/json",
};

const first: CodexSession = {
  id: "session-first",
  name: "First session",
  preview: "first preview",
  modelProvider: "ai-gateway",
  updatedAt: 1_700_000_000,
  cwd: "C:\\Code\\alpha",
};

const refreshed: CodexSession = {
  ...first,
  name: "Refreshed session",
  preview: "snapshot returned by Codex",
  modelProvider: "openai",
  updatedAt: first.updatedAt + 1,
};

async function fulfillJson(route: Route, body: unknown, status = 200) {
  await route.fulfill({ status, headers: corsHeaders, body: JSON.stringify(body) });
}

async function installBaseRoutes(page: Page, sessions: () => CodexSession[]) {
  await page.addInitScript(() => localStorage.setItem("mochiport.section", "sessions"));
  await page.route(`${managementOrigin}/**`, async (route) => {
    const request = route.request();
    if (request.method() === "OPTIONS") {
      await route.fulfill({ status: 204, headers: corsHeaders });
      return;
    }
    const path = new URL(request.url()).pathname.replace(/^\//, "");
    if (path === "healthz") return fulfillJson(route, { service: "mochiport", apiMajor: 1, ready: true });
    if (path === "api/v1/manage/dashboard") return fulfillJson(route, fixtureDashboard);
    if (path === "api/v1/manage/lifecycle") return fulfillJson(route, fixtureLifecycle);
    if (path === "api/v1/manage/gateway/sub2api") return fulfillJson(route, { configured: false, baseUrl: "", secretSet: false });
    if (path === "api/v1/manage/im/accounts") return fulfillJson(route, { accounts: fixtureAccounts });
    if (path === "api/v1/manage/sessions") {
      expect(request.method()).toBe("GET");
      const threads = sessions();
      return fulfillJson(route, { ok: true, threads, providers: ["ai-gateway", "openai"], total: threads.length });
    }
    return fulfillJson(route, { error: `未处理的测试请求：${path}` }, 404);
  });
}

test("refresh replaces the read-only session snapshot from Codex", async ({ page }) => {
  let snapshot = [first];
  await installBaseRoutes(page, () => snapshot);

  await page.goto("/");
  await expect(page.locator(".data-table__row", { hasText: "First session" })).toBeVisible();

  snapshot = [refreshed];
  await page.getByRole("button", { name: "刷新", exact: true }).last().click();

  const row = page.locator(".data-table__row", { hasText: "Refreshed session" });
  await expect(row).toContainText("snapshot returned by Codex");
  await expect(row.locator(".route-cell small")).toHaveText("openai");
  await expect(page.getByRole("checkbox")).toHaveCount(0);
});
