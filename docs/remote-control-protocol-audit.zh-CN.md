# Codex Remote-Control 协议审计

更新时间：2026-06-04

本文只记录 Codex 官方 remote-control 协议事实、当前 `codexhub` 的对接现状，以及下一步排查方向。不把 IM 绑定兜底、图片发送、Telegram/微信/飞书渲染作为协议原因。

## 1. 参考源码

官方实现位于 `references/codex-main/codex-rs`：

- `app-server-transport/src/transport/remote_control/protocol.rs`
  - 定义 wire envelope：`ClientEnvelope`、`ServerEnvelope`、`ClientEvent`、`ServerEvent`、`PongStatus`。
- `app-server-transport/src/transport/remote_control/websocket.rs`
  - Codex app-server 作为 WebSocket 客户端连接 remote-control backend。
  - 管理重连、WebSocket ping/pong、server envelope buffer、ACK 清理、分片。
- `app-server-transport/src/transport/remote_control/client_tracker.rs`
  - 把 remote-control `(client_id, stream_id)` 映射为 app-server 内部 `ConnectionId`。
  - `remote_control_client_unknown` 的根源在这里。
- `app-server/src/transport.rs`
  - app-server 向连接写消息时的 backpressure 行为。
- `app-server-transport/src/transport/remote_control/tests.rs`
  - 覆盖 ping unknown、initialize、ACK 清 buffer、重连等行为。

我们当前实现主要在：

- `src/remote_control_backend.rs`
- `src/app_state.rs`
- IM 入口在 `src/im/**`，但 IM 不是 remote-control wire 协议的一部分。

## 2. 官方协议角色

官方命名是从 Codex app-server 视角看的：

- `ClientEnvelope`：remote-control backend 发给 Codex app-server 的 envelope。
- `ServerEnvelope`：Codex app-server 发给 remote-control backend 的 envelope。
- 我们的 `codexhub` 在这个协议里扮演 remote-control backend。
- Codex Desktop/App 的 app-server 会主动连接我们的 `/backend-api/wham/remote/control/server` WebSocket。

因此我们代码里的 `OutgoingClientEnvelope` 对应官方 `ClientEnvelope`；`IncomingServerEnvelope` 对应官方 `ServerEnvelope`。

## 3. HTTP 接入流程

官方 URL 由 `normalize_remote_control_url()` 生成：

- `POST /backend-api/wham/remote/control/server/enroll`
- `POST /backend-api/wham/remote/control/server/refresh`
- `GET /backend-api/wham/remote/control/server`

WebSocket 请求头官方要求：

- `x-codex-server-id`
- `x-codex-name`，base64 编码的 server name
- `x-codex-protocol-version = 3`
- `authorization = Bearer <remote_control_token>`
- `x-codex-installation-id`
- 可选 `x-codex-subscribe-cursor`

官方 `load_remote_control_auth()` 明确要求 ChatGPT auth；API key auth 不支持。这是 Codex app-server 侧启用 remote-control 的前置限制，不是 WebSocket wire envelope 的内容。我们支持第三方 key 的方案应尽量只处理本地 backend 兼容，不要把官方 ChatGPT 远程能力假装成第三方 key 能完整支持。

## 4. Wire Envelope

`ClientEnvelope` 字段：

- `type`
- `client_id`
- `stream_id`，现代协议应携带；官方仍有 legacy 兼容逻辑
- `seq_id`
- `cursor`

`ClientEvent` 类型：

- `client_message`
- `client_message_chunk`
- `ack`
- `ping`
- `client_closed`

`ServerEnvelope` 字段：

- `type`
- `client_id`
- `stream_id`
- `seq_id`

`ServerEvent` 类型：

- `server_message`
- `server_message_chunk`
- `ack`
- `pong`

分片限制官方常量：

- target：`100 KiB`
- 单 envelope 最大：`150 KiB`
- reassembled 最大：`100 MiB`
- segment count 最大：`1024`

## 5. Initialize 是创建连接的唯一入口

官方 `client_tracker.rs` 里只有 `ClientEvent::ClientMessage` 且 JSON-RPC method 为 `initialize` 时，才会创建 app-server 内部 remote-control connection。

结论：

- `initialized` 不是创建连接的请求，只是 initialize 后的普通通知。
- initialize 之前，非 initialize 的 client message 会被官方忽略。
- 如果同一个 `(client_id, stream_id)` 再次发送 initialize，官方会先关闭旧 connection，再创建新 connection。

这意味着：`remote_control_client_unknown` 后，重新发送 `initialize` 是协议允许的；但 initialize 只恢复 JSON-RPC 连接，不会自动恢复业务层 thread 订阅。

