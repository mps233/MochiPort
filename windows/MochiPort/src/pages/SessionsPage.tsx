import {
  Copy,
  Folder,
  Inbox,
  Laptop,
  RefreshCw,
  Search,
} from "lucide-react";
import { useMemo, useState } from "react";
import type { CodexSession } from "../api/types";
import { Button, Card, EmptyState, InlineError, SearchField, SectionHeading, StatusPill, cn } from "../components/ui";
import { useAppModel } from "../state/AppModel";
import { formatDateTime } from "../utils/format";

type RouteFilter = "all" | "gateway" | "direct";
const MOCHIPORT_PROVIDER = "MochiPort";
const isMochiPortProvider = (provider: string) => provider === MOCHIPORT_PROVIDER || provider === "ai-gateway";
type SessionSourceState = "waiting" | "connected" | "offline" | "unavailable";

function sessionTitle(session: CodexSession): string {
  return session.name?.trim() || session.preview.trim() || "未命名会话";
}

function projectPath(session: CodexSession): string | undefined {
  const value = session.cwd?.trim();
  if (!value) return undefined;
  if (/^file:\/\//i.test(value)) {
    try {
      const url = new URL(value);
      const decoded = decodeURIComponent(url.pathname);
      return /^\/[a-z]:\//i.test(decoded) ? decoded.slice(1).replaceAll("/", "\\") : decoded;
    } catch {
      return value;
    }
  }
  return value.replace(/[\\/]+$/, "");
}

function projectTitle(path: string | undefined): string {
  if (!path) return "未指定项目";
  return path.split(/[\\/]/).filter(Boolean).at(-1) ?? path;
}

function sourceCopy(state: SessionSourceState) {
  switch (state) {
    case "waiting": return { title: "等待连接", detail: "正在等待本地服务和 Codex App 就绪。", emptyTitle: "正在等待 Codex App", emptyMessage: "连接建立后，会话会自动显示在这里。", tone: "neutral" as const };
    case "connected": return { title: "已连接", detail: "可读取当前 Codex App 可见的本机会话。", emptyTitle: "当前没有可见会话", emptyMessage: "在 Codex App 中创建或打开会话后，刷新即可看到。", tone: "positive" as const };
    case "offline": return { title: "未连接", detail: "打开 Codex App，确认远程控制已启用，然后刷新。", emptyTitle: "Codex App 尚未连接", emptyMessage: "请打开 Codex App，确认远程控制已启用，然后刷新。", tone: "warning" as const };
    case "unavailable": return { title: "不可用", detail: "尚未配置 Codex App 的远程控制。", emptyTitle: "Codex App 尚未配置", emptyMessage: "请先完成 Codex App 接入，再读取本机会话。", tone: "negative" as const };
  }
}

export function SessionsPage() {
  const model = useAppModel();
  const [query, setQuery] = useState("");
  const [routeFilter, setRouteFilter] = useState<RouteFilter>("all");
  const filtered = useMemo(() => {
    const matching = model.sessions.filter((session) => {
      const matchesQuery = !query.trim() || `${sessionTitle(session)} ${session.preview} ${session.cwd ?? ""} ${session.modelProvider} ${session.id}`.toLowerCase().includes(query.toLowerCase());
      const gatewayRoute = isMochiPortProvider(session.modelProvider);
      const matchesRoute = routeFilter === "all" || (routeFilter === "gateway" ? gatewayRoute : !gatewayRoute);
      return matchesQuery && matchesRoute;
    });
    const groups = new Map<string, { title: string; sessions: CodexSession[] }>();
    for (const session of matching) {
      const path = projectPath(session);
      const key = path?.toLocaleLowerCase() ?? "__mochiport_unknown_project__";
      const group = groups.get(key) ?? { title: projectTitle(path), sessions: [] };
      group.sessions.push(session);
      groups.set(key, group);
    }
    return [...groups.values()]
      .map((group) => ({
        ...group,
        sessions: group.sessions.sort((left, right) => right.updatedAt - left.updatedAt || left.id.localeCompare(right.id)),
      }))
      .sort((left, right) => {
        const activity = (right.sessions[0]?.updatedAt ?? 0) - (left.sessions[0]?.updatedAt ?? 0);
        return activity || left.title.localeCompare(right.title, undefined, { numeric: true });
      })
      .flatMap((group) => group.sessions);
  }, [model.sessions, query, routeFilter]);
  const sourceState: SessionSourceState = !model.dashboard
    ? model.loading.sessions ? "waiting" : "unavailable"
    : model.dashboard.executionClients.codexApp.connected
      ? "connected"
      : model.dashboard.executionClients.codexApp.configured ? "offline" : "unavailable";
  const source = sourceCopy(sourceState);

  return (
    <div className="page page--table">
      <Card className="source-banner">
        <div className="source-banner__icon"><Laptop size={19} /></div>
        <div><strong>读取来源：当前 Codex App</strong><p>{source.detail} 会话保留在 Codex 原位置。</p></div>
        <StatusPill tone={source.tone}>{source.title} · 共 {model.sessions.length} 个</StatusPill>
      </Card>
      {model.errors.sessions && <InlineError message={model.errors.sessions} onRetry={() => void model.loadSection("sessions", true)} onDismiss={() => model.dismissError("sessions")} />}

      <div className="sessions-toolbar">
        <SearchField value={query} placeholder="搜索会话" onChange={(event) => setQuery(event.target.value)} />
        <div className="filter-pills" aria-label="会话路由筛选">
          {(["all", "gateway", "direct"] as RouteFilter[]).map((entry) => <button type="button" className={cn(routeFilter === entry && "active")} key={entry} onClick={() => setRouteFilter(entry)}>{entry === "all" ? "全部" : entry === "gateway" ? "AI 网关" : "原始 Provider"}</button>)}
        </div>
        <Button variant="ghost" icon={RefreshCw} size="small" loading={model.loading.sessions} onClick={() => void model.loadSection("sessions", true)}>刷新</Button>
      </div>

      <SectionHeading title="会话" description={`${filtered.length} 个会话`} />
      <Card className="data-table-card sessions-table-card">
        {filtered.length ? (
          <div className="data-table sessions-table">
            <div className="data-table__header session-table-grid">
              <span>会话</span><span>项目</span><span>路由</span><span>最后更新</span><span />
            </div>
            {filtered.map((session) => {
              const throughGateway = isMochiPortProvider(session.modelProvider);
              return (
                <div className="data-table__row session-table-grid" key={session.id}>
                  <div className="session-primary"><strong>{sessionTitle(session)}</strong><span>{session.preview || session.id}</span></div>
                  <div className="session-project"><strong><Folder size={11} />{projectTitle(projectPath(session))}</strong><small>{projectPath(session) ?? "没有工作目录"}</small></div>
                  <div className="route-cell"><StatusPill tone={throughGateway ? "accent" : "neutral"}>{throughGateway ? "MochiPort" : "原始"}</StatusPill><small>{throughGateway ? MOCHIPORT_PROVIDER : session.modelProvider}</small></div>
                  <span className="time-cell">{formatDateTime(session.updatedAt)}</span>
                  <Button variant="ghost" size="small" icon={Copy} aria-label="复制会话 ID" onClick={() => void navigator.clipboard.writeText(session.id)}>复制 ID</Button>
                </div>
              );
            })}
          </div>
        ) : (
          <EmptyState icon={query ? Search : Inbox} title={query || routeFilter !== "all" ? "没有匹配的会话" : source.emptyTitle} description={query || routeFilter !== "all" ? "调整搜索或路由筛选后重试。" : source.emptyMessage} />
        )}
      </Card>
    </div>
  );
}
