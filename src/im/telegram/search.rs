use std::collections::HashSet;

use serde_json::Value;
use url::Url;

use super::rich_blocks;

const MAX_VISIBLE_RESULTS: usize = 5;
const MAX_TITLE_CHARS: usize = 180;
const MAX_SNIPPET_CHARS: usize = 260;
const MAX_URL_CHARS: usize = 320;
const MAX_RAW_JSON_CHARS: usize = 3600;

#[derive(Debug, Clone)]
pub(crate) struct TelegramWebSearchRender {
    pub blocks: Vec<Value>,
    pub fallback_markdown: String,
}

/// Render a completed web search as a compact Telegram rich message.
///
/// Search providers have emitted a few different result envelopes over time,
/// so this intentionally accepts both a direct `results` array and common
/// nested wrappers such as `{ results: { items: [...] } }`.
pub(crate) fn render_web_search(item: &Value) -> Option<TelegramWebSearchRender> {
    let query = search_query(item).unwrap_or_default();
    let (visible_results, total_results) = search_results(item);
    let action = item.get("action").filter(|value| !value.is_null());
    let raw_results = item.get("results").filter(|value| !value.is_null());

    if query.is_empty() && visible_results.is_empty() && action.is_none() && raw_results.is_none() {
        return None;
    }

    let query_text = if query.is_empty() {
        "未提供关键词"
    } else {
        query.as_str()
    };
    let mut blocks = vec![
        rich_blocks::heading(rich_blocks::text("🔎 搜索"), 3),
        rich_blocks::paragraph(rich_blocks::rich_text(vec![
            rich_blocks::bold("关键词"),
            rich_blocks::text(" "),
            rich_blocks::code(compact_inline(query_text, MAX_TITLE_CHARS)),
        ])),
    ];

    if total_results > 0 {
        blocks.push(rich_blocks::paragraph(rich_blocks::bold(format!(
            "{} 条结果",
            total_results
        ))));
        let items = visible_results
            .iter()
            .map(result_list_item)
            .collect::<Vec<_>>();
        if !items.is_empty() {
            blocks.push(rich_blocks::list(items));
        }
        if total_results > visible_results.len() {
            blocks.push(rich_blocks::paragraph(rich_blocks::text(format!(
                "另有 {} 条结果已收起",
                total_results - visible_results.len()
            ))));
        }
    } else {
        blocks.push(rich_blocks::paragraph(rich_blocks::text("未返回结果")));
    }

    let mut raw_blocks = Vec::new();
    if let Some(action) = action {
        raw_blocks.push(rich_blocks::preformatted(
            truncate_json(action),
            Some("json"),
        ));
    }
    if let Some(results) = raw_results {
        raw_blocks.push(rich_blocks::preformatted(
            truncate_json(results),
            Some("json"),
        ));
    }
    if !raw_blocks.is_empty() {
        blocks.push(rich_blocks::details(
            rich_blocks::text("原始搜索数据"),
            raw_blocks,
            false,
        ));
    }

    Some(TelegramWebSearchRender {
        fallback_markdown: fallback_markdown(query_text, &visible_results, total_results),
        blocks,
    })
}

fn result_list_item(result: &Value) -> Value {
    let title = result_text(result, &["title", "name", "heading"])
        .map(|text| compact_inline(&text, MAX_TITLE_CHARS));
    let raw_url = result_text(result, &["url", "link", "href"]);
    let url = raw_url
        .as_deref()
        .map(|text| compact_inline(text, MAX_URL_CHARS));
    let link_target = raw_url.as_deref().filter(|url| is_http_url(url));
    let domain = result_text(result, &["domain", "site", "source"])
        .or_else(|| raw_url.as_deref().and_then(url_domain));
    let snippet = result_text(result, &["snippet", "description", "summary", "text"])
        .map(|text| compact_inline(&text, MAX_SNIPPET_CHARS));

    let has_title = title.is_some();
    let mut line = Vec::new();
    match (title, link_target) {
        (Some(title), Some(url)) => line.push(rich_blocks::url(rich_blocks::bold(title), url)),
        (Some(title), None) => line.push(rich_blocks::bold(title)),
        (None, Some(url)) => line.push(rich_blocks::url(rich_blocks::text("未命名结果"), url)),
        (None, None) => line.push(rich_blocks::text("未命名结果")),
    }
    if let Some(domain) = domain {
        if has_title {
            line.push(rich_blocks::text(" · "));
            line.push(rich_blocks::code(domain));
        }
    }

    let mut blocks = vec![rich_blocks::paragraph(rich_blocks::rich_text(line))];
    if let Some(snippet) = snippet {
        blocks.push(rich_blocks::paragraph(rich_blocks::text(snippet)));
    }
    if link_target.is_none()
        && let Some(url) = url
    {
        blocks.push(rich_blocks::paragraph(rich_blocks::text(url)));
    }
    rich_blocks::list_item(blocks)
}