## 6. ACK 语义

官方注释说明：backend 对 `ServerEnvelope` 发送 `Ack`，表示该 `(client_id, stream_id)` 下 `seq_id <= ack.seq_id` 的 server envelope 已确认。

分片 ACK：

- chunk ACK 携带 `segment_id`。
- `segment_id = None` 等同于确认整个 `seq_id`。
- 官方 buffer 用 `(seq_id, segment_id)` 清理已 ACK 的 wire chunk。

ACK 的提交点：

- remote 收到 `ServerEnvelope` 后，应先做基本协议校验：
  - `client_id` / `stream_id` 是否是当前连接。
  - `seq_id` / `segment_id` 是否重复或乱序。
  - 分片消息是否已记录 chunk 状态；如果是最后一个 chunk，是否已成功重组为完整 JSON-RPC message。
- 校验通过后，remote 应把消息交给内部处理队列：
  - 普通 `server_message`：成功入队后立刻 ACK。
  - `server_message_chunk`：成功记录该 chunk 状态后立刻 ACK；如果最后一个 chunk 完成重组，则完整消息入队后继续由业务队列处理。
  - `pong`：更新 heartbeat 状态后立刻 ACK。
- ACK 只表示 remote 已经接管这条 wire message，不表示 IM 平台已经发送成功。

关键点：

- ACK 是 transport 层确认，不是“已经成功发到飞书/TG/微信”。
- 如果 ACK 被 IM 渲染、日志写入、图片上传等下游处理阻塞，app-server 会认为 backend 消费慢。
- 更符合协议的实现应是：WebSocket reader 快速校验、维护分片状态、把完整消息或状态事件放进内部队列，然后立即 ACK；IM 分发异步处理。

## 7. Backpressure 与断连

官方 `websocket.rs` 有 `BoundedOutboundBuffer`，容量来自 `CHANNEL_CAPACITY = 128`。

流程：

1. app-server 生成 `ServerEnvelope`。
2. 写入 outbound buffer。
3. 发给 backend。
4. backend 发 ACK。
5. app-server 收到 ACK 后从 outbound buffer 清掉。

当 outbound buffer 达到容量，官方 writer 会暂停从 `server_event_rx` 读取。再往 app-server 的 remote-control connection 写消息时，`app-server/src/transport.rs` 使用 `try_send`；如果队列满，会记录：

`disconnecting slow connection after outbound queue filled`

并断开该 remote-control connection。

这是一条重要协议链路：大量 `server_message` 如果 ACK 不够快，最终可能导致 app-server 删除当前 `(client_id, stream_id)`。

## 8. `remote_control_client_unknown` 的协议含义

官方 `client_tracker.rs` 中 `ClientEvent::Ping` 的逻辑：

- 找到 `(client_id, stream_id)`：返回 `PongStatus::Active`。
- 找不到：返回 `PongStatus::Unknown`。

所以 `remote_control_client_unknown` 的严格含义只有一个：

当前 Codex app-server 的 `ClientTracker` 里已经没有这个 `(client_id, stream_id)`。

它不直接等价于：

- IM 绑定丢失
- 飞书/TG/微信发送失败
- 图片处理失败
- thread listener 一定丢失

可能导致 `(client_id, stream_id)` 不存在的官方路径包括：

- 从未成功 initialize。
- backend 用错了 `client_id` 或 `stream_id`。
- backend 发送了 `client_closed`。
- app-server idle sweep 认为 client 10 分钟无活动。
- app-server 连接 worker 退出，`bookkeep_join_set()` 后关闭 client。
- app-server 向 remote-control connection 写消息时 backpressure 满，触发 slow connection disconnect。
- app-server 进程或 remote-control task 重启，内存中的 `ClientTracker` 清空。

当前要排查的是这些路径中的哪一个真实发生，而不是先加业务兜底。

## 9. 当前实现审计

### 9.0 2026-06-03 已落地调整

- 已移除 `listener_epoch_by_thread`、`ensure_thread_listener()` 这类 IM/thread 业务兜底，`remote_control_client_unknown` 不再清理 IM listener 状态。
- 已取消针对 `item/commandExecution/outputDelta` 的 method-specific fast ACK。
- `handle_server_envelope()` 已调整为 transport 优先：
  - 校验当前 `client_id/stream_id` 和 duplicate cursor。
  - 普通 `server_message` 成功进入内部 work queue 后立刻 ACK。
  - `server_message_chunk` 成功记录 chunk 后立刻 ACK；最后一个 chunk 完成重组后把完整 message 入 work queue。
  - `pong` 先记录 heartbeat 状态并 ACK；`unknown` 后的 reinitialize 放在 ACK 之后的 worker 中处理。
  - IM 分发、server request/response 处理、日志摘要不再阻塞 ACK 路径。
