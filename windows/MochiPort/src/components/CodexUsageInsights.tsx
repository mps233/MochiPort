import {
  Activity,
  AlertTriangle,
  BellRing,
  CalendarDays,
  Coins,
  Flag,
  FolderKanban,
  Gauge,
  RefreshCw,
  RotateCcw,
  Sparkles,
  TerminalSquare,
  Trash2,
  Trophy,
  Zap,
  type LucideIcon,
} from "lucide-react";
import { type KeyboardEvent, useId, useMemo, useState } from "react";
import { useCodexUsage, type UsageDay, type UsageHistoryEvent } from "../state/CodexUsage";
import { compactNumber, formatUsageCost, formatUsageTokens, relativeTime } from "../utils/format";
import { Button, Card, SectionHeading, StatusPill } from "./ui";

const numberFormatter = new Intl.NumberFormat("zh-CN");

function parseDay(day: string): Date {
  return new Date(`${day}T00:00:00`);
}

function dayLabel(day: string): string {
  return new Intl.DateTimeFormat("zh-CN", { weekday: "short" }).format(parseDay(day));
}

function shortDateLabel(day: string): string {
  return new Intl.DateTimeFormat("zh-CN", { month: "numeric", day: "numeric" }).format(parseDay(day));
}

function fullDateLabel(day: string): string {
  return new Intl.DateTimeFormat("zh-CN", {
    year: "numeric",
    month: "long",
    day: "numeric",
    weekday: "short",
  }).format(parseDay(day));
}

function monthLabel(day: string): string {
  return new Intl.DateTimeFormat("zh-CN", { month: "short" }).format(parseDay(day));
}

function localDayKey(date = new Date()): string {
  return new Intl.DateTimeFormat("sv-SE", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  }).format(date);
}

function heatmapLevel(tokens: number, maxTokens: number): number {
  if (tokens <= 0 || maxTokens <= 0) return 0;
  return Math.max(1, Math.min(4, Math.ceil(tokens / maxTokens * 4)));
}

function niceAxisMaximum(value: number): number {
  if (value <= 0) return 1;
  const magnitude = 10 ** Math.floor(Math.log10(value));
  const normalized = value / magnitude;
  const factor = [1, 2, 2.5, 5, 10].find((candidate) => candidate >= normalized) ?? 10;
  return factor * magnitude;
}

type UsageRange = "week" | "month" | "heatmap" | "projects";

interface HeatmapCell {
  key: string;
  entry?: UsageDay;
  entryIndex?: number;
}

interface HeatmapWeek {
  key: string;
  days: HeatmapCell[];
  month: string;
}

const usageEventIcons: Record<UsageHistoryEvent["kind"], LucideIcon> = {
  limit: Gauge,
  depletion: AlertTriangle,
  reset: RotateCcw,
  burnSpike: Zap,
  comeback: Sparkles,
  briefing: BellRing,
  milestone: Flag,
  record: Trophy,
};

function quotaLabel(kind: string): string {
  return kind === "session5h" ? "5 小时额度" : kind === "weekly" ? "每周额度" : `${kind} 额度`;
}

function resetCountdown(timestamp: number | null | undefined): string {
  if (!timestamp) return "重置时间未知";
  const minutes = Math.max(0, Math.ceil((timestamp - Date.now()) / 60_000));
  if (minutes >= 24 * 60) return `${Math.ceil(minutes / (24 * 60))} 天后重置`;
  if (minutes >= 60) return `${Math.floor(minutes / 60)} 小时 ${minutes % 60} 分后重置`;
  return `${minutes} 分后重置`;
}