fn fallback_markdown(query: &str, results: &[Value], total: usize) -> String {
    let mut lines = vec![
        "🔎 搜索".to_string(),
        String::new(),
        format!("关键词：`{}`", markdown_inline(query)),
        format!("结果：{} 条", total),
    ];
    for result in results {
        let title = result_text(result, &["title", "name", "heading"])
            .map(|text| markdown_inline(&compact_inline(&text, MAX_TITLE_CHARS)))
            .unwrap_or_else(|| "未命名结果".to_string());
        let url = result_text(result, &["url", "link", "href"])
            .map(|text| compact_inline(&text, MAX_URL_CHARS));
        let domain = result_text(result, &["domain", "site", "source"])
            .or_else(|| url.as_deref().and_then(url_domain));
        let suffix = domain
            .map(|domain| format!(" · `{}`", markdown_inline(&domain)))
            .unwrap_or_default();
        lines.push(format!("- **{title}**{suffix}"));
        if let Some(snippet) = result_text(result, &["snippet", "description", "summary", "text"]) {
            lines.push(format!("  {}", compact_inline(&snippet, MAX_SNIPPET_CHARS)));
        }
        if let Some(url) = url {
            lines.push(format!("  {url}"));
        }
    }
    if total > results.len() {
        lines.push(format!("另有 {} 条结果已收起", total - results.len()));
    }
    lines.join("\n")
}

fn search_query(item: &Value) -> Option<String> {
    string_at(item, &["query"])
        .or_else(|| {
            item.get("action")
                .and_then(|action| string_at(action, &["query"]))
        })
        .or_else(|| {
            item.get("action")
                .and_then(|action| action.get("queries"))
                .and_then(Value::as_array)
                .and_then(|queries| queries.iter().find_map(Value::as_str))
                .map(str::trim)
                .filter(|query| !query.is_empty())
                .map(str::to_string)
        })
}

fn search_results(item: &Value) -> (Vec<Value>, usize) {
    let Some(raw) = item
        .get("results")
        .filter(|value| !value.is_null())
        .or_else(|| item.get("result").filter(|value| !value.is_null()))
    else {
        return (Vec::new(), 0);
    };
    let mut all = Vec::new();
    collect_results(raw, &mut all);
    let mut seen = HashSet::new();
    all.retain(|result| {
        let key = result_text(result, &["ref_id", "url", "link", "title"])
            .unwrap_or_else(|| result.to_string());
        seen.insert(key)
    });
    let total = all.len();
    all.truncate(MAX_VISIBLE_RESULTS);
    (all, total)
}

fn collect_results(value: &Value, output: &mut Vec<Value>) {
    match value {
        Value::Array(values) => {
            for value in values {
                if looks_like_result(value) {
                    output.push(value.clone());
                } else {
                    collect_results(value, output);
                }
            }
        }
        Value::Object(map) => {
            if looks_like_result(value) {
                output.push(value.clone());
                return;
            }
            for key in ["results", "items", "data", "content"] {
                if let Some(value) = map.get(key) {
                    collect_results(value, output);
                    if !output.is_empty() {
                        return;
                    }
                }
            }
        }
        _ => {}
    }
}

fn looks_like_result(value: &Value) -> bool {
    value.is_object()
        && ["title", "url", "link", "snippet", "description", "domain"]
            .iter()
            .any(|key| value.get(*key).is_some())
}

fn result_text(result: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| string_at(result, &[*key]))
}

fn string_at(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn url_domain(url: &str) -> Option<String> {
    Url::parse(url).ok()?.host_str().map(str::to_string)
}

fn is_http_url(url: &str) -> bool {
    Url::parse(url)
        .ok()
        .is_some_and(|parsed| matches!(parsed.scheme(), "http" | "https"))
}

fn compact_inline(text: &str, max_chars: usize) -> String {
    let text = text
        .replace(['\r', '\n'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    truncate(&text, max_chars)
}

fn markdown_inline(text: &str) -> String {
    text.replace('`', "'")
        .replace('*', "·")
        .replace('_', "-")
        .replace('[', "(")
        .replace(']', ")")
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let keep = max_chars.saturating_sub(1);
    format!("{}…", text.chars().take(keep).collect::<String>())
}

fn truncate_json(value: &Value) -> String {
    let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    truncate(&text, MAX_RAW_JSON_CHARS)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::render_web_search;

    #[test]
    fn renders_compact_results_and_collapsed_raw_data() {
        let rendered = render_web_search(&json!({
            "type": "webSearch",
            "query": "Telegram sendMessageDraft",
            "action": {"type": "openPage", "url": "https://core.telegram.org/bots/api"},
            "results": [{
                "title": "Telegram Bot API",
                "domain": "core.telegram.org",
                "snippet": "Total lines: 6874",
                "url": "https://core.telegram.org/bots/api"
            }]
        }))
        .expect("search render");

        assert_eq!(rendered.blocks[0]["type"], "heading");
        assert_eq!(rendered.blocks[2]["type"], "paragraph");
        assert!(rendered.blocks.iter().any(|block| block["type"] == "list"));
        let details = rendered
            .blocks
            .iter()
            .find(|block| block["type"] == "details")
            .expect("raw details");
        assert_eq!(details["is_open"], Value::Null);
        assert!(rendered.fallback_markdown.contains("Telegram Bot API"));
        assert!(!rendered.fallback_markdown.contains("\"openPage\""));
    }

    #[test]
    fn accepts_nested_result_envelopes_and_action_query() {
        let rendered = render_web_search(&json!({
            "action": {"query": "rust", "queries": ["rust"]},
            "results": {"items": [{"name": "Rust", "link": "https://rust-lang.org"}]}
        }))
        .expect("nested search render");
        assert!(rendered.fallback_markdown.contains("关键词：`rust`"));
        assert!(rendered.fallback_markdown.contains("结果：1 条"));
    }

    #[test]
    fn ignores_empty_search_items() {
        assert!(render_web_search(&json!({"type": "webSearch"})).is_none());
    }
}
