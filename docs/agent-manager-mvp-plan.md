# CodexHub Agent Manager MVP 实施计划

## 📋 项目概述

### 目标
为 CodexHub 用户提供可视化的 Agent 管理能力，降低 Codex Agent 的使用门槛，让个人用户可以轻松创建、配置、管理和追踪自定义 Agent。

### 价值主张
- **降低配置门槛**：无需手写 TOML，通过表单即可创建 Agent
- **提升可观测性**：实时查看 Agent 调用链，理解多 Agent 协作过程
- **快速上手**：官方模板库提供最佳实践，一键复制即用
- **本地优先**：所有数据存储在本地，保护用户隐私

### 目标用户
个人 Codex 用户（VSCode/Desktop/CLI），尤其是希望定制 Agent 行为但不熟悉 TOML 配置的开发者。

---

### Week 5：调用链追踪（上）

**目标**：实现事件监听和数据记录

- [ ] **Day 1-2**：Agent Tracker 核心模块
  - 创建 `agent_tracker.rs`
  - 定义 `AgentSession` 数据模型
  - 实现事件处理方法（spawn/complete/fail）
  - 写入 `agent_sessions` 表

- [ ] **Day 3-4**：Remote Control 集成
  - 分析现有 `remote_control_backend` 代码
  - 识别 Sub-agent spawn 事件位置
  - 在事件发生时调用 `agent_tracker` 记录
  - 确保线程安全（使用 `Arc<Mutex<AgentTracker>>`）

- [ ] **Day 5**：历史数据查询
  - 实现 `get_session_tree()` 方法（查询父子关系）
  - 实现 `get_agent_history()` 方法（某个 Agent 的所有会话）
  - 单元测试：构造复杂调用链并验证查询结果

**交付物**：
- ✅ Agent Tracker 模块
- ✅ Remote Control 事件集成
- ✅ 历史数据查询 API

---

### Week 6：调用链追踪（下）+ 测试与发布

**目标**：实现可视化界面并完成整体测试

- [ ] **Day 1-3**：调用链树形视图
  - 创建 `agent_tree_panel.rs`
  - 使用 `wxTreeCtrl` 或 `wxDataViewTreeCtrl` 实现树形图
  - 从数据库加载数据 → 构建树结构
  - 节点显示：Agent 名称 + 状态图标 + 执行时间
  - 实时更新：监听 `agent_tracker` 事件 → 刷新 UI

- [ ] **Day 4**：详情展示
  - 点击节点 → 显示详情面板
  - 展示内容：输入/输出预览、token 消耗、错误信息
  - 颜色编码：运行中（蓝）、完成（绿）、失败（红）

- [ ] **Day 5**：集成测试
  - 端到端测试：创建 Agent → 使用 Codex spawn → 查看调用链
  - 压力测试：创建 50+ Agent，验证列表性能
  - 兼容性测试：解析各种格式的 TOML 文件
  - Bug 修复和性能优化

**交付物**：
- ✅ 调用链可视化界面
- ✅ 实时更新功能
- ✅ 完整的功能测试报告

---

## ✅ 验收标准

### 功能完整性
- [ ] 能够扫描并展示所有本地 Agent（`$CODEX_HOME/agents/*.toml`）
- [ ] 能够通过表单创建新 Agent，自动生成合法 TOML
- [ ] 能够编辑现有 Agent，修改保存后立即生效
- [ ] 能够删除 Agent（含确认对话框）
- [ ] 能够从 5 个内置模板快速创建 Agent
- [ ] 能够实时追踪 Agent 调用链并显示树形结构
- [ ] 能够查看每个 Agent 的使用统计（次数、最后使用时间）

### 性能要求
- [ ] 列表加载时间 < 500ms（100 个 Agent 以内）
- [ ] 搜索/过滤响应时间 < 100ms
- [ ] 调用链树刷新延迟 < 1s（从事件发生到 UI 更新）
- [ ] TOML 解析错误不导致程序崩溃（显示错误提示）

