import { expect, test, type Page, type Route } from "@playwright/test";

const managementOrigin = "http://127.0.0.1:3847";
const corsHeaders = {
  "access-control-allow-origin": "*",
  "access-control-allow-methods": "GET, POST, OPTIONS",
  "access-control-allow-headers": "content-type",
};

async function fulfillJson(route: Route, body: unknown, status = 200) {
  await route.fulfill({
    status,
    headers: { ...corsHeaders, "content-type": "application/json" },
    body: JSON.stringify(body),
  });
}

function dashboard() {
  return {
    service: {
      service: "threadrelay",
      apiMajor: 1,
      ready: true,
      instanceId: "response-validation-test",
      pid: 42,
      startedAtMs: 1,
    },
    bridgeRunning: true,
    remoteControlConnected: false,
    remoteControlHealthy: false,
    executionClients: {
      codexApp: { configured: false, connected: false },
      vscode: { configured: false, connected: false },
      cli: { configured: false, connected: false },
    },
    messageChannels: {
      telegram: { accountCount: 0, connectedAccountCount: 0 },
      feishu: { accountCount: 0, connectedAccountCount: 0 },
      wechat: { accountCount: 0, connectedAccountCount: 0 },
      wecom: { accountCount: 0, connectedAccountCount: 0 },
    },
    aiGatewayEnabled: false,
    aiGatewayProviderCount: 0,
    requestLoggingEnabled: false,
  };
}

function legacyDashboard() {
  const current = dashboard();
  const { executionClients: _executionClients, messageChannels: _messageChannels, ...legacy } = current;
  return {
    ...legacy,
    codexAppConfigured: true,
    imAccountCount: 3,
    connectedImAccountCount: 2,
  };
}

function lifecycle() {
  return {
    service: dashboard().service,
    executable: "C:\\Program Files\\MochiPort\\threadrelay.exe",
    executableSha256: null,
    configPath: "C:\\Users\\Test\\.threadrelay\\config.json",
    bind: "127.0.0.1:3847",
    runtime: { state: "ready", productVersion: "0.5.3", buildNumber: 503, apiMajor: 1 },
    protectedWorkItems: {
      aiGatewayRequests: 0,
      codexTurns: 0,
      imStreams: 0,
      pendingApprovals: 0,
      remoteControlRequests: 0,
      total: 0,
    },
    management: {
      state: "ready",
      mode: "local",
      canControl: true,
      installationId: "response-validation-test",
      leaseGeneration: null,
      leaseExpiresAtMs: null,
      managementTokenGeneration: 1,
    },
  };
}

test("a supported legacy v1 dashboard is normalized instead of rejected", async ({ page }) => {
  await page.route(`${managementOrigin}/**`, async (route) => {
    const request = route.request();
    if (request.method() === "OPTIONS") {
      await route.fulfill({ status: 204, headers: corsHeaders });
      return;
    }
    const path = new URL(request.url()).pathname.replace(/^\//, "");
    if (path === "healthz") return fulfillJson(route, { service: "threadrelay", apiMajor: 1, ready: true });
    if (path === "api/v1/manage/dashboard") return fulfillJson(route, legacyDashboard());
    if (path === "api/v1/manage/im/accounts") return fulfillJson(route, { accounts: [] });
    if (path === "api/v1/manage/lifecycle") return fulfillJson(route, lifecycle());
    if (path === "api/v1/manage/gateway/sub2api") return fulfillJson(route, { configured: false, baseUrl: "", secretSet: false });
    return fulfillJson(route, { error: `未处理的测试请求：${path}` }, 404);
  });

  await page.goto("/");
  await expect(page.getByText("运行正常", { exact: true }).first()).toBeVisible();
  await expect(page.getByText("2 个已连接")).toBeVisible();
  await expect(page.getByText("本地服务返回的响应格式错误")).toHaveCount(0);
});

test("legacy sessions with omitted display fields keep safe defaults", async ({ page }) => {
  await page.route(`${managementOrigin}/**`, async (route) => {
    const request = route.request();
    if (request.method() === "OPTIONS") {
      await route.fulfill({ status: 204, headers: corsHeaders });
      return;
    }
    const path = new URL(request.url()).pathname.replace(/^\//, "");
    if (path === "healthz") return fulfillJson(route, { service: "threadrelay", apiMajor: 1, ready: true });
    if (path === "api/v1/manage/dashboard") return fulfillJson(route, dashboard());
    if (path === "api/v1/manage/im/accounts") return fulfillJson(route, { accounts: [] });
    if (path === "api/v1/manage/lifecycle") return fulfillJson(route, lifecycle());
    if (path === "api/v1/manage/gateway/sub2api") return fulfillJson(route, { configured: false, baseUrl: "", secretSet: false });
    if (path === "api/v1/manage/sessions") {
      return fulfillJson(route, { ok: true, threads: [{ id: "legacy-session", name: "旧会话" }], providers: ["openai"], total: 1 });
    }
    return fulfillJson(route, { error: `未处理的测试请求：${path}` }, 404);
  });
  await page.addInitScript(() => localStorage.setItem("mochiport.section", "sessions"));

  await page.goto("/");
  await expect(page.getByText("旧会话", { exact: true })).toBeVisible();
  await expect(page.getByText("本地服务返回的响应格式错误")).toHaveCount(0);
});

async function installManagementMock(page: Page, invalidGatewayBody: string, contentType: string) {
  await page.route(`${managementOrigin}/**`, async (route) => {
    const request = route.request();
    if (request.method() === "OPTIONS") {
      await route.fulfill({ status: 204, headers: corsHeaders });
      return;
    }

    const path = new URL(request.url()).pathname.replace(/^\//, "");
    if (path === "healthz") {
      await fulfillJson(route, { service: "threadrelay", apiMajor: 1, ready: true });
      return;
    }
    if (path === "api/v1/manage/dashboard") {
      await fulfillJson(route, dashboard());
      return;
    }
    if (path === "api/v1/manage/im/accounts") {
      await fulfillJson(route, { accounts: [] });
      return;
    }
    if (path === "api/v1/manage/lifecycle") {
      await fulfillJson(route, lifecycle());
      return;
    }
    if (path === "api/v1/manage/gateway/sub2api") {
      await fulfillJson(route, { configured: false, baseUrl: "", secretSet: false });
      return;
    }
    if (path === "api/v1/manage/gateway") {
      await route.fulfill({
        status: 200,
        headers: { ...corsHeaders, "content-type": contentType },
        body: invalidGatewayBody,
      });
      return;
    }
    await fulfillJson(route, { error: `未处理的测试请求：${path}` }, 404);
  });
}

for (const response of [
  { name: "plain text", body: "gateway temporarily unavailable", contentType: "text/plain" },
  { name: "empty body", body: "", contentType: "application/json" },
  { name: "structurally invalid JSON", body: "{}", contentType: "application/json" },
]) {
  test(`a 2xx ${response.name} management response is shown as a format error`, async ({ page }) => {
    await installManagementMock(page, response.body, response.contentType);
    await page.addInitScript(() => localStorage.setItem("mochiport.section", "gateway"));

    await page.goto("/");
    await page.waitForLoadState("networkidle");

    await expect(page.getByRole("heading", { name: "AI 网关", exact: true })).toBeVisible();
    const formatError = page.getByRole("alert");
    await expect(formatError).toContainText("本地服务返回的响应格式错误");
    await expect(formatError).toContainText("GET /api/v1/manage/gateway");
  });
}
