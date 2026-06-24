use serde_json::{Map, Value};

use crate::ai_gateway::apply_patch_tool::{
    APPLY_PATCH_DESCRIPTION, APPLY_PATCH_INPUT_DESCRIPTION, APPLY_PATCH_TOOL_NAME,
};

pub(super) fn custom_tool_description(tool: &Map<String, Value>) -> String {
    let name = tool.get("name").and_then(Value::as_str).unwrap_or("");
    let base_description = tool
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if name == APPLY_PATCH_TOOL_NAME {
        return APPLY_PATCH_DESCRIPTION.to_string();
    }

    let mut description = if base_description.is_empty() {
        "Use this custom tool with freeform input.".to_string()
    } else {
        base_description.to_string()
    };

    description.push_str(
        "\n\nProvider adapter note: call this tool with JSON arguments and put the entire \
freeform tool input in the `input` string. Do not split, summarize, escape as a nested JSON \
object, or add extra wrapper text inside `input`.",
    );

    if let Some(format) = tool.get("format").and_then(Value::as_object) {
        let format_type = format.get("type").and_then(Value::as_str);
        let syntax = format.get("syntax").and_then(Value::as_str);
        if format_type.is_some() || syntax.is_some() {
            description.push_str("\n\nFreeform format:");
            if let Some(format_type) = format_type {
                description.push_str("\n- type: ");
                description.push_str(format_type);
            }
            if let Some(syntax) = syntax {
                description.push_str("\n- syntax: ");
                description.push_str(syntax);
            }
        }
        if let Some(definition) = format
            .get("definition")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            description.push_str("\n\nFormat definition:\n");
            description.push_str(definition);
        }
    }

    description
}

pub(super) fn custom_tool_input_description(name: &str) -> &'static str {
    if name == APPLY_PATCH_TOOL_NAME {
        APPLY_PATCH_INPUT_DESCRIPTION
    } else {
        "The entire freeform input for the custom tool."
    }
}
