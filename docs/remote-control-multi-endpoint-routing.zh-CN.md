# Remote-Control 多端优先路由设计

更新时间：2026-09-04

状态：已实现。本文记录 `mochiport` 支持 Codex App、VS Code 插件、Codex CLI/TUI 同时接入 remote-control backend 的连接表与优先级路由设计。目标不是广播，也不是每个 IM 会话手动选择执行端，而是让多个 Codex app-server 可以共存，并把 IM 请求自动发送给最高优先级的可用执行端。

## 1. 问题背景

Codex App、VS Code 插件、Codex CLI/TUI 都可能启动各自的 Codex app-server，并连接 `mochiport` 暴露的 remote-control WebSocket：

- `GET /backend-api/wham/remote/control/server`
- `GET /backend-api/remote/control/server`

日志验证到的 `initialize` 返回来源包括：

- `Codex Desktop/0.137.0-alpha.4 ...`：Codex App
- `codex_vscode/0.137.0-alpha.4 ...`：VS Code 插件
- `mochiport/0.137.0 ... WindowsTerminal ...`：CLI/TUI

WebSocket 握手 header 的 `user-agent` 通常为空，`x-codex-name` 是机器名，不能区分来源。可靠来源在 `initialize` 响应的 `result.userAgent` 中。

早期实现是单连接模型：

- `remote.outbound_tx` 只有一个
- `remote.connection_epoch` 只有一个
- `remote.clients` 是全局一份
- `remote.pending` 随 client state 全局混用

多个 app-server 同时连接时，新连接会覆盖旧连接，旧连接 writer 关闭后又重连，形成连接风暴：

- `ws_open` 快速增长
- `remote_control_disconnected reason=websocket closed` 快速增长
- Codex 侧可能报 `Incoming line queue overflow`

当前实现改为连接表模型（见第 4 节）：每个 WebSocket 连接拥有独立的连接级状态，互不覆盖；v0.5.6 起连接状态完全按连接隔离，不再维护全局汇总的 legacy 字段。

## 2. 目标

目标（均已达成）：

1. Codex App、VS Code、CLI/TUI 可以同时连接到 `mochiport`。
2. 连接之间不互相覆盖，不因为某个连接断开把 remote-control 整体标为断开。
3. IM 请求只发送给一个执行端，不做广播。
4. 自动选择最高优先级且已初始化的执行端：
   - Codex App
   - VS Code
   - CLI/TUI
   - Unknown
5. GUI/API 能展示当前执行端和已连接端。

非目标：

1. 不把一条 IM 消息同时发送给多个 app-server。
2. 不支持每个 IM 会话手动选择不同执行端。
3. 不实现跨 app-server 事件合并或广播去重。
4. 不复制官方完整多 controller/client tracker 体系。

## 3. 路由原则

`mochiport` 只负责选择一个 app-server 作为 IM 执行目标：

```text
IM message -> selected app-server -> Codex thread/turn
```

Codex App 和 VS Code 对同一个 thread 的本地同步由 Codex 官方本地机制负责。`mochiport` 不主动把同一条 IM 请求复制给多个 app-server。

默认优先级：

```text
Codex App > VS Code > CLI/TUI > Unknown
```

当高优先级连接可用时，新的 IM 请求使用高优先级连接。已有请求的 response 仍回到发出该请求的连接 state，不能被新连接抢走。

## 4. 数据结构

连接级状态（`src/app_state.rs` 中的 `RemoteControlServerConnection`）：

```rust
RemoteControlServerConnection {
    connection_id: String,
    connection_epoch: u64,
    default_client_key: String,
    connected: bool,
    source_kind: RemoteControlSourceKind,
    user_agent: Option<String>,
    server_id: Option<String>,
    environment_id: Option<String>,
    server_name: Option<String>,
    installation_id: Option<String>,
    account_id: Option<String>,
    subscribe_cursor: Option<String>,
    outbound_tx: Option<UnboundedSender<OutboundWsMessage>>,
    connected_at_ms: Option<u128>,
    last_ws_inbound_at_ms: Option<u128>,
    last_ws_ping_at_ms: Option<u128>,
    last_ws_pong_at_ms: Option<u128>,
    last_error: Option<String>,
    clients: HashMap<String, RemoteControlClientState>,
    server_ack_cursors: HashMap<String, (u64, Option<usize>)>,
    stream_diagnostics: HashMap<String, RemoteControlStreamDiagnostics>,
}
```

