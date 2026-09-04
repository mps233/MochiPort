import {
  Bell,
  Clipboard,
  CircleAlert,
  ExternalLink,
  FolderOpen,
  Gauge,
  Info,
  KeyRound,
  Network,
  RefreshCw,
  Settings as SettingsIcon,
  ShieldCheck,
  Stethoscope,
} from "lucide-react";
import { useEffect, useState } from "react";
import { api } from "../api/client";
import { Button, Card, Field, InlineError, Modal, SectionHeading, Select, SettingsRow, StatusPill, Switch } from "../components/ui";
import {
  autostartEnabled,
  ensureNotificationPermission,
  openNativeLogDirectory,
  openNativeReleasePage,
  setAutostartEnabled,
  showNativeNotification,
} from "../native/windowsIntegration";
import { useAppModel } from "../state/AppModel";
import { useUpdateState } from "../state/useUpdateNotifications";
import {
  customMessageDrafts,
  customizableNotificationEvents,
  customMessagesFromDrafts,
  CUSTOM_NOTIFICATION_MESSAGES_STORAGE_KEY,
  NOTIFICATION_REAL_MODE_STORAGE_KEY,
  NOTIFICATION_SOUND_STORAGE_KEY,
  NOTIFY_UPDATE_STORAGE_KEY,
  parseCustomMessages,
} from "../utils/notificationMessages";

type SettingsTab = "general" | "network" | "usage" | "diagnostics";

function SettingsReadinessNotice({ loading }: { loading: boolean }) {
  return (
    <div className="settings-readiness" role="status">
      <CircleAlert size={15} aria-hidden />
      <span>{loading
        ? "正在读取后台设置，加载完成后才能保存。"
        : "后台设置暂不可用，重试成功后才能保存。"}</span>
    </div>
  );
}

function GeneralSettings() {
  const model = useAppModel();
  const settings = model.settings;
  const settingsReady = settings !== undefined;
  const [language, setLanguage] = useState(settings?.language ?? "zh-CN");
  const [theme, setTheme] = useState(settings?.theme ?? "system");
  const [closeBehavior, setCloseBehavior] = useState(() => localStorage.getItem("mochiport.close-behavior") ?? "tray");
  const [launchAtLogin, setLaunchAtLogin] = useState(false);
  const [autostartLoading, setAutostartLoading] = useState(true);
  const [autostartError, setAutostartError] = useState<string>();
  useEffect(() => { setLanguage(settings?.language ?? "zh-CN"); setTheme(settings?.theme ?? "system"); }, [settings]);
  useEffect(() => {
    let active = true;
    void autostartEnabled()
      .then((enabled) => { if (active) setLaunchAtLogin(enabled); })
      .catch((error) => { if (active) setAutostartError(error instanceof Error ? error.message : String(error)); })
      .finally(() => { if (active) setAutostartLoading(false); });
    return () => { active = false; };
  }, []);
  const updateAutostart = async (enabled: boolean) => {
    setAutostartLoading(true);
    setAutostartError(undefined);
    try {
      const actual = await setAutostartEnabled(enabled);
      setLaunchAtLogin(actual);
      if (actual !== enabled) throw new Error("Windows 没有接受登录启动设置");
    } catch (error) {
      setAutostartError(error instanceof Error ? error.message : String(error));
      setLaunchAtLogin(await autostartEnabled().catch(() => false));
    } finally {
      setAutostartLoading(false);
    }
  };
  return (
    <div className="settings-section">
      <SectionHeading title="窗口" />
      <Card className="settings-card">
        <SettingsRow title="关闭主窗口时" description="后台服务会继续运行，消息渠道和正在执行的任务不会中断。" control={<Select value={closeBehavior} onChange={(event) => { const behavior = event.target.value === "quit" ? "quit" : "tray"; setCloseBehavior(behavior); localStorage.setItem("mochiport.close-behavior", behavior); void api.setCloseBehavior(behavior); }}><option value="tray">隐藏到系统托盘</option><option value="quit">退出界面</option></Select>} />
        <SettingsRow title="登录 Windows 时启动" description={autostartError ?? "登录后在系统托盘中启动 MochiPort，不主动打开主窗口。"} control={<Switch label="登录 Windows 时启动" checked={launchAtLogin} disabled={autostartLoading} onChange={(enabled) => void updateAutostart(enabled)} />} />
      </Card>
      <SectionHeading title="显示" />
      <Card className="settings-card">
        <SettingsRow title="服务消息语言" description="用于后台服务在消息软件中的回复和提示。" control={<Select aria-label="服务消息语言" value={language} disabled={!settingsReady} onChange={(event) => setLanguage(event.target.value)}><option value="zh-CN">简体中文</option><option value="en-US">English</option></Select>} />
        <SettingsRow title="外观" description="选择系统时会自动跟随 Windows 浅色或深色模式。" control={<Select aria-label="外观" value={theme} disabled={!settingsReady} onChange={(event) => setTheme(event.target.value)}><option value="system">跟随系统</option><option value="light">浅色</option><option value="dark">深色</option></Select>} />
      </Card>
      {!settingsReady && <SettingsReadinessNotice loading={Boolean(model.loading.settings)} />}
      <div className="page-save-bar"><span>外观会立即应用到 MochiPort 窗口</span><Button variant="primary" disabled={!settingsReady} loading={model.busy["settings-save"]} onClick={() => {
        if (!settings) return;
        void model.saveSettings({ language, theme, localConnectionMode: settings.localConnectionMode, outboundProxyMode: settings.outboundProxy.mode });
      }}>保存设置</Button></div>
    </div>
  );
}