### 用户体验
- [ ] 表单填写流程流畅，无卡顿
- [ ] 错误提示清晰（如：名称重复、必填项为空）
- [ ] TOML 预览实时更新，格式正确
- [ ] 调用链树节点可展开/折叠
- [ ] 支持键盘快捷键（如：Ctrl+N 新建 Agent）

### 兼容性
- [ ] 兼容 Codex 官方 Agent TOML 格式
- [ ] 支持 Windows/macOS/Linux 三平台
- [ ] 与现有 remote-control 协议无冲突

---

## ⚠️ 风险与缓解措施

### 风险 1：TOML 格式兼容性问题
**描述**：Codex 未来可能调整 Agent TOML 格式，导致生成的文件不兼容。

**缓解措施**：
- 参考 `references/codex-main/codex-rs/core/src/agent/role.rs` 确保格式一致
- 设计时保留扩展性（使用 `[extra]` 字段存储未知配置）
- 添加版本号字段（`format_version = "1.0"`），为未来迁移做准备

### 风险 2：Remote Control 事件遗漏
**描述**：如果 remote-control 协议未充分暴露 Sub-agent 事件，调用链追踪功能无法实现。

**缓解措施**：
- 提前审查 `src/remote_control_backend/protocol.rs`
- 如事件不存在，需修改 remote-control 代码添加事件（增加 1 周开发时间）
- 备选方案：通过解析日志文件推断调用关系（精度较低）

### 风险 3：GUI 性能问题
**描述**：大量 Agent 或深层调用链导致界面卡顿。

**缓解措施**：
- 使用虚拟列表（Virtual List）只渲染可见项
- 调用链树懒加载子节点（点击展开时才加载）
- 数据库查询添加索引（已在数据库设计中考虑）
- 如仍有性能问题，Week 6 预留性能优化时间

### 风险 4：跨平台 GUI 一致性
**描述**：wxWidgets 在不同平台上表现差异，导致布局错乱。

**缓解措施**：
- 使用相对布局（Sizers）而非绝对坐标
- 在 Windows/macOS/Linux 上分别测试（CI 环境覆盖）
- 参考现有 `src/gui/` 代码的跨平台实践

---

## 🚀 未来迭代方向（Phase 2/3）

### Phase 2：协作与共享（2-3 个月）
- **Agent 市场**：用户可上传/下载社区 Agent
- **版本管理**：Agent 配置历史记录，支持回滚
- **团队协作**：共享 Agent 配置到团队仓库（Git 集成）
- **权限控制**：标记 Agent 为"私有"或"公开"

### Phase 3：高级功能（3-4 个月）
- **Agent 组合**：定义 Agent 工作流（DAG 图）
- **性能分析**：Agent 执行时间分布、Token 成本统计
- **A/B 测试**：对比不同 prompt 的效果
- **自动优化**：根据历史数据推荐 prompt 改进

### 企业版功能（Phase 4+）
- **集中管理**：企业级 Agent 配置中心
- **审计日志**：记录所有 Agent 调用（合规要求）
- **配额管理**：限制用户 Agent 数量和调用频率
- **SSO 集成**：企业身份认证

---

## 📚 附录

### 附录 A：TOML 配置示例

**示例 1：简单 Agent**
```toml
name = "Quick Helper"
description = "快速响应的通用助手"

[model]
name = "gpt-4o-mini"
temperature = 0.7

[system]
prompt = """
你是一个快速响应的助手，专注于简洁高效地解决问题。
"""
```

**示例 2：复杂 Agent（含工具）**
```toml
name = "Research Agent"
description = "深度研究助手，支持联网搜索"

[model]
name = "claude-3.5-sonnet"
temperature = 1.0
max_tokens = 4096
thinking_mode = "detailed"

[system]
prompt = """
你是一个专业的研究助手，擅长深入分析和综合信息。
工作流程：
1. 理解研究主题
2. 使用搜索工具收集资料
3. 分析并整理关键信息
4. 提供结构化的研究报告
"""

[tools]
enabled = ["web_search", "browser", "filesystem"]

[extra]
# 自定义字段（供未来扩展）
max_search_results = 10
output_format = "markdown"
```

