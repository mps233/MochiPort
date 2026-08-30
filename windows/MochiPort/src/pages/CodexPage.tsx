import {
  AppWindow,
  Boxes,
  Check,
  CircleAlert,
  Code2,
  ExternalLink,
  FileKey,
  KeyRound,
  LoaderCircle,
  Play,
  RefreshCw,
  RotateCcw,
  ShieldCheck,
  Sparkles,
  TerminalSquare,
  Trash2,
  Unplug,
  Wifi,
} from "lucide-react";
import { useState } from "react";
import { Button, Card, InlineError, Modal, SectionHeading, SettingsRow, StatusPill, Switch } from "../components/ui";
import { useAppModel } from "../state/AppModel";
import { providerTypeLabel } from "../utils/format";

function CheckRow({ ok, title, detail }: { ok: boolean; title: string; detail: string }) {
  return (
    <div className="check-row">
      <span className={ok ? "check-row__icon check-row__icon--ok" : "check-row__icon check-row__icon--warning"}>
        {ok ? <Check size={15} /> : <CircleAlert size={15} />}
      </span>
      <div>
        <strong>{title}</strong>
        <span>{detail}</span>
      </div>
    </div>
  );
}

function providerModeLabel(mode: string | null | undefined): string {
  switch (mode) {
    case "threadrelay": return "MochiPort AI 网关";
    case "direct-api": return "Codex 原始设置";
    case "unknown": return "尚未连接";
    default: return "未配置";
  }
}

function providerDisplayName(name: string | null | undefined): string {
  return name?.trim().toLowerCase() === "ai-gateway" ? "MochiPort" : name ?? "";
}

function enhancedPhaseLabel(phase: string): string {
  switch (phase) {
    case "preparing": return "正在准备";
    case "launching": return "正在启动";
    case "waitingForApp": return "等待 Codex";
    case "injecting": return "正在同步模型";
    case "ready": return "已就绪";
    case "failed": return "启动失败";
    case "cancelled": return "已取消";
    default: return phase;
  }
}