export function CodexUsageInsights() {
  const { snapshot, loading, error, history: eventHistory, refresh, clearHistory } = useCodexUsage();
  const [range, setRange] = useState<UsageRange>("week");
  const [inspectedHeatmapDay, setInspectedHeatmapDay] = useState<UsageDay>();
  const [heatmapFocusDay, setHeatmapFocusDay] = useState<string>();
  const trendDescriptionId = useId();
  const heatmapCaptionId = useId();

  const history = useMemo(
    () => snapshot?.dailyUsage?.length ? snapshot.dailyUsage : snapshot?.sevenDay ?? [],
    [snapshot?.dailyUsage, snapshot?.sevenDay],
  );
  const trend = useMemo(
    () => history.slice(range === "month" ? -30 : -7),
    [history, range],
  );
  const trendAxisMax = useMemo(
    () => niceAxisMaximum(Math.max(0, ...trend.map((entry) => entry.tokens))),
    [trend],
  );
  const trendAxisTicks = useMemo(() => [trendAxisMax, trendAxisMax / 2, 0], [trendAxisMax]);
  const trendDescription = useMemo(
    () => trend.map((entry) => `${fullDateLabel(entry.day)} ${numberFormatter.format(entry.tokens)} Token`).join("；"),
    [trend],
  );
  const projectUsage = snapshot?.sevenDayProjects ?? [];
  const visibleProjects = projectUsage.slice(0, 6);
  const projectTotal = Math.max(1, projectUsage.reduce((total, entry) => total + entry.tokens, 0));

  const heatmapEntries = useMemo(() => history.slice(-105), [history]);
  const heatmapMaxTokens = useMemo(
    () => Math.max(1, ...heatmapEntries.map((entry) => entry.tokens)),
    [heatmapEntries],
  );
  const heatmapWeeks = useMemo<HeatmapWeek[]>(() => {
    if (!heatmapEntries.length) return [];
    const firstWeekday = (parseDay(heatmapEntries[0].day).getDay() + 6) % 7;
    const cells: HeatmapCell[] = [
      ...Array.from({ length: firstWeekday }, (_, index) => ({ key: `leading-${index}` })),
      ...heatmapEntries.map((entry, entryIndex) => ({ key: entry.day, entry, entryIndex })),
    ];
    const trailingCount = (7 - cells.length % 7) % 7;
    cells.push(...Array.from({ length: trailingCount }, (_, index) => ({ key: `trailing-${index}` })));

    return Array.from({ length: cells.length / 7 }, (_, weekIndex) => {
      const days = cells.slice(weekIndex * 7, weekIndex * 7 + 7);
      const monthStart = days.find((cell) => cell.entry && parseDay(cell.entry.day).getDate() === 1)?.entry;
      const firstVisibleDay = weekIndex === 0 ? days.find((cell) => cell.entry)?.entry : undefined;
      return {
        key: `week-${weekIndex}`,
        days,
        month: monthStart ? monthLabel(monthStart.day) : firstVisibleDay ? monthLabel(firstVisibleDay.day) : "",
      };
    });
  }, [heatmapEntries]);

  const todayKey = localDayKey();
  const currentHeatmapDay = inspectedHeatmapDay ?? heatmapEntries.at(-1);
  const currentHeatmapCaption = currentHeatmapDay
    ? `${fullDateLabel(currentHeatmapDay.day)} · ${numberFormatter.format(currentHeatmapDay.tokens)} Token`
    : "最近 105 天暂无用量数据";
  const defaultHeatmapFocusDay = heatmapFocusDay ?? heatmapEntries.at(-1)?.day;

  const focusHeatmapCell = (event: KeyboardEvent<HTMLButtonElement>, entryIndex: number, weekdayIndex: number) => {
    let nextIndex: number | undefined;
    if (event.key === "ArrowLeft") nextIndex = entryIndex - 7;
    else if (event.key === "ArrowRight") nextIndex = entryIndex + 7;
    else if (event.key === "ArrowUp" && weekdayIndex > 0) nextIndex = entryIndex - 1;
    else if (event.key === "ArrowDown" && weekdayIndex < 6) nextIndex = entryIndex + 1;
    else if (event.key === "Home") nextIndex = 0;
    else if (event.key === "End") nextIndex = heatmapEntries.length - 1;
    if (nextIndex === undefined || nextIndex < 0 || nextIndex >= heatmapEntries.length) return;

    const next = event.currentTarget.closest(".usage-heatmap")
      ?.querySelector<HTMLButtonElement>(`[data-heatmap-index="${nextIndex}"]`);
    if (!next) return;
    event.preventDefault();
    next.focus();
  };

  return (
    <section className="usage-insights">
      <SectionHeading
        title="Codex 本机使用量"
        description="只读解析当前 Windows 用户的 Codex 会话日志，每 30 秒增量刷新"
        trailing={<Button variant="ghost" size="small" icon={RefreshCw} loading={loading} onClick={() => void refresh()}>刷新</Button>}
      />
      {error ? (
        <Card className="usage-unavailable">
          <TerminalSquare size={20} />
          <div><strong>无法读取 Codex 使用量</strong><p>{error}</p></div>
        </Card>
      ) : snapshot && !snapshot.available ? (
        <Card className="usage-unavailable">
          <TerminalSquare size={20} />
          <div><strong>还没有本机 Codex 日志</strong><p>开始一次 Codex 会话后，这里会显示 Token、成本估算和用量趋势。</p><code>{snapshot.sourceDirectory}</code></div>
        </Card>
      ) : (
        <Card className="usage-dashboard">
          <div className="usage-metrics">
            <div><span className="usage-metric__icon"><Gauge size={16} /></span><p>今日 Token</p><strong>{formatUsageTokens(snapshot?.todayTokens ?? 0)}</strong><small>{snapshot?.todayRequests ?? 0} 次用量记录</small></div>
            <div><span className="usage-metric__icon"><Coins size={16} /></span><p>今日成本</p><strong>{formatUsageCost(snapshot?.estimatedCostUsd ?? 0)}</strong><small>API 等值估算，不是订阅账单</small></div>
            <div><span className="usage-metric__icon"><Activity size={16} /></span><p>近 3 分钟速度</p><strong>{compactNumber(snapshot?.tokensPerMinute ?? 0)}</strong><small>Token / 分钟</small></div>
          </div>
          <div className="quota-windows" aria-label="Codex 额度窗口">
            {snapshot?.quotaWindows.length ? snapshot.quotaWindows.map((window) => (
              <div className="quota-window" key={window.kind}>
                <div className="quota-window__heading"><strong>{quotaLabel(window.kind)}</strong><span>{window.usedPercent.toLocaleString(undefined, { maximumFractionDigits: 1 })}%</span></div>
                <div className="quota-window__track" role="progressbar" aria-label={quotaLabel(window.kind)} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(window.usedPercent)}><span style={{ width: `${Math.max(0, Math.min(100, window.usedPercent))}%` }} /></div>
                <small>{resetCountdown(window.resetsAtMs)}</small>
              </div>
            )) : (
              <div className="quota-windows__empty"><Gauge size={15} /><span>Codex 日志暂未提供 5 小时和每周额度窗口</span></div>
            )}
          </div>
          <div className="usage-trend">
            <div className="usage-trend__heading">
              <div><strong>{range === "week" ? "最近 7 天" : range === "month" ? "最近 30 天" : range === "heatmap" ? "最近 105 天" : "最近 7 天项目"}</strong><span>{snapshot?.topProject ? <><FolderKanban size={12} /> 今日主要项目 {snapshot.topProject}</> : "等待项目数据"}</span></div>
              <div className="usage-trend__controls">
                <div className="usage-range-picker" role="group" aria-label="用量范围">
                  <button type="button" aria-pressed={range === "week"} onClick={() => setRange("week")}>7 天</button>
                  <button type="button" aria-pressed={range === "month"} onClick={() => setRange("month")}>30 天</button>
                  <button type="button" aria-pressed={range === "heatmap"} onClick={() => setRange("heatmap")}>热力图</button>
                  <button type="button" aria-pressed={range === "projects"} onClick={() => setRange("projects")}>项目</button>
                </div>
                <StatusPill tone="neutral">{snapshot?.lastActivityAtMs ? `${relativeTime(snapshot.lastActivityAtMs)}活动` : "暂无活动"}</StatusPill>
              </div>
            </div>
            {range === "projects" ? (
              <section className="usage-projects" aria-label="最近 7 天项目 Token 占比">
                {visibleProjects.length ? visibleProjects.map((entry) => {
                  const share = entry.tokens / projectTotal * 100;
                  return (
                    <div className="usage-project" key={entry.project}>
                      <div className="usage-project__heading">
                        <strong title={entry.project}>{entry.project}</strong>
                        <span>{numberFormatter.format(entry.tokens)} Token · {share.toFixed(share >= 10 ? 0 : 1)}%</span>
                      </div>
                      <div className="usage-project__track" role="progressbar" aria-label={`${entry.project} 项目 Token 占比`} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Number(share.toFixed(1))}>
                        <span style={{ width: `${share}%` }} />
                      </div>
                    </div>
                  );
                }) : (
                  <div className="usage-projects__empty"><FolderKanban size={18} /><span>最近 7 天暂无可识别的项目数据</span></div>
                )}
              </section>
            ) : range === "heatmap" ? (
              <div className="usage-heatmap">
                <div className="usage-heatmap__calendar">
                  <div aria-hidden />
                  <div
                    className="usage-heatmap__months"
                    style={{ gridTemplateColumns: `repeat(${Math.max(1, heatmapWeeks.length)}, var(--usage-heatmap-cell-size))` }}
                    aria-hidden
                  >
                    {heatmapWeeks.map((week) => <span className="usage-heatmap__month-label" key={week.key}>{week.month}</span>)}
                  </div>
                  <div className="usage-heatmap__weekdays" aria-hidden><span>周一</span><span /><span>周三</span><span /><span>周五</span><span /><span>周日</span></div>
                  <div
                    className="usage-heatmap__grid"
                    style={{ gridTemplateColumns: `repeat(${Math.max(1, heatmapWeeks.length)}, var(--usage-heatmap-cell-size))` }}
                    role="grid"
                    aria-label="最近 105 天 Codex Token 热力图"
                    aria-describedby={heatmapCaptionId}
                    aria-rowcount={7}
                    aria-colcount={heatmapWeeks.length}
                  >
                    {Array.from({ length: 7 }, (_, weekdayIndex) => (
                      <div className="usage-heatmap__row" role="row" key={`weekday-${weekdayIndex}`}>
                        {heatmapWeeks.map((week) => {
                          const cell = week.days[weekdayIndex];
                          if (!cell.entry || cell.entryIndex === undefined) {
                            return <span className="usage-heatmap__cell usage-heatmap__cell--padding" role="gridcell" aria-hidden key={cell.key} />;
                          }
                          const label = `${fullDateLabel(cell.entry.day)}，${numberFormatter.format(cell.entry.tokens)} Token`;
                          return (
                            <button
                              type="button"
                              role="gridcell"
                              className={`usage-heatmap__cell usage-heatmap__cell--${heatmapLevel(cell.entry.tokens, heatmapMaxTokens)}${cell.entry.day === todayKey ? " usage-heatmap__cell--today" : ""}`}
                              aria-label={label}
                              aria-current={cell.entry.day === todayKey ? "date" : undefined}
                              data-heatmap-index={cell.entryIndex}
                              tabIndex={defaultHeatmapFocusDay === cell.entry.day ? 0 : -1}
                              title={`${cell.entry.day} · ${compactNumber(cell.entry.tokens)} Token`}
                              onPointerEnter={() => setInspectedHeatmapDay(cell.entry)}
                              onPointerLeave={(pointerEvent) => {
                                if (document.activeElement !== pointerEvent.currentTarget) setInspectedHeatmapDay(undefined);
                              }}
                              onFocus={() => {
                                setHeatmapFocusDay(cell.entry?.day);
                                setInspectedHeatmapDay(cell.entry);
                              }}
                              onBlur={() => setInspectedHeatmapDay(undefined)}
                              onKeyDown={(keyboardEvent) => focusHeatmapCell(keyboardEvent, cell.entryIndex!, weekdayIndex)}
                              key={cell.key}
                            />
                          );
                        })}
                      </div>
                    ))}
                  </div>
                </div>
                <div className="usage-heatmap__footer">
                  <p className="usage-heatmap__caption" id={heatmapCaptionId}><CalendarDays size={12} aria-hidden /><strong>{currentHeatmapCaption}</strong><span>悬停或聚焦方格查看当天用量</span></p>
                  <div className="usage-heatmap__legend" aria-label="使用强度：从少到多"><span>少</span>{[0, 1, 2, 3, 4].map((level) => <i className={`usage-heatmap__cell usage-heatmap__cell--${level}`} key={level} />)}<span>多</span></div>
                </div>
              </div>
            ) : (
              <>
                <div className="usage-chart" role="img" aria-label={`最近${range === "month" ? "三十" : "七"}天 Codex Token 趋势`} aria-describedby={trendDescriptionId}>
                  <div className="usage-chart__y-axis" aria-hidden>
                    {trendAxisTicks.map((tick) => <span key={tick}>{compactNumber(tick)}</span>)}
                  </div>
                  <div className="usage-chart__plot">
                    <div className="usage-chart__gridlines" aria-hidden><i /><i /><i /></div>
                    <div className={`usage-bars${range === "month" ? " usage-bars--month" : ""}`}>
                      {trend.map((entry, index) => (
                        <div className="usage-bar" key={entry.day} title={`${entry.day} · ${compactNumber(entry.tokens)} Token`}>
                          <div className="usage-bar__track"><span style={{ height: entry.tokens > 0 ? `${Math.max(4, entry.tokens / trendAxisMax * 100)}%` : 0 }} /></div>
                          <small>{range === "week" ? dayLabel(entry.day) : index % 5 === 0 || index === trend.length - 1 ? shortDateLabel(entry.day) : ""}</small>
                        </div>
                      ))}
                    </div>
                  </div>
                </div>
                <p className="sr-only" id={trendDescriptionId}>{trendDescription || "暂无 Token 用量数据"}</p>
              </>
            )}
          </div>
          <div className="usage-history">
            <div className="usage-history__heading">
              <div><BellRing size={15} /><strong>提醒记录</strong><span>最近的额度、消耗与活动事件</span></div>
              {eventHistory.length > 0 && <Button variant="ghost" size="small" icon={Trash2} onClick={clearHistory}>清空</Button>}
            </div>
            {eventHistory.length > 0 ? (
              <div className="usage-history__list">
                {eventHistory.map((event) => {
                  const EventIcon = usageEventIcons[event.kind];
                  return (
                    <div className="usage-history__event" key={`${event.id}:${event.occurredAtMs}`}>
                      <span className={`usage-history__icon usage-history__icon--${event.kind}`}><EventIcon size={13} aria-hidden /></span>
                      <div><strong>{event.title}</strong><p>{event.body}</p></div>
                      <time dateTime={new Date(event.occurredAtMs).toISOString()}>{relativeTime(event.occurredAtMs)}</time>
                    </div>
                  );
                })}
              </div>
            ) : <p className="usage-history__empty">暂无提醒记录</p>}
          </div>
          <div className="usage-dashboard__footer"><span>扫描 {snapshot?.scannedFiles ?? 0} 个近期会话文件</span><code>{snapshot?.sourceDirectory ?? ""}</code></div>
        </Card>
      )}
    </section>
  );
}
