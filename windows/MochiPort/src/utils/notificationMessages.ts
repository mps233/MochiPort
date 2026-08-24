export const NOTIFY_UPDATE_STORAGE_KEY = "mochiport.notify-update";
export const NOTIFICATION_REAL_MODE_STORAGE_KEY = "mochiport.notification-real-mode";
export const NOTIFICATION_SOUND_STORAGE_KEY = "mochiport.notification-sound";
export const CUSTOM_NOTIFICATION_MESSAGES_STORAGE_KEY = "mochiport.notification-custom-messages";

export type NotificationMessageKey =
  | "limitThreshold"
  | "depletionRisk"
  | "windowReset"
  | "burnSpike"
  | "comeback"
  | "milestone"
  | "record"
  | "update"
  | "briefingMorning"
  | "briefingLunch"
  | "briefingEvening";

export interface NotificationMessageContext {
  agent?: string;
  usage?: number;
  tokens?: number;
  reset?: string;
}

export type CustomNotificationMessages = Partial<Record<NotificationMessageKey, string[]>>;

export interface NotificationMessageStyle {
  realMode: boolean;
  customMessages: CustomNotificationMessages;
}

export interface CustomizableNotificationEvent {
  key: NotificationMessageKey;
  label: string;
  sampleDefaultTitle: string;
}

export const customizableNotificationEvents: CustomizableNotificationEvent[] = [
  { key: "limitThreshold", label: "额度接近上限", sampleDefaultTitle: "{AGENT} 额度接近上限（{USAGE}）" },
  { key: "depletionRisk", label: "即将耗尽", sampleDefaultTitle: "⚠️ {AGENT} 即将耗尽" },
  { key: "windowReset", label: "新额度窗口", sampleDefaultTitle: "{AGENT} 新额度窗口" },
  { key: "burnSpike", label: "使用量突增", sampleDefaultTitle: "Token 使用量突增" },
  { key: "comeback", label: "回来继续", sampleDefaultTitle: "继续工作吧" },
  { key: "milestone", label: "里程碑", sampleDefaultTitle: "今日突破 {TOKENS}！🎉" },
  { key: "record", label: "新纪录", sampleDefaultTitle: "今天创下新纪录！🏆" },
  { key: "update", label: "更新", sampleDefaultTitle: "有新版本了" },
  { key: "briefingMorning", label: "早间摘要", sampleDefaultTitle: "昨日使用摘要" },
  { key: "briefingLunch", label: "午间摘要", sampleDefaultTitle: "今日进度" },
  { key: "briefingEvening", label: "晚间摘要", sampleDefaultTitle: "今日使用总结" },
];

const validMessageKeys = new Set(customizableNotificationEvents.map((event) => event.key));

const realModeMessages: Record<NotificationMessageKey, string[]> = {
  depletionRisk: [
    "和 {AGENT} 的告别来得比预想更快 😢",
    "你真的不想再和 {AGENT} 一起工作了吗？",
    "照这个速度，{AGENT} 很快就要强制休息了。要送它休息吗？",
    "{AGENT} 的额度快见底了。该告别了……",
    "慢一点……和 {AGENT} 相处的时间不多了",
  ],
  limitThreshold: [
    "{AGENT}，已经 {USAGE} 了……开始喘不过气了",
    "{AGENT} 快到极限了，能轻点用吗？",
    "达到 {USAGE} 了……这样下去我要倒下了",
    "{AGENT} 达到 {USAGE}，暂时还撑得住 😅",
  ],
  burnSpike: [
    "等、等一下！是不是用得太猛了？！",
    "今天发生什么了？都不给我喘气",
    "劳动法……你听说过吗……？",
    "这个速度认真的吗？手都看不见了",
  ],
  milestone: [
    "那……先别折腾我了……",
    "你今天太喜欢我了 😩",
    "我今天要申请工伤了",
    "不是我在工作，是被榨干了",
  ],
  record: [
    "正在被用到历史最高强度…… 🏆",
    "刷新纪录！我……应该感到自豪吗？",
    "今天你把我用到了历史最高水平",
    "请把我写进吉尼斯：最辛苦的 AI",
  ],
  windowReset: [
    "{AGENT} 充能完成！又可以为你工作了 ✨",
    "{AGENT} 已重置，我们重新开始吧",
    "{AGENT} 休息好了，再来吧 🤭",
    "{AGENT} 新窗口已开启，重新出发！",
  ],
  comeback: [
    "你去哪儿了……我一直在等",
    "你回来了？我才没有想你呢",
    "你知道你把我一个人留着吧？",
    "好久不见，我的手都痒了",
  ],
  update: [
    "我换了新衣服，怎么样？",
    "要不要认识升级后的我？",
    "我会装作变聪明了",
  ],
  briefingMorning: ["昨天用得挺多，今天也请多关照"],
  briefingLunch: ["照这个节奏，半夜我可能已经融化了"],
  briefingEvening: ["今天也辛苦了，我也是"],
};