---

### 附录 B：GUI 界面 Mockup

**Agent 列表界面**
```
┌─────────────────────────────────────────────────────────────┐
│ CodexHub - Agent Manager                         [_][□][X] │
├─────────────────────────────────────────────────────────────┤
│ 🔍 Search...          [+ New Agent] [📚 Templates]          │
├──────────────────────┬──────────────────────────────────────┤
│ Agent List (12)      │ Agent Details                        │
│                      │                                      │
│ 📝 Code Reviewer     │ Name: Code Reviewer                  │
│    Used 45 times     │ Model: gpt-4o                        │
│    2 hours ago       │ Description: 专业代码审查...         │
│                      │                                      │
│ 📚 Doc Writer        │ System Prompt:                       │
│    Used 23 times     │ 你是一个专业的代码审查员...          │
│    1 day ago         │                                      │
│                      │ Tools: [computer_use] [filesystem]   │
│ 🧪 Test Engineer     │                                      │
│    Used 12 times     │ [Edit] [Clone] [Delete]              │
│    3 days ago        │                                      │
│                      │                                      │
│ ...                  │                                      │
└──────────────────────┴──────────────────────────────────────┘
```

**Agent 编辑器**
```
┌─────────────────────────────────────────────────────────────┐
│ Edit Agent: Code Reviewer                        [_][□][X] │
├──────────────────────────┬──────────────────────────────────┤
│ Configuration            │ TOML Preview                     │
│                          │                                  │
│ Name: [Code Reviewer   ] │ name = "Code Reviewer"           │
│                          │ description = "专业代码审查"     │
│ Description:             │                                  │
│ ┌──────────────────────┐ │ [model]                          │
│ │专业代码审查，发现潜在│ │ name = "gpt-4o"                  │
│ │问题和性能瓶颈        │ │ temperature = 0.8                │
│ └──────────────────────┘ │                                  │
│                          │ [system]                         │
│ Model: [gpt-4o       ▼] │ prompt = """                     │
│                          │ 你是一个专业的代码审查员...      │
│ Temperature: [====•===  ] │ """                             │
│              0.8         │                                  │
│                          │                                  │
│ System Prompt:           │ [tools]                          │
│ ┌──────────────────────┐ │ enabled = ["computer_use"]      │
│ │你是一个专业的代码审查│ │                                  │
│ │员，专注于...         │ │                                  │
│ └──────────────────────┘ │                                  │
│                          │                                  │
│ Tools:                   │                                  │
│ ☑ computer_use           │                                  │
│ ☐ browser                │                                  │
│ ☑ filesystem             │                                  │
│                          │                                  │
│         [Cancel] [Save]  │                                  │
└──────────────────────────┴──────────────────────────────────┘
```

**调用链追踪界面**
```
┌─────────────────────────────────────────────────────────────┐
│ Agent Call Chain - Session #abc123                [_][□][X] │
├─────────────────────────────────────────────────────────────┤
│ Started: 2026-07-03 14:23:15      Duration: 2m 34s          │
├──────────────────────────┬──────────────────────────────────┤
│ Call Tree                │ Session Details                  │
│                          │                                  │
│ ▼ 🟢 Main Agent          │ Agent: Main Agent                │
│   ├─ 🟢 Code Reviewer    │ Status: ✅ Completed             │
│   │   └─ 🟢 Test Engineer│ Started: 14:23:15                │
│   └─ 🔵 Doc Writer (...)  │ Finished: 14:25:49               │
│                          │                                  │
│ Duration: 2m 34s         │ Input Tokens: 1,234              │
│ Total Tokens: 5,678      │ Output Tokens: 4,444             │
│                          │                                  │
│ 🟢 Completed             │ Input Preview:                   │
│ 🔵 Running               │ "Review the code in src/..."     │
│ 🔴 Failed                │                                  │
│                          │ Output Preview:                  │
│                          │ "Found 3 issues: 1. Memory..."   │
└──────────────────────────┴──────────────────────────────────┘
```

