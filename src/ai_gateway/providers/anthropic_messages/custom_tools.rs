use serde_json::{Map, Value};

const APPLY_PATCH_TOOL_NAME: &str = "apply_patch";

pub(super) fn custom_tool_description(tool: &Map<String, Value>) -> String {
    let name = tool.get("name").and_then(Value::as_str).unwrap_or("");
    let base_description = tool
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if name == APPLY_PATCH_TOOL_NAME {
        return apply_patch_description();
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
        "The entire apply_patch patch body."
    } else {
        "The entire freeform input for the custom tool."
    }
}

fn apply_patch_description() -> String {
    "Use the `apply_patch` tool to edit files.\n\
Your patch language is a stripped-down, file-oriented diff format designed to be easy to parse and safe to apply. A patch must use this envelope:\n\n\
*** Begin Patch\n\
[ one or more file sections ]\n\
*** End Patch\n\n\
Within that envelope, each file operation starts with one of these headers:\n\n\
*** Add File: <path> - create a new file. Every following line is a + line.\n\
*** Delete File: <path> - remove an existing file. Nothing follows.\n\
*** Update File: <path> - patch an existing file in place, optionally followed by *** Move to: <new path>.\n\n\
Update hunks start with @@ and contain lines prefixed with a space, -, or +. File references must be relative, never absolute.\n\n\
Call this tool with JSON arguments matching {\"input\":\"<the entire patch body>\"}. The `input` value must contain only the patch body and its final non-whitespace line must be exactly `*** End Patch`."
        .to_string()
}
