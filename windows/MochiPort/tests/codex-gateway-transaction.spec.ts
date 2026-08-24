import { expect, test, type Page, type Route } from "@playwright/test";
import type { CodexStatus, Gateway } from "../src/api/types";

const managementOrigin = "http://127.0.0.1:3847";
const corsHeaders = {
  "access-control-allow-origin": "*",
  "access-control-allow-methods": "GET, POST, OPTIONS",
  "access-control-allow-headers": "content-type",
  "content-type": "application/json",
};

interface RecordedRequest {
  method: string;
  path: string;
  body?: unknown;
}

interface MockManagementState {
  gateway: Gateway;
  codex: CodexStatus;
  configureFails: boolean;
  requests: RecordedRequest[];
  directApiStatusGate?: Promise<void>;
  waitForDirectApiStatus?: boolean;
}

function gateway(enabled: boolean): Gateway {
  return {
    enabled,
    filterImageGenerationTool: true,
    requestLoggingEnabled: false,
    requestLogDetailsEnabled: true,
    codexVisibleModels: ["gpt-transaction-test", "claude-transaction-test"],
    providers: [],
  };
}

function codexStatus(connected: boolean): CodexStatus {
  return {
    codexHome: "C:\\Users\\Test\\.codex",
    configured: connected,
    configOk: connected,
    authOk: true,
    providerOk: true,
    guiConfigured: true,
    remoteControlSupported: true,
    remoteControlConfigured: connected,
    providers: [],
    imageGenerationEnabled: true,
    connectionMode: "remoteControl",
    providerMode: connected ? "threadrelay" : "direct-api",
    providerModeMessage: connected ? "Codex 通过 MochiPort 连接" : "Codex 使用直连 API",
    activeProvider: connected ? "mochiport" : "openai",
  };
}

async function fulfillJson(route: Route, body: unknown, status = 200) {
  await route.fulfill({ status, headers: corsHeaders, body: JSON.stringify(body) });
}

