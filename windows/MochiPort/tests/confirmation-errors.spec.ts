import { expect, test, type Page, type Route } from "@playwright/test";
import {
  fixtureAccounts,
  fixtureCodexStatus,
  fixtureDashboard,
  fixtureGateway,
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

type Section = "codex" | "messaging" | "requestLogs";

async function fulfillJson(route: Route, body: unknown, status = 200) {
  await route.fulfill({ status, headers: corsHeaders, body: JSON.stringify(body) });
}

async function installFailureRoutes(page: Page, section: Section) {
  await page.addInitScript((initialSection) => {
    localStorage.setItem("mochiport.section", initialSection);
  }, section);

  await page.route(`${managementOrigin}/**`, async (route) => {
    const request = route.request();
    if (request.method() === "OPTIONS") {
      await route.fulfill({ status: 204, headers: corsHeaders });
      return;
    }

    const path = new URL(request.url()).pathname.replace(/^\//, "");
    if (path === "healthz") return fulfillJson(route, { service: "threadrelay", apiMajor: 1, ready: true });
    if (path === "api/v1/manage/dashboard") return fulfillJson(route, fixtureDashboard);
    if (path === "api/v1/manage/im/accounts") return fulfillJson(route, { accounts: fixtureAccounts });
    if (path === "api/v1/manage/lifecycle") return fulfillJson(route, fixtureLifecycle);
    if (path === "api/v1/manage/gateway/sub2api") {
      return fulfillJson(route, { configured: false, baseUrl: "", secretSet: false });
    }
    if (path === "api/v1/manage/codex/status") return fulfillJson(route, fixtureCodexStatus);
    if (path === "api/v1/manage/gateway") return fulfillJson(route, fixtureGateway);
    if (path === "api/v1/manage/codex/enhanced/operation") {
      return fulfillJson(route, { ok: true, operation: null });
    }
    if (path === "api/v1/manage/request-logs") {
      return fulfillJson(route, { logs: fixtureLogs, nextCursor: null, hasMore: false });
    }

    if (path === "api/v1/manage/codex/uninstall") {
      return fulfillJson(route, { error: "Codex 连接配置正被占用，无法断开。" }, 409);
    }
    if (path === "api/v1/manage/im/account/delete") {
      return fulfillJson(route, { error: "消息账号仍有进行中的流式回复，无法删除。" }, 409);
    }
    if (path === "api/v1/manage/request-logs/clear-old") {
      return fulfillJson(route, { error: "请求日志数据库暂时被占用。" }, 503);
    }

    return fulfillJson(route, { error: `未处理的测试请求：${path}` }, 404);
  });
}

test("Codex disconnect failures remain visible inside the confirmation dialog", async ({ page }) => {
  await installFailureRoutes(page, "codex");
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Codex 接入", exact: true })).toBeVisible();

  await page.getByRole("switch", { name: "连接 MochiPort" }).click();
  const dialog = page.getByRole("dialog", { name: "断开 Codex？" });
  await dialog.getByRole("button", { name: "断开连接" }).click();

  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("alert")).toContainText("Codex 连接配置正被占用");
  await expect(page.getByRole("alert")).toHaveCount(1);
});

test("message-account deletion failures remain visible inside the confirmation dialog", async ({ page }) => {
  await installFailureRoutes(page, "messaging");
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "消息渠道", exact: true })).toBeVisible();

  const account = page.locator(".account-card", { hasText: "MochiPort Bot" });
  await account.getByRole("button", { name: "展开账号详情 MochiPort Bot" }).click();
  await account.getByRole("button", { name: "删除账号" }).click();
  const dialog = page.getByRole("dialog", { name: "删除消息账号？" });
  await dialog.getByRole("button", { name: "删除账号" }).click();

  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("alert")).toContainText("仍有进行中的流式回复");
  await expect(page.getByRole("alert")).toHaveCount(1);
  await expect(account).toBeVisible();
});

test("request-log cleanup failures remain visible inside the confirmation dialog", async ({ page }) => {
  await installFailureRoutes(page, "requestLogs");
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "请求日志", exact: true })).toBeVisible();

  await page.getByRole("button", { name: "清理日志" }).click();
  const dialog = page.getByRole("dialog", { name: "清理旧请求日志" });
  await dialog.getByRole("button", { name: "删除旧日志" }).click();

  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("alert")).toContainText("请求日志数据库暂时被占用");
  await expect(page.getByRole("alert")).toHaveCount(1);
  await expect(page.locator(".log-row")).toHaveCount(fixtureLogs.length);
});
