import {
  Boxes,
  CheckCircle2,
  Circle,
  CircleAlert,
  CircleDollarSign,
  Edit3,
  KeyRound,
  Plus,
  RefreshCw,
  Router,
  Search,
  Server,
  Settings2,
  ShieldCheck,
  Trash2,
  Unplug,
  X,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api/client";
import { fixtureCodexModelCatalog } from "../api/fixtures";
import type {
  CodexCatalogModel,
  Gateway,
  GatewayProvider,
  GatewayProviderModelAttempt,
  GatewayProviderRecentAccountResponse,
  GatewayProviderTemplate,
  GatewayProviderUsage,
} from "../api/types";
import { useAppModel, type ProviderDraft } from "../state/AppModel";
import { providerTypeLabel } from "../utils/format";
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
  SettingsRow,
  StatusPill,
  Switch,
  cn,
} from "../components/ui";

type GatewayTab = "general" | "providers" | "pool";
type ProviderFilter = "all" | "enabled" | "disabled";

const fixtureProviderTemplates: GatewayProviderTemplate[] = [
  { id: "openai", displayName: "OpenAI", providerType: "open_ai_responses", baseUrl: "https://api.openai.com/v1", models: [] },
  { id: "grok", displayName: "Grok", providerType: "grok_responses", baseUrl: "https://api.x.ai/v1", models: [] },
  { id: "deepseek-responses", displayName: "DeepSeek Responses", providerType: "deepseek_responses", baseUrl: "https://api.deepseek.com/v1", models: ["deepseek-v4-pro"] },
  { id: "anthropic", displayName: "Anthropic", providerType: "anthropic_messages", compatibility: "anthropic", baseUrl: "https://api.anthropic.com/v1", models: [] },
  { id: "glm", displayName: "GLM", providerType: "anthropic_messages", compatibility: "glm_anthropic", baseUrl: "https://open.bigmodel.cn/api/anthropic", modelsUrl: "https://open.bigmodel.cn/api/paas/v4/models", models: [] },
];

const emptyProvider = (): ProviderDraft => ({
  name: "",
  enabled: true,
  providerType: "open_ai_responses",
  compatibility: null,
  baseUrl: "https://api.openai.com/v1",
  modelsUrl: null,
  models: [],
  modelAliases: {},
  promptCacheRetention: null,
  weight: 100,
  timeoutSecs: 600,
  secretSet: false,
  originalName: null,
  apiKey: "",
});

