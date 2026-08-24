import { expect, test, type Page, type Route } from "@playwright/test";

const managementOrigin = "http://127.0.0.1:3847";
const installationId = "windows-installation-test";
const sha256 = "a".repeat(64);
const corsHeaders = {
  "access-control-allow-origin": "*",
  "access-control-allow-methods": "GET, POST, OPTIONS",
  "access-control-allow-headers": "content-type",
  "content-type": "application/json",
};

async function fulfillJson(route: Route, body: unknown, status = 200) {
  await route.fulfill({ status, headers: corsHeaders, body: JSON.stringify(body) });
}

function service(instanceId = "daemon-before", pid = 42, startedAtMs = 1_700_000_000_000) {
  return { service: "threadrelay", apiMajor: 1, ready: true, instanceId, pid, startedAtMs };
}

function dashboard(instance = service()) {
  return {
    service: instance,
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

function lifecycle(
  instance = service(),
  owner: string | null = null,
  leaseGeneration: number | null = null,
  protectedTotal = 0,
) {
  return {
    service: instance,
    executable: "C:\\Program Files\\MochiPort\\mochiport-daemon.exe",
    executableSha256: sha256,
    configPath: "C:\\Users\\Test\\AppData\\Local\\MochiPort\\config.toml",
    bind: "127.0.0.1:3847",
    runtime: { state: "active", productVersion: "0.5.3", buildNumber: 439, apiMajor: 1 },
    protectedWorkItems: {
      aiGatewayRequests: protectedTotal,
      codexTurns: 0,
      imStreams: 0,
      pendingApprovals: 0,
      remoteControlRequests: 0,
      total: protectedTotal,
    },
    management: {
      state: owner ? "managed" : "unmanaged",
      mode: owner ? "managed" : "readOnly",
      canControl: owner === installationId,
      installationId: owner,
      leaseGeneration,
      leaseExpiresAtMs: owner ? Date.now() + 60_000 : null,
      managementTokenGeneration: 1,
    },
  };
}

async function initializeSettings(page: Page) {
  await page.addInitScript(({ section, installation }) => {
    localStorage.setItem("mochiport.section", section);
    localStorage.setItem("mochiport.lifecycle.installation-id", installation);
  }, { section: "settings", installation: installationId });
}

test("safe restart is sent only after confirmation and reclaims the same path and build", async ({ page }) => {
  await initializeSettings(page);
  let currentLifecycle = lifecycle();
  let restartCalls = 0;
  let replacementClaims = 0;
  let restartBody: Record<string, unknown> | undefined;

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
      await fulfillJson(route, dashboard(currentLifecycle.service));
      return;
    }
    if (path === "api/v1/manage/im/accounts") {
      await fulfillJson(route, { accounts: [] });
      return;
    }
    if (path === "api/v1/manage/settings") {
      await fulfillJson(route, {
        language: "zh-CN",
        theme: "system",
        localConnectionMode: "standard",
        bind: "127.0.0.1:3847",
        outboundProxy: { mode: "system", url: "<none>", credentialSet: false },
      });
      return;
    }
    if (path === "api/v1/manage/lifecycle" && request.method() === "GET") {
      await fulfillJson(route, currentLifecycle);
      return;
    }
    if (path === "api/v1/manage/lifecycle/lease/claim") {
      const body = request.postDataJSON();
      expect(body.installationId).toBe(installationId);
      expect(body.daemonIdentity).toEqual({
        pid: currentLifecycle.service.pid,
        startedAtMs: currentLifecycle.service.startedAtMs,
        executable: currentLifecycle.executable,
        executableSha256: sha256,
        bind: "127.0.0.1:3847",
      });
      if (currentLifecycle.service.instanceId === "daemon-after") replacementClaims += 1;
      currentLifecycle = lifecycle(currentLifecycle.service, installationId, replacementClaims ? 8 : 7);
      await fulfillJson(route, currentLifecycle);
      return;
    }
    if (path === "api/v1/manage/lifecycle/lease/renew") {
      currentLifecycle = lifecycle(currentLifecycle.service, installationId, currentLifecycle.management.leaseGeneration);
      await fulfillJson(route, currentLifecycle);
      return;
    }
    if (path === "api/v1/manage/lifecycle/restart") {
      restartCalls += 1;
      restartBody = request.postDataJSON();
      currentLifecycle = lifecycle(service("daemon-after", 84, 1_700_000_100_000));
      await fulfillJson(route, { ok: true, state: "restarting" });
      return;
    }
    if (path === "api/v1/manage/gateway/sub2api") {
      await fulfillJson(route, { configured: false, baseUrl: "", secretSet: false });
      return;
    }
    await fulfillJson(route, { error: `未处理的测试请求：${path}` }, 404);
  });

  await page.goto("/");
  await page.getByRole("button", { name: "更新与诊断" }).click();
  const restartButton = page.getByRole("button", { name: "安全重启后台服务" });
  await expect(restartButton).toBeVisible();

  await restartButton.click();
  const confirmation = page.getByRole("dialog", { name: "重启后台服务？" });
  await expect(confirmation).toBeVisible();
  expect(restartCalls).toBe(0);

  await confirmation.getByRole("button", { name: "确认安全重启" }).click();
  await expect(confirmation).not.toBeVisible();
  await expect(page.getByText("后台服务已安全重启")).toBeVisible();
  expect(restartCalls).toBe(1);
  expect(restartBody).toEqual({
    installationId,
    daemonInstanceId: "daemon-before",
    leaseGeneration: 7,
    force: false,
  });
  expect(replacementClaims).toBe(1);
  await expect(page.getByText("进程 84 · 127.0.0.1:3847")).toBeVisible();
});

