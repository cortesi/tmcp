//! Prompt types and helpers.

use serde::{Deserialize, Serialize};

use super::*;
use crate::macros::{with_basename, with_meta};

/// The server's response to a prompts/list request from the client.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ListPromptsResult {
    /// The list of prompts available on the server.
    pub prompts: Vec<Prompt>,
    #[serde(rename = "nextCursor", skip_serializing_if = "Option::is_none")]
    /// Cursor for the next page of results.
    pub next_cursor: Option<Cursor>,
}

impl ListPromptsResult {
    /// Create a new empty result.
    pub fn new() -> Self {
        Self {
            prompts: Vec::new(),
            next_cursor: None,
        }
    }

    /// Add a single prompt to the result.
    pub fn with_prompt(mut self, prompt: Prompt) -> Self {
        self.prompts.push(prompt);
        self
    }

    /// Add multiple prompts to the result.
    pub fn with_prompts(mut self, prompts: impl IntoIterator<Item = Prompt>) -> Self {
        self.prompts.extend(prompts);
        self
    }

    /// Set the cursor for the next page of results.
    pub fn with_cursor(mut self, cursor: impl Into<Cursor>) -> Self {
        self.next_cursor = Some(cursor.into());
        self
    }
}

/// The server's response to a prompts/get request from the client.
#[with_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetPromptResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    /// A description of the prompt.
    pub description: Option<String>,
    /// The messages that make up the prompt.
    pub messages: Vec<PromptMessage>,
}

impl GetPromptResult {
    /// Create a new empty result.
    pub fn new() -> Self {
        Self {
            description: None,
            messages: Vec::new(),
            _meta: None,
        }
    }

    /// Set the description of the prompt.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Add a message to the prompt.
    pub fn with_message(mut self, message: PromptMessage) -> Self {
        self.messages.push(message);
        self
    }

    /// Add multiple messages to the prompt.
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
    /// A description of what the prompt does.
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// A list of arguments that the prompt accepts.
    pub arguments: Option<Vec<PromptArgument>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Optional icon metadata.
    pub icons: Option<Vec<Icon>>,
}

impl Prompt {
    /// Create a new prompt with a name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            description: None,
            arguments: None,
            icons: None,
            name: name.into(),
            title: None,
            _meta: None,
        }
    }

    /// Set the description of the prompt.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Add an argument to the prompt.
    pub fn with_argument(mut self, argument: PromptArgument) -> Self {
        self.arguments.get_or_insert_with(Vec::new).push(argument);
        self
    }

    /// Add multiple arguments to the prompt.
    pub fn with_arguments(mut self, arguments: impl IntoIterator<Item = PromptArgument>) -> Self {
        self.arguments = Some(arguments.into_iter().collect());
        self
    }

    /// Set the icons for the prompt.
    pub fn with_icons(mut self, icons: impl IntoIterator<Item = Icon>) -> Self {
        self.icons = Some(icons.into_iter().collect());
        self
    }
}

/// Describes an argument that a prompt can accept.
#[with_basename]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptArgument {
    #[serde(skip_serializing_if = "Option::is_none")]
    /// A description of the argument.
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Whether the argument is required.
    pub required: Option<bool>,
}

impl PromptArgument {
    /// Create a new prompt argument with a name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            description: None,
            required: None,
            name: name.into(),
            title: None,
        }
    }

    /// Set the description of the argument.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set whether the argument is required.
    pub fn with_required(mut self, required: bool) -> Self {
        self.required = Some(required);
        self
    }
}

/// A message in a prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptMessage {
    /// The role of the message sender.
    pub role: Role,
    /// The content of the message.
    pub content: ContentBlock,
}

impl PromptMessage {
    /// Create a new prompt message.
    pub fn new(role: Role, content: ContentBlock) -> Self {
        Self { role, content }
    }

    /// Create a user message.
    pub fn user(content: ContentBlock) -> Self {
        Self {
            role: Role::User,
            content,
        }
    }

    /// Create an assistant message.
    pub fn assistant(content: ContentBlock) -> Self {
        Self {
            role: Role::Assistant,
            content,
        }
    }

    /// Create a user message with text content.
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: ContentBlock::Text(TextContent::new(text)),
        }
    }

    /// Create an assistant message with text content.
    pub fn assistant_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: ContentBlock::Text(TextContent::new(text)),
        }
    }
}
