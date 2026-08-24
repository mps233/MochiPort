import {
  ArrowRight,
  Bot,
  Boxes,
  ChevronDown,
  CircleAlert,
  CircleDot,
  Clock3,
  Download,
  ExternalLink,
  Laptop,
  MessageCircleMore,
  RefreshCw,
  Router,
  ShieldCheck,
  Sparkles,
  X,
} from "lucide-react";
import { useState } from "react";
import { useAppModel } from "../state/AppModel";
import { useUpdateState } from "../state/useUpdateNotifications";
import { openNativeReleasePage } from "../native/windowsIntegration";
import { relativeTime } from "../utils/format";
import { Button, Card, cn, EmptyState, IconButton, InlineError, SectionHeading, StatusPill } from "../components/ui";
import { CodexUsageInsights } from "../components/CodexUsageInsights";

interface StartStepProps {
  number: string;
  title: string;
  detail: string;
  action?: string;
  onAction?: () => void;
  primary?: boolean;
}

function StartStep({ number, title, detail, action, onAction, primary }: StartStepProps) {
  return (
    <div className="start-step">
      <span className="start-step__number">{number}</span>
      <div>
        <h3>{title}</h3>
        <p>{detail}</p>
        {action && onAction && (
          <Button variant={primary ? "primary" : "link"} size="small" onClick={onAction}>
            {action}<ArrowRight size={13} />
          </Button>
        )}
      </div>
    </div>
  );
}

function ConnectionTopology() {
  const { dashboard } = useAppModel();
  const codexConnected = dashboard?.remoteControlConnected ?? false;
  const channelCount = dashboard
    ? Object.values(dashboard.messageChannels).reduce((sum, channel) => sum + channel.connectedAccountCount, 0)
    : 0;
  return (
    <Card className="topology-card">
      <div className="topology-card__heading">
        <div>
          <h3>连接拓扑</h3>
          <p>消息从你的设备进入 MochiPort，再交给当前 Codex 客户端。</p>
        </div>
        <StatusPill tone={codexConnected ? "positive" : "warning"}>{codexConnected ? "链路就绪" : "等待 Codex"}</StatusPill>
      </div>
      <div className="topology" aria-label="消息渠道到 MochiPort 再到 Codex 的连接拓扑">
        <div className={cn("topology-node", channelCount > 0 && "topology-node--active")}>
          <MessageCircleMore size={20} />
          <span>消息渠道</span>
          <small>{channelCount} 个已连接</small>
        </div>
        <div className={cn("topology-link", channelCount > 0 && "topology-link--active")}><span /></div>
        <div className="topology-node topology-node--hub topology-node--active">
          <span className="brand-mark brand-mark--topology" aria-hidden><span /></span>
          <span>MochiPort</span>
          <small>本机安全转发</small>
        </div>
        <div className={cn("topology-link", codexConnected && "topology-link--active")}><span /></div>
        <div className={cn("topology-node", codexConnected && "topology-node--active")}>
          <Bot size={20} />
          <span>Codex</span>
          <small>{codexConnected ? "远程控制已连接" : "尚未连接"}</small>
        </div>
      </div>
    </Card>
  );
}

function updateVersionLabel(version: string): string {
  const normalized = version.trim().replace(/^[vV]+/u, "");
  return `v${normalized}`;
}

function OverviewUpdateNotice() {
  const update = useUpdateState();
  const result = update.result;
  if (update.status !== "success" || !result?.updateAvailable || update.dismissed) return null;

  return (
    <Card className="update-notice" role="status" aria-live="polite">
      <div className="update-notice__icon" aria-hidden><Download size={19} /></div>
      <div className="update-notice__copy">
        <h2>MochiPort 有新版本</h2>
        <p>当前 {updateVersionLabel(result.currentVersion)}，可更新至 {updateVersionLabel(result.latestVersion)}。</p>
      </div>
      <Button
        variant="primary"
        size="small"
        icon={ExternalLink}
        onClick={() => void openNativeReleasePage(result.releaseUrl).catch(() => undefined)}
      >
        打开发布页
      </Button>
      <IconButton aria-label="关闭更新提示" title="本次会话不再提示" onClick={update.dismiss}>
        <X size={15} />
      </IconButton>
    </Card>
  );
}

