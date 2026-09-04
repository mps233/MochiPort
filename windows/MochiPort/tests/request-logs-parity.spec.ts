import { expect, test, type Page, type Route } from "@playwright/test";
import {
  fixtureAccounts,
  fixtureDashboard,
  fixtureLifecycle,
  fixtureLogs,
} from "../src/api/fixtures";

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

async function installBaseRoutes(
  page: Page,
  requestLogs: (route: Route, url: URL) => Promise<void>,
  requestLogDetail?: (route: Route, id: number) => Promise<void>,
) {
  await page.addInitScript(() => localStorage.setItem("mochiport.section", "requestLogs"));
  await page.route(`${managementOrigin}/**`, async (route) => {
    const request = route.request();
    if (request.method() === "OPTIONS") {
      await route.fulfill({ status: 204, headers: corsHeaders });
      return;
    }
    const url = new URL(request.url());
    const path = url.pathname.replace(/^\//, "");
    if (path === "healthz") return fulfillJson(route, { service: "mochiport", apiMajor: 1, ready: true });
    if (path === "api/v1/manage/dashboard") return fulfillJson(route, fixtureDashboard);
    if (path === "api/v1/manage/im/accounts") return fulfillJson(route, { accounts: fixtureAccounts });
    if (path === "api/v1/manage/lifecycle") return fulfillJson(route, fixtureLifecycle);
    if (path === "api/v1/manage/gateway/sub2api") {
      return fulfillJson(route, { configured: false, baseUrl: "", secretSet: false });
    }
    if (path === "api/v1/manage/request-logs") return requestLogs(route, url);
    const detailMatch = path.match(/^api\/v1\/manage\/request-logs\/(\d+)$/);
    if (detailMatch && requestLogDetail) return requestLogDetail(route, Number(detailMatch[1]));
    return fulfillJson(route, { error: `未处理的测试请求：${path}` }, 404);
  });
}

test("the five-second first-page refresh preserves loaded pages and their cursor", async ({ page }) => {
  let firstPageCalls = 0;
  let tailLoaded = false;
  const requestedCursors: string[] = [];
  const initial = [
    { ...fixtureLogs[0], id: 300, requestId: "request-300" },
    { ...fixtureLogs[1], id: 299, requestId: "request-299" },
  ];
  const loadedTail = [
    { ...fixtureLogs[1], id: 299, requestId: "request-299" },
    { ...fixtureLogs[1], id: 298, requestId: "request-298" },
    { ...fixtureLogs[1], id: 297, requestId: "request-297" },
  ];
  const refreshed = [
    { ...fixtureLogs[0], id: 301, requestId: "request-301" },
    { ...fixtureLogs[0], id: 300, requestId: "request-300" },
  ];

  await installBaseRoutes(page, async (route, url) => {
    const cursor = url.searchParams.get("cursor");
    if (cursor) requestedCursors.push(cursor);
    if (cursor === "cursor-1") {
      tailLoaded = true;
      return fulfillJson(route, { logs: loadedTail, nextCursor: "cursor-2", hasMore: true });
    }
    if (cursor === "cursor-2") {
      return fulfillJson(route, {
        logs: [{ ...fixtureLogs[2], id: 296, requestId: "request-296" }],
        nextCursor: null,
        hasMore: false,
      });
    }
    firstPageCalls += 1;
    return fulfillJson(route, {
      logs: tailLoaded ? refreshed : initial,
      nextCursor: tailLoaded ? "new-first-page-cursor" : "cursor-1",
      hasMore: true,
    });
  });

  await page.goto("/");
  await expect(page.getByRole("heading", { name: "请求日志", exact: true })).toBeVisible();
  await expect(page.locator(".log-row")).toHaveCount(2);
  await page.getByRole("button", { name: "加载更多" }).click();
  await expect(page.locator(".log-row")).toHaveCount(4);

  const firstPageCallsAfterTail = firstPageCalls;
  await expect.poll(() => firstPageCalls, { timeout: 7_000 }).toBeGreaterThan(firstPageCallsAfterTail);
  await expect(page.locator(".log-primary small")).toHaveText([
    "request-301",
    "request-300",
    "request-299",
    "request-298",
    "request-297",
  ]);
  await expect(page.getByRole("button", { name: "加载更多" })).toBeVisible();

  await page.getByRole("button", { name: "加载更多" }).click();
  await expect(page.locator(".log-primary small")).toHaveText([
    "request-301",
    "request-300",
    "request-299",
    "request-298",
    "request-297",
    "request-296",
  ]);
  expect(requestedCursors).toEqual(["cursor-1", "cursor-2"]);
  await expect(page.getByRole("button", { name: "加载更多" })).toHaveCount(0);
});

test("changing a server-side filter resets the loaded tail and starts a new cursor chain", async ({ page }) => {
  const requestedCursors: string[] = [];
  const initial = { ...fixtureLogs[0], id: 410, requestId: "unfiltered-first" };
  const oldTail = { ...fixtureLogs[1], id: 409, requestId: "unfiltered-tail" };
  const filtered = { ...fixtureLogs[2], id: 408, requestId: "filtered-first", status: "failed" };
  const filteredTail = { ...fixtureLogs[2], id: 407, requestId: "filtered-tail", status: "failed" };

  await installBaseRoutes(page, async (route, url) => {
    const cursor = url.searchParams.get("cursor");
    if (cursor) requestedCursors.push(cursor);
    if (url.searchParams.get("status") === "failed") {
      if (cursor === "failed-cursor") {
        return fulfillJson(route, { logs: [filteredTail], nextCursor: null, hasMore: false });
      }
      return fulfillJson(route, { logs: [filtered], nextCursor: "failed-cursor", hasMore: true });
    }
    if (cursor === "default-cursor") {
      return fulfillJson(route, { logs: [oldTail], nextCursor: "old-tail-cursor", hasMore: true });
    }
    return fulfillJson(route, { logs: [initial], nextCursor: "default-cursor", hasMore: true });
  });

  await page.goto("/");
  await expect(page.locator(".log-primary small")).toHaveText(["unfiltered-first"]);
  await page.getByRole("button", { name: "加载更多" }).click();
  await expect(page.locator(".log-primary small")).toHaveText(["unfiltered-first", "unfiltered-tail"]);

  await page.getByRole("combobox", { name: "状态" }).selectOption("failed");
  await expect(page.locator(".log-primary small")).toHaveText(["filtered-first"]);
  await expect(page.locator(".log-row")).not.toContainText("unfiltered-tail");
  await page.getByRole("button", { name: "加载更多" }).click();
  await expect(page.locator(".log-primary small")).toHaveText(["filtered-first", "filtered-tail"]);
  expect(requestedCursors).toEqual(["default-cursor", "failed-cursor"]);
});

test("cache metrics, request bytes, line numbers, and safe detail search are rendered", async ({ page }) => {
  const log = {
    ...fixtureLogs[0],
    id: 520,
    requestId: "request-cache-detail",
    readCacheTokens: 12_640,
    readCacheHitRate: 0.6862,
    writeCacheTokens: 3_072,
    writeCache5mTokens: 2_048,
    writeCache1hTokens: 1_024,
    upstreamRequestBodyBytes: 18_432,
  };
  const detail = {
    ...log,
    requestHeadersJson: "{\n  \"authorization\": \"<redacted>\",\n  \"x-search\": \"needle\"\n}",
    requestJson: "{\n  \"prompt\": \"Needle in a redacted request\"\n}",
    upstreamRequestHeadersJson: "{\n  \"authorization\": \"<redacted>\"\n}",
    upstreamRequestJson: "{\n  \"model\": \"gpt-5.4\"\n}",
    upstreamResponseSse: "data: {\"status\":\"completed\"}",
    responseJson: "{\n  \"needleResult\": true\n}",
  };

  await installBaseRoutes(
    page,
    (route) => fulfillJson(route, { logs: [log], nextCursor: null, hasMore: false }),
    (route, id) => fulfillJson(route, id === log.id ? { log: detail } : { error: "not found" }, id === log.id ? 200 : 404),
  );

  await page.goto("/");
  const row = page.locator(".log-row");
  await expect(row).toContainText("读 13k · 68.6%");
  await expect(row).toContainText("写 3.1k");
  await expect(row).toContainText("5m 2.0k · 1h 1.0k");
  await expect(row).toContainText("18.0 KB");
  await row.click();

  await expect(page.getByText("读缓存", { exact: true })).toBeVisible();
  await expect(page.getByText("13k · 68.6%", { exact: true })).toBeVisible();
  await expect(page.getByText("5m 2.0k · 1h 1.0k", { exact: true })).toBeVisible();
  await expect(page.getByText("18.0 KB", { exact: true })).toBeVisible();

  const requestHeaders = page.locator(".detail-code-block", {
    has: page.getByRole("heading", { name: "请求 Headers", exact: true }),
  });
  await expect(requestHeaders.locator('[data-line-number="2"]')).toContainText('"authorization": "<redacted>"');
  await expect(requestHeaders.locator('[data-line-number="3"]')).toContainText('"x-search": "needle"');
  await expect(page.getByText("<redacted>", { exact: false })).toHaveCount(2);

  const search = page.getByRole("searchbox", { name: "在请求详情中搜索" });
  await search.fill("needle");
  await expect(page.locator(".detail-search-status")).toHaveText("找到 3 处匹配");
  await expect(page.locator(".detail-code-line mark")).toHaveCount(3);
  await expect(requestHeaders.locator(".detail-match-count")).toHaveText("1 处匹配");

  await search.fill("definitely-absent");
  await expect(page.locator(".detail-search-status")).toHaveText("无匹配");
  await expect(page.locator(".detail-code-line mark")).toHaveCount(0);
  await expect(requestHeaders.locator(".detail-match-count")).toHaveText("无匹配");
});

test("request log status compatibility treats success aliases as positive and exposes cancellation", async ({ page }) => {
  const logs = [
    { ...fixtureLogs[0], id: 501, requestId: "compat-success", status: "success" },
    { ...fixtureLogs[1], id: 500, requestId: "cancelled-request", status: "cancelled" },
  ];
  await installBaseRoutes(page, async (route, url) => {
    const status = url.searchParams.get("status");
    const filtered = status ? logs.filter((log) => log.status === status) : logs;
    return fulfillJson(route, { logs: filtered, nextCursor: null, hasMore: false });
  });

  await page.goto("/");
  await expect(page.getByRole("button", { name: "成功" })).toBeVisible();
  await expect(page.getByRole("button", { name: "已取消" })).toBeVisible();
  await expect(page.locator(".log-row").filter({ hasText: "成功" })).toHaveCount(1);
  await expect(page.locator(".log-row").filter({ hasText: "已取消" })).toHaveCount(1);

  await page.getByRole("combobox", { name: "状态" }).selectOption("success");
  await expect(page.locator(".log-row")).toHaveCount(1);
  await expect(page.locator(".log-row")).toContainText("成功");
});
