use serde::{Deserialize, Serialize};

use super::*;
use crate::macros::with_meta;

/// After receiving an initialize request from the client, the server sends this response.
#[with_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeResult {
    /// The version of the Model Context Protocol that the server wants to use.
    /// This may not match the version that the client requested. If the
    /// client cannot support this version, it MUST disconnect.
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    #[serde(rename = "serverInfo")]
    pub server_info: Implementation,
    /// Instructions describing how to use the server and its features.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

impl InitializeResult {
    /// Create a new InitializeResult with the latest protocol version and default server version
    ///
    /// The default server version is set to "0.0.1". Use `with_version()` to set a custom version.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            protocol_version: LATEST_PROTOCOL_VERSION.to_string(),
            capabilities: ServerCapabilities::default(),
            server_info: Implementation::new(name, "0.0.1"),
            instructions: None,
            _meta: None,
        }
    }

    /// Set the version of the server tool (not the MCP protocol version)
    ///
    /// This sets the version of your server implementation, not the Model Context Protocol version.
    /// The MCP protocol version is automatically set to the latest supported version.
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.server_info.version = version.into();
        self
    }

    /// Set the MCP protocol version
    pub fn with_mcp_version(mut self, version: impl Into<String>) -> Self {
        self.protocol_version = version.into();
        self
    }

    /// Set the instructions for the server
    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    /// Set the server capabilities
    pub fn with_capabilities(mut self, capabilities: ServerCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// Enable logging capability
    pub fn with_logging(mut self) -> Self {
        self.capabilities.logging = Some(serde_json::Value::Object(serde_json::Map::new()));
        self
    }

    /// Enable prompts capability
    pub fn with_prompts(mut self, list_changed: bool) -> Self {
        self.capabilities.prompts = Some(PromptsCapability {
            list_changed: Some(list_changed),
        });
        self
    }

    /// Enable resources capability
    pub fn with_resources(mut self, subscribe: bool, list_changed: bool) -> Self {
        self.capabilities.resources = Some(ResourcesCapability {
            subscribe: Some(subscribe),
            list_changed: Some(list_changed),
        });
        self
    }

    /// Enable tools capability
    pub fn with_tools(mut self, list_changed: bool) -> Self {
        self.capabilities.tools = Some(ToolsCapability {
            list_changed: Some(list_changed),
        });
        self
    }

    /// Enable completions capability
    pub fn with_completions(mut self) -> Self {
        self.capabilities.completions = Some(serde_json::Value::Object(serde_json::Map::new()));
        self
    }
}