function NetworkSettings() {
  const model = useAppModel();
  const settings = model.settings;
  const settingsReady = settings !== undefined;
  const [connectionMode, setConnectionMode] = useState(settings?.localConnectionMode ?? "standard");
  const [proxyMode, setProxyMode] = useState(settings?.outboundProxy.mode ?? "system");
  const [proxyUrl, setProxyUrl] = useState(settings?.outboundProxy.url === "<none>" ? "" : settings?.outboundProxy.url ?? "");
  const [proxyUrlDirty, setProxyUrlDirty] = useState(false);
  const [clearProxyCredentials, setClearProxyCredentials] = useState(false);
  useEffect(() => {
    setConnectionMode(settings?.localConnectionMode ?? "standard");
    setProxyMode(settings?.outboundProxy.mode ?? "system");
    setProxyUrl(settings?.outboundProxy.url === "<none>" ? "" : settings?.outboundProxy.url ?? "");
    setProxyUrlDirty(false);
    setClearProxyCredentials(false);
  }, [settings]);
  return (
    <div className="settings-section">
      <SectionHeading title="本地连接" />
      <Card className="settings-card">
        <SettingsRow title="连接模式" description="VPN 兼容模式使用 localhost，避免部分代理软件拦截 127.0.0.1。" control={<Select aria-label="连接模式" value={connectionMode} disabled={!settingsReady} onChange={(event) => setConnectionMode(event.target.value)}><option value="standard">标准（127.0.0.1）</option><option value="vpnCompatible">VPN 兼容（localhost）</option></Select>} />
        <SettingsRow title="监听地址" description="MochiPort 管理服务只接受本机连接。" control={<code>{settings?.bind ?? "127.0.0.1:3847"}</code>} />
      </Card>
      <SectionHeading title="出站代理" />
      <Card className="settings-card">
        <SettingsRow title="代理模式" description="影响 AI Provider、消息平台和更新检查的出站连接。" control={<Select aria-label="代理模式" value={proxyMode} disabled={!settingsReady} onChange={(event) => setProxyMode(event.target.value)}><option value="system">跟随系统</option><option value="direct">直连</option><option value="custom">自定义</option></Select>} />
        {proxyMode === "custom" && <div className="settings-form-row"><Field label="代理 URL" hint={settings?.outboundProxy.credentialSet ? "当前只显示脱敏地址；保持不改会保留凭据，替换时请输入完整 URL。" : "支持 HTTP、HTTPS 和 SOCKS5。"}><input value={proxyUrl} disabled={!settingsReady} placeholder="socks5://127.0.0.1:1080" onChange={(event) => { setProxyUrl(event.target.value); setProxyUrlDirty(true); }} /></Field></div>}
        {proxyMode === "custom" && settings?.outboundProxy.credentialSet && <SettingsRow title="清除已保存的代理凭据" description="保存后会保留当前代理地址，但移除其中已保存的用户名和密码。" control={<Switch checked={clearProxyCredentials} label="清除已保存的代理凭据" disabled={!settingsReady} onChange={setClearProxyCredentials} />} />}
      </Card>
      {!settingsReady && <SettingsReadinessNotice loading={Boolean(model.loading.settings)} />}
      <div className="page-save-bar"><span>修改连接模式后，后台服务会在下次安全重启时应用。</span><Button variant="primary" disabled={!settingsReady || (proxyMode === "custom" && !proxyUrl.trim())} loading={model.busy["settings-save"]} onClick={() => {
        if (!settings) return;
        void model.saveSettings({ language: settings.language ?? null, theme: settings.theme ?? null, localConnectionMode: connectionMode, outboundProxyMode: proxyMode, outboundProxyUrl: proxyMode === "custom" && (proxyUrlDirty || clearProxyCredentials) ? proxyUrl : undefined });
      }}>保存网络设置</Button></div>
    </div>
  );
}