- 每次新的 WebSocket 会话都会重新发送 `initialize`。原因是官方 `ClientTracker` 是 Codex app-server 内存态，remote 进程自己的 `initialized=true` 不能证明 app-server 仍持有 `(client_id, stream_id)`。
- 每次新的 WebSocket 会话会清理当前 `(client_id, stream_id)` 的 server ACK cursor，避免 app-server 重启后 server `seq_id` 从 1 开始时被旧 cursor 误判为 duplicate。

### 9.1 基本符合

- 我们实现了 enroll、refresh、server WebSocket endpoint。
- 我们发送 `initialize`，收到 response 后发送 `initialized`。
- 我们支持 `client_message` 分片。
- 我们支持接收 `server_message_chunk` 并重组。
- 我们对 `server_message` / `server_message_chunk` / `pong` 发送 ACK。
- 我们用 `client_id + stream_id + seq_id + segment_id` 做重复 envelope 判断。

### 9.2 明确不完整或需要修正

1. WebSocket 鉴权没有按官方 token 严格校验。

   官方 WebSocket 带 `authorization = Bearer <remote_control_token>`。当前 `websocket()` 只检查 `x-codex-protocol-version` 并继续 upgrade，token 基本没有参与 server WebSocket 校验。这是协议和安全层面的缺口。

2. 协议版本不匹配只记录 warning。

   官方协议版本是 `3`。当前版本不匹配仍允许连接。严格对齐时应拒绝或至少不进入正常 remote-control 会话。

3. `ClientClosed` 没有作为正常 shutdown 路径使用。

   官方支持 `client_closed`。当前主要依赖 WebSocket 关闭和重连。这不一定是当前 bug 的原因，但如果要严格实现逻辑 client 生命周期，需要明确什么时候发 `client_closed`。

4. `x-codex-subscribe-cursor` / `cursor` 没有实际使用。

   官方 app-server 会把 backend 发来的 `cursor` 保存，并在后续 WebSocket reconnect 时通过 `x-codex-subscribe-cursor` 发给 backend。当前本地 backend 没有 backend-command replay 需求，可以不支持，但文档上要明确这是未实现能力，而不是遗漏了还能假装完整协议。

### 9.3 `unknown` 后的协议对齐恢复策略

`pong status=unknown` 的恢复必须以官方 `ClientTracker` 行为为准，不能为了绕开问题默认生成新的 `stream_id`。

官方事实：

- `(client_id, stream_id)` 是 remote-control logical client 的身份。
- `initialize` 是创建 logical client 的唯一入口。
- 同一个 `(client_id, stream_id)` 再次发送 `initialize` 时，官方会先关闭旧 client，再创建新的 app-server connection。
- 非 `initialize` 的 client message 在 logical client 不存在时会被忽略。
- `ping -> unknown` 只说明官方 `ClientTracker.clients` 里没有当前 `(client_id, stream_id)`，不说明 WebSocket 一定断开，也不说明业务 thread 订阅一定还有效。
- 官方测试只把 `unknown` 当作 `ClientTracker` 缺失 logical client 的状态回包；没有“收到 unknown 后自动恢复 thread/turn 订阅”的专门协议分支或测试用例。

因此补救顺序应为：

1. 收到 `unknown` 后，保持同一个 `(client_id, stream_id)`，立即重新发送 `initialize`。
2. reinitialize 期间暂停普通业务请求继续投递到旧状态；已有 pending 请求可以失败一次，由请求层在初始化恢复后按幂等性决定是否重试。
3. 如果同 stream reinitialize 在短超时内没有完成，再主动关闭当前 WebSocket，让官方 app-server 走自己的 reconnect loop。
4. 新 WebSocket 建立后继续使用当前 logical client 状态重新 `initialize`，并清理当前 stream 的 server ACK cursor，避免 app-server 侧 seq 从 1 重新开始时被误判为 duplicate。
5. 不默认发送 `client_closed`。`client_closed` 是明确关闭 logical client 的协议动作，只应在我们主动废弃 client 或 shutdown 时使用。
6. 不默认换 `stream_id`。换 stream 会创建另一个 logical client，可能绕开官方状态机，并让现有 thread/turn 状态更难判断。
7. 不自动重放非幂等请求，尤其是 `turn/start`、`turn/steer`、`thread/start`、`thread/fork`。这些请求如果在 unknown/reinitialize 窗口中失败，应暴露为失败或由上层显式决定下一步，不能悄悄重复用户输入。

业务订阅补救：

