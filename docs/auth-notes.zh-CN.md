# 认证说明

这份文档说明两件事：

1. 为什么仓库里会保留一些 auth 结构参考文件
2. 这些文件和 `codex-remote` 正常运行链路之间是什么关系

它不是“如何绕过官方 Codex / ChatGPT 登录”的说明文档。

## 范围

`codex-remote` 的定位是 remote-control backend + 飞书 bridge，不是 Codex 的认证层。

正常运行时，职责边界是这样的：

- 官方 Codex 负责账号状态、登录状态、provider 选择，以及 app-server 能否启动
- `codex-remote` 只在 app-server 已经能运行之后，负责 remote-control 传输和飞书适配

## 支持路径

受支持的工作流是：

1. 先把官方 Codex 配置到可以正常启动
2. 确认官方 Codex app-server 可以运行
3. 再通过 remote-control 把 `codex-remote` 接上去

如果官方 Codex 因为账号或登录状态本身就无法启动，应该先在 Codex 侧解决。bridge 只有在 app-server 已经能跑起来之后才有意义。

## `codex-remote` 会读取什么

`codex-remote` 会读取：

- `config.toml`
- 本地 bridge 状态文件
- 飞书凭证和 bridge 本地配置

`codex-remote` 不会读取或管理：

- `~/.codex/auth.json`
- Codex 的 session refresh 逻辑
- ChatGPT access token
- Codex 账号存储内部状态

daemon 和 shim 都不会加载 `references/` 目录下的 auth 参考文件。

## 为什么会有 `references/`

仓库里可能会保留一些本地参考材料。原因只有一个：之前做过本地研究，观察过 auth 文件结构，所以把参考信息留了下来。

保留它们的典型用途是：

- 对比观察过的 `auth.json` 结构
- 回看某次本地实验里有哪些字段
- 验证 remote-control bridge 和 Codex auth 存储其实是正交问题

它们不参与：

- daemon 启动
- shim 启动
- 飞书接入
- remote-control 握手

## 它和 remote-control 的关系

remote-control 链路开始的前提，是官方 Codex app-server 已经活着。

到了这一步，`codex-remote` 真正关心的是：

- app-server 是否连到了 `/backend-api/wham/remote/control/server`
- remote-control 的 `initialize` / `initialized`
- thread / turn 通知
- approval 请求与响应

它并不关心 Codex 最初是通过什么方式进入“可运行状态”的，只要那是一个官方支持或至少由用户自己维护、并且确实能启动 app-server 的 Codex 配置即可。

换句话说：

- auth 状态决定 Codex 能不能启动
- remote-control 决定一个已经运行的 Codex 会话如何桥接到飞书

这是相邻问题，不是同一个问题。

## 不支持的方向

这个仓库不会把下面这些内容当成正式支持路径：

- 手工拼装或伪造 token 后作为生产方案
- 把 `references/` 当成真实登录状态的替代品
- 把本地 auth 实验当成公开 bridge 契约的一部分

如果你要对外发布或运营 `codex-remote`，推荐表述应该是：

- 先有官方 Codex
- 再有 remote-control
- 最后才是飞书 bridge

## 实际调试建议

调试时，把问题拆开看：

- “官方 Codex 能不能启动？”  
  这是 Codex 认证 / provider / 配置问题。

- “Codex 启动后，`codex-remote` 能不能收到 remote-control 事件？”  
  这是 bridge / 协议 / 运行态问题。

- “thread 订阅后，飞书能不能正常收发消息？”  
  这是飞书 bridge 问题。

把这些边界拆清楚，排障和文档都会简单很多。