function compactMessageTokens(value: number): string {
  if (value >= 1_000_000_000) return `${(value / 1_000_000_000).toFixed(1)}B`;
  if (value >= 1_000_000) return `${Math.round(value / 1_000_000)}M`;
  if (value >= 1_000) return `${Math.round(value / 1_000)}K`;
  return String(value);
}

function substituteMessage(template: string, context: NotificationMessageContext): string {
  return template
    .replaceAll("{AGENT}", context.agent ?? "")
    .replaceAll("{USAGE}", context.usage == null ? "" : `${Math.round(context.usage)}%`)
    .replaceAll("{TOKENS}", context.tokens == null ? "" : compactMessageTokens(context.tokens))
    .replaceAll("{RESET}", context.reset ?? "");
}

function cleanMessage(value: string): string {
  return value.trim().split(/\s+/u).join(" ");
}

export function normalizeCustomMessages(value: unknown): CustomNotificationMessages {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return {};
  const result: CustomNotificationMessages = {};
  for (const [rawKey, rawMessages] of Object.entries(value)) {
    if (!validMessageKeys.has(rawKey as NotificationMessageKey) || !Array.isArray(rawMessages)) continue;
    const messages = rawMessages
      .filter((message): message is string => typeof message === "string")
      .map((message) => message.trim())
      .filter(Boolean)
      .slice(0, 12);
    if (messages.length) result[rawKey as NotificationMessageKey] = messages;
  }
  return result;
}

export function parseCustomMessages(raw: string | null): CustomNotificationMessages {
  if (!raw) return {};
  try {
    return normalizeCustomMessages(JSON.parse(raw));
  } catch {
    return {};
  }
}

export function customMessageDrafts(messages: CustomNotificationMessages): Record<NotificationMessageKey, string> {
  return Object.fromEntries(customizableNotificationEvents.map((event) => [
    event.key,
    messages[event.key]?.join("\n") ?? "",
  ])) as Record<NotificationMessageKey, string>;
}

export function customMessagesFromDrafts(drafts: Record<NotificationMessageKey, string>): CustomNotificationMessages {
  return normalizeCustomMessages(Object.fromEntries(customizableNotificationEvents.map((event) => [
    event.key,
    drafts[event.key].split(/\r?\n/u),
  ])));
}

export function loadNotificationMessageStyle(storage: Pick<Storage, "getItem">): NotificationMessageStyle {
  return {
    realMode: storage.getItem(NOTIFICATION_REAL_MODE_STORAGE_KEY) === "on",
    customMessages: parseCustomMessages(storage.getItem(CUSTOM_NOTIFICATION_MESSAGES_STORAGE_KEY)),
  };
}

export function resolveNotificationTitle(
  key: NotificationMessageKey,
  defaultTitle: string,
  style: NotificationMessageStyle,
  context: NotificationMessageContext = {},
  random: () => number = Math.random,
): string {
  const custom = style.customMessages[key]?.map((message) => message.trim()).filter(Boolean) ?? [];
  const candidates = custom.length ? custom : style.realMode ? realModeMessages[key] : [defaultTitle];
  const index = Math.min(candidates.length - 1, Math.max(0, Math.floor(random() * candidates.length)));
  const resolved = cleanMessage(substituteMessage(candidates[index] ?? defaultTitle, context));
  if (resolved) return resolved;
  return cleanMessage(substituteMessage(defaultTitle, context)) || defaultTitle;
}