async function installManagementMock(page: Page, state: MockManagementState) {
  await page.route(`${managementOrigin}/**`, async (route) => {
    const request = route.request();
    if (request.method() === "OPTIONS") {
      await route.fulfill({ status: 204, headers: corsHeaders });
      return;
    }

    const path = new URL(request.url()).pathname.replace(/^\//, "");
    const postData = request.postData();
    const body = postData ? JSON.parse(postData) : undefined;
    state.requests.push({ method: request.method(), path, body });

    if (path === "healthz") {
      await fulfillJson(route, { service: "threadrelay", apiMajor: 1, ready: true });
      return;
    }
    if (path === "api/v1/manage/dashboard") {
      await fulfillJson(route, {
        service: { service: "threadrelay", apiMajor: 1, ready: true, instanceId: "test", pid: 42, startedAtMs: 1 },
        bridgeRunning: true,
        remoteControlConnected: true,
        remoteControlHealthy: true,
        executionClients: {
          codexApp: { configured: state.codex.configured, connected: state.codex.configured },
          vscode: { configured: false, connected: false },
          cli: { configured: false, connected: false },
        },
        messageChannels: {
          telegram: { accountCount: 0, connectedAccountCount: 0 },
          feishu: { accountCount: 0, connectedAccountCount: 0 },
          wechat: { accountCount: 0, connectedAccountCount: 0 },
          wecom: { accountCount: 0, connectedAccountCount: 0 },
        },
        aiGatewayEnabled: state.gateway.enabled,
        aiGatewayProviderCount: 0,
        requestLoggingEnabled: state.gateway.requestLoggingEnabled,
      });
      return;
    }
    if (path === "api/v1/manage/im/accounts") {
      await fulfillJson(route, { accounts: [] });
      return;
    }
    if (path === "api/v1/manage/lifecycle") {
      await fulfillJson(route, {});
      return;
    }
    if (path === "api/v1/manage/codex/status") {
      if (state.waitForDirectApiStatus) {
        await state.directApiStatusGate;
        state.waitForDirectApiStatus = false;
      }
      await fulfillJson(route, state.codex);
      return;
    }
    if (path === "api/v1/manage/gateway" && request.method() === "GET") {
      await fulfillJson(route, state.gateway);
      return;
    }
    if (path === "api/v1/manage/codex/enhanced/operation") {
      await fulfillJson(route, { ok: true, operation: null });
      return;
    }
    if (path === "api/v1/manage/gateway/settings") {
      state.gateway = { ...state.gateway, ...(body as Partial<Gateway>) };
      await fulfillJson(route, { ok: true, gateway: state.gateway });
      return;
    }
    if (path === "api/v1/manage/codex/configure") {
      if (state.configureFails) {
        await fulfillJson(route, { error: "Codex 配置失败（网络 mock）" }, 500);
      } else {
        state.codex = codexStatus(true);
        await fulfillJson(route, { ok: true });
      }
      return;
    }
    if (path === "api/v1/manage/codex/uninstall") {
      state.codex = codexStatus(false);
      await fulfillJson(route, { ok: true });
      return;
    }
    if (path === "api/v1/manage/codex/direct-api-mode") {
      state.codex = codexStatus(false);
      state.waitForDirectApiStatus = true;
      await fulfillJson(route, { ok: true });
      return;
    }

    await fulfillJson(route, { error: `未处理的测试请求：${path}` }, 404);
  });
}

async function openCodexPage(page: Page) {
  await page.addInitScript(() => localStorage.setItem("mochiport.section", "codex"));
  await page.goto("/");
  await page.waitForLoadState("networkidle");
  await expect(page.getByRole("heading", { name: "Codex 接入" })).toBeVisible();
}

test("Codex configure rolls Gateway back on failure and uninstall turns it off", async ({ page }) => {
  const state: MockManagementState = {
    gateway: gateway(false),
    codex: codexStatus(false),
    configureFails: true,
    requests: [],
  };
  await installManagementMock(page, state);
  await openCodexPage(page);

  const connectionSwitch = page.getByRole("switch", { name: "连接 MochiPort" });
  await expect(connectionSwitch).toHaveAttribute("aria-checked", "false");
  state.requests.length = 0;

  await test.step("a failed configure restores the previous Gateway enabled value", async () => {
    await connectionSwitch.click();
    await expect(page.getByRole("alert")).toContainText("Codex 配置失败（网络 mock）");
    await expect(connectionSwitch).toHaveAttribute("aria-checked", "false");

    const writes = state.requests.filter((request) => request.method === "POST");
    expect(writes.map((request) => request.path)).toEqual([
      "api/v1/manage/gateway/settings",
      "api/v1/manage/codex/configure",
      "api/v1/manage/gateway/settings",
    ]);
    expect(writes.filter((request) => request.path.endsWith("gateway/settings")).map((request) => request.body)).toEqual([
      {
        enabled: true,
        filterImageGenerationTool: true,
        requestLoggingEnabled: false,
        requestLogDetailsEnabled: true,
        codexVisibleModels: ["gpt-transaction-test", "claude-transaction-test"],
      },
      {
        enabled: false,
        filterImageGenerationTool: true,
        requestLoggingEnabled: false,
        requestLogDetailsEnabled: true,
        codexVisibleModels: ["gpt-transaction-test", "claude-transaction-test"],
      },
    ]);
    const rollbackIndex = state.requests.findIndex((request, index) =>
      index > 0 && request.path === "api/v1/manage/gateway/settings" && (request.body as { enabled?: boolean }).enabled === false,
    );
    const refreshedPaths = state.requests.slice(rollbackIndex + 1).map((request) => request.path);
    expect(refreshedPaths).toEqual(expect.arrayContaining([
      "api/v1/manage/codex/status",
      "api/v1/manage/gateway",
    ]));
  });

  await test.step("a successful uninstall disables Gateway and refreshes both states", async () => {
    state.configureFails = false;
    state.gateway = gateway(true);
    state.codex = codexStatus(true);
    await page.reload();
    await page.waitForLoadState("networkidle");
    await expect(connectionSwitch).toHaveAttribute("aria-checked", "true");
    state.requests.length = 0;

    await connectionSwitch.click();
    const dialog = page.getByRole("dialog", { name: "断开 Codex？" });
    await dialog.getByRole("button", { name: "断开连接" }).click();

    await expect(dialog).not.toBeVisible();
    await expect(connectionSwitch).toHaveAttribute("aria-checked", "false");
    await expect(page.getByRole("status")).toContainText("已恢复原来的 Codex 设置，MochiPort 已关闭");

    const writes = state.requests.filter((request) => request.method === "POST");
    expect(writes.map((request) => request.path)).toEqual([
      "api/v1/manage/codex/uninstall",
      "api/v1/manage/gateway/settings",
    ]);
    expect(writes[1].body).toEqual({
      enabled: false,
      filterImageGenerationTool: true,
      requestLoggingEnabled: false,
      requestLogDetailsEnabled: true,
      codexVisibleModels: ["gpt-transaction-test", "claude-transaction-test"],
    });
    const refreshedPaths = state.requests.slice(2).map((request) => request.path);
    expect(refreshedPaths).toEqual(expect.arrayContaining([
      "api/v1/manage/codex/status",
      "api/v1/manage/gateway",
    ]));
  });
});

test("Codex is connected only when the Mac-compatible readiness checks pass", async ({ page }) => {
  const state: MockManagementState = {
    gateway: gateway(true),
    codex: {
      ...codexStatus(true),
      guiConfigured: false,
      guiError: "桌面环境尚未配置",
      remoteControlSupported: false,
      remoteControlConfigured: false,
    },
    configureFails: false,
    requests: [],
  };
  await installManagementMock(page, state);
  await openCodexPage(page);

  const connectionSwitch = page.getByRole("switch", { name: "连接 MochiPort" });
  await expect(connectionSwitch).toHaveAttribute("aria-checked", "true");
  await expect(page.locator(".codex-connection-card")).toContainText("需处理");
  await expect(page.locator(".check-row", { hasText: "桌面控制" })).toContainText("桌面环境尚未配置");
  await expect(page.locator(".check-row", { hasText: "远程控制" })).toContainText("当前 Codex 版本不支持，连接不受影响");

  state.codex.guiConfigured = true;
  state.codex.guiError = null;
  await page.reload();
  await page.waitForLoadState("networkidle");
  await expect(connectionSwitch).toHaveAttribute("aria-checked", "true");
  await expect(page.locator(".codex-connection-card")).toContainText("已连接");
});

test("direct API mode guides connection instead of allowing enhanced launch", async ({ page }) => {
  const state: MockManagementState = {
    gateway: gateway(true),
    codex: {
      ...codexStatus(true),
      configured: true,
      providerMode: "direct-api",
      providerModeMessage: "Codex 使用直连 API",
      activeProvider: "openai",
    },
    configureFails: false,
    requests: [],
  };
  await installManagementMock(page, state);
  await openCodexPage(page);

  await expect(page.getByRole("button", { name: "增强模式启动 Codex" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "连接 MochiPort", exact: true })).toBeVisible();
  await expect(page.getByText("当前使用直连 API。请先连接 MochiPort，再使用增强启动。", { exact: true })).toBeVisible();
  expect(state.requests.some((request) => request.path === "api/v1/manage/codex/enhanced/operation/start")).toBe(false);
});

test("an unconfigured Codex reveals enhanced launch only after connection", async ({ page }) => {
  const state: MockManagementState = {
    gateway: gateway(false),
    codex: {
      ...codexStatus(false),
      providerMode: "unknown",
      providerModeMessage: "尚未接入 MochiPort",
      activeProvider: null,
    },
    configureFails: false,
    requests: [],
  };
  await installManagementMock(page, state);
  await openCodexPage(page);

  await expect(page.getByRole("button", { name: "增强模式启动 Codex" })).toHaveCount(0);
  const connectButton = page.getByRole("button", { name: "连接 MochiPort", exact: true });
  await expect(connectButton).toBeEnabled();
  await expect(page.getByText("请先完成 MochiPort 接入，再使用增强启动。", { exact: true })).toBeVisible();

  await connectButton.click();
  await expect(page.getByRole("button", { name: "增强模式启动 Codex" })).toBeVisible();
  expect(state.requests.some((request) => request.path === "api/v1/manage/codex/configure")).toBe(true);
  expect(state.requests.some((request) => request.path === "api/v1/manage/codex/enhanced/operation/start")).toBe(false);
});

test("enhanced launch stays disabled while switching to direct API mode", async ({ page }) => {
  let releaseDirectApiStatus = () => {};
  const directApiStatusGate = new Promise<void>((resolve) => { releaseDirectApiStatus = resolve; });
  const state: MockManagementState = {
    gateway: gateway(true),
    codex: codexStatus(true),
    configureFails: false,
    requests: [],
    directApiStatusGate,
  };
  await installManagementMock(page, state);
  await openCodexPage(page);

  const enhancedLaunch = page.getByRole("button", { name: "增强模式启动 Codex" });
  await expect(enhancedLaunch).toBeEnabled();
  await page.getByRole("button", { name: "切换到直连 API" }).click();

  await expect(enhancedLaunch).toBeDisabled();
  expect(state.requests.some((request) => request.method === "POST" && request.path === "api/v1/manage/codex/direct-api-mode")).toBe(true);
  expect(state.requests.some((request) => request.path === "api/v1/manage/codex/enhanced/operation/start")).toBe(false);

  releaseDirectApiStatus();
  await expect(enhancedLaunch).toHaveCount(0);
  await expect(page.getByRole("button", { name: "连接 MochiPort", exact: true })).toBeVisible();
});