function GeneralSettings({ gateway }: { gateway: Gateway }) {
  const model = useAppModel();
  const [draft, setDraft] = useState(gateway);
  const [modelInput, setModelInput] = useState("");
  const [modelCatalog, setModelCatalog] = useState<CodexCatalogModel[]>(
    model.fixtureMode ? fixtureCodexModelCatalog : [],
  );
  const [catalogLoaded, setCatalogLoaded] = useState(model.fixtureMode);
  useEffect(() => setDraft(gateway), [gateway]);
  useEffect(() => {
    if (model.fixtureMode) return;
    let active = true;
    void api.codexModelCatalog()
      .then((catalog) => {
        if (active) setModelCatalog(catalog);
      })
      .catch(() => {
        // Older daemons do not expose the catalog. The free-form editor stays
        // fully functional in that case, matching the macOS fallback.
        if (active) setModelCatalog([]);
      })
      .finally(() => {
        if (active) setCatalogLoaded(true);
      });
    return () => { active = false; };
  }, [model.fixtureMode]);
  const update = <K extends keyof Gateway>(key: K, value: Gateway[K]) => setDraft((current) => ({ ...current, [key]: value }));
  const catalogIds = useMemo(() => new Set(modelCatalog.map((entry) => entry.id)), [modelCatalog]);
  const customModels = useMemo(
    () => draft.codexVisibleModels.filter((entry) => !catalogIds.has(entry)),
    [catalogIds, draft.codexVisibleModels],
  );
  const orderedVisibleModels = useMemo(() => {
    const selected = new Set(draft.codexVisibleModels);
    return [
      ...modelCatalog.filter((entry) => selected.has(entry.id)).map((entry) => entry.id),
      ...customModels,
    ];
  }, [customModels, draft.codexVisibleModels, modelCatalog]);
  const toggleCatalogModel = (id: string) => {
    setDraft((current) => ({
      ...current,
      codexVisibleModels: current.codexVisibleModels.includes(id)
        ? current.codexVisibleModels.filter((entry) => entry !== id)
        : [...current.codexVisibleModels, id],
    }));
  };
  const addModel = () => {
    const value = modelInput.trim();
    if (!value || draft.codexVisibleModels.includes(value)) return;
    update("codexVisibleModels", [...draft.codexVisibleModels, value]);
    setModelInput("");
  };

  return (
    <div className="gateway-general">
      <SectionHeading title="网关状态" description="统一控制模型路由和请求记录行为" />
      <Card className="settings-card">
        <SettingsRow title="启用 AI 网关" description="Codex 请求会由 MochiPort 根据模型和权重选择 Provider。" control={<Switch label="启用 AI 网关" checked={draft.enabled} onChange={(value) => update("enabled", value)} />} />
        <SettingsRow title="过滤图像生成工具" description="隐藏当前 Provider 不支持的图像生成工具。" control={<Switch label="过滤图像生成工具" checked={draft.filterImageGenerationTool} onChange={(value) => update("filterImageGenerationTool", value)} />} />
        <SettingsRow title="记录请求摘要" description="保存模型、渠道、token、费用和延迟等非敏感信息。" control={<Switch label="记录请求摘要" checked={draft.requestLoggingEnabled} onChange={(value) => update("requestLoggingEnabled", value)} />} />
        <SettingsRow title="记录请求详情" description="额外保存脱敏后的请求与响应正文，便于诊断。" control={<Switch label="记录请求详情" disabled={!draft.requestLoggingEnabled} checked={draft.requestLogDetailsEnabled} onChange={(value) => update("requestLogDetailsEnabled", value)} />} />
      </Card>

      <SectionHeading title="Codex 可见模型" description="只把下面的模型显示在 Codex 的模型选择器中。" />
      <Card className="visible-models-card">
        <div className="model-catalog__heading">
          <strong>Codex 可用模型</strong>
          <span>已选 {orderedVisibleModels.length} 个</span>
        </div>
        {!catalogLoaded ? (
          <div className="model-catalog__empty">正在读取内置模型目录…</div>
        ) : modelCatalog.length > 0 ? (
          <div className="model-catalog" aria-label="Codex 内置模型目录">
            {modelCatalog.map((entry) => {
              const selected = draft.codexVisibleModels.includes(entry.id);
              return (
                <button
                  type="button"
                  key={entry.id}
                  className={cn("model-catalog__item", selected && "model-catalog__item--selected")}
                  aria-label={`可见模型 ${entry.displayName}`}
                  aria-pressed={selected}
                  onClick={() => toggleCatalogModel(entry.id)}
                >
                  {selected ? <CheckCircle2 size={16} /> : <Circle size={16} />}
                  <span><strong>{entry.displayName}</strong><code>{entry.id}</code></span>
                </button>
              );
            })}
          </div>
        ) : (
          <div className="model-catalog__empty">暂时没有目录模型，可在下方添加自定义模型。</div>
        )}
        <div className="custom-models__heading"><strong>自定义模型</strong><span>{customModels.length} 个</span></div>
        <div className="model-chips">
          {customModels.map((entry) => (
            <span className="model-chip" key={entry}>{entry}<button type="button" aria-label={`移除 ${entry}`} onClick={() => update("codexVisibleModels", draft.codexVisibleModels.filter((modelName) => modelName !== entry))}><X size={12} /></button></span>
          ))}
          {customModels.length === 0 && <span className="model-chips__empty">暂无自定义模型，在下方输入名称添加。</span>}
        </div>
        <div className="inline-add">
          <input value={modelInput} placeholder="输入模型 ID，例如 gpt-5.4" onChange={(event) => setModelInput(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter") { event.preventDefault(); addModel(); } }} />
          <Button icon={Plus} size="small" onClick={addModel}>添加</Button>
        </div>
      </Card>

      <div className="page-save-bar">
        <span>{gateway.providers.filter((provider) => provider.enabled).length} 个模型服务参与路由</span>
        <Button variant="primary" loading={model.busy["gateway-settings"]} onClick={() => void model.saveGatewaySettings({
          enabled: draft.enabled,
          filterImageGenerationTool: draft.filterImageGenerationTool,
          requestLoggingEnabled: draft.requestLoggingEnabled,
          requestLogDetailsEnabled: draft.requestLogDetailsEnabled,
          codexVisibleModels: orderedVisibleModels,
        })}>保存网关设置</Button>
      </div>
    </div>
  );
}

interface ProviderEditorProps {
  draft?: ProviderDraft;
  onClose: () => void;
}

interface ModelAliasEntry {
  id: string;
  alias: string;
  target: string;
}

const modelAliasEntry = (alias = "", target = ""): ModelAliasEntry => ({
  id: crypto.randomUUID(),
  alias,
  target,
});

const splitModelIds = (value: string): string[] =>
  value.split(/[\n,]/).map((entry) => entry.trim()).filter(Boolean);

function providerNumericValidationError(draft: ProviderDraft): string | undefined {
  const errors: string[] = [];
  if (!Number.isInteger(draft.weight) || draft.weight < 1 || draft.weight > 10_000) {
    errors.push("路由权重必须是 1 到 10000 之间的整数。");
  }
  if (!Number.isInteger(draft.timeoutSecs) || draft.timeoutSecs < 1 || draft.timeoutSecs > 3_600) {
    errors.push("超时必须是 1 到 3600 秒之间的整数。");
  }
  return errors.length > 0 ? errors.join(" ") : undefined;
}

function mergeModelIds(existing: string, fetched: string[]): string[] {
  const seen = new Set<string>();
  const merged: string[] = [];
  for (const entry of [...splitModelIds(existing), ...fetched.map((value) => value.trim()).filter(Boolean)]) {
    if (seen.has(entry)) continue;
    seen.add(entry);
    merged.push(entry);
  }
  return merged;
}

function inferredModelAlias(model: string): string | undefined {
  const normalized = model.trim().toLocaleLowerCase();
  if (normalized === "claude-opus-4-8") return "opus-4.8";
  if (normalized === "claude-sonnet-4-6") return "sonnet-4.6";
  return undefined;
}

function mergeModelAliases(models: string[], explicit: Record<string, string>): Record<string, string> {
  const merged = { ...explicit };
  for (const modelName of models) {
    const alias = inferredModelAlias(modelName);
    if (!alias || models.includes(alias) || alias in merged) continue;
    merged[alias] = modelName;
  }
  return merged;
}

function providerModelAttemptLine(attempt: GatewayProviderModelAttempt): string {
  const parts = [attempt.url];
  if (attempt.status != null) parts.push(`HTTP ${attempt.status}`);
  else if (attempt.error?.trim()) parts.push(attempt.error.trim());
  else parts.push("无响应");
  if (attempt.preview?.trim()) parts.push(attempt.preview.trim().slice(0, 120));
  return parts.join(" — ");
}

function ProviderEditor({ draft: initialDraft, onClose }: ProviderEditorProps) {
  const model = useAppModel();
  const [draft, setDraft] = useState<ProviderDraft>(initialDraft ?? emptyProvider());
  const [modelsText, setModelsText] = useState((initialDraft?.models ?? []).join("\n"));
  const [templates, setTemplates] = useState<GatewayProviderTemplate[]>(model.fixtureMode ? fixtureProviderTemplates : []);
  const [selectedTemplate, setSelectedTemplate] = useState("");
  const [modelFetchError, setModelFetchError] = useState<string>();
  const [modelFetchNotice, setModelFetchNotice] = useState<{ message: string; positive: boolean }>();
  const [modelFetchAttempts, setModelFetchAttempts] = useState<GatewayProviderModelAttempt[]>([]);
  const [fetchingModels, setFetchingModels] = useState(false);
  const [aliasEntries, setAliasEntries] = useState<ModelAliasEntry[]>(() =>
    Object.entries(initialDraft?.modelAliases ?? {})
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([alias, target]) => modelAliasEntry(alias, target)));
  const modelFetchGeneration = useRef(0);
  const update = <K extends keyof ProviderDraft>(key: K, value: ProviderDraft[K]) => setDraft((current) => ({ ...current, [key]: value }));
  const normalizedAliases = aliasEntries
    .map((entry) => ({ alias: entry.alias.trim(), target: entry.target.trim() }))
    .filter((entry) => entry.alias || entry.target);
  const aliasesIncomplete = normalizedAliases.some((entry) => !entry.alias || !entry.target);
  const aliasesDuplicate = new Set(normalizedAliases.map((entry) => entry.alias)).size !== normalizedAliases.length;
  const numericValidationError = providerNumericValidationError(draft);
  const valid = Boolean(draft.name.trim() && draft.baseUrl.trim() && draft.providerType && !aliasesIncomplete && !aliasesDuplicate && !numericValidationError);
  useEffect(() => {
    if (model.fixtureMode) return;
    let active = true;
    void api.providerTemplates()
      .then((response) => { if (active) setTemplates(response); })
      .catch(() => { if (active) setTemplates(fixtureProviderTemplates); });
    return () => { active = false; };
  }, [model.fixtureMode]);
  useEffect(() => {
    modelFetchGeneration.current += 1;
    setModelFetchError(undefined);
    setModelFetchNotice(undefined);
    setModelFetchAttempts([]);
  }, [draft.apiKey, draft.baseUrl, draft.modelsUrl, draft.providerType]);
  const applyTemplate = (id: string) => {
    setSelectedTemplate(id);
    const template = templates.find((entry) => entry.id === id);
    if (!template) return;
    setDraft((current) => ({
      ...current,
      name: current.originalName ? current.name : template.id,
      providerType: template.providerType,
      compatibility: template.compatibility ?? null,
      baseUrl: template.baseUrl,
      modelsUrl: template.modelsUrl ?? null,
      models: template.models,
    }));
    setModelsText(template.models.join("\n"));
    setModelFetchError(undefined);
    setModelFetchNotice(undefined);
    setModelFetchAttempts([]);
  };
  const fetchModels = async () => {
    if (!draft.baseUrl.trim()) return;
    const generation = modelFetchGeneration.current + 1;
    modelFetchGeneration.current = generation;
    setFetchingModels(true);
    setModelFetchError(undefined);
    setModelFetchNotice(undefined);
    setModelFetchAttempts([]);
    try {
      const response = model.fixtureMode
        ? { ok: true, models: modelsText.trim() ? modelsText.split(/[\n,]/).map((value) => value.trim()).filter(Boolean) : ["gpt-5.4", "gpt-5.4-mini"], attempts: [] }
        : await api.fetchProviderModels({
          providerName: draft.originalName ?? null,
          baseUrl: draft.baseUrl.trim(),
          modelsUrl: draft.modelsUrl?.trim() || null,
          providerType: draft.providerType,
          apiKey: draft.apiKey?.trim() || null,
        });
      if (modelFetchGeneration.current !== generation) return;
      if (!response.ok) {
        setModelFetchNotice({ message: "获取模型失败", positive: false });
        setModelFetchAttempts(response.attempts.slice(0, 4));
        if (response.attempts.length === 0) setModelFetchError("上游未返回模型列表。");
        return;
      }
      if (response.models.length === 0) {
        setModelFetchNotice({ message: "上游返回空列表", positive: false });
        return;
      }
      const existing = new Set(splitModelIds(modelsText));
      const mergedModels = mergeModelIds(modelsText, response.models);
      const addedCount = mergedModels.filter((entry) => !existing.has(entry)).length;
      setModelsText(mergedModels.join("\n"));
      setDraft((current) => ({ ...current, models: mergedModels }));
      setModelFetchNotice({
        message: addedCount > 0
          ? `已获取 ${response.models.length} 个模型，新增 ${addedCount} 个`
          : `已获取 ${response.models.length} 个模型，没有新增条目`,
        positive: true,
      });
    } catch (error) {
      if (modelFetchGeneration.current === generation) {
        setModelFetchError(error instanceof Error ? error.message : "无法获取模型列表");
      }
    } finally {
      if (modelFetchGeneration.current === generation) setFetchingModels(false);
    }
  };
  const save = async () => {
    if (providerNumericValidationError(draft)) return;
    const models = mergeModelIds(modelsText, []);
    const explicitAliases = Object.fromEntries(normalizedAliases.map((entry) => [entry.alias, entry.target]));
    const modelAliases = mergeModelAliases(models, explicitAliases);
    model.dismissError("gateway");
    if (await model.saveProvider({
      ...draft,
      name: draft.name.trim(),
      baseUrl: draft.baseUrl.trim(),
      compatibility: draft.compatibility?.trim() || null,
      promptCacheRetention: draft.promptCacheRetention?.trim() || null,
      models,
      modelAliases,
    })) onClose();
  };
  return (
    <Modal
      open
      title={initialDraft?.originalName ? "编辑模型服务" : "添加模型服务"}
      description="API Key 只写入后台服务，保存后不会再显示在界面中。"
      onClose={onClose}
      size="large"
      footer={<><Button onClick={onClose}>取消</Button><Button variant="primary" disabled={!valid} loading={model.busy["provider-save"]} onClick={() => void save()}>保存模型服务</Button></>}
    >
      <div className="provider-editor-grid">
        {model.errors.gateway && <div className="provider-editor-grid__wide"><InlineError message={model.errors.gateway} onDismiss={() => model.dismissError("gateway")} /></div>}
        <Field label="服务模板" hint="模板来自当前后台服务；应用后仍可修改所有字段。">
          <Select value={selectedTemplate} onChange={(event) => applyTemplate(event.target.value)}>
            <option value="">自定义</option>
            {templates.map((template) => <option key={template.id} value={template.id}>{template.displayName}</option>)}
          </Select>
        </Field>
        <Field label="名称"><input value={draft.name} autoFocus placeholder="例如 OpenAI" onChange={(event) => update("name", event.target.value)} /></Field>
        <Field label="协议">
          <Select value={draft.providerType} onChange={(event) => update("providerType", event.target.value)}>
            <option value="open_ai_responses">OpenAI Responses</option>
            <option value="grok_responses">Grok Responses</option>
            <option value="deepseek_responses">DeepSeek Responses</option>
            <option value="chat_completions">Chat Completions</option>
            <option value="anthropic_messages">Anthropic Messages</option>
          </Select>
        </Field>
        <Field label="兼容配置" hint="可选；用于 GLM Anthropic 等协议变体。"><input value={draft.compatibility ?? ""} placeholder="例如 glm_anthropic" onChange={(event) => update("compatibility", event.target.value || null)} /></Field>
        <Field label="API 地址"><input value={draft.baseUrl} placeholder="https://api.example.com/v1" onChange={(event) => update("baseUrl", event.target.value)} /></Field>
        <Field label="模型列表地址" hint="可选；留空时由后台根据协议推断。"><input value={draft.modelsUrl ?? ""} placeholder="https://api.example.com/v1/models" onChange={(event) => update("modelsUrl", event.target.value || null)} /></Field>
        <Field label="API Key" hint={draft.secretSet ? "已保存密钥；留空会继续保留。" : "凭据会加密保存在 MochiPort 本地配置中。"}><input type="password" disabled={draft.clearApiKey} value={draft.apiKey ?? ""} placeholder={draft.secretSet ? "••••••••（保持不变）" : "输入 API Key"} onChange={(event) => update("apiKey", event.target.value)} /></Field>
        {draft.secretSet && <SettingsRow title="清除已保存的 API Key" description="保存后立即删除这个模型服务的本地凭据。" control={<Switch label="清除已保存的 API Key" checked={draft.clearApiKey ?? false} onChange={(value) => setDraft((current) => ({ ...current, clearApiKey: value, apiKey: value ? "" : current.apiKey }))} />} />}
        <Field label="Prompt Cache Retention" hint="可选，例如 1h 或 24h；原样发送给支持的上游。"><input value={draft.promptCacheRetention ?? ""} placeholder="例如 24h" onChange={(event) => update("promptCacheRetention", event.target.value || null)} /></Field>
        <div className="provider-editor-grid__pair">
          <Field label="路由权重"><input type="number" min={1} max={10000} step={1} value={draft.weight} onChange={(event) => update("weight", Number(event.target.value))} /></Field>
          <Field label="超时（秒）"><input type="number" min={1} max={3600} step={1} value={draft.timeoutSecs} onChange={(event) => update("timeoutSecs", Number(event.target.value))} /></Field>
        </div>
        {numericValidationError && <div className="form-error provider-editor-grid__wide" role="alert">{numericValidationError}</div>}
        <Field label="模型 ID" hint="每行一个，也可以用逗号分隔。"><textarea rows={5} value={modelsText} placeholder={"gpt-5.4\ngpt-5.4-mini"} onChange={(event) => setModelsText(event.target.value)} /></Field>
        <div className="provider-model-fetch">
          <Button icon={RefreshCw} loading={fetchingModels} disabled={!draft.baseUrl.trim()} onClick={() => void fetchModels()}>从服务商获取模型</Button>
          <span>新输入的 API Key 只会经本机管理 API 发送给后台服务。</span>
        </div>
        {modelFetchNotice && (
          <div className={cn("provider-fetch-notice provider-editor-grid__wide", modelFetchNotice.positive ? "provider-fetch-notice--positive" : "provider-fetch-notice--negative")} role="status">
            {modelFetchNotice.positive ? <CheckCircle2 size={15} /> : <CircleAlert size={15} />}
            <span>{modelFetchNotice.message}</span>
          </div>
        )}
        {modelFetchAttempts.length > 0 && (
          <details className="provider-fetch-attempts provider-editor-grid__wide">
            <summary><CircleAlert size={14} /><span>查看获取详情</span><small>{modelFetchAttempts.length} 次尝试</small></summary>
            <div>{modelFetchAttempts.map((attempt, index) => <code key={`${attempt.url}:${index}`}>{providerModelAttemptLine(attempt)}</code>)}</div>
          </details>
        )}
        {modelFetchError && <div className="form-error provider-editor-grid__wide">{modelFetchError}</div>}
        <div className="provider-aliases provider-editor-grid__wide">
          <div className="provider-aliases__heading"><div><strong>模型映射</strong><p>Codex 使用对外别名时，网关会替换为对应的上游模型；手动映射优先于自动 Claude 别名。</p></div><Button size="small" icon={Plus} onClick={() => setAliasEntries((current) => [...current, modelAliasEntry()])}>添加映射</Button></div>
          {aliasEntries.map((entry) => (
            <div className="provider-alias-row" key={entry.id}>
              <input aria-label="对外别名" value={entry.alias} placeholder="对外别名" onChange={(event) => setAliasEntries((current) => current.map((item) => item.id === entry.id ? { ...item, alias: event.target.value } : item))} />
              <span>→</span>
              <input aria-label="上游模型" value={entry.target} placeholder="上游模型 ID" onChange={(event) => setAliasEntries((current) => current.map((item) => item.id === entry.id ? { ...item, target: event.target.value } : item))} />
              <Button variant="ghost" size="small" icon={Trash2} aria-label={`删除映射 ${entry.alias || "空白"}`} onClick={() => setAliasEntries((current) => current.filter((item) => item.id !== entry.id))} />
            </div>
          ))}
          {aliasEntries.length === 0 && <span className="provider-aliases__empty">没有手动映射</span>}
          {(aliasesIncomplete || aliasesDuplicate) && <div className="form-error">{aliasesDuplicate ? "存在重复的对外别名，请先去重。" : "每条映射都需要填写别名和上游模型。"}</div>}
        </div>
        <SettingsRow title="启用此服务" description="停用后保留配置，但不会参与模型路由。" control={<Switch label="启用此模型服务" checked={draft.enabled} onChange={(value) => update("enabled", value)} />} />
      </div>
    </Modal>
  );
}

function Providers({ providers, onConfirmationChange }: { providers: GatewayProvider[]; onConfirmationChange: (open: boolean) => void }) {
  const model = useAppModel();
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<ProviderFilter>("all");
  const [editor, setEditor] = useState<ProviderDraft | null | undefined>(undefined);
  const [deleting, setDeleting] = useState<GatewayProvider>();
  const [usageProvider, setUsageProvider] = useState<GatewayProvider>();
  const [usage, setUsage] = useState<GatewayProviderUsage>();
  const [usageError, setUsageError] = useState<string>();
  const [recentAccountResponse, setRecentAccountResponse] = useState<GatewayProviderRecentAccountResponse>();
  const [recentAccountError, setRecentAccountError] = useState<string>();
  const [usageLoading, setUsageLoading] = useState(false);
  const usageGeneration = useRef(0);
  useEffect(() => {
    onConfirmationChange(Boolean(deleting));
    return () => onConfirmationChange(false);
  }, [deleting, onConfirmationChange]);
  const filtered = useMemo(() => providers.filter((provider) => {
    const matchesQuery = !query.trim() || `${provider.name} ${provider.baseUrl} ${provider.models.join(" ")}`.toLowerCase().includes(query.toLowerCase());
    const matchesFilter = filter === "all" || (filter === "enabled" ? provider.enabled : !provider.enabled);
    return matchesQuery && matchesFilter;
  }), [filter, providers, query]);
  const recentAccount = model.sub2ApiPool?.accounts.find(
    (account) => account.id === recentAccountResponse?.account?.accountId,
  );
  const recentRate = recentAccount?.upstreamBilling.effectiveRateMultiplier
    ?? recentAccount?.upstreamBilling.resolvedRateMultiplier
    ?? usage?.effectiveRateMultiplier
    ?? usage?.resolvedRateMultiplier;
  const inspectUsage = async (provider: GatewayProvider, forceAccountRefresh = false) => {
    const generation = usageGeneration.current + 1;
    usageGeneration.current = generation;
    setUsageProvider(provider);
    setUsage(undefined);
    setUsageError(undefined);
    setRecentAccountResponse(undefined);
    setRecentAccountError(undefined);
    setUsageLoading(true);
    const usageRequest = model.fixtureMode
      ? Promise.resolve({
          ok: true,
          providerName: provider.name,
          usage: {
            source: "sub2api",
            balanceStatus: "available",
            billingStatus: "available",
            remaining: 42.86,
            unlimited: false,
            unit: "USD",
            todayCost: 1.47,
            effectiveRateMultiplier: 0.8,
            observedAt: new Date().toISOString(),
          } satisfies GatewayProviderUsage,
        })
      : api.providerUsage(provider.name);
    const accountRequest = model.sub2ApiAdmin?.configured
      ? Promise.all([
        model.refreshSub2ApiPool(forceAccountRefresh),
        model.fixtureMode
          ? Promise.resolve({
            ok: true,
            providerName: provider.name,
            account: { accountId: model.sub2ApiPool?.accounts[0]?.id ?? 12, accountName: "OpenAI 主账号", createdAt: new Date().toISOString() },
          } satisfies GatewayProviderRecentAccountResponse)
          : api.providerRecentAccount(provider.name),
      ]).then(([, response]) => response)
      : Promise.resolve(undefined);
    const [usageResult, accountResult] = await Promise.allSettled([usageRequest, accountRequest]);
    if (usageGeneration.current !== generation) return;
    if (usageResult.status === "fulfilled") setUsage(usageResult.value.usage);
    else setUsageError(usageResult.reason instanceof Error ? usageResult.reason.message : "无法读取余额和计费信息");
    if (accountResult.status === "fulfilled") setRecentAccountResponse(accountResult.value);
    else setRecentAccountError(accountResult.reason instanceof Error ? accountResult.reason.message : "无法识别最近使用账号");
    setUsageLoading(false);
  };

  return (
    <div>
      <div className="provider-toolbar">
        <SearchField placeholder="搜索模型服务" value={query} onChange={(event) => setQuery(event.target.value)} />
        <SegmentedControl label="筛选模型服务" value={filter} onChange={setFilter} options={[{ value: "all", label: "全部" }, { value: "enabled", label: "已启用" }, { value: "disabled", label: "已停用" }]} />
        <Button variant="primary" icon={Plus} onClick={() => setEditor(null)}>添加服务</Button>
      </div>
      <div className="provider-list">
        {filtered.map((provider) => (
          <Card className="provider-card" key={provider.name}>
            <div className="provider-card__brand"><Server size={19} /></div>
            <div className="provider-card__main">
              <div className="provider-card__title">
                <h3>{provider.name}</h3>
                <StatusPill tone={provider.enabled ? "positive" : "neutral"}>{provider.enabled ? "已启用" : "已停用"}</StatusPill>
              </div>
              <p>{provider.baseUrl}</p>
              <div className="provider-card__meta">
                <span>{providerTypeLabel(provider.providerType)}</span>
                <span>{provider.models.length} 个模型</span>
                <span>权重 {provider.weight}</span>
                <span>{provider.secretSet ? <><KeyRound size={12} /> 密钥已保存</> : "缺少密钥"}</span>
              </div>
              <div className="provider-card__models">
                {provider.models.slice(0, 4).map((entry) => <span key={entry}>{entry}</span>)}
                {provider.models.length > 4 && <span>+{provider.models.length - 4}</span>}
              </div>
            </div>
            <div className="provider-card__actions">
              <Button variant="ghost" size="small" icon={CircleDollarSign} disabled={!provider.secretSet} onClick={() => void inspectUsage(provider)}>用量</Button>
              <Button variant="ghost" size="small" icon={Edit3} onClick={() => setEditor({ ...provider, originalName: provider.name, apiKey: "" })}>编辑</Button>
              <Button variant="ghost" size="small" icon={Trash2} onClick={() => { model.dismissError("gateway"); setDeleting(provider); }}>删除</Button>
            </div>
          </Card>
        ))}
        {filtered.length === 0 && <Card><EmptyState icon={Search} title={providers.length ? "没有匹配的模型服务" : "还没有模型服务"} description={providers.length ? "调整搜索词或筛选条件后重试。" : "添加至少一个 Provider，Codex 才能通过 AI 网关访问模型。"} action={!providers.length && <Button variant="primary" icon={Plus} onClick={() => setEditor(null)}>添加服务</Button>} /></Card>}
      </div>
      {editor !== undefined && <ProviderEditor draft={editor ?? undefined} onClose={() => setEditor(undefined)} />}
      <Modal
        open={Boolean(usageProvider)}
        title={`${usageProvider?.name ?? "模型服务"} · 余额与计费`}
        description="数据由后台服务使用已保存的 API Key 查询；密钥不会返回此窗口。"
        onClose={() => { usageGeneration.current += 1; setUsageProvider(undefined); setRecentAccountResponse(undefined); }}
        size="small"
        footer={<><Button onClick={() => { usageGeneration.current += 1; setUsageProvider(undefined); setRecentAccountResponse(undefined); }}>关闭</Button>{usageProvider && <Button icon={RefreshCw} loading={usageLoading} onClick={() => void inspectUsage(usageProvider, true)}>刷新</Button>}</>}
      >
        {usageLoading && !usage && <div className="provider-usage-loading"><RefreshCw size={18} />正在查询服务商…</div>}
        {usageError && <div className="form-error">{usageError}</div>}
        {recentAccountError && !recentAccount && <p className="provider-usage__context-note">{recentAccountError}</p>}
        {(usage || recentAccount) && (
          <div className="provider-usage">
            {recentAccount && <div className="provider-usage__context"><Router size={16} /><div><span>最近使用账号</span><strong>{recentAccount.name}</strong></div></div>}
            <div className="provider-usage__hero">
              <span>可用余额</span>
              <strong>{recentAccount?.upstreamBalance.unlimited || usage?.unlimited
                ? "不限额"
                : recentAccount?.upstreamBalance.remaining != null
                  ? recentAccount.upstreamBalance.remaining.toLocaleString(undefined, { maximumFractionDigits: 4 })
                  : usage?.remaining != null
                    ? usage.remaining.toLocaleString(undefined, { maximumFractionDigits: 4 })
                    : "未提供"}</strong>
              <small>{recentAccount?.upstreamBalance.unit ?? usage?.unit ?? "服务商未提供单位"}</small>
            </div>
            <div className="provider-usage__grid">
              <div><span>今日费用</span><strong>{usage?.todayActualCost ?? usage?.todayCost ?? "—"}</strong></div>
              <div><span>当前倍率</span><strong>{recentRate != null ? `${recentRate}×` : "—"}</strong></div>
              <div><span>套餐</span><strong>{recentAccount?.upstreamBalance.planName ?? recentAccount?.upstreamBalance.mode ?? usage?.planName ?? usage?.balanceMode ?? "—"}</strong></div>
              <div><span>账号状态</span><strong>{recentAccount?.upstreamBalance.accountStatus ?? usage?.accountStatus ?? ((recentAccount?.upstreamBalance.accountValid === false || usage?.accountValid === false) ? "不可用" : "正常")}</strong></div>
            </div>
            <p className="provider-usage__footnote">{recentAccount
              ? `来源：Sub2API 最近路由账号 · 余额 ${recentAccount.upstreamBalance.state} · 计费 ${recentAccount.upstreamBilling.state}`
              : `来源：${usage?.source ?? "未知"} · 余额 ${usage?.balanceStatus ?? "未知"} · 计费 ${usage?.billingStatus ?? "未知"}`}
              {(recentAccount?.upstreamBalance.observedAt ?? usage?.observedAt) ? ` · ${new Date(recentAccount?.upstreamBalance.observedAt ?? usage?.observedAt ?? "").toLocaleString()}` : ""}</p>
          </div>
        )}
      </Modal>
      <Modal open={Boolean(deleting)} title="删除模型服务？" description="删除后，这个 Provider 将立即停止参与模型路由。" onClose={() => setDeleting(undefined)} size="small" footer={<><Button onClick={() => setDeleting(undefined)}>取消</Button><Button variant="danger" icon={Trash2} loading={deleting ? model.busy[`provider-delete:${deleting.name}`] : false} onClick={async () => { if (deleting && await model.deleteProvider(deleting.name)) setDeleting(undefined); }}>删除</Button></>}>
        {model.errors.gateway && <InlineError message={model.errors.gateway} onDismiss={() => model.dismissError("gateway")} />}
        <div className="confirmation-copy"><Server size={22} /><p>{deleting?.name} 的 API Key 和本地配置也会一并删除。</p></div>
      </Modal>
    </div>
  );
}

function AccountPool({ onConfirmationChange }: { onConfirmationChange: (open: boolean) => void }) {
  const model = useAppModel();
  const [editing, setEditing] = useState(!model.sub2ApiAdmin?.configured);
  const [baseUrl, setBaseUrl] = useState(model.sub2ApiAdmin?.baseUrl ?? "");
  const [adminKey, setAdminKey] = useState("");
  const [confirmDisconnect, setConfirmDisconnect] = useState(false);
  useEffect(() => {
    onConfirmationChange(confirmDisconnect);
    return () => onConfirmationChange(false);
  }, [confirmDisconnect, onConfirmationChange]);
  useEffect(() => {
    setBaseUrl(model.sub2ApiAdmin?.baseUrl ?? "");
    if (model.sub2ApiAdmin?.configured) setEditing(false);
  }, [model.sub2ApiAdmin]);
  const accounts = model.sub2ApiPool?.accounts ?? [];
  return (
    <div className="account-pool-page">
      <SectionHeading title="管理连接" description="连接 Sub2API 管理接口后，MochiPort 可读取账号余额、倍率和调度状态。" />
      {editing ? (
        <Card className="pool-connect-card">
          <div className="pool-connect-card__icon"><Router size={22} /></div>
          <div className="pool-connect-card__form">
            <Field label="Sub2API 地址"><input autoFocus value={baseUrl} placeholder="https://sub2api.example.com" onChange={(event) => setBaseUrl(event.target.value)} /></Field>
            <Field label="管理 API Key" hint={model.sub2ApiAdmin?.secretSet ? "已保存密钥；留空会继续保留。" : "只写入本地后台服务，不会显示在连接状态中。"}><input type="password" value={adminKey} placeholder={model.sub2ApiAdmin?.secretSet ? "••••••••（保持不变）" : "输入管理 API Key"} onChange={(event) => setAdminKey(event.target.value)} /></Field>
            <div className="pool-connect-card__actions">
              {model.sub2ApiAdmin?.configured && <Button onClick={() => setEditing(false)}>取消</Button>}
              <Button variant="primary" disabled={!baseUrl.trim() || (!model.sub2ApiAdmin?.secretSet && !adminKey.trim())} loading={model.busy["sub2api-save"]} onClick={() => void model.saveSub2Api(baseUrl.trim(), adminKey).then((ok) => ok && setEditing(false))}>保存并连接</Button>
            </div>
          </div>
        </Card>
      ) : (
        <Card className="pool-connection-card">
          <div className="pool-connection-card__icon"><ShieldCheck size={20} /></div>
          <div><strong>{model.sub2ApiAdmin?.baseUrl}</strong><p>连接已验证 · 管理密钥已安全保存</p></div>
          <StatusPill tone="positive">已连接</StatusPill>
          <Button variant="ghost" size="small" icon={Settings2} onClick={() => setEditing(true)}>编辑</Button>
          <Button variant="ghost" size="small" icon={Unplug} onClick={() => { model.dismissError("gateway"); setConfirmDisconnect(true); }}>断开</Button>
        </Card>
      )}

      <SectionHeading title="账号" description={accounts.length ? `${accounts.length} 个账号，${accounts.filter((account) => account.schedulable).length} 个可调度` : "尚未读取到账号"} trailing={model.sub2ApiAdmin?.configured && <Button variant="ghost" icon={RefreshCw} size="small" loading={model.sub2ApiPoolLoading} onClick={() => void model.refreshSub2ApiPool(true)}>刷新</Button>} />
      {model.sub2ApiPoolError && <InlineError message={model.sub2ApiPoolError} onRetry={() => void model.refreshSub2ApiPool(true)} />}
      {Boolean(model.sub2ApiPool?.warnings?.length) && <div className="pool-warnings" role="status"><CircleAlert size={16} /><div>{model.sub2ApiPool?.warnings?.map((warning) => <p key={warning}>{warning}</p>)}</div></div>}
      <Card className="data-table-card">
        {accounts.length ? (
          <div className="data-table">
            <div className="data-table__header pool-table-grid"><span>账号</span><span>状态</span><span>倍率</span><span>余额</span></div>
            {accounts.map((account) => (
              <div className="data-table__row pool-table-grid" key={account.id}>
                <div className="table-primary"><span className="account-platform-icon"><CircleDollarSign size={16} /></span><div><strong>{account.name}</strong><small>{account.platform} · {account.accountType}</small></div></div>
                <StatusPill tone={account.schedulable ? "positive" : "warning"}>{account.schedulable ? "可调度" : account.status}</StatusPill>
                <span className="mono-value">{account.upstreamBilling.effectiveRateMultiplier ?? account.localRateMultiplier ?? "—"}×</span>
                <div className="balance-cell"><strong>{account.upstreamBalance.unlimited ? "不限额" : account.upstreamBalance.remaining != null ? account.upstreamBalance.remaining.toFixed(2) : "—"}</strong><small>{account.upstreamBalance.unit ?? ""}</small></div>
              </div>
            ))}
          </div>
        ) : (
          <EmptyState icon={Router} title={model.sub2ApiAdmin?.configured ? "账号池暂时为空" : "连接后查看账号池"} description={model.sub2ApiAdmin?.configured ? "确认 Sub2API 中已有可用账号，然后刷新。" : "MochiPort 不会修改账号，只读取用于路由的状态。"} />
        )}
      </Card>

      <Modal open={confirmDisconnect} title="断开 Sub2API 账号池？" description="只会删除本机保存的管理连接，不会修改 Sub2API 中的账号。" onClose={() => setConfirmDisconnect(false)} size="small" footer={<><Button onClick={() => setConfirmDisconnect(false)}>取消</Button><Button variant="danger" loading={model.busy["sub2api-disconnect"]} onClick={() => void model.disconnectSub2Api().then((ok) => ok && setConfirmDisconnect(false))}>断开连接</Button></>}>
        {model.errors.gateway && <InlineError message={model.errors.gateway} onDismiss={() => model.dismissError("gateway")} />}
        <div className="confirmation-copy"><Unplug size={22} /><p>断开后，概览页将不再显示账号余额和倍率。</p></div>
      </Modal>
    </div>
  );
}

export function GatewayPage() {
  const model = useAppModel();
  const [tab, setTab] = useState<GatewayTab>("general");
  const [confirmationOwnsGatewayError, setConfirmationOwnsGatewayError] = useState(false);
  const gateway = model.gateway;
  return (
    <div className="page">
      <div className="page-tabs-wrap">
        <SegmentedControl label="AI 网关页面" value={tab} onChange={setTab} options={[{ value: "general", label: "常规" }, { value: "providers", label: "模型服务" }, { value: "pool", label: "账号池" }]} />
        {gateway && <StatusPill tone={gateway.enabled ? "positive" : "neutral"}>{gateway.enabled ? "网关已开启" : "网关已关闭"}</StatusPill>}
      </div>
      {model.errors.gateway && !confirmationOwnsGatewayError && <InlineError message={model.errors.gateway} onRetry={() => void model.loadSection("gateway", true)} onDismiss={() => model.dismissError("gateway")} />}
      {!gateway ? (
        <Card><EmptyState icon={Boxes} title={model.loading.gateway ? "正在读取 AI 网关…" : "无法读取 AI 网关"} description={model.loading.gateway ? "正在从本地服务加载配置。" : "确认后台服务在线后重试。"} /></Card>
      ) : tab === "general" ? <GeneralSettings gateway={gateway} /> : tab === "providers" ? <Providers providers={gateway.providers} onConfirmationChange={setConfirmationOwnsGatewayError} /> : <AccountPool onConfirmationChange={setConfirmationOwnsGatewayError} />}
    </div>
  );
}
