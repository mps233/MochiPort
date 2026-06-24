use serde_json::{Value, json};

pub(super) fn convert_anthropic_citations(
    citations: &[Value],
    start_index: usize,
    end_index: usize,
) -> Vec<Value> {
    citations
        .iter()
        .filter_map(|citation| convert_anthropic_citation_at(citation, start_index, end_index))
        .collect()
}

pub(super) fn convert_anthropic_citation_at(
    citation: &Value,
    start_index: usize,
    end_index: usize,
) -> Option<Value> {
    let citation_type = citation.get("type").and_then(Value::as_str)?;
    match citation_type {
        "web_search_result_location" | "web_search_result" => {
            let url = citation.get("url").and_then(Value::as_str).unwrap_or("");
            if url.is_empty() {
                return None;
            }
            let title = citation.get("title").and_then(Value::as_str).unwrap_or("");
            Some(json!({
                "type": "url_citation",
                "start_index": start_index,
                "end_index": end_index,
                "url": url,
                "title": title,
            }))
        }
        _ => None,
    }
}