function UsageSettings() {
  const [notifications, setNotifications] = useState(() => localStorage.getItem("mochiport.notifications") === "on");
  const [limitThreshold, setLimitThreshold] = useState(() => localStorage.getItem("mochiport.notify-limit-threshold") !== "off");
  const [warn, setWarn] = useState(() => Number(localStorage.getItem("mochiport.warn-threshold") ?? 70));
  const [critical, setCritical] = useState(() => Number(localStorage.getItem("mochiport.critical-threshold") ?? 90));
  const [depletion, setDepletion] = useState(() => localStorage.getItem("mochiport.notify-depletion") !== "off");
  const [burnSpike, setBurnSpike] = useState(() => localStorage.getItem("mochiport.notify-burn-spike") !== "off");
  const [windowReset, setWindowReset] = useState(() => localStorage.getItem("mochiport.notify-window-reset") !== "off");
  const [comeback, setComeback] = useState(() => localStorage.getItem("mochiport.notify-comeback") !== "off");
  const [briefing, setBriefing] = useState(() => localStorage.getItem("mochiport.notify-briefing") !== "off");
  const [includeStreak, setIncludeStreak] = useState(() => localStorage.getItem("mochiport.fun-streak") !== "off");
  const [includeWeeklyReport, setIncludeWeeklyReport] = useState(() => localStorage.getItem("mochiport.fun-weekly-report") !== "off");
  const [milestoneRecord, setMilestoneRecord] = useState(() => localStorage.getItem("mochiport.notify-milestone-record") !== "off");
  const [realMode, setRealMode] = useState(() => localStorage.getItem(NOTIFICATION_REAL_MODE_STORAGE_KEY) === "on");
  const [soundEnabled, setSoundEnabled] = useState(() => localStorage.getItem(NOTIFICATION_SOUND_STORAGE_KEY) === "on");
  const [notifyUpdate, setNotifyUpdate] = useState(() => localStorage.getItem(NOTIFY_UPDATE_STORAGE_KEY) !== "off");
  const [customDrafts, setCustomDrafts] = useState(() => customMessageDrafts(
    parseCustomMessages(localStorage.getItem(CUSTOM_NOTIFICATION_MESSAGES_STORAGE_KEY)),
  ));
  const [saving, setSaving] = useState(false);
  const [notificationError, setNotificationError] = useState<string>();
  const [notificationStatus, setNotificationStatus] = useState<string>();
  const save = async () => {
    setNotificationError(undefined);
    setNotificationStatus(undefined);
    if (!Number.isFinite(warn) || !Number.isFinite(critical) || warn >= critical) {
      setNotificationError("提醒线必须低于严重线。");
      return;
    }
    setSaving(true);
    try {
      if (notifications && !await ensureNotificationPermission()) {
        throw new Error("Windows 通知权限未授予，请在系统设置中允许 MochiPort 通知。");
      }
      localStorage.setItem("mochiport.notifications", notifications ? "on" : "off");
      localStorage.setItem("mochiport.warn-threshold", String(warn));
      localStorage.setItem("mochiport.critical-threshold", String(critical));
      localStorage.setItem("mochiport.notify-limit-threshold", limitThreshold ? "on" : "off");
      localStorage.setItem("mochiport.notify-depletion", depletion ? "on" : "off");
      localStorage.setItem("mochiport.notify-burn-spike", burnSpike ? "on" : "off");
      localStorage.setItem("mochiport.notify-window-reset", windowReset ? "on" : "off");
      localStorage.setItem("mochiport.notify-comeback", comeback ? "on" : "off");
      localStorage.setItem("mochiport.notify-briefing", briefing ? "on" : "off");
      localStorage.setItem("mochiport.fun-streak", includeStreak ? "on" : "off");
      localStorage.setItem("mochiport.fun-weekly-report", includeWeeklyReport ? "on" : "off");
      localStorage.setItem("mochiport.notify-milestone-record", milestoneRecord ? "on" : "off");
      localStorage.setItem(NOTIFICATION_REAL_MODE_STORAGE_KEY, realMode ? "on" : "off");
      localStorage.setItem(NOTIFICATION_SOUND_STORAGE_KEY, soundEnabled ? "on" : "off");
      localStorage.setItem(NOTIFY_UPDATE_STORAGE_KEY, notifyUpdate ? "on" : "off");
      const customMessages = customMessagesFromDrafts(customDrafts);
      localStorage.setItem(CUSTOM_NOTIFICATION_MESSAGES_STORAGE_KEY, JSON.stringify(customMessages));
      setCustomDrafts(customMessageDrafts(customMessages));
      setNotificationStatus("通知设置已保存");
    } catch (error) {
      setNotificationError(error instanceof Error ? error.message : String(error));
    } finally {
      setSaving(false);
    }
  };
  const sendTestNotification = async () => {
    setNotificationError(undefined);
    try {
      if (!await showNativeNotification("MochiPort", "Windows 系统通知已连接。", soundEnabled)) {
        throw new Error("Windows 通知权限未授予，请在系统设置中允许 MochiPort 通知。");
      }
      setNotificationStatus("测试通知已发送");
    } catch (error) {
      setNotificationError(error instanceof Error ? error.message : String(error));
    }
  };
  return (
    <div className="settings-section">
      <SectionHeading title="Codex 使用量" />
      <Card className="usage-note"><Gauge size={21} /><div><strong>只读取本机 Codex 会话日志</strong><p>MochiPort 不会修改 Codex 或后台服务，也不会把使用记录发送到外部。</p></div></Card>
      <SectionHeading title="额度提醒" />
      <Card className="settings-card">
        <SettingsRow title="启用系统通知" description="在 Windows 通知中心显示额度和消耗提醒。" control={<Switch label="启用系统通知" checked={notifications} onChange={setNotifications} />} />
        <div className="slider-row"><div><strong>提醒线</strong><p>使用率达到该值时提醒一次。</p></div><input type="range" min={1} max={99} value={warn} onChange={(event) => setWarn(Number(event.target.value))} /><output>{warn}%</output></div>
        <div className="slider-row"><div><strong>严重线</strong><p>接近上限时显示高优先级通知。</p></div><input type="range" min={1} max={100} value={critical} onChange={(event) => setCritical(Number(event.target.value))} /><output>{critical}%</output></div>
      </Card>
      <SectionHeading title="通知类型" />
      <Card className="settings-card">
        <SettingsRow title="额度接近上限" description="达到提醒线或严重线时分别提醒一次。" control={<Switch label="额度接近上限" checked={limitThreshold} onChange={setLimitThreshold} />} />
        <SettingsRow title="预计即将耗尽" description="根据当前消耗速度估算剩余时间。" control={<Switch label="预计即将耗尽" checked={depletion} onChange={setDepletion} />} />
        <SettingsRow title="消耗速度突增" description="短时间内 token 消耗显著升高时提醒。" control={<Switch label="消耗速度突增" checked={burnSpike} onChange={setBurnSpike} />} />
        <SettingsRow title="额度窗口重置" description="额度窗口重新开始时提醒。" control={<Switch label="额度窗口重置" checked={windowReset} onChange={setWindowReset} />} />
        <SettingsRow title="回来继续工作" description="离开至少 3 小时后再次产生 Codex 活动时提醒。" control={<Switch label="回来继续工作" checked={comeback} onChange={setComeback} />} />
        <SettingsRow title="时段摘要" description="在上午、午后和晚间汇总今日 Token 使用量。" control={<Switch label="时段摘要" checked={briefing} onChange={setBriefing} />} />
        <SettingsRow title="时段摘要包含连续使用天数" description="在普通早间摘要中显示当前连续使用天数。" control={<Switch label="时段摘要包含连续使用天数" checked={includeStreak} onChange={setIncludeStreak} />} />
        <SettingsRow title="周一显示上周报告" description="周一早间摘要改为上周 Token、费用和项目报告。" control={<Switch label="周一显示上周报告" checked={includeWeeklyReport} onChange={setIncludeWeeklyReport} />} />
        <SettingsRow title="里程碑和新纪录" description="今日 Token 跨过里程碑或刷新近期单日纪录时提醒。" control={<Switch label="里程碑和新纪录" checked={milestoneRecord} onChange={setMilestoneRecord} />} />
      </Card>
      <SectionHeading title="摘要与通知样式" />
      <Card className="settings-card">
        <SettingsRow title="拟人化提示语" description="用更有个性的随机文案替换通知标题，通知详情仍保留用量信息。" control={<Switch label="拟人化提示语" checked={realMode} onChange={setRealMode} />} />
        <SettingsRow title="提示音" description="发送 Windows 原生通知时播放系统默认通知音。" control={<Switch label="提示音" checked={soundEnabled} onChange={setSoundEnabled} />} />
        <SettingsRow title="检测新版本" description="应用启动 15 秒后检查新版本；有更新时即使额度通知关闭也会提醒。" control={<Switch label="检测新版本" checked={notifyUpdate} onChange={setNotifyUpdate} />} />
      </Card>
      <SectionHeading title="自定义通知文案" />
      <Card className="custom-notification-card">
        <p className="notification-style-note">每行写一条候选文案，最多 12 条；支持 <code>{"{AGENT}"}</code>、<code>{"{USAGE}"}</code>、<code>{"{TOKENS}"}</code> 和 <code>{"{RESET}"}</code> 占位符。留空则使用内置文案。</p>
        {customizableNotificationEvents.map((event) => (
          <details className="custom-notification-event" key={event.key}>
            <summary><span>{event.label}</span><small>默认：{event.sampleDefaultTitle}</small></summary>
            <textarea
              aria-label={`${event.label}自定义文案`}
              rows={3}
              value={customDrafts[event.key]}
              placeholder="每行一条候选文案（最多 12 条）"
              onChange={(changeEvent) => setCustomDrafts((current) => ({
                ...current,
                [event.key]: changeEvent.target.value,
              }))}
            />
          </details>
        ))}
      </Card>
      {notificationError && <div className="form-error" role="alert">{notificationError}</div>}
      {notificationStatus && <p className="diagnostic-operation-note" role="status">{notificationStatus}</p>}
      <div className="page-save-bar"><span>原生通知只在正式安装的 Windows App 中显示</span>{notifications && <Button onClick={() => void sendTestNotification()}>发送测试通知</Button>}<Button variant="primary" loading={saving} onClick={() => void save()}>保存提醒设置</Button></div>
    </div>
  );
}