`RemoteControlInner` 只保留连接表与授权/事件等全局簿记，不再有 `active_connection_id`、全局 `outbound_tx`、全局 `connected` 等 legacy 汇总字段：

```rust
connections: HashMap<String, RemoteControlServerConnection>
next_connection_epoch: u64
```

"当前执行端"不落盘为字段，而是按需由 `select_active_connection_id_locked` 在每次发送/查询时从 `connections` 里现算（见第 6 节），避免连接增减时出现汇总字段与连接表不一致的问题。

## 5. 来源识别

连接刚建立时来源是 `unknown`。发送 `initialize` 后，从响应里读取：

```json
{
  "result": {
    "userAgent": "Codex Desktop/0.137.0-alpha.4 ..."
  }
}
```

分类规则：

```text
starts_with("Codex Desktop/") -> codex_app
starts_with("codex_vscode/") -> vscode
contains("WindowsTerminal") 或 CLI 形态 -> cli
其它 -> unknown
```

更新来源后，下一次选择执行端时会按新优先级现算 active connection。

## 6. 请求发送

选择函数（`src/remote_control_backend/client_state.rs`）：

```rust
select_active_connection_id_locked(remote) -> Option<String>
```

实现为一次 `max_by_key`，键为四元组，依次比较：

1. `connected == true` 且 `outbound_tx.is_some()`（过滤条件）
2. default client 已初始化（`connection_initialized`）
3. 来源优先级：`codex_app(40) > vscode(30) > cli(20) > unknown(10)`
4. 最近活跃时间（`last_ws_inbound_at_ms`，缺省回退 `connected_at_ms`）；再相同则取更大的 `connection_epoch`

请求发送流程：

```text
request_for_client()
  -> select active connection
  -> ensure client initialized on that connection
  -> create pending under that connection.clients[client_key]
  -> send envelopes through that connection.outbound_tx
```

响应处理流程：

```text
server envelope in
  -> locate connection by connection_id + epoch
  -> locate client by client_id + stream_id inside that connection
  -> resolve pending inside that connection only
```

## 7. 断开处理

某个 WebSocket 关闭时：

1. 只标记该 `connection_id` 为 disconnected。
2. 清理该连接的 `outbound_tx`。
3. 保留其它连接。
4. 后续请求重新按优先级选择 active connection。
5. 如果没有任何可用连接，汇总状态才显示 disconnected。

早期全局状态会随任一连接关闭被整体置断（等效于执行 `remote.connected = false; remote.outbound_tx = None;`），这正是连接风暴的放大器；连接表模型下不存在这些全局字段，单连接断开不再影响其它连接。

## 8. 状态 API/GUI

`/api/remote-control/status` 顶层仍输出汇总字段（`connected`、`clientId`、`streamId`、`serverId` 等），但它们在响应时从 active connection 现算派生，不再是独立保存的状态；同时输出连接列表：

```json
{
  "activeConnectionId": "...",
  "activeSourceKind": "codex_app",
  "activeUserAgent": "Codex Desktop/...",
  "connections": [
    {
      "id": "...",
      "connectionEpoch": 3,
      "sourceKind": "codex_app",
      "userAgent": "Codex Desktop/...",
      "connected": true,
      "initialized": true,
      "healthy": true,
      "lastError": null
    }
  ]
}
```

GUI 使用新增字段显示：

- 当前执行端：Codex App / VS Code / CLI
- 已连接端列表
- 连接异常时显示最近错误和来源

## 9. 实施状态

连接表、来源识别、优先级选择、按连接隔离的状态 API 与 GUI 展示均已落地。v0.5.6 移除了旧实现遗留的全局汇总字段与同步逻辑（`sync_legacy_from_active_connection_locked`），状态展示统一从 `connections` 派生。

后续可选项（未实施）：

1. 根据真实测试结果决定是否支持手动 pin 执行端。
2. 根据需要再扩展 IM 会话级 route 绑定。

## 10. 验证场景

1. 只开 Codex App：active source 为 `codex_app`。
2. 只开 VS Code：active source 为 `vscode`。
3. 同时开 Codex App + VS Code：两者都 connected，active source 为 `codex_app`。
4. 同时开 VS Code + CLI：active source 为 `vscode`。
5. Codex App 断开后，active 自动降级到 VS Code。
6. 多端同时连接时日志不再出现快速 `ws_open/disconnected` 风暴。
7. IM 发送消息只触发一个 `turn/start`。
