//! Extracted verbatim from `progress.rs`; no behavior changes.

use super::*;

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::{
        im::core::i18n::ImText,
        im::runtime::{
            TelegramCollabProgressEntry, TelegramCollabProgressSnapshot,
            TelegramCollabProgressStatus, TelegramCommandProgressEntry,
            TelegramCommandProgressEntryKind, TelegramCommandProgressSnapshot,
            TelegramCommandProgressStatus, TelegramCommentaryEntry,
        },
    };

    use super::interleaved_tool_indices;

    use super::{
        TELEGRAM_COMMAND_PROGRESS_MAX_CHARS, TELEGRAM_DIFF_TABLE_PATH_CHARS,
        TELEGRAM_REASONING_RENDER_CHARS, commentary_entry_blocks, completed_entry,
        diff_file_display_name, diff_summary_from_diff, file_change_diff_summary,
        mcp_completed_entry, mcp_running_entry, parse_plan_update, reasoning_summary_from_item,
        render_command_progress, render_task_progress, rich_command_entry_blocks, running_entry,
    };

    use crate::im::runtime::{
        TelegramDiffFileSummary, TelegramDiffSummary, TelegramPlanStep, TelegramPlanStepStatus,
        TelegramWebSearchProgressEntry,
    };

    #[test]
    fn parses_array_commands_and_keeps_failure_tail() {
        let item = json!({
            "commandActions": [{"command": ["cargo", "test", "--all"]}],
            "exitCode": 2,
            "durationMs": 1_250,
            "aggregatedOutput": "one\ntwo\nthree\nfour\nfive\nsix\nseven"
        });

        let entry = completed_entry("item-1", &item);

        assert_eq!(entry.command, "cargo test --all");
        assert_eq!(entry.status, TelegramCommandProgressStatus::Failed);
        assert_eq!(entry.duration_ms, Some(1_250));
        let output = entry.failure_output.expect("failure output");
        assert!(!output.contains("one"));
        assert!(output.contains("two"));
        assert!(output.contains("seven"));
    }

    #[test]
    fn mcp_entries_use_a_compact_tool_label() {
        let item = json!({
            "type": "mcpToolCall",
            "server": "browser",
            "tool": "screenshot",
            "arguments": {"title": "获取页面截图"},
            "status": "completed",
            "durationMs": 850
        });

        let running = mcp_running_entry("mcp-1", &item);
        assert_eq!(running.kind, TelegramCommandProgressEntryKind::McpTool);
        assert_eq!(running.command, "browser.screenshot · 获取页面截图");
        assert_eq!(running.status, TelegramCommandProgressStatus::Running);

        let completed = mcp_completed_entry("mcp-1", &item);
        assert_eq!(completed.status, TelegramCommandProgressStatus::Succeeded);
        assert_eq!(completed.duration_ms, Some(850));
    }

    #[test]
    fn render_mcp_entry_without_a_shell_code_block() {
        let rendered = render_command_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 1,
                message_id: None,
                entries: vec![mcp_running_entry(
                    "mcp-1",
                    &json!({
                        "type": "mcpToolCall",
                        "server": "browser",
                        "tool": "screenshot",
                        "arguments": {"title": "获取页面截图"}
                    }),
                )],
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                commentary: Vec::new(),
                commentary_dropped_entries: 0,
                collab: None,
                final_reply: None,
                elapsed_ms: None,
                completed: false,
                failed: false,
            },
            ImText::zh_cn(),
        );

        assert!(rendered.contains("进行中 · MCP\nbrowser.screenshot · 获取页面截图"));
        assert!(!rendered.contains("```shell"));
    }

    #[test]
    fn mcp_result_error_is_rendered_as_a_bounded_failure_summary() {
        let entry = mcp_completed_entry(
            "mcp-1",
            &json!({
                "type": "mcpToolCall",
                "server": "browser",
                "tool": "navigate",
                "status": "completed",
                "result": {
                    "isError": true,
                    "content": [{"type": "text", "text": "503 Service Unavailable"}]
                }
            }),
        );
        assert_eq!(entry.status, TelegramCommandProgressStatus::Failed);
        assert_eq!(
            entry.failure_output.as_deref(),
            Some("503 Service Unavailable")
        );

        let rendered = render_command_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 2,
                message_id: Some("42".to_string()),
                entries: vec![entry],
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                commentary: Vec::new(),
                commentary_dropped_entries: 0,
                collab: None,
                final_reply: None,
                elapsed_ms: None,
                completed: false,
                failed: false,
            },
            ImText::zh_cn(),
        );
        assert!(rendered.contains("失败 · MCP\nbrowser.navigate"));
        assert!(rendered.contains("503 Service Unavailable"));
    }

    #[test]
    fn mcp_protocol_error_prefers_the_message_field() {
        let entry = mcp_completed_entry(
            "mcp-1",
            &json!({
                "type": "mcpToolCall",
                "server": "browser",
                "tool": "navigate",
                "status": "failed",
                "error": {
                    "message": "MCP server unavailable",
                    "code": -32000
                }
            }),
        );

        assert_eq!(entry.status, TelegramCommandProgressStatus::Failed);
        assert_eq!(
            entry.failure_output.as_deref(),
            Some("MCP server unavailable")
        );
    }

    #[test]
    fn rich_progress_folds_every_mcp_step_into_the_tools_summary() {
        let entries = (0..8)
            .map(|index| {
                mcp_completed_entry(
                    &format!("mcp-{index}"),
                    &json!({
                        "type": "mcpToolCall",
                        "server": "browser",
                        "tool": format!("tool-{index}"),
                        "status": "completed"
                    }),
                )
            })
            .collect();
        let rendered = render_task_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 8,
                message_id: Some("42".to_string()),
                entries,
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                commentary: Vec::new(),
                commentary_dropped_entries: 0,
                collab: None,
                final_reply: None,
                elapsed_ms: None,
                completed: true,
                failed: false,
            },
            ImText::zh_cn(),
        );

        let tools = rendered
            .blocks
            .iter()
            .find(|block| block["type"] == "details")
            .expect("tools summary panel");
        assert_eq!(tools["summary"], "工具摘要（8）");
        let panel = tools["blocks"].as_array().expect("tools panel blocks");
        let visible_commands = panel
            .iter()
            .filter(|block| block["type"] == "pre")
            .collect::<Vec<_>>();
        assert_eq!(visible_commands.len(), 8);
        assert!(
            visible_commands
                .iter()
                .all(|block| block["language"] == "text")
        );
        assert!(
            !panel
                .iter()
                .any(|block| block["text"] == "… 另外 5 个较早步骤"),
            "steps inside the history budget stay available in the folded panel"
        );

        let encoded = serde_json::to_string(&rendered.blocks).expect("rich progress");
        assert!(encoded.contains("browser.tool-7"));
        assert!(encoded.contains("browser.tool-0"));
        assert!(rendered.fallback_markdown.contains("另外 5 个较早步骤"));
        assert!(!rendered.fallback_markdown.contains("browser.tool-2"));
        assert!(rendered.fallback_markdown.contains("browser.tool-7"));
    }

    #[test]
    fn render_prioritizes_running_and_failed_steps() {
        let mut entries = (0..8)
            .map(|index| {
                completed_entry(
                    &format!("item-{index}"),
                    &json!({"command": format!("command {index}"), "exitCode": 0}),
                )
            })
            .collect::<Vec<_>>();
        entries[1] = completed_entry(
            "item-1",
            &json!({"command": "failed early", "exitCode": 1, "aggregatedOutput": "boom"}),
        );
        entries[2] = running_entry("item-2", &json!({"command": "still running"}));
        let rendered = render_command_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 9,
                message_id: Some("42".to_string()),
                entries,
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                commentary: Vec::new(),
                commentary_dropped_entries: 0,
                collab: None,
                final_reply: None,
                elapsed_ms: None,
                completed: false,
                failed: false,
            },
            ImText::zh_cn(),
        );

        assert!(rendered.contains("执行中"));
        assert!(rendered.contains("failed early"));
        assert!(rendered.contains("still running"));
        assert!(rendered.contains("另外 5 个较早步骤"));
        assert!(!rendered.contains("command 0"));
    }

    #[test]
    fn render_is_bounded_to_one_telegram_message() {
        let output = "x".repeat(20_000);
        let entries = (0..128)
            .map(|index| {
                completed_entry(
                    &format!("item-{index}"),
                    &json!({
                        "command": format!("{} {index}", "c".repeat(2_000)),
                        "exitCode": 1,
                        "aggregatedOutput": output
                    }),
                )
            })
            .collect();
        let rendered = render_command_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 128,
                message_id: None,
                entries,
                dropped_entries: 25,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                commentary: Vec::new(),
                commentary_dropped_entries: 0,
                collab: None,
                final_reply: None,
                elapsed_ms: None,
                completed: true,
                failed: false,
            },
            ImText::zh_cn(),
        );

        assert!(rendered.chars().count() <= TELEGRAM_COMMAND_PROGRESS_MAX_CHARS);
        assert!(rendered.contains("执行完成"));
        assert!(rendered.contains("另外 150 个较早步骤"));
    }

    #[test]
    fn render_bounds_three_visible_failures_with_a_max_retry_error() {
        let entries = (0..5)
            .map(|index| {
                completed_entry(
                    &format!("item-{index}"),
                    &json!({
                        "command": format!("{} command-tail-{index}", "命".repeat(2_000)),
                        "exitCode": 1,
                        "aggregatedOutput": format!(
                            "{} failure-tail-{index}",
                            "误".repeat(2_000)
                        )
                    }),
                )
            })
            .collect();
        let retry_error = "错".repeat(600);
        let rendered = render_command_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 10,
                message_id: Some("42".to_string()),
                entries,
                dropped_entries: 0,
                retry_count: 5,
                retry_error: Some(retry_error.clone()),
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                commentary: Vec::new(),
                commentary_dropped_entries: 0,
                collab: None,
                final_reply: None,
                elapsed_ms: None,
                completed: true,
                failed: true,
            },
            ImText::zh_cn(),
        );

        assert!(rendered.chars().count() <= TELEGRAM_COMMAND_PROGRESS_MAX_CHARS);
        assert!(rendered.contains(&retry_error));
        for index in 2..5 {
            assert!(rendered.contains(&format!("command-tail-{index}")));
            assert!(rendered.contains(&format!("failure-tail-{index}")));
        }
        assert!(!rendered.contains("command-tail-0"));
        assert!(!rendered.contains("command-tail-1"));
    }

    #[test]
    fn render_distinguishes_an_interrupted_terminal_step() {
        let mut entry = running_entry("item", &json!({"command": "cargo test"}));
        entry.status = TelegramCommandProgressStatus::Interrupted;
        let rendered = render_command_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 2,
                message_id: Some("42".to_string()),
                entries: vec![entry],
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                commentary: Vec::new(),
                commentary_dropped_entries: 0,
                collab: None,
                final_reply: None,
                elapsed_ms: None,
                completed: true,
                failed: false,
            },
            ImText::zh_cn(),
        );

        assert!(rendered.contains("执行结束 · 1 步 · 1 个中断"));
        assert!(rendered.contains("已中断\n```shell\ncargo test\n```"));
        assert!(!rendered.contains("进行中"));
    }

    #[test]
    fn render_marks_a_failed_turn_even_when_commands_succeeded() {
        let rendered = render_command_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 3,
                message_id: Some("42".to_string()),
                entries: vec![completed_entry(
                    "item",
                    &json!({"command": "cargo test", "exitCode": 0}),
                )],
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                commentary: Vec::new(),
                commentary_dropped_entries: 0,
                collab: None,
                final_reply: None,
                elapsed_ms: None,
                completed: true,
                failed: true,
            },
            ImText::zh_cn(),
        );

        assert!(rendered.contains("执行失败 · 1 步"));
        assert!(!rendered.contains("执行完成"));
    }

    #[test]
    fn render_retry_only_progress_and_terminal_state() {
        let mut snapshot = TelegramCommandProgressSnapshot {
            turn_id: "turn".to_string(),
            revision: 2,
            message_id: Some("42".to_string()),
            entries: Vec::new(),
            dropped_entries: 0,
            retry_count: 2,
            retry_error: Some("503 Service Unavailable".to_string()),
            reasoning_summary: None,
            plan_explanation: None,
            plan: Vec::new(),
            diff_summary: None,
            web_searches: Vec::new(),
            dropped_web_searches: 0,
            commentary: Vec::new(),
            commentary_dropped_entries: 0,
            collab: None,
            final_reply: None,
            elapsed_ms: None,
            completed: false,
            failed: false,
        };

        let running = render_command_progress(&snapshot, ImText::zh_cn());
        assert!(running.contains("模型请求重试中 · 第 2 次"));
        assert!(running.contains("```text\n503 Service Unavailable\n```"));

        snapshot.completed = true;
        snapshot.failed = true;
        let failed = render_command_progress(&snapshot, ImText::zh_cn());
        assert!(failed.contains("模型请求失败 · 已重试 2 次"));
    }

    #[test]
    fn render_uses_native_shell_blocks_for_commands() {
        let rendered = render_command_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 1,
                message_id: None,
                entries: vec![running_entry(
                    "item",
                    &json!({"command": "printf 12345678"}),
                )],
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                commentary: Vec::new(),
                commentary_dropped_entries: 0,
                collab: None,
                final_reply: None,
                elapsed_ms: None,
                completed: false,
                failed: false,
            },
            ImText::zh_cn(),
        );

        assert!(rendered.contains("```shell\nprintf 12345678\n```"));
        assert!(!rendered.contains("`printf 12345678`"));
    }

    #[test]
    fn parse_plan_update_maps_protocol_statuses_and_skips_blank_steps() {
        let params = json!({
            "threadId": "thread",
            "turnId": "turn",
            "explanation": "  inspect, implement, verify  ",
            "plan": [
                {"step": " inspect ", "status": "completed"},
                {"step": "implement", "status": "inProgress"},
                {"step": "verify", "status": "pending"},
                {"step": " ", "status": "completed"}
            ]
        });

        let (explanation, plan) = parse_plan_update(&params);

        assert_eq!(explanation.as_deref(), Some("inspect, implement, verify"));
        assert_eq!(plan.len(), 3);
        assert_eq!(plan[0].status, TelegramPlanStepStatus::Completed);
        assert_eq!(plan[1].status, TelegramPlanStepStatus::InProgress);
        assert_eq!(plan[2].status, TelegramPlanStepStatus::Pending);
        assert_eq!(plan[0].step, "inspect");
    }

    #[test]
    fn reasoning_summary_parser_uses_only_the_latest_summary_part() {
        let item = json!({
            "type": "reasoning",
            "summary": ["first", "first", {"text": "second"}],
            "content": ["second", {"text": "third"}]
        });

        assert_eq!(
            reasoning_summary_from_item(&item).as_deref(),
            Some("second")
        );
    }

    #[test]
    fn diff_summary_counts_only_unified_hunk_lines() {
        let diff = "diff --git a/src/a.rs b/src/a.rs\nindex 1..2 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,2 +1,3 @@\n-old\n+new\n+another\n+metadata-like-line\ndiff --git a/src/b.rs b/src/b.rs\n--- a/src/b.rs\n+++ b/src/b.rs\n@@ -1,2 +1,1 @@\n-removed\n-removed-again\n kept\n";

        let summary = diff_summary_from_diff(diff).expect("diff summary");

        assert_eq!(summary.file_count, 2);
        assert_eq!(summary.additions, 3);
        assert_eq!(summary.deletions, 3);
        assert_eq!(summary.paths, vec!["src/a.rs", "src/b.rs"]);
        assert_eq!(
            summary.files,
            vec![
                TelegramDiffFileSummary {
                    path: "src/a.rs".to_string(),
                    additions: 3,
                    deletions: 1,
                },
                TelegramDiffFileSummary {
                    path: "src/b.rs".to_string(),
                    additions: 0,
                    deletions: 2,
                },
            ]
        );
        assert_eq!(summary.omitted_paths, 0);
    }

    #[test]
    fn file_change_summary_includes_move_path_and_unified_stats() {
        let item = json!({
            "type": "fileChange",
            "changes": [{
                "path": "src/old.rs",
                "kind": {"type": "update", "move_path": "src/new.rs"},
                "diff": "--- a/src/old.rs\n+++ b/src/new.rs\n@@ -1 +1 @@\n-old\n+new\n"
            }]
        });

        let summary = file_change_diff_summary(&item).expect("file change summary");

        assert_eq!(summary.file_count, 1);
        assert_eq!(summary.additions, 1);
        assert_eq!(summary.deletions, 1);
        assert_eq!(summary.paths, vec!["src/old.rs -> src/new.rs"]);
        assert_eq!(
            summary.files,
            vec![TelegramDiffFileSummary {
                path: "src/old.rs -> src/new.rs".to_string(),
                additions: 1,
                deletions: 1,
            }]
        );
    }

    #[test]
    fn file_change_summary_counts_raw_add_and_delete_content() {
        let item = json!({
            "type": "fileChange",
            "changes": [
                {
                    "path": "src/new.rs",
                    "kind": {"type": "add"},
                    "diff": "fn main() {}\n\n"
                },
                {
                    "path": "src/old.rs",
                    "kind": {"type": "delete"},
                    "diff": "fn old() {}\nremoved\n"
                }
            ]
        });

        let summary = file_change_diff_summary(&item).expect("file change summary");

        assert_eq!(summary.additions, 2);
        assert_eq!(summary.deletions, 2);
        assert_eq!(
            summary.files,
            vec![
                TelegramDiffFileSummary {
                    path: "src/new.rs".to_string(),
                    additions: 2,
                    deletions: 0,
                },
                TelegramDiffFileSummary {
                    path: "src/old.rs".to_string(),
                    additions: 0,
                    deletions: 2,
                },
            ]
        );
    }

    #[test]
    fn diff_summary_splits_unified_headers_without_git_markers() {
        let diff = "--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-old\n+new\n--- a/src/b.rs\n+++ b/src/b.rs\n@@ -1 +1,2 @@\n-kept\n+kept\n+added\n";

        let summary = diff_summary_from_diff(diff).expect("diff summary");

        assert_eq!(summary.file_count, 2);
        assert_eq!(summary.additions, 3);
        assert_eq!(summary.deletions, 2);
        assert_eq!(summary.files.len(), 2);
        assert_eq!(summary.files[1].path, "src/b.rs");
        assert_eq!(summary.files[1].additions, 2);
        assert_eq!(summary.files[1].deletions, 1);
    }

    #[test]
    fn render_includes_reasoning_plan_and_diff_as_compact_text() {
        let rendered = render_command_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 4,
                message_id: None,
                entries: Vec::new(),
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: Some("first thought\n\nsecond thought".to_string()),
                plan_explanation: Some("work in order".to_string()),
                plan: vec![
                    TelegramPlanStep {
                        step: "inspect".to_string(),
                        status: TelegramPlanStepStatus::Completed,
                    },
                    TelegramPlanStep {
                        step: "verify".to_string(),
                        status: TelegramPlanStepStatus::InProgress,
                    },
                ],
                diff_summary: Some(TelegramDiffSummary {
                    file_count: 1,
                    additions: 2,
                    deletions: 1,
                    files: vec![TelegramDiffFileSummary {
                        path: "src/main.rs".to_string(),
                        additions: 2,
                        deletions: 1,
                    }],
                    paths: vec!["src/main.rs".to_string()],
                    omitted_paths: 0,
                }),
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                commentary: Vec::new(),
                commentary_dropped_entries: 0,
                collab: None,
                final_reply: None,
                elapsed_ms: None,
                completed: true,
                failed: false,
            },
            ImText::zh_cn(),
        );

        assert!(rendered.chars().count() <= TELEGRAM_COMMAND_PROGRESS_MAX_CHARS);
        // 思考摘要已去掉标题，只保留那一行内容。
        assert!(!rendered.contains("思考摘要"));
        assert!(rendered.contains("first thought second thought"));
        assert!(rendered.contains("计划 · 1/2"));
        assert!(rendered.contains("文件修改 · 1 个文件 · +2 -1"));
        assert!(rendered.contains("• main.rs"));
        assert!(!rendered.contains("• src/main.rs"));
        assert!(!rendered.contains("```diff"));
    }

    /// 回归：「思考摘要」必须**只占一行**。
    ///
    /// 症状（用户实测）：Codex 的 reasoning summary 是累积式的一大段（带项目符号、
    /// 多行），卡片顶部把整段铺出来，把「执行完成」气泡撑得很高。
    ///
    /// 原因：原先走的是保留换行的 `compact_text(.., 720)`：
    ///   1. 换行被保留 → 多行原样输出；
    ///   2. 720 的阈值又高于多数摘要长度 → 连截断都不触发，等于全量显示。
    ///
    /// 断言渲染结果里思考摘要**恰好一行**，且超长时以 `…` 结尾。
    #[test]
    fn reasoning_summary_renders_as_a_single_truncated_line() {
        // 复刻真实形态：多行、带项目符号、总长远超单行预算。
        let reasoning = "The folder is open. Now write the final message.\n\nFinal message:\n- 找到了，已整理到 Downloads\n- 01 英文矢量 SVG\n- 02 英文 960px\n- 03 中文版 logo\nKeep concise. Done.";
        let snapshot = TelegramCommandProgressSnapshot {
            turn_id: "turn".to_string(),
            revision: 1,
            message_id: None,
            entries: Vec::new(),
            dropped_entries: 0,
            retry_count: 0,
            retry_error: None,
            reasoning_summary: Some(reasoning.to_string()),
            plan_explanation: None,
            plan: Vec::new(),
            diff_summary: None,
            web_searches: Vec::new(),
            dropped_web_searches: 0,
            commentary: Vec::new(),
            commentary_dropped_entries: 0,
            collab: None,
            final_reply: None,
            elapsed_ms: None,
            completed: true,
            failed: false,
        };
        let rendered = render_command_progress(&snapshot, ImText::zh_cn());

        // 已去掉「思考摘要」标题：正文行直接以开头文案起始。
        assert!(!rendered.contains("思考摘要"), "不应再渲染「思考摘要」标题");
        let line = rendered
            .lines()
            .find(|line| line.starts_with("The folder is open"))
            .expect("应渲染出思考摘要正文行")
            .to_string();

        // 1) 必须是一行：正文里不能再夹带原始换行/项目符号。
        assert!(!line.contains('\n'), "思考摘要不应包含换行，实际: {line:?}");
        // 2) 超长时必须截断并加省略号，且不超过预算。
        assert!(
            line.ends_with('…'),
            "超长思考摘要应以 … 结尾，实际: {line:?}"
        );
        assert!(
            line.chars().count() <= TELEGRAM_REASONING_RENDER_CHARS,
            "思考摘要超过单行预算: {} > {}",
            line.chars().count(),
            TELEGRAM_REASONING_RENDER_CHARS
        );
        // 3) 原始的多行内容不得整段出现。
        assert!(
            !rendered.contains("Keep concise. Done."),
            "思考摘要不应把整段累积文本都渲染出来"
        );
    }

    /// 回归：最终回复里的 markdown **表格**与**标题**不能露出原文。
    ///
    /// 症状（用户实测）：气泡里直接显示
    /// `|时间|最后那条消息|记录显示|` / `|---|---|---|` 和 `## 结论`。
    ///
    /// 原因：内嵌走 `commentary_entry_blocks` 后，它只处理段落/代码/列表/链接，
    /// 没有表格与标题分支，于是这些行被并进普通段落原样输出。旧路径由 Telegram
    /// 原生渲染 markdown，所以这是内嵌改造引入的回退。
    #[test]
    fn final_reply_markdown_renders_tables_and_headings() {
        let reply = "研究完了——原因找到了。直接说结论：\n## 结论\n四个时间点全部对得上：\n| 时间 | 最后那条消息 | 记录显示 |\n|---|---|---|\n|11:29:38|好，继续找战双|正常完成|\n|11:58:42|好，找《终末地》|同上|\n证据链都来自本机日志。";
        let blocks = commentary_entry_blocks(reply);
        let encoded = serde_json::to_string(&serde_json::Value::Array(blocks.clone())).unwrap();

        // 1) 表格必须变成 table 块，而不是带竖线的段落原文。
        assert!(!encoded.contains("|---|"), "表格分隔行原文泄漏: {encoded}");
        assert!(!encoded.contains("|11:29:38|"), "表格数据行原文泄漏");
        let table = blocks
            .iter()
            .find(|block| block["type"] == "table")
            .expect("应渲染出 table 块");
        let rows = table["cells"].as_array().expect("table cells");
        assert_eq!(rows.len(), 3, "表头 + 两条数据行: {table}");
        // 表头单元格要标记 is_header，三列都要有。
        let header = rows[0].as_array().unwrap();
        assert_eq!(header.len(), 3);
        assert!(
            header.iter().all(|cell| cell["is_header"] == true),
            "表头单元格应标记 is_header"
        );
        // 2) `## 结论` 必须变成 heading 块，记号被剥掉。
        assert!(!encoded.contains("## 结论"), "标题记号未剥离");
        let heading = blocks
            .iter()
            .find(|block| block["type"] == "heading")
            .expect("应渲染出 heading 块");
        assert_eq!(heading["text"], "结论");
        // 3) 正文标题必须比卡片大标题（size 3）**小**，否则喧宾夺主。
        assert!(
            heading["size"].as_u64().unwrap_or(0) > 3,
            "正文标题应小于卡片标题: {heading}"
        );
    }

    /// 表格对齐要尊重分隔行的 `:` 标记；列数不齐的数据行不能把表打乱。
    #[test]
    fn markdown_table_honors_alignment_and_ragged_rows() {
        let blocks = commentary_entry_blocks(
            "| 左 | 中 | 右 |\n|:---|---:|:---:|\n| a | b | c |\n| 只有一列 |",
        );
        let table = blocks
            .iter()
            .find(|block| block["type"] == "table")
            .expect("table");
        let rows = table["cells"].as_array().unwrap();
        let header = rows[0].as_array().unwrap();
        assert_eq!(header[0]["align"], "left");
        assert_eq!(header[1]["align"], "right");
        assert_eq!(header[2]["align"], "center");
        // 列数不足的数据行补空单元格，保持列数一致。
        let ragged = rows[2].as_array().unwrap();
        assert_eq!(ragged.len(), 3, "缺列应补空: {ragged:?}");
        assert_eq!(ragged[0]["text"], "只有一列");
        assert_eq!(ragged[1]["text"], "");
    }

    /// 不是表格的普通竖线文本不能被误判成表格。
    #[test]
    fn non_table_pipe_text_stays_a_paragraph() {
        let blocks = commentary_entry_blocks("管道符 | 不是表格\n第二行");
        assert!(
            blocks.iter().all(|block| block["type"] != "table"),
            "不应误判为表格: {blocks:?}"
        );
        assert!(
            blocks.iter().any(|block| block["type"] == "paragraph"),
            "应保持段落"
        );
    }

    /// `#1` 这类没有空格分隔的文本不能当成标题吃掉。
    #[test]
    fn hash_without_space_is_not_a_heading() {
        let blocks = commentary_entry_blocks("#1 号方案 与 #2 号方案");
        assert!(
            blocks.iter().all(|block| block["type"] != "heading"),
            "`#1` 不应被当作标题: {blocks:?}"
        );
        assert!(blocks.iter().any(|block| block["type"] == "paragraph"));
    }

    /// 回归：最终回复的 markdown 不能以"原文"形式露出。
    ///
    /// 症状（用户实测）：气泡里直接显示 `[Logo 合集/战双 logo](/Users/...)`，
    /// 以及成排的 `- xxx` 列表原文。
    ///
    /// 原因：最终回复内嵌进 blocks 后，走的是 `commentary_entry_blocks`，而它当时
    /// ① 只把 http(s) 链接转成 url（本地路径原样输出）② 完全不处理 `- ` 列表。
    /// 旧路径用的是 `TelegramInputRichMessage::markdown(..)`，由 Telegram 原生渲染，
    /// 所以这是内嵌改造引入的回退。
    #[test]
    fn final_reply_markdown_does_not_leak_raw_syntax() {
        let reply = "找到了，已整理到 [Logo 合集/战双 logo](/Users/miaopasi/Downloads/Logo 合集/战双 logo)（Finder 已打开）。\n核心的几张：\n- [01 游戏 LOGO](/Users/miaopasi/a.png) —— 早期 LOGO\n- [02 Steam 头图](/Users/miaopasi/b.jpg)\n_更多_ 里还有一张维基版图标。来源都写在 [README.md](https://example.com/r.md) 里。";
        let blocks = commentary_entry_blocks(reply);
        let encoded = serde_json::to_string(&serde_json::Value::Array(blocks.clone())).unwrap();

        // 1) 本地路径链接不能以 `[文字](/Users/...)` 原文出现。
        assert!(
            !encoded.contains("](/Users/"),
            "本地路径链接原文泄漏: {encoded}"
        );
        // 2) 链接文字要保留为可读文本。
        assert!(encoded.contains("Logo 合集/战双 logo"), "链接文字应保留");
        assert!(encoded.contains("01 游戏 LOGO"), "列表项链接文字应保留");
        // 3) `- ` 列表要变成 list 块，而不是带 `-` 的普通段落。
        let list = blocks
            .iter()
            .find(|block| block["type"] == "list")
            .expect("`- ` 列表应渲染为 list 块");
        assert_eq!(
            list["items"].as_array().map(Vec::len),
            Some(2),
            "两条列表项应合入同一个 list: {list}"
        );
        // 4) 斜体 `_更多_` 的记号要剥掉。
        assert!(!encoded.contains("_更多_"), "斜体记号未剥离: {encoded}");
        assert!(encoded.contains("更多"), "斜体文字应保留");
        // 5) http 链接仍要转成可点 url。
        assert!(encoded.contains("https://example.com/r.md"));
        assert!(encoded.contains(r#""type":"url""#));
    }

    /// 回归：思考摘要必须用 `code`（等宽蓝底）渲染，且剥掉内联 markdown 记号。
    ///
    /// 需求：让它和底部 `turn <id>` 一样显示成蓝色。`code` 是**字面量**渲染、
    /// 不解析 markdown，所以 `**Check**` 必须被剥成 `Check`，否则界面上会出现星号。
    #[test]
    fn reasoning_summary_renders_as_blue_code_without_markdown_markers() {
        let snapshot = TelegramCommandProgressSnapshot {
            turn_id: "turn".to_string(),
            revision: 1,
            message_id: None,
            entries: Vec::new(),
            dropped_entries: 0,
            retry_count: 0,
            retry_error: None,
            reasoning_summary: Some(
                "**Check** the `Telegram` [state](https://telegram.org)".to_string(),
            ),
            plan_explanation: None,
            plan: Vec::new(),
            diff_summary: None,
            web_searches: Vec::new(),
            dropped_web_searches: 0,
            commentary: Vec::new(),
            commentary_dropped_entries: 0,
            collab: None,
            final_reply: None,
            elapsed_ms: None,
            completed: false,
            failed: false,
        };
        let rendered = render_command_progress(&snapshot, ImText::zh_cn());
        let blocks =
            serde_json::Value::Array(render_task_progress(&snapshot, ImText::zh_cn()).blocks);
        let encoded = serde_json::to_string(&blocks).expect("serialize");

        // 必须有一个 code 类型的段落，内容已剥掉 markdown 记号。
        let code_block = blocks
            .as_array()
            .unwrap()
            .iter()
            .find(|block| block["type"] == "paragraph" && block["text"]["type"] == "code")
            .expect("思考摘要应渲染为 code 块（蓝色）");
        let code_text = code_block["text"]["text"].as_str().unwrap();
        assert!(
            code_text.contains("Check") && code_text.contains("Telegram"),
            "应保留可读文字，实际: {code_text:?}"
        );
        assert!(!code_text.contains("**"), "不应残留 ** 记号: {code_text:?}");
        assert!(!code_text.contains('`'), "不应残留反引号: {code_text:?}");
        assert!(
            !code_text.contains("https://"),
            "链接应压成文字，实际: {code_text:?}"
        );
        assert!(!encoded.contains("**Check**"));
        assert!(rendered.contains("Check"));
    }

    /// 回归：「最终回复」必须内嵌在完成气泡里，且是**默认展开**的折叠块。
    ///
    /// 需求：最终回复不再单独发一条气泡，改在「执行完成」气泡内做成可折叠板块，
    /// 默认不折叠。
    #[test]
    fn final_reply_is_embedded_as_an_open_details_block() {
        let snapshot = TelegramCommandProgressSnapshot {
            turn_id: "turn".to_string(),
            revision: 1,
            message_id: None,
            entries: Vec::new(),
            dropped_entries: 0,
            retry_count: 0,
            retry_error: None,
            reasoning_summary: None,
            plan_explanation: None,
            plan: Vec::new(),
            diff_summary: None,
            web_searches: Vec::new(),
            dropped_web_searches: 0,
            commentary: Vec::new(),
            commentary_dropped_entries: 0,
            collab: None,
            final_reply: Some("搞定了，已把 6 个文件夹归到总文件夹。".to_string()),
            elapsed_ms: Some(25_000),
            completed: true,
            failed: false,
        };
        let rendered = render_task_progress(&snapshot, ImText::zh_cn());
        let blocks = serde_json::Value::Array(rendered.blocks.clone());

        let details = blocks
            .as_array()
            .unwrap()
            .iter()
            .find(|block| block["type"] == "details" && block["summary"] == "最终回复")
            .expect("应有「最终回复」折叠块");
        // 默认展开：`is_open` 必须为 true。
        assert_eq!(
            details["is_open"],
            serde_json::Value::Bool(true),
            "「最终回复」折叠块必须默认展开"
        );
        let inner = serde_json::to_string(&details["blocks"]).expect("serialize");
        assert!(inner.contains("6 个文件夹"), "折叠块内应包含正文: {inner}");

        // 耗时拼到顶部标题（原来在单独的「✅ 已完成」气泡上）。
        assert!(
            rendered.fallback_markdown.contains("25秒"),
            "标题应带耗时，实际: {}",
            rendered
                .fallback_markdown
                .lines()
                .next()
                .unwrap_or_default()
        );
        // 回退文本里也要有最终回复，否则富消息不可用时会丢内容。
        assert!(rendered.fallback_markdown.contains("最终回复"));
        assert!(rendered.fallback_markdown.contains("6 个文件夹"));
    }

    /// 没有最终回复时，不应出现空的「最终回复」折叠块。
    #[test]
    fn absent_final_reply_renders_no_details_block() {
        let snapshot = TelegramCommandProgressSnapshot {
            turn_id: "turn".to_string(),
            revision: 1,
            message_id: None,
            entries: Vec::new(),
            dropped_entries: 0,
            retry_count: 0,
            retry_error: None,
            reasoning_summary: None,
            plan_explanation: None,
            plan: Vec::new(),
            diff_summary: None,
            web_searches: Vec::new(),
            dropped_web_searches: 0,
            commentary: Vec::new(),
            commentary_dropped_entries: 0,
            collab: None,
            final_reply: None,
            elapsed_ms: None,
            completed: true,
            failed: false,
        };
        let blocks =
            serde_json::Value::Array(render_task_progress(&snapshot, ImText::zh_cn()).blocks);
        assert!(
            !blocks
                .as_array()
                .unwrap()
                .iter()
                .any(|block| block["type"] == "details" && block["summary"] == "最终回复"),
            "无最终回复时不应渲染该折叠块"
        );
    }

    /// 短思考摘要不截断、不加省略号。
    #[test]
    fn short_reasoning_summary_is_not_ellipsized() {
        let snapshot = TelegramCommandProgressSnapshot {
            turn_id: "turn".to_string(),
            revision: 1,
            message_id: None,
            entries: Vec::new(),
            dropped_entries: 0,
            retry_count: 0,
            retry_error: None,
            reasoning_summary: Some("checking the build output".to_string()),
            plan_explanation: None,
            plan: Vec::new(),
            diff_summary: None,
            web_searches: Vec::new(),
            dropped_web_searches: 0,
            commentary: Vec::new(),
            commentary_dropped_entries: 0,
            collab: None,
            final_reply: None,
            elapsed_ms: None,
            completed: true,
            failed: false,
        };
        let rendered = render_command_progress(&snapshot, ImText::zh_cn());
        assert!(rendered.contains("checking the build output"));
        assert!(!rendered.contains("checking the build output…"));
    }

    /// 思考文案与工具步骤必须按到达序号交错：文案可见、工具折叠、顺序与发生顺序一致。
    /// 回归：展示窗口必须是**连续的最新一段**，否则刚完成的步骤会"闪一下就消失"。
    ///
    /// 曾经从前往后 `.take(N)` 取填充窗口，拿到的是**最旧**的 N 条。步骤一完成就
    /// 掉出"最后 3 条"的优先级窗口，又不在最旧的 N 条里，于是从气泡中消失。
    ///
    /// 只断言"最新一条可见"抓不到这个 bug（它始终在优先级窗口里）；正确的特征是
    /// **窗口必须是从最新一条往回的连续区间，中间没有空洞**。
    /// 回归：工具数超过名额上限时，思考**不能**全被挤到一起。
    ///
    /// 症状（用户实测）：任务前面显示正常，步数涨上去后"思考过程和工具摘要
    /// 突然全部合并"。
    ///
    /// 原因：名额原先只按"最近的 N 条"分配，全堆在时间轴末端。总量一超过名额，
    /// 中段工具被整体挤出，思考之间失去间隔，于是各自并成一大批。
    ///
    /// 修法：为每条思考锚定它前后紧邻的工具，剩余名额再按最近补齐。
    #[test]
    fn thinking_stays_interleaved_when_tool_count_exceeds_the_budget() {
        // 三条思考分布在工具流的前段，之后是大量工具——正是长任务的实际形态。
        for tool_count in [30usize, 44, 60, 100] {
            let commentary_sequences = [0u64, 8, 16];
            let mut entries = Vec::new();
            let mut sequence = 1u64;
            for _ in 0..tool_count {
                while commentary_sequences.contains(&sequence) {
                    sequence += 1;
                }
                let mut entry = completed_entry(
                    &format!("cmd-{sequence}"),
                    &json!({"command": "cargo test"}),
                );
                entry.sequence = sequence;
                entries.push(entry);
                sequence += 1;
            }

            let snapshot = TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 1,
                message_id: None,
                entries,
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                commentary: commentary_sequences
                    .iter()
                    .map(|sequence| TelegramCommentaryEntry {
                        item_id: format!("think-{sequence}"),
                        text: "思考".to_string(),
                        sequence: *sequence,
                    })
                    .collect(),
                commentary_dropped_entries: 0,
                collab: None,
                final_reply: None,
                elapsed_ms: None,
                completed: false,
                failed: false,
            };

            let shown = interleaved_tool_indices(&snapshot);

            // 组装排序后的条目序列（思考 + 命中的工具），断言**没有两条思考相邻**。
            //
            // 这正是"思考过程全部合并"的直接特征：相邻同类会被合并成一个批次。
            let mut items: Vec<(u64, bool)> = snapshot
                .commentary
                .iter()
                .map(|entry| (entry.sequence, true))
                .chain(
                    shown
                        .iter()
                        .map(|index| (snapshot.entries[*index].sequence, false)),
                )
                .collect();
            items.sort_unstable();

            for window in items.windows(2) {
                assert!(
                    !(window[0].1 && window[1].1),
                    "共 {tool_count} 个工具时，思考 {:?} 与 {:?} 相邻，\
                     会被合并成一批：{items:?}",
                    window[0].0,
                    window[1].0
                );
            }
        }
    }

    #[test]
    fn recent_tool_window_is_contiguous_so_completed_steps_do_not_vanish() {
        for count in [4usize, 13, 16, 20, 40] {
            let entries: Vec<TelegramCommandProgressEntry> = (0..count)
                .map(|index| {
                    let mut entry =
                        completed_entry(&format!("cmd-{index}"), &json!({"command": "cargo test"}));
                    entry.sequence = index as u64;
                    entry
                })
                .collect();

            let snapshot = TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 1,
                message_id: None,
                entries,
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                commentary: Vec::new(),
                commentary_dropped_entries: 0,
                collab: None,
                final_reply: None,
                elapsed_ms: None,
                completed: false,
                failed: false,
            };

            let shown = interleaved_tool_indices(&snapshot);
            let max_index = snapshot.entries.len() - 1;
            // 期望：从最新一条往回、长度为 shown.len() 的连续区间。
            // 旧实现取"最旧 N 条"，会在中段留下空洞（例如 16 条时缺下标 12），
            // 这正是"闪一下就消失"的表现。
            let expected: Vec<usize> =
                ((max_index + 1).saturating_sub(shown.len())..=max_index).collect();
            assert_eq!(
                shown, expected,
                "共 {count} 条时展示窗口不是连续的最新区间（中间有空洞）"
            );
        }
    }

    #[test]
    fn render_task_progress_interleaves_commentary_and_tool_batches() {
        let mut tool_a = completed_entry("tool-a", &json!({"command": "cargo test"}));
        tool_a.sequence = 1;
        let mut tool_b = completed_entry("tool-b", &json!({"command": "cargo build"}));
        tool_b.sequence = 3;

        let rendered = render_task_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn-interleave".to_string(),
                revision: 1,
                message_id: None,
                entries: vec![tool_a, tool_b],
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                commentary: vec![
                    TelegramCommentaryEntry {
                        item_id: "c1".to_string(),
                        text: "先看事件流".to_string(),
                        sequence: 0,
                    },
                    TelegramCommentaryEntry {
                        item_id: "c2".to_string(),
                        text: "再看渲染".to_string(),
                        sequence: 2,
                    },
                ],
                commentary_dropped_entries: 0,
                collab: None,
                final_reply: None,
                elapsed_ms: None,
                completed: false,
                failed: false,
            },
            ImText::zh_cn(),
        );

        // 顶层块序列：思考批次与工具批次交替出现。
        let summaries: Vec<String> = rendered
            .blocks
            .iter()
            .filter(|block| block["type"] == "details")
            .map(|block| {
                format!(
                    "{}|open={}",
                    block["summary"].as_str().unwrap_or(""),
                    block["is_open"].as_bool().unwrap_or(false)
                )
            })
            .collect();

        assert_eq!(
            summaries,
            vec![
                // 思考默认展开（is_open=true），工具默认折叠。
                "思考过程（1）|open=true",
                "工具摘要（1）|open=false",
                "思考过程（1）|open=true",
                "工具摘要（1）|open=false",
            ],
            "思考与工具应按到达序号交错，且思考展开、工具折叠"
        );

        // 文案内容确实在「思考过程」块里，而不是被折叠丢弃。
        let first_thinking = rendered
            .blocks
            .iter()
            .find(|block| block["type"] == "details" && block["summary"] == "思考过程（1）")
            .expect("思考过程块");
        let encoded = first_thinking.to_string();
        assert!(encoded.contains("先看事件流"), "思考块应含文案：{encoded}");
    }

    #[test]
    fn render_task_progress_builds_one_complete_rich_message() {
        let rendered = render_task_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn-019f-example".to_string(),
                revision: 7,
                message_id: Some("42".to_string()),
                entries: vec![
                    running_entry("cmd", &json!({"command": "cargo test"})),
                    mcp_completed_entry(
                        "mcp",
                        &json!({
                            "type": "mcpToolCall",
                            "server": "browser",
                            "tool": "screenshot",
                            "status": "completed",
                            "durationMs": 850
                        }),
                    ),
                ],
                dropped_entries: 2,
                retry_count: 2,
                retry_error: Some("503 Service Unavailable".to_string()),
                reasoning_summary: Some(
                    "**Check** the active `Telegram` delivery [state](https://telegram.org)."
                        .to_string(),
                ),
                plan_explanation: Some("Inspect, implement, verify.".to_string()),
                plan: vec![
                    TelegramPlanStep {
                        step: "Inspect the current flow".to_string(),
                        status: TelegramPlanStepStatus::Completed,
                    },
                    TelegramPlanStep {
                        step: "Run regression tests".to_string(),
                        status: TelegramPlanStepStatus::InProgress,
                    },
                ],
                diff_summary: Some(TelegramDiffSummary {
                    file_count: 1,
                    additions: 12,
                    deletions: 3,
                    files: vec![TelegramDiffFileSummary {
                        path: "src/im/events.rs".to_string(),
                        additions: 12,
                        deletions: 3,
                    }],
                    paths: vec!["src/im/events.rs".to_string()],
                    omitted_paths: 0,
                }),
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                commentary: Vec::new(),
                commentary_dropped_entries: 0,
                collab: Some(TelegramCollabProgressSnapshot {
                    entries: vec![TelegramCollabProgressEntry {
                        agent_id: "secret-agent-id".to_string(),
                        name: "api_review".to_string(),
                        status: TelegramCollabProgressStatus::Running,
                        detail: Some("Reviewing the official Telegram API".to_string()),
                        started_at_ms: 1_000,
                        updated_at_ms: 2_000,
                    }],
                    dropped_entries: 0,
                    completed: false,
                }),
                final_reply: None,
                elapsed_ms: None,
                completed: false,
                failed: false,
            },
            ImText::zh_cn(),
        );

        let blocks = serde_json::Value::Array(rendered.blocks.clone());
        let encoded = serde_json::to_string(&blocks).expect("rich blocks should serialize");
        assert_eq!(blocks[0]["type"], "heading");
        assert_eq!(blocks[0]["text"], "任务进行中");
        assert_eq!(
            blocks[blocks.as_array().unwrap().len() - 1]["type"],
            "footer"
        );
        let tools_index = blocks
            .as_array()
            .unwrap()
            .iter()
            // 标题显示的是**该批次**的步骤数（与图中 `工具摘要（1）` 的口径一致），
            // 历史省略数由面板内的"...另外 N 个较早步骤"提示。
            .position(|block| block["type"] == "details" && block["summary"] == "工具摘要（2）")
            .expect("tools should be folded into one summary panel");
        let panel = blocks[tools_index]["blocks"]
            .as_array()
            .expect("tools panel blocks");
        assert_eq!(panel[0]["type"], "paragraph");
        assert_eq!(panel[0]["text"]["type"], "bold");
        assert_eq!(panel[0]["text"]["text"], "执行中 · 4 步 · 1 个进行中");
        assert_eq!(panel[1]["type"], "pre");
        assert_eq!(panel[1]["language"], "shell");
        assert!(
            panel
                .iter()
                .any(|block| block["text"] == "… 另外 2 个较早步骤")
        );
        assert_eq!(
            blocks
                .as_array()
                .unwrap()
                .iter()
                .filter(|block| block["type"] == "divider")
                .count(),
            0,
            "the folded tools summary replaces the plan-to-execution divider"
        );
        assert!(encoded.contains("details"));
        assert!(encoded.contains("browser.screenshot"));
        assert!(encoded.contains("api_review"));
        assert!(encoded.contains("503 Service Unavailable"));
        assert!(encoded.contains("events.rs"));
        assert!(!encoded.contains("src/im/events.rs"));
        assert!(encoded.contains("\"type\":\"table\""));
        assert!(encoded.contains("\"is_bordered\":true"));
        assert!(encoded.contains("\"is_striped\":true"));
        assert!(encoded.contains("\"is_header\":true"));
        assert!(encoded.contains("\"text\":\"+12\""));
        assert!(encoded.contains("\"text\":\"-3\""));
        assert!(!encoded.contains("secret-agent-id"));
        assert_eq!(encoded.matches("\"has_checkbox\":true").count(), 3);
        assert_eq!(encoded.matches("\"is_checked\":true").count(), 1);
        // 思考摘要已去掉标题，并改为 `code`（等宽蓝底，与底部 `turn <id>` 一致）。
        assert!(!encoded.contains("思考摘要"), "不应再渲染「思考摘要」标题");
        let reasoning_body = blocks
            .as_array()
            .unwrap()
            .iter()
            .find(|block| {
                block["type"] == "paragraph"
                    && block["text"]["type"] == "code"
                    && block["text"]["text"]
                        .as_str()
                        .is_some_and(|t| t.contains("Check"))
            })
            .expect("reasoning body should be always visible");
        assert_eq!(reasoning_body["type"], "paragraph");
        assert_eq!(reasoning_body["text"]["type"], "code");
        assert!(
            !blocks
                .as_array()
                .unwrap()
                .iter()
                .any(|block| { block["type"] == "details" && block["summary"] == "思考摘要" })
        );
        assert!(!encoded.contains("**Check**"));
        for marker in ["✅", "❌", "⚠️", "⏳", "🛠", "🔄"] {
            assert!(!encoded.contains(marker), "rich progress leaked {marker}");
        }
        assert!(rendered.fallback_markdown.chars().count() <= 3_800);
        assert!(rendered.fallback_markdown.contains("api_review"));
        assert!(
            rendered
                .fallback_markdown
                .starts_with("🔄 任务进行中\n──────────────")
        );
        let fallback_plan = rendered
            .fallback_markdown
            .find("计划 · 1/2")
            .expect("fallback plan heading");
        let fallback_execution = rendered
            .fallback_markdown
            .find("执行中 · 4 步 · 1 个进行中")
            .expect("fallback execution heading");
        // 思考摘要已去掉标题，用正文内容定位它在 fallback 中的位置。
        let fallback_reasoning = rendered
            .fallback_markdown
            .find("Check")
            .expect("fallback reasoning body");
        let fallback_diff = rendered
            .fallback_markdown
            .find("文件修改 · 1 个文件 · +12 -3")
            .expect("fallback diff heading");
        assert!(fallback_plan < fallback_execution);
        assert!(fallback_execution < fallback_reasoning);
        assert!(fallback_reasoning < fallback_diff);
        // 卡片头（🔄/❌）现在是 fallback 的合法组成部分，不再视为泄漏。
        for marker in ["✅", "⚠️", "⏳", "🛠"] {
            assert!(
                !rendered.fallback_markdown.contains(marker),
                "fallback progress leaked {marker}"
            );
        }
    }

    #[test]
    fn web_searches_are_folded_into_the_task_progress_message() {
        let web_searches = (1..=4)
            .map(|index| TelegramWebSearchProgressEntry {
                item_id: format!("search-{index}"),
                summary: format!("搜索 · query {index} · 1 条结果"),
                blocks: vec![json!({
                    "type": "paragraph",
                    "text": format!("result {index}"),
                })],
                fallback_markdown: format!(
                    "🔎 搜索\n\n关键词：`query {index}`\n结果：1 条\n- result {index}"
                ),
            })
            .collect();
        let rendered = render_task_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 4,
                message_id: Some("42".to_string()),
                entries: Vec::new(),
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                web_searches,
                dropped_web_searches: 2,
                commentary: Vec::new(),
                commentary_dropped_entries: 0,
                collab: None,
                final_reply: None,
                elapsed_ms: None,
                completed: false,
                failed: false,
            },
            ImText::zh_cn(),
        );

        let blocks = serde_json::Value::Array(rendered.blocks);
        assert_eq!(blocks[0]["text"], "任务进行中");
        assert!(blocks.as_array().unwrap().iter().any(|block| {
            block["type"] == "paragraph"
                && block["text"]["type"] == "bold"
                && block["text"]["text"] == "搜索 · 6 次"
        }));
        assert!(
            blocks
                .as_array()
                .unwrap()
                .iter()
                .any(|block| block["summary"] == "较早搜索 · 4 次")
        );
        assert!(
            blocks
                .as_array()
                .unwrap()
                .iter()
                .any(|block| block["summary"] == "搜索 · query 3 · 1 条结果")
        );
        assert!(
            blocks
                .as_array()
                .unwrap()
                .iter()
                .any(|block| block["summary"] == "搜索 · query 4 · 1 条结果")
        );
        assert!(rendered.fallback_markdown.contains("搜索 · 6 次"));
        assert!(
            rendered
                .fallback_markdown
                .contains("较早搜索 · 4 次（已折叠）")
        );
        assert!(rendered.fallback_markdown.contains("query 3"));
        assert!(rendered.fallback_markdown.contains("query 4"));
        assert!(!rendered.fallback_markdown.contains("query 1"));
    }

    #[test]
    fn rich_diff_table_limits_rows_and_shows_only_file_names() {
        let files = (0..10)
            .map(|index| TelegramDiffFileSummary {
                path: format!(
                    "src/very/long/path/that/should/stay/readable/on/mobile/file-{index}.rs"
                ),
                additions: index + 1,
                deletions: index,
            })
            .collect::<Vec<_>>();
        let paths = files.iter().map(|file| file.path.clone()).collect();
        let rendered = render_task_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 1,
                message_id: None,
                entries: Vec::new(),
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: Some(TelegramDiffSummary {
                    file_count: 10,
                    additions: 55,
                    deletions: 45,
                    files,
                    paths,
                    omitted_paths: 0,
                }),
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                commentary: Vec::new(),
                commentary_dropped_entries: 0,
                collab: None,
                final_reply: None,
                elapsed_ms: None,
                completed: true,
                failed: false,
            },
            ImText::zh_cn(),
        );

        let details = rendered
            .blocks
            .iter()
            .find(|block| block["summary"] == "文件修改 · 10 个文件 · +55 -45")
            .expect("file change details");
        let table = &details["blocks"][0];
        assert_eq!(table["type"], "table");
        assert_eq!(table["cells"].as_array().unwrap().len(), 9);
        assert_eq!(table["cells"][0][0]["text"], "文件");
        assert_eq!(table["cells"][0][1]["text"], "新增");
        assert_eq!(table["cells"][0][2]["text"], "删除");
        assert_eq!(table["cells"][1][1]["text"], "+1");
        assert_eq!(table["cells"][1][2]["text"], "-0");
        assert_eq!(table["cells"][1][0]["text"]["text"], "file-0.rs");
        assert!("file-0.rs".chars().count() <= TELEGRAM_DIFF_TABLE_PATH_CHARS);
        assert_eq!(details["blocks"][1]["text"], "… 另外 2 个文件");
        assert!(rendered.fallback_markdown.contains("• file-0.rs"));
        assert!(!rendered.fallback_markdown.contains("src/very/long/path"));
        assert!(rendered.fallback_markdown.contains("… 另外 2 个文件"));
    }

    #[test]
    fn diff_file_display_name_handles_moves_and_both_path_separators() {
        assert_eq!(diff_file_display_name("src/main.rs"), "main.rs");
        assert_eq!(
            diff_file_display_name(r"C:\\workspace\\src\\main.rs"),
            "main.rs"
        );
        assert_eq!(
            diff_file_display_name("src/old.rs -> nested/new.rs"),
            "old.rs -> new.rs"
        );
    }

    #[test]
    fn rich_failure_details_and_step_share_a_compact_command_budget() {
        let command = format!("prefix-{}-suffix", "x".repeat(100));
        let rendered = render_task_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 1,
                message_id: None,
                entries: vec![completed_entry(
                    "failed",
                    &json!({
                        "command": command,
                        "exitCode": 1,
                        "aggregatedOutput": "boom",
                    }),
                )],
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                commentary: Vec::new(),
                commentary_dropped_entries: 0,
                collab: None,
                final_reply: None,
                elapsed_ms: None,
                completed: true,
                failed: true,
            },
            ImText::zh_cn(),
        );

        let tools = rendered
            .blocks
            .iter()
            .find(|block| block["type"] == "details" && block["summary"] == "工具摘要（1）")
            .expect("tools summary panel");
        let panel = tools["blocks"].as_array().expect("tools panel blocks");
        let error_details = panel
            .iter()
            .find(|block| block["type"] == "details" && block["blocks"][0]["type"] == "pre")
            .expect("failure details");
        assert_eq!(
            error_details["summary"],
            json!([
                "错误摘要 ",
                {
                    "type": "code",
                    "text": "prefix-xxxxxxxxxxxxxxxxxxx...xxxxxxxxxxxxxxxxxxxx-suffix",
                },
            ])
        );
        assert_eq!(error_details["blocks"][0]["text"], "boom");

        let command_block_index = panel
            .iter()
            .position(|block| block["type"] == "pre" && block["language"] == "shell")
            .expect("command block");
        assert_eq!(
            panel[command_block_index],
            json!({
                "type": "pre",
                "text": "prefix-xxxxxxxxxxxxxxxxxxx...xxxxxxxxxxxxxxxxxxxx-suffix",
                "language": "shell",
            })
        );
        assert_eq!(panel[command_block_index + 1]["type"], "footer");
        assert_eq!(
            panel[command_block_index + 1]["text"],
            json!([
                {"type": "bold", "text": "失败"},
                " · exit 1",
            ])
        );
        assert!(rendered.fallback_markdown.contains(&command));
    }

    #[test]
    fn rich_progress_without_plan_folds_the_only_step_into_the_tools_summary() {
        let rendered = render_task_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 1,
                message_id: None,
                entries: vec![running_entry("cmd", &json!({"command": "cargo test"}))],
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                commentary: Vec::new(),
                commentary_dropped_entries: 0,
                collab: None,
                final_reply: None,
                elapsed_ms: None,
                completed: false,
                failed: false,
            },
            ImText::zh_cn(),
        );

        let blocks = serde_json::Value::Array(rendered.blocks);
        assert_eq!(blocks[0]["type"], "heading");
        assert_eq!(blocks[0]["text"], "执行中 · 1 步 · 1 个进行中");
        assert_eq!(blocks[1]["type"], "details");
        assert_eq!(blocks[1]["summary"], "工具摘要（1）");
        let panel = blocks[1]["blocks"].as_array().expect("tools panel blocks");
        assert_eq!(panel[0]["type"], "pre");
        assert_eq!(panel[0]["language"], "shell");
        assert_eq!(panel[1]["type"], "footer");
        assert_eq!(
            blocks
                .as_array()
                .unwrap()
                .iter()
                .filter(|block| block["type"] == "divider")
                .count(),
            0,
            "the footer should not add a redundant divider"
        );
        assert!(
            rendered
                .fallback_markdown
                .starts_with("🔄 执行中 · 1 步 · 1 个进行中\n──────────────")
        );
    }

    #[test]
    fn commentary_is_visible_and_tools_are_folded() {
        let rendered = render_task_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 3,
                message_id: None,
                entries: vec![{
                    // 真实系统里 upsert 会分配递增序号；这里给工具序号 2，
                    // 表示两条思考之后才发生（避免与文案的序号冲突）。
                    let mut entry =
                        completed_entry("cmd", &json!({"command": "cargo test", "exitCode": 0}));
                    entry.sequence = 2;
                    entry
                }],
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                commentary: vec![
                    TelegramCommentaryEntry {
                        item_id: "commentary-1".to_string(),
                        text: "先看 `src/im/events.rs`".to_string(),
                        sequence: 0,
                    },
                    TelegramCommentaryEntry {
                        item_id: "commentary-2".to_string(),
                        text: "跑测试：\n```shell\ncargo test\n```".to_string(),
                        sequence: 1,
                    },
                ],
                commentary_dropped_entries: 2,
                collab: None,
                final_reply: None,
                elapsed_ms: None,
                completed: false,
                failed: false,
            },
            ImText::zh_cn(),
        );

        let blocks = serde_json::Value::Array(rendered.blocks.clone());
        let array = blocks.as_array().expect("rich blocks");

        // 思考过程：可折叠但默认展开（is_open = true），内容直接可见。
        // 两条连续的思考文案合并成一张卡片（中间没有工具插入）。
        let thinking = array
            .iter()
            .find(|block| block["type"] == "details" && block["summary"] == "思考过程（2）")
            .expect("thinking card");
        assert!(
            thinking["is_open"].as_bool().unwrap_or(false),
            "思考过程必须默认展开，否则用户读不到内容"
        );
        let thinking_encoded = thinking.to_string();
        assert!(thinking_encoded.contains("先看"), "{thinking_encoded}");
        assert!(thinking_encoded.contains("跑测试"), "{thinking_encoded}");
        // 代码围栏保留在思考块内（不被折叠丢弃）。
        assert!(
            thinking["blocks"]
                .as_array()
                .expect("thinking panel")
                .iter()
                .any(|block| block["type"] == "pre" && block["language"] == "shell"),
            "思考块内应保留代码围栏"
        );

        // 工具摘要：默认折叠。
        let tools = array
            .iter()
            .find(|block| block["type"] == "details" && block["summary"] == "工具摘要（1）")
            .expect("tools summary panel");
        // rich_blocks::details 在折叠（false）时不写 is_open 字段，缺省即折叠。
        assert!(
            !tools["is_open"].as_bool().unwrap_or(false),
            "工具摘要必须默认折叠"
        );

        // 省略提示仍然出现（2 条较早进展被丢弃），且位于思考卡片之前。
        let omitted_block = array
            .iter()
            .position(|block| block["text"] == "… 另外 2 条较早进展已省略")
            .expect("省略提示");
        let thinking_index = array
            .iter()
            .position(|block| block["type"] == "details" && block["summary"] == "思考过程（2）")
            .expect("thinking");
        assert!(omitted_block < thinking_index, "省略提示应在思考卡片之前");
        let tools_index = array
            .iter()
            .position(|block| block["type"] == "details" && block["summary"] == "工具摘要（1）")
            .expect("tools");
        assert!(thinking_index < tools_index);
    }

    #[test]
    fn commentary_and_tools_stay_within_the_fallback_budget() {
        let rendered = render_task_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 5,
                message_id: None,
                entries: vec![completed_entry(
                    "cmd",
                    &json!({"command": "cargo test", "exitCode": 0}),
                )],
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                commentary: (0..4)
                    .map(|index| TelegramCommentaryEntry {
                        item_id: format!("commentary-{index}"),
                        text: "x".repeat(1_500),
                        sequence: 0,
                    })
                    .collect(),
                commentary_dropped_entries: 0,
                collab: None,
                final_reply: None,
                elapsed_ms: None,
                completed: false,
                failed: false,
            },
            ImText::zh_cn(),
        );

        assert!(
            rendered.fallback_markdown.chars().count()
                <= super::TELEGRAM_TASK_PROGRESS_FALLBACK_MAX_CHARS
        );
        assert!(
            rendered.fallback_markdown.contains("cargo test"),
            "the tools fallback must survive the commentary truncation"
        );
    }

    #[test]
    fn command_entries_use_left_aligned_shell_blocks_with_status_footers() {
        let mut interrupted = running_entry("interrupted", &json!({"command": "stop-me"}));
        interrupted.status = TelegramCommandProgressStatus::Interrupted;
        let entries = [
            running_entry("running", &json!({"command": "still-running"})),
            completed_entry("succeeded", &json!({"command": "done", "exitCode": 0})),
            completed_entry("failed", &json!({"command": "broken", "exitCode": 1})),
            interrupted,
        ];

        let rendered = entries
            .iter()
            .map(|entry| rich_command_entry_blocks(entry, ImText::zh_cn()))
            .collect::<Vec<_>>();
        assert!(rendered.iter().all(|blocks| blocks.len() == 2));
        assert!(rendered.iter().all(|blocks| blocks[0]["type"] == "pre"));
        assert!(
            rendered
                .iter()
                .all(|blocks| blocks[0]["language"] == "shell")
        );
        assert!(rendered.iter().all(|blocks| blocks[1]["type"] == "footer"));

        let encoded = serde_json::to_string(&rendered).expect("entries should serialize");
        assert!(encoded.contains("成功"));
        assert!(!encoded.contains("已完成"));
        assert!(encoded.contains("进行中"));
        assert!(encoded.contains("失败"));
        assert!(encoded.contains("已中断"));
        assert!(!encoded.contains("has_checkbox"));
        assert!(!encoded.contains("✅"));
        assert!(!encoded.contains("❌"));
        assert!(!encoded.contains("⚠️"));
    }
}
