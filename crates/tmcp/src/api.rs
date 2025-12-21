use async_trait::async_trait;

use crate::{
    error::Result,
    schema::{
        ArgumentInfo, CallToolResult, ClientCapabilities, CompleteResult, CreateMessageParams,
        CreateMessageResult, Cursor, ElicitParams, ElicitResult, GetPromptResult, Implementation,
        InitializeResult, ListPromptsResult, ListResourceTemplatesResult, ListResourcesResult,
        ListRootsResult, ListToolsResult, LoggingLevel, ReadResourceResult, Reference,
    },
};

/// ServerAPI holds all server methods defined by the MCP specification. These methods are exposed
/// by the server and called by the client.
#[async_trait]
pub trait ServerAPI: Send + Sync {
    /// Initialize the connection with protocol version and capabilities
    async fn initialize(
        &mut self,
        protocol_version: String,
        capabilities: ClientCapabilities,
        client_info: Implementation,
    ) -> Result<InitializeResult>;

    /// Respond to ping requests
    async fn ping(&mut self) -> Result<()>;

    /// List available tools with optional pagination
    async fn list_tools(
        &mut self,
        cursor: impl Into<Option<Cursor>> + Send,
    ) -> Result<ListToolsResult>;

    /// Call a tool with the given name and arguments
    async fn call_tool(
        &mut self,
        name: impl Into<String> + Send,
        arguments: Option<crate::Arguments>,
    ) -> Result<CallToolResult>;

    /// List available resources with optional pagination
    async fn list_resources(
        &mut self,
        cursor: impl Into<Option<Cursor>> + Send,
    ) -> Result<ListResourcesResult>;

    /// List resource templates with optional pagination
    async fn list_resource_templates(
        &mut self,
        cursor: impl Into<Option<Cursor>> + Send,
    ) -> Result<ListResourceTemplatesResult>;

    /// Read a resource by URI
    async fn resources_read(&mut self, uri: impl Into<String> + Send)
    -> Result<ReadResourceResult>;

    /// Subscribe to resource updates
    async fn resources_subscribe(&mut self, uri: impl Into<String> + Send) -> Result<()>;

    /// Unsubscribe from resource updates
    async fn resources_unsubscribe(&mut self, uri: impl Into<String> + Send) -> Result<()>;

    /// List available prompts with optional pagination
    async fn list_prompts(
        &mut self,
        cursor: impl Into<Option<Cursor>> + Send,
    ) -> Result<ListPromptsResult>;

    /// Get a prompt by name with optional arguments
    async fn get_prompt(
        &mut self,
        name: impl Into<String> + Send,
        arguments: Option<crate::Arguments>,
    ) -> Result<GetPromptResult>;

    /// Handle completion requests
    async fn complete(
        &mut self,
        reference: Reference,
        argument: ArgumentInfo,
    ) -> Result<CompleteResult>;

    /// Set the logging level
    async fn set_level(&mut self, level: LoggingLevel) -> Result<()>;
}

/// ClientAPI holds all client methods defined by the MCP specification. These methods are exposed
/// by the client and called by the server.
#[async_trait]
pub trait ClientAPI: Send + Sync {
    /// Respond to ping requests from the server
    async fn ping(&mut self) -> Result<()>;

    /// Handle LLM sampling requests from the server
    async fn create_message(&mut self, params: CreateMessageParams) -> Result<CreateMessageResult>;

    /// List available filesystem roots
    async fn list_roots(&mut self) -> Result<ListRootsResult>;

    /// Handle elicitation requests from the server
    async fn elicit(&mut self, params: ElicitParams) -> Result<ElicitResult>;
}
