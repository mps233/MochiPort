use serde::Deserialize;

use crate::{
    app_state::SharedState,
    config::TelegramReplyGranularity,
    im_runtime::PendingApproval,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ImLocale {
    #[default]
    ZhCn,
    EnUs,
}

impl ImLocale {
    fn from_code(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "zh" | "zh-cn" | "zh_cn" | "cn" => Some(Self::ZhCn),
            "en" | "en-us" | "en_us" | "us" => Some(Self::EnUs),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ImText {
    locale: ImLocale,
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ImConfigLanguage {
    language: Option<String>,
}

pub(crate) fn im_text_for_state(state: &SharedState) -> ImText {
    let locale = std::fs::read_to_string(&state.config_path)
        .ok()
        .and_then(|raw| toml::from_str::<ImConfigLanguage>(&raw).ok())
        .and_then(|config| config.language)
        .and_then(|language| ImLocale::from_code(&language))
        .unwrap_or_default();
    ImText { locale }
}

impl ImText {
    #[cfg(test)]
    pub(crate) fn zh_cn() -> Self {
        Self {
            locale: ImLocale::ZhCn,
        }
    }

    fn choose(self, zh_cn: &'static str, en_us: &'static str) -> &'static str {
        match self.locale {
            ImLocale::ZhCn => zh_cn,
            ImLocale::EnUs => en_us,
        }
    }

    pub(crate) fn field_line(self, label: &str, value: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("{label}：{value}"),
            ImLocale::EnUs => format!("{label}: {value}"),
        }
    }

    pub(crate) fn no_running_turn(self) -> &'static str {
        self.choose("当前没有运行中的 turn。", "There is no running turn.")
    }

    pub(crate) fn interrupted(self) -> &'static str {
        self.choose("已中断当前任务。", "Interrupted the current task.")
    }

    pub(crate) fn exited(self) -> &'static str {
        self.choose("已退出当前会话。", "Exited the current session.")
    }

    pub(crate) fn unsupported_command(self, command: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => {
                format!("不支持的命令：{command}。当前只支持 /s 中断当前任务、/q 退出当前会话。")
            }
            ImLocale::EnUs => {
                format!(
                    "Unsupported command: {command}. Only /s interrupt and /q exit are supported."
                )
            }
        }
    }

    pub(crate) fn turn_busy_notice(self) -> &'static str {
        self.choose(
            "任务还在进行中，打断 /s，退出会话 /q。",
            "A task is still running. Use /s to interrupt or /q to exit the session.",
        )
    }

    pub(crate) fn turn_busy_attachments_held(self, count: usize) -> String {
        match self.locale {
            ImLocale::ZhCn => {
                format!("任务还在进行中，已暂存 {count} 个附件；任务完成后发送文字说明即可继续。")
            }
            ImLocale::EnUs => format!(
                "A task is still running. {count} attachment(s) were saved; send a text description after it finishes to continue."
            ),
        }
    }

    pub(crate) fn inbound_queue_busy_notice(self) -> &'static str {
        self.choose(
            "消息处理队列已满，这条消息未提交，请稍后重试。",
            "The message queue is full. This message was not submitted; please try again shortly.",
        )
    }

    pub(crate) fn turn_completed_notice(self) -> &'static str {
        self.choose("✅ 已完成", "✅ Completed")
    }

    pub(crate) fn telegram_turn_completed_footer(self) -> &'static str {
        self.choose("已完成", "Completed")
    }

    pub(crate) fn telegram_context_compaction_title(self) -> &'static str {
        self.choose("上下文已自动压缩", "Context compressed automatically")
    }

    pub(crate) fn telegram_context_compaction_credit(self) -> &'static str {
        self.choose("任务继续", "Task continues")
    }

    pub(crate) fn telegram_desktop_user_credit(self) -> &'static str {
        self.telegram_desktop_user_credit_for_os(std::env::consts::OS)
    }

    fn telegram_desktop_user_credit_for_os(self, os: &str) -> &'static str {
        match os {
            "macos" | "darwin" => self.choose("你 · Codex Mac 端", "You · Codex for Mac"),
            "windows" => self.choose("你 · Codex Windows 端", "You · Codex for Windows"),
            "linux" => self.choose("你 · Codex Linux 端", "You · Codex for Linux"),
            _ => self.choose("你 · Codex 电脑端", "You · Codex Desktop"),
        }
    }

    pub(crate) fn turn_failed_notice(self, summary: Option<&str>) -> String {
        let summary = summary
            .map(str::trim)
            .filter(|summary| !summary.is_empty())
            .unwrap_or_else(|| self.choose("系统错误", "System error"));
        match self.locale {
            ImLocale::ZhCn => format!("❌ 任务失败\n\n错误：{summary}"),
            ImLocale::EnUs => format!("❌ Task failed\n\nError: {summary}"),
        }
    }

    pub(crate) fn telegram_command_progress_title(
        self,
        completed: bool,
        failed_task: bool,
        total: usize,
        failed: usize,
        running: usize,
        interrupted: usize,
    ) -> String {
        let title = match (self.locale, completed, failed_task) {
            (ImLocale::ZhCn, true, true) => format!("执行失败 · {total} 步"),
            (ImLocale::EnUs, true, true) => format!("Execution failed · {total} steps"),
            (ImLocale::ZhCn, true, false) if interrupted > 0 => {
                format!("执行结束 · {total} 步")
            }
            (ImLocale::ZhCn, true, false) => format!("执行完成 · {total} 步"),
            (ImLocale::EnUs, true, false) if interrupted > 0 => {
                format!("Execution stopped · {total} steps")
            }
            (ImLocale::EnUs, true, false) => format!("Execution complete · {total} steps"),
            (ImLocale::ZhCn, false, _) => format!("执行中 · {total} 步"),
            (ImLocale::EnUs, false, _) => format!("Running · {total} steps"),
        };
        let failed = match (self.locale, failed) {
            (_, 0) => String::new(),
            (ImLocale::ZhCn, count) => format!(" · {count} 个失败"),
            (ImLocale::EnUs, count) => format!(" · {count} failed"),
        };
        let running = match (self.locale, running, completed) {
            (_, 0, _) | (_, _, true) => String::new(),
            (ImLocale::ZhCn, count, false) => format!(" · {count} 个进行中"),
            (ImLocale::EnUs, count, false) => format!(" · {count} running"),
        };
        let interrupted = match (self.locale, interrupted) {
            (_, 0) => String::new(),
            (ImLocale::ZhCn, count) => format!(" · {count} 个中断"),
            (ImLocale::EnUs, count) => format!(" · {count} interrupted"),
        };
        format!("{title}{failed}{running}{interrupted}")
    }

    pub(crate) fn telegram_command_progress_omitted(self, count: usize) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("… 另外 {count} 个较早步骤"),
            ImLocale::EnUs => format!("… {count} earlier steps"),
        }
    }

    pub(crate) fn telegram_commentary_earlier(self, count: usize) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("较早进展 · {count} 条"),
            ImLocale::EnUs => format!("Earlier updates · {count}"),
        }
    }

    pub(crate) fn telegram_commentary_omitted(self, count: usize) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("… 另外 {count} 条较早进展已省略"),
            ImLocale::EnUs => format!("… {count} earlier updates omitted"),
        }
    }

    pub(crate) fn telegram_command_progress_error_summary(self) -> &'static str {
        self.choose("错误摘要：", "Error summary:")
    }

    pub(crate) fn telegram_task_progress_title(
        self,
        completed: bool,
        failed: bool,
    ) -> &'static str {
        match (self.locale, completed, failed) {
            (ImLocale::ZhCn, true, true) => "任务失败",
            (ImLocale::EnUs, true, true) => "Task failed",
            (ImLocale::ZhCn, true, false) => "任务完成",
            (ImLocale::EnUs, true, false) => "Task complete",
            (ImLocale::ZhCn, false, _) => "任务进行中",
            (ImLocale::EnUs, false, _) => "Task in progress",
        }
    }

    pub(crate) fn telegram_reasoning_heading(self) -> &'static str {
        self.choose("思考摘要", "Reasoning summary")
    }

    pub(crate) fn telegram_plan_heading(self, completed: usize, total: usize) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("计划 · {completed}/{total}"),
            ImLocale::EnUs => format!("Plan · {completed}/{total}"),
        }
    }

    pub(crate) fn telegram_progress_status_label(self, status: &str) -> &'static str {
        match (self.locale, status) {
            (ImLocale::ZhCn, "succeeded") => "成功",
            (ImLocale::EnUs, "succeeded") => "Succeeded",
            (ImLocale::ZhCn, "completed") => "完成",
            (ImLocale::EnUs, "completed") => "Done",
            (ImLocale::ZhCn, "responded") => "已回复",
            (ImLocale::EnUs, "responded") => "Responded",
            (ImLocale::ZhCn, "running" | "in_progress") => "进行中",
            (ImLocale::EnUs, "running" | "in_progress") => "Running",
            (ImLocale::ZhCn, "failed") => "失败",
            (ImLocale::EnUs, "failed") => "Failed",
            (ImLocale::ZhCn, "interrupted") => "已中断",
            (ImLocale::EnUs, "interrupted") => "Interrupted",
            (ImLocale::ZhCn, "pending") => "待处理",
            (ImLocale::EnUs, "pending") => "Pending",
            (ImLocale::ZhCn, _) => "未知",
            (ImLocale::EnUs, _) => "Unknown",
        }
    }

    pub(crate) fn telegram_plan_omitted(self, count: usize) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("… 另外 {count} 个计划步骤"),
            ImLocale::EnUs => format!("… {count} more plan steps"),
        }
    }

    pub(crate) fn telegram_diff_heading(
        self,
        files: usize,
        additions: usize,
        deletions: usize,
    ) -> String {
        match self.locale {
            ImLocale::ZhCn => {
                format!("文件修改 · {files} 个文件 · +{additions} -{deletions}")
            }
            ImLocale::EnUs => {
                format!("File changes · {files} files · +{additions} -{deletions}")
            }
        }
    }

    pub(crate) fn telegram_diff_omitted(self, count: usize) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("… 另外 {count} 个文件"),
            ImLocale::EnUs => format!("… {count} more files"),
        }
    }

    pub(crate) fn telegram_diff_table_file(self) -> &'static str {
        match self.locale {
            ImLocale::ZhCn => "文件",
            ImLocale::EnUs => "File",
        }
    }

    pub(crate) fn telegram_diff_table_additions(self) -> &'static str {
        match self.locale {
            ImLocale::ZhCn => "新增",
            ImLocale::EnUs => "Added",
        }
    }

    pub(crate) fn telegram_diff_table_deletions(self) -> &'static str {
        match self.locale {
            ImLocale::ZhCn => "删除",
            ImLocale::EnUs => "Deleted",
        }
    }

    pub(crate) fn telegram_collab_progress_title(
        self,
        completed: bool,
        total: usize,
        failed: usize,
        running: usize,
        interrupted: usize,
    ) -> String {
        let mut title = match (self.locale, completed, failed + interrupted + running) {
            (ImLocale::ZhCn, false, _) => format!("协作中 · {total} 个子代理"),
            (ImLocale::EnUs, false, _) => format!("Collaborating · {total} agents"),
            (ImLocale::ZhCn, true, 0) => format!("协作完成 · {total} 个子代理"),
            (ImLocale::EnUs, true, 0) => format!("Collaboration complete · {total} agents"),
            (ImLocale::ZhCn, true, _) => format!("协作结束 · {total} 个子代理"),
            (ImLocale::EnUs, true, _) => format!("Collaboration ended · {total} agents"),
        };
        match self.locale {
            ImLocale::ZhCn => {
                if running > 0 && !completed {
                    title.push_str(&format!(" · {running} 个进行中"));
                }
                if failed > 0 {
                    title.push_str(&format!(" · {failed} 个失败"));
                }
                if interrupted > 0 {
                    title.push_str(&format!(" · {interrupted} 个未完成"));
                }
            }
            ImLocale::EnUs => {
                if running > 0 && !completed {
                    title.push_str(&format!(" · {running} running"));
                }
                if failed > 0 {
                    title.push_str(&format!(" · {failed} failed"));
                }
                if interrupted > 0 {
                    title.push_str(&format!(" · {interrupted} unfinished"));
                }
            }
        }
        title
    }

    pub(crate) fn telegram_collab_progress_omitted(self, count: usize) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("… 另外 {count} 个子代理"),
            ImLocale::EnUs => format!("… {count} more agents"),
        }
    }

    pub(crate) fn telegram_collab_duration(self, duration_ms: u128) -> String {
        if duration_ms < 1_000 {
            return match self.locale {
                ImLocale::ZhCn => format!("{duration_ms}毫秒"),
                ImLocale::EnUs => format!("{duration_ms}ms"),
            };
        }
        let seconds = duration_ms / 1_000;
        if seconds < 60 {
            return match self.locale {
                ImLocale::ZhCn => format!("{seconds}秒"),
                ImLocale::EnUs => format!("{seconds}s"),
            };
        }
        let minutes = seconds / 60;
        let remaining_seconds = seconds % 60;
        match self.locale {
            ImLocale::ZhCn => format!("{minutes}分{remaining_seconds}秒"),
            ImLocale::EnUs => format!("{minutes}m {remaining_seconds}s"),
        }
    }

    pub(crate) fn telegram_retry_progress_title(
        self,
        completed: bool,
        failed: bool,
        count: usize,
    ) -> String {
        match (self.locale, completed, failed, count) {
            (ImLocale::ZhCn, true, true, count) => {
                format!("模型请求失败 · 已重试 {count} 次")
            }
            (ImLocale::ZhCn, true, false, count) => {
                format!("模型请求已恢复 · 共重试 {count} 次")
            }
            (ImLocale::ZhCn, false, _, count) => {
                format!("模型请求重试中 · 第 {count} 次")
            }
            (ImLocale::EnUs, true, true, 1) => "Model request failed · retried once".to_string(),
            (ImLocale::EnUs, true, true, count) => {
                format!("Model request failed · retried {count} times")
            }
            (ImLocale::EnUs, true, false, 1) => {
                "Model request recovered · retried once".to_string()
            }
            (ImLocale::EnUs, true, false, count) => {
                format!("Model request recovered · retried {count} times")
            }
            (ImLocale::EnUs, false, _, count) => {
                format!("Retrying model request · attempt {count}")
            }
        }
    }

    pub(crate) fn telegram_retry_progress_summary(self, count: usize) -> String {
        match (self.locale, count) {
            (ImLocale::ZhCn, count) => format!("模型请求重试：{count} 次"),
            (ImLocale::EnUs, 1) => "Model request retried once".to_string(),
            (ImLocale::EnUs, count) => format!("Model request retried {count} times"),
        }
    }

    pub(crate) fn telegram_retry_error_summary(self) -> &'static str {
        self.choose("最近错误：", "Latest error:")
    }

    pub(crate) fn image_description_needed(self) -> &'static str {
        self.choose(
            "图片已收到，请补充说明你希望我做什么。",
            "Image received. Please describe what you would like me to do.",
        )
    }

    pub(crate) fn approval_request_heading(self) -> &'static str {
        self.choose("审批请求", "approval request")
    }

    pub(crate) fn approval_pending_title(self) -> &'static str {
        self.choose("审批待处理", "approval pending")
    }

    pub(crate) fn approval_type_label(self) -> &'static str {
        self.choose("类型", "Type")
    }

    pub(crate) fn approval_details_label(self) -> &'static str {
        self.choose("请求内容", "Request")
    }

    pub(crate) fn approval_actions_label(self) -> &'static str {
        self.choose("可选操作", "Actions")
    }

    pub(crate) fn approval_kind_label(self, kind: &str) -> String {
        let kind = kind.trim();
        match self.locale {
            ImLocale::ZhCn => match kind {
                "command" | "commandExecution" => "命令执行".to_string(),
                "fileChange" => "文件修改".to_string(),
                "permissions" => "权限请求".to_string(),
                "mcpElicitation" => "外部服务请求".to_string(),
                "review" => "变更审查".to_string(),
                _ => kind.to_string(),
            },
            ImLocale::EnUs => match kind {
                "command" | "commandExecution" => "Command execution".to_string(),
                "fileChange" => "File change".to_string(),
                "permissions" => "Permissions".to_string(),
                "mcpElicitation" => "External service".to_string(),
                "review" => "Change review".to_string(),
                _ => kind.to_string(),
            },
        }
    }

    pub(crate) fn approval_decision_label(self, label: &str) -> String {
        let label = label.trim();
        if !matches!(self.locale, ImLocale::ZhCn) {
            return label.to_string();
        }
        if let Some(prefix) = label
            .strip_prefix("Yes, and don't ask again for commands that start with `")
            .and_then(|value| value.strip_suffix('`'))
        {
            return format!("不再询问此前缀命令：`{prefix}`");
        }
        match label {
            "Yes, just this once" | "Yes, allow once" => "仅本次允许".to_string(),
            "Yes, proceed" => "允许执行".to_string(),
            "Yes, allow for this session" => "本会话允许".to_string(),
            "Yes, always allow" => "始终允许".to_string(),
            "Yes, and allow this host for this conversation" => "允许此会话访问该主机".to_string(),
            "Yes, allow these permissions for this turn" => "仅本次允许这些权限".to_string(),
            "Yes, and allow these permissions for this session" => "本会话允许这些权限".to_string(),
            "Yes, and don't ask again for this command in this session" => {
                "本会话不再询问此命令".to_string()
            }
            "Yes, and don't ask again for these files" => "这些文件不再询问".to_string(),
            "No, continue without running it" => "拒绝执行".to_string(),
            "No, and tell Codex what to do differently" => {
                "拒绝，并告诉 Codex 如何调整".to_string()
            }
            "No, continue without granting them" => "拒绝授权".to_string(),
            "No, and block this host in the future" => "拒绝并屏蔽此主机".to_string(),
            "Yes, and allow this host in the future" => "允许并记住此主机".to_string(),
            "No, continue without it" => "拒绝".to_string(),
            "Cancel this request" => "取消请求".to_string(),
            "Allow" => "允许".to_string(),
            "Deny" | "Decline" => "拒绝".to_string(),
            _ => label.to_string(),
        }
    }

    pub(crate) fn telegram_approval_reply_footer(self, hint: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("点击下方按钮，或回复 {hint} 处理。"),
            ImLocale::EnUs => format!("Tap a button below, or reply {hint} to choose."),
        }
    }

    pub(crate) fn telegram_command_menu(self) -> Vec<(&'static str, &'static str)> {
        match self.locale {
            ImLocale::ZhCn => vec![
                ("new", "创建新会话"),
                ("sessions", "恢复历史会话"),
                ("model", "管理模型、推理强度和速度"),
                ("status", "查看连接和任务状态"),
                ("steer", "调整当前任务方向"),
                ("queue", "排队下一条任务"),
                ("stop", "中断当前任务"),
                ("granularity", "切换回复详细程度"),
                ("exit", "退出当前会话"),
                ("help", "查看命令帮助"),
            ],
            ImLocale::EnUs => vec![
                ("new", "Create a new session"),
                ("sessions", "Resume a previous session"),
                ("model", "Manage model, reasoning, and speed"),
                ("status", "Show connection and task status"),
                ("steer", "Steer the current task"),
                ("queue", "Queue the next task"),
                ("stop", "Interrupt the current task"),
                ("granularity", "Switch reply detail level"),
                ("exit", "Exit the current session"),
                ("help", "Show command help"),
            ],
        }
    }

    pub(crate) fn telegram_help(self) -> &'static str {
        self.choose(
            "Telegram 命令\n\n会话\n/new    创建新会话\n/sessions    恢复历史会话\n/model    管理当前会话的模型、推理强度和速度\n/status    查看连接、任务和队列状态\n/exit    退出当前会话\n\n任务\n/steer <内容>    调整当前任务方向\n/queue <内容>    当前任务完成后执行\n/stop    中断当前任务\n/回复 <档位>    切换回复详细程度（摘要 / 标准 / 完整）\n\n任务运行时直接发送普通文字，也会调整当前任务方向。\n\n/help    查看这份帮助\n\n兼容别名：/s = /stop，/q = /exit。审批和会话选择请按消息中的按钮或编号操作；会话设置仅使用按钮。",
            "Telegram commands\n\nSessions\n/new    Create a new session\n/sessions    Resume a previous session\n/model    Manage model, reasoning, and speed for the current session\n/status    Show connection, task, and queue status\n/exit    Exit the current session\n\nTask\n/steer <text>    Steer the current task\n/queue <text>    Run after the current task\n/stop    Interrupt the current task\n/reply <mode>    Switch reply detail (summary / standard / full)\n\nOrdinary text sent while a task is running also steers the current task.\n\n/help    Show this help\n\nCompatibility aliases: /s = /stop, /q = /exit. Use the buttons or numbers shown in approval and session messages; session settings use buttons only.",
        )
    }

    pub(crate) fn telegram_granularity_label(
        self,
        granularity: TelegramReplyGranularity,
    ) -> &'static str {
        match (granularity, self.locale) {
            (TelegramReplyGranularity::Summary, ImLocale::ZhCn) => "摘要回复",
            (TelegramReplyGranularity::Summary, ImLocale::EnUs) => "Summary reply",
            (TelegramReplyGranularity::Standard, ImLocale::ZhCn) => "标准回复",
            (TelegramReplyGranularity::Standard, ImLocale::EnUs) => "Standard reply",
            (TelegramReplyGranularity::Full, ImLocale::ZhCn) => "完整回复",
            (TelegramReplyGranularity::Full, ImLocale::EnUs) => "Full reply",
        }
    }

    pub(crate) fn telegram_granularity_status(self, current: TelegramReplyGranularity) -> String {
        let current_label = self.telegram_granularity_label(current);
        match self.locale {
            ImLocale::ZhCn => format!(
                "当前回复颗粒度：{current_label}。\n\n用 /回复 <档位> 切换：\n• 摘要回复 —— 只发过程文本和最终结果\n• 标准回复 —— 所有信息合并进一条气泡更新\n• 完整回复 —— 所有信息逐条独立发送"
            ),
            ImLocale::EnUs => format!(
                "Current reply granularity: {current_label}.\n\nUse /reply <mode> to switch:\n• Summary reply — process text and final result only\n• Standard reply — everything merged into one updating bubble\n• Full reply — every event as its own message"
            ),
        }
    }

    pub(crate) fn telegram_granularity_set(self, granularity: TelegramReplyGranularity) -> String {
        let label = self.telegram_granularity_label(granularity);
        match self.locale {
            ImLocale::ZhCn => format!("回复颗粒度已切换为 {label}。"),
            ImLocale::EnUs => format!("Reply granularity changed to {label}."),
        }
    }

    pub(crate) fn telegram_granularity_unknown(self) -> &'static str {
        self.choose(
            "未找到回复颗粒度。可选：摘要回复 / 标准回复 / 完整回复。",
            "Reply granularity not found. Options: summary / standard / full.",
        )
    }

    pub(crate) fn telegram_unknown_command(self, command: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("未知命令：{command}\n发送 /help 查看可用命令。"),
            ImLocale::EnUs => {
                format!("Unknown command: {command}\nSend /help to see available commands.")
            }
        }
    }

    pub(crate) fn telegram_turn_busy_notice(self) -> &'static str {
        self.choose(
            "当前任务仍在运行。直接发送文字可调整方向；使用 /queue <内容> 排到后面；/stop 中断任务。",
            "A task is still running. Send text to steer it, use /queue <text> to run it afterward, or /stop to interrupt.",
        )
    }

    pub(crate) fn telegram_steer_accepted(self) -> &'static str {
        self.choose(
            "已将这条消息追加到当前任务，正在按新方向继续。",
            "Added your message to the current task. Continuing with the updated direction.",
        )
    }

    pub(crate) fn telegram_steer_usage(self) -> &'static str {
        self.choose(
            "用法：/steer <新的方向>。任务运行时直接发送普通文字也可以调整方向。",
            "Usage: /steer <new direction>. You can also send ordinary text while a task is running.",
        )
    }

    pub(crate) fn telegram_steer_failed(self) -> &'static str {
        self.choose(
            "当前任务不接受方向调整，请使用 /stop 后重新发送。",
            "The current task does not accept steering. Use /stop and send the message again.",
        )
    }

    pub(crate) fn telegram_queue_usage(self) -> &'static str {
        self.choose(
            "用法：/queue <要排队的内容>。它会在当前任务完成后自动执行。",
            "Usage: /queue <text to queue>. It will run automatically after the current task finishes.",
        )
    }

    pub(crate) fn telegram_queue_added(self, position: usize) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("已排队，这是当前任务后的第 {position} 条消息。"),
            ImLocale::EnUs => format!("Queued. This is item {position} after the current task."),
        }
    }

    pub(crate) fn telegram_queue_full(self, max_count: usize) -> String {
        match self.locale {
            ImLocale::ZhCn => {
                format!("排队已满（最多 {max_count} 条），请等前面的任务完成后再试。")
            }
            ImLocale::EnUs => format!(
                "The queue is full (maximum {max_count}). Try again after an item finishes."
            ),
        }
    }

    pub(crate) fn telegram_queue_requires_running(self) -> &'static str {
        self.choose(
            "当前没有运行中的任务，不需要排队；直接发送内容即可。",
            "There is no running task to queue behind. Send the text directly instead.",
        )
    }

    pub(crate) fn telegram_queue_started(self) -> &'static str {
        self.choose("已开始执行排队消息。", "Started the queued message.")
    }

    pub(crate) fn telegram_queue_start_failed(self, error: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("排队消息连续启动失败，已跳过。\n{error}"),
            ImLocale::EnUs => {
                format!("The queued message could not start and was skipped.\n{error}")
            }
        }
    }

    pub(crate) fn telegram_turn_starting_notice(self) -> &'static str {
        self.choose(
            "任务正在启动，暂时无法调整或退出。请稍后重试，或使用 /queue <内容> 排到后面。",
            "The task is still starting, so it cannot be steered or exited yet. Try again shortly, or use /queue <text>.",
        )
    }

    pub(crate) fn telegram_status(
        self,
        connected: bool,
        thread_id: Option<&str>,
        task: &str,
        queued: usize,
    ) -> String {
        let connection = match (self.locale, connected) {
            (ImLocale::ZhCn, true) => "已连接",
            (ImLocale::ZhCn, false) => "未连接",
            (ImLocale::EnUs, true) => "Connected",
            (ImLocale::EnUs, false) => "Disconnected",
        };
        let (session_label, task_label) = match self.locale {
            ImLocale::ZhCn => ("会话", "任务"),
            ImLocale::EnUs => ("Session", "Task"),
        };
        let session = match (self.locale, thread_id) {
            (ImLocale::ZhCn, Some(thread_id)) => format!("已绑定 `{thread_id}`"),
            (ImLocale::ZhCn, None) => "未绑定".to_string(),
            (ImLocale::EnUs, Some(thread_id)) => format!("Bound `{thread_id}`"),
            (ImLocale::EnUs, None) => "Not bound".to_string(),
        };
        match self.locale {
            ImLocale::ZhCn => format!(
                "Telegram 状态\n\n连接：{connection}\n{session_label}：{session}\n{task_label}：{task}\n排队：{queued} 条"
            ),
            ImLocale::EnUs => format!(
                "Telegram status\n\nConnection: {connection}\n{session_label}: {session}\n{task_label}: {task}\nQueued: {queued}"
            ),
        }
    }

    pub(crate) fn telegram_task_status(
        self,
        running: bool,
        waiting_approval: bool,
    ) -> &'static str {
        match (self.locale, running, waiting_approval) {
            (ImLocale::ZhCn, true, true) => "等待审批",
            (ImLocale::ZhCn, true, false) => "运行中",
            (ImLocale::ZhCn, false, true) => "等待审批",
            (ImLocale::ZhCn, false, false) => "空闲",
            (ImLocale::EnUs, true, true) => "Waiting for approval",
            (ImLocale::EnUs, true, false) => "Running",
            (ImLocale::EnUs, false, true) => "Waiting for approval",
            (ImLocale::EnUs, false, false) => "Idle",
        }
    }

    pub(crate) fn approval_accept_command_label(self) -> &'static str {
        self.choose("同意 / approve", "approve")
    }

    pub(crate) fn approval_decline_command_label(self) -> &'static str {
        self.choose("拒绝 / decline", "decline")
    }

    pub(crate) fn approval_resolved_title(self) -> &'static str {
        self.choose("审批已处理", "approval resolved")
    }

    pub(crate) fn available_decisions_label(self) -> &'static str {
        self.choose("可选决定", "availableDecisions")
    }

    pub(crate) fn approval_reply_hint(self, pending: &PendingApproval) -> String {
        let options = pending
            .decisions
            .iter()
            .enumerate()
            .map(|(index, _)| format!("/{}", index + 1))
            .collect::<Vec<_>>();
        if options.is_empty() {
            match self.locale {
                ImLocale::ZhCn => "`/y` 或 `/n`".to_string(),
                ImLocale::EnUs => "`/y` or `/n`".to_string(),
            }
        } else {
            options.join(self.choose("、", ", "))
        }
    }

    pub(crate) fn approval_reply_footer(self, hint: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("回复 {hint} 处理。"),
            ImLocale::EnUs => format!("Reply {hint} to choose."),
        }
    }

    pub(crate) fn approval_decision_submitted(self) -> &'static str {
        self.choose("审批决定已提交。", "Approval decision submitted.")
    }

    pub(crate) fn approval_decision_submitted_label(self, label: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("已提交：{label}"),
            ImLocale::EnUs => format!("Submitted: {label}"),
        }
    }

    pub(crate) fn approval_selected_label(self, option_index: usize, label: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("已选择 /{option_index}：{label}"),
            ImLocale::EnUs => format!("selected /{option_index}: {label}"),
        }
    }

    pub(crate) fn no_pending_approval(self) -> &'static str {
        self.choose("当前没有待处理审批。", "No pending approval.")
    }

    pub(crate) fn approval_not_current(self) -> &'static str {
        self.choose(
            "这个审批请求已经不是当前待处理项。",
            "This approval is no longer current.",
        )
    }

    pub(crate) fn invalid_approval_reply(self, hint: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("审批回复无效，请回复 {hint}。"),
            ImLocale::EnUs => format!("Invalid approval reply. Reply {hint}."),
        }
    }

    pub(crate) fn unsupported_approval_callback(self) -> &'static str {
        self.choose("不支持的审批回调。", "Unsupported approval callback.")
    }

    pub(crate) fn telegram_creation_action_only(self) -> &'static str {
        self.choose(
            "这个创建操作只支持 Telegram 按钮流程。",
            "This creation action is only supported in the Telegram button flow.",
        )
    }

    pub(crate) fn remote_not_connected(self) -> &'static str {
        self.choose(
            "Codex remote-control 还没有连接。请在项目目录运行 codex，确认它已经通过 remote-control 连接到 MochiPort。",
            "Codex remote-control is not connected yet. Run codex in the project directory and make sure it is connected to MochiPort through remote-control.",
        )
    }

    pub(crate) fn inbound_expired(self) -> &'static str {
        self.choose(
            "这条消息是在上一轮任务期间收到的，已跳过。请重新发送最新指令。",
            "This message arrived during the previous task and was skipped. Send the latest instruction again.",
        )
    }

    pub(crate) fn app_message_failed(self, error: &dyn std::fmt::Display) -> String {
        match self.locale {
            ImLocale::ZhCn => {
                format!(
                    "Codex 没有接收这条消息：{error}\n\n当前 IM 会话绑定的 Codex 端点可能已经退出或断开。请回复 /q 退出当前会话，然后重新新建会话或恢复历史会话。"
                )
            }
            ImLocale::EnUs => {
                format!(
                    "Codex did not accept this message: {error}\n\nThe Codex endpoint bound to this IM session may have exited or disconnected. Reply /q to exit the current session, then create a new session or resume a historical session."
                )
            }
        }
    }

    pub(crate) fn creating_new_thread(self) -> &'static str {
        self.choose(
            "正在创建新的 Codex 会话...",
            "Creating a new Codex session...",
        )
    }

    pub(crate) fn created_new_session_title(self) -> &'static str {
        self.choose("已创建新会话", "New session created")
    }

    pub(crate) fn creating_session_title(self) -> &'static str {
        self.choose("正在创建会话", "Creating Session")
    }

    pub(crate) fn created_new_session_body(self, thread_id: &str, summary: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => {
                format!("已接入新 thread `{thread_id}`。\n\n{summary}\n\n现在可以直接发送消息。")
            }
            ImLocale::EnUs => {
                format!(
                    "Attached to new thread `{thread_id}`.\n\n{summary}\n\nYou can now send messages directly."
                )
            }
        }
    }

    pub(crate) fn subscribing_thread(self, thread_id: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("正在订阅 thread `{thread_id}` 的后续事件..."),
            ImLocale::EnUs => format!("Subscribing to thread `{thread_id}` events..."),
        }
    }

    pub(crate) fn subscribed_session_title(self) -> &'static str {
        self.choose("已订阅会话", "Session attached")
    }

    pub(crate) fn subscribing_session_title(self) -> &'static str {
        self.choose("正在接入会话", "Attaching Session")
    }

    pub(crate) fn subscribed_session_body(
        self,
        thread_id: &str,
        title: &str,
        cwd: &str,
        status: &str,
    ) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("已接入 thread `{thread_id}`。\n\n{title}\n{cwd}\n{status}"),
            ImLocale::EnUs => {
                format!("Attached to thread `{thread_id}`.\n\n{title}\n{cwd}\n{status}")
            }
        }
    }

    pub(crate) fn resumed_session_title(self) -> &'static str {
        self.choose("已接入历史会话", "History session attached")
    }

    pub(crate) fn resumed_session_body(self, title: &str, cwd: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("{title}\n{cwd}\n\n现在可以直接发送消息。"),
            ImLocale::EnUs => format!("{title}\n{cwd}\n\nYou can now send messages directly."),
        }
    }

    pub(crate) fn create_settings_menu_suffix(self) -> &'static str {
        self.choose(
            "\n\n1. 修改目录\n2. 修改模型\n3. 修改推理强度\n4. 修改权限\n5. 创建会话\n6. 恢复历史会话\n\n回复数字选择。也可以回复 y 创建，n 取消。",
            "\n\n1. Change directory\n2. Change model\n3. Change reasoning effort\n4. Change permissions\n5. Create session\n6. Restore history session\n\nReply with a number. You can also reply y to create or n to cancel.",
        )
    }

    pub(crate) fn invalid_create_settings_reply(self) -> &'static str {
        self.choose(
            "请回复 1~6，或回复 y 创建、n 取消。",
            "Reply 1~6, or reply y to create, n to cancel.",
        )
    }

    pub(crate) fn create_cancelled(self) -> &'static str {
        self.choose("已取消创建会话。", "Session creation cancelled.")
    }

    pub(crate) fn create_option_unavailable(self) -> &'static str {
        self.choose(
            "这个创建选项不可用，请重新打开创建设置。",
            "This creation option is unavailable. Open creation settings again.",
        )
    }

    pub(crate) fn create_option_expired(self) -> &'static str {
        self.choose(
            "这个创建选项已经失效，请重新打开创建设置。",
            "This creation option has expired. Open creation settings again.",
        )
    }

    pub(crate) fn invalid_option_index(self) -> &'static str {
        self.choose(
            "这个选项序号不可用，请按当前列表里的数字选择。",
            "This option number is unavailable. Choose a number from the current list.",
        )
    }

    pub(crate) fn invalid_create_form(self, error: &dyn std::fmt::Display) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("新建会话参数不正确：{error}"),
            ImLocale::EnUs => format!("Invalid new session settings: {error}"),
        }
    }

    pub(crate) fn custom_cwd_prompt_wechat(self) -> &'static str {
        self.choose(
            "请发送项目目录的绝对路径。目录不存在时，创建会话时会自动创建。\n\n回复 n 取消。",
            "Send the absolute project directory path. If it does not exist, it will be created when the session starts.\n\nReply n to cancel.",
        )
    }

    pub(crate) fn custom_cwd_prompt_telegram(self) -> &'static str {
        self.choose(
            "请发送项目目录的绝对路径。目录不存在时，创建 thread 时会自动创建。\n\n发送 /cancel 取消。",
            "Send the absolute project directory path. If it does not exist, it will be created when the thread starts.\n\nSend /cancel to cancel.",
        )
    }

    pub(crate) fn cwd_must_be_absolute_wechat(self) -> &'static str {
        self.choose(
            "项目目录需要是绝对路径。请重新发送绝对路径，或回复 n 取消。",
            "The project directory must be an absolute path. Send an absolute path again, or reply n to cancel.",
        )
    }

    pub(crate) fn cwd_must_be_absolute_telegram(self) -> &'static str {
        self.choose(
            "项目目录需要是绝对路径。请重新发送一个绝对路径，或发送 /cancel 取消。",
            "The project directory must be an absolute path. Send an absolute path again, or send /cancel to cancel.",
        )
    }

    pub(crate) fn custom_cwd_label(self) -> &'static str {
        self.choose("自定义或新建目录", "Custom or new directory")
    }

    pub(crate) fn custom_cwd_summary(self) -> &'static str {
        self.choose(
            "选择后发送绝对路径。目录不存在时会自动创建。",
            "Select this, then send an absolute path. Missing directories are created automatically.",
        )
    }

    pub(crate) fn create_choice_wechat(self) -> &'static str {
        self.choose(
            "当前微信会话还没有接入 Codex 会话。\n\n1. 新建会话\n2. 恢复历史会话或接入当前 Codex 活跃会话\n\n回复 1 或 2。",
            "This WeChat chat is not attached to a Codex session yet.\n\n1. Create new session\n2. Restore a history session or attach to the current active Codex session\n\nReply 1 or 2.",
        )
    }

    pub(crate) fn create_choice_telegram(self) -> &'static str {
        self.choose(
            "当前 Telegram 会话还没有接入 Codex thread。\n请选择创建新会话，或恢复历史会话或接入当前 Codex 活跃会话。",
            "This Telegram chat is not attached to a Codex thread yet.\nCreate a new session, or restore a history session or attach to the current active Codex session.",
        )
    }

    pub(crate) fn invalid_route_choice_wechat(self) -> &'static str {
        self.choose(
            "请回复 1 新建会话，或回复 2 恢复历史会话或接入当前 Codex 活跃会话。",
            "Reply 1 to create a session, or 2 to restore a history session or attach to the current active Codex session.",
        )
    }

    pub(crate) fn no_history_create_hint_wechat(self) -> &'static str {
        self.choose(
            "当前没有可恢复的历史会话。\n\n回复 1 创建新会话。",
            "There are no restorable history sessions.\n\nReply 1 to create a new session.",
        )
    }

    pub(crate) fn list_load_failed(self) -> &'static str {
        self.choose(
            "会话列表加载失败：Codex App 暂时没有响应，请稍后重试。",
            "Failed to load the session list: Codex App is not responding right now. Try again later.",
        )
    }

    pub(crate) fn list_load_failed_title(self) -> &'static str {
        self.choose("会话列表加载失败", "Failed to Load Sessions")
    }

    pub(crate) fn first_page(self) -> &'static str {
        self.choose("已经是第一页。", "Already on the first page.")
    }

    pub(crate) fn last_page(self) -> &'static str {
        self.choose("已经是最后一页。", "Already on the last page.")
    }

    pub(crate) fn invalid_thread_index(self) -> &'static str {
        self.choose(
            "这个序号不在当前会话列表里，请按列表里的数字选择。",
            "This number is not in the current session list. Choose a number from the list.",
        )
    }

    pub(crate) fn invalid_thread_index_restart(self) -> &'static str {
        self.choose(
            "这个会话序号不可用，请重新发送一条消息触发会话选择。",
            "This session number is unavailable. Send a new message to reopen session selection.",
        )
    }

    pub(crate) fn thread_operation_expired(self) -> &'static str {
        self.choose(
            "这个 thread 操作已经失效，请重新发送一条消息触发会话选择。",
            "This thread operation has expired. Send a new message to reopen session selection.",
        )
    }

    pub(crate) fn thread_choice_card_expired(self) -> &'static str {
        self.choose(
            "这张 thread 选择卡片已经失效，请重新发送消息。",
            "This thread selection card has expired. Send a new message.",
        )
    }

    pub(crate) fn thread_choice_not_current(self) -> &'static str {
        self.choose(
            "这个 thread 选择不属于当前会话。",
            "This thread selection does not belong to the current chat.",
        )
    }

    pub(crate) fn thread_list_not_current(self) -> &'static str {
        self.choose(
            "这个 thread 列表不属于当前会话。",
            "This thread list does not belong to the current chat.",
        )
    }

    pub(crate) fn thread_selection_expired(self) -> &'static str {
        self.choose(
            "这个 thread 选择已经失效，请重新打开列表。",
            "This thread selection has expired. Open the list again.",
        )
    }

    pub(crate) fn stale_thread_unbound(self) -> &'static str {
        self.choose(
            "当前绑定的 Codex thread 已失效，已解除绑定。",
            "The attached Codex thread is no longer valid and has been detached.",
        )
    }

    pub(crate) fn unsupported_thread_action(self, action: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("不支持的 thread 操作：{action}"),
            ImLocale::EnUs => format!("Unsupported thread action: {action}"),
        }
    }

    pub(crate) fn thread_list_title_wechat(self) -> &'static str {
        self.choose("恢复历史会话", "Restore History Session")
    }

    pub(crate) fn thread_list_title_telegram(self) -> &'static str {
        self.choose("恢复历史会话", "Restore history session")
    }

    pub(crate) fn telegram_thread_settings_title(self) -> &'static str {
        self.choose("会话设置", "Session settings")
    }

    pub(crate) fn telegram_thread_settings_effective_heading(self) -> &'static str {
        self.choose("已生效", "Effective")
    }

    pub(crate) fn telegram_thread_settings_draft_heading(self) -> &'static str {
        self.choose("待应用", "Pending")
    }

    pub(crate) fn telegram_thread_settings_unchanged(self) -> &'static str {
        self.choose("未修改", "Unchanged")
    }

    pub(crate) fn telegram_thread_settings_unknown(self) -> &'static str {
        self.choose(
            "保持当前值（尚未观察到）",
            "Keep current value (not observed yet)",
        )
    }

    pub(crate) fn telegram_thread_settings_standard_speed(self) -> &'static str {
        self.choose("标准", "Standard")
    }

    pub(crate) fn telegram_thread_settings_fast_speed(self) -> &'static str {
        self.choose("快速", "Fast")
    }

    pub(crate) fn telegram_thread_settings_model_button(self) -> &'static str {
        self.choose("模型", "Model")
    }

    pub(crate) fn telegram_thread_settings_effort_button(self) -> &'static str {
        self.choose("推理强度", "Reasoning")
    }

    pub(crate) fn telegram_thread_settings_speed_button(self) -> &'static str {
        self.choose("速度", "Speed")
    }

    pub(crate) fn telegram_thread_settings_apply_button(self) -> &'static str {
        self.choose("应用", "Apply")
    }

    pub(crate) fn telegram_thread_settings_cancel_button(self) -> &'static str {
        self.choose("取消", "Cancel")
    }

    pub(crate) fn telegram_thread_settings_back_button(self) -> &'static str {
        self.choose("返回", "Back")
    }

    pub(crate) fn telegram_thread_settings_choose_model(self) -> &'static str {
        self.choose("选择模型", "Choose model")
    }

    pub(crate) fn telegram_thread_settings_choose_effort(self) -> &'static str {
        self.choose("选择推理强度", "Choose reasoning effort")
    }

    pub(crate) fn telegram_thread_settings_choose_speed(self) -> &'static str {
        self.choose("选择速度", "Choose speed")
    }

    pub(crate) fn telegram_thread_settings_fast_unavailable(self) -> &'static str {
        self.choose(
            "所选模型不支持快速模式。",
            "The selected model does not support Fast mode.",
        )
    }

    pub(crate) fn telegram_thread_settings_effort_unavailable(self) -> &'static str {
        self.choose(
            "所选模型没有可选的推理强度。",
            "The selected model has no selectable reasoning efforts.",
        )
    }

    pub(crate) fn telegram_thread_settings_confirm_title(self) -> &'static str {
        self.choose("需要确认兼容性", "Compatibility confirmation")
    }

    pub(crate) fn telegram_thread_settings_confirm_body(
        self,
        reset_effort: bool,
        reset_speed: bool,
    ) -> String {
        let mut items = Vec::new();
        if reset_effort {
            items.push(self.choose(
                "推理强度将恢复为目标模型默认值",
                "Reasoning will use the target model default",
            ));
        }
        if reset_speed {
            items.push(self.choose("速度将恢复为标准", "Speed will return to Standard"));
        }
        match self.locale {
            ImLocale::ZhCn => format!("{}。确认后才会提交设置。", items.join("；")),
            ImLocale::EnUs => format!("{}. Confirm to submit the settings.", items.join("; ")),
        }
    }

    pub(crate) fn telegram_thread_settings_confirm_button(self) -> &'static str {
        self.choose("确认并应用", "Confirm and apply")
    }

    pub(crate) fn telegram_thread_settings_stale(self) -> &'static str {
        self.choose(
            "Codex 客户端已更新本草稿涉及的设置，草稿已失效。请重新发送 /model。",
            "Codex updated a setting in this draft. The draft expired; send /model again.",
        )
    }

    pub(crate) fn telegram_thread_settings_applied(self) -> &'static str {
        self.choose(
            "设置已应用，Codex 客户端会同步显示。",
            "Settings applied. Codex will show the update.",
        )
    }

    pub(crate) fn telegram_thread_settings_submitted_unconfirmed(self) -> &'static str {
        self.choose(
            "设置已提交，尚未确认。",
            "Settings submitted but not yet confirmed.",
        )
    }

    pub(crate) fn telegram_thread_settings_no_changes(self) -> &'static str {
        self.choose(
            "没有待应用的设置。",
            "There are no pending settings to apply.",
        )
    }

    pub(crate) fn telegram_thread_settings_cancelled(self) -> &'static str {
        self.choose("已取消设置更改。", "Settings changes cancelled.")
    }

    pub(crate) fn telegram_thread_settings_apply_failed(
        self,
        error: &dyn std::fmt::Display,
    ) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("设置应用失败：{error}"),
            ImLocale::EnUs => format!("Failed to apply settings: {error}"),
        }
    }

    pub(crate) fn telegram_model_switch_requires_thread(self) -> &'static str {
        self.choose(
            "当前话题还没有绑定 Codex 会话。请先使用 /new 或 /sessions 接入会话。",
            "This topic is not attached to a Codex session. Use /new or /sessions first.",
        )
    }

    pub(crate) fn telegram_model_list_failed(self, error: &dyn std::fmt::Display) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("模型列表加载失败：{error}"),
            ImLocale::EnUs => format!("Failed to load the model list: {error}"),
        }
    }

    pub(crate) fn telegram_model_list_empty(self) -> &'static str {
        self.choose(
            "Codex App 当前没有返回可切换的模型。",
            "Codex App did not return any switchable models.",
        )
    }

    pub(crate) fn telegram_model_switch_expired(self) -> &'static str {
        self.choose(
            "这个模型菜单已经失效，请重新发送 /model。",
            "This model menu has expired. Send /model again.",
        )
    }

    pub(crate) fn telegram_model_switch_not_current(self) -> &'static str {
        self.choose(
            "这个模型菜单不属于当前会话，或会话绑定已经变化。请重新发送 /model。",
            "This model menu no longer belongs to the current session. Send /model again.",
        )
    }

    pub(crate) fn thread_list_title_feishu(self) -> &'static str {
        self.choose("选择 Codex 会话", "Select Codex Session")
    }

    pub(crate) fn thread_list_body_telegram(self, provider: Option<&str>) -> String {
        let mut body = self
            .choose(
                "请选择一个会话接入后续事件。",
                "Choose a session to attach future events.",
            )
            .to_string();
        if let Some(provider) = provider {
            body.push_str(&match self.locale {
                ImLocale::ZhCn => format!("\n已按当前 Codex App provider `{provider}` 过滤。"),
                ImLocale::EnUs => {
                    format!("\nFiltered by current Codex App provider `{provider}`.")
                }
            });
        }
        body
    }

    pub(crate) fn thread_list_body_feishu(self, provider: Option<&str>) -> String {
        let mut body = self
            .choose(
                "当前飞书会话还没有订阅任何 Codex thread。请选择一个会话接入后续事件。",
                "This Feishu chat is not subscribed to any Codex thread yet. Choose a session to attach future events.",
            )
            .to_string();
        if let Some(provider) = provider {
            body.push_str(&match self.locale {
                ImLocale::ZhCn => {
                    format!("\n\n<font color='grey'>已按当前 Codex App provider `{provider}` 过滤。</font>")
                }
                ImLocale::EnUs => {
                    format!("\n\n<font color='grey'>Filtered by current Codex App provider `{provider}`.</font>")
                }
            });
        }
        body
    }

    pub(crate) fn provider_filter_line(self, provider: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("已按当前 Codex App provider `{provider}` 过滤。"),
            ImLocale::EnUs => format!("Filtered by current Codex App provider `{provider}`."),
        }
    }

    pub(crate) fn reply_choose_session_range(self, count: usize) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("回复 1~{count} 选择会话"),
            ImLocale::EnUs => format!("Reply 1~{count} to choose a session"),
        }
    }

    pub(crate) fn reply_choose_range(self, count: usize) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("回复 1~{count} 选择"),
            ImLocale::EnUs => format!("Reply 1~{count} to choose"),
        }
    }

    pub(crate) fn page_hint(self, page: usize, actions: &[String]) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("第 {} 页 · {}", page.max(1), actions.join("，")),
            ImLocale::EnUs => format!("Page {} · {}", page.max(1), actions.join(", ")),
        }
    }

    pub(crate) fn page_click_hint(self, page: usize, count: usize) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("第 {} 页 · 点击 /1 ~ /{} 选择", page.max(1), count),
            ImLocale::EnUs => format!("Page {} · Click /1 ~ /{} to choose", page.max(1), count),
        }
    }

    pub(crate) fn page_label(self, page: usize) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("第 {} 页", page.max(1)),
            ImLocale::EnUs => format!("Page {}", page.max(1)),
        }
    }

    pub(crate) fn no_options(self) -> &'static str {
        self.choose("当前没有可选项。", "No options are available.")
    }

    pub(crate) fn no_restorable_history(self) -> &'static str {
        self.choose(
            "当前没有可恢复的历史会话。",
            "There are no restorable history sessions.",
        )
    }

    pub(crate) fn no_restorable_history_workspace(self) -> &'static str {
        self.choose(
            "当前工作区下没有可恢复的历史会话。",
            "There are no restorable history sessions in this workspace.",
        )
    }

    pub(crate) fn prev_action_markdown(self) -> &'static str {
        self.choose("**p** 上一页", "**p** Previous")
    }

    pub(crate) fn next_action_markdown(self) -> &'static str {
        self.choose("**n** 下一页", "**n** Next")
    }

    pub(crate) fn back_create_settings_markdown(self) -> &'static str {
        self.choose("**0** 返回设置", "**0** Back to settings")
    }

    pub(crate) fn previous_page_button(self) -> &'static str {
        self.choose("上一页", "Previous")
    }

    pub(crate) fn next_page_button(self) -> &'static str {
        self.choose("下一页", "Next")
    }

    pub(crate) fn create_new_session_button(self) -> &'static str {
        self.choose("创建新会话", "Create new session")
    }

    pub(crate) fn restore_history_button(self) -> &'static str {
        self.choose("恢复历史会话", "Restore history session")
    }

    pub(crate) fn select_session_action(self) -> &'static str {
        self.choose("选择会话", "Select session")
    }

    pub(crate) fn more_actions(self) -> &'static str {
        self.choose("更多操作", "More actions")
    }

    pub(crate) fn directory_button(self) -> &'static str {
        self.choose("目录", "Directory")
    }

    pub(crate) fn model_button(self) -> &'static str {
        self.choose("模型", "Model")
    }

    pub(crate) fn effort_button(self) -> &'static str {
        self.choose("推理强度", "Reasoning")
    }

    pub(crate) fn permission_button(self) -> &'static str {
        self.choose("权限", "Permissions")
    }

    pub(crate) fn create_button(self) -> &'static str {
        self.choose("创建", "Create")
    }

    pub(crate) fn back_to_create_settings_button(self) -> &'static str {
        self.choose("返回创建设置", "Back to settings")
    }

    pub(crate) fn create_choice_title_feishu(self) -> &'static str {
        self.choose("未绑定会话", "No Session Attached")
    }

    pub(crate) fn create_choice_body_feishu(self) -> &'static str {
        self.choose(
            "当前飞书会话还没有接入 Codex thread。请选择新建会话，或恢复历史会话或接入当前 Codex 活跃会话。",
            "This Feishu chat is not attached to a Codex thread yet. Create a new session, or restore a history session or attach to the current active Codex session.",
        )
    }

    pub(crate) fn create_choice_body_wecom(self) -> &'static str {
        self.choose(
            "当前企业微信会话还没有接入 Codex thread。请选择新建会话，或恢复历史会话。",
            "This WeCom chat is not attached to a Codex thread yet. Create a new session or restore a history session.",
        )
    }

    pub(crate) fn create_choice_tip_feishu(self) -> &'static str {
        self.choose(
            "提示：回复 `/q` 可退出当前会话，回复 `/s` 可中断当前任务。",
            "Tip: reply `/q` to exit the current session, or `/s` to interrupt the current task.",
        )
    }

    pub(crate) fn create_new_description_feishu(self) -> &'static str {
        self.choose(
            "创建一个新的 Codex thread，并接入后续消息。",
            "Create a new Codex thread and attach future messages.",
        )
    }

    pub(crate) fn restore_history_description_feishu(self) -> &'static str {
        self.choose(
            "查看 Codex App 当前可恢复的历史 thread 列表。",
            "View restorable Codex App history threads.",
        )
    }

    pub(crate) fn selected_prefix_feishu(self, label: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("已选择 · {label}"),
            ImLocale::EnUs => format!("Selected · {label}"),
        }
    }

    pub(crate) fn create_settings_card_intro(self) -> &'static str {
        self.choose(
            "选择这次新会话的属性。Provider 固定使用 Codex App 当前配置。",
            "Choose settings for this new session. Provider uses the current Codex App configuration.",
        )
    }

    pub(crate) fn create_settings_card_title(self) -> &'static str {
        self.choose("新建会话设置", "New Session Settings")
    }

    pub(crate) fn cwd_section(self) -> &'static str {
        self.choose("项目目录", "Project Directory")
    }

    pub(crate) fn cwd_select_placeholder(self) -> &'static str {
        self.choose("选择已有项目目录", "Select an existing project directory")
    }

    pub(crate) fn cwd_custom_placeholder(self) -> &'static str {
        self.choose(
            "可选：填绝对路径；不存在会自动创建",
            "Optional: absolute path; created automatically if missing",
        )
    }

    pub(crate) fn model_section(self) -> &'static str {
        self.choose("模型", "Model")
    }

    pub(crate) fn model_select_placeholder(self) -> &'static str {
        self.choose("选择模型", "Select model")
    }

    pub(crate) fn effort_section(self) -> &'static str {
        self.choose("推理强度", "Reasoning Effort")
    }

    pub(crate) fn effort_select_placeholder(self) -> &'static str {
        self.choose("选择推理强度", "Select reasoning effort")
    }

    pub(crate) fn permission_section(self) -> &'static str {
        self.choose("权限", "Permissions")
    }

    pub(crate) fn permission_select_placeholder(self) -> &'static str {
        self.choose("选择权限", "Select permissions")
    }

    pub(crate) fn confirm_create_button(self) -> &'static str {
        self.choose("确认创建", "Create")
    }

    pub(crate) fn create_default_button(self) -> &'static str {
        self.choose("使用默认配置创建", "Create with defaults")
    }

    pub(crate) fn create_default_description(self) -> &'static str {
        self.choose(
            "使用当前 provider，不指定目录、模型和推理强度。",
            "Use the current provider without overriding directory, model, or reasoning effort.",
        )
    }

    pub(crate) fn back_button(self) -> &'static str {
        self.choose("返回", "Back")
    }

    pub(crate) fn back_description(self) -> &'static str {
        self.choose(
            "回到新建/恢复会话选择。",
            "Return to create/restore session selection.",
        )
    }

    pub(crate) fn remote_label(self) -> &'static str {
        self.choose("远端", "Remote")
    }

    pub(crate) fn cwd_label(self) -> &'static str {
        self.choose("目录", "Directory")
    }

    pub(crate) fn provider_label(self) -> &'static str {
        "Provider"
    }

    pub(crate) fn model_label(self) -> &'static str {
        self.choose("模型", "Model")
    }

    pub(crate) fn effort_label(self) -> &'static str {
        self.choose("推理强度", "Reasoning effort")
    }

    pub(crate) fn permission_label_title(self) -> &'static str {
        self.choose("权限", "Permissions")
    }

    pub(crate) fn not_connected(self) -> &'static str {
        self.choose("未连接", "Not connected")
    }

    pub(crate) fn codex_app_default_value(self) -> &'static str {
        self.choose("使用 Codex App 默认值", "Use Codex App default")
    }

    pub(crate) fn create_thread_heading(self) -> &'static str {
        self.choose("创建新 Codex thread", "Create New Codex Thread")
    }

    pub(crate) fn current_settings_heading(self) -> &'static str {
        self.choose("当前设置：", "Current settings:")
    }

    pub(crate) fn create_help_footer(self) -> &'static str {
        self.choose(
            "请选择要修改的设置，确认后创建。",
            "Select settings to change, then create the session.",
        )
    }

    pub(crate) fn current_prefix(self, value: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("当前：{value}"),
            ImLocale::EnUs => format!("Current: {value}"),
        }
    }

    pub(crate) fn current_default_prefix(self, value: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("当前默认：{value}"),
            ImLocale::EnUs => format!("Current default: {value}"),
        }
    }

    pub(crate) fn selected_prefix(self, label: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("已选：{label}"),
            ImLocale::EnUs => format!("Selected: {label}"),
        }
    }

    pub(crate) fn selected(self) -> &'static str {
        self.choose("已选", "Selected")
    }

    pub(crate) fn use_current_provider(self) -> &'static str {
        self.choose(
            "使用 Codex App 当前 provider",
            "Use Codex App current provider",
        )
    }

    pub(crate) fn use_default_cwd(self) -> &'static str {
        self.choose("使用 Codex App 默认目录", "Use Codex App default directory")
    }

    pub(crate) fn waiting_custom_cwd(self) -> &'static str {
        self.choose("等待输入自定义目录", "Waiting for custom directory")
    }

    pub(crate) fn use_default_cwd_with_path(self, cwd: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("使用 Codex App 默认目录（{cwd}）"),
            ImLocale::EnUs => format!("Use Codex App default directory ({cwd})"),
        }
    }

    pub(crate) fn use_current_model(self) -> &'static str {
        self.choose("使用当前模型", "Use current model")
    }

    pub(crate) fn use_current_model_with_value(self, model: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("使用当前模型（{model}）"),
            ImLocale::EnUs => format!("Use current model ({model})"),
        }
    }

    pub(crate) fn do_not_override_model(self) -> &'static str {
        self.choose(
            "不覆盖模型，由 Codex App 决定",
            "Do not override model; Codex App decides",
        )
    }

    pub(crate) fn use_model_default_effort(self) -> &'static str {
        self.choose("使用模型默认推理强度", "Use model default reasoning effort")
    }

    pub(crate) fn use_default_effort_with_value(self, effort: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("使用默认推理强度（{effort}）"),
            ImLocale::EnUs => format!("Use default reasoning effort ({effort})"),
        }
    }

    pub(crate) fn do_not_override_effort(self) -> &'static str {
        self.choose(
            "不覆盖推理强度，由模型决定",
            "Do not override reasoning effort; the model decides",
        )
    }

    pub(crate) fn use_current_permission(self) -> &'static str {
        self.choose(
            "使用 Codex App 当前权限",
            "Use Codex App current permissions",
        )
    }

    pub(crate) fn use_current_permission_with_value(self, permission: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("使用 Codex App 当前权限（{permission}）"),
            ImLocale::EnUs => format!("Use Codex App current permissions ({permission})"),
        }
    }

    pub(crate) fn do_not_override_permission(self) -> &'static str {
        self.choose("不覆盖权限配置", "Do not override permission settings")
    }

    pub(crate) fn select_project_dir_title(self) -> &'static str {
        self.choose("选择项目目录", "Select Project Directory")
    }

    pub(crate) fn select_model_title(self) -> &'static str {
        self.choose("选择模型", "Select Model")
    }

    pub(crate) fn select_effort_title(self) -> &'static str {
        self.choose("选择推理强度", "Select Reasoning Effort")
    }

    pub(crate) fn select_permission_title(self) -> &'static str {
        self.choose("选择权限", "Select Permissions")
    }

    pub(crate) fn default_permission_label(self) -> &'static str {
        self.choose("默认权限", "Default permissions")
    }

    pub(crate) fn auto_review_label(self) -> &'static str {
        self.choose("自动审查", "Auto review")
    }

    pub(crate) fn full_access_label(self) -> &'static str {
        self.choose("完全访问权限", "Full access")
    }

    pub(crate) fn read_only_label(self) -> &'static str {
        self.choose("只读", "Read only")
    }

    pub(crate) fn custom_label(self) -> &'static str {
        self.choose("自定义", "Custom")
    }

    pub(crate) fn default_permission_summary(self) -> &'static str {
        self.choose(
            "适合常规项目，需要时由用户确认。",
            "Good for normal projects; asks the user when needed.",
        )
    }

    pub(crate) fn auto_review_summary(self) -> &'static str {
        self.choose(
            "需要审批时优先交给自动审查。",
            "Use automatic review first when approval is needed.",
        )
    }

    pub(crate) fn full_access_summary(self) -> &'static str {
        self.choose(
            "不再请求确认，允许完整本机访问。",
            "Do not ask for confirmation; allow full local access.",
        )
    }

    pub(crate) fn reasoning_effort_label(self, effort: &str) -> String {
        match (self.locale, effort.trim()) {
            (ImLocale::ZhCn, "none") => "无 (none)".to_string(),
            (ImLocale::ZhCn, "minimal") => "极低 (minimal)".to_string(),
            (ImLocale::ZhCn, "low") => "低 (low)".to_string(),
            (ImLocale::ZhCn, "medium") => "中 (medium)".to_string(),
            (ImLocale::ZhCn, "high") => "高 (high)".to_string(),
            (ImLocale::ZhCn, "xhigh") => "超高 (xhigh)".to_string(),
            (ImLocale::EnUs, "none") => "None (none)".to_string(),
            (ImLocale::EnUs, "minimal") => "Minimal (minimal)".to_string(),
            (ImLocale::EnUs, "low") => "Low (low)".to_string(),
            (ImLocale::EnUs, "medium") => "Medium (medium)".to_string(),
            (ImLocale::EnUs, "high") => "High (high)".to_string(),
            (ImLocale::EnUs, "xhigh") => "Extra high (xhigh)".to_string(),
            (_, other) => other.to_string(),
        }
    }

    pub(crate) fn permission_label(self, permission: &str) -> String {
        match permission.trim() {
            "workspace_user" | "default" | "default_permissions" | "auto" => {
                self.default_permission_label().to_string()
            }
            "auto_review" | "guardian-approvals" | "guardian_approvals" => {
                self.auto_review_label().to_string()
            }
            "full_access" | "full-access" => self.full_access_label().to_string(),
            "read_only" | "read-only" => self.read_only_label().to_string(),
            other => other.to_string(),
        }
    }

    pub(crate) fn thread_title_fallback(self, thread_id: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("会话 {thread_id}"),
            ImLocale::EnUs => format!("Session {thread_id}"),
        }
    }

    pub(crate) fn untitled_session(self) -> &'static str {
        self.choose("未命名会话", "Untitled session")
    }

    pub(crate) fn thread_cwd_summary(self, cwd: Option<String>) -> String {
        match (self.locale, cwd) {
            (ImLocale::ZhCn, Some(cwd)) => format!("目录：`{cwd}`"),
            (ImLocale::EnUs, Some(cwd)) => format!("Directory: `{cwd}`"),
            (ImLocale::ZhCn, None) => "目录未知".to_string(),
            (ImLocale::EnUs, None) => "Directory unknown".to_string(),
        }
    }

    pub(crate) fn thread_status(self, status: &str) -> String {
        match (self.locale, status) {
            (ImLocale::ZhCn, "active") => "运行中".to_string(),
            (ImLocale::ZhCn, "idle") => "空闲".to_string(),
            (ImLocale::ZhCn, "notLoaded") => "未加载".to_string(),
            (ImLocale::ZhCn, "systemError") => "系统错误".to_string(),
            (ImLocale::EnUs, "active") => "Running".to_string(),
            (ImLocale::EnUs, "idle") => "Idle".to_string(),
            (ImLocale::EnUs, "notLoaded") => "Not loaded".to_string(),
            (ImLocale::EnUs, "systemError") => "System error".to_string(),
            (_, other) => other.to_string(),
        }
    }

    pub(crate) fn route_state_current(self) -> &'static str {
        self.choose("当前会话", "Current session")
    }

    pub(crate) fn route_state_loaded(self) -> &'static str {
        self.choose("已加载，可接入", "Loaded, attachable")
    }

    pub(crate) fn route_state_history(self) -> &'static str {
        self.choose("历史会话，可接入", "History session, attachable")
    }

    pub(crate) fn current_short(self) -> &'static str {
        self.choose("当前", "Current")
    }

    pub(crate) fn loaded_short(self) -> &'static str {
        self.choose("已加载", "Loaded")
    }

    pub(crate) fn project_header(self, name: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("项目：{name}"),
            ImLocale::EnUs => format!("Project: {name}"),
        }
    }

    pub(crate) fn unknown_project_header(self) -> &'static str {
        self.choose("项目：未知目录", "Project: Unknown directory")
    }
}