---

### 附录 C：参考资料

**Codex 源码关键文件**
- `references/codex-main/codex-rs/core/src/agent/role.rs` - Agent 定义
- `references/codex-main/codex-rs/core/src/tools/handlers/multi_agents_v2/` - Multi-agent 实现
- `references/codex-main/codex-desktop/src/remote-control/` - Remote control 协议

**技术栈文档**
- Rust TOML 库: https://docs.rs/toml/latest/toml/
- wxWidgets Rust 绑定: https://docs.rs/wxrust/latest/
- SQLite Rust 驱动: https://docs.rs/rusqlite/latest/rusqlite/

**设计参考**
- VS Code Extension Manager (UI 设计参考)
- Docker Desktop Agent Manager (调用链可视化参考)
- Postman Collections (模板库交互参考)

---

### 附录 D：关键决策记录

**决策 1：为什么使用 TOML 而非 JSON/YAML？**
- Codex 官方已采用 TOML 格式
- TOML 人类可读性强，注释支持好
- Rust 生态有成熟的 `toml` crate

**决策 2：为什么使用 SQLite 而非文件系统扫描？**
- 使用统计需要持久化存储
- 调用链关系适合关系型数据库
- CodexHub 已使用 SQLite，无需额外依赖

**决策 3：为什么 MVP 不做云端同步？**
- 个人用户优先，企业功能后续迭代
- 减少开发复杂度，避免服务端开发
- 本地优先符合 Codex "离线可用" 理念

**决策 4：为什么调用链追踪基于事件而非日志解析？**
- 事件机制实时性更好（< 1s 延迟）
- 结构化数据更易处理（无需正则解析）
- 如 remote-control 不支持，可回退到日志方案

---

## 📝 总结

本计划旨在通过 **6 周时间**，为 CodexHub 构建一个完整的 Agent Manager MVP 功能，包含：

1. ✅ **Agent 列表管理** - 可视化展示所有本地 Agent
2. ✅ **可视化编辑器** - 表单化创建/编辑 Agent，自动生成 TOML
3. ✅ **官方模板库** - 5 个精品模板，一键复制使用
4. ✅ **调用链追踪** - 实时可视化 Agent 调用树

**核心价值**：
- 降低配置门槛，让非技术用户也能定制 Agent
- 提升透明度，理解 Multi-agent 协作过程
- 提供最佳实践，加速 Agent 开发

**技术亮点**：
- 纯本地方案，无服务端依赖
- 实时追踪，低延迟可视化
- 跨平台 GUI，一致的用户体验

**下一步**：
- 完成本计划审核后，立即启动 Week 1 开发
- 每周进行进度回顾，及时调整计划
- Week 3 和 Week 6 安排关键里程碑演示

---

**文档版本**: v1.0  
**创建日期**: 2026-07-03  
**最后更新**: 2026-07-03  
**作者**: CodexHub Team  
**审核状态**: 待审核

---

## 🔍 补充评估：wxdragon UI 框架可行性分析（修订版）

### ✅ 重要发现：wxdragon 完全支持事件驱动架构！

**之前的担忧**：最初担心 wxdragon 只能用 Timer 轮询，无法实现实时的 Agent 调用链追踪。

**实际情况**：通过深入分析 `src/gui.rs` 代码，发现 CodexHub **已经实现了完整的事件驱动架构**！

---

### 现有事件驱动机制解析

**核心模式**（参考 `src/gui.rs`）：