export function CodexPage() {
  const model = useAppModel();
  const status = model.codexStatus;
  const [technicalOpen, setTechnicalOpen] = useState(false);
  const [confirmUninstall, setConfirmUninstall] = useState(false);
  const gatewayEnabled = model.gateway?.enabled ?? model.dashboard?.aiGatewayEnabled;
  const routeEnabled = status?.providerMode === "threadrelay" && gatewayEnabled !== false;
  const canEnhancedLaunch = Boolean(status?.configured && routeEnabled);
  const codexConnectionTransitionInProgress = Boolean(
    model.busy["codex:configure"]
      || model.busy["codex:uninstall"]
      || model.busy["codex:direct-api-mode"],
  );
  const remoteControlReady = Boolean(status && (!status.remoteControlSupported || status.remoteControlConfigured));
  const ready = Boolean(
    routeEnabled
      && status.configOk
      && status.authOk
      && status.providerOk
      && status.guiConfigured
      && remoteControlReady,
  );
  const enhancedOperation = model.codexEnhancedOperation;

  const toggleConnection = async (enabled: boolean) => {
    if (enabled) await model.runCodexAction("configure");
    else setConfirmUninstall(true);
  };

  return (
    <div className="page">
      {model.errors.codex && !confirmUninstall && (
        <InlineError message={model.errors.codex} onRetry={() => void model.loadSection("codex", true)} onDismiss={() => model.dismissError("codex")} />
      )}

      <Card className="codex-connection-card">
        <div className="codex-connection-card__lead">
          <div className="codex-orbit" aria-hidden>
            <span className="codex-orbit__ring" />
            <Code2 size={24} />
            <span className="codex-orbit__pulse" />
          </div>
          <div>
            <div className="codex-connection-card__title">
              <h2>连接 MochiPort</h2>
              <StatusPill tone={ready ? "positive" : routeEnabled ? "warning" : "neutral"}>{ready ? "已连接" : routeEnabled ? "需处理" : "未连接"}</StatusPill>
            </div>
            <p>让 Codex App、VS Code 插件和 CLI 通过 MochiPort 接收来自消息渠道的任务。</p>
          </div>
        </div>
        <Switch checked={Boolean(routeEnabled)} onChange={(value) => void toggleConnection(value)} disabled={model.busy["codex:configure"] || model.busy["codex:uninstall"]} label="连接 MochiPort" />
      </Card>

      <div className="codex-grid">
        <div className="codex-grid__main">
          <SectionHeading title="连接状态" description="各项检查全部通过后，Codex 才能安全接收远程任务。" />
          <Card className="check-list">
            <CheckRow ok={Boolean(status?.configOk)} title="Codex 配置" detail={status?.configOk ? "本地配置已指向 MochiPort" : status?.configError ?? "尚未写入连接配置"} />
            <CheckRow ok={Boolean(status?.authOk)} title="身份认证" detail={status?.authOk ? "ChatGPT 兼容认证可用" : status?.authError ?? "需要登录 Codex"} />
            <CheckRow ok={Boolean(status?.providerOk)} title="模型服务" detail={status?.providerOk ? `${status?.providers.length ?? 0} 个 Provider 可用` : "请先配置 AI 网关"} />
            <CheckRow ok={Boolean(status?.guiConfigured)} title="桌面控制" detail={status?.guiConfigured ? "可以管理 Codex App" : status?.guiError ?? "需要修复桌面环境"} />
            <CheckRow ok={remoteControlReady} title="远程控制" detail={!status?.remoteControlSupported ? "当前 Codex 版本不支持，连接不受影响" : status.remoteControlConfigured ? "已允许 MochiPort 控制这台电脑" : status.remoteControlError ?? "需要开启 remote-control"} />
          </Card>

          <SectionHeading title="模型连接" description="Codex 当前看到的 Provider 与模型路由" trailing={<Button variant="ghost" size="small" icon={RefreshCw} loading={model.busy["codex:models/refresh"]} onClick={() => void model.runCodexAction("models/refresh")}>刷新模型</Button>} />
          <Card className="provider-status-list">
            {(status?.providers ?? []).map((provider) => (
              <div className="provider-status-row" key={provider.name}>
                <div className="provider-logo"><Boxes size={17} /></div>
                <div className="provider-status-row__copy">
                  <strong>{providerDisplayName(provider.name)}</strong>
                  <span>{provider.baseUrl ?? "本地默认地址"}</span>
                </div>
                <StatusPill tone={provider.secretSet ? "positive" : "warning"}>{provider.secretSet ? "密钥已保存" : "缺少密钥"}</StatusPill>
                {provider.supportsWebsockets && <span className="quiet-label"><Wifi size={13} /> WebSocket</span>}
              </div>
            ))}
            {!status?.providers.length && (
              <div className="compact-empty"><Unplug size={18} /> 尚未读取到 Codex Provider</div>
            )}
          </Card>
        </div>

        <aside className="codex-grid__aside">
          <SectionHeading title="快速操作" />
          <Card className="action-stack">
            {canEnhancedLaunch ? (
              <>
                <Button
                  variant="primary"
                  icon={Play}
                  loading={model.codexEnhancedLaunchInProgress}
                  disabled={model.codexEnhancedLaunchInProgress || codexConnectionTransitionInProgress}
                  onClick={() => void model.beginCodexEnhancedLaunch()}
                >{model.codexEnhancedLaunchError ? "重新尝试增强启动" : "增强模式启动 Codex"}</Button>
                <p>启动前会检查正在运行的 Codex，避免覆盖未完成的会话。</p>
              </>
            ) : (
              <>
                <Button
                  variant="primary"
                  icon={Sparkles}
                  loading={model.busy["codex:configure"]}
                  disabled={!status}
                  onClick={() => void model.runCodexAction("configure")}
                >连接 MochiPort</Button>
                <p>{!status
                  ? "正在读取 Codex 接入状态，请稍候。"
                  : status.providerMode === "direct-api"
                    ? "当前使用直连 API。请先连接 MochiPort，再使用增强启动。"
                    : "请先完成 MochiPort 接入，再使用增强启动。"}</p>
              </>
            )}
            {enhancedOperation && (
              <div className={`enhanced-operation enhanced-operation--${enhancedOperation.phase}`}>
                <div className="enhanced-operation__status">
                  {model.codexEnhancedLaunchInProgress ? <LoaderCircle size={16} /> : enhancedOperation.phase === "ready" ? <Check size={16} /> : <CircleAlert size={16} />}
                  <div>
                    <strong>{enhancedOperation.message}</strong>
                    <span>{enhancedPhaseLabel(enhancedOperation.phase)}{model.codexEnhancedUsesLegacyFallback ? " · 兼容模式" : ""}</span>
                  </div>
                </div>
                {enhancedOperation.error && <p className="enhanced-operation__error">{enhancedOperation.error}</p>}
                {enhancedOperation.recovery && <p>{enhancedOperation.recovery}</p>}
                {model.canCancelCodexEnhancedLaunch && (
                  <Button variant="ghost" size="small" onClick={() => void model.cancelCodexEnhancedLaunch()}>取消启动</Button>
                )}
              </div>
            )}
            <div className="action-stack__rule" />
            <Button variant="secondary" icon={RotateCcw} loading={model.busy["codex:repair"]} onClick={() => void model.runCodexAction("repair")}>修复连接配置</Button>
            <Button variant="secondary" icon={ExternalLink} loading={model.busy["codex:direct-api-mode"]} disabled={status?.providerMode === "direct-api"} onClick={() => void model.runCodexAction("direct-api-mode")}>切换到直连 API</Button>
          </Card>

          <SectionHeading title="当前模式" />
          <Card className="mode-card">
            <div className="mode-card__icon"><ShieldCheck size={19} /></div>
            <strong>{providerModeLabel(status?.providerMode)}</strong>
            <p>{status?.providerModeMessage ?? "连接后会在这里显示 Codex 的路由模式。"}</p>
            <div className="mode-card__meta"><TerminalSquare size={14} /> {providerDisplayName(status?.activeProvider) || "无活动 Provider"}</div>
          </Card>
        </aside>
      </div>

      <SectionHeading title="技术信息" description="排查连接问题时使用；路径和状态均来自本机后台服务。" />
      <Card className="technical-card">
        <button type="button" className="disclosure-button" aria-expanded={technicalOpen} onClick={() => setTechnicalOpen((value) => !value)}>
          <FileKey size={17} /> 查看 Codex 环境与连接详情
          <span>{technicalOpen ? "收起" : "展开"}</span>
        </button>
        {technicalOpen && (
          <div className="technical-card__content">
            <SettingsRow title="Codex Home" control={<code>{status?.codexHome ?? "—"}</code>} />
            <SettingsRow title="连接模式" control={<code>{status?.connectionMode ?? "—"}</code>} />
            <SettingsRow title="图像生成" control={<StatusPill tone={status?.imageGenerationEnabled ? "positive" : "neutral"}>{status?.imageGenerationEnabled ? "已启用" : "未启用"}</StatusPill>} />
            <SettingsRow title="GUI 环境" control={<StatusPill tone={status?.guiConfigured ? "positive" : "warning"}>{status?.guiConfigured ? "已配置" : "需要修复"}</StatusPill>} />
          </div>
        )}
      </Card>

      <Modal
        open={model.codexEnhancedWaitingForAppExit}
        title="请先退出 Codex"
        description="MochiPort 会持续检查；Codex 完全退出后会自动继续增强启动。"
        onClose={() => void model.cancelCodexEnhancedLaunch()}
        size="small"
        footer={<Button onClick={() => void model.cancelCodexEnhancedLaunch()}>取消启动</Button>}
      >
        <div className="enhanced-wait-panel">
          <div className="enhanced-wait-panel__icon"><AppWindow size={27} /><LoaderCircle size={15} /></div>
          <strong>等待 Codex App 退出</strong>
          <p>请保存当前工作并从 Codex 菜单中选择退出。无需再次点击启动。</p>
        </div>
      </Modal>

      <Modal
        open={confirmUninstall}
        title="断开 Codex？"
        description="这会移除 MochiPort 写入的本地连接配置，不会删除会话或卸载 Codex。"
        onClose={() => setConfirmUninstall(false)}
        size="small"
        footer={
          <>
            <Button onClick={() => setConfirmUninstall(false)}>取消</Button>
            <Button variant="danger" icon={Trash2} loading={model.busy["codex:uninstall"]} onClick={async () => { if (await model.runCodexAction("uninstall")) setConfirmUninstall(false); }}>断开连接</Button>
          </>
        }
      >
        {model.errors.codex && <InlineError message={model.errors.codex} onDismiss={() => model.dismissError("codex")} />}
        <div className="confirmation-copy"><KeyRound size={22} /><p>已保存的模型服务密钥仍保留在 MochiPort 后台服务中。</p></div>
      </Modal>
    </div>
  );
}
