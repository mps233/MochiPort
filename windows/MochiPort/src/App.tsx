import {
  Bot,
  Boxes,
  Clock3,
  FileClock,
  Gauge,
  MessageCircleMore,
  Play,
  RefreshCw,
  Settings,
} from "lucide-react";
import { Sidebar } from "./components/Sidebar";
import { GatewayQuotaDock } from "./components/GatewayQuotaDock";
import { TitleBar } from "./components/TitleBar";
import { Button, StatusPill, Toast } from "./components/ui";
import { CodexPage } from "./pages/CodexPage";
import { GatewayPage } from "./pages/GatewayPage";
import { MessagingPage } from "./pages/MessagingPage";
import { OverviewPage } from "./pages/OverviewPage";
import { RequestLogsPage } from "./pages/RequestLogsPage";
import { SessionsPage } from "./pages/SessionsPage";
import { SettingsPage } from "./pages/SettingsPage";
import { useAppModel } from "./state/AppModel";
import { UpdateProvider } from "./state/useUpdateNotifications";

const metadata = {
  overview: { title: "概览", icon: Gauge, subtitle: "连接状态与开始使用" },
  codex: { title: "Codex 接入", icon: Bot, subtitle: "连接和管理本机 Codex" },
  gateway: { title: "AI 网关", icon: Boxes, subtitle: "模型服务、路由与账号池" },
  messaging: { title: "消息渠道", icon: MessageCircleMore, subtitle: "连接手机里的消息账号" },
  sessions: { title: "会话", icon: Clock3, subtitle: "查看和移动 Codex 会话" },
  requestLogs: { title: "请求日志", icon: FileClock, subtitle: "检查模型请求和响应" },
  settings: { title: "设置", icon: Settings, subtitle: "MochiPort 偏好与诊断" },
} as const;

function CurrentPage() {
  const { selection } = useAppModel();
  if (selection === "overview") return <OverviewPage />;
  if (selection === "codex") return <CodexPage />;
  if (selection === "gateway") return <GatewayPage />;
  if (selection === "messaging") return <MessagingPage />;
  if (selection === "sessions") return <SessionsPage />;
  if (selection === "requestLogs") return <RequestLogsPage />;
  return <SettingsPage />;
}

function AppContent() {
  const model = useAppModel();
  const page = metadata[model.selection];
  const PageIcon = page.icon;
  const statusTone = model.status === "available" ? "positive" : model.status === "checking" ? "neutral" : model.status === "bridgeAvailable" ? "warning" : "negative";
  return (
    <div className="app-shell">
      <TitleBar />
      <div className="app-shell__body">
        <Sidebar />
        <main className="main-panel">
          <header className="page-header">
            <div className="page-header__title">
              <div className="page-header__icon"><PageIcon size={19} /></div>
              <div><h1>{page.title}</h1><p>{page.subtitle}</p></div>
            </div>
            <div className="page-header__actions">
              {model.fixtureMode && <StatusPill tone="accent">预览数据</StatusPill>}
              {model.selection === "overview" && <StatusPill tone={statusTone}>{model.status === "available" ? "运行正常" : model.status === "checking" ? "检查中" : model.status === "bridgeAvailable" ? "需要更新" : "服务不可用"}</StatusPill>}
              {model.status === "unavailable" && (
                <Button
                  variant="primary"
                  size="small"
                  icon={Play}
                  loading={model.daemonTransitionInProgress}
                  onClick={() => void model.startDaemon()}
                >
                  启动本地服务
                </Button>
              )}
              <Button variant="ghost" size="small" icon={RefreshCw} onClick={() => void (model.selection === "overview" ? model.refresh() : model.loadSection(model.selection, true))}>刷新</Button>
            </div>
          </header>
          <div className="page-viewport">
            <CurrentPage />
          </div>
          <GatewayQuotaDock />
          {model.feedback && <Toast message={model.feedback} onClose={model.clearFeedback} />}
        </main>
      </div>
    </div>
  );
}

export default function App() {
  return (
    <UpdateProvider>
      <AppContent />
    </UpdateProvider>
  );
}