```rust
// 1. 定义 GUI 消息枚举
enum GuiMessage {
    CodexAction(CodexActionResult),
    ImAction(ImActionResult),
    AiGwAction(AiGwActionResult),
    DashboardUpdate,
}

// 2. 创建跨线程消息通道
let (gui_tx, gui_rx) = tokio_mpsc::unbounded_channel::<GuiMessage>();

// 3. 后台线程发送消息 + 唤醒 GUI
thread::spawn(move || {
    let result = perform_some_task();
    let _ = gui_tx.send(GuiMessage::SomeAction(result));
    wxdragon::wake_up_idle();  // 🔑 立即触发 GUI 刷新
});

// 4. GUI 主线程通过 on_idle 事件处理消息
frame.on_idle(move |event| {
    let mut processed = 0;
    // 批量处理消息（每次最多 20 条）
    while let Ok(message) = gui_rx.try_recv() {
        match message {
            GuiMessage::SomeAction(result) => update_ui(result),
            // ...
        }
        processed += 1;
        if processed >= 20 { break; }
    }
    
    // 如果还有消息，继续请求 idle 事件
    if let WindowEventData::Idle(idle) = event {
        idle.request_more(processed >= 20);
    }
});
```

**关键特性**：
- ✅ **真实时**：`wake_up_idle()` 立即唤醒 GUI 线程（< 16ms，单帧内完成）
- ✅ **零空闲消耗**：无消息时 CPU 使用率 ~0%（不像 Timer 持续轮询）
- ✅ **批量优化**：每次最多处理 20 条消息，避免长时间阻塞 UI 主线程
- ✅ **线程安全**：tokio channel 天然支持多生产者单消费者

---

### Agent 调用链追踪的实现方案（修订）

#### ✅ 推荐方案：复用现有事件驱动架构

**实现步骤**：

**Step 1：扩展 GuiMessage 枚举**
```rust
// src/gui.rs
enum GuiMessage {
    // ... 现有消息类型 ...
    AgentTracking(AgentTrackingEvent),  // 🆕 新增
}

enum AgentTrackingEvent {
    AgentSpawned {
        session_id: String,
        agent_id: String,
        parent_id: Option<String>,
        timestamp: DateTime<Utc>,
    },
    AgentCompleted {
        session_id: String,
        input_tokens: u32,
        output_tokens: u32,
    },
    AgentFailed {
        session_id: String,
        error: String,
    },
}
```

**Step 2：remote_control 后端发送事件**
```rust
// src/remote_control_backend/server_messages.rs
pub async fn observe_app_server_message(
    state: &SharedState,
    connection_epoch: u64,
    client_id: &str,
    stream_id: &str,
    message: &Value,
) {
    // ... 现有逻辑 ...
    
    // 🆕 检测 sub-agent 相关事件
    if let Some(method) = message.get("method").and_then(|v| v.as_str()) {
        match method {
            "spawn_sub_agent" => {
                if let Some(gui_tx) = &state.gui_event_tx {
                    let _ = gui_tx.send(GuiMessage::AgentTracking(
                        AgentTrackingEvent::AgentSpawned {
                            session_id: extract_session_id(message),
                            agent_id: extract_agent_id(message),
                            parent_id: extract_parent_id(message),
                            timestamp: Utc::now(),
                        }
                    ));
                    wxdragon::wake_up_idle();  // 🔑 关键
                }
            }
            "sub_agent_complete" => {
                // 类似处理...
            }
            _ => {}
        }
    }
}
```

**Step 3：GUI 处理 Agent 事件**
```rust
// src/gui.rs - 现有的 on_idle 处理器
frame.on_idle(move |event| {
    // ... 现有消息处理 ...
    
    while let Ok(message) = gui_rx.try_recv() {
        match message {
            // ... 现有分支 ...
            GuiMessage::AgentTracking(event) => {
                // 🆕 更新 Agent Tree UI
                match event {
                    AgentTrackingEvent::AgentSpawned { session_id, agent_id, parent_id, .. } => {
                        handles.agent_tracker.add_node(session_id, agent_id, parent_id);
                    }
                    AgentTrackingEvent::AgentCompleted { session_id, .. } => {
                        handles.agent_tracker.mark_completed(session_id);
                    }
                    AgentTrackingEvent::AgentFailed { session_id, error } => {
                        handles.agent_tracker.mark_failed(session_id, error);
                    }
                }
            }
        }
    }
});
```