export function OverviewPage() {
  const model = useAppModel();
  const [expanded, setExpanded] = useState(() => localStorage.getItem("mochiport.start-here") !== "collapsed");
  const dashboard = model.dashboard;
  const connectedChannels = dashboard
    ? Object.values(dashboard.messageChannels).reduce((sum, channel) => sum + channel.connectedAccountCount, 0)
    : 0;
  const configuredClients = dashboard
    ? Object.values(dashboard.executionClients).filter((client) => client.configured).length
    : 0;

  const toggleGuide = () => {
    setExpanded((current) => {
      localStorage.setItem("mochiport.start-here", current ? "collapsed" : "expanded");
      return !current;
    });
  };

  return (
    <div className="page page--overview">
      {model.status === "unavailable" && model.errors.overview && (
        <InlineError message={model.errors.overview} onRetry={() => void model.startDaemon()} />
      )}
      <OverviewUpdateNotice />
      <section className={cn("start-here", !expanded && "start-here--collapsed")}>
        <button type="button" className="start-here__title" onClick={toggleGuide} aria-expanded={expanded}>
          <div className="start-here__glyph"><Sparkles size={20} /></div>
          <div>
            <h2>从这里开始</h2>
            <p>{expanded ? "MochiPort 把这台电脑上的 Codex 连接到手机里的消息软件。" : "连接模型、消息渠道与 Codex"}</p>
          </div>
          <ChevronDown size={17} className={cn("start-here__chevron", expanded && "start-here__chevron--open")} />
        </button>
        {expanded && (
          <div className="start-here__content">
            <div className="start-here__rule" />
            <h3>第一次使用只需要四步</h3>
            <div className="start-here__steps">
              <StartStep number="1" title="添加模型服务" detail="填写 API 地址和 Key，保存即可。" action="配置模型" primary onAction={() => model.setSelection("gateway")} />
              <StartStep number="2" title="连接消息渠道" detail="选择 Telegram、飞书、微信或企业微信。" action="连接消息渠道" onAction={() => model.setSelection("messaging")} />
              <StartStep number="3" title="连接 Codex" detail="打开连接，让 Codex 接入 MochiPort。" action="连接 Codex" onAction={() => model.setSelection("codex")} />
              <StartStep number="4" title="从手机开始使用" detail="给机器人发一条消息，任务会在这台电脑上执行。" />
            </div>
            <div className="start-here__note"><ShieldCheck size={15} /> 凭据只保存在本机，浏览器界面不会读取或显示已保存的 Key。</div>
          </div>
        )}
      </section>

      <SectionHeading title="运行概览" description="本机后台服务与主要连接的即时状态" />
      <div className="metric-grid">
        <Card className="metric-card">
          <div className="metric-card__icon metric-card__icon--green"><CircleDot size={18} /></div>
          <span>后台服务</span>
          <strong>{model.status === "available" ? "运行正常" : model.status === "checking" ? "检查中" : "需要处理"}</strong>
          <small>{model.status === "available" && model.lastCheckedAt ? `${relativeTime(model.lastCheckedAt)}刷新` : model.statusMessage}</small>
        </Card>
        <Card className="metric-card">
          <div className="metric-card__icon"><Boxes size={18} /></div>
          <span>模型服务</span>
          <strong>{dashboard?.aiGatewayProviderCount ?? 0}</strong>
          <small>{dashboard?.aiGatewayEnabled ? "AI 网关已开启" : "AI 网关未开启"}</small>
        </Card>
        <Card className="metric-card">
          <div className="metric-card__icon"><MessageCircleMore size={18} /></div>
          <span>消息渠道</span>
          <strong>{connectedChannels}</strong>
          <small>{connectedChannels > 0 ? "账号在线" : "等待连接账号"}</small>
        </Card>
        <Card className="metric-card">
          <div className="metric-card__icon"><Laptop size={18} /></div>
          <span>执行客户端</span>
          <strong>{configuredClients}</strong>
          <small>{dashboard?.remoteControlConnected ? "Codex 已连接" : "等待 Codex 连接"}</small>
        </Card>
      </div>

      <CodexUsageInsights />

      <ConnectionTopology />

      <SectionHeading
        title="Sub2API 账号池"
        description="余额、倍率与调度可用性由后台服务统一读取"
        trailing={model.sub2ApiAdmin?.configured && <Button variant="ghost" size="small" icon={RefreshCw} loading={model.sub2ApiPoolLoading} onClick={() => void model.refreshSub2ApiPool(true)}>刷新</Button>}
      />
      {model.sub2ApiPoolError && <InlineError message={model.sub2ApiPoolError} onRetry={() => void model.refreshSub2ApiPool(true)} />}
      {Boolean(model.sub2ApiPool?.warnings?.length) && <div className="pool-warnings" role="status"><CircleAlert size={16} /><div>{model.sub2ApiPool?.warnings?.map((warning) => <p key={warning}>{warning}</p>)}</div></div>}
      {!model.sub2ApiAdmin?.configured ? (
        <Card>
          <EmptyState icon={Router} title="尚未连接 Sub2API" description="连接管理接口后，可在这里查看账号余额、倍率和调度状态。" action={<Button variant="primary" onClick={() => model.setSelection("gateway")}>前往连接</Button>} />
        </Card>
      ) : (
        <Card className="pool-summary">
          <div className="pool-summary__header">
            <div>
              <h3>{model.sub2ApiAdmin.baseUrl}</h3>
              <p>已安全连接 · 管理密钥不会回传到界面</p>
            </div>
            <StatusPill tone="positive">已连接</StatusPill>
          </div>
          <div className="pool-accounts">
            {(model.sub2ApiPool?.accounts ?? []).slice(0, 4).map((account) => (
              <div className="pool-account" key={account.id}>
                <div className="pool-account__icon"><Router size={16} /></div>
                <div className="pool-account__copy">
                  <strong>{account.name}</strong>
                  <span>{account.platform} · {account.schedulable ? "可调度" : "暂停调度"}</span>
                </div>
                <div className="pool-account__balance">
                  <strong>{account.upstreamBalance.unlimited ? "不限额" : account.upstreamBalance.remaining != null ? `${account.upstreamBalance.remaining.toFixed(2)} ${account.upstreamBalance.unit ?? ""}` : "—"}</strong>
                  <span>{account.upstreamBilling.effectiveRateMultiplier ? `${account.upstreamBilling.effectiveRateMultiplier}× 倍率` : "倍率未知"}</span>
                </div>
              </div>
            ))}
            {!model.sub2ApiPool?.accounts.length && <div className="compact-empty"><Clock3 size={17} /> 尚未读取到账号数据</div>}
          </div>
          <button type="button" className="pool-summary__footer" onClick={() => model.setSelection("gateway")}>
            管理完整账号池 <ArrowRight size={14} />
          </button>
        </Card>
      )}
    </div>
  );
}