- `initialize` 只恢复 remote-control JSON-RPC logical connection，不保证原 connection 上的 thread 实时订阅还能继续。
- transport recovery 完成后，如果本地 client 仍记录 `current_thread_id`，可以立即发一次 `thread/resume { threadId, excludeTurns: true }`。官方 app-server 对 loaded/running thread 会通过 thread listener 把当前 connection 重新加入 subscriber 集合。
- 这一步是业务层补救，不是官方 `unknown` transport 协议的一部分；它不能恢复已经丢失的 connection-scoped `outputDelta`，也不能保证旧 turn 还在运行。
- 如果 `thread/resume` 响应或后续 `thread/status/changed` 显示 `idle`、`notLoaded`、`systemError`，本地应清掉该 thread 的 current turn，避免后续 IM 消息继续误判为可 `turn/steer` 的活跃 turn。

`cursor` 的对齐边界：

- backend 发出的 `cursor` 是给 app-server 保存的订阅位置。
- app-server reconnect 时会通过 `x-codex-subscribe-cursor` 把最后看到的 cursor 带回 backend。
- 当前本地 backend 可以先做到“记录 header、为发出的 client envelope 填 cursor、写日志”，但如果没有持久化 backend command log，就不能宣称支持完整 replay。
- `cursor` 不是 `unknown` 的根修复；`unknown` 仍然必须靠 `initialize` 恢复 `ClientTracker`。

## 10. 当前日志能确认的事实

从现有 release 日志能确认：

- `remote_control_client_unknown` 前，backend 收到 app-server 的 `pong status:"unknown"`。
- 这个 `unknown` 是 app-server 返回的，不是 IM 层生成的。
- clean case 中出现时当前 `(client_id, stream_id)` 为：
  - `client_id = codexhub-feishu`
  - `stream_id = stream_3d922febc185190d`
- 该现象发生在 `computer-use` 读取技能说明期间，前面出现大量 `item/commandExecution/outputDelta`。
- 当前实现没有在 transport ACK 后跳过 `item/commandExecution/outputDelta`；日志显示它会进入 `server_work_begin` / `server_message_in` / `im_trace`，随后才在通知分发前返回。
- IM 层已经忽略 `item/commandExecution/outputDelta`，所以它对用户侧展示没有价值，但会制造 transport、worker 和诊断日志压力。
- 现有日志还不能单独证明是 slow connection disconnect，因为没有 app-server 自己的 `disconnecting slow connection after outbound queue filled` 日志。

因此当前合理结论是：

`unknown` 表明 app-server 已经丢失当前 remote-control logical client；大量 outputDelta 与 ACK/日志压力是高风险相关因素，但不是已确认根因。需要继续用协议级日志证明 ACK 是否变慢、worker 队列是否堆积、app-server 是否主动关闭 logical client。

当前版本已增加诊断点：

- `command_output_delta_pressure`：按 `(client_id, stream_id)` 聚合 `item/commandExecution/outputDelta` 计数、最近 thread/item、worker 队列剩余容量。
- `server_ack_slow`：记录 ACK 耗时超过阈值的 server envelope。
- `remote_control_client_unknown_context`：在 `pong status=unknown` 时打印该 stream 的 outputDelta/ACK 摘要和当前已注册 streams。
- `stale_server_envelope`：不再只打印 default stream，而是打印真实 resolved client key 和注册 stream 列表，避免误判多 stream 状态。

## 11. 下一步排查顺序

1. 先停止继续加 IM 兜底逻辑。

   `unknown` 是 transport/client_tracker 层问题，先不要用飞书/TG/微信绑定恢复解释它。

2. 保持协议层和 IM 层分离。

   不再用 IM 绑定恢复解释 `remote_control_client_unknown`。后续如果要恢复业务 thread 订阅，应建立在 transport/client_tracker 已确认健康之后。

3. 增加协议级日志，而不是 IM 日志。

   必须能看到：

   - 收到每个 `ServerEnvelope` 的 `client_id/stream_id/seq_id/type/method`。
   - 发出 ACK 的 `seq_id/segment_id`。
   - WebSocket close 原因。
   - 是否收到 `server_pong status=unknown`。
   - 每次 initialize 的 `client_id/stream_id/seq_id`。
   - app-server 是否发生 process restart 或 remote-control reconnect。

   如果下一次复现时 `remote_control_client_unknown_context` 显示：

   - `output_delta_count` 在短时间内持续升高；
   - `last_ack_seq_id` 已接近 `output_delta_last_seq_id`，但 app-server 仍返回 `unknown`；
   - `last_ack_elapsed_ms` / `max_ack_elapsed_ms` 没有明显升高；

   那么可以排除“remote ACK 慢导致 app-server 丢 client”，继续查 app-server outbound/connection close。反之，如果 ACK 延迟或 worker capacity 异常，就可以把 outputDelta/backpressure 作为实证根因推进修复。

