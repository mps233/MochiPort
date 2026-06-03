# Codex Remote-Control 协议审计

更新时间：2026-06-03

本文只记录 Codex 官方 remote-control 协议事实、当前 `codex-remote` 的对接现状，以及下一步排查方向。不把 IM 绑定兜底、图片发送、Telegram/微信/飞书渲染作为协议原因。

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
- 我们的 `codex-remote` 在这个协议里扮演 remote-control backend。
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

## 10. 当前日志能确认的事实

从现有 release 日志能确认：

- `remote_control_client_unknown` 前，backend 收到 app-server 的 `pong status:"unknown"`。
- 这个 `unknown` 是 app-server 返回的，不是 IM 层生成的。
- clean case 中出现时当前 `(client_id, stream_id)` 为：
  - `client_id = codex-remote-feishu`
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
