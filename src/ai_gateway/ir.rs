use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayTurn {
    pub model: String,
    pub instructions: Option<String>,
    pub input: Vec<GatewayItem>,
    pub tools: Vec<GatewayTool>,
    pub tool_choice: Option<Value>,
    pub reasoning: Option<GatewayReasoning>,
    pub text: Option<GatewayTextOptions>,
    pub stream: bool,
    pub max_output_tokens: Option<i64>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub prompt_cache_key: Option<String>,
    pub prompt_cache_retention: Option<String>,
    pub previous_response_id: Option<String>,
    pub raw: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayReasoning {
    pub effort: Option<String>,
    pub budget_tokens: Option<i64>,
    pub generate_summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayTextOptions {
    pub format: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GatewayItem {
    Message(GatewayMessage),
    Reasoning(GatewayReasoningItem),
    FunctionCall(GatewayFunctionCall),
    FunctionCallOutput(GatewayToolOutput),
    ToolSearchCall(GatewayToolSearchCall),
    ToolSearchOutput(GatewayToolSearchOutput),
    CustomToolCall(GatewayCustomToolCall),
    CustomToolCallOutput(GatewayToolOutput),
    BuiltinCall(GatewayBuiltinCall),
    Unknown(GatewayUnknownItem),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayMessage {
    pub role: String,
    pub content: Vec<GatewayContentBlock>,
    pub status: Option<String>,
    pub raw: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayReasoningItem {
    pub id: Option<String>,
    pub status: Option<String>,
    pub summary: Vec<String>,
    pub encrypted_content: Option<String>,
    pub raw: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayFunctionCall {
    pub id: Option<String>,
    pub call_id: Option<String>,
    pub namespace: Option<String>,
    pub name: String,
    pub arguments: Value,
    pub status: Option<String>,
    pub raw: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayCustomToolCall {
    pub id: Option<String>,
    pub call_id: Option<String>,
    pub name: String,
    pub input: String,
    pub status: Option<String>,
    pub raw: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayToolSearchCall {
    pub id: Option<String>,
    pub call_id: Option<String>,
    pub execution: String,
    pub arguments: Value,
    pub status: Option<String>,
    pub raw: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayToolOutput {
    pub call_id: Option<String>,
    pub name: Option<String>,
    pub output: Value,
    pub raw: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayToolSearchOutput {
    pub call_id: Option<String>,
    pub status: String,
    pub execution: String,
    pub tools: Vec<GatewayTool>,
    pub raw_tools: Vec<Value>,
    pub raw: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayBuiltinCall {
    pub id: Option<String>,
    pub item_type: String,
    pub status: Option<String>,
    pub action: Option<Value>,
    pub raw: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayUnknownItem {
    pub item_type: String,
    pub raw: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GatewayContentBlock {
    Text {
        text: String,
        kind: TextKind,
        raw: Option<Value>,
    },
    Image {
        image_url: String,
        detail: Option<String>,
        raw: Option<Value>,
    },
    Unknown {
        block_type: String,
        raw: Value,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextKind {
    Input,
    Output,
    Summary,
    Plain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolKey {
    pub namespace: Option<String>,
    pub name: String,
    pub kind: ToolKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolKind {
    Function,
    ToolSearch,
    Custom,
    WebSearch,
    ImageGeneration,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GatewayTool {
    Function(GatewayFunctionTool),
    Namespace(GatewayNamespaceTool),
    ToolSearch(GatewayToolSearch),
    Custom(GatewayCustomTool),
    Builtin(GatewayBuiltinTool),
    Unknown(GatewayUnknownTool),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayFunctionTool {
    pub key: ToolKey,
    pub description: Option<String>,
    pub parameters: Option<Value>,
    pub strict: Option<bool>,
    pub raw: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayNamespaceTool {
    pub name: String,
    pub description: Option<String>,
    pub tools: Vec<GatewayTool>,
    pub raw: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayToolSearch {
    pub key: ToolKey,
    pub execution: Option<String>,
    pub description: Option<String>,
    pub parameters: Option<Value>,
    pub raw: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayCustomTool {
    pub key: ToolKey,
    pub description: Option<String>,
    pub format: Option<Value>,
    pub raw: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayBuiltinTool {
    pub key: ToolKey,
    pub raw: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayUnknownTool {
    pub tool_type: String,
    pub raw: Value,
}