**优势分析**：
- ✅ **完全实时**：从 remote_control 事件到 UI 更新 < 100ms
- ✅ **零架构新增**：完全复用现有 `GuiMessage` + `on_idle` 机制
- ✅ **零空闲消耗**：无 Agent 活动时不消耗 CPU
- ✅ **实现简单**：只需在现有代码基础上添加新消息类型
- ✅ **与现有代码一致**：遵循项目已有的事件处理模式

---

### Timer 在项目中的真实用途

**澄清**：项目中确实使用了 `Timer`（`update.rs`、`session_history.rs`），但它们的用途是：

1. **定期拉取外部数据源**
   - 检查 GitHub 版本更新（每次启动 + 手动触发）
   - Dashboard 统计刷新（10 秒间隔，非关键）

2. **超时控制**
   - 下载进度对话框的轮询检查
   - 长时间操作的状态监控

3. **延迟加载**
   - Request Log 懒加载（5 秒间隔，仅在标签页激活时）

**重要区别**：
- Timer 用于**非关键、定期任务**（可以容忍延迟）
- 事件驱动用于**关键交互**（需要立即响应）

**Agent 调用链追踪属于关键交互**，应该也必须使用**事件驱动**实现。

---

### TreeCtrl 可用性验证

**现状**：
- 项目大量使用 `DataViewCtrl`（虚拟列表）
- 未见 `TreeCtrl` 或 `DataViewTreeCtrl` 实际使用案例
- wxdragon 提供了相关 API，但稳定性未知

**验证计划（Week 5 Day 1 上午）**：
```rust
// 测试 DataViewTreeCtrl 基本功能
let tree = DataViewTreeCtrl::builder(&panel).build();
let root = tree.append_container(&None, "Root", -1, -1);
let child = tree.append_item(&root, "Child", -1, -1, None);

// 测试点：
// 1. 创建节点 ✓
// 2. 展开/折叠 ✓
// 3. 点击事件 ✓
// 4. 实时添加子节点 ✓
// 5. 节点状态更新（图标/文本） ✓
```

**备选方案（如验证失败）**：
使用 `DataViewListCtrl` + 缩进字符模拟树形结构：
```
▼ Main Agent (2m 34s)          [状态：完成]
  ├─ Code Reviewer (45s)       [状态：完成]
  │  └─ Test Engineer (12s)    [状态：完成]
  └─ Doc Writer (1m 37s)       [状态：运行中...]
```
- ✅ 100% 可用（已有成功案例）
- ❌ 无法展开/折叠（但可通过过滤器实现）
- ✅ 对 2-3 层调用链完全够用

---

### 🎉 最终结论：完全可行，比预期更好！

**修正后的评估**：

| 功能 | 技术选型 | 风险 | 实现难度 |
|------|---------|------|----------|
| Agent 列表 | DataViewListCtrl | 🟢 低 | 简单 |
| 编辑器 | 标准控件 | 🟢 低 | 简单 |
| 模板库 | 网格布局/列表 | 🟢 低 | 简单 |
| **调用链实时更新** | **事件驱动（< 100ms）** | 🟢 低 | **简单** |
| 树形展示 | DataViewTreeCtrl（备选：缩进列表） | 🟡 中 | 中等 |

**关键改变**：
- ✅ **实时性**：从"准实时（1.5s）"升级为"真实时（< 100ms）"
- ✅ **架构复杂度**：从"需要新架构"降级为"复用现有架构"
- ✅ **CPU 消耗**：从"持续轮询"改为"零空闲消耗"
- ⚠️ **唯一风险**：TreeCtrl 可用性（有保底方案）

**时间表影响**：
- Week 5 Day 1 增加 TreeCtrl 验证（2 小时）
- Week 5 Day 2-3 实现事件集成（比预期更简单，因为复用现有架构）
- 总体时间不变，甚至可能提前完成

---

**最后更新**: 2026-07-03 16:15  
**修订理由**: 纠正了对 wxdragon 事件机制的误解，基于实际代码重新评估

---

