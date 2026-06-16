//! Responses API input → Chat Completions messages 转换。
//! 参考 AxonHub `responses/inbound.go` 的 `convertInputToMessages`。

use serde_json::{Value, json};

use crate::ai_gateway::model::{
    GatewayRequest, ItemContent, ItemType, Reasoning, ResponseItem, TextFormat,
};

/// Chat Completions 请求 body（JSON）。
pub fn build_chat_request(request: &GatewayRequest, deepseek_mode: bool) -> Result<Value, String> {
    let mut messages = Vec::new();

    // 1. instructions → system message
    if let Some(instructions) = &request.instructions {
        messages.push(json!({
            "role": "system",
            "content": instructions,
        }));
    }

    // 2. input items → messages
    convert_input_to_messages(&request.input, &mut messages, deepseek_mode)?;

    // 3. 构建请求 body
    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "stream": request.stream,
    });

    // stream_options: 流式时请求 usage
    if request.stream {
        body["stream_options"] = json!({"include_usage": true});
    }

    // 4. tools
    if !request.tools.is_empty() {
        let chat_tools: Vec<Value> = request
            .tools
            .iter()
            .filter(|t| {
                t.get("type")
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| s == "function")
            })
            .cloned()
            .collect();
        if !chat_tools.is_empty() {
            body["tools"] = json!(chat_tools);
        }
    }

    // 5. tool_choice
    if let Some(tc) = &request.tool_choice {
        body["tool_choice"] = tc.clone();
    }

    // 6. temperature / top_p
    if let Some(t) = request.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(p) = request.top_p {
        body["top_p"] = json!(p);
    }

    // 7. max_output_tokens → max_tokens
    if let Some(max) = request.max_output_tokens {
        body["max_tokens"] = json!(max);
    }

    // 8. reasoning → thinking (DeepSeek) / reasoning_effort (OpenAI Chat)
    if let Some(reasoning) = &request.reasoning {
        apply_reasoning(&mut body, reasoning, deepseek_mode);
    }

    // 9. text.format → response_format
    if let Some(text) = &request.text {
        if let Some(format) = &text.format {
            apply_response_format(&mut body, format, deepseek_mode);
        }
    }

    // 10. DeepSeek 后处理
    if deepseek_mode {
        let thinking_enabled = body
            .get("thinking")
            .and_then(|t| t.get("type"))
            .and_then(|v| v.as_str())
            == Some("enabled");

        // 10a. developer → system
        normalize_developer_messages(&mut body);

        // 10b. 丢弃仅含 reasoning 无 content/tool_calls 的 assistant message
        drop_invalid_assistant_messages(&mut body);

        if thinking_enabled {
            // 10c. 补空 reasoning_content
            pad_reasoning_content(&mut body);

            // 10d. tool_calls 轮次回填 reasoning_content
            ensure_thinking_tool_call_reasoning_content(&mut body);

            // 10e. thinking 启用时移除无效参数
            body.as_object_mut().map(|m| {
                m.remove("temperature");
                m.remove("top_p");
                m.remove("presence_penalty");
                m.remove("frequency_penalty");
            });
        }
    }

    Ok(body)
}

/// 将 Responses API input items 转换为 Chat messages。
/// 处理 reasoning + function_call 合并等边界情况。
fn convert_input_to_messages(
    items: &[ResponseItem],
    messages: &mut Vec<Value>,
    _deepseek_mode: bool,
) -> Result<(), String> {
    let mut i = 0;
    while i < items.len() {
        let item = &items[i];
        match item.item_type {
            ItemType::InputText => {
                let text = match &item.content {
                    Some(ItemContent::Text(s)) => s.clone(),
                    _ => String::new(),
                };
                messages.push(json!({"role": "user", "content": text}));
                i += 1;
            }
            ItemType::InputImage => {
                let image_url = match &item.content {
                    Some(ItemContent::Text(s)) => s.clone(),
                    Some(ItemContent::Parts(parts)) => parts
                        .iter()
                        .find_map(|p| p.image_url.clone())
                        .unwrap_or_default(),
                    _ => String::new(),
                };
                messages.push(json!({
                    "role": "user",
                    "content": [{
                        "type": "image_url",
                        "image_url": {"url": image_url}
                    }]
                }));
                i += 1;
            }
            ItemType::Message => {
                let role = item.role.as_deref().unwrap_or("user");
                let content = extract_message_content(item);
                messages.push(json!({"role": role, "content": content}));
                i += 1;
            }
            ItemType::Reasoning => {
                // reasoning 后面紧跟 function_call 时，合并为同一个 assistant message
                i = convert_reasoning_with_following(items, i, messages);
            }
            ItemType::FunctionCall => {
                // 连续 function_call 合并到同一个 assistant message
                i = convert_function_calls(items, i, messages);
            }
            ItemType::FunctionCallOutput => {
                let call_id = item.call_id.as_deref().unwrap_or("");
                let output = item
                    .output
                    .as_deref()
                    .ok_or_else(|| "function_call_output missing output".to_string())?;
                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": call_id,
                    "content": output,
                }));
                i += 1;
            }
            ItemType::OutputText | ItemType::Unknown => {
                // 跳过不支持的 item
                i += 1;
            }
        }
    }
    Ok(())
}

