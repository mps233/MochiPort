import {
  ArrowDown,
  ArrowUp,
  ChevronLeft,
  Copy,
  FileJson,
  FileSearch,
  FilterX,
  RefreshCw,
  Search,
  Trash2,
  XCircle,
} from "lucide-react";
import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import type { RequestLog, RequestLogDetail } from "../api/types";
import { Button, Card, EmptyState, InlineError, Modal, SearchField, Select, StatusPill } from "../components/ui";
import { useAppModel } from "../state/AppModel";
import { compactNumber, formatBytes, formatDateTime, providerTypeLabel } from "../utils/format";

type SortOrder = "newest" | "oldest";

function normalizedLogStatus(status: string): string {
  return status.trim().toLocaleLowerCase();
}

function isSuccessfulLogStatus(status: string): boolean {
  const normalized = normalizedLogStatus(status);
  return normalized === "success"
    || normalized === "completed"
    || normalized === "ok"
    || /^2\d\d$/.test(normalized);
}

function logStatusTone(status: string): "positive" | "negative" | "warning" | "neutral" {
  const normalized = normalizedLogStatus(status);
  if (isSuccessfulLogStatus(normalized)) return "positive";
  if (normalized === "failed" || normalized === "error") return "negative";
  if (normalized === "cancelled" || normalized === "canceled") return "neutral";
  return "warning";
}

function logStatusLabel(status: string): string {
  const normalized = normalizedLogStatus(status);
  if (isSuccessfulLogStatus(normalized)) return normalized === "completed" ? "已完成" : "成功";
  if (normalized === "failed" || normalized === "error") return "失败";
  if (normalized === "cancelled" || normalized === "canceled") return "已取消";
  if (normalized === "running" || normalized === "pending") return "进行中";
  return status;
}

function cacheReadLabel(log: RequestLog): string {
  if (log.readCacheTokens == null) return "—";
  const hitRate = log.readCacheHitRate == null ? "" : ` · ${(log.readCacheHitRate * 100).toFixed(1)}%`;
  return `${compactNumber(log.readCacheTokens)}${hitRate}`;
}

function cacheWriteLabel(log: RequestLog): string {
  return log.writeCacheTokens == null ? "—" : compactNumber(log.writeCacheTokens);
}

function cacheWriteSplitLabel(log: RequestLog): string | undefined {
  if (log.writeCache5mTokens == null && log.writeCache1hTokens == null) return undefined;
  return `5m ${compactNumber(log.writeCache5mTokens ?? 0)} · 1h ${compactNumber(log.writeCache1hTokens ?? 0)}`;
}

function countMatches(value: string, query: string): number {
  const needle = query.trim().toLocaleLowerCase();
  if (!needle) return 0;
  const haystack = value.toLocaleLowerCase();
  let count = 0;
  let offset = 0;
  while (offset <= haystack.length - needle.length) {
    const match = haystack.indexOf(needle, offset);
    if (match < 0) break;
    count += 1;
    offset = match + needle.length;
  }
  return count;
}

function highlightedText(value: string, query: string): ReactNode {
  const needle = query.trim();
  if (!needle) return value || " ";
  const lowerValue = value.toLocaleLowerCase();
  const lowerNeedle = needle.toLocaleLowerCase();
  const parts: ReactNode[] = [];
  let offset = 0;
  let matchIndex = lowerValue.indexOf(lowerNeedle);
  while (matchIndex >= 0) {
    if (matchIndex > offset) parts.push(value.slice(offset, matchIndex));
    const end = matchIndex + needle.length;
    parts.push(<mark key={`${matchIndex}-${end}`}>{value.slice(matchIndex, end)}</mark>);
    offset = end;
    matchIndex = lowerValue.indexOf(lowerNeedle, offset);
  }
  if (offset < value.length) parts.push(value.slice(offset));
  return parts.length ? parts : value || " ";
}

