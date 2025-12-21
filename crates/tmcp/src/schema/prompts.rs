use serde::{Deserialize, Serialize};

use super::*;
use crate::macros::{with_basename, with_meta};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ListPromptsResult {
    pub prompts: Vec<Prompt>,
    #[serde(rename = "nextCursor", skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

impl ListPromptsResult {
    pub fn new() -> Self {
        Self {
            prompts: Vec::new(),
            next_cursor: None,
        }
    }

    pub fn with_prompt(mut self, prompt: Prompt) -> Self {
        self.prompts.push(prompt);
        self
    }

    pub fn with_prompts(mut self, prompts: impl IntoIterator<Item = Prompt>) -> Self {
        self.prompts.extend(prompts);
        self
    }

    pub fn with_cursor(mut self, cursor: impl Into<Cursor>) -> Self {
        self.next_cursor = Some(cursor.into());
        self
    }
}

#[with_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetPromptResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub messages: Vec<PromptMessage>,
}

impl GetPromptResult {
    pub fn new() -> Self {
        Self {
            description: None,
            messages: Vec::new(),
            _meta: None,
        }
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_message(mut self, message: PromptMessage) -> Self {
        self.messages.push(message);
        self
    }

    pub fn with_messages(mut self, messages: impl IntoIterator<Item = PromptMessage>) -> Self {
        self.messages.extend(messages);
        self
    }
}

impl Default for GetPromptResult {
    fn default() -> Self {
        Self::new()
    }
}

/// A prompt or prompt template that the server offers.
#[with_meta]
#[with_basename]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prompt {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Vec<PromptArgument>>,
}

impl Prompt {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            description: None,
            arguments: None,
            name: name.into(),
            title: None,
            _meta: None,
        }
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_argument(mut self, argument: PromptArgument) -> Self {
        self.arguments.get_or_insert_with(Vec::new).push(argument);
        self
    }

    pub fn with_arguments(mut self, arguments: impl IntoIterator<Item = PromptArgument>) -> Self {
        self.arguments = Some(arguments.into_iter().collect());
        self
    }
}

/// Describes an argument that a prompt can accept.
#[with_basename]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptArgument {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
}

impl PromptArgument {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            description: None,
            required: None,
            name: name.into(),
            title: None,
        }
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn required(mut self, required: bool) -> Self {
        self.required = Some(required);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptMessage {
    pub role: Role,
    pub content: Content,
}

impl PromptMessage {
    pub fn new(role: Role, content: Content) -> Self {
        Self { role, content }
    }

    pub fn user(content: Content) -> Self {
        Self {
            role: Role::User,
            content,
        }
    }

    pub fn assistant(content: Content) -> Self {
        Self {
            role: Role::Assistant,
            content,
        }
    }

    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: Content::Text(TextContent {
                text: text.into(),
                annotations: None,
                _meta: None,
            }),
        }
    }

    pub fn assistant_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: Content::Text(TextContent {
                text: text.into(),
                annotations: None,
                _meta: None,
            }),
        }
    }
}