/// 处理 reasoning item，检查后面是否紧跟 function_call，如果是则合并。
/// 参考 AxonHub `convertReasoningWithFollowing`。
fn convert_reasoning_with_following(
    items: &[ResponseItem],
    start: usize,
    messages: &mut Vec<Value>,
) -> usize {
    let reasoning_item = &items[start];
    let reasoning_text = extract_reasoning_text(reasoning_item);

    let next = start + 1;
    if next < items.len() && items[next].item_type == ItemType::FunctionCall {
        // 合并 reasoning + function_calls 为单个 assistant message
        let mut tool_calls = Vec::new();
        let mut i = next;
        while i < items.len() && items[i].item_type == ItemType::FunctionCall {
            tool_calls.push(build_tool_call(&items[i], tool_calls.len()));
            i += 1;
        }
        let mut msg = json!({
            "role": "assistant",
            "content": null,
            "tool_calls": tool_calls,
        });
        if !reasoning_text.is_empty() {
            msg["reasoning_content"] = json!(reasoning_text);
        }
        messages.push(msg);
        i
    } else {
        // 独立的 reasoning item → assistant message with reasoning_content
        let mut msg = json!({"role": "assistant", "content": null});
        if !reasoning_text.is_empty() {
            msg["reasoning_content"] = json!(reasoning_text);
        }
        messages.push(msg);
        next
    }
}

/// 连续 function_call 合并到同一个 assistant message。
fn convert_function_calls(
    items: &[ResponseItem],
    start: usize,
    messages: &mut Vec<Value>,
) -> usize {
    let mut tool_calls = Vec::new();
    let mut i = start;
    while i < items.len() && items[i].item_type == ItemType::FunctionCall {
        tool_calls.push(build_tool_call(&items[i], tool_calls.len()));
        i += 1;
    }
    messages.push(json!({
        "role": "assistant",
        "content": null,
        "tool_calls": tool_calls,
    }));
    i
}

fn build_tool_call(item: &ResponseItem, index: usize) -> Value {
    json!({
        "index": index,
        "id": item.call_id.as_deref().unwrap_or(""),
        "type": "function",
        "function": {
            "name": item.name.as_deref().unwrap_or(""),
            "arguments": item.arguments.as_deref().unwrap_or("{}"),
        }
    })
}

fn extract_message_content(item: &ResponseItem) -> Value {
    match &item.content {
        Some(ItemContent::Text(s)) => json!(s),
        Some(ItemContent::Parts(parts)) => {
            let content_parts: Vec<Value> = parts
                .iter()
                .map(|p| {
                    if p.part_type == "output_text" || p.part_type == "text" {
                        json!({"type": "text", "text": p.text.as_deref().unwrap_or("")})
                    } else if p.part_type == "image_url" || p.part_type == "input_image" {
                        json!({"type": "image_url", "image_url": {"url": p.image_url.as_deref().unwrap_or("")}})
                    } else {
                        json!({"type": "text", "text": p.text.as_deref().unwrap_or("")})
                    }
                })
                .collect();
            json!(content_parts)
        }
        None => json!(null),
    }
}

fn extract_reasoning_text(item: &ResponseItem) -> String {
    if let Some(summary) = &item.summary {
        summary
            .iter()
            .map(|p| p.text.as_str())
            .collect::<Vec<_>>()
            .join("")
    } else {
        String::new()
    }
}

/// reasoning 参数处理。
/// DeepSeek: effort 精细映射，参考 axonhub fork dev 分支。
/// OpenAI Chat: reasoning_effort 透传。
fn apply_reasoning(body: &mut Value, reasoning: &Reasoning, deepseek_mode: bool) {
    if deepseek_mode {
        match reasoning.effort.as_deref() {
            Some("none") => {
                body["thinking"] = json!({"type": "disabled"});
                // 不发 reasoning_effort
            }
            Some(effort) => {
                body["thinking"] = json!({"type": "enabled"});
                if let Some(budget) = reasoning.budget_tokens {
                    body["thinking"]["budget_tokens"] = json!(budget);
                }
                // DeepSeek 只接受 high/max，其余映射
                let mapped = normalize_deepseek_effort(effort);
                body["reasoning_effort"] = json!(mapped);
            }
            None => {
                // 无 effort → 启用 thinking，使用 provider 默认
                body["thinking"] = json!({"type": "enabled"});
                if let Some(budget) = reasoning.budget_tokens {
                    body["thinking"]["budget_tokens"] = json!(budget);
                }
            }
        }
    } else {
        if let Some(effort) = &reasoning.effort {
            body["reasoning_effort"] = json!(effort);
        }
    }
}

/// DeepSeek reasoning effort 映射：low/medium/minimal → high, xhigh → max, 其余保留。
fn normalize_deepseek_effort(effort: &str) -> &str {
    match effort {
        "low" | "medium" | "minimal" => "high",
        "xhigh" => "max",
        other => other, // "high", "max" 等直接透传
    }
}