#[cfg(test)]
mod tests {
    use super::{ImLocale, ImText};

    #[test]
    fn telegram_context_compaction_copy_is_localized() {
        let zh = ImText {
            locale: ImLocale::ZhCn,
        };
        let en = ImText {
            locale: ImLocale::EnUs,
        };

        assert_eq!(zh.telegram_context_compaction_title(), "上下文已自动压缩");
        assert_eq!(zh.telegram_context_compaction_credit(), "任务继续");
        assert_eq!(
            en.telegram_context_compaction_title(),
            "Context compressed automatically"
        );
        assert_eq!(en.telegram_context_compaction_credit(), "Task continues");
    }

    #[test]
    fn telegram_desktop_user_credit_names_each_host_platform() {
        let zh = ImText {
            locale: ImLocale::ZhCn,
        };
        let en = ImText {
            locale: ImLocale::EnUs,
        };

        assert_eq!(
            zh.telegram_desktop_user_credit_for_os("macos"),
            "你 · Codex Mac 端"
        );
        assert_eq!(
            zh.telegram_desktop_user_credit_for_os("windows"),
            "你 · Codex Windows 端"
        );
        assert_eq!(
            zh.telegram_desktop_user_credit_for_os("linux"),
            "你 · Codex Linux 端"
        );
        assert_eq!(
            en.telegram_desktop_user_credit_for_os("macos"),
            "You · Codex for Mac"
        );
        assert_eq!(
            en.telegram_desktop_user_credit_for_os("unknown"),
            "You · Codex Desktop"
        );
    }