test("a protected-work rejection is surfaced without forcing or claiming a replacement", async ({ page }) => {
  await initializeSettings(page);
  let currentLifecycle = lifecycle();
  let restartBody: Record<string, unknown> | undefined;
  let replacementClaims = 0;

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
    if (path === "api/v1/manage/settings") return fulfillJson(route, { language: "zh-CN", theme: "system", localConnectionMode: "standard", bind: "127.0.0.1:3847", outboundProxy: { mode: "system", url: "<none>", credentialSet: false } });
    if (path === "api/v1/manage/lifecycle" && request.method() === "GET") return fulfillJson(route, currentLifecycle);
    if (path === "api/v1/manage/lifecycle/lease/claim") {
      if (currentLifecycle.service.instanceId !== "daemon-before") replacementClaims += 1;
      currentLifecycle = lifecycle(currentLifecycle.service, installationId, 7);
      return fulfillJson(route, currentLifecycle);
    }
    if (path === "api/v1/manage/lifecycle/restart") {
      restartBody = request.postDataJSON();
      return fulfillJson(route, {
        error: "后台服务仍有 1 项受保护任务，已取消重启。",
        protectedWorkItems: lifecycle(service(), installationId, 7, 1).protectedWorkItems,
      }, 409);
    }
    if (path === "api/v1/manage/gateway/sub2api") return fulfillJson(route, { configured: false, baseUrl: "", secretSet: false });
    return fulfillJson(route, { error: `未处理的测试请求：${path}` }, 404);
  });

  await page.goto("/");
  await page.getByRole("button", { name: "更新与诊断" }).click();
  await page.getByRole("button", { name: "安全重启后台服务" }).click();
  await page.getByRole("dialog", { name: "重启后台服务？" }).getByRole("button", { name: "确认安全重启" }).click();

  const confirmation = page.getByRole("dialog", { name: "重启后台服务？" });
  await expect(confirmation.getByRole("alert")).toContainText("后台服务仍有 1 项受保护任务");
  await expect(page.getByRole("alert")).toHaveCount(1);
  expect(restartBody?.force).toBe(false);
  expect(replacementClaims).toBe(0);
});

