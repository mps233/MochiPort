//! Extracted verbatim from `adapter.rs`; no behavior changes.

use super::*;

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use crate::{
        im::core::{i18n::ImText, thread::ThreadCreateOption},
        im::runtime::{
            ApprovalDecisionOption, ObservedSetting, PendingApproval,
            TelegramModelSwitchRequestState, TelegramThreadSettingsDraft,
            TelegramThreadSettingsModelChoice, TelegramThreadSettingsSpeed,
            TelegramThreadSettingsStage, ThreadSettingsSnapshot,
        },
        im::telegram::{
            api::{TELEGRAM_MAX_MEDIA_GROUP_ITEMS, TelegramApi},
            types::TelegramSettings,
        },
    };

    use super::{
        TELEGRAM_MAX_MESSAGE_CHARS, TelegramAdapter, approval_keyboard, approval_text,
        create_options_keyboard, empty_inline_keyboard, resolved_approval_text,
        telegram_cleanup_text, telegram_context_compaction_messages, telegram_markdown_to_html,
        telegram_text_chunks, telegram_turn_completed_chunks, telegram_turn_completed_messages,
        telegram_user_message_chunks, telegram_user_message_messages, thread_settings_html,
        thread_settings_keyboard,
    };

    #[tokio::test]
    async fn thinking_draft_skips_targets_that_cannot_be_private_chats() {
        let adapter = TelegramAdapter::new(TelegramApi::new(TelegramSettings::default()));

        assert!(!adapter.send_thinking_draft("-1001", 7).await.unwrap());
        assert!(!adapter.send_thinking_draft("@channel", 7).await.unwrap());
        assert!(!adapter.send_thinking_draft("42", 0).await.unwrap());
    }

    #[test]
    fn balances_large_image_sets_without_singleton_albums() {
        assert_eq!(
            TelegramAdapter::telegram_media_group_sizes(0),
            Vec::<usize>::new()
        );
        assert_eq!(TelegramAdapter::telegram_media_group_sizes(1), vec![1]);
        assert_eq!(TelegramAdapter::telegram_media_group_sizes(10), vec![10]);
        assert_eq!(TelegramAdapter::telegram_media_group_sizes(11), vec![6, 5]);
        assert_eq!(
            TelegramAdapter::telegram_media_group_sizes(21),
            vec![7, 7, 7]
        );
        assert!(
            TelegramAdapter::telegram_media_group_sizes(101)
                .iter()
                .all(|size| (2..=TELEGRAM_MAX_MEDIA_GROUP_ITEMS).contains(size))
        );
    }

    /// 英文标记的快捷入口，便于断言；默认 locale（中文）的标记由适配层选择。
    fn chunks_en(text: &str) -> Vec<String> {
        telegram_text_chunks(text, "(continues...)", "(continued)")
    }

    #[test]
    fn chunks_long_text_on_char_boundaries() {
        let chunks = chunks_en(&"你好世界".repeat(1100));

        assert!(chunks.len() > 1);
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.chars().count() <= TELEGRAM_MAX_MESSAGE_CHARS)
        );
        assert!(chunks[0].ends_with("(continues...)"));
        assert!(chunks[1].starts_with("(continued)"));
    }

    #[test]
    fn keeps_single_message_when_within_limit() {
        let text = "hello";
        let chunks = chunks_en(text);

        assert_eq!(chunks, vec!["hello"]);
    }

    #[test]
    fn empty_message_uses_space_placeholder() {
        let chunks = chunks_en("  \n ");

        assert_eq!(chunks, vec![" "]);
    }

    #[test]
    fn telegram_cleanup_removes_codex_ui_directives_from_standalone_lines() {
        let text = concat!(
            "提交完成\n\n",
            "::git-stage{cwd=\"/tmp/codexhub\"}\n",
            "::git-commit{cwd=\"/tmp/codexhub\"}\n\n",
            "下一行"
        );

        let cleaned = telegram_cleanup_text(text);

        assert_eq!(cleaned, "提交完成\n\n下一行");
        assert!(!cleaned.contains("::git-stage"));
        assert!(!cleaned.contains("::git-commit"));
    }

    #[test]
    fn telegram_cleanup_preserves_code_examples_and_unknown_directives() {
        let text = concat!(
            "```text\n",
            "::git-commit{cwd=\"/tmp/codexhub\"}\n",
            "```\n",
            "::custom{value=\"keep\"}\n",
            "::git-not-a-real-directive{value=\"keep\"}"
        );

        let cleaned = telegram_cleanup_text(text);

        assert!(cleaned.contains("```text\n::git-commit{cwd=\"/tmp/codexhub\"}\n```"));
        assert!(cleaned.contains("::custom{value=\"keep\"}"));
        assert!(cleaned.contains("::git-not-a-real-directive{value=\"keep\"}"));
    }

    #[test]
    fn prefers_newline_split_for_long_text() {
        let first = "a".repeat(3000);
        let second = "b".repeat(3000);
        let chunks = chunks_en(&format!("{first}\n{second}"));

        assert!(chunks[0].contains("(continues...)"));
        assert!(chunks[0].contains('\n'));
        assert!(chunks[0].trim_start().starts_with('a'));
        assert!(chunks[1].contains('b'));
    }

    #[test]
    fn turn_completed_message_uses_a_card_header_with_plain_fallback() {
        let (rich, fallback) =
            telegram_turn_completed_messages("🤖 Codex\n\n**Build:** `323`", "✅ 已完成 · 3分12秒");

        assert_eq!(
            rich,
            "**✅ 已完成 · 3分12秒**\n──────────────\n\n🤖 Codex\n\n**Build:** `323`"
        );
        assert_eq!(
            fallback,
            "✅ 已完成 · 3分12秒\n──────────────\n\n🤖 Codex\n\n**Build:** `323`"
        );
    }

    #[test]
    fn turn_completed_header_handles_an_empty_reply() {
        let (rich, fallback) = telegram_turn_completed_messages("  ", "Done & closed");

        assert_eq!(rich, "**Done & closed**");
        assert_eq!(fallback, "Done & closed");
    }

    #[test]
    fn turn_completed_chunks_reserve_space_for_the_card_header() {
        for text in ["a".repeat(TELEGRAM_MAX_MESSAGE_CHARS), "界".repeat(4_500)] {
            let chunks = telegram_turn_completed_chunks(
                &text,
                "✅ 已完成 · 3分12秒",
                "(continues...)",
                "(continued)",
            );
            assert!(chunks.len() > 1);
            assert!(
                chunks[..chunks.len() - 1]
                    .iter()
                    .all(|chunk| chunk.chars().count() <= TELEGRAM_MAX_MESSAGE_CHARS)
            );
            assert!(
                chunks[..chunks.len() - 1]
                    .iter()
                    .all(|chunk| !chunk.contains("✅ 已完成"))
            );
            let (rich, fallback) = telegram_turn_completed_messages(
                chunks.last().expect("final chunk"),
                "✅ 已完成 · 3分12秒",
            );
            assert!(fallback.chars().count() <= TELEGRAM_MAX_MESSAGE_CHARS);
            assert!(rich.matches("✅ 已完成").count() == 1);
            assert!(!rich.contains("<footer>"));
        }
    }

    #[test]
    fn context_compaction_uses_a_native_aside_with_credit_and_plain_fallback() {
        let (rich, fallback) =
            telegram_context_compaction_messages("上下文 <已> & 压缩", "任务 & 继续");

        assert_eq!(
            rich,
            "<aside>上下文 &lt;已&gt; &amp; 压缩<cite>任务 &amp; 继续</cite></aside>"
        );
        assert_eq!(fallback, "上下文 <已> & 压缩\n\n任务 & 继续");
    }

    #[test]
    fn user_message_uses_a_native_quote_with_credit_and_plain_fallback() {
        let (rich, fallback) = telegram_user_message_messages(
            "**打开** `<draft>` & [文档](https://example.com)",
            "你 & 我 · Codex 电脑端",
        );

        assert_eq!(
            rich,
            "<blockquote><b>打开</b> <code>&lt;draft&gt;</code> &amp; <a href=\"https://example.com\">文档</a>\n<cite>你 &amp; 我 · Codex 电脑端</cite></blockquote>"
        );
        assert_eq!(
            fallback,
            "**打开** `<draft>` & [文档](https://example.com)\n\n你 & 我 · Codex 电脑端"
        );
        assert!(!fallback.contains('🤖'));
        assert!(!fallback.contains('👤'));
    }

    #[test]
    fn long_user_message_chunks_preserve_text_and_credit_each_quote() {
        let source = "界".repeat(4_500);
        let credit = "你 · Codex 电脑端";
        let chunks = telegram_user_message_chunks(&source, credit, "(continues...)", "(continued)");

        assert!(chunks.len() > 1);
        let restored = chunks
            .iter()
            .map(|chunk| {
                chunk
                    .strip_prefix("(continued)\n\n")
                    .unwrap_or(chunk)
                    .strip_suffix("\n\n(continues...)")
                    .unwrap_or_else(|| chunk.strip_prefix("(continued)\n\n").unwrap_or(chunk))
            })
            .collect::<String>();
        assert_eq!(restored, source);
        for chunk in chunks {
            let (rich, fallback) = telegram_user_message_messages(&chunk, credit);
            assert_eq!(rich.matches("<cite>").count(), 1);
            assert!(fallback.chars().count() <= TELEGRAM_MAX_MESSAGE_CHARS);
        }
    }

    #[test]
    fn resolved_approval_text_replaces_pending_instructions() {
        let approval = PendingApproval {
            request_id: json!("request-1"),
            request_kind: "commandExecution".to_string(),
            method: "item/commandExecution/requestApproval".to_string(),
            params: json!({}),
            summary: "Run `cargo test`".to_string(),
            decisions: vec![ApprovalDecisionOption {
                label: "Allow".to_string(),
                decision: json!("approved"),
            }],
            message_id: Some("77".to_string()),
            remote_client_key: Some("client".to_string()),
        };

        let text = resolved_approval_text(&approval, "Allow", ImText::zh_cn());

        assert!(text.contains("审批已处理"));
        assert!(text.contains("已选择：允许"));
        assert!(text.contains("Run `cargo test`"));
        assert!(!text.contains("回复 /1 处理"));
    }

    #[test]
    fn approval_text_uses_localized_sections_and_fallback_commands() {
        let approval = PendingApproval {
            request_id: json!("request-1"),
            request_kind: "command".to_string(),
            method: "item/commandExecution/requestApproval".to_string(),
            params: json!({}),
            summary: "Run `cargo test`".to_string(),
            decisions: vec![ApprovalDecisionOption {
                label: "Yes, just this once".to_string(),
                decision: json!("approved"),
            }],
            message_id: None,
            remote_client_key: None,
        };

        let text = approval_text(&approval, ImText::zh_cn());

        assert!(text.starts_with("**审批待处理**\n类型：`命令执行`"));
        assert!(text.contains("**请求内容**"));
        assert!(!text.contains("可选操作"));
        assert!(text.contains("点击下方按钮处理。"));
        assert!(!text.contains("request_kind"));
    }

    #[test]
    fn approval_keyboard_localizes_labels_without_changing_callback_shape() {
        let approval = PendingApproval {
            request_id: json!("request-1"),
            request_kind: "command".to_string(),
            method: "item/commandExecution/requestApproval".to_string(),
            params: json!({}),
            summary: "Run `cargo test`".to_string(),
            decisions: vec![ApprovalDecisionOption {
                label: "Yes, just this once".to_string(),
                decision: json!("approved"),
            }],
            message_id: None,
            remote_client_key: None,
        };

        let keyboard = approval_keyboard(&approval, ImText::zh_cn()).expect("keyboard");
        let button = &keyboard["inline_keyboard"][0][0];
        assert_eq!(button["text"], "仅本次允许");
        assert!(
            button["callback_data"]
                .as_str()
                .is_some_and(|value| { value.starts_with("ap:") && value.ends_with(":1") })
        );
    }

    #[test]
    fn thread_settings_editor_uses_revision_callbacks_without_numeric_reply_hints() {
        let request = TelegramModelSwitchRequestState {
            request_id: "thread-model-7".to_string(),
            conversation_key: "telegram:bot:chat".to_string(),
            account_id: "bot".to_string(),
            chat_id: "chat".to_string(),
            expected_thread_id: "thread-7".to_string(),
            remote_client_key: "im:telegram:bot:chat".to_string(),
            catalog: vec![TelegramThreadSettingsModelChoice {
                model: "gpt-primary".to_string(),
                label: "GPT <Primary>".to_string(),
                supported_efforts: vec!["medium".to_string()],
                default_effort: Some("medium".to_string()),
                supports_fast: true,
            }],
            observed: ThreadSettingsSnapshot {
                model: ObservedSetting::Known(Some("gpt-primary".to_string())),
                effort: ObservedSetting::Known(Some("medium".to_string())),
                service_tier: ObservedSetting::Known(None),
            },
            draft: TelegramThreadSettingsDraft {
                speed: Some(TelegramThreadSettingsSpeed::Fast),
                ..Default::default()
            },
            revision: 9,
            expires_at_ms: 1,
            stage: TelegramThreadSettingsStage::Overview,
            model_page: 1,
            compatibility: None,
            pending_apply: None,
            stale: false,
            message_id: None,
        };
        let text = ImText::zh_cn();
        let keyboard = thread_settings_keyboard(&request, text);

        assert_eq!(
            keyboard["inline_keyboard"][0][0]["callback_data"],
            "tmo:thread-model-7:9:model"
        );
        assert_eq!(
            keyboard["inline_keyboard"][0][1]["callback_data"],
            "tmo:thread-model-7:9:effort"
        );
        assert_eq!(
            keyboard["inline_keyboard"][1][0]["callback_data"],
            "tmo:thread-model-7:9:speed"
        );
        assert_eq!(
            keyboard["inline_keyboard"][2][1]["callback_data"],
            "tma:thread-model-7:9"
        );

        let rendered = thread_settings_html(&request, text);
        assert!(rendered.contains("<b>已生效</b>"));
        assert!(rendered.contains("<b>待应用</b>"));
        assert!(rendered.contains("快速"));
        assert!(!rendered.contains("/1"));
    }

    #[test]
    fn approval_text_keeps_plain_reply_commands_for_rich_text_fallback() {
        let approval = PendingApproval {
            request_id: json!("request-1"),
            request_kind: "command".to_string(),
            method: "item/commandExecution/requestApproval".to_string(),
            params: json!({}),
            summary: "Run `cargo test`".to_string(),
            decisions: vec![ApprovalDecisionOption {
                label: "Yes, proceed".to_string(),
                decision: json!("approved"),
            }],
            message_id: None,
            remote_client_key: None,
        };

        let plain = approval_text(&approval, ImText::zh_cn());
        let rich = telegram_markdown_to_html(&plain);

        assert!(!plain.contains("`/1`"));
        assert!(plain.contains("点击下方按钮处理。"));
        assert!(rich.contains("<b>审批待处理</b>"));
    }

    #[test]
    fn approval_cards_bound_oversized_summaries_to_one_message() {
        let approval = PendingApproval {
            request_id: json!("request-1"),
            request_kind: "command".to_string(),
            method: "item/commandExecution/requestApproval".to_string(),
            params: json!({}),
            summary: "x".repeat(8_000),
            decisions: vec![ApprovalDecisionOption {
                label: "Yes, proceed".to_string(),
                decision: json!("approved"),
            }],
            message_id: None,
            remote_client_key: None,
        };

        let pending = approval_text(&approval, ImText::zh_cn());
        let resolved = resolved_approval_text(&approval, "Yes, proceed", ImText::zh_cn());

        assert!(pending.chars().count() <= TELEGRAM_MAX_MESSAGE_CHARS);
        assert!(resolved.chars().count() <= TELEGRAM_MAX_MESSAGE_CHARS);
        assert!(pending.contains('…'));
        assert!(resolved.contains('…'));
    }

    #[test]
    fn approval_cards_with_many_options_stay_on_one_editable_message() {
        let approval = PendingApproval {
            request_id: json!("request-many-options"),
            request_kind: "command".to_string(),
            method: "item/commandExecution/requestApproval".to_string(),
            params: json!({}),
            summary: "Run a command".to_string(),
            decisions: (0..600)
                .map(|index| ApprovalDecisionOption {
                    label: format!("Option {index}: allow this command for this session"),
                    decision: json!("approved"),
                })
                .collect(),
            message_id: None,
            remote_client_key: None,
        };

        let pending = approval_text(&approval, ImText::zh_cn());
        assert!(pending.chars().count() <= TELEGRAM_MAX_MESSAGE_CHARS);
        assert_eq!(
            telegram_text_chunks(&pending, "(continues...)", "(continued)").len(),
            1
        );
        assert!(pending.contains("点击下方按钮处理。"));
        assert!(!pending.contains("`/1`"));
        assert!(!pending.contains("`/600`"));
        assert!(!pending.contains("Option 599"));
    }

    #[test]
    fn approval_text_trims_oversized_summary_to_fit_one_message() {
        let approval = PendingApproval {
            request_id: json!("request-summary-and-options"),
            request_kind: "command".to_string(),
            method: "item/commandExecution/requestApproval".to_string(),
            params: json!({}),
            summary: "x".repeat(8_000),
            decisions: vec![ApprovalDecisionOption {
                label: "Yes, proceed".to_string(),
                decision: json!("approved"),
            }],
            message_id: None,
            remote_client_key: None,
        };

        let pending = approval_text(&approval, ImText::zh_cn());
        assert!(pending.chars().count() <= TELEGRAM_MAX_MESSAGE_CHARS);
        assert!(pending.contains("请求内容"));
        assert!(pending.contains('…'));
        assert!(pending.contains("点击下方按钮处理。"));
    }

    #[test]
    fn create_options_keyboard_gives_each_option_a_button() {
        let options = vec![
            ThreadCreateOption {
                label: "使用 Codex App 当前权限".to_string(),
                summary: Some("已选".to_string()),
            },
            ThreadCreateOption {
                label: "默认权限".to_string(),
                summary: Some("适合常规项目，需要时由用户确认。".to_string()),
            },
        ];
        let keyboard = create_options_keyboard(
            "thread-7",
            "permission",
            1,
            &options,
            false,
            true,
            ImText::zh_cn(),
        );
        let rows = keyboard["inline_keyboard"]
            .as_array()
            .expect("keyboard rows");

        let first = rows[0][0].as_object().expect("option button");
        assert_eq!(first["text"], "使用 Codex App 当前权限");
        assert_eq!(first["callback_data"], "tcs:thread-7:permission:1:0");
        let second = rows[1][0].as_object().expect("option button");
        assert_eq!(second["callback_data"], "tcs:thread-7:permission:1:1");
        let nav = rows[2][0].as_object().expect("nav button");
        assert_eq!(nav["callback_data"], "tcp:thread-7:permission:next");
        let back = rows[3][0].as_object().expect("back button");
        assert_eq!(back["callback_data"], "trc:thread-7:new");

        // cwd 字段额外提供自定义入口按钮。
        let cwd_keyboard = create_options_keyboard(
            "thread-7",
            "cwd",
            1,
            &options,
            false,
            false,
            ImText::zh_cn(),
        );
        let cwd_rows = cwd_keyboard["inline_keyboard"].as_array().expect("rows");
        let custom = cwd_rows[2][0].as_object().expect("custom cwd button");
        assert_eq!(custom["callback_data"], "tcv:thread-7:cwd:__custom__");
    }

    #[test]
    fn empty_keyboard_removes_all_inline_buttons() {
        assert_eq!(empty_inline_keyboard(), json!({ "inline_keyboard": [] }));
    }

    #[test]
    fn code_fence_language_annotates_html() {
        let html = telegram_markdown_to_html("```rust\nfn main() {}\n```");
        assert!(html.contains("<pre><code class=\"language-rust\">"));
        assert!(html.contains("</code></pre>"));

        let plain = telegram_markdown_to_html("```\nplain\n```");
        assert!(plain.contains("<pre><code>plain"));

        // 语言标注只保留安全字符，防止围栏行注入属性。
        let hostile = telegram_markdown_to_html("```rust onclick=alert(1)\nfn main() {}\n```");
        assert!(hostile.contains("language-rust"));
        assert!(!hostile.contains("onclick"));
    }

    #[test]
    fn chunks_rebalance_code_fences_across_segments() {
        let mut text = String::from("介绍\n```rust\n");
        for i in 0..400 {
            text.push_str(&format!("let value_{i} = {i}; // 注释内容\n"));
        }
        text.push_str("```\n结尾");

        let chunks = chunks_en(&text);
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            let fence_lines = chunk
                .lines()
                .filter(|line| line.trim_start().starts_with("```"))
                .count();
            assert_eq!(fence_lines % 2, 0, "chunk fences unbalanced:\n{chunk}");
            let html = telegram_markdown_to_html(chunk);
            assert_eq!(
                html.matches("<pre><code").count(),
                html.matches("</code></pre>").count()
            );
        }
        // 续段自动补回围栏和语言标注。
        assert!(chunks[1].contains("```rust"));
        assert!(telegram_markdown_to_html(&chunks[1]).contains("language-rust"));
    }

    #[test]
    fn chunk_markers_follow_adapter_locale() {
        let zh = TelegramAdapter::new(TelegramApi::new(TelegramSettings::default()));
        assert_eq!(zh.chunk_markers(), ("（未完待续）", "（接上文）"));
        let en = TelegramAdapter::with_locale(
            TelegramApi::new(TelegramSettings::default()),
            crate::im::core::i18n::ImLocale::EnUs,
        );
        assert_eq!(en.chunk_markers(), ("(continues...)", "(continued)"));
    }

    #[tokio::test]
    async fn turn_completed_long_output_sends_document() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock Telegram server");
        let address = listener.local_addr().expect("mock Telegram address");
        let captured = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
        let captured_task = captured.clone();
        tokio::spawn(async move {
            loop {
                if let Ok((mut stream, _)) = listener.accept().await {
                    let mut buf = vec![0_u8; 65_536];
                    let count = stream.read(&mut buf).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buf[..count]);
                    let line = request.lines().next().unwrap_or("").to_string();
                    if let Ok(mut slot) = captured_task.lock() {
                        slot.get_or_insert(line);
                    }
                    let body = r#"{"ok":true,"result":{"message_id":77,"chat":{"id":42,"type":"private"}}}"#;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                }
            }
        });
        let api = TelegramApi::new(TelegramSettings {
            account_id: "tg_1".to_string(),
            bot_token: "test-token".to_string(),
            ..Default::default()
        })
        .with_test_api_base(format!("http://{address}"));
        let adapter = TelegramAdapter::new(api);
        let reply = "任务".repeat(4300);

        let result = adapter
            .send_turn_completed("42", &reply, "你 · Codex 电脑端", None)
            .await
            .expect("send long turn output");

        assert_eq!(result, "77");
        let line = captured
            .lock()
            .expect("capture lock")
            .clone()
            .unwrap_or_default();
        assert!(line.contains("sendDocument"), "request line: {line}");
    }
}
