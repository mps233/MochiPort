use std::cell::RefCell;
use std::rc::Rc;

use wxdragon::prelude::*;
use wxdragon::widgets::dataview::CustomDataViewVirtualListModel;

use super::UiHandles;
use super::api::RequestLogItem;

pub(super) type RequestLogRows = Rc<RefCell<Vec<RequestLogItem>>>;
pub(super) type RequestLogModel = Rc<RefCell<CustomDataViewVirtualListModel>>;

pub(super) fn request_log_cell(rows: &RequestLogRows, row: usize, col: usize) -> Variant {
    let rows = rows.borrow();
    let Some(log) = rows.get(row) else {
        return String::new().into();
    };

    match col {
        0 => format!("#{}", log.id).into(),
        1 => log.model_id.clone().into(),
        2 => {
            if log.stream {
                "Streaming".to_string().into()
            } else {
                "No".to_string().into()
            }
        }
        3 => log.channel.clone().into(),
        4 => status_label(&log.status).into(),
        5 => format_tokens(log).into(),
        6 => format_read_cache(log).into(),
        7 => log
            .write_cache_tokens
            .map(format_int)
            .unwrap_or_else(|| "-".to_string())
            .into(),
        8 => log
            .cost_usd
            .map(|cost| format!("${cost:.6}"))
            .unwrap_or_else(|| "-".to_string())
            .into(),
        9 => format_optional_duration(log.ttft_ms).into(),
        10 => format_optional_duration(log.latency_ms).into(),
        11 => log.created_at.clone().into(),
        _ => String::new().into(),
    }
}

pub(super) fn refresh_request_log_list(handles: &UiHandles, logs: Vec<RequestLogItem>) {
    let mut current_rows = handles.request_log_rows.borrow_mut();
    if *current_rows == logs {
        return;
    }

    let previous_len = current_rows.len();
    let selected_row = handles.request_log_list.get_selected_row();
    let new_len = logs.len();
    *current_rows = logs;
    drop(current_rows);

    if previous_len != new_len {
        handles.request_log_model.borrow_mut().reset(new_len);
        if let Some(row) = selected_row.filter(|row| *row < new_len) {
            handles.request_log_list.select_row(row);
        }
    } else {
        let model = handles.request_log_model.borrow();
        for row in 0..new_len {
            model.row_changed(row);
        }
    }
}

fn status_label(status: &str) -> String {
    match status {
        "completed" => "Completed",
        "incomplete" => "Incomplete",
        "failed" => "Failed",
        "running" => "Running",
        other => other,
    }
    .to_string()
}

fn format_tokens(log: &RequestLogItem) -> String {
    match (log.total_tokens, log.input_tokens, log.output_tokens) {
        (Some(total), Some(input), Some(output)) => {
            format!(
                "Total: {} | In: {} | Out: {}",
                format_int(total),
                format_int(input),
                format_int(output)
            )
        }
        (Some(total), _, _) => format!("Total: {}", format_int(total)),
        _ => "-".to_string(),
    }
}

fn format_read_cache(log: &RequestLogItem) -> String {
    let Some(tokens) = log.read_cache_tokens else {
        return "-".to_string();
    };
    match log.read_cache_hit_rate {
        Some(rate) => format!("{} ({:.1}%)", format_int(tokens), rate * 100.0),
        None => format_int(tokens),
    }
}

fn format_optional_duration(ms: Option<i64>) -> String {
    ms.map(format_duration).unwrap_or_else(|| "-".to_string())
}

fn format_duration(ms: i64) -> String {
    if ms >= 60_000 {
        format!("{:.1}m", ms as f64 / 60_000.0)
    } else if ms >= 1_000 {
        format!("{:.1}s", ms as f64 / 1_000.0)
    } else {
        format!("{ms}ms")
    }
}

fn format_int(value: i64) -> String {
    let mut chars: Vec<char> = value.abs().to_string().chars().rev().collect();
    let mut grouped = String::new();
    for (idx, ch) in chars.drain(..).enumerate() {
        if idx > 0 && idx % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    let mut result: String = grouped.chars().rev().collect();
    if value < 0 {
        result.insert(0, '-');
    }
    result
}
