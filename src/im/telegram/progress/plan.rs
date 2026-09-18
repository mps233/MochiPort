//! Extracted verbatim from `progress.rs`; no behavior changes.

use super::*;

/// Test-only alias kept so the plan-parsing fixtures can call the same entry
/// point the router used before it moved to `plan_from_params` directly.
#[cfg(test)]
pub(crate) fn parse_plan_update(params: &Value) -> (Option<String>, Vec<TelegramPlanStep>) {
    plan_from_params(params)
}

pub(crate) fn plan_from_item(item: &Value) -> (Option<String>, Vec<TelegramPlanStep>) {
    let explanation = item
        .get("text")
        .and_then(Value::as_str)
        .or_else(|| item.get("summary").and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    (explanation, Vec::new())
}

pub(super) fn has_plan_progress(snapshot: &TelegramCommandProgressSnapshot) -> bool {
    snapshot.plan_explanation.is_some() || !snapshot.plan.is_empty()
}

pub(super) fn render_plan_progress(
    snapshot: &TelegramCommandProgressSnapshot,
    text: ImText,
) -> Option<String> {
    render_plan_standalone(snapshot.plan_explanation.as_deref(), &snapshot.plan, text)
}

/// 完整颗粒度下的独立计划消息：每次计划更新单独成条，不进聚合气泡。
pub(crate) fn render_plan_standalone(
    explanation: Option<&str>,
    steps: &[TelegramPlanStep],
    text: ImText,
) -> Option<String> {
    if explanation.is_none() && steps.is_empty() {
        return None;
    }
    let completed = steps
        .iter()
        .filter(|step| step.status == TelegramPlanStepStatus::Completed)
        .count();
    let mut lines = vec![text.telegram_plan_heading(completed, steps.len())];
    if let Some(explanation) = explanation {
        lines.push(compact_text(explanation, TELEGRAM_PLAN_STEP_CHARS));
    }
    for step in steps.iter().take(TELEGRAM_PLAN_RENDER_STEPS) {
        let status = match step.status {
            TelegramPlanStepStatus::Pending => "pending",
            TelegramPlanStepStatus::InProgress => "in_progress",
            TelegramPlanStepStatus::Completed => "completed",
        };
        lines.push(format!(
            "{} · {}",
            text.telegram_progress_status_label(status),
            compact_text(&step.step, TELEGRAM_PLAN_STEP_CHARS)
        ));
    }
    if steps.len() > TELEGRAM_PLAN_RENDER_STEPS {
        lines.push(text.telegram_plan_omitted(steps.len() - TELEGRAM_PLAN_RENDER_STEPS));
    }
    Some(lines.join("\n"))
}
