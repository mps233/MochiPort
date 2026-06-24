# Remote-Control 多端优先路由设计

更新时间：2026-06-07

本文记录 `codexhub` 支持 Codex App、VS Code 插件、Codex CLI/TUI 同时接入 remote-control backend 的设计。当前目标不是广播，也不是每个 IM 会话手动选择执行端，而是让多个 Codex app-server 可以共存，并把 IM 请求自动发送给最高优先级的可用执行端。

## 1. 问题背景

Codex App、VS Code 插件、Codex CLI/TUI 都可能启动各自的 Codex app-server，并连接 `codexhub` 暴露的 remote-control WebSocket：

- `GET /backend-api/wham/remote/control/server`
- `GET /backend-api/remote/control/server`

日志验证到的 `initialize` 返回来源包括：

- `Codex Desktop/0.137.0-alpha.4 ...`：Codex App
- `codex_vscode/0.137.0-alpha.4 ...`：VS Code 插件
- `codexhub/0.137.0 ... WindowsTerminal ...`：CLI/TUI

WebSocket 握手 header 的 `user-agent` 通常为空，`x-codex-name` 是机器名，不能区分来源。可靠来源在 `initialize` 响应的 `result.userAgent` 中。

当前实现是单连接模型：

- `remote.outbound_tx` 只有一个
- `remote.connection_epoch` 只有一个
- `remote.clients` 是全局一份
- `remote.pending` 随 client state 全局混用

多个 app-server 同时连接时，新连接会覆盖旧连接，旧连接 writer 关闭后又重连，形成连接风暴：

- `ws_open` 快速增长
- `remote_control_disconnected reason=websocket closed` 快速增长
- Codex 侧可能报 `Incoming line queue overflow`

## 2. 目标

第一版目标：

1. Codex App、VS Code、CLI/TUI 可以同时连接到 `codexhub`。
2. 连接之间不互相覆盖，不因为某个连接断开把全局 remote-control 标为断开。
3. IM 请求只发送给一个执行端，不做广播。
4. 自动选择最高优先级且已初始化的执行端：
   - Codex App
   - VS Code
   - CLI/TUI
   - Unknown
5. GUI/API 能展示当前执行端和已连接端。

非目标：

1. 不把一条 IM 消息同时发送给多个 app-server。
2. 不在第一版支持每个 IM 会话手动选择不同执行端。
3. 不实现跨 app-server 事件合并或广播去重。
4. 不复制官方完整多 controller/client tracker 体系。

## 3. 路由原则

`codexhub` 只负责选择一个 app-server 作为 IM 执行目标：

```text
IM message -> selected app-server -> Codex thread/turn
```

Codex App 和 VS Code 对同一个 thread 的本地同步由 Codex 官方本地机制负责。`codexhub` 不主动把同一条 IM 请求复制给多个 app-server。

默认优先级：

```text
Codex App > VS Code > CLI/TUI > Unknown
```

当高优先级连接可用时，新的 IM 请求使用高优先级连接。已有请求的 response 仍回到发出该请求的连接 state，不能被新连接抢走。

## 4. 数据结构

新增连接级状态：

```rust
RemoteControlServerConnection {
    connection_id: String,
    connection_epoch: u64,
    connected: bool,
    initialized: bool,
    source_kind: RemoteControlSourceKind,
    user_agent: Option<String>,
    server_id: Option<String>,
    server_name: Option<String>,
    installation_id: Option<String>,
    account_id: Option<String>,
    subscribe_cursor: Option<String>,
    outbound_tx: Option<UnboundedSender<OutboundWsMessage>>,
    connected_at_ms: Option<u128>,
    last_ws_inbound_at_ms: Option<u128>,
    last_ws_ping_at_ms: Option<u128>,
    last_ws_pong_at_ms: Option<u128>,
    clients: HashMap<String, RemoteControlClientState>,
    stream_diagnostics: HashMap<String, RemoteControlStreamDiagnostics>,
}
```

`RemoteControlInner` 保留 legacy 汇总字段用于兼容旧 API/GUI，但真实发送与接收状态以 `connections` 为准：

```rust
connections: HashMap<String, RemoteControlServerConnection>
active_connection_id: Option<String>
next_connection_epoch: u64
```

`active_connection_id` 不是“唯一连接”，只是当前按优先级选出的执行端。

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

更新来源后重新计算 `active_connection_id`。

## 6. 请求发送

新增选择函数：

```rust
select_active_connection_locked(remote) -> Option<String>
```

选择条件：

1. `connected == true`
2. `outbound_tx.is_some()`
3. default client initialized
4. source priority 最大
5. 同优先级时选择最近连接或最近活跃连接

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
4. 重新选择 `active_connection_id`。
5. 如果没有任何可用连接，汇总状态才显示 disconnected。

不允许旧连接关闭时执行：

```rust
remote.connected = false;
remote.outbound_tx = None;
```

除非它是最后一个连接。

## 8. 状态 API/GUI

`/api/remote-control/status` 继续保留 legacy 字段，但新增：

```json
{
  "activeConnectionId": "...",
  "activeSourceKind": "codex_app",
  "activeUserAgent": "Codex Desktop/...",
  "connections": [
    {
      "id": "...",
      "sourceKind": "codex_app",
      "userAgent": "Codex Desktop/...",
      "connected": true,
      "initialized": true,
      "healthy": true
    }
  ]
}
```

GUI 使用新增字段显示：

- 当前执行端：Codex App / VS Code / CLI
- 已连接端列表
- 连接异常时显示最近错误和来源

## 9. 实施顺序

第一阶段：

1. 添加连接级结构和来源枚举。
2. WebSocket open 时创建 connection，不覆盖其它连接。
3. WebSocket close 时只关闭当前 connection。
4. initialize result 更新 connection `user_agent/source_kind`。
5. 请求发送选择最高优先级 connection。

第二阶段：

1. status API 输出连接列表。
2. GUI 显示当前执行端和多端连接状态。
3. 清理连接风暴诊断日志，保留关键来源日志。

第三阶段：

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
