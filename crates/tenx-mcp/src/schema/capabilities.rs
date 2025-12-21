use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Capabilities a client may support. Known capabilities are defined here,
/// in this schema, but this is not a closed set: any client can define its own,
/// additional capabilities.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClientCapabilities {
    /// Experimental, non-standard capabilities that the client supports.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experimental: Option<HashMap<String, Value>>,
    /// Present if the client supports listing roots.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roots: Option<RootsCapability>,
    /// Present if the client supports sampling from an LLM.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling: Option<Value>,
    /// Present if the client supports elicitation requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elicitation: Option<Value>,
}

impl ClientCapabilities {
    /// Create a new empty ClientCapabilities
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an experimental capability
    pub fn with_experimental_capability(mut self, key: impl Into<String>, value: Value) -> Self {
        self.experimental
            .get_or_insert_with(HashMap::new)
            .insert(key.into(), value);
        self
    }

    /// Enable roots capability.
    ///
    /// list_changed indicates whether the client supports notifications for changes to the roots
    /// list.
    pub fn with_roots_capability(mut self, list_changed: bool) -> Self {
        self.roots = Some(RootsCapability {
            list_changed: Some(list_changed),
        });
        self
    }

    /// Enable sampling capability
    pub fn with_sampling(mut self) -> Self {
        self.sampling = Some(Value::Object(serde_json::Map::new()));
        self
    }

    /// Enable elicitation capability
    pub fn with_elicitation(mut self) -> Self {
        self.elicitation = Some(Value::Object(serde_json::Map::new()));
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootsCapability {
    /// Whether the client supports notifications for changes to the roots list.
    #[serde(rename = "listChanged", skip_serializing_if = "Option::is_none")]
    pub list_changed: Option<bool>,
}

/// Capabilities that a server may support. Known capabilities are defined here,
/// in this schema, but this is not a closed set: any server can define its own,
/// additional capabilities.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServerCapabilities {
    /// Experimental, non-standard capabilities that the server supports.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experimental: Option<HashMap<String, Value>>,
    /// Present if the server supports sending log messages to the client.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logging: Option<Value>,
    /// Present if the server supports argument autocompletion suggestions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completions: Option<Value>,
    /// Present if the server offers any prompt templates.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompts: Option<PromptsCapability>,
    /// Present if the server offers any resources to read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourcesCapability>,
    /// Present if the server offers any tools to call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolsCapability>,
}

impl ServerCapabilities {
    /// Enable experimental capabilities
    pub fn with_experimental(mut self, experimental: HashMap<String, Value>) -> Self {
        self.experimental = Some(experimental);
        self
    }

    /// Enable logging capability
    pub fn with_logging(mut self) -> Self {
        self.logging = Some(Value::Object(serde_json::Map::new()));
        self
    }

    /// Enable completions capability
    pub fn with_completions(mut self) -> Self {
        self.completions = Some(Value::Object(serde_json::Map::new()));
        self
    }

    /// Enable prompts capability with optional list_changed support
    pub fn with_prompts(mut self, list_changed: Option<bool>) -> Self {
        self.prompts = Some(PromptsCapability { list_changed });
        self
    }

    /// Enable resources capability with optional subscribe and list_changed support
    pub fn with_resources(mut self, subscribe: Option<bool>, list_changed: Option<bool>) -> Self {
        self.resources = Some(ResourcesCapability {
            subscribe,
            list_changed,
        });
        self
    }

    /// Enable tools capability with optional list_changed support
    pub fn with_tools(mut self, list_changed: Option<bool>) -> Self {
        self.tools = Some(ToolsCapability { list_changed });
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptsCapability {
    /// Whether this server supports notifications for changes to the prompt
    /// list.
    #[serde(rename = "listChanged", skip_serializing_if = "Option::is_none")]
    pub list_changed: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourcesCapability {
    /// Whether this server supports subscribing to resource updates.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscribe: Option<bool>,
    /// Whether this server supports notifications for changes to the resource
    /// list.
    #[serde(rename = "listChanged", skip_serializing_if = "Option::is_none")]
    pub list_changed: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsCapability {
    /// Whether this server supports notifications for changes to the tool list.
    #[serde(rename = "listChanged", skip_serializing_if = "Option::is_none")]
    pub list_changed: Option<bool>,
}