    #[test]
    fn telegram_desktop_user_credit_uses_the_build_host_platform() {
        let text = ImText {
            locale: ImLocale::ZhCn,
        };
        assert_eq!(
            text.telegram_desktop_user_credit(),
            text.telegram_desktop_user_credit_for_os(std::env::consts::OS)
        );
    }

    #[test]
    fn telegram_reserves_completed_copy_for_the_final_reply_footer() {
        let zh = ImText {
            locale: ImLocale::ZhCn,
        };
        let en = ImText {
            locale: ImLocale::EnUs,
        };

        assert_eq!(zh.telegram_progress_status_label("succeeded"), "成功");
        assert_eq!(zh.telegram_progress_status_label("completed"), "完成");
        assert_eq!(zh.telegram_turn_completed_footer(), "已完成");
        assert_eq!(en.telegram_progress_status_label("succeeded"), "Succeeded");
        assert_eq!(en.telegram_progress_status_label("completed"), "Done");
        assert_eq!(en.telegram_turn_completed_footer(), "Completed");
    }

    #[test]
    fn telegram_command_menu_uses_standard_names_and_status_is_localized() {
        let zh = ImText {
            locale: ImLocale::ZhCn,
        };
        let en = ImText {
            locale: ImLocale::EnUs,
        };
        let zh_commands = zh
            .telegram_command_menu()
            .into_iter()
            .map(|(command, _)| command)
            .collect::<Vec<_>>();
        assert_eq!(
            zh_commands,
            vec![
                "new", "sessions", "model", "status", "steer", "queue", "stop", "granularity",
                "exit", "help"
            ]
        );
        assert!(zh.telegram_help().contains("/stop"));
        assert!(zh.telegram_help().contains("/queue"));
        assert!(zh.telegram_steer_accepted().contains("追加"));
        assert_eq!(
            zh.approval_decision_label("Yes, allow these permissions for this turn"),
            "仅本次允许这些权限"
        );
        assert!(zh.telegram_turn_starting_notice().contains("正在启动"));
        assert!(
            zh.telegram_status(true, Some("thread-1"), "运行中", 0)
                .contains("已连接")
        );
        assert!(
            en.telegram_status(false, None, "Idle", 0)
                .contains("Disconnected")
        );
        assert_eq!(zh.telegram_task_status(true, true), "等待审批");
    }
}
