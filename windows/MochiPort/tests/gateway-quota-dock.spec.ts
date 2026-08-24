import { expect, test } from "@playwright/test";

test("global gateway quota dock follows the selected Provider", async ({ page }) => {
  const runtimeErrors: string[] = [];
  page.on("console", (message) => {
    if (message.type() === "error") runtimeErrors.push(`console: ${message.text()}`);
  });
  page.on("pageerror", (error) => runtimeErrors.push(`page: ${error.message}`));

  await page.goto("/?fixture=1");
  await page.waitForLoadState("networkidle");

  const dock = page.getByTestId("gateway-quota-dock");
  await expect(dock).toBeVisible();
  await expect(dock).toContainText("$42.86");
  await expect(dock).toContainText("余额充足");
  await expect(dock).toContainText("倍率 1×");

  await dock.getByLabel("选择额度 Provider").selectOption("Anthropic");
  await expect(dock).toContainText("无限额度");
  await expect(dock).toContainText("倍率 0.8×");

  await dock.getByRole("button", { name: "额度详情" }).click();
  const details = page.getByRole("dialog", { name: /额度详情/ });
  await expect(details).toContainText("Claude 工作区");
  await expect(details).toContainText("无限额度");
  await details.getByRole("button", { name: "关闭", exact: true }).first().click();

  await dock.getByRole("button", { name: "打开日志列表" }).click();
  await expect(page.getByRole("heading", { name: "请求日志", level: 1 })).toBeVisible();

  const layout = await dock.evaluate((element) => {
    const bounds = element.getBoundingClientRect();
    return {
      bottom: Math.round(bounds.bottom),
      height: Math.round(bounds.height),
      width: Math.round(bounds.width),
      windowHeight: window.innerHeight,
      windowWidth: window.innerWidth,
      documentOverflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
    };
  });
  expect(layout.bottom).toBe(layout.windowHeight);
  expect(layout.height).toBe(68);
  expect(layout.width).toBeLessThanOrEqual(layout.windowWidth);
  expect(layout.documentOverflow).toBeLessThanOrEqual(1);
  expect(runtimeErrors).toEqual([]);
});