4. 用 mock app-server 复现协议链路。

   最小测试应覆盖：

   - ping before initialize 返回 unknown。
   - initialize 后 ping 返回 active。
   - server envelope flood 时 backend 能及时 ACK。
   - backend ACK 后不会重复处理。
   - app-server 断开并以同一 stream 重连时，不误判新消息 duplicate。
   - app-server 进程重启导致 seq 从 1 开始时，backend 不应因旧 cursor 丢消息。

5. 协议修复方向应是 transport 队列解耦。

   WebSocket reader 只做：

   - envelope 校验
   - chunk 重组
   - 入内部队列
   - ACK

   IM 渲染、飞书图片、TG/微信发送、日志落盘都不能阻塞 ACK 路径。

## 12. 对 `remote_control_client_unknown` 的判断标准

以后看到 `remote_control_client_unknown`，先按这个顺序查：

1. 该 `(client_id, stream_id)` 是否曾成功 initialize。
2. initialize 后是否有 `server_message` 正常返回。
3. unknown 前 ACK 是否持续发出，ACK 延迟是否升高。
4. unknown 前是否出现大量 server envelope 且 ACK 间隔异常。
5. Codex app-server 是否重启或 remote-control websocket 是否断过。
6. 是否发生 `client_closed`。
7. 是否超过 10 分钟没有 client activity。
8. 是否出现 app-server 慢连接断开日志。

只有确认 transport/client_tracker 正常后，才继续看 IM thread 绑定或平台发送问题。

## 13. 官方测试用例对照

这一节把官方 `references/codex-main/codex-rs/app-server-transport/src/transport/remote_control/**` 的测试当作协议契约，而不是只看实现细节。后续修复 `unknown`、重连、ACK 压力时，优先按这里补本地 mock app-server 测试。

### 13.1 连接生命周期、initialize 与 unknown

官方覆盖：

- `tests.rs::remote_control_transport_manages_virtual_clients_and_routes_messages`
  - initialize 前 `ping` 返回 `pong status=unknown`。
  - initialize 前的非 initialize `client_message` 被忽略。
  - `client_message(method=initialize)` 是创建 logical client 的唯一入口，会产生 `ConnectionOpened`，随后转发 initialize 到 app-server transport。
  - initialize 后 `ping` 返回 `active`。
  - app-server transport writer 发送的消息会被包装为 `server_message` 发给 backend。
  - `client_closed` 会删除该 logical client，随后 `ping` 再次变成 `unknown`。
- `tests.rs::remote_control_transport_reconnects_after_disconnect`
  - WebSocket 断开后官方 remote-control loop 会重新连接 backend。
  - 新 WebSocket 上仍然通过 initialize 创建 logical client。
  - WebSocket 握手携带 `authorization: Bearer <remote_control_server_token>`。
- `tests.rs::remote_control_handle_enable_disable_stops_and_restarts_connections`
  - enable/disable 是 remote-control loop 的外层状态开关，disable 会停止连接，enable 后重新进入连接流程。

我们当前状态：

- 新 WebSocket 建立后会重新发送所有已知 logical client 的 initialize，不再只依赖本地 `initialized=true`。
- 收到 `pong status=unknown` 后，当前实现按同一个 `(client_id, stream_id)` 触发 reinitialize；短超时内未恢复则主动关闭 WebSocket，让官方 app-server 走 reconnect loop。
- 默认不发送 `client_closed`，避免把“恢复 logical client”误做成“主动关闭 logical client”。
- 已支持多 virtual client 共用 `client_id`、区分 `stream_id`，符合官方 `initialize_with_new_stream_id_opens_new_connection_for_same_client` 的身份模型。

缺口和后续动作：

- 还缺一个本地 mock app-server 生命周期测试：WS open -> backend initialize -> app-server response -> backend initialized -> app-server pong active/unknown -> backend same-stream reinitialize。
- 还缺测试证明 unknown 恢复期间不会默认换 `stream_id`，也不会发送 `client_closed`。
- 还缺测试覆盖 reinitialize 超时后 backend 主动 close WebSocket，随后 app-server reconnect 时 backend 会重新 initialize 所有已知 clients。

### 13.2 `ClientTracker` 删除路径与队列超时

官方覆盖：

- `client_tracker.rs::cancelled_outbound_task_emits_connection_closed`
  - logical client 的 outbound task 退出后，`bookkeep_join_set()` 会发现该 client，并通过 `close_client()` 发出 `ConnectionClosed`。
