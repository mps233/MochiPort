import { expect, test, type Page, type Route } from "@playwright/test";
import { fixtureAccounts, fixtureDashboard, fixtureLifecycle } from "../src/api/fixtures";
import type { CodexSession } from "../src/api/types";

const managementOrigin = "http://127.0.0.1:3847";
const corsHeaders = {
  "access-control-allow-origin": "*",
  "access-control-allow-methods": "GET, POST, OPTIONS",
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
const second: CodexSession = {
  id: "session-second",
  name: "Second session",
  preview: "second preview",
  modelProvider: "ai-gateway",
  updatedAt: 1_700_000_001,
  cwd: "C:\\Code\\alpha",
};

async function fulfillJson(route: Route, body: unknown, status = 200) {
  await route.fulfill({ status, headers: corsHeaders, body: JSON.stringify(body) });
}

async function installBaseRoutes(page: Page, sessions: (route: Route) => Promise<void>) {
  await page.addInitScript(() => localStorage.setItem("mochiport.section", "sessions"));
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
    if (path === "api/v1/manage/im/accounts") return fulfillJson(route, { accounts: fixtureAccounts });
    if (path === "api/v1/manage/sessions" || path === "api/v1/manage/sessions/provider") return sessions(route);
    return fulfillJson(route, { error: `未处理的测试请求：${path}` }, 404);
  });
}

function sessionRow(page: Page, name: string) {
  return page.locator(".data-table__row", { hasText: name });
}

test("a partial batch move reloads authoritative rows and keeps only failed selections", async ({ page }) => {
  let snapshot = [first, second];
  await installBaseRoutes(page, async (route) => {
    const request = route.request();
    if (request.method() === "GET") return fulfillJson(route, { ok: true, threads: snapshot, providers: ["ai-gateway", "openai"] });
    const body = request.postDataJSON() as { threadId: string };
    if (body.threadId === second.id) return fulfillJson(route, { error: "第二条会话被占用" }, 409);
    snapshot = [
      { ...first, name: "First authoritative", preview: "rewritten by daemon", modelProvider: "openai", updatedAt: first.updatedAt + 100 },
      second,
    ];
    return fulfillJson(route, { ok: true });
  });

  await page.goto("/");
  await sessionRow(page, "First session").getByRole("checkbox").check();
  await sessionRow(page, "Second session").getByRole("checkbox").check();
  await page.getByLabel("目标 Provider").selectOption("openai");
  await page.getByRole("button", { name: "移动会话" }).click();

  const authoritative = sessionRow(page, "First authoritative");
  await expect(authoritative).toContainText("rewritten by daemon");
  await expect(authoritative.locator(".route-cell small")).toHaveText("openai");
  await expect(authoritative.getByRole("checkbox")).not.toBeChecked();
  await expect(sessionRow(page, "Second session").getByRole("checkbox")).toBeChecked();
  await expect(page.getByRole("alert")).toContainText("移动完成：成功 1 条、失败 1 条。第二条会话被占用");
});

test("a failed authoritative reload reports the failure without inventing local state", async ({ page }) => {
  let sessionReads = 0;
  await installBaseRoutes(page, async (route) => {
    if (route.request().method() === "POST") return fulfillJson(route, { ok: true });
    sessionReads += 1;
    if (sessionReads > 1) return fulfillJson(route, { error: "会话存储暂时不可用" }, 503);
    return fulfillJson(route, { ok: true, threads: [first], providers: ["ai-gateway", "openai"] });
  });

  await page.goto("/");
  const row = sessionRow(page, "First session");
  await row.getByRole("checkbox").check();
  await page.getByLabel("目标 Provider").selectOption("openai");
  await page.getByRole("button", { name: "移动会话" }).click();

  await expect(page.getByRole("alert")).toContainText("会话列表刷新失败：会话存储暂时不可用");
  await expect(row.locator(".route-cell small")).toHaveText("ai-gateway");
});

test("an older manual reload cannot overwrite the post-move authoritative snapshot", async ({ page }) => {
  let sessionReads = 0;
  let releaseOldRead = () => {};
  const oldReadGate = new Promise<void>((resolve) => { releaseOldRead = resolve; });
  await installBaseRoutes(page, async (route) => {
    if (route.request().method() === "POST") return fulfillJson(route, { ok: true });
    sessionReads += 1;
    if (sessionReads === 2) {
      await oldReadGate;
      return fulfillJson(route, { ok: true, threads: [{ ...first, name: "Stale manual result" }], providers: ["ai-gateway", "openai"] });
    }
    if (sessionReads >= 3) {
      return fulfillJson(route, { ok: true, threads: [{ ...first, name: "Authoritative after move", modelProvider: "openai" }], providers: ["ai-gateway", "openai"] });
    }
    return fulfillJson(route, { ok: true, threads: [first], providers: ["ai-gateway", "openai"] });
  });

  await page.goto("/");
  await expect(sessionRow(page, "First session")).toBeVisible();
  await page.getByRole("button", { name: "刷新", exact: true }).last().click();
  await expect.poll(() => sessionReads).toBe(2);

  await sessionRow(page, "First session").getByRole("checkbox").check();
  await page.getByLabel("目标 Provider").selectOption("openai");
  await page.getByRole("button", { name: "移动会话" }).click();
  await expect(sessionRow(page, "Authoritative after move")).toBeVisible();

  releaseOldRead();
  await expect.poll(() => sessionReads).toBeGreaterThanOrEqual(3);
  await expect(sessionRow(page, "Stale manual result")).toHaveCount(0);
  await expect(sessionRow(page, "Authoritative after move").locator(".route-cell small")).toHaveText("openai");
});
