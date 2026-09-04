import { expect, test, type Page, type Route } from "@playwright/test";
import { fixtureAccounts, fixtureDashboard, fixtureLifecycle } from "../src/api/fixtures";

const managementOrigin = "http://127.0.0.1:3847";
const corsHeaders = {
  "access-control-allow-origin": "*",
  "access-control-allow-methods": "GET, POST, OPTIONS",
  "access-control-allow-headers": "content-type",
  "content-type": "application/json",
};

const testQr = (name: string) => `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><title>${name}</title><rect width="10" height="10"/></svg>`;

async function fulfillJson(route: Route, body: unknown, status = 200) {
  await route.fulfill({ status, headers: corsHeaders, body: JSON.stringify(body) });
}

async function setInitialMessagingSection(page: Page) {
  await page.addInitScript(() => localStorage.setItem("mochiport.section", "messaging"));
}

type OnboardingRouteHandler = (route: Route, path: string) => Promise<boolean>;

async function installManagementRoutes(
  page: Page,
  handleOnboarding: OnboardingRouteHandler,
  onAccountsRead?: () => void,
) {
  await page.route(`${managementOrigin}/**`, async (route) => {
    if (route.request().method() === "OPTIONS") {
      await route.fulfill({ status: 204, headers: corsHeaders });
      return;
    }

    const path = new URL(route.request().url()).pathname.replace(/^\//, "");
    if (await handleOnboarding(route, path)) return;
    if (path === "healthz") {
      await fulfillJson(route, { service: "mochiport", apiMajor: 1, ready: true });
      return;
    }
    if (path === "api/v1/manage/dashboard") {
      await fulfillJson(route, fixtureDashboard);
      return;
    }
    if (path === "api/v1/manage/im/accounts") {
      onAccountsRead?.();
      await fulfillJson(route, { accounts: fixtureAccounts });
      return;
    }
    if (path === "api/v1/manage/lifecycle") {
      await fulfillJson(route, fixtureLifecycle);
      return;
    }
    await fulfillJson(route, { error: `未处理的测试请求：${path}` }, 404);
  });
}

async function openOnboarding(page: Page, platform: "Telegram" | "飞书" | "微信") {
  await page.getByRole("button", { name: "连接渠道" }).click();
  const dialog = page.getByRole("dialog", { name: "连接消息渠道" });
  await expect(dialog).toBeVisible();
  if (platform !== "Telegram") {
    await dialog.getByRole("tab", { name: platform === "微信" ? /^微 微信 / : /飞书/ }).click();
  }
  return dialog;
}

function feishuStart(deviceCode = "feishu-device", interval = 1) {
  return {
    verificationUri: "https://example.test/verify",
    verificationUriComplete: `https://example.test/verify?code=${deviceCode}`,
    deviceCode,
    expiresIn: 120,
    interval,
    qrSvg: testQr(deviceCode),
  };
}

function wechatStart(sessionKey = "wechat-session") {
  return {
    sessionKey,
    qrcodeUrl: `https://example.test/qr/${sessionKey}`,
    qrSvg: testQr(sessionKey),
    expiresIn: 120,
  };
}

test("a poll from a closed modal cannot finish a newly opened scan", async ({ page }) => {
  let oldPollStarted = false;
  let releaseOldPoll!: () => void;
  const oldPollGate = new Promise<void>((resolve) => { releaseOldPoll = resolve; });

  await setInitialMessagingSection(page);
  await installManagementRoutes(page, async (route, path) => {
    if (path === "api/v1/manage/im/onboarding/feishu/start") {
      await fulfillJson(route, feishuStart("old-feishu"));
      return true;
    }
    if (path === "api/v1/manage/im/onboarding/feishu/poll") {
      oldPollStarted = true;
      await oldPollGate;
      await fulfillJson(route, { done: true, appId: "old-app", displayName: "旧飞书账号", error: null, errorDescription: null });
      return true;
    }
    if (path === "api/v1/manage/im/onboarding/wechat/start") {
      await fulfillJson(route, wechatStart("new-wechat"));
      return true;
    }
    if (path === "api/v1/manage/im/onboarding/wechat/poll") {
      await fulfillJson(route, { done: false, status: "wait", needVerifyCode: false, error: null });
      return true;
    }
    return false;
  });

  await page.goto("/");
  await expect(page.getByRole("heading", { name: "消息渠道", exact: true, level: 1 })).toBeVisible();
  let dialog = await openOnboarding(page, "飞书");
  await dialog.getByRole("button", { name: "生成二维码" }).click();
  await expect(dialog.getByRole("img", { name: "飞书 连接二维码" })).toBeVisible();
  await expect.poll(() => oldPollStarted, { timeout: 4_000 }).toBe(true);

  await dialog.getByRole("button", { name: "关闭" }).click();
  await expect(dialog).toBeHidden();
  dialog = await openOnboarding(page, "微信");
  await dialog.getByRole("button", { name: "生成二维码" }).click();
  await expect(dialog.getByRole("img", { name: "微信 连接二维码" })).toBeVisible();

  releaseOldPoll();
  await page.waitForTimeout(300);
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("img", { name: "微信 连接二维码" })).toBeVisible();
});