function DetailBlock({ title, value, query }: { title: string; value?: string | null; query: string }) {
  if (!value) return null;
  const lines = value.split(/\r?\n/);
  const matches = countMatches(value, query);
  const searching = Boolean(query.trim());
  return (
    <div className="detail-code-block">
      <div className="detail-code-block__header">
        <h3>{title}</h3>
        <div>
          {searching && <span className={matches ? "detail-match-count" : "detail-match-count detail-match-count--empty"}>{matches ? `${matches} 处匹配` : "无匹配"}</span>}
          <Button variant="ghost" size="small" icon={Copy} onClick={() => void navigator.clipboard.writeText(value)}>复制</Button>
        </div>
      </div>
      <div className="detail-code-lines" aria-label={title}>
        {lines.map((line, index) => (
          <div className="detail-code-line" data-line-number={index + 1} key={`${index}-${line.length}`}>
            <span className="detail-code-line__number" aria-hidden>{index + 1}</span>
            <code>{highlightedText(line, query)}</code>
          </div>
        ))}
      </div>
    </div>
  );
}

function LogDetailPage({ detail, onBack }: { detail: RequestLogDetail; onBack: () => void }) {
  const [detailQuery, setDetailQuery] = useState("");
  const detailValues = [
    detail.requestHeadersJson,
    detail.requestJson,
    detail.upstreamRequestHeadersJson,
    detail.upstreamRequestJson,
    detail.upstreamResponseSse,
    detail.responseJson,
  ].filter((value): value is string => Boolean(value));
  const totalMatches = detailValues.reduce((sum, value) => sum + countMatches(value, detailQuery), 0);
  const searching = Boolean(detailQuery.trim());
  const writeSplit = cacheWriteSplitLabel(detail);
  return (
    <div className="log-detail-page">
      <div className="detail-nav"><Button variant="ghost" icon={ChevronLeft} onClick={onBack}>返回请求日志</Button></div>
      <div className="log-detail-header">
        <div><span>请求详情</span><h2>{detail.modelId}</h2><p>{detail.requestId}</p></div>
        <StatusPill tone={logStatusTone(detail.status)}>{logStatusLabel(detail.status)}</StatusPill>
      </div>
      <div className="detail-metrics">
        <Card><span>渠道</span><strong>{detail.channel}</strong></Card>
        <Card><span>总 token</span><strong>{compactNumber(detail.totalTokens)}</strong></Card>
        <Card><span>读缓存</span><strong>{cacheReadLabel(detail)}</strong><small>{detail.readCacheHitRate == null ? "未记录命中率" : "tokens · 命中率"}</small></Card>
        <Card><span>写缓存</span><strong>{cacheWriteLabel(detail)}</strong><small>{writeSplit ?? "未记录 5m / 1h 分拆"}</small></Card>
        <Card><span>请求大小</span><strong>{formatBytes(detail.upstreamRequestBodyBytes)}</strong></Card>
        <Card><span>延迟</span><strong>{detail.latencyMs != null ? `${detail.latencyMs} ms` : "—"}</strong></Card>
        <Card><span>TTFT</span><strong>{detail.ttftMs != null ? `${detail.ttftMs} ms` : "—"}</strong></Card>
        <Card><span>费用</span><strong>{detail.costUsd != null ? `$${detail.costUsd.toFixed(4)}` : "—"}</strong></Card>
      </div>
      {detailValues.length > 0 && (
        <div className="detail-search-toolbar">
          <SearchField aria-label="在请求详情中搜索" value={detailQuery} placeholder="在 Headers 和 Body 中搜索" onChange={(event) => setDetailQuery(event.target.value)} />
          <span className={searching && totalMatches === 0 ? "detail-search-status detail-search-status--empty" : "detail-search-status"} role="status">
            {!searching ? "支持检索下方所有脱敏内容" : totalMatches > 0 ? `找到 ${totalMatches} 处匹配` : "无匹配"}
          </span>
        </div>
      )}
      {detail.errorMessage && <div className="log-detail-error"><XCircle size={16} />{detail.errorMessage}</div>}
      <DetailBlock title="请求 Headers" value={detail.requestHeadersJson} query={detailQuery} />
      <DetailBlock title="请求 Body" value={detail.requestJson} query={detailQuery} />
      <DetailBlock title="上游请求 Headers" value={detail.upstreamRequestHeadersJson} query={detailQuery} />
      <DetailBlock title="上游请求" value={detail.upstreamRequestJson} query={detailQuery} />
      <DetailBlock title="上游流式响应" value={detail.upstreamResponseSse} query={detailQuery} />
      <DetailBlock title="最终响应" value={detail.responseJson} query={detailQuery} />
      {detailValues.length === 0 && <Card><EmptyState icon={FileJson} title="未保存请求详情" description="在 AI 网关中开启“记录请求详情”后，新请求的脱敏正文会显示在这里。" /></Card>}
    </div>
  );
}

