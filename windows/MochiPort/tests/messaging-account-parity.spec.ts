import { expect, test, type Route } from "@playwright/test";
import { fixtureDashboard, fixtureLifecycle } from "../src/api/fixtures";
import type { IMAccount } from "../src/api/types";

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

const pngAvatar = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";

function account(overrides: Partial<IMAccount> & Pick<IMAccount, "accountId" | "displayName">): IMAccount {
  return {
    platform: "telegram",
    avatarData: null,
    enabled: true,
    configured: true,
    secretSet: true,
    connecting: false,
    polling: false,
    connected: false,
    lastError: null,
    lastEventAtMs: null,
    lastInboundAtMs: null,
    ...overrides,
  };
}

test("messaging account states and safe avatars match the Mac client", async ({ page }) => {
  const now = Date.now();
  const accounts: IMAccount[] = [
    account({ accountId: "error", displayName: "Error beats connecting", connecting: true, lastError: "  token 已失效  " }),
    account({ accountId: "incomplete", displayName: "Needs setup", configured: false, secretSet: false }),
    account({ accountId: "polling", displayName: "Polling account", polling: true, lastInboundAtMs: now - 30_000 }),
    account({ accountId: "activity", displayName: "Activity fallback", lastEventAtMs: now - 60_000 }),
    account({ accountId: "avatar", displayName: "Avatar Good", avatarData: pngAvatar }),
    account({ accountId: "remote", displayName: "Remote Avatar Rejected", avatarData: "https://example.invalid/avatar.png" }),
    account({ accountId: "svg", displayName: "SVG Avatar Rejected", avatarData: "data:image/svg+xml;base64,PHN2Zy8+" }),
  ];
  Object.assign(accounts[0], { secret: "MESSAGING_SECRET_CANARY" });

  await page.addInitScript(() => localStorage.setItem("mochiport.section", "messaging"));
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
    if (path === "api/v1/manage/im/accounts") return fulfillJson(route, { accounts });
    return fulfillJson(route, { error: `未处理的测试请求：${path}` }, 404);
  });

  await page.goto("/");

  const errorCard = page.locator(".account-card", { hasText: "Error beats connecting" });
  await expect(errorCard.locator(".status-pill")).toHaveText("连接异常");
  await expect(errorCard.locator(".account-card__expanded")).toHaveCount(0);
  await errorCard.getByRole("button", { name: "展开账号详情 Error beats connecting" }).click();
  await expect(errorCard.getByRole("button", { name: "收起账号详情 Error beats connecting" })).toHaveAttribute("aria-expanded", "true");
  await expect(errorCard.locator(".account-card__facts")).toContainText("配置完整");
  await expect(errorCard.locator(".account-card__facts")).toContainText("凭据已设置");
  await expect(page.getByText("MESSAGING_SECRET_CANARY", { exact: true })).toHaveCount(0);
  await expect(errorCard.locator(".account-card__error")).toHaveText(/token 已失效/);

  const incompleteCard = page.locator(".account-card", { hasText: "Needs setup" });
  await expect(incompleteCard.locator(".status-pill")).toHaveText("待配置");
  await incompleteCard.getByRole("button", { name: "展开账号详情 Needs setup" }).click();
  await expect(incompleteCard.locator(".account-card__facts")).toContainText("配置不完整");
  await expect(incompleteCard.locator(".account-card__facts")).toContainText("凭据未设置");
  await expect(incompleteCard.locator(".account-card__facts")).not.toContainText("轮询");

  const pollingCard = page.locator(".account-card", { hasText: "Polling account" });
  await pollingCard.getByRole("button", { name: "展开账号详情 Polling account" }).click();
  await expect(pollingCard.locator(".account-card__facts")).toContainText("轮询运行中");
  await expect(pollingCard.locator(".account-card__activity")).toContainText("最近收到消息：");

  const activityCard = page.locator(".account-card", { hasText: "Activity fallback" });
  await activityCard.getByRole("button", { name: "展开账号详情 Activity fallback" }).click();
  await expect(activityCard.locator(".account-card__activity")).toContainText("最近活动：");
  await activityCard.getByRole("button", { name: "收起账号详情 Activity fallback" }).click();
  await expect(activityCard.locator(".account-card__expanded")).toHaveCount(0);

  const validAvatar = page.locator(".account-card", { hasText: "Avatar Good" });
  await expect(validAvatar.locator(".account-avatar img")).toHaveAttribute("src", pngAvatar);
  await expect(validAvatar.locator(".account-avatar img")).toHaveAttribute("aria-hidden", "true");

  for (const displayName of ["Remote Avatar Rejected", "SVG Avatar Rejected"]) {
    const card = page.locator(".account-card", { hasText: displayName });
    await expect(card.locator(".account-avatar img")).toHaveCount(0);
    await expect(card.locator(".platform-badge")).toBeVisible();
  }
});