test("conflicting lease takeover and credential rotation require separate confirmations", async ({ page }) => {
  await initializeSettings(page);
  let currentLifecycle = lifecycle(service("daemon-conflict", 52), "other-installation", 11);
  let takeoverCalls = 0;
  let rotationCalls = 0;

  await page.route(`${managementOrigin}/**`, async (route) => {
    const request = route.request();
    if (request.method() === "OPTIONS") {
      await route.fulfill({ status: 204, headers: corsHeaders });
      return;
    }
    const path = new URL(request.url()).pathname.replace(/^\//, "");
    if (path === "healthz") return fulfillJson(route, { service: "threadrelay", apiMajor: 1, ready: true });
    if (path === "api/v1/manage/dashboard") return fulfillJson(route, dashboard(currentLifecycle.service));
    if (path === "api/v1/manage/im/accounts") return fulfillJson(route, { accounts: [] });
    if (path === "api/v1/manage/settings") return fulfillJson(route, { language: "zh-CN", theme: "system", localConnectionMode: "standard", bind: "127.0.0.1:3847", outboundProxy: { mode: "system", url: "<none>", credentialSet: false } });
    if (path === "api/v1/manage/lifecycle" && request.method() === "GET") return fulfillJson(route, currentLifecycle);
    if (path === "api/v1/manage/lifecycle/lease/takeover") {
      takeoverCalls += 1;
      const body = request.postDataJSON();
      expect(body.force).toBe(true);
      expect(body.expectedLeaseGeneration).toBe(11);
      expect(body.expectedManagementTokenGeneration).toBe(1);
      expect(body.daemonIdentity.executableSha256).toBe(sha256);
      currentLifecycle = lifecycle(currentLifecycle.service, installationId, 12);
      currentLifecycle.management.managementTokenGeneration = 2;
      return fulfillJson(route, { ok: true, rotated: true, requestId: body.requestId, managementTokenGeneration: 2 });
    }
    if (path === "api/v1/manage/lifecycle/credential/rotate") {
      rotationCalls += 1;
      const body = request.postDataJSON();
      expect(body.reason).toBe("leakRecovery");
      expect(body.leaseGeneration).toBe(12);
      expect(body.expectedManagementTokenGeneration).toBe(2);
      currentLifecycle.management.managementTokenGeneration = 3;
      return fulfillJson(route, { ok: true, rotated: true, requestId: body.requestId, managementTokenGeneration: 3 });
    }
    if (path === "api/v1/manage/gateway/sub2api") return fulfillJson(route, { configured: false, baseUrl: "", secretSet: false });
    return fulfillJson(route, { error: `未处理的测试请求：${path}` }, 404);
  });

  await page.goto("/");
  await page.getByRole("button", { name: "更新与诊断" }).click();
  const takeoverButton = page.getByRole("button", { name: "接管管理权" });
  await expect(takeoverButton).toBeVisible();
  await takeoverButton.click();
  const takeoverDialog = page.getByRole("dialog", { name: "接管后台服务管理权？" });
  await expect(takeoverDialog).toBeVisible();
  expect(takeoverCalls).toBe(0);
  await takeoverDialog.getByRole("button", { name: "确认接管" }).click();
  await expect(takeoverDialog).not.toBeVisible();
  await expect(page.getByText("已接管后台服务")).toBeVisible();
  expect(takeoverCalls).toBe(1);

  const rotationButton = page.getByRole("button", { name: "重新生成管理凭据" });
  await expect(rotationButton).toBeVisible();
  await rotationButton.click();
  const rotationDialog = page.getByRole("dialog", { name: "重新生成管理凭据？" });
  await expect(rotationDialog).toBeVisible();
  expect(rotationCalls).toBe(0);
  await rotationDialog.getByRole("button", { name: "确认重新生成" }).click();
  await expect(rotationDialog).not.toBeVisible();
  await expect(page.getByText("管理凭据已重新生成")).toBeVisible();
  expect(rotationCalls).toBe(1);
});