test("switching platform clears the already scheduled next poll", async ({ page }) => {
  let feishuPolls = 0;

  await setInitialMessagingSection(page);
  await installManagementRoutes(page, async (route, path) => {
    if (path === "api/v1/manage/im/onboarding/feishu/start") {
      await fulfillJson(route, feishuStart("timer-feishu"));
      return true;
    }
    if (path === "api/v1/manage/im/onboarding/feishu/poll") {
      feishuPolls += 1;
      await fulfillJson(route, { done: false, appId: null, displayName: null, error: "authorization_pending", errorDescription: null });
      return true;
    }
    return false;
  });

  await page.goto("/");
  const dialog = await openOnboarding(page, "飞书");
  await dialog.getByRole("button", { name: "生成二维码" }).click();
  await expect.poll(() => feishuPolls, { timeout: 4_000 }).toBe(1);
  await dialog.getByRole("tab", { name: /^微 微信 / }).click();
  await page.waitForTimeout(1_300);

  expect(feishuPolls).toBe(1);
  await expect(dialog.getByRole("heading", { name: "生成一次性二维码" })).toBeVisible();
});

test("feishu slow_down adds five seconds to every subsequent poll interval", async ({ page }) => {
  const pollTimes: number[] = [];

  await setInitialMessagingSection(page);
  await installManagementRoutes(page, async (route, path) => {
    if (path === "api/v1/manage/im/onboarding/feishu/start") {
      await fulfillJson(route, feishuStart("slow-feishu"));
      return true;
    }
    if (path === "api/v1/manage/im/onboarding/feishu/poll") {
      pollTimes.push(Date.now());
      await fulfillJson(route, pollTimes.length === 1
        ? { done: false, appId: null, displayName: null, error: "slow_down", errorDescription: "please retry more slowly" }
        : { done: false, appId: null, displayName: null, error: "authorization_pending", errorDescription: null });
      return true;
    }
    return false;
  });

  await page.goto("/");
  const dialog = await openOnboarding(page, "飞书");
  await dialog.getByRole("button", { name: "生成二维码" }).click();
  await expect.poll(() => pollTimes.length, { timeout: 9_000 }).toBeGreaterThanOrEqual(2);

  expect(pollTimes[1] - pollTimes[0]).toBeGreaterThanOrEqual(5_500);
  await expect(page.getByText(/slow_down|please retry more slowly/)).toHaveCount(0);
  if (await dialog.isVisible()) await dialog.getByRole("button", { name: "关闭" }).click();
});

