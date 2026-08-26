import {
  Bot,
  CheckCircle2,
  ChevronRight,
  CircleAlert,
  FolderOpen,
  MessageCircleMore,
  Plus,
  QrCode,
  RefreshCw,
  Search,
  Send,
  ShieldCheck,
  Save,
  Trash2,
  UserRound,
  Wifi,
} from "lucide-react";
import { type CSSProperties, type FormEvent, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api/client";
import type { IMAccount, TelegramProjectGroup } from "../api/types";
import { useAppModel } from "../state/AppModel";
import { platformLabel, relativeTime } from "../utils/format";
import {
  Button,
  Card,
  EmptyState,
  Field,
  InlineError,
  Modal,
  SearchField,
  SectionHeading,
  SegmentedControl,
  Select,
  StatusPill,
  Switch,
  cn,
} from "../components/ui";

type Platform = "telegram" | "feishu" | "wechat" | "wecom";
type AccountFilter = "all" | "connected" | "disabled" | Platform;

const platformMeta: Record<Platform, { label: string; short: string; color: string; description: string }> = {
  telegram: { label: "Telegram", short: "TG", color: "#2aabee", description: "使用 BotFather token 连接机器人" },
  feishu: { label: "飞书", short: "飞", color: "#3370ff", description: "扫码或使用 App ID / App Secret" },
  wechat: { label: "微信", short: "微", color: "#07c160", description: "扫描二维码连接微信 iLink Bot" },
  wecom: { label: "企业微信", short: "企", color: "#2f7cf6", description: "扫描二维码接入企业微信 AI Bot" },
};

function PlatformBadge({ platform, size = "medium" }: { platform: string; size?: "medium" | "large" }) {
  const meta = platformMeta[platform as Platform] ?? { short: platform.slice(0, 2).toUpperCase(), color: "#657080" };
  return <span className={cn("platform-badge", size === "large" && "platform-badge--large")} style={{ "--platform-color": meta.color } as CSSProperties}>{meta.short}</span>;
}

function safeAvatarData(value?: string | null): string | undefined {
  const data = value?.trim();
  if (!data || data.length > 5_000_000) return undefined;
  if (!/^data:image\/(?!svg\+xml(?:;|,))[a-z0-9.+-]+;base64,[a-z0-9+/]+={0,2}$/i.test(data)) return undefined;
  return data;
}

function AccountAvatar({ account }: { account: IMAccount }) {
  const source = safeAvatarData(account.avatarData);
  const [failed, setFailed] = useState(false);
  useEffect(() => setFailed(false), [source]);

  if (!source || failed) return <PlatformBadge platform={account.platform} size="large" />;
  return (
    <span className="account-avatar">
      <img src={source} alt="" aria-hidden onError={() => setFailed(true)} />
    </span>
  );
}

function accountState(account: IMAccount): { label: string; tone: "positive" | "warning" | "negative" | "neutral" } {
  if (!account.enabled) return { label: "已停用", tone: "neutral" };
  if (account.connected) return { label: "已连接", tone: "positive" };
  if (account.lastError?.trim()) return { label: "连接异常", tone: "negative" };
  if (account.connecting || account.polling) return { label: "连接中", tone: "warning" };
  if (!account.configured || !account.secretSet) return { label: "待配置", tone: "neutral" };
  return { label: "未连接", tone: "neutral" };
}

function TelegramProjectGroupsModal({
  account,
  initialGroups,
  onSave,
  onClose,
}: {
  account: IMAccount;
  initialGroups: TelegramProjectGroup[];
  onSave: (groups: TelegramProjectGroup[]) => Promise<boolean>;
  onClose: () => void;
}) {
  const [groups, setGroups] = useState<TelegramProjectGroup[]>(initialGroups);
  const [localError, setLocalError] = useState<string>();
  const [saving, setSaving] = useState(false);

  const update = (index: number, field: keyof TelegramProjectGroup, value: string) => {
    setGroups((current) => current.map((group, position) => position === index ? { ...group, [field]: value } : group));
  };

  const save = async () => {
    const normalized = groups.map((group) => ({
      chatId: group.chatId.trim(),
      projectName: group.projectName.trim(),
      cwd: group.cwd.trim(),
    }));
    if (normalized.some((group) => !group.chatId || !group.projectName || !group.cwd)) {
      setLocalError("每个项目群都需要填写项目名称、群组 ID 和项目目录。");
      return;
    }
    if (new Set(normalized.map((group) => group.chatId)).size !== normalized.length) {
      setLocalError("群组 ID 不能重复。");
      return;
    }
    setSaving(true);
    setLocalError(undefined);
    if (await onSave(normalized)) onClose();
    else setLocalError("保存失败，请查看页面上的错误提示。");
    setSaving(false);
  };

  return (
    <Modal
      open
      title="Telegram 项目群"
      description={`${account.displayName ?? account.accountId} · 一个群对应一个项目`}
      onClose={onClose}
      size="large"
      footer={<><Button onClick={onClose}>取消</Button><Button variant="primary" icon={Save} loading={saving} onClick={() => void save()}>保存配置</Button></>}
    >
      <div className="telegram-project-groups-editor">
        <div className="telegram-project-groups-editor__intro">
          <FolderOpen size={16} />
          <p>机器人收到群里的第一条消息后，会自动创建 Topic，并把这个 Topic 绑定到对应项目目录。保存后需要手动重启后台服务才会生效。</p>
        </div>
        {groups.length === 0 && <EmptyState icon={FolderOpen} title="还没有项目群" description="添加一个 Telegram Forum 群组后即可开始。" />}
        <div className="telegram-project-groups-editor__list">
          {groups.map((group, index) => (
            <div className="telegram-project-group-row" key={`${index}-${group.chatId}`}>
              <div className="telegram-project-group-row__header"><strong>项目群 {index + 1}</strong><Button variant="link" size="small" icon={Trash2} onClick={() => setGroups((current) => current.filter((_, position) => position !== index))}>删除</Button></div>
              <div className="provider-editor-grid__pair">
                <Field label="项目名称"><input value={group.projectName} placeholder="例如：MochiPort" onChange={(event) => update(index, "projectName", event.target.value)} /></Field>
                <Field label="Telegram 群组 ID"><input value={group.chatId} placeholder="例如：-1001234567890" onChange={(event) => update(index, "chatId", event.target.value)} /></Field>
              </div>
              <Field label="项目目录"><input value={group.cwd} placeholder="例如：C:\\Projects\\MochiPort" onChange={(event) => update(index, "cwd", event.target.value)} /></Field>
            </div>
          ))}
        </div>
        <Button icon={Plus} onClick={() => setGroups((current) => [...current, { chatId: "", projectName: "", cwd: "" }])}>添加项目群</Button>
        <p className="field__hint">需要使用 Forum 群组，并确保 Bot 有管理 Topic 的权限。群成员都可以向该项目发送消息。</p>
        {localError && <InlineError message={localError} onDismiss={() => setLocalError(undefined)} />}
      </div>
    </Modal>
  );
}

interface ScanState {
  generation: number;
  platform: Platform;
  qrSvg: string;
  expiresAt: number;
  interval: number;
  sessionKey?: string;
  deviceCode?: string;
  status: "waiting" | "verify" | "done" | "error";
  message: string;
}

const fixtureQr = `
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 168 168">
  <rect width="168" height="168" rx="12" fill="white"/>
  <g fill="#111827">
    <path d="M16 16h48v48H16zm8 8v32h32V24zM104 16h48v48h-48zm8 8v32h32V24zM16 104h48v48H16zm8 8v32h32v-32z"/>
    <path d="M80 16h8v8h-8zm0 16h16v8H80zm8 16h8v16h-8zM72 72h16v8H72zm24 0h24v8H96zM64 88h16v16H64zm24 0h8v8h-8zm16 0h8v24h-8zm16 0h32v8h-32zm0 16h8v8h-8zm16 0h16v16h-16zm-56 8h16v8H80zm-8 16h24v8H72zm32-8h8v32h-8zm16 8h8v8h-8zm16 0h16v24h-16zm-56 16h16v8H80z"/>
  </g>
</svg>`;

function OnboardingModal({ open, onClose }: { open: boolean; onClose: () => void }) {
  const model = useAppModel();
  const [platform, setPlatform] = useState<Platform>("telegram");
  const [feishuMode, setFeishuMode] = useState<"scan" | "manual">("scan");
  const [token, setToken] = useState("");
  const [mentionOnly, setMentionOnly] = useState(false);
  const [appId, setAppId] = useState("");
  const [appSecret, setAppSecret] = useState("");
  const [scan, setScan] = useState<ScanState>();
  const [verifyCode, setVerifyCode] = useState("");
  const [localError, setLocalError] = useState<string>();
  const [startingScan, setStartingScan] = useState(false);
  const [submittingVerifyCode, setSubmittingVerifyCode] = useState(false);
  const scanGenerationRef = useRef(0);
  const pollTimerRef = useRef<{ generation: number; id: number } | undefined>(undefined);
  const startRequestRef = useRef<number | undefined>(undefined);
  const verifyRequestRef = useRef<number | undefined>(undefined);
  const finishRequestRef = useRef<number | undefined>(undefined);

  const clearPollTimer = useCallback((generation?: number) => {
    const timer = pollTimerRef.current;
    if (!timer || (generation !== undefined && timer.generation !== generation)) return;
    window.clearTimeout(timer.id);
    pollTimerRef.current = undefined;
  }, []);

  const invalidateScan = useCallback(() => {
    scanGenerationRef.current += 1;
    clearPollTimer();
    startRequestRef.current = undefined;
    verifyRequestRef.current = undefined;
    finishRequestRef.current = undefined;
    setScan(undefined);
    setLocalError(undefined);
    setVerifyCode("");
    setStartingScan(false);
    setSubmittingVerifyCode(false);
  }, [clearPollTimer]);

  const isCurrentGeneration = useCallback((generation: number) => (
    scanGenerationRef.current === generation
  ), []);

  useEffect(() => {
    if (!open) invalidateScan();
  }, [invalidateScan, open]);

  const closeModal = useCallback(() => {
    invalidateScan();
    onClose();
  }, [invalidateScan, onClose]);

  const finish = useCallback(async (generation: number, connectedPlatform: Platform) => {
    if (!isCurrentGeneration(generation) || finishRequestRef.current === generation) return;
    finishRequestRef.current = generation;
    clearPollTimer(generation);
    try {
      if (model.fixtureMode) model.completeFixtureOnboarding(connectedPlatform);
      else await model.loadSection("messaging", true);
      if (!isCurrentGeneration(generation)) return;
      closeModal();
    } finally {
      if (finishRequestRef.current === generation) finishRequestRef.current = undefined;
    }
  }, [clearPollTimer, closeModal, isCurrentGeneration, model.completeFixtureOnboarding, model.fixtureMode, model.loadSection]);

  const startScan = async () => {
    if (startRequestRef.current === scanGenerationRef.current) return;
    const requestedPlatform = platform;
    invalidateScan();
    const generation = scanGenerationRef.current;
    startRequestRef.current = generation;
    setStartingScan(true);
    setLocalError(undefined);
    try {
      if (model.fixtureMode) {
        if (!isCurrentGeneration(generation)) return;
        setScan({ generation, platform: requestedPlatform, qrSvg: fixtureQr, expiresAt: Date.now() + 120_000, interval: 2, sessionKey: "fixture", deviceCode: "fixture", status: "waiting", message: "等待扫码…" });
        return;
      }
      if (requestedPlatform === "feishu") {
        const response = await api.startFeishuOnboarding();
        if (!isCurrentGeneration(generation)) return;
        setScan({ generation, platform: requestedPlatform, qrSvg: response.qrSvg, expiresAt: Date.now() + response.expiresIn * 1000, interval: response.interval, deviceCode: response.deviceCode, status: "waiting", message: "等待你在飞书中确认…" });
      } else if (requestedPlatform === "wechat") {
        const response = await api.startWechatOnboarding();
        if (!isCurrentGeneration(generation)) return;
        setScan({ generation, platform: requestedPlatform, qrSvg: response.qrSvg, expiresAt: Date.now() + response.expiresIn * 1000, interval: 2, sessionKey: response.sessionKey, status: "waiting", message: "等待微信扫码…" });
      } else if (requestedPlatform === "wecom") {
        const response = await api.startWecomOnboarding();
        if (!isCurrentGeneration(generation)) return;
        setScan({ generation, platform: requestedPlatform, qrSvg: response.qrSvg, expiresAt: Date.now() + response.expiresIn * 1000, interval: response.interval, sessionKey: response.sessionKey, status: "waiting", message: "等待企业微信扫码…" });
      }
    } catch (error) {
      if (isCurrentGeneration(generation)) setLocalError(error instanceof Error ? error.message : "无法开始扫码");
    } finally {
      if (startRequestRef.current === generation) {
        startRequestRef.current = undefined;
        if (isCurrentGeneration(generation)) setStartingScan(false);
      }
    }
  };

  useEffect(() => {
    if (!scan || scan.status !== "waiting" || model.fixtureMode) return;
    const activeScan = scan;
    const generation = activeScan.generation;
    let disposed = false;
    let pollInFlight = false;
    let pollInterval = Math.max(1, activeScan.interval);

    const isCurrent = () => !disposed && isCurrentGeneration(generation);
    const updateCurrentScan = (update: (current: ScanState) => ScanState) => {
      setScan((current) => current?.generation === generation && isCurrentGeneration(generation) ? update(current) : current);
    };
    const clearOwnTimer = () => clearPollTimer(generation);

    function schedulePoll(delaySeconds: number) {
      if (!isCurrent() || pollInFlight || pollTimerRef.current) return;
      const timerId = window.setTimeout(() => {
        if (pollTimerRef.current?.id === timerId) pollTimerRef.current = undefined;
        void poll();
      }, Math.max(1, delaySeconds) * 1000);
      pollTimerRef.current = { generation, id: timerId };
    }

    async function poll() {
      if (!isCurrent() || pollInFlight) return;
      if (Date.now() >= activeScan.expiresAt) {
        updateCurrentScan((current) => ({ ...current, status: "error", message: "二维码已过期，请重新生成。" }));
        return;
      }
      pollInFlight = true;
      let pollAgain = false;
      try {
        if (activeScan.platform === "feishu" && activeScan.deviceCode) {
          const response = await api.pollFeishuOnboarding(activeScan.deviceCode);
          if (!isCurrent()) return;
          if (response.done) { await finish(generation, activeScan.platform); return; }
          if (response.error === "slow_down") {
            pollInterval += 5;
          } else if (response.error && response.error !== "authorization_pending") {
            throw new Error(response.errorDescription ?? response.error);
          }
        } else if (activeScan.platform === "wechat" && activeScan.sessionKey) {
          const response = await api.pollWechatOnboarding(activeScan.sessionKey);
          if (!isCurrent()) return;
          if (response.done) { await finish(generation, activeScan.platform); return; }
          if (response.needVerifyCode) {
            updateCurrentScan((current) => ({ ...current, status: "verify", message: "微信需要验证码" }));
            return;
          }
          if (response.error) throw new Error(response.error);
        } else if (activeScan.platform === "wecom" && activeScan.sessionKey) {
          const response = await api.pollWecomOnboarding(activeScan.sessionKey);
          if (!isCurrent()) return;
          if (response.done) { await finish(generation, activeScan.platform); return; }
          if (response.error) throw new Error(response.error);
        }
        pollAgain = true;
      } catch (error) {
        if (isCurrent()) updateCurrentScan((current) => ({ ...current, status: "error", message: error instanceof Error ? error.message : "扫码连接失败" }));
      } finally {
        pollInFlight = false;
        if (pollAgain && isCurrent()) schedulePoll(pollInterval);
      }
    }

    schedulePoll(pollInterval);
    return () => {
      disposed = true;
      clearOwnTimer();
    };
  }, [scan?.generation, scan?.status, model.fixtureMode]);

  const submitVerifyCode = async () => {
    const activeScan = scan;
    const code = verifyCode.trim();
    if (!activeScan?.sessionKey || activeScan.platform !== "wechat" || activeScan.status !== "verify" || !code) return;
    const generation = activeScan.generation;
    if (verifyRequestRef.current === generation) return;
    verifyRequestRef.current = generation;
    setSubmittingVerifyCode(true);
    setLocalError(undefined);
    try {
      const response = await api.pollWechatOnboarding(activeScan.sessionKey, code);
      if (!isCurrentGeneration(generation)) return;
      if (response.done) await finish(generation, activeScan.platform);
      else if (response.error) throw new Error(response.error);
      else setScan((current) => current?.generation === generation ? {
        ...current,
        status: response.needVerifyCode ? "verify" : "waiting",
        message: response.needVerifyCode ? "微信需要验证码" : response.status ?? "继续等待微信确认…",
      } : current);
    } catch (error) {
      if (isCurrentGeneration(generation)) setLocalError(error instanceof Error ? error.message : "验证码校验失败");
    } finally {
      if (verifyRequestRef.current === generation) {
        verifyRequestRef.current = undefined;
        if (isCurrentGeneration(generation)) setSubmittingVerifyCode(false);
      }
    }
  };

  const submitManual = async (event: FormEvent) => {
    event.preventDefault();
    setLocalError(undefined);
    model.dismissError("messaging");
    let ok = false;
    if (platform === "telegram") ok = await model.addTelegram(token.trim(), mentionOnly);
    if (platform === "feishu") ok = await model.addFeishu(appId.trim(), appSecret.trim());
    if (ok) closeModal();
    else {
      // AppModel owns the section error. Yield once so its state update is
      // visible inside this still-open modal rather than only behind it.
      await Promise.resolve();
    }
  };

  const isScan = platform === "wechat" || platform === "wecom" || (platform === "feishu" && feishuMode === "scan");
  const qrUrl = scan ? `data:image/svg+xml;charset=utf-8,${encodeURIComponent(scan.qrSvg)}` : undefined;
  return (
    <Modal open={open} title="连接消息渠道" description="凭据由本地后台服务验证并保存，不会显示在账号列表中。" onClose={closeModal} size="large">
      <div className="onboarding-layout">
        <div className="platform-picker" role="tablist" aria-label="选择消息渠道">
          {(Object.keys(platformMeta) as Platform[]).map((entry) => {
            const meta = platformMeta[entry];
            return (
              <button type="button" role="tab" aria-selected={platform === entry} className={cn("platform-option", platform === entry && "platform-option--selected")} key={entry} onClick={() => {
                if (entry === platform) return;
                invalidateScan();
                setPlatform(entry);
              }}>
                <PlatformBadge platform={entry} />
                <div><strong>{meta.label}</strong><span>{meta.description}</span></div>
              </button>
            );
          })}
        </div>
        <div className="onboarding-content">
          <div className="onboarding-content__heading"><PlatformBadge platform={platform} size="large" /><div><h3>{platformMeta[platform].label}</h3><p>{platformMeta[platform].description}</p></div></div>
          {model.errors.messaging && <InlineError message={model.errors.messaging} onDismiss={() => model.dismissError("messaging")} />}
          {platform === "feishu" && <SegmentedControl label="飞书连接方式" value={feishuMode} onChange={(mode) => {
            if (mode === feishuMode) return;
            invalidateScan();
            setFeishuMode(mode);
          }} options={[{ value: "scan", label: "扫码连接" }, { value: "manual", label: "手动填写" }]} />}
          {localError && <div className="form-error" role="alert"><CircleAlert size={15} />{localError}</div>}
          {isScan ? (
            <div className="scan-panel">
              {!scan ? (
                <div className="scan-intro">
                  <div className="scan-intro__icon"><QrCode size={30} /></div>
                  <h3>生成一次性二维码</h3>
                  <p>二维码只用于当前连接流程，过期后需要重新生成。</p>
                  <Button variant="primary" icon={QrCode} loading={startingScan} onClick={() => void startScan()}>生成二维码</Button>
                </div>
              ) : scan.status === "verify" ? (
                <div className="verify-panel">
                  <ShieldCheck size={30} />
                  <h3>输入微信验证码</h3>
                  <p>验证码显示在手机微信中，用来确认本次连接。</p>
                  <input value={verifyCode} autoFocus inputMode="numeric" placeholder="输入验证码" onChange={(event) => setVerifyCode(event.target.value)} />
                  <Button variant="primary" disabled={!verifyCode.trim()} loading={submittingVerifyCode} onClick={() => void submitVerifyCode()}>继续</Button>
                </div>
              ) : (
                <div className="qr-panel">
                  <div className="qr-panel__code">{qrUrl && <img src={qrUrl} alt={`${platformMeta[platform].label} 连接二维码`} />}</div>
                  <div className={cn("qr-panel__status", scan.status === "error" && "qr-panel__status--error")}>
                    {scan.status === "waiting" ? <RefreshCw className="spin" size={15} /> : <CircleAlert size={15} />}
                    {scan.message}
                  </div>
                  {model.fixtureMode && scan.status === "waiting" && <Button variant="primary" icon={CheckCircle2} onClick={() => void finish(scan.generation, scan.platform)}>模拟扫码完成</Button>}
                  {scan.status === "error" && <Button icon={RefreshCw} loading={startingScan} onClick={() => void startScan()}>重新生成</Button>}
                </div>
              )}
            </div>
          ) : (
            <form className="onboarding-form" onSubmit={submitManual}>
              {platform === "telegram" ? (
                <>
                  <Field label="Bot token" hint="在 Telegram 中向 @BotFather 创建机器人后获得。"><input type="password" autoFocus value={token} placeholder="123456789:AA..." onChange={(event) => setToken(event.target.value)} /></Field>
                  <label className="checkbox-row"><input type="checkbox" checked={mentionOnly} onChange={(event) => setMentionOnly(event.target.checked)} /><span><strong>群聊中仅响应 @机器人 的消息</strong><small>私聊消息始终会处理。</small></span></label>
                  <Button variant="primary" icon={Send} type="submit" disabled={!token.trim()} loading={model.busy.onboarding}>验证并连接</Button>
                </>
              ) : (
                <>
                  <Field label="App ID"><input autoFocus value={appId} placeholder="cli_xxxxxxxxxxxxxxxx" onChange={(event) => setAppId(event.target.value)} /></Field>
                  <Field label="App Secret"><input type="password" value={appSecret} placeholder="输入 App Secret" onChange={(event) => setAppSecret(event.target.value)} /></Field>
                  <Button variant="primary" type="submit" disabled={!appId.trim() || !appSecret.trim()} loading={model.busy.onboarding}>验证并连接</Button>
                </>
              )}
            </form>
          )}
        </div>
      </div>
    </Modal>
  );
}

export function MessagingPage() {
  const model = useAppModel();
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<AccountFilter>("all");
  const [onboarding, setOnboarding] = useState(false);
  const [deleting, setDeleting] = useState<IMAccount>();
  const [editingProjectGroups, setEditingProjectGroups] = useState<IMAccount>();
  const [expandedIds, setExpandedIds] = useState<Set<string>>(() => new Set());
  const pageError = model.errors.messaging ?? model.accountsRefreshError;
  const filtered = useMemo(() => model.accounts.filter((account) => {
    const matchesQuery = !query.trim() || `${account.displayName ?? ""} ${account.accountId} ${account.platform}`.toLowerCase().includes(query.toLowerCase());
    const matchesFilter = filter === "all" || (filter === "connected" ? account.connected && account.enabled : filter === "disabled" ? !account.enabled : account.platform === filter);
    return matchesQuery && matchesFilter;
  }), [filter, model.accounts, query]);

  return (
    <div className="page">
      {pageError && !deleting && !onboarding && <InlineError message={pageError} onRetry={() => void model.loadSection("messaging", true)} onDismiss={() => {
        if (model.errors.messaging) model.dismissError("messaging");
        else model.dismissAccountsRefreshError();
      }} />}
      <div className="messaging-toolbar">
        <SearchField value={query} placeholder="搜索账号" onChange={(event) => setQuery(event.target.value)} />
        <Select value={filter} onChange={(event) => setFilter(event.target.value as AccountFilter)} aria-label="筛选消息账号">
          <option value="all">全部账号</option><option value="connected">已连接</option><option value="disabled">已停用</option><option value="telegram">Telegram</option><option value="feishu">飞书</option><option value="wechat">微信</option><option value="wecom">企业微信</option>
        </Select>
        <Button variant="primary" icon={Plus} onClick={() => setOnboarding(true)}>连接渠道</Button>
      </div>

      <SectionHeading title="消息账号" description={`${model.accounts.length} 个账号 · ${model.accounts.filter((account) => account.connected && account.enabled).length} 个在线`} trailing={<Button variant="ghost" size="small" icon={RefreshCw} loading={model.loading.messaging} onClick={() => void model.loadSection("messaging", true)}>刷新</Button>} />
      <div className="account-list">
        {filtered.map((account) => {
          const state = accountState(account);
          const accountError = account.lastError?.trim();
          const id = `${account.platform}:${account.accountId}`;
          const expanded = expandedIds.has(id);
          const detailId = `account-details-${encodeURIComponent(id)}`;
          const displayName = account.displayName?.trim() || platformLabel(account.platform);
          return (
            <Card className={cn("account-card", expanded && "account-card--expanded")} key={id}>
              <div className="account-card__row">
                <button
                  type="button"
                  className="account-card__disclosure"
                  aria-expanded={expanded}
                  aria-controls={detailId}
                  aria-label={`${expanded ? "收起" : "展开"}账号详情 ${displayName}`}
                  onClick={() => setExpandedIds((current) => {
                    const next = new Set(current);
                    if (next.has(id)) next.delete(id); else next.add(id);
                    return next;
                  })}
                >
                  <ChevronRight className="account-card__chevron" size={14} aria-hidden />
                  <AccountAvatar account={account} />
                  <div className="account-card__main">
                    <h3>{displayName}</h3>
                    <p>{platformLabel(account.platform)} · {account.accountId}</p>
                  </div>
                </button>
                <StatusPill tone={state.tone}>{state.label}</StatusPill>
                <Switch checked={account.enabled} label={`${account.enabled ? "停用" : "启用"}${displayName}`} disabled={model.busy[`account:${id}`]} onChange={(enabled) => void model.toggleAccount(account, enabled)} />
              </div>
              {expanded && (
                <div className="account-card__expanded" id={detailId}>
                  <div className="account-card__facts">
                    <div><span>配置</span><strong>{account.configured ? "完整" : "不完整"}</strong></div>
                    <div><span>凭据</span><strong>{account.secretSet ? "已设置" : "未设置"}</strong></div>
                    {account.polling && <div><span>轮询</span><strong>运行中</strong></div>}
                  </div>
                  {accountError ? (
                    <div className="account-card__error"><CircleAlert size={13} />{accountError}</div>
                  ) : account.lastInboundAtMs ? (
                    <div className="account-card__activity"><Wifi size={13} />最近收到消息：{relativeTime(account.lastInboundAtMs)}</div>
                  ) : account.lastEventAtMs ? (
                    <div className="account-card__activity"><UserRound size={13} />最近活动：{relativeTime(account.lastEventAtMs)}</div>
                  ) : null}
                  {account.platform === "telegram" && (
                    <Button variant="secondary" size="small" icon={FolderOpen} onClick={() => setEditingProjectGroups(account)}>
                      配置项目群
                    </Button>
                  )}
                  <Button variant="link" size="small" icon={Trash2} onClick={() => { model.dismissError("messaging"); setDeleting(account); }}>删除账号</Button>
                </div>
              )}
            </Card>
          );
        })}
        {filtered.length === 0 && <Card><EmptyState icon={query || filter !== "all" ? Search : MessageCircleMore} title={model.accounts.length ? "没有匹配的账号" : "还没有消息账号"} description={model.accounts.length ? "调整搜索或筛选条件后重试。" : "连接一个消息渠道，就能从手机向这台电脑上的 Codex 发起任务。"} action={!model.accounts.length && <Button variant="primary" icon={Plus} onClick={() => setOnboarding(true)}>连接消息渠道</Button>} /></Card>}
      </div>

      <OnboardingModal open={onboarding} onClose={() => setOnboarding(false)} />
      {editingProjectGroups && (
        <TelegramProjectGroupsModal
          account={editingProjectGroups}
          initialGroups={model.telegramProjectGroupAccounts.find((item) => item.accountId === editingProjectGroups.accountId)?.projectGroups ?? []}
          onSave={(groups) => model.saveTelegramProjectGroups(editingProjectGroups.accountId, groups)}
          onClose={() => setEditingProjectGroups(undefined)}
        />
      )}
      <Modal open={Boolean(deleting)} title="删除消息账号？" description="删除后需要重新验证凭据或扫码才能再次连接。" onClose={() => setDeleting(undefined)} size="small" footer={<><Button onClick={() => setDeleting(undefined)}>取消</Button><Button variant="danger" icon={Trash2} loading={deleting ? model.busy[`account-delete:${deleting.platform}:${deleting.accountId}`] : false} onClick={async () => { if (deleting && await model.deleteAccount(deleting)) setDeleting(undefined); }}>删除账号</Button></>}>
        {model.errors.messaging && <InlineError message={model.errors.messaging} onDismiss={() => model.dismissError("messaging")} />}
        <div className="confirmation-copy"><PlatformBadge platform={deleting?.platform ?? "telegram"} /><p>{deleting?.displayName ?? deleting?.accountId} 将从 MochiPort 本地配置中移除。</p></div>
      </Modal>
    </div>
  );
}
