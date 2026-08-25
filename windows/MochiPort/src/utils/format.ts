export function relativeTime(timestamp?: number | null): string {
  if (!timestamp) return "暂无记录";
  const seconds = Math.max(0, Math.round((Date.now() - timestamp) / 1000));
  if (seconds < 10) return "刚刚";
  if (seconds < 60) return `${seconds} 秒前`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} 分钟前`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours} 小时前`;
  return `${Math.round(hours / 24)} 天前`;
}

export function compactNumber(value?: number | null): string {
  if (value === undefined || value === null) return "—";
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(value >= 10_000_000 ? 0 : 1)}m`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(value >= 10_000 ? 0 : 1)}k`;
  return new Intl.NumberFormat("zh-CN").format(value);
}

/** AI Token Monitor v0.20.5 usage-card formatting. */
export function formatUsageTokens(tokens: number): string {
  if (tokens >= 1_000_000_000) return `${(tokens / 1_000_000_000).toFixed(1)}B`;
  if (tokens >= 1_000_000) return `${(tokens / 1_000_000).toFixed(1)}M`;
  if (tokens >= 1_000) return `${(tokens / 1_000).toFixed(1)}K`;
  return new Intl.NumberFormat("zh-CN").format(tokens);
}

/** AI Token Monitor v0.20.5 cost precision tiers. */
export function formatUsageCost(cost: number): string {
  if (cost >= 100) return `$${cost.toFixed(0)}`;
  if (cost >= 1) return `$${cost.toFixed(2)}`;
  return `$${cost.toFixed(4)}`;
}

export function formatBytes(value?: number | null): string {
  if (value === undefined || value === null) return "—";
  if (value >= 1_048_576) return `${(value / 1_048_576).toFixed(2)} MB`;
  if (value >= 1_024) return `${(value / 1_024).toFixed(1)} KB`;
  return `${new Intl.NumberFormat("zh-CN").format(value)} B`;
}

export function formatDateTime(timestamp?: number | null): string {
  if (!timestamp) return "—";
  const normalized = timestamp < 10_000_000_000 ? timestamp * 1000 : timestamp;
  return new Intl.DateTimeFormat("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  }).format(new Date(normalized));
}

export function platformLabel(platform: string): string {
  const labels: Record<string, string> = {
    telegram: "Telegram",
    feishu: "飞书",
    wechat: "微信",
    wecom: "企业微信",
  };
  return labels[platform.toLowerCase()] ?? platform;
}

export function providerTypeLabel(type: string): string {
  const labels: Record<string, string> = {
    open_ai_responses: "OpenAI Responses",
    grok_responses: "Grok Responses",
    deepseek_responses: "DeepSeek Responses",
    chat_completions: "Chat Completions",
    anthropic_messages: "Anthropic Messages",
  };
  return labels[type] ?? type;
}
