import { expect, test } from "@playwright/test";

test("provider tools and request-log filters remain interactive", async ({ page }) => {
  const runtimeErrors: string[] = [];
  page.on("console", (message) => {
    if (message.type() === "error") runtimeErrors.push(`console: ${message.text()}`);
  });
  page.on("pageerror", (error) => runtimeErrors.push(`page: ${error.message}`));

  await page.goto("/?fixture=1");
  await page.waitForLoadState("networkidle");
  await page.getByRole("button", { name: /AI 网关/ }).click();
  await page.getByRole("tab", { name: "模型服务", exact: true }).click();

  await page.getByRole("button", { name: "用量" }).first().click();
  const usageDialog = page.getByRole("dialog", { name: /余额与计费/ });
  await expect(usageDialog.locator(".provider-usage__hero strong")).toHaveText("42.86");
  await usageDialog.locator(".modal__footer").getByRole("button", { name: "关闭", exact: true }).click();

  await page.getByRole("button", { name: "添加服务", exact: true }).click();
  const editor = page.getByRole("dialog", { name: "添加模型服务" });
  await editor.getByLabel("服务模板").selectOption("anthropic");
  await expect(editor.getByLabel("名称")).toHaveValue("anthropic");
  await expect(editor.getByRole("combobox", { name: /^协议/ })).toHaveValue("anthropic_messages");
  await expect(editor.getByLabel("API 地址", { exact: true })).toHaveValue("https://api.anthropic.com/v1");
  await editor.getByRole("button", { name: "从服务商获取模型" }).click();
  await expect(editor.getByLabel("模型 ID")).toHaveValue(/gpt-5\.4/);
  await editor.locator(".modal__footer").getByRole("button", { name: "取消" }).click();

  await page.getByRole("button", { name: "请求日志" }).click();
  await page.getByRole("combobox", { name: "状态" }).selectOption("failed");
  await expect(page.locator(".log-row")).toHaveCount(1);
  await page.getByPlaceholder("搜索请求 ID、模型、渠道或状态").fill("req_81e9010f");
  await expect(page.locator(".log-row")).toHaveCount(1);

  expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
  expect(runtimeErrors).toEqual([]);
});

test("session routes and enhanced launch use the Mac-compatible semantics", async ({ page }) => {
  const runtimeErrors: string[] = [];
  page.on("console", (message) => {
    if (message.type() === "error") runtimeErrors.push(`console: ${message.text()}`);
  });
  page.on("pageerror", (error) => runtimeErrors.push(`page: ${error.message}`));

  await page.goto("/?fixture=1");
  await page.waitForLoadState("networkidle");

  await page.getByRole("button", { name: "会话" }).click();
  const gatewaySession = page.locator(".data-table__row", { hasText: "Windows 客户端界面复刻" });
  const directSession = page.locator(".data-table__row", { hasText: "优化消息渠道接入" });
  await expect(gatewaySession.locator(".route-cell small")).toHaveText("ai-gateway");
  await expect(directSession.locator(".route-cell small")).toHaveText("openai");
  await gatewaySession.getByRole("checkbox").check();
  await page.getByLabel("目标 Provider").selectOption("openai");
  await page.getByRole("button", { name: "移动会话" }).click();
  await expect(gatewaySession.locator(".route-cell small")).toHaveText("openai");

  await gatewaySession.getByRole("checkbox").check();
  await page.getByLabel("目标 Provider").selectOption("ai-gateway");
  await page.getByRole("button", { name: "移动会话" }).click();
  await expect(gatewaySession.locator(".route-cell small")).toHaveText("ai-gateway");

  await page.getByRole("button", { name: "Codex" }).click();
  await expect(page.locator(".mode-card > strong")).toHaveText("MochiPort AI 网关");
  await page.getByRole("button", { name: "增强模式启动 Codex" }).click();
  await expect(page.locator(".enhanced-operation")).toContainText("增强模式已就绪");
  await expect(page.getByRole("button", { name: "增强模式启动 Codex" })).toBeEnabled();

  expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
  expect(runtimeErrors).toEqual([]);
});
