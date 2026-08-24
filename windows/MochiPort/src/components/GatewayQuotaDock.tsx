import {
  Info,
  ListTree,
  LoaderCircle,
  RefreshCw,
  Server,
  Settings2,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api/client";
import type {
  GatewayProvider,
  GatewayProviderRecentAccountResponse,
  GatewayProviderUsage,
  Sub2ApiAccount,
} from "../api/types";
import { useAppModel } from "../state/AppModel";
import { providerTypeLabel } from "../utils/format";
import { Button, IconButton, Modal, cn } from "./ui";

const PROGRESS_REFERENCE = 20;
const WARNING_THRESHOLD = 3;

type QuotaTone = "normal" | "warning" | "critical" | "unavailable";

interface MeterPresentation {
  fraction?: number;
  statusText: string;
  tone: QuotaTone;
  warningThreshold?: number;
}

interface BalanceLike {
  remaining?: number | null;
  unlimited: boolean;
  accountValid?: boolean | null;
  accountStatus?: string | null;
  state?: string;
  balanceStatus?: string;
}

function meterPresentation(balance: BalanceLike): MeterPresentation {
  if (balance.accountValid === false) {
    return { fraction: 0, statusText: balance.accountStatus?.trim() || "账户不可用", tone: "critical" };
  }
  if (balance.unlimited) return { fraction: 1, statusText: "无限额度", tone: "normal" };
  if (typeof balance.remaining !== "number" || !Number.isFinite(balance.remaining)) {
    const available = (balance.state ?? balance.balanceStatus) === "available";
    return { statusText: available ? "额度可用" : "额度暂不可用", tone: available ? "normal" : "unavailable" };
  }
  const fraction = Math.max(0, Math.min(balance.remaining / PROGRESS_REFERENCE, 1));
  if (balance.remaining <= 0) return { fraction: 0, statusText: "额度耗尽", tone: "critical", warningThreshold: WARNING_THRESHOLD };
  if (balance.remaining < WARNING_THRESHOLD) return { fraction, statusText: "余额偏低", tone: "warning", warningThreshold: WARNING_THRESHOLD };
  return { fraction, statusText: "余额充足", tone: "normal", warningThreshold: WARNING_THRESHOLD };
}

function cachedPresentation(
  presentation: MeterPresentation,
  loading: boolean,
  failed: boolean,
): MeterPresentation {
  if (!loading && !failed) return presentation;
  return {
    ...presentation,
    statusText: loading ? "正在刷新 · 上次数据" : "刷新失败 · 上次数据",
    tone: loading ? "unavailable" : presentation.tone === "critical" ? "critical" : "warning",
  };
}

function amountText(value: number, unit?: string | null): string {
  if (!Number.isFinite(value)) return "—";
  const amount = Math.abs(value).toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
  const sign = value < 0 ? "-" : "";
  const normalizedUnit = unit?.trim().toUpperCase();
  if (normalizedUnit === "USD") return `${sign}$${amount}`;
  if (normalizedUnit === "CNY" || normalizedUnit === "RMB") return `${sign}¥${amount}`;
  return normalizedUnit ? `${sign}${amount} ${normalizedUnit}` : `${sign}${amount}`;
}

function siteDisplayName(siteUrl?: string | null): string | undefined {
  if (!siteUrl) return undefined;
  try {
    const labels = new URL(siteUrl).hostname.toLowerCase().split(".");
    if (labels.length < 2) return labels[0];
    const candidates = labels.slice(0, -1);
    const infrastructure = new Set(["api", "openai", "vip", "www"]);
    return candidates.find((entry) => !infrastructure.has(entry)) ?? candidates.at(-1);
  } catch {
    return undefined;
  }
}

function fixtureUsage(provider: GatewayProvider): GatewayProviderUsage {
  return {
    source: "sub2api",
    balanceStatus: "available",
    billingStatus: "available",
    remaining: provider.name === "Anthropic" ? null : 42.86,
    unlimited: provider.name === "Anthropic",
    unit: "USD",
    todayCost: 1.47,
    effectiveRateMultiplier: provider.name === "Anthropic" ? 0.8 : 1,
    observedAt: new Date().toISOString(),
  };
}

function fixtureRecentAccount(
  provider: GatewayProvider,
  accounts: Sub2ApiAccount[],
): GatewayProviderRecentAccountResponse {
  const preferred = accounts.find((account) => (
    account.platform.toLowerCase().includes(provider.name.toLowerCase())
  )) ?? accounts[0];
  return {
    ok: true,
    providerName: provider.name,
    account: preferred
      ? { accountId: preferred.id, accountName: preferred.name, createdAt: new Date().toISOString() }
      : null,
  };
}

export function GatewayQuotaDock() {
  const model = useAppModel();
  const providers = useMemo(() => [...(model.gateway?.providers ?? [])].sort((left, right) => (
    Number(right.enabled) - Number(left.enabled)
      || right.weight - left.weight
      || left.name.localeCompare(right.name)
  )), [model.gateway?.providers]);
  const [selectedProviderName, setSelectedProviderName] = useState<string>();
  const [usage, setUsage] = useState<GatewayProviderUsage>();
  const [usageError, setUsageError] = useState<string>();
  const [recentAccountResponse, setRecentAccountResponse] = useState<GatewayProviderRecentAccountResponse>();
  const [recentAccountError, setRecentAccountError] = useState<string>();
  const [loading, setLoading] = useState(false);
  const [detailsOpen, setDetailsOpen] = useState(false);
  const requestGeneration = useRef(0);

  useEffect(() => {
    if (!model.gateway && model.status === "available") void model.loadSection("gateway");
  }, [model.gateway, model.loadSection, model.status]);

  useEffect(() => {
    if (selectedProviderName && providers.some((provider) => provider.name === selectedProviderName)) return;
    setSelectedProviderName(
      providers.find((provider) => provider.enabled && provider.secretSet)?.name
        ?? providers.find((provider) => provider.enabled)?.name
        ?? providers[0]?.name,
    );
  }, [providers, selectedProviderName]);

  const selectedProvider = providers.find((provider) => provider.name === selectedProviderName);
  const recentAccount = model.sub2ApiPool?.accounts.find(
    (account) => account.id === recentAccountResponse?.account?.accountId,
  );

  const loadQuota = useCallback(async (forceAccountRefresh = false) => {
    const provider = selectedProvider;
    const generation = requestGeneration.current + 1;
    requestGeneration.current = generation;
    if (!provider || !provider.secretSet) {
      setUsage(undefined);
      setUsageError(undefined);
      setRecentAccountResponse(undefined);
      setRecentAccountError(undefined);
      setLoading(false);
      return;
    }
    setLoading(true);
    setUsageError(undefined);
    setRecentAccountError(undefined);
    const usageRequest = model.fixtureMode
      ? Promise.resolve({ ok: true, providerName: provider.name, usage: fixtureUsage(provider) })
      : api.providerUsage(provider.name);
    const recentAccountRequest = model.sub2ApiAdmin?.configured
      ? Promise.all([
          model.refreshSub2ApiPool(forceAccountRefresh),
          model.fixtureMode
            ? Promise.resolve(fixtureRecentAccount(provider, model.sub2ApiPool?.accounts ?? []))
            : api.providerRecentAccount(provider.name),
        ]).then(([, response]) => response)
      : Promise.resolve(undefined);
    const [usageResult, recentAccountResult] = await Promise.allSettled([usageRequest, recentAccountRequest]);
    if (requestGeneration.current !== generation) return;
    if (usageResult.status === "fulfilled") setUsage(usageResult.value.usage);
    else setUsageError(usageResult.reason instanceof Error ? usageResult.reason.message : "无法读取余额和计费信息");
    if (recentAccountResult.status === "fulfilled") setRecentAccountResponse(recentAccountResult.value);
    else setRecentAccountError(recentAccountResult.reason instanceof Error ? recentAccountResult.reason.message : "无法识别最近使用账号");
    setLoading(false);
  }, [model.fixtureMode, model.refreshSub2ApiPool, model.sub2ApiAdmin?.configured, model.sub2ApiPool?.accounts, selectedProvider]);

  useEffect(() => {
    requestGeneration.current += 1;
    setUsage(undefined);
    setUsageError(undefined);
    setRecentAccountResponse(undefined);
    setRecentAccountError(undefined);
    setDetailsOpen(false);
    void loadQuota();
    return () => { requestGeneration.current += 1; };
  }, [selectedProviderName]); // loadQuota deliberately runs once for each selected Provider.

  useEffect(() => {
    if (!selectedProvider || !model.sub2ApiAdmin?.configured) return;
    let active = true;
    const provider = selectedProvider;
    const timer = window.setInterval(() => {
      const request = model.fixtureMode
        ? Promise.resolve(fixtureRecentAccount(provider, model.sub2ApiPool?.accounts ?? []))
        : api.providerRecentAccount(provider.name);
      void request.then((response) => {
        if (active) {
          setRecentAccountResponse(response);
          setRecentAccountError(undefined);
        }
      }).catch((error) => {
        if (active) setRecentAccountError(error instanceof Error ? error.message : "无法识别最近使用账号");
      });
    }, 8_000);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [model.fixtureMode, model.sub2ApiAdmin?.configured, model.sub2ApiPool?.accounts, selectedProvider]);

  const balance = recentAccount?.upstreamBalance ?? usage;
  const basePresentation: MeterPresentation = balance
    ? meterPresentation(balance)
    : selectedProvider?.secretSet === false
      ? { statusText: "需要 API Key", tone: "unavailable" as const }
      : loading
        ? { statusText: "正在读取", tone: "unavailable" as const }
        : usageError
          ? { statusText: "读取失败", tone: "critical" as const }
          : { statusText: selectedProvider ? "等待刷新" : "未配置", tone: "unavailable" as const };
  const presentation: MeterPresentation = balance
    ? cachedPresentation(
        basePresentation,
        loading || model.sub2ApiPoolLoading,
        Boolean(usageError || model.sub2ApiPoolError),
      )
    : basePresentation;
  const balanceText = recentAccount?.upstreamBalance.unlimited || usage?.unlimited
    ? "无限额度"
    : typeof recentAccount?.upstreamBalance.remaining === "number"
      ? amountText(recentAccount.upstreamBalance.remaining, recentAccount.upstreamBalance.unit)
      : typeof usage?.remaining === "number"
        ? amountText(usage.remaining, usage.unit)
        : selectedProvider?.secretSet === false
          ? "未保存 API Key"
          : loading
            ? "正在读取"
            : usageError
              ? "暂不可用"
              : selectedProvider ? "尚未读取" : "未配置";
  const rate = recentAccount?.upstreamBilling.effectiveRateMultiplier
    ?? recentAccount?.upstreamBilling.resolvedRateMultiplier
    ?? recentAccount?.localRateMultiplier
    ?? usage?.effectiveRateMultiplier
    ?? usage?.resolvedRateMultiplier;
  const accountTitle = siteDisplayName(recentAccount?.siteUrl)
    ?? recentAccount?.name
    ?? selectedProvider?.name
    ?? "AI Gateway";
  const accountSubtitle = recentAccount
    ? `最近使用 · ${recentAccount.name}`
    : (selectedProvider ? providerTypeLabel(selectedProvider.providerType) : "尚未配置 Provider");
  const observedAt = recentAccount?.upstreamBalance.observedAt ?? usage?.observedAt;
  const plan = recentAccount?.upstreamBalance.planName
    ?? recentAccount?.upstreamBalance.mode
    ?? usage?.planName
    ?? usage?.balanceMode;

  return (
    <footer className="gateway-quota-dock" aria-label="AI 网关额度" data-testid="gateway-quota-dock">
      <div className="gateway-quota-dock__surface">
        <label className="gateway-quota-dock__provider">
          <span className="gateway-quota-dock__brand"><Server size={16} aria-hidden /></span>
          <span className="gateway-quota-dock__provider-copy"><strong>{accountTitle}</strong><small>{accountSubtitle}</small></span>
          <select
            aria-label="选择额度 Provider"
            value={selectedProviderName ?? ""}
            disabled={!providers.length}
            onChange={(event) => setSelectedProviderName(event.target.value || undefined)}
          >
            {!providers.length && <option value="">尚未配置 Provider</option>}
            {providers.map((provider) => <option value={provider.name} key={provider.name}>{provider.name}</option>)}
          </select>
        </label>
        <div className="gateway-quota-dock__summary" aria-label={`${accountTitle}，剩余额度 ${balanceText}，${presentation.statusText}${rate != null ? `，倍率 ${rate}×` : ""}`}>
          <div className="gateway-quota-dock__line"><span>剩余额度</span><strong>{balanceText}</strong><em className={`quota-tone quota-tone--${presentation.tone}`}>{presentation.statusText}</em>{rate != null && <small>倍率 {rate}×</small>}</div>
          <div className="gateway-quota-dock__track" title={presentation.warningThreshold == null ? presentation.statusText : `进度按 ${amountText(PROGRESS_REFERENCE, recentAccount?.upstreamBalance.unit ?? usage?.unit)} 参考值显示；低于 ${amountText(presentation.warningThreshold, recentAccount?.upstreamBalance.unit ?? usage?.unit)} 时提醒。`}>
            {presentation.fraction != null && <span className={`quota-tone-bg quota-tone-bg--${presentation.tone}`} style={{ width: `${Math.max(0, presentation.fraction) * 100}%` }} />}
          </div>
        </div>
        <div className="gateway-quota-dock__actions">
          <IconButton aria-label="刷新额度" disabled={!selectedProvider?.secretSet || loading} onClick={() => void loadQuota(true)}>{loading ? <LoaderCircle className="spin" size={15} /> : <RefreshCw size={15} />}</IconButton>
          <IconButton aria-label="额度详情" disabled={!selectedProvider} onClick={() => setDetailsOpen(true)}><Info size={15} /></IconButton>
          <IconButton aria-label="打开日志列表" onClick={() => model.setSelection("requestLogs")}><ListTree size={15} /></IconButton>
          <IconButton aria-label="打开网关设置" onClick={() => model.setSelection("gateway")}><Settings2 size={15} /></IconButton>
        </div>
      </div>
      <Modal
        open={detailsOpen}
        title={`${accountTitle} · 额度详情`}
        description={selectedProvider ? `${selectedProvider.name} · ${providerTypeLabel(selectedProvider.providerType)}` : undefined}
        size="small"
        onClose={() => setDetailsOpen(false)}
        footer={<><Button onClick={() => setDetailsOpen(false)}>关闭</Button><Button icon={RefreshCw} loading={loading} disabled={!selectedProvider?.secretSet} onClick={() => void loadQuota(true)}>刷新</Button></>}
      >
        <div className="quota-detail">
          <div className="quota-detail__hero"><span>剩余额度</span><strong>{balanceText}</strong><small className={`quota-tone quota-tone--${presentation.tone}`}>{presentation.statusText}</small></div>
          <dl>
            {recentAccount && <><dt>最近使用账号</dt><dd>{recentAccount.name}</dd></>}
            <dt>当前倍率</dt><dd>{rate != null ? `${rate}×` : "—"}</dd>
            <dt>账户方案</dt><dd>{plan || "—"}</dd>
            <dt>余额观测</dt><dd>{observedAt ? new Date(observedAt).toLocaleString() : "—"}</dd>
            {presentation.warningThreshold != null && <><dt>进度参考值</dt><dd>{amountText(PROGRESS_REFERENCE, recentAccount?.upstreamBalance.unit ?? usage?.unit)}</dd><dt>余额偏低线</dt><dd>{amountText(presentation.warningThreshold, recentAccount?.upstreamBalance.unit ?? usage?.unit)}</dd></>}
          </dl>
          {(usageError || recentAccountError || model.sub2ApiPoolError) && <p className={cn("quota-detail__error", Boolean(balance) && "quota-detail__error--cached")}>{balance ? "刷新失败，当前显示上次成功读取的数据。" : "读取额度失败。"} {usageError ?? recentAccountError ?? model.sub2ApiPoolError}</p>}
        </div>
      </Modal>
    </footer>
  );
}