- `client_tracker.rs::shutdown_cancels_blocked_outbound_forwarding`
  - 即使 server_event queue 被堵住，shutdown 也不能卡死。
- `client_tracker.rs::non_close_transport_event_send_times_out_when_queue_stays_full`
  - `IncomingMessage` 这类非 close transport event 发送到 app-server transport queue 时，如果队列一直满，会超时并返回错误。
- `client_tracker.rs::incoming_message_timeout_does_not_advance_seq_id`
  - inbound message 只有成功送进 app-server transport 后，才会推进 inbound seq dedupe 游标；失败后允许同 seq 重试。
- `client_tracker.rs::initialize_timeout_closes_open_connection`
  - initialize 已创建 connection 但 initialize message 转发超时时，官方会回滚并关闭刚创建的 connection。
- `client_tracker.rs::close_client_waits_for_transport_event_queue_capacity`
  - close 事件会等待 transport event queue 有容量，不会悄悄丢。
- `client_tracker.rs::close_client_keeps_forwarding_after_caller_is_aborted`
  - close 已开始后，即使调用方 task 被 abort，也要继续把 `ConnectionClosed` 送出去。
- `client_tracker.rs::legacy_initialize_without_stream_id_resets_inbound_seq_id`
  - legacy 无 `stream_id` initialize 有兼容路径，但现代协议不应依赖这个 fallback。

我们当前状态：

- 我们是 backend，不实现官方 app-server 的 `ClientTracker`，但必须把对端这些删除路径视为真实可能发生的 `unknown` 来源。
- 当前恢复逻辑已经把 `unknown` 解释为“官方 `ClientTracker.clients` 缺失当前 key”，而不是 IM/thread 丢失。
- 新 WebSocket reset 会清掉本地 initialize 状态和对应 server ACK cursor，但保留可重放的普通 pending request。

缺口和后续动作：

- 本地测试需要模拟 app-server transport queue 满导致 initialize 或普通 inbound message 失败，再确认 backend 对同 seq / 同 stream 的重试策略。
- 需要把“outbound task 退出 / idle sweep / client_closed / initialize rollback”作为 `unknown` 日志判断维度，日志里不要只看 WebSocket 是否还连着。
- `legacy stream_id=None` 只应作为兼容知识记录；我们自己的 outbound envelope 应继续携带 `stream_id`。

### 13.3 ACK、backpressure 与 `CHANNEL_CAPACITY=128`

官方覆盖：

- `tests.rs::remote_control_transport_clears_outgoing_buffer_when_backend_acks`
  - backend ACK 后，官方 app-server 的 outbound buffer 会清理对应 `(client_id, stream_id, seq_id, segment_id)`。
- `websocket.rs::outbound_buffer_acks_by_stream_id`
  - ACK 只清当前 `(client_id, stream_id)`，不会误清其他 client 或其他 stream。
- `websocket.rs::outbound_buffer_retains_unacked_messages_until_ack_advances`
  - ACK 游标没有推进到的 envelope 必须保留。
- `websocket.rs::outbound_buffer_advances_segmented_acks_by_wire_cursor`
  - 分片 ACK 按 `(seq_id, segment_id)` 推进。
- `websocket.rs::outbound_buffer_treats_segmentless_acks_as_seq_level_acks`
  - `segment_id=None` 表示确认整个 `seq_id`。
- `websocket.rs::run_server_writer_inner_assigns_contiguous_seq_ids_per_stream`
  - 官方发给 backend 的 `ServerEnvelope.seq_id` 是按 `(client_id, stream_id)` 连续递增的。
- `websocket.rs::run_server_writer_inner_sends_periodic_ping_frames`
  - 官方 writer 会定期发 WebSocket ping，reader 要及时 pong。

关键推论：

- 官方 `BoundedOutboundBuffer` 容量是 `CHANNEL_CAPACITY=128`。它满了以后，writer 暂停从 `server_event_rx` 取新 envelope。
- 如果 backend ACK 慢，官方 app-server 的 remote-control connection writer 可能继续被上游写入压满，最终走 app-server transport 的 slow connection disconnect 路径。
- 这条路径是源码和测试支持的风险链路，但真实复现是否命中它，仍要靠日志中的 ACK 延迟、queue capacity、WebSocket close 原因来证明。

我们当前状态：

- `server_message`、`server_message_chunk`、`pong` 已调整为 transport 接管后快速 ACK，IM 分发不再阻塞 ACK。
- `server_message_chunk` 成功记录 chunk 后立即 ACK；最后一个 chunk 重组完成后再把完整 JSON-RPC message 放入内部 work queue。
- 已有 `ack_cursor_gt` 和 chunk 重组基础单测，但还没有 flood 压测级别的 ACK 时延回归。
- 本地 server work queue 容量是 `4096`，它是 backend 内部消费队列，不等于官方 app-server 的 `CHANNEL_CAPACITY=128`；不能用它证明官方 buffer 不会满。