/// text.format → response_format。
/// DeepSeek: json_schema → 降级为 json_object。
fn apply_response_format(body: &mut Value, format: &TextFormat, deepseek_mode: bool) {
    match format.format_type.as_str() {
        "json_schema" => {
            if deepseek_mode {
                // DeepSeek 不支持 json_schema，降级为 json_object
                body["response_format"] = json!({"type": "json_object"});
            } else {
                let mut rf = json!({"type": "json_schema"});
                if let Some(schema) = &format.schema {
                    rf["json_schema"] = json!({
                        "schema": schema,
                        "name": format.name.as_deref().unwrap_or("response"),
                    });
                }
                body["response_format"] = rf;
            }
        }
        "json_object" => {
            body["response_format"] = json!({"type": "json_object"});
        }
        _ => {}
    }
}

/// DeepSeek: thinking 启用时，所有 assistant message 缺少 reasoning_content 的补空字符串。
/// 参考 AxonHub `deepseek/outbound.go`。
fn pad_reasoning_content(body: &mut Value) {
    if let Some(messages) = body.get_mut("messages").and_then(|m| m.as_array_mut()) {
        for msg in messages.iter_mut() {
            if msg.get("role").and_then(|v| v.as_str()) == Some("assistant")
                && msg.get("reasoning_content").is_none()
            {
                msg["reasoning_content"] = json!("");
            }
        }
    }
}

/// DeepSeek: 有 tool_calls 的 assistant message 必须回传 reasoning_content。
/// 如果缺失，从前一个有 reasoning_content 的 assistant message 回填。
/// 参考 axonhub fork `ensureThinkingToolCallReasoningContent`。
fn ensure_thinking_tool_call_reasoning_content(body: &mut Value) {
    let messages = match body.get_mut("messages").and_then(|m| m.as_array_mut()) {
        Some(m) => m,
        None => return,
    };

    let mut last_reasoning_content: Option<String> = None;

    for msg in messages.iter_mut() {
        if msg.get("role").and_then(|v| v.as_str()) != Some("assistant") {
            continue;
        }

        // 记录最近的 reasoning_content
        if let Some(rc) = msg.get("reasoning_content").and_then(|v| v.as_str()) {
            if !rc.is_empty() {
                last_reasoning_content = Some(rc.to_string());
            }
        }

        // 有 tool_calls 但缺少 reasoning_content 时，从前一个回填
        let has_tool_calls = msg
            .get("tool_calls")
            .and_then(|v| v.as_array())
            .is_some_and(|a| !a.is_empty());
        let has_reasoning = msg
            .get("reasoning_content")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty());

        if has_tool_calls && !has_reasoning {
            if let Some(rc) = &last_reasoning_content {
                msg["reasoning_content"] = json!(rc);
            }
        }
    }
}

/// DeepSeek: developer role → system role。
fn normalize_developer_messages(body: &mut Value) {
    if let Some(messages) = body.get_mut("messages").and_then(|m| m.as_array_mut()) {
        for msg in messages.iter_mut() {
            if msg.get("role").and_then(|v| v.as_str()) == Some("developer") {
                msg["role"] = json!("system");
            }
        }
    }
}