test("a verification response cannot complete a different scan generation", async ({ page }) => {
  let verifyPollStarted = false;
  let releaseVerifyPoll!: () => void;
  const verifyPollGate = new Promise<void>((resolve) => { releaseVerifyPoll = resolve; });

  await setInitialMessagingSection(page);
  await installManagementRoutes(page, async (route, path) => {
    if (path === "api/v1/manage/im/onboarding/wechat/start") {
      await fulfillJson(route, wechatStart("verify-wechat"));
      return true;
    }
    if (path === "api/v1/manage/im/onboarding/wechat/poll") {
      const body = route.request().postDataJSON() as { verifyCode?: string };
      if (body.verifyCode) {
        verifyPollStarted = true;
        await verifyPollGate;
        await fulfillJson(route, { done: true, status: "confirmed", needVerifyCode: false, accountId: "stale-wechat", alreadyConnected: false, error: null });
      } else {
        await fulfillJson(route, { done: false, status: "need_verifycode", needVerifyCode: true, error: null });
      }
      return true;
    }
    if (path === "api/v1/manage/im/onboarding/feishu/start") {
      await fulfillJson(route, feishuStart("new-feishu"));
      return true;
    }
    if (path === "api/v1/manage/im/onboarding/feishu/poll") {
      await fulfillJson(route, { done: false, appId: null, displayName: null, error: "authorization_pending", errorDescription: null });
      return true;
    }
    return false;
  });

  await page.goto("/");
  const dialog = await openOnboarding(page, "微信");
  await dialog.getByRole("button", { name: "生成二维码" }).click();
  const codeInput = dialog.getByPlaceholder("输入验证码");
  await expect(codeInput).toBeVisible({ timeout: 5_000 });
  await codeInput.fill("123456");
  await dialog.getByRole("button", { name: "继续" }).click();
  await expect.poll(() => verifyPollStarted).toBe(true);

  await dialog.getByRole("tab", { name: /飞书/ }).click();
  await dialog.getByRole("button", { name: "生成二维码" }).click();
  await expect(dialog.getByRole("img", { name: "飞书 连接二维码" })).toBeVisible();
  releaseVerifyPoll();
  await page.waitForTimeout(300);

  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("img", { name: "飞书 连接二维码" })).toBeVisible();
});

test("double-triggering regenerate starts only one replacement session", async ({ page }) => {
  let startCalls = 0;
  let releaseReplacement!: () => void;
  const replacementGate = new Promise<void>((resolve) => { releaseReplacement = resolve; });

  await setInitialMessagingSection(page);
  await installManagementRoutes(page, async (route, path) => {
    if (path === "api/v1/manage/im/onboarding/feishu/start") {
      startCalls += 1;
      if (startCalls > 1) await replacementGate;
      await fulfillJson(route, feishuStart(`regenerate-${startCalls}`));
      return true;
    }
    if (path === "api/v1/manage/im/onboarding/feishu/poll") {
      await fulfillJson(route, { done: false, appId: null, displayName: null, error: "expired_token", errorDescription: "二维码已过期" });
      return true;
    }
    return false;
  });

  await page.goto("/");
  const dialog = await openOnboarding(page, "飞书");
  await dialog.getByRole("button", { name: "生成二维码" }).click();
  const regenerate = dialog.getByRole("button", { name: "重新生成" });
  await expect(regenerate).toBeVisible({ timeout: 4_000 });
  await regenerate.evaluate((button) => {
    (button as HTMLButtonElement).click();
    (button as HTMLButtonElement).click();
  });

  await expect.poll(() => startCalls).toBe(2);
  await page.waitForTimeout(250);
  expect(startCalls).toBe(2);
  releaseReplacement();
  await expect(dialog.getByRole("img", { name: "飞书 连接二维码" })).toBeVisible();
});

test("manual credential failures remain visible inside the onboarding modal", async ({ page }) => {
  await setInitialMessagingSection(page);
  await installManagementRoutes(page, async (route, path) => {
    if (path === "api/v1/manage/im/account/telegram") {
      await fulfillJson(route, { error: "Bot token 无效" }, 400);
      return true;
    }
    return false;
  });

  await page.goto("/");
  const dialog = await openOnboarding(page, "Telegram");
  await dialog.getByLabel("Bot token").fill("invalid-token");
  await dialog.getByRole("button", { name: "验证并连接" }).click();

  await expect(dialog).toBeVisible();
  await expect(dialog.getByText("Bot token 无效", { exact: true })).toBeVisible();
});