缺口和后续动作：

- 补 mock app-server flood 测试：在 50ms 内发送至少 300 条 `server_message` / `outputDelta`，断言 backend 对每条有效 envelope 都及时 ACK，且 ACK 不等待 IM 日志、图片、平台发送。
- 补分片 ACK 测试：app-server 发送多 chunk server message，backend 应逐 chunk ACK，最终只投递一次完整 message。
- 补 reconnect seq reset 测试：app-server reconnect 后 server `seq_id` 从 1 开始，backend 不应被旧 ACK cursor 误判为 duplicate。

### 13.4 WebSocket 连接维护、cursor 与鉴权

官方覆盖：

- `websocket.rs::connect_remote_control_websocket_recovers_after_unauthorized_enrollment`
  - enroll 或 connect 遇到 unauthorized 时，会尝试 auth recovery。
- `websocket.rs::connect_remote_control_websocket_recovers_after_unauthorized_refresh`
  - refresh unauthorized 也走恢复链路。
- `websocket.rs::connect_remote_control_websocket_requires_sqlite_state_db`
  - remote-control 启用时要求 state db；缺失时会报 disabled/不可用状态。
- `websocket.rs::connect_remote_control_websocket_requires_chatgpt_auth`
  - 官方 remote-control 依赖 ChatGPT auth，不支持只靠 API key。
- `websocket.rs::run_remote_control_websocket_loop_shutdown_cancels_reconnect_backoff`
  - shutdown 可以打断 reconnect backoff，不会卡住退出。
- `websocket.rs::run_websocket_reader_inner_times_out_without_pong_frames`
  - WebSocket pong 超时会断开当前连接。
- `websocket.rs::build_remote_control_websocket_request`
  - 连接 backend 时带 `x-codex-server-id`、`x-codex-name`、`x-codex-protocol-version=3`、`authorization`、`x-codex-installation-id`，有 cursor 时带 `x-codex-subscribe-cursor`。

我们当前状态：

- server 端会读取并记录 `x-codex-subscribe-cursor`，也会给发出的 `ClientEnvelope` 填基础 `cursor`。
- 当前 cursor 只是“记录 header + 透传/生成 cursor + 日志可见”，没有持久化 backend command log，因此不支持完整 replay 语义。
- WebSocket recovery 超时后会主动 close，让官方 app-server reconnect。
- WebSocket token 和 protocol version 当前仍偏宽松：协议版本不匹配只是 warning；`authorization` 没有按 enroll/refresh 颁发的 server token 严格校验。

缺口和后续动作：

- 补 WebSocket header 测试：缺失/错误 `x-codex-protocol-version`、缺失/错误 bearer token 时，应按我们最终决定的兼容策略处理，并写清楚是否严格拒绝。
- 补 cursor 测试：收到 `x-codex-subscribe-cursor` 只更新状态和日志，不宣称 replay；发出的 client envelope 带当前 cursor。
- 如果要完整对齐官方 cursor，需要新增 backend command log，并按 cursor replay；这不是修 `unknown` 的前置条件。

### 13.5 分片、重复包与 stream invalidation

官方覆盖：

- `segment_tests.rs::reassembles_client_message_chunks`
  - backend 发给 app-server 的 client chunks 能重组为完整 JSON-RPC message。
- `segment_tests.rs::splits_large_server_messages_into_wire_chunks`
  - app-server 发给 backend 的大 server message 会被拆成 `server_message_chunk`。
- `segment_tests.rs::invalidates_incomplete_stream_assemblies`
  - stream 被关闭或失效后，未完成分片 assembly 必须失效。
- `segment_tests.rs::resets_incomplete_client_assembly_when_stream_changes`
  - 同 client 切换 stream 时，旧 stream 未完成 assembly 不应污染新 stream。
- `segment_tests.rs::ignores_stale_chunks_without_dropping_newer_assembly`
  - 旧 seq 的 stale chunk 不能破坏当前更新的 assembly。
- `segment_tests.rs::ignores_invalid_stale_chunks_without_dropping_newer_assembly`
  - 无效 stale chunk 也不能破坏当前 assembly。
- `segment_tests.rs::ignores_invalid_duplicate_chunks_without_dropping_current_assembly`
  - 无效 duplicate chunk 不能破坏当前 assembly。
- `websocket.rs::websocket_state_*`
  - 覆盖 duplicate/replay/oversize/out-of-order chunk，以及 stream invalidation 后清 cursor。

