import { expect, test, type Page, type Route } from "@playwright/test";
import {
  fixtureAccounts,
  fixtureDashboard,
  fixtureGateway,
  fixtureLifecycle,
  fixtureSettings,
  fixtureSub2ApiPool,
} from "../src/api/fixtures";
import type { Gateway, GatewayProviderModelsResponse, Settings } from "../src/api/types";

const managementOrigin = "http://127.0.0.1:3847";
const corsHeaders = {
  "access-control-allow-origin": "*",
  "access-control-allow-methods": "GET, POST, OPTIONS",
  "access-control-allow-headers": "content-type",
  "content-type": "application/json",
};

interface ManagementState {
  gateway: Gateway;
  settings: Settings;
  writes: Array<{ path: string; body: Record<string, unknown> }>;
  settingsGetGate?: Promise<void>;
  failSettingsGet?: boolean;
  failProviderSave?: boolean;
  failProviderDelete?: boolean;
  failSub2ApiDisconnect?: boolean;
  sub2ApiConfigured?: boolean;
  providerModelsResponse?: GatewayProviderModelsResponse;
}

async function fulfillJson(route: Route, body: unknown, status = 200) {
  await route.fulfill({ status, headers: corsHeaders, body: JSON.stringify(body) });
}

async function installManagementMock(page: Page, state: ManagementState) {
  await page.route(`${managementOrigin}/**`, async (route) => {
    const request = route.request();
    if (request.method() === "OPTIONS") {
      await route.fulfill({ status: 204, headers: corsHeaders });
      return;
    }
    const path = new URL(request.url()).pathname.replace(/^\//, "");
    const body = request.postDataJSON() as Record<string, unknown> | null;
    if (request.method() === "POST") state.writes.push({ path, body: body ?? {} });

    if (path === "healthz") return fulfillJson(route, { service: "threadrelay", apiMajor: 1, ready: true });
    if (path === "api/v1/manage/dashboard") return fulfillJson(route, fixtureDashboard);
    if (path === "api/v1/manage/im/accounts") return fulfillJson(route, { accounts: fixtureAccounts });
    if (path === "api/v1/manage/lifecycle") return fulfillJson(route, fixtureLifecycle);
    if (path === "api/v1/manage/gateway" && request.method() === "GET") return fulfillJson(route, state.gateway);
    if (path === "api/v1/manage/gateway/sub2api" && request.method() === "GET") {
      const configured = state.sub2ApiConfigured ?? false;
      return fulfillJson(route, {
        configured,
        baseUrl: configured ? "https://sub2api.example.com" : "",
        secretSet: configured,
      });
    }
    if (path === "api/v1/manage/gateway/sub2api/accounts") {
      return fulfillJson(route, { ok: true, pool: fixtureSub2ApiPool });
    }
    if (path === "api/v1/manage/gateway/sub2api/disconnect") {
      if (state.failSub2ApiDisconnect) return fulfillJson(route, { error: "断开 Sub2API 失败" }, 502);
      state.sub2ApiConfigured = false;
      return fulfillJson(route, { ok: true, sub2api: { configured: false, baseUrl: "", secretSet: false } });
    }
    if (path === "api/v1/manage/gateway/provider-templates") return fulfillJson(route, { templates: [] });
    if (path === "api/v1/manage/codex/models/catalog") {
      return fulfillJson(route, {
        models: [
          { id: "gpt-5.4", displayName: "GPT-5.4" },
          { id: "gpt-5.4-mini", displayName: "GPT-5.4 mini" },
        ],
      });
    }
    if (path === "api/v1/manage/gateway/provider/models/fetch") {
      return fulfillJson(route, state.providerModelsResponse ?? {
        ok: true,
        models: ["gpt-5.4-mini", "manual-first", "fetched-last"],
        attempts: [],
      });
    }
    if (path === "api/v1/manage/settings" && request.method() === "GET") {
      await state.settingsGetGate;
      if (state.failSettingsGet) return fulfillJson(route, { error: "后台设置读取失败" }, 503);
      return fulfillJson(route, state.settings);
    }
    if (path === "api/v1/manage/gateway/provider/delete") {
      if (state.failProviderDelete) return fulfillJson(route, { error: "删除模型服务失败" }, 502);
      const name = String(body?.name ?? "");
      state.gateway = { ...state.gateway, providers: state.gateway.providers.filter((provider) => provider.name !== name) };
      return fulfillJson(route, { ok: true, gateway: state.gateway });
    }
    if (path === "api/v1/manage/gateway/provider") {
      if (state.failProviderSave) return fulfillJson(route, { error: "上游保存失败" }, 502);
      const originalName = String(body?.originalName ?? body?.name ?? "");
      const provider = {
        name: String(body?.name ?? ""),
        enabled: Boolean(body?.enabled),
        providerType: String(body?.providerType ?? ""),
        compatibility: body?.compatibility as string | null,
        baseUrl: String(body?.baseUrl ?? ""),
        modelsUrl: body?.modelsUrl as string | null,
        models: body?.models as string[],
        modelAliases: body?.modelAliases as Record<string, string>,
        promptCacheRetention: body?.promptCacheRetention as string | null,
        weight: Number(body?.weight),
        timeoutSecs: Number(body?.timeoutSecs),
        secretSet: body?.clearApiKey !== true,
      };
      state.gateway = {
        ...state.gateway,
        providers: [...state.gateway.providers.filter((entry) => entry.name !== originalName), provider],
      };
      return fulfillJson(route, { ok: true, gateway: state.gateway });
    }
    if (path === "api/v1/manage/settings" && request.method() === "POST") {
      const nextProxyUrl = typeof body?.outboundProxyUrl === "string"
        ? body.outboundProxyUrl
        : state.settings.outboundProxy.url;
      let credentialSet = state.settings.outboundProxy.credentialSet;
      if (typeof body?.outboundProxyUrl === "string") {
        try {
          const parsed = new URL(body.outboundProxyUrl);
          credentialSet = Boolean(parsed.username || parsed.password);
        } catch {
          credentialSet = false;
        }
      }
      state.settings = {
        ...state.settings,
        language: body?.language as string | null,
        theme: body?.theme as string | null,
        localConnectionMode: String(body?.localConnectionMode),
        outboundProxy: {
          ...state.settings.outboundProxy,
          mode: String(body?.outboundProxyMode),
          url: nextProxyUrl,
          credentialSet,
        },
      };
      return fulfillJson(route, { ok: true, settings: state.settings });
    }
    if (path === "api/v1/manage/gateway/settings" && request.method() === "POST") {
      state.gateway = {
        ...state.gateway,
        enabled: Boolean(body?.enabled),
        filterImageGenerationTool: Boolean(body?.filterImageGenerationTool),
        requestLoggingEnabled: Boolean(body?.requestLoggingEnabled),
        requestLogDetailsEnabled: Boolean(body?.requestLogDetailsEnabled),
        codexVisibleModels: body?.codexVisibleModels as string[],
      };
      return fulfillJson(route, { ok: true, gateway: state.gateway });
    }

    return fulfillJson(route, { error: `未处理的测试请求：${path}` }, 404);
  });
}

test("provider aliases, retention, and API-key removal are validated and persisted", async ({ page }) => {
  const state: ManagementState = {
    gateway: structuredClone(fixtureGateway),
    settings: structuredClone(fixtureSettings),
    writes: [],
  };
  await page.addInitScript(() => localStorage.setItem("mochiport.section", "gateway"));
  await installManagementMock(page, state);
  await page.goto("/");
  await page.getByRole("tab", { name: "模型服务", exact: true }).click();

  const openAiCard = page.locator(".provider-card", { hasText: "OpenAI" });
  await openAiCard.getByRole("button", { name: "编辑" }).click();
  const editor = page.getByRole("dialog", { name: "编辑模型服务" });
  await editor.getByLabel("Prompt Cache Retention").fill("24h");
  await editor.getByLabel("模型 ID").fill("claude-opus-4-8\nclaude-sonnet-4-6\nsonnet-4.6");
  await editor.getByRole("button", { name: "添加映射" }).click();
  await editor.getByLabel("对外别名").fill("gpt-codex");
  await editor.getByLabel("上游模型").fill("gpt-5.4");

  await editor.getByRole("button", { name: "添加映射" }).click();
  await editor.getByLabel("对外别名").nth(1).fill("gpt-codex");
  await editor.getByLabel("上游模型").nth(1).fill("gpt-5.4-mini");
  await expect(editor.getByText("存在重复的对外别名，请先去重。")).toBeVisible();
  await expect(editor.getByRole("button", { name: "保存模型服务" })).toBeDisabled();

  await editor.getByLabel("对外别名").nth(1).fill("gpt-codex-mini");
  await editor.getByRole("switch", { name: "清除已保存的 API Key" }).click();
  await editor.getByRole("button", { name: "保存模型服务" }).click();
  await expect(editor).not.toBeVisible();

  const write = state.writes.find((entry) => entry.path === "api/v1/manage/gateway/provider");
  expect(write?.body).toMatchObject({
    originalName: "OpenAI",
    name: "OpenAI",
    compatibility: null,
    promptCacheRetention: "24h",
    clearApiKey: true,
    apiKey: null,
    modelAliases: {
      "gpt-codex": "gpt-5.4",
      "gpt-codex-mini": "gpt-5.4-mini",
      "opus-4.8": "claude-opus-4-8",
    },
  });
  await expect(openAiCard).toContainText("缺少密钥");
});

test("fetching provider models appends new IDs without replacing manual edits", async ({ page }) => {
  const state: ManagementState = {
    gateway: structuredClone(fixtureGateway),
    settings: structuredClone(fixtureSettings),
    writes: [],
  };
  await page.addInitScript(() => localStorage.setItem("mochiport.section", "gateway"));
  await installManagementMock(page, state);
  await page.goto("/");
  await page.getByRole("tab", { name: "模型服务", exact: true }).click();
  await page.locator(".provider-card", { hasText: "OpenAI" }).getByRole("button", { name: "编辑" }).click();

  const editor = page.getByRole("dialog", { name: "编辑模型服务" });
  await editor.getByLabel("模型 ID").fill("manual-first\ngpt-5.4");
  await editor.getByRole("button", { name: "从服务商获取模型" }).click();
  await expect(editor.getByLabel("模型 ID")).toHaveValue("manual-first\ngpt-5.4\ngpt-5.4-mini\nfetched-last");
  await expect(editor.getByText("已获取 3 个模型，新增 2 个", { exact: true })).toBeVisible();
});

test("provider model discovery shows every attempted URL, status, and response preview", async ({ page }) => {
  const state: ManagementState = {
    gateway: structuredClone(fixtureGateway),
    settings: structuredClone(fixtureSettings),
    writes: [],
    providerModelsResponse: {
      ok: false,
      models: [],
      attempts: [
        { url: "https://models.example.test/v1/models", status: 401, error: null, preview: "{\"error\":\"invalid key\"}" },
        { url: "https://models.example.test/models", status: 404, error: null, preview: "not found" },
        { url: "https://fallback.example.test/models", status: null, error: "连接超时", preview: null },
      ],
    },
  };
  await page.addInitScript(() => localStorage.setItem("mochiport.section", "gateway"));
  await installManagementMock(page, state);
  await page.goto("/");
  await page.getByRole("tab", { name: "模型服务", exact: true }).click();
  await page.locator(".provider-card", { hasText: "OpenAI" }).getByRole("button", { name: "编辑" }).click();

  const editor = page.getByRole("dialog", { name: "编辑模型服务" });
  await editor.getByRole("button", { name: "从服务商获取模型" }).click();
  await expect(editor.getByText("获取模型失败", { exact: true })).toBeVisible();
  await editor.getByText("查看获取详情", { exact: true }).click();

  const details = editor.locator(".provider-fetch-attempts");
  await expect(details).toContainText("3 次尝试");
  await expect(details).toContainText("https://models.example.test/v1/models — HTTP 401 — {\"error\":\"invalid key\"}");
  await expect(details).toContainText("https://models.example.test/models — HTTP 404 — not found");
  await expect(details).toContainText("https://fallback.example.test/models — 连接超时");
});

test("provider save failures remain visible inside the editor", async ({ page }) => {
  const state: ManagementState = {
    gateway: structuredClone(fixtureGateway),
    settings: structuredClone(fixtureSettings),
    writes: [],
    failProviderSave: true,
  };
  await page.addInitScript(() => localStorage.setItem("mochiport.section", "gateway"));
  await installManagementMock(page, state);
  await page.goto("/");
  await page.getByRole("tab", { name: "模型服务", exact: true }).click();
  await page.locator(".provider-card", { hasText: "OpenAI" }).getByRole("button", { name: "编辑" }).click();

  const editor = page.getByRole("dialog", { name: "编辑模型服务" });
  await editor.getByRole("button", { name: "保存模型服务" }).click();
  await expect(editor).toBeVisible();
  await expect(editor.getByText("上游保存失败", { exact: true })).toBeVisible();
});

test("provider numeric defaults, boundaries, and validation match the daemon contract", async ({ page }) => {
  const state: ManagementState = {
    gateway: structuredClone(fixtureGateway),
    settings: structuredClone(fixtureSettings),
    writes: [],
  };
  await page.addInitScript(() => localStorage.setItem("mochiport.section", "gateway"));
  await installManagementMock(page, state);
  await page.goto("/");
  await page.getByRole("tab", { name: "模型服务", exact: true }).click();
  await page.getByRole("button", { name: "添加服务", exact: true }).click();

  let editor = page.getByRole("dialog", { name: "添加模型服务" });
  const weight = editor.getByLabel("路由权重");
  const timeout = editor.getByLabel("超时（秒）");
  await expect(weight).toHaveValue("100");
  await expect(weight).toHaveAttribute("min", "1");
  await expect(weight).toHaveAttribute("max", "10000");
  await expect(timeout).toHaveValue("600");
  await expect(timeout).toHaveAttribute("min", "1");
  await expect(timeout).toHaveAttribute("max", "3600");

  await editor.getByLabel("名称").fill("边界服务");
  await weight.fill("1");
  await timeout.fill("1");
  await editor.getByRole("button", { name: "保存模型服务" }).click();
  await expect(editor).not.toBeVisible();
  expect(state.writes.filter((entry) => entry.path === "api/v1/manage/gateway/provider").at(-1)?.body)
    .toMatchObject({ weight: 1, timeoutSecs: 1 });

  await page.locator(".provider-card", { hasText: "边界服务" }).getByRole("button", { name: "编辑" }).click();
  editor = page.getByRole("dialog", { name: "编辑模型服务" });
  await editor.getByLabel("路由权重").fill("10000");
  await editor.getByLabel("超时（秒）").fill("3600");
  await editor.getByRole("button", { name: "保存模型服务" }).click();
  await expect(editor).not.toBeVisible();
  expect(state.writes.filter((entry) => entry.path === "api/v1/manage/gateway/provider").at(-1)?.body)
    .toMatchObject({ weight: 10000, timeoutSecs: 3600 });

  await page.locator(".provider-card", { hasText: "边界服务" }).getByRole("button", { name: "编辑" }).click();
  editor = page.getByRole("dialog", { name: "编辑模型服务" });
  const providerWritesBeforeInvalidEdit = state.writes.filter((entry) => entry.path === "api/v1/manage/gateway/provider").length;
  await editor.getByLabel("路由权重").fill("0");
  await editor.getByLabel("超时（秒）").fill("3601");
  await expect(editor.getByRole("alert")).toContainText("路由权重必须是 1 到 10000 之间的整数。");
  await expect(editor.getByRole("alert")).toContainText("超时必须是 1 到 3600 秒之间的整数。");
  await expect(editor.getByRole("button", { name: "保存模型服务" })).toBeDisabled();
  expect(state.writes.filter((entry) => entry.path === "api/v1/manage/gateway/provider")).toHaveLength(providerWritesBeforeInvalidEdit);
});

test("gateway mutation errors have a single alert inside their confirmation dialog", async ({ page }) => {
  const state: ManagementState = {
    gateway: structuredClone(fixtureGateway),
    settings: structuredClone(fixtureSettings),
    writes: [],
    failProviderDelete: true,
    failSub2ApiDisconnect: true,
    sub2ApiConfigured: true,
  };
  await page.addInitScript(() => localStorage.setItem("mochiport.section", "gateway"));
  await installManagementMock(page, state);
  await page.goto("/");

  await page.getByRole("tab", { name: "模型服务", exact: true }).click();
  await page.locator(".provider-card", { hasText: "OpenAI" }).getByRole("button", { name: "删除" }).click();
  let confirmation = page.getByRole("dialog", { name: "删除模型服务？" });
  await confirmation.getByRole("button", { name: "删除", exact: true }).click();
  await expect(confirmation.getByRole("alert")).toHaveText(/删除模型服务失败/);
  await expect(page.getByRole("alert")).toHaveCount(1);
  await confirmation.getByRole("button", { name: "取消" }).click();
  await page.getByRole("alert").getByRole("button", { name: "关闭错误" }).click();

  await page.getByRole("tab", { name: "账号池", exact: true }).click();
  await page.locator(".pool-connection-card").getByRole("button", { name: "断开" }).click();
  confirmation = page.getByRole("dialog", { name: "断开 Sub2API 账号池？" });
  await confirmation.getByRole("button", { name: "断开连接" }).click();
  await expect(confirmation.getByRole("alert")).toHaveText(/断开 Sub2API 失败/);
  await expect(page.getByRole("alert")).toHaveCount(1);
});

test("Codex model catalog selections and custom models are merged in Mac-compatible order", async ({ page }) => {
  const state: ManagementState = {
    gateway: {
      ...structuredClone(fixtureGateway),
      codexVisibleModels: ["custom-z", "gpt-5.4"],
    },
    settings: structuredClone(fixtureSettings),
    writes: [],
  };
  await page.addInitScript(() => localStorage.setItem("mochiport.section", "gateway"));
  await installManagementMock(page, state);
  await page.goto("/");

  const selected = page.getByRole("button", { name: "可见模型 GPT-5.4", exact: true });
  const unselected = page.getByRole("button", { name: "可见模型 GPT-5.4 mini", exact: true });
  await expect(selected).toHaveAttribute("aria-pressed", "true");
  await expect(unselected).toHaveAttribute("aria-pressed", "false");
  await expect(page.locator(".model-chip", { hasText: "custom-z" })).toBeVisible();

  await selected.click();
  await unselected.click();
  await page.locator(".inline-add input").fill("private-model");
  await page.locator(".inline-add").getByRole("button", { name: "添加", exact: true }).click();
  await page.getByRole("button", { name: "保存网关设置" }).click();

  const write = state.writes.find((entry) => entry.path === "api/v1/manage/gateway/settings");
  expect(write?.body.codexVisibleModels).toEqual(["gpt-5.4-mini", "custom-z", "private-model"]);
  await expect(page.getByRole("status")).toContainText("网关设置已保存");
});

test("masked proxy credentials are preserved by default and cleared only when requested", async ({ page }) => {
  const maskedProxy = "http://proxy.example.com:8080";
  const state: ManagementState = {
    gateway: structuredClone(fixtureGateway),
    settings: {
      ...structuredClone(fixtureSettings),
      localConnectionMode: "standard",
      outboundProxy: { mode: "custom", url: maskedProxy, credentialSet: true },
    },
    writes: [],
  };
  await page.addInitScript(() => localStorage.setItem("mochiport.section", "settings"));
  await installManagementMock(page, state);
  await page.goto("/");
  await page.getByRole("button", { name: /网络/ }).click();

  await expect(page.getByLabel("代理 URL")).toHaveValue(maskedProxy);
  await page.getByRole("combobox", { name: "连接模式" }).selectOption("vpnCompatible");
  await page.getByRole("button", { name: "保存网络设置" }).click();
  await expect(page.getByRole("status")).toContainText("设置已保存");

  const writes = state.writes.filter((entry) => entry.path === "api/v1/manage/settings");
  expect(writes).toHaveLength(1);
  expect(writes[0].body).not.toHaveProperty("outboundProxyUrl");
  expect(state.settings.outboundProxy.url).toBe(maskedProxy);

  await page.getByRole("switch", { name: "清除已保存的代理凭据" }).click();
  await page.getByRole("button", { name: "保存网络设置" }).click();
  expect(state.writes.filter((entry) => entry.path === "api/v1/manage/settings").at(-1)?.body)
    .toMatchObject({ outboundProxyUrl: maskedProxy });
  expect(state.settings.outboundProxy.credentialSet).toBe(false);
  await expect(page.getByRole("switch", { name: "清除已保存的代理凭据" })).toHaveCount(0);

  await page.getByLabel("代理 URL").fill("socks5://127.0.0.1:1080");
  await page.getByRole("button", { name: "保存网络设置" }).click();
  expect(state.writes.filter((entry) => entry.path === "api/v1/manage/settings").at(-1)?.body)
    .toMatchObject({ outboundProxyUrl: "socks5://127.0.0.1:1080" });
});

test("backend settings cannot be saved before a successful read", async ({ page }) => {
  let releaseSettingsGet = () => {};
  const settingsGetGate = new Promise<void>((resolve) => { releaseSettingsGet = resolve; });
  const state: ManagementState = {
    gateway: structuredClone(fixtureGateway),
    settings: structuredClone(fixtureSettings),
    writes: [],
    settingsGetGate,
    failSettingsGet: true,
  };
  await page.addInitScript(() => localStorage.setItem("mochiport.section", "settings"));
  await installManagementMock(page, state);
  await page.goto("/");

  await expect(page.getByText("正在读取后台设置，加载完成后才能保存。", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "保存设置", exact: true })).toBeDisabled();
  await expect(page.getByRole("combobox", { name: "服务消息语言" })).toBeDisabled();

  await page.getByRole("button", { name: /^网络/ }).click();
  await expect(page.getByRole("button", { name: "保存网络设置" })).toBeDisabled();
  await expect(page.getByRole("combobox", { name: "连接模式" })).toBeDisabled();
  await expect(page.getByRole("combobox", { name: "代理模式" })).toBeDisabled();

  await page.getByRole("button", { name: /^使用量/ }).click();
  await expect(page.getByRole("button", { name: "保存提醒设置" })).toBeEnabled();

  releaseSettingsGet();
  await page.getByRole("button", { name: /^通用/ }).click();
  await expect(page.getByText("后台设置暂不可用，重试成功后才能保存。", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "保存设置", exact: true })).toBeDisabled();
  await page.getByRole("button", { name: /^网络/ }).click();
  await expect(page.getByRole("button", { name: "保存网络设置" })).toBeDisabled();
  expect(state.writes.filter((entry) => entry.path === "api/v1/manage/settings")).toHaveLength(0);
});