export function RequestLogsPage() {
  const model = useAppModel();
  const [query, setQuery] = useState("");
  const [status, setStatus] = useState("all");
  const [channel, setChannel] = useState("all");
  const [modelFilter, setModelFilter] = useState("all");
  const [sort, setSort] = useState<SortOrder>("newest");
  const [detail, setDetail] = useState<RequestLogDetail>();
  const [detailLoading, setDetailLoading] = useState(false);
  const [cleanup, setCleanup] = useState<"old" | "all">();
  const initialServerQuery = useRef(true);
  const channels = useMemo(() => [...new Set(model.requestLogs.map((log) => log.channel))].sort(), [model.requestLogs]);
  const models = useMemo(() => [...new Set(model.requestLogs.map((log) => log.modelId))].sort(), [model.requestLogs]);
  const filters = useMemo(() => ({
    query,
    status: status === "all" ? null : status,
    channel: channel === "all" ? null : channel,
    modelId: modelFilter === "all" ? null : modelFilter,
    sort,
  }), [channel, modelFilter, query, sort, status]);
  useEffect(() => {
    if (model.fixtureMode) return;
    if (initialServerQuery.current) {
      initialServerQuery.current = false;
      return;
    }
    const timer = window.setTimeout(() => void model.queryRequestLogs(filters), query.trim() ? 250 : 0);
    return () => window.clearTimeout(timer);
  }, [filters, model.fixtureMode, model.queryRequestLogs, query]);
  useEffect(() => {
    if (model.fixtureMode || detail) return;
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible" && !model.loading.requestLogs) {
        void model.queryRequestLogs(filters);
      }
    }, 5_000);
    return () => window.clearInterval(timer);
  }, [detail, filters, model.fixtureMode, model.loading.requestLogs, model.queryRequestLogs]);
  const filtered = useMemo(() => (model.fixtureMode ? model.requestLogs.filter((log) => {
    const text = `${log.requestId} ${log.modelId} ${log.channel} ${log.providerType} ${log.status}`.toLowerCase();
    return (!query.trim() || text.includes(query.toLowerCase())) && (status === "all" || log.status === status) && (channel === "all" || log.channel === channel) && (modelFilter === "all" || log.modelId === modelFilter);
  }).sort((a, b) => sort === "newest" ? b.createdAtMs - a.createdAtMs : a.createdAtMs - b.createdAtMs) : model.requestLogs), [channel, model.fixtureMode, model.requestLogs, modelFilter, query, sort, status]);
  const hasFilters = query || status !== "all" || channel !== "all" || modelFilter !== "all";
  const openDetail = async (log: RequestLog) => {
    setDetailLoading(true);
    const next = await model.loadRequestLogDetail(log.id);
    setDetailLoading(false);
    if (next) setDetail(next);
  };
  if (detail) return <div className="page"><LogDetailPage detail={detail} onBack={() => setDetail(undefined)} /></div>;

  return (
    <div className="page page--table">
      {model.errors.requestLogs && !cleanup && <InlineError message={model.errors.requestLogs} onRetry={() => void model.queryRequestLogs(filters)} onDismiss={() => model.dismissError("requestLogs")} />}
      <div className="logs-toolbar">
        <SearchField value={query} placeholder="搜索请求 ID、模型、渠道或状态" onChange={(event) => setQuery(event.target.value)} />
        <Select value={status} onChange={(event) => setStatus(event.target.value)} aria-label="状态"><option value="all">全部状态</option><option value="running">进行中</option><option value="completed">已完成</option><option value="failed">失败</option><option value="cancelled">已取消</option><option value="success">成功（兼容）</option></Select>
        <Select value={channel} onChange={(event) => setChannel(event.target.value)} aria-label="渠道"><option value="all">全部渠道</option>{channels.map((entry) => <option key={entry}>{entry}</option>)}</Select>
        <Select value={modelFilter} onChange={(event) => setModelFilter(event.target.value)} aria-label="模型"><option value="all">全部模型</option>{models.map((entry) => <option key={entry}>{entry}</option>)}</Select>
        <Button variant="ghost" icon={sort === "newest" ? ArrowDown : ArrowUp} size="small" onClick={() => setSort((value) => value === "newest" ? "oldest" : "newest")}>{sort === "newest" ? "最新优先" : "最早优先"}</Button>
      </div>
      <div className="logs-subtoolbar">
        <span>{filtered.length} 条请求{hasFilters ? "（已筛选）" : ""}</span>
        <div>
          {hasFilters && <Button variant="ghost" size="small" icon={FilterX} onClick={() => { setQuery(""); setStatus("all"); setChannel("all"); setModelFilter("all"); }}>清除筛选</Button>}
          <Button variant="ghost" size="small" icon={RefreshCw} loading={model.loading.requestLogs} onClick={() => void model.queryRequestLogs(filters)}>刷新</Button>
          <Button variant="ghost" size="small" icon={Trash2} onClick={() => { model.dismissError("requestLogs"); setCleanup("old"); }}>清理日志</Button>
        </div>
      </div>

      <Card className="data-table-card logs-table-card">
        {filtered.length ? (
          <div className="data-table logs-table">
            <div className="data-table__header log-table-grid"><span>时间</span><span>模型与请求</span><span>渠道</span><span>状态</span><span>Token</span><span>缓存</span><span>请求大小</span><span>延迟</span><span>费用</span></div>
            {filtered.map((log) => (
              <button type="button" className="data-table__row log-table-grid log-row" key={log.id} onClick={() => void openDetail(log)}>
                <span className="time-cell">{formatDateTime(log.createdAtMs)}</span>
                <div className="log-primary"><strong>{log.modelId}</strong><small>{log.requestId}</small><span>{providerTypeLabel(log.providerType)}</span></div>
                <span>{log.channel}</span>
                <StatusPill tone={logStatusTone(log.status)}>{logStatusLabel(log.status)}</StatusPill>
                <span className="mono-value">{compactNumber(log.totalTokens)}</span>
                <span className="log-cache-cell"><span>读 {cacheReadLabel(log)}</span><span>写 {cacheWriteLabel(log)}</span>{cacheWriteSplitLabel(log) && <small>{cacheWriteSplitLabel(log)}</small>}</span>
                <span className="mono-value">{formatBytes(log.upstreamRequestBodyBytes)}</span>
                <span className="mono-value">{log.latencyMs != null ? `${log.latencyMs} ms` : "—"}</span>
                <span className="mono-value">{log.costUsd != null ? `$${log.costUsd.toFixed(4)}` : "—"}</span>
              </button>
            ))}
          </div>
        ) : (
          <EmptyState icon={hasFilters ? Search : FileSearch} title={hasFilters ? "没有匹配的请求" : "没有请求日志"} description={hasFilters ? "调整搜索或筛选条件后重试。" : "在 AI 网关中开启请求日志后，新请求会显示在这里。"} />
        )}
      </Card>
      {!model.fixtureMode && model.requestLogsHasMore && (
        <div className="table-load-more"><Button loading={model.loading.requestLogs} onClick={() => void model.queryRequestLogs(filters, true)}>加载更多</Button><span>已加载 {filtered.length} 条</span></div>
      )}
      {detailLoading && <div className="page-loading-overlay"><RefreshCw className="spin" size={18} />正在读取请求详情…</div>}

      <Modal open={Boolean(cleanup)} title={cleanup === "all" ? "清空全部请求日志？" : "清理旧请求日志"} description={cleanup === "all" ? "这项操作无法撤销。" : "删除 3 天前的日志，保留最近的诊断记录。"} onClose={() => setCleanup(undefined)} size="small" footer={<><Button onClick={() => setCleanup(undefined)}>取消</Button>{cleanup === "old" && <Button onClick={() => setCleanup("all")}>改为全部清空</Button>}<Button variant="danger" icon={Trash2} loading={model.busy["logs-clear"]} onClick={async () => { if (await model.clearRequestLogs(cleanup === "old" ? 3 : undefined)) { setCleanup(undefined); if (!model.fixtureMode) await model.queryRequestLogs(filters); } }}>{cleanup === "all" ? "全部清空" : "删除旧日志"}</Button></>}>
        {model.errors.requestLogs && <InlineError message={model.errors.requestLogs} onDismiss={() => model.dismissError("requestLogs")} />}
        <div className="confirmation-copy"><Trash2 size={22} /><p>{cleanup === "all" ? "请求摘要和已保存的脱敏详情都会删除。" : "清理可能需要一些时间，后台服务会在完成后回收存储空间。"}</p></div>
      </Modal>
    </div>
  );
}