我们当前状态：

- 已支持 backend outbound `client_message` 分片。
- 已支持接收 app-server inbound `server_message_chunk` 并顺序重组。
- 当前 server chunk 处理更像“严格顺序 assembly”：duplicate、stale、invalid stale 是否完全按官方 nuance 处理，还需要专门测试确认。

缺口和后续动作：

- 补 server chunk duplicate/stale/out-of-order/oversize 测试，尤其要确认无效旧 chunk 不会清掉当前有效 assembly。
- 补 stream reset 测试：WebSocket reconnect、`unknown` 恢复、client stream 更换时，未完成 chunk assembly 不能跨 stream 继续生效。
- 补大消息 outbound 分片大小测试，确保每个 client chunk 序列化后不超过官方 `150 KiB` 上限。

### 13.6 enroll、pairing 与 client management 周边测试

官方覆盖：

- `tests.rs::remote_control_http_mode_enrolls_before_connecting`
  - HTTP mode 先 enroll，再连接 WebSocket。
- `tests.rs::remote_control_http_mode_refreshes_persisted_enrollment_before_connecting`
  - 已持久化 enrollment 时先 refresh。
- `tests.rs::remote_control_http_mode_reenrolls_when_refresh_reports_stale_enrollment`
  - refresh 发现 stale enrollment 时会重新 enroll。
- `tests.rs::remote_control_http_mode_clears_stale_persisted_enrollment_after_404`
  - 404 会清理匹配的 stale enrollment。
- `tests.rs::remote_control_stdio_mode_waits_for_client_name_before_connecting`
  - stdio mode 等 app-server client name 后再连接。
- `clients_tests.rs`
  - list/revoke clients 覆盖 disabled 状态、unauthorized retry、forbidden 不重试、decode error 保留上下文。
- `pairing_tests.rs`
  - start pairing、auth recovery、backend error/decode error 上下文、mismatched enrollment、disable 清 current enrollment。
- `enroll.rs` 单测
  - server token 到期前 refresh、响应体 token 脱敏、按 target/account 持久化、多条 enrollment 精确清理。
- `protocol.rs` 单测
  - remote-control URL 只接受官方 ChatGPT host 和 localhost；拒绝 unsupported URL。

我们当前状态：

- 本地 backend 已模拟 enroll/refresh/client list/client enroll finish 等必要接口，主要目标是让 Codex Desktop/App 能连上本地服务。
- 这些周边测试不是本次 `unknown` 的直接根因，但它们决定 WebSocket 能否稳定建立和重建。

缺口和后续动作：

- 如果后续严格校验 token，需要补 enroll/refresh -> WS authorization 的闭环测试。
- client management/list/revoke 目前更多是兼容 UI 需要，优先级低于 transport lifecycle、ACK、chunk、reconnect。
- URL normalization 和 ChatGPT auth 是官方 app-server 侧约束；本地 backend 文档应继续声明兼容边界，不把第三方 key 说成官方 remote-control 完整能力。

### 13.7 本地最小回归套件建议

优先补下面这些测试，再继续动实现：

1. `unknown_reinitializes_same_stream`

   mock app-server 连接 backend，完成 initialize 后返回 `pong status=unknown`；断言 backend 使用同一 `(client_id, stream_id)` 重新发送 initialize，不发送 `client_closed`，不更换 stream。

2. `unknown_reinitialize_timeout_closes_ws`

   mock app-server 对 reinitialize 不响应；断言 backend 在短超时后 close 当前 WebSocket。

3. `ws_reconnect_reinitializes_all_clients`

   注册多个 virtual clients，断开后用新 WebSocket 重连；断言每个 known client 都重新 initialize，且旧 server ACK cursor 被清理。

4. `server_flood_fast_ack`

   mock app-server 在短窗口内发送大量 `item/commandExecution/outputDelta`；断言 ACK 路径不等待 IM 分发，ACK 延迟保持在可接受阈值内。

5. `server_chunk_ack_and_reassembly`

   多 chunk server message 每个 chunk 都得到 ACK；最后一个 chunk 才投递完整 message；duplicate/stale/oversize chunk 不破坏当前 assembly。

6. `subscribe_cursor_is_recorded_but_not_replayed`

   WebSocket header 带 `x-codex-subscribe-cursor` 时，backend 记录并打日志；后续发出的 client envelope 带新 cursor；文档和测试都不宣称 replay。

7. `strict_ws_headers_when_enabled`

   按最终兼容策略验证 protocol version 和 bearer token。若短期仍允许宽松模式，测试名和断言要明确这是兼容模式，不是官方完整协议。