/// DeepSeek: 丢弃仅含 reasoning 但无 content 且无 tool_calls 的 assistant message。
/// 这种消息会导致 DeepSeek API 报错。
/// 参考 axonhub fork `shouldDropInvalidAssistantMessage`。
fn drop_invalid_assistant_messages(body: &mut Value) {
    if let Some(messages) = body.get_mut("messages").and_then(|m| m.as_array_mut()) {
        messages.retain(|msg| {
            if msg.get("role").and_then(|v| v.as_str()) != Some("assistant") {
                return true;
            }
            let has_content = msg
                .get("content")
                .is_some_and(|v| !v.is_null() && v.as_str() != Some(""));
            let has_tool_calls = msg
                .get("tool_calls")
                .and_then(|v| v.as_array())
                .is_some_and(|a| !a.is_empty());
            // 有 content 或有 tool_calls → 保留，否则丢弃
            has_content || has_tool_calls
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_gateway::model::*;

    fn make_request(input: Vec<ResponseItem>) -> GatewayRequest {
        GatewayRequest {
            model: "deepseek-chat".into(),
            instructions: None,
            input,
            tools: vec![],
            tool_choice: None,
            reasoning: None,
            text: None,
            stream: false,
            max_output_tokens: None,
            temperature: None,
            top_p: None,
            prompt_cache_key: None,
            prompt_cache_retention: None,
            previous_response_id: None,
        }
    }

    fn make_item(item_type: ItemType) -> ResponseItem {
        ResponseItem {
            item_type,
            id: None,
            role: None,
            content: None,
            name: None,
            call_id: None,
            arguments: None,
            output: None,
            status: None,
            summary: None,
            encrypted_content: None,
        }
    }

    // ─── input_text ────────────────────────────────────────────

    #[test]
    fn test_input_text_to_user_message() {
        let mut item = make_item(ItemType::InputText);
        item.content = Some(ItemContent::Text("hello".into()));
        let req = make_request(vec![item]);
        let body = build_chat_request(&req, false).unwrap();
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["content"], "hello");
    }

    // ─── instructions → system ─────────────────────────────────

    #[test]
    fn test_instructions_to_system_message() {
        let mut req = make_request(vec![]);
        req.instructions = Some("You are helpful.".into());
        let body = build_chat_request(&req, false).unwrap();
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[0]["content"], "You are helpful.");
    }

    // ─── message items ─────────────────────────────────────────

    #[test]
    fn test_message_item_user_and_assistant() {
        let mut user = make_item(ItemType::Message);
        user.role = Some("user".into());
        user.content = Some(ItemContent::Text("hi".into()));

        let mut asst = make_item(ItemType::Message);
        asst.role = Some("assistant".into());
        asst.content = Some(ItemContent::Text("hello".into()));

        let req = make_request(vec![user, asst]);
        let body = build_chat_request(&req, false).unwrap();
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[1]["role"], "assistant");
    }

    // ─── function_call → tool_calls ────────────────────────────

    #[test]
    fn test_function_calls_merged() {
        let mut fc1 = make_item(ItemType::FunctionCall);
        fc1.call_id = Some("call_1".into());
        fc1.name = Some("get_weather".into());
        fc1.arguments = Some(r#"{"city":"NYC"}"#.into());

        let mut fc2 = make_item(ItemType::FunctionCall);
        fc2.call_id = Some("call_2".into());
        fc2.name = Some("get_time".into());
        fc2.arguments = Some(r#"{}"#.into());

        let req = make_request(vec![fc1, fc2]);
        let body = build_chat_request(&req, false).unwrap();
        let msgs = body["messages"].as_array().unwrap();
        // 两个连续 function_call 应合并到同一个 assistant message
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "assistant");
        let tcs = msgs[0]["tool_calls"].as_array().unwrap();
        assert_eq!(tcs.len(), 2);
        assert_eq!(tcs[0]["function"]["name"], "get_weather");
        assert_eq!(tcs[1]["function"]["name"], "get_time");
    }

    // ─── function_call_output → tool message ───────────────────

    #[test]
    fn test_function_call_output_to_tool_message() {
        let mut fco = make_item(ItemType::FunctionCallOutput);
        fco.call_id = Some("call_1".into());
        fco.output = Some("sunny".into());

        let req = make_request(vec![fco]);
        let body = build_chat_request(&req, false).unwrap();
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "tool");
        assert_eq!(msgs[0]["tool_call_id"], "call_1");
        assert_eq!(msgs[0]["content"], "sunny");
    }

    #[test]
    fn test_function_call_output_missing_output_errors() {
        let fco = make_item(ItemType::FunctionCallOutput);
        // output is None → should error
        let req = make_request(vec![fco]);
        assert!(build_chat_request(&req, false).is_err());
    }

    // ─── reasoning + function_call 合并 ────────────────────────

    #[test]
    fn test_reasoning_followed_by_function_call_merged() {
        let mut reasoning = make_item(ItemType::Reasoning);
        reasoning.summary = Some(vec![SummaryPart {
            part_type: "summary_text".into(),
            text: "I should call the tool".into(),
        }]);

        let mut fc = make_item(ItemType::FunctionCall);
        fc.call_id = Some("call_1".into());
        fc.name = Some("search".into());
        fc.arguments = Some(r#"{"q":"rust"}"#.into());

        let req = make_request(vec![reasoning, fc]);
        let body = build_chat_request(&req, false).unwrap();
        let msgs = body["messages"].as_array().unwrap();
        // 应合并为一个 assistant message
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "assistant");
        assert_eq!(msgs[0]["reasoning_content"], "I should call the tool");
        assert!(msgs[0]["tool_calls"].as_array().unwrap().len() == 1);
    }

    #[test]
    fn test_standalone_reasoning() {
        let mut reasoning = make_item(ItemType::Reasoning);
        reasoning.summary = Some(vec![SummaryPart {
            part_type: "summary_text".into(),
            text: "thinking...".into(),
        }]);

        // 后面不跟 function_call
        let mut user = make_item(ItemType::InputText);
        user.content = Some(ItemContent::Text("next".into()));

        let req = make_request(vec![reasoning, user]);
        let body = build_chat_request(&req, false).unwrap();
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "assistant");
        assert_eq!(msgs[0]["reasoning_content"], "thinking...");
        assert!(msgs[0].get("tool_calls").is_none());
        assert_eq!(msgs[1]["role"], "user");
    }

    // ─── DeepSeek reasoning ────────────────────────────────────

    #[test]
    fn test_deepseek_effort_none_disables_thinking() {
        let mut req = make_request(vec![]);
        req.reasoning = Some(Reasoning {
            effort: Some("none".into()),
            budget_tokens: None,
            generate_summary: None,
        });
        let body = build_chat_request(&req, true).unwrap();
        assert_eq!(body["thinking"]["type"], "disabled");
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn test_deepseek_effort_high_enables_thinking() {
        let mut req = make_request(vec![]);
        req.reasoning = Some(Reasoning {
            effort: Some("high".into()),
            budget_tokens: Some(4096),
            generate_summary: None,
        });
        let body = build_chat_request(&req, true).unwrap();
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 4096);
        assert_eq!(body["reasoning_effort"], "high");
    }

    // ─── DeepSeek json_schema 降级 ─────────────────────────────

    #[test]
    fn test_deepseek_json_schema_downgrade() {
        let mut req = make_request(vec![]);
        req.text = Some(TextOptions {
            format: Some(TextFormat {
                format_type: "json_schema".into(),
                schema: Some(json!({"type": "object"})),
                name: Some("test".into()),
            }),
        });
        let body = build_chat_request(&req, true).unwrap();
        assert_eq!(body["response_format"]["type"], "json_object");
    }

    #[test]
    fn test_openai_json_schema_preserved() {
        let mut req = make_request(vec![]);
        req.text = Some(TextOptions {
            format: Some(TextFormat {
                format_type: "json_schema".into(),
                schema: Some(json!({"type": "object"})),
                name: Some("test".into()),
            }),
        });
        let body = build_chat_request(&req, false).unwrap();
        assert_eq!(body["response_format"]["type"], "json_schema");
    }

    // ─── DeepSeek reasoning_content 补空 ───────────────────────

    #[test]
    fn test_deepseek_pads_reasoning_content() {
        let mut asst = make_item(ItemType::Message);
        asst.role = Some("assistant".into());
        asst.content = Some(ItemContent::Text("hi".into()));

        let mut req = make_request(vec![asst]);
        req.reasoning = Some(Reasoning {
            effort: Some("high".into()),
            budget_tokens: None,
            generate_summary: None,
        });
        let body = build_chat_request(&req, true).unwrap();
        let msgs = body["messages"].as_array().unwrap();
        // assistant message 应该被补上 reasoning_content=""
        assert_eq!(msgs[0]["reasoning_content"], "");
    }

    // ─── stream_options ────────────────────────────────────────

    #[test]
    fn test_stream_includes_usage_option() {
        let mut req = make_request(vec![]);
        req.stream = true;
        let body = build_chat_request(&req, false).unwrap();
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
    }

    // ─── 完整多轮对话 ──────────────────────────────────────────

    #[test]
    fn test_full_multiturn_conversation() {
        let mut user1 = make_item(ItemType::InputText);
        user1.content = Some(ItemContent::Text("What's the weather?".into()));

        let mut fc = make_item(ItemType::FunctionCall);
        fc.call_id = Some("call_w".into());
        fc.name = Some("get_weather".into());
        fc.arguments = Some(r#"{"city":"SF"}"#.into());

        let mut fco = make_item(ItemType::FunctionCallOutput);
        fco.call_id = Some("call_w".into());
        fco.output = Some("72°F sunny".into());

        let mut asst = make_item(ItemType::Message);
        asst.role = Some("assistant".into());
        asst.content = Some(ItemContent::Text("It's 72°F and sunny in SF.".into()));

        let mut user2 = make_item(ItemType::InputText);
        user2.content = Some(ItemContent::Text("Thanks!".into()));

        let mut req = make_request(vec![user1, fc, fco, asst, user2]);
        req.instructions = Some("You are a weather assistant.".into());
        let body = build_chat_request(&req, false).unwrap();
        let msgs = body["messages"].as_array().unwrap();
        // system, user1, fc→assistant, fco→tool, asst→assistant, user2→user = 6
        assert_eq!(msgs.len(), 6);
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[1]["role"], "user");
        assert_eq!(msgs[2]["role"], "assistant"); // function_call
        assert_eq!(msgs[2]["tool_calls"].as_array().unwrap().len(), 1);
        assert_eq!(msgs[3]["role"], "tool");
        assert_eq!(msgs[4]["role"], "assistant");
        assert_eq!(msgs[5]["role"], "user");
    }

    // ─── 多工具并行调用 + 结果回填 ─────────────────────────────

    #[test]
    fn test_parallel_tool_calls_and_outputs() {
        // user → 2 个 function_call → 2 个 function_call_output → assistant 回答
        let mut user = make_item(ItemType::InputText);
        user.content = Some(ItemContent::Text(
            "What's the weather in NYC and SF?".into(),
        ));

        let mut fc1 = make_item(ItemType::FunctionCall);
        fc1.call_id = Some("call_1".into());
        fc1.name = Some("get_weather".into());
        fc1.arguments = Some(r#"{"city":"NYC"}"#.into());

        let mut fc2 = make_item(ItemType::FunctionCall);
        fc2.call_id = Some("call_2".into());
        fc2.name = Some("get_weather".into());
        fc2.arguments = Some(r#"{"city":"SF"}"#.into());

        let mut fco1 = make_item(ItemType::FunctionCallOutput);
        fco1.call_id = Some("call_1".into());
        fco1.output = Some("72°F sunny".into());

        let mut fco2 = make_item(ItemType::FunctionCallOutput);
        fco2.call_id = Some("call_2".into());
        fco2.output = Some("65°F foggy".into());

        let mut asst = make_item(ItemType::Message);
        asst.role = Some("assistant".into());
        asst.content = Some(ItemContent::Text("NYC is 72°F, SF is 65°F.".into()));

        let req = make_request(vec![user, fc1, fc2, fco1, fco2, asst]);
        let body = build_chat_request(&req, false).unwrap();
        let msgs = body["messages"].as_array().unwrap();

        // user, assistant(2 tool_calls), tool, tool, assistant = 5
        assert_eq!(msgs.len(), 5);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[1]["role"], "assistant");
        let tcs = msgs[1]["tool_calls"].as_array().unwrap();
        assert_eq!(tcs.len(), 2);
        assert_eq!(tcs[0]["function"]["arguments"], r#"{"city":"NYC"}"#);
        assert_eq!(tcs[1]["function"]["arguments"], r#"{"city":"SF"}"#);
        assert_eq!(msgs[2]["role"], "tool");
        assert_eq!(msgs[2]["tool_call_id"], "call_1");
        assert_eq!(msgs[3]["role"], "tool");
        assert_eq!(msgs[3]["tool_call_id"], "call_2");
        assert_eq!(msgs[4]["role"], "assistant");
        assert_eq!(msgs[4]["content"], "NYC is 72°F, SF is 65°F.");
    }

    // ─── reasoning + 多工具调用 + 结果 + 继续回答 ──────────────

    #[test]
    fn test_reasoning_multi_tool_call_full_loop() {
        let mut reasoning = make_item(ItemType::Reasoning);
        reasoning.summary = Some(vec![SummaryPart {
            part_type: "summary_text".into(),
            text: "I need both weather and time.".into(),
        }]);

        let mut fc1 = make_item(ItemType::FunctionCall);
        fc1.call_id = Some("call_w".into());
        fc1.name = Some("get_weather".into());
        fc1.arguments = Some(r#"{"city":"NYC"}"#.into());

        let mut fc2 = make_item(ItemType::FunctionCall);
        fc2.call_id = Some("call_t".into());
        fc2.name = Some("get_time".into());
        fc2.arguments = Some(r#"{"tz":"EST"}"#.into());

        let mut fco1 = make_item(ItemType::FunctionCallOutput);
        fco1.call_id = Some("call_w".into());
        fco1.output = Some("72°F".into());

        let mut fco2 = make_item(ItemType::FunctionCallOutput);
        fco2.call_id = Some("call_t".into());
        fco2.output = Some("3:00 PM".into());

        let mut asst = make_item(ItemType::Message);
        asst.role = Some("assistant".into());
        asst.content = Some(ItemContent::Text("NYC: 72°F at 3:00 PM.".into()));

        let req = make_request(vec![reasoning, fc1, fc2, fco1, fco2, asst]);
        let body = build_chat_request(&req, false).unwrap();
        let msgs = body["messages"].as_array().unwrap();

        // reasoning+fc1+fc2 → 1 assistant msg, fco1 → tool, fco2 → tool, asst → assistant = 4
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[0]["role"], "assistant");
        assert_eq!(
            msgs[0]["reasoning_content"],
            "I need both weather and time."
        );
        let tcs = msgs[0]["tool_calls"].as_array().unwrap();
        assert_eq!(tcs.len(), 2);
        assert_eq!(msgs[1]["role"], "tool");
        assert_eq!(msgs[2]["role"], "tool");
        assert_eq!(msgs[3]["role"], "assistant");
    }

    // ─── 工具调用链：第一轮工具 → 回答 → 第二轮工具 ────────────

    #[test]
    fn test_chained_tool_calls_across_turns() {
        let mut user = make_item(ItemType::InputText);
        user.content = Some(ItemContent::Text("Plan my trip".into()));

        // 第一轮工具调用
        let mut fc1 = make_item(ItemType::FunctionCall);
        fc1.call_id = Some("call_flight".into());
        fc1.name = Some("search_flights".into());
        fc1.arguments = Some(r#"{"from":"SFO","to":"JFK"}"#.into());

        let mut fco1 = make_item(ItemType::FunctionCallOutput);
        fco1.call_id = Some("call_flight".into());
        fco1.output = Some("Flight AA123 $299".into());

        // 第二轮工具调用
        let mut fc2 = make_item(ItemType::FunctionCall);
        fc2.call_id = Some("call_hotel".into());
        fc2.name = Some("search_hotels".into());
        fc2.arguments = Some(r#"{"city":"NYC"}"#.into());

        let mut fco2 = make_item(ItemType::FunctionCallOutput);
        fco2.call_id = Some("call_hotel".into());
        fco2.output = Some("Hotel Lux $150/night".into());

        let mut asst = make_item(ItemType::Message);
        asst.role = Some("assistant".into());
        asst.content = Some(ItemContent::Text("Found flight and hotel.".into()));

        let req = make_request(vec![user, fc1, fco1, fc2, fco2, asst]);
        let body = build_chat_request(&req, false).unwrap();
        let msgs = body["messages"].as_array().unwrap();

        // user, assistant(fc1), tool(fco1), assistant(fc2), tool(fco2), assistant = 6
        assert_eq!(msgs.len(), 6);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[1]["role"], "assistant");
        assert_eq!(
            msgs[1]["tool_calls"].as_array().unwrap()[0]["function"]["name"],
            "search_flights"
        );
        assert_eq!(msgs[2]["role"], "tool");
        assert_eq!(msgs[3]["role"], "assistant");
        assert_eq!(
            msgs[3]["tool_calls"].as_array().unwrap()[0]["function"]["name"],
            "search_hotels"
        );
        assert_eq!(msgs[4]["role"], "tool");
        assert_eq!(msgs[5]["role"], "assistant");
    }

    // ─── DeepSeek 工具调用 + reasoning padding ─────────────────

    #[test]
    fn test_deepseek_tool_calls_with_reasoning_padding() {
        let mut fc = make_item(ItemType::FunctionCall);
        fc.call_id = Some("call_1".into());
        fc.name = Some("calc".into());
        fc.arguments = Some(r#"{"expr":"1+1"}"#.into());

        let mut fco = make_item(ItemType::FunctionCallOutput);
        fco.call_id = Some("call_1".into());
        fco.output = Some("2".into());

        let mut asst = make_item(ItemType::Message);
        asst.role = Some("assistant".into());
        asst.content = Some(ItemContent::Text("The answer is 2.".into()));

        let mut req = make_request(vec![fc, fco, asst]);
        req.reasoning = Some(Reasoning {
            effort: Some("high".into()),
            budget_tokens: None,
            generate_summary: None,
        });
        let body = build_chat_request(&req, true).unwrap();
        let msgs = body["messages"].as_array().unwrap();

        // assistant(tool_calls) and assistant(content) 应该都被补上 reasoning_content=""
        for msg in msgs {
            if msg["role"] == "assistant" {
                assert!(
                    msg.get("reasoning_content").is_some(),
                    "assistant message missing reasoning_content padding"
                );
            }
        }
    }

    // ─── input 为纯字符串 ──────────────────────────────────────

    #[test]
    fn test_input_string_deserialized_as_input_text() {
        let raw = r#"{"model":"test","input":"hello world","stream":false}"#;
        let req: GatewayRequest = serde_json::from_str(raw).unwrap();
        assert_eq!(req.input.len(), 1);
        assert_eq!(req.input[0].item_type, ItemType::InputText);
        match &req.input[0].content {
            Some(ItemContent::Text(s)) => assert_eq!(s, "hello world"),
            _ => panic!("expected Text content"),
        }
    }

    // ─── tools 过滤：只保留 function 类型 ──────────────────────

    #[test]
    fn test_non_function_tools_filtered() {
        let mut req = make_request(vec![]);
        req.tools = vec![
            json!({"type": "function", "function": {"name": "search"}}),
            json!({"type": "web_search", "web_search": {}}),
            json!({"type": "function", "function": {"name": "calc"}}),
        ];
        let body = build_chat_request(&req, false).unwrap();
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["function"]["name"], "search");
        assert_eq!(tools[1]["function"]["name"], "calc");
    }

    // ═══ DeepSeek 严格约束测试 ═══════════════════════════════════

    // ─── reasoning effort 精细映射 ─────────────────────────────

    #[test]
    fn test_deepseek_effort_low_maps_to_high() {
        let mut req = make_request(vec![]);
        req.reasoning = Some(Reasoning {
            effort: Some("low".into()),
            budget_tokens: None,
            generate_summary: None,
        });
        let body = build_chat_request(&req, true).unwrap();
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["reasoning_effort"], "high");
    }

    #[test]
    fn test_deepseek_effort_medium_maps_to_high() {
        let mut req = make_request(vec![]);
        req.reasoning = Some(Reasoning {
            effort: Some("medium".into()),
            budget_tokens: None,
            generate_summary: None,
        });
        let body = build_chat_request(&req, true).unwrap();
        assert_eq!(body["reasoning_effort"], "high");
    }

    #[test]
    fn test_deepseek_effort_xhigh_maps_to_max() {
        let mut req = make_request(vec![]);
        req.reasoning = Some(Reasoning {
            effort: Some("xhigh".into()),
            budget_tokens: None,
            generate_summary: None,
        });
        let body = build_chat_request(&req, true).unwrap();
        assert_eq!(body["reasoning_effort"], "max");
    }

    #[test]
    fn test_deepseek_effort_max_preserved() {
        let mut req = make_request(vec![]);
        req.reasoning = Some(Reasoning {
            effort: Some("max".into()),
            budget_tokens: None,
            generate_summary: None,
        });
        let body = build_chat_request(&req, true).unwrap();
        assert_eq!(body["reasoning_effort"], "max");
    }

    // ─── thinking 启用时移除 temperature/top_p ─────────────────

    #[test]
    fn test_deepseek_thinking_strips_temperature_top_p() {
        let mut req = make_request(vec![]);
        req.reasoning = Some(Reasoning {
            effort: Some("high".into()),
            budget_tokens: None,
            generate_summary: None,
        });
        req.temperature = Some(0.7);
        req.top_p = Some(0.9);
        let body = build_chat_request(&req, true).unwrap();
        assert!(
            body.get("temperature").is_none(),
            "temperature should be stripped"
        );
        assert!(body.get("top_p").is_none(), "top_p should be stripped");
    }

    #[test]
    fn test_deepseek_thinking_disabled_keeps_temperature() {
        let mut req = make_request(vec![]);
        req.reasoning = Some(Reasoning {
            effort: Some("none".into()),
            budget_tokens: None,
            generate_summary: None,
        });
        req.temperature = Some(0.7);
        let body = build_chat_request(&req, true).unwrap();
        // thinking disabled → temperature 保留
        assert_eq!(body["temperature"], 0.7);
    }

    // ─── developer → system ────────────────────────────────────

    #[test]
    fn test_deepseek_developer_role_to_system() {
        // 直接构造含 developer role 的场景
        // instructions 用 system，再插入一个 developer message
        let mut req = make_request(vec![]);
        req.instructions = Some("You are helpful.".into());
        let mut body = build_chat_request(&req, true).unwrap();
        // 手动改第一个 msg 为 developer 来测试 normalize
        body["messages"][0]["role"] = json!("developer");
        normalize_developer_messages(&mut body);
        assert_eq!(body["messages"][0]["role"], "system");
    }

    // ─── 丢弃 reasoning-only assistant message ─────────────────

    #[test]
    fn test_deepseek_drops_reasoning_only_assistant() {
        let mut reasoning = make_item(ItemType::Reasoning);
        reasoning.summary = Some(vec![SummaryPart {
            part_type: "summary_text".into(),
            text: "just thinking...".into(),
        }]);

        // reasoning 后面不跟 function_call，生成独立 assistant msg
        let mut user = make_item(ItemType::InputText);
        user.content = Some(ItemContent::Text("hello".into()));

        let mut req = make_request(vec![reasoning, user]);
        req.reasoning = Some(Reasoning {
            effort: Some("high".into()),
            budget_tokens: None,
            generate_summary: None,
        });
        let body = build_chat_request(&req, true).unwrap();
        let msgs = body["messages"].as_array().unwrap();

        // reasoning-only assistant msg 应被丢弃，只剩 user
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
    }

    // ─── tool_calls 轮次回填 reasoning_content ─────────────────

    #[test]
    fn test_deepseek_backfills_reasoning_content_for_tool_calls() {
        // 模拟：reasoning + fc → tool output → 第二轮 fc（无 reasoning）
        let mut reasoning = make_item(ItemType::Reasoning);
        reasoning.summary = Some(vec![SummaryPart {
            part_type: "summary_text".into(),
            text: "I need to search".into(),
        }]);

        let mut fc1 = make_item(ItemType::FunctionCall);
        fc1.call_id = Some("call_1".into());
        fc1.name = Some("search".into());
        fc1.arguments = Some(r#"{"q":"rust"}"#.into());

        let mut fco1 = make_item(ItemType::FunctionCallOutput);
        fco1.call_id = Some("call_1".into());
        fco1.output = Some("found results".into());

        // 第二轮 fc，没有 reasoning
        let mut fc2 = make_item(ItemType::FunctionCall);
        fc2.call_id = Some("call_2".into());
        fc2.name = Some("fetch".into());
        fc2.arguments = Some(r#"{"url":"..."}"#.into());

        let mut fco2 = make_item(ItemType::FunctionCallOutput);
        fco2.call_id = Some("call_2".into());
        fco2.output = Some("page content".into());

        let mut asst = make_item(ItemType::Message);
        asst.role = Some("assistant".into());
        asst.content = Some(ItemContent::Text("Here's what I found.".into()));

        let mut req = make_request(vec![reasoning, fc1, fco1, fc2, fco2, asst]);
        req.reasoning = Some(Reasoning {
            effort: Some("high".into()),
            budget_tokens: None,
            generate_summary: None,
        });
        let body = build_chat_request(&req, true).unwrap();
        let msgs = body["messages"].as_array().unwrap();

        // 找到有 tool_calls 的 assistant messages，检查 reasoning_content
        for msg in msgs {
            if msg["role"] == "assistant"
                && msg
                    .get("tool_calls")
                    .and_then(|v| v.as_array())
                    .is_some_and(|a| !a.is_empty())
            {
                let rc = msg["reasoning_content"].as_str().unwrap_or("");
                assert!(
                    !rc.is_empty(),
                    "tool_calls assistant message should have reasoning_content backfilled, got empty"
                );
            }
        }
    }

    // ─── Codex 专属 tool type 在 input 中被跳过 ────────────────

    #[test]
    fn test_unknown_item_types_skipped() {
        // web_search_call, image_generation_call 等 → ItemType::Unknown → 跳过
        let raw = r#"{
            "model": "test",
            "stream": false,
            "input": [
                {"type": "input_text", "content": "hello"},
                {"type": "web_search_call", "id": "ws_1", "status": "completed"},
                {"type": "image_generation_call", "id": "ig_1", "status": "completed"},
                {"type": "input_text", "content": "world"}
            ]
        }"#;
        let req: GatewayRequest = serde_json::from_str(raw).unwrap();
        let body = build_chat_request(&req, true).unwrap();
        let msgs = body["messages"].as_array().unwrap();
        // 只有 2 个 user messages，web_search 和 image_generation 被跳过
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["content"], "hello");
        assert_eq!(msgs[1]["content"], "world");
    }
}