function DiagnosticsSettings() {
  const model = useAppModel();
  const update = useUpdateState();
  const lifecycle = model.lifecycle;
  const [confirmsRestart, setConfirmsRestart] = useState(false);
  const [confirmsTakeover, setConfirmsTakeover] = useState(false);
  const [confirmsCredentialRotation, setConfirmsCredentialRotation] = useState(false);
  const openLogs = async () => {
    try {
      await openNativeLogDirectory();
    } catch {
      // Error is already represented by the Settings section connection state.
    }
  };
  const copyDiagnostics = async () => {
    const lines = [
      `MochiPort Windows ${__MOCHIPORT_VERSION__}`,
      `服务：${model.statusMessage}`,
      `后台版本：${lifecycle?.runtime.productVersion ?? "未知"}`,
      `后台进程：${lifecycle?.service.pid ?? "未知"}`,
      `配置：${lifecycle?.configPath ?? "未知"}`,
      `受保护任务：${lifecycle?.protectedWorkItems.total ?? "未知"}`,
    ];
    await navigator.clipboard.writeText(lines.join("\n"));
  };
  const updateText = update.status === "checking"
    ? "正在检查更新"
    : update.status === "unsupported"
      ? "预览模式不检查更新"
      : update.status === "error"
        ? `检查失败：${update.error ?? "未知错误"}`
        : update.status === "success" && update.result
          ? update.result.updateAvailable
            ? `发现 ${update.result.latestVersion}，可前往发布页下载`
            : `已是最新版本（${update.result.currentVersion}）`
          : "尚未检查更新";
  const releaseUrl = update.result?.releaseUrl ?? "https://github.com/mps233/mochiport/releases/latest";
  const compatibilityWarning = model.status === "bridgeAvailable";
  return (
    <div className="settings-section">
      <SectionHeading title="本地服务" />
      <Card className="diagnostic-summary">
        <div className="diagnostic-summary__status"><div className="diagnostic-summary__icon"><ShieldCheck size={22} /></div><div><strong>{model.status === "available" ? "运行正常" : model.statusMessage}</strong><p>{lifecycle ? `进程 ${lifecycle.service.pid} · ${lifecycle.bind}` : "尚未读取到后台服务信息"}</p></div><StatusPill tone={model.status === "available" ? "positive" : compatibilityWarning ? "warning" : "warning"}>{model.status === "available" ? "在线" : compatibilityWarning ? "需要更新" : "需检查"}</StatusPill></div>
        {lifecycle && <div className="diagnostic-grid"><div><span>版本</span><strong>{lifecycle.runtime.productVersion}</strong></div><div><span>构建</span><strong>{lifecycle.runtime.buildNumber ?? "旧版"}</strong></div><div><span>受保护任务</span><strong>{lifecycle.protectedWorkItems.total}</strong></div><div><span>管理权限</span><strong>{lifecycle.management.canControl ? "可控制" : "只读"}</strong></div></div>}
        <div className="diagnostic-paths">{lifecycle && <><p><span>运行文件</span><code>{lifecycle.executable}</code></p><p><span>配置文件</span><code>{lifecycle.configPath}</code></p></>}</div>
        <div className="diagnostic-actions"><Button icon={RefreshCw} onClick={() => void model.refresh()}>刷新状态</Button><Button icon={FolderOpen} onClick={() => void openLogs()}>打开日志目录</Button><Button icon={Clipboard} onClick={() => void copyDiagnostics()}>复制诊断摘要</Button>{model.ownsDaemonLease && <><Button variant="danger" icon={RefreshCw} loading={model.daemonTransitionInProgress} onClick={() => { model.clearLifecycleOperationError(); setConfirmsRestart(true); }}>安全重启后台服务</Button><Button icon={KeyRound} loading={model.managementCredentialRotationInProgress} onClick={() => { model.clearLifecycleOperationError(); setConfirmsCredentialRotation(true); }}>重新生成管理凭据</Button></>}{model.daemonLeaseConflict && <Button variant="danger" icon={ShieldCheck} loading={model.daemonLeaseTakeoverInProgress} onClick={() => { model.clearLifecycleOperationError(); setConfirmsTakeover(true); }}>接管管理权</Button>}</div>
        {!model.ownsDaemonLease && lifecycle && !model.daemonLeaseConflict && <p className="diagnostic-operation-note" role="status">当前窗口为只读状态。只有完成 Windows 原生进程身份核验并持有有效租约后，才会显示安全重启操作。</p>}
        {model.daemonLeaseConflict && <p className="diagnostic-operation-note diagnostic-operation-note--warning" role="status">检测到其他 MochiPort 安装仍持有管理租约。接管会立即使对方管理权限和旧凭据失效，只能在你确认后执行。</p>}
        {model.lifecycleOperationError && !confirmsRestart && <InlineError message={model.lifecycleOperationError} onDismiss={model.clearLifecycleOperationError} />}
      </Card>
      <SectionHeading title="版本与更新" />
      <Card className="settings-card">
        <SettingsRow title="MochiPort Windows" description="React 19 · Tauri 2 · WebView2 Evergreen" control={<StatusPill tone="accent">{__MOCHIPORT_VERSION__}</StatusPill>} />
        <SettingsRow title="后台服务" description={lifecycle ? `构建 ${lifecycle.runtime.buildNumber ?? "未知"}` : "尚未连接"} control={<strong>{lifecycle?.runtime.productVersion ?? "—"}</strong>} />
        <div className="update-row"><Button icon={RefreshCw} loading={update.status === "checking"} onClick={() => void update.checkNow()}>检查更新</Button><span role="status" aria-live="polite">{updateText}</span><Button variant="link" icon={ExternalLink} onClick={() => void openNativeReleasePage(releaseUrl)}>打开发布页</Button></div>
      </Card>
      <Card className="webview-note"><Info size={16} /><p>Windows 安装包使用 WebView2 Evergreen Bootstrapper。运行时会随 Microsoft Edge 自动获得安全和性能更新。</p></Card>
      <Modal open={confirmsRestart} title="重启后台服务？" description="此操作只针对当前已核验的后台进程，不会安装、替换或切换二进制。" size="small" onClose={() => !model.daemonTransitionInProgress && setConfirmsRestart(false)} footer={<><Button disabled={model.daemonTransitionInProgress} onClick={() => setConfirmsRestart(false)}>取消</Button><Button variant="danger" loading={model.daemonTransitionInProgress} onClick={() => void model.restartDaemon().then((ok) => ok && setConfirmsRestart(false))}>确认安全重启</Button></>}>
        {model.lifecycleOperationError && <InlineError message={model.lifecycleOperationError} onDismiss={model.clearLifecycleOperationError} />}
        <div className="confirmation-copy"><CircleAlert size={22} /><p>MochiPort 会再次核验 PID、启动时间、可执行路径、SHA-256 和监听地址，并以 <code>force=false</code> 请求服务检查受保护任务。重启后只接受相同路径、相同构建的新实例。</p></div>
      </Modal>
      <Modal open={confirmsTakeover} title="接管后台服务管理权？" description="此操作会使其他安装的管理权限和旧凭据立即失效。" size="small" onClose={() => !model.daemonLeaseTakeoverInProgress && setConfirmsTakeover(false)} footer={<><Button disabled={model.daemonLeaseTakeoverInProgress} onClick={() => setConfirmsTakeover(false)}>取消</Button><Button variant="danger" loading={model.daemonLeaseTakeoverInProgress} onClick={() => void model.takeOverDaemonManagement().then((ok) => ok && setConfirmsTakeover(false))}>确认接管</Button></>}>
        {model.lifecycleOperationError && <InlineError message={model.lifecycleOperationError} onDismiss={model.clearLifecycleOperationError} />}
        <div className="confirmation-copy"><ShieldCheck size={22} /><p>MochiPort 会再次核验当前后台进程的 PID、启动时间、可执行路径、SHA-256 和监听地址，并要求服务确认接管请求。后台刷新不会自动执行此操作。</p></div>
      </Modal>
      <Modal open={confirmsCredentialRotation} title="重新生成管理凭据？" description="旧凭据会立即失效，其他使用旧凭据的管理客户端将需要重新连接。" size="small" onClose={() => !model.managementCredentialRotationInProgress && setConfirmsCredentialRotation(false)} footer={<><Button disabled={model.managementCredentialRotationInProgress} onClick={() => setConfirmsCredentialRotation(false)}>取消</Button><Button variant="danger" loading={model.managementCredentialRotationInProgress} onClick={() => void model.rotateManagementCredential().then((ok) => ok && setConfirmsCredentialRotation(false))}>确认重新生成</Button></>}>
        {model.lifecycleOperationError && <InlineError message={model.lifecycleOperationError} onDismiss={model.clearLifecycleOperationError} />}
        <div className="confirmation-copy"><KeyRound size={22} /><p>只有当前 Windows 安装持有有效租约时才能轮换。操作完成后 MochiPort 会重新读取管理状态，并确认新凭据 generation 与当前后台实例一致。</p></div>
      </Modal>
    </div>
  );
}

