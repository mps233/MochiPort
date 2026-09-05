# MochiPort

Domain language for Codex thread settings exposed through MochiPort's IM
channels.

## Thread Settings

**Thread Model**:
The model selected for subsequent turns in one Codex thread.
_Avoid_: Chat model, session engine

**Reasoning Effort**:
A model-specific Codex thread setting that controls the requested reasoning
depth for subsequent turns.
_Avoid_: Reasoning speed, thinking mode

**Fast Mode**:
A per-thread service-tier setting that requests the Fast tier for a supported
model. It increases model speed at a higher usage rate; Standard is the
non-Fast setting.
_Avoid_: Reasoning effort, a different model

**Settings Draft**:
The Telegram-local, unapplied proposed values for Thread Model, Reasoning
Effort, and Fast Mode in one bound Codex thread.
_Avoid_: Saved settings, global profile

## 账号池

**账号池**:
MochiPort 从 Sub2API 平台管理接口获取的账号集合；侧栏的“账号池”页面专指它。
_Avoid_: 消息渠道账号、Codex 凭据、官方账号

**池账号**:
账号池中的一个 Sub2API 账号，带平台、状态、可调度标记和余额/账单快照。
_Avoid_: 渠道账号

**可调度**:
池账号在 Sub2API 平台上是否参与上游调用的持久标记；MochiPort 可在账号池页面逐账号切换它，但不会创建、删除或编辑账号，也不会清除错误或临时冷却状态。
_Avoid_: 启用/禁用

**连接配置**:
访问 Sub2API 管理接口所需的 Base URL 与 Admin API Key 组合；Admin API Key 只保存在 daemon，不进入 GUI 界面长期展示或仓库。
_Avoid_: 账号密码、登录凭据
