import { expect, test } from "@playwright/test";
import { formatUsageCost, formatUsageTokens } from "../src/utils/format";

test("usage card formatting matches AI Token Monitor precision tiers", () => {
  expect(formatUsageTokens(163_505_410)).toBe("163.5M");
  expect(formatUsageTokens(1_250_000_000)).toBe("1.3B");
  expect(formatUsageCost(129.442171)).toBe("$129");
  expect(formatUsageCost(12.3456)).toBe("$12.35");
  expect(formatUsageCost(0.123456)).toBe("$0.1235");
});

test("usage insights switch between seven-day, thirty-day, and heatmap views", async ({ page }) => {
  await page.goto("/?fixture=1");
  await expect(page.getByRole("heading", { name: "概览", exact: true })).toBeVisible();

  const range = page.getByRole("group", { name: "用量范围" });
  await expect(range.getByRole("button", { name: "7 天" })).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByText("今日 Token", { exact: true })).toBeVisible();
  await expect(page.getByText("今日成本", { exact: true })).toBeVisible();
  await expect(page.getByText("$129", { exact: true })).toBeVisible();
  await expect(page.getByText("今日请求 Token", { exact: true })).toHaveCount(0);
  await expect(page.locator(".usage-metrics > div").filter({ hasText: "今日 Token" }).locator("strong")).toHaveText(/^\d+\.\dK$/);
  const weekChart = page.getByRole("img", { name: "最近七天 Codex Token 趋势" });
  await expect(weekChart.locator(".usage-bar")).toHaveCount(7);
  await expect(weekChart.locator(".usage-chart__y-axis span")).toHaveCount(3);
  await expect(weekChart.locator(".usage-chart__gridlines i")).toHaveCount(3);
  await expect(weekChart).toHaveAttribute("aria-describedby", /.+/);

  await range.getByRole("button", { name: "30 天" }).click();
  await expect(range.getByRole("button", { name: "30 天" })).toHaveAttribute("aria-pressed", "true");
  const monthChart = page.getByRole("img", { name: "最近三十天 Codex Token 趋势" });
  await expect(monthChart.locator(".usage-bar")).toHaveCount(30);
  await expect(monthChart.locator(".usage-chart__y-axis span")).toHaveCount(3);

  await range.getByRole("button", { name: "热力图" }).click();
  await expect(range.getByRole("button", { name: "热力图" })).toHaveAttribute("aria-pressed", "true");
  const heatmap = page.getByRole("grid", { name: "最近 105 天 Codex Token 热力图" });
  const cells = heatmap.locator(".usage-heatmap__cell:not(.usage-heatmap__cell--padding)");
  await expect(heatmap).toBeVisible();
  await expect(heatmap).toHaveAttribute("aria-rowcount", "7");
  await expect(cells).toHaveCount(105);
  await expect(page.locator(".usage-heatmap__cell--today")).toHaveAttribute("aria-current", "date");
  await expect(page.locator(".usage-heatmap__caption")).toContainText("Token");
  expect(await page.locator(".usage-heatmap__months .usage-heatmap__month-label:not(:empty)").count()).toBeGreaterThanOrEqual(3);

  const cellSizes = await cells.evaluateAll((elements) => elements.map((element) => {
    const rect = element.getBoundingClientRect();
    return { width: rect.width, height: rect.height };
  }));
  expect(cellSizes.every(({ width, height }) => Math.abs(width - height) <= 0.5)).toBe(true);
  expect(cellSizes.every(({ width }) => width >= 12)).toBe(true);

  const weekdayLabels = await page.locator(".usage-heatmap__weekdays span").evaluateAll((elements) => elements
    .filter((element) => element.textContent)
    .map((element) => ({ label: element.textContent, top: element.getBoundingClientRect().top })));
  expect(weekdayLabels.map(({ label }) => label)).toEqual(["周一", "周三", "周五", "周日"]);
  expect(weekdayLabels.every((label, index) => index === 0 || label.top > weekdayLabels[index - 1].top)).toBe(true);

  await range.getByRole("button", { name: "项目" }).click();
  await expect(range.getByRole("button", { name: "项目" })).toHaveAttribute("aria-pressed", "true");
  const projects = page.getByRole("region", { name: "最近 7 天项目 Token 占比" });
  await expect(projects.getByRole("progressbar")).toHaveCount(4);
  await expect(projects.getByText("MochiPort", { exact: true })).toBeVisible();
  await expect(projects.getByRole("progressbar", { name: "MochiPort 项目 Token 占比" })).toHaveAttribute("aria-valuenow", /\d+(?:\.\d+)?/);
});

test("overview usage sections keep macOS-like separation", async ({ page }) => {
  await page.goto("/?fixture=1");
  const gaps = await page.evaluate(() => {
    const metricGrid = document.querySelector<HTMLElement>(".metric-grid")?.getBoundingClientRect();
    const usage = document.querySelector<HTMLElement>(".usage-insights")?.getBoundingClientRect();
    const topology = document.querySelector<HTMLElement>(".topology-card")?.getBoundingClientRect();
    if (!metricGrid || !usage || !topology) return undefined;
    return {
      beforeUsage: usage.top - metricGrid.bottom,
      afterUsage: topology.top - usage.bottom,
    };
  });
  expect(gaps).toBeDefined();
  expect(gaps?.beforeUsage).toBeGreaterThanOrEqual(23);
  expect(gaps?.afterUsage).toBeGreaterThanOrEqual(23);
});