export function SettingsPage() {
  const model = useAppModel();
  const [tab, setTab] = useState<SettingsTab>("general");
  return (
    <div className="page page--settings">
      {model.errors.settings && <InlineError message={model.errors.settings} onRetry={() => void model.loadSection("settings", true)} onDismiss={() => model.dismissError("settings")} />}
      <div className="settings-layout">
        <nav className="settings-tabs" aria-label="设置分类">
          <button type="button" className={tab === "general" ? "active" : ""} onClick={() => setTab("general")}><SettingsIcon size={17} /><span><strong>通用</strong><small>窗口、语言与外观</small></span></button>
          <button type="button" className={tab === "network" ? "active" : ""} onClick={() => setTab("network")}><Network size={17} /><span><strong>网络</strong><small>本地连接与代理</small></span></button>
          <button type="button" className={tab === "usage" ? "active" : ""} onClick={() => setTab("usage")}><Bell size={17} /><span><strong>使用量</strong><small>额度与系统通知</small></span></button>
          <button type="button" className={tab === "diagnostics" ? "active" : ""} onClick={() => setTab("diagnostics")}><Stethoscope size={17} /><span><strong>更新与诊断</strong><small>服务状态与版本</small></span></button>
        </nav>
        <div className="settings-content">{tab === "general" ? <GeneralSettings /> : tab === "network" ? <NetworkSettings /> : tab === "usage" ? <UsageSettings /> : <DiagnosticsSettings />}</div>
      </div>
    </div>
  );
}
