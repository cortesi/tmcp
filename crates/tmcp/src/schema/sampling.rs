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

impl CreateMessageParams {
    /// Create a simple user message request.
    ///
    /// This is a convenience method for the common case of sending a single
    /// user text message with reasonable defaults (max_tokens: 1024).
    ///
    /// # Example
    ///
    /// ```ignore
    /// let params = CreateMessageParams::user_message("What is the weather today?");
    /// // Or with a custom max_tokens:
    /// let params = CreateMessageParams::user_message("Tell me a story").with_max_tokens(2048);
    /// ```
    pub fn user_message(text: impl Into<String>) -> Self {
        Self {
            messages: vec![SamplingMessage::user_text(text)],
            model_preferences: None,
            system_prompt: None,
            include_context: None,
            temperature: None,
            max_tokens: 1024,
            stop_sequences: None,
            metadata: None,
            tools: None,
            tool_choice: None,
            task: None,
            _meta: None,
        }
    }

    /// Set the maximum number of tokens for the response.
    pub fn with_max_tokens(mut self, max_tokens: i64) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    /// Set the system prompt.
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Set the temperature for sampling.
    pub fn with_temperature(mut self, temperature: f64) -> Self {
        self.temperature = Some(temperature);
        self
    }
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

impl SamplingMessage {
    /// Create a user message with text content.
    ///
    /// This is a convenience method for creating simple user messages.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let msg = SamplingMessage::user_text("Hello, world!");
    /// ```
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: OneOrMany::One(SamplingMessageContentBlock::Text(TextContent::new(text))),
            _meta: None,
        }
    }

    /// Create an assistant message with text content.
    ///
    /// This is a convenience method for creating simple assistant messages.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let msg = SamplingMessage::assistant_text("I can help with that.");
    /// ```
    pub fn assistant_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: OneOrMany::One(SamplingMessageContentBlock::Text(TextContent::new(text))),
            _meta: None,
        }
    }
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
