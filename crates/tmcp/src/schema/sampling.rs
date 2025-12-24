#![allow(missing_docs)]

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::*;
use crate::macros::with_meta;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateMessageParams {
    pub messages: Vec<SamplingMessage>,
    #[serde(rename = "modelPreferences", skip_serializing_if = "Option::is_none")]
    pub model_preferences: Option<ModelPreferences>,
    #[serde(rename = "systemPrompt", skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(rename = "includeContext", skip_serializing_if = "Option::is_none")]
    pub include_context: Option<IncludeContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(rename = "maxTokens")]
    pub max_tokens: i64,
    #[serde(rename = "stopSequences", skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
    #[serde(rename = "toolChoice", skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskMetadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _meta: Option<RequestMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IncludeContext {
    None,
    ThisServer,
    AllServers,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolChoice {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<ToolChoiceMode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolChoiceMode {
    Auto,
    Required,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateMessageResult {
    #[serde(flatten)]
    pub message: SamplingMessage,
    pub model: String,
    #[serde(rename = "stopReason", skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StopReason {
    EndTurn,
    StopSequence,
    MaxTokens,
    ToolUse,
    #[serde(untagged)]
    Other(String),
}

#[with_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SamplingMessage {
    pub role: Role,
    pub content: OneOrMany<SamplingMessageContentBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPreferences {
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Optional model preference hints.
    pub hints: Option<Vec<ModelHint>>,
    #[serde(rename = "costPriority", skip_serializing_if = "Option::is_none")]
    /// Relative preference for lower cost.
    pub cost_priority: Option<f64>,
    #[serde(rename = "speedPriority", skip_serializing_if = "Option::is_none")]
    /// Relative preference for faster responses.
    pub speed_priority: Option<f64>,
    #[serde(
        rename = "intelligencePriority",
        skip_serializing_if = "Option::is_none"
    )]
    /// Relative preference for higher intelligence.
    pub intelligence_priority: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Hint describing a preferred model by name.
pub struct ModelHint {
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Optional model name hint.
    pub name: Option<String>,
}