test("Windows autostart and native-notification preferences are wired through Settings", async ({ page }) => {
  const state: ManagementState = {
    gateway: structuredClone(fixtureGateway),
    settings: structuredClone(fixtureSettings),
    writes: [],
  };
  await page.addInitScript(() => {
    localStorage.setItem("mochiport.section", "settings");
    localStorage.removeItem("mochiport.autostart-preview");
    localStorage.removeItem("mochiport.notifications");
    localStorage.removeItem("mochiport.notification-real-mode");
    localStorage.removeItem("mochiport.notification-sound");
    localStorage.removeItem("mochiport.notify-update");
    localStorage.removeItem("mochiport.notification-custom-messages");
  });
  await installManagementMock(page, state);
  await page.goto("/");

  const autostart = page.getByRole("switch", { name: "登录 Windows 时启动" });
  await expect(autostart).toBeEnabled();
  await expect(autostart).toHaveAttribute("aria-checked", "false");
  await autostart.click();
  await expect(autostart).toHaveAttribute("aria-checked", "true");
  expect(await page.evaluate(() => localStorage.getItem("mochiport.autostart-preview"))).toBe("on");

  await page.getByRole("button", { name: /使用量/ }).click();
  const notifications = page.getByRole("switch", { name: "启用系统通知" });
  await expect(notifications).toHaveAttribute("aria-checked", "false");
  await notifications.click();
  await page.getByRole("switch", { name: "回来继续工作" }).click();
  await page.getByRole("switch", { name: "时段摘要", exact: true }).click();
  await page.getByRole("switch", { name: "里程碑和新纪录" }).click();
  await page.getByRole("switch", { name: "拟人化提示语" }).click();
  await page.getByRole("switch", { name: "提示音" }).click();
  await page.getByRole("switch", { name: "检测新版本" }).click();
  const customEvent = page.locator(".custom-notification-event", { hasText: "额度接近上限" });
  await customEvent.locator("summary").click();
  await customEvent.getByLabel("额度接近上限自定义文案").fill("{AGENT} 已经 {USAGE}\n第二条提醒");
  await page.getByRole("button", { name: "保存提醒设置" }).click();
  await expect(page.getByText("通知设置已保存", { exact: true })).toBeVisible();
  expect(await page.evaluate(() => localStorage.getItem("mochiport.notifications"))).toBe("on");
  expect(await page.evaluate(() => ({
    comeback: localStorage.getItem("mochiport.notify-comeback"),
    briefing: localStorage.getItem("mochiport.notify-briefing"),
    milestone: localStorage.getItem("mochiport.notify-milestone-record"),
    threshold: localStorage.getItem("mochiport.notify-limit-threshold"),
    realMode: localStorage.getItem("mochiport.notification-real-mode"),
    sound: localStorage.getItem("mochiport.notification-sound"),
    notifyUpdate: localStorage.getItem("mochiport.notify-update"),
    customMessages: JSON.parse(localStorage.getItem("mochiport.notification-custom-messages") ?? "{}"),
  }))).toEqual({
    comeback: "off",
    briefing: "off",
    milestone: "off",
    threshold: "on",
    realMode: "on",
    sound: "on",
    notifyUpdate: "off",
    customMessages: { limitThreshold: ["{AGENT} 已经 {USAGE}", "第二条提醒"] },
  });

  await page.getByRole("button", { name: "发送测试通知" }).click();
  await expect(page.getByText("测试通知已发送", { exact: true })).toBeVisible();
});
