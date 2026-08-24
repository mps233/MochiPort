import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { expect, test } from "@playwright/test";

const sections = [
  { navigation: "概览", heading: "概览", fileName: "overview" },
  { navigation: "Codex 接入", heading: "Codex 接入", fileName: "codex" },
  { navigation: "AI 网关", heading: "AI 网关", fileName: "gateway" },
  { navigation: "消息渠道", heading: "消息渠道", fileName: "messaging" },
  { navigation: "会话", heading: "会话", fileName: "sessions" },
  { navigation: "请求日志", heading: "请求日志", fileName: "request-logs" },
  { navigation: "设置", heading: "设置", fileName: "settings" },
] as const;

test("all primary views fit the standard Windows viewport", async ({ page }) => {
  const runtimeErrors: string[] = [];
  const screenshotDirectory = process.env.MOCHIPORT_VISUAL_OUTPUT;
  page.on("console", (message) => {
    if (message.type() === "error") runtimeErrors.push(`console: ${message.text()}`);
  });
  page.on("pageerror", (error) => runtimeErrors.push(`page: ${error.message}`));

  if (screenshotDirectory) await mkdir(screenshotDirectory, { recursive: true });
  await page.goto("/?fixture=1");
  await page.waitForLoadState("networkidle");

  for (const section of sections) {
    await page.getByRole("button", { name: section.navigation, exact: true }).click();
    await page.locator(".page-viewport").evaluate((element) => element.scrollTo(0, 0));
    await expect(page.getByRole("heading", { name: section.heading, level: 1 })).toBeVisible();

    const layout = await page.evaluate(() => {
      const root = document.documentElement;
      const shell = document.querySelector<HTMLElement>(".app-shell");
      const viewport = document.querySelector<HTMLElement>(".page-viewport");
      return {
        documentOverflow: root.scrollWidth - root.clientWidth,
        shellRight: shell ? Math.round(shell.getBoundingClientRect().right) : -1,
        viewportRight: viewport ? Math.round(viewport.getBoundingClientRect().right) : -1,
        windowWidth: window.innerWidth,
      };
    });

    expect(layout.documentOverflow, `${section.heading} must not overflow horizontally`).toBeLessThanOrEqual(1);
    expect(layout.shellRight, `${section.heading} shell must remain inside the window`).toBeLessThanOrEqual(layout.windowWidth);
    expect(layout.viewportRight, `${section.heading} content viewport must remain inside the window`).toBeLessThanOrEqual(layout.windowWidth);

    if (screenshotDirectory) {
      await page.screenshot({ path: join(screenshotDirectory, `${section.fileName}.png`) });
      if (section.fileName === "overview") {
        await page.locator(".usage-insights").scrollIntoViewIfNeeded();
        await page.screenshot({ path: join(screenshotDirectory, "overview-usage.png") });
      }
    }
  }

  expect(runtimeErrors).toEqual([]);
});
