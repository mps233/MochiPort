import {
  Bot,
  Boxes,
  Clock3,
  FileClock,
  Gauge,
  MessageCircleMore,
  Settings,
  type LucideIcon,
} from "lucide-react";
import type { AppSection } from "../api/types";
import { useAppModel } from "../state/AppModel";
import { cn, StatusPill } from "./ui";

interface NavItem {
  id: AppSection;
  label: string;
  icon: LucideIcon;
}

const main: NavItem[] = [{ id: "overview", label: "概览", icon: Gauge }];
const configuration: NavItem[] = [
  { id: "codex", label: "Codex 接入", icon: Bot },
  { id: "gateway", label: "AI 网关", icon: Boxes },
  { id: "messaging", label: "消息渠道", icon: MessageCircleMore },
  { id: "sessions", label: "会话", icon: Clock3 },
];
const diagnostics: NavItem[] = [{ id: "requestLogs", label: "请求日志", icon: FileClock }];

export function Sidebar() {
  const model = useAppModel();
  const row = ({ id, label, icon: Icon }: NavItem) => (
    <button
      type="button"
      key={id}
      className={cn("nav-item", model.selection === id && "nav-item--selected")}
      aria-current={model.selection === id ? "page" : undefined}
      onClick={() => model.setSelection(id)}
    >
      <Icon size={17} strokeWidth={1.8} aria-hidden />
      <span>{label}</span>
    </button>
  );
  const statusTone = model.status === "available" ? "positive" : model.status === "checking" ? "neutral" : model.status === "bridgeAvailable" ? "warning" : "negative";

  return (
    <aside className="sidebar">
      <nav aria-label="主导航">
        <div className="nav-group">{main.map(row)}</div>
        <div className="nav-group">
          <div className="nav-group__label">配置</div>
          {configuration.map(row)}
        </div>
        <div className="nav-group">
          <div className="nav-group__label">诊断</div>
          {diagnostics.map(row)}
        </div>
      </nav>
      <div className="sidebar__bottom">
        <button
          type="button"
          className={cn("nav-item", model.selection === "settings" && "nav-item--selected")}
          onClick={() => model.setSelection("settings")}
        >
          <Settings size={17} strokeWidth={1.8} />
          <span>设置</span>
        </button>
        <div className="sidebar-identity">
          <span className="brand-mark" aria-hidden><span /></span>
          <div className="sidebar-identity__copy">
            <strong>MochiPort</strong>
            <StatusPill tone={statusTone} dot>{model.status === "available" ? "服务在线" : model.status === "checking" ? "连接中" : model.status === "bridgeAvailable" ? "需要更新" : "服务离线"}</StatusPill>
          </div>
        </div>
      </div>
    </aside>
  );
}
