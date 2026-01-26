use std::collections::HashMap;

use async_trait::async_trait;

use crate::{
    Error, Result,
    context::{ClientCtx, ServerCtx},
    schema::{
        self, Cursor, ElicitRequestParams, ElicitResult, GetPromptResult, InitializeResult,
        ListPromptsResult, ListResourceTemplatesResult, ListResourcesResult, ListRootsResult,
        ListTasksResult, ListToolsResult, LoggingLevel, ReadResourceResult,
    },
};

/// Handler trait that client implementers must implement.
///
/// Each client connection will have its own instance of the implementation.
/// All methods take `&self` to allow concurrent request handling.
/// Implementations should use interior mutability (`Arc<Mutex<_>>`, `RwLock<_>`, etc.)
/// for any mutable state.
#[async_trait]
pub trait ClientHandler: Send + Sync {
    /// Called after the initialization handshake completes.
    async fn on_connect(&self, _context: &ClientCtx) -> Result<()> {
        Ok(())
    }

    /// Called when the connection is being closed
    async fn on_shutdown(&self, _context: &ClientCtx) -> Result<()> {
        Ok(())
    }

    /// Respond to a ping request from the server
    async fn pong(&self, _context: &ClientCtx) -> Result<()> {
        Ok(())
    }

    /// Request the server to create a model message.
    async fn create_message(
        &self,
        _context: &ClientCtx,
        _method: &str,
        _params: schema::CreateMessageParams,
    ) -> Result<schema::CreateMessageResult> {
        Err(Error::InvalidRequest(
            "create_message not implemented".into(),
        ))
    }

    /// Request the server to list roots.
    async fn list_roots(&self, _context: &ClientCtx) -> Result<schema::ListRootsResult> {
        Err(Error::InvalidRequest("list_roots not implemented".into()))
    }

    /// Request the server to elicit user input.
    async fn elicit(
        &self,
        _context: &ClientCtx,
        _params: ElicitRequestParams,
    ) -> Result<ElicitResult> {
        Err(Error::InvalidRequest("elicit not implemented".into()))
    }

    /// Retrieve the state of a task.
    async fn get_task(
        &self,
        _context: &ClientCtx,
        _task_id: String,
    ) -> Result<schema::GetTaskResult> {
        Err(Error::InvalidRequest("get_task not implemented".into()))
    }

    /// Retrieve the result of a completed task.
    async fn get_task_payload(
        &self,
        _context: &ClientCtx,
        _task_id: String,
    ) -> Result<schema::GetTaskPayloadResult> {
        Err(Error::InvalidRequest(
            "get_task_payload not implemented".into(),
        ))
    }

    /// List tasks.
    async fn list_tasks(
        &self,
        _context: &ClientCtx,
        _cursor: Option<Cursor>,
    ) -> Result<ListTasksResult> {
        Ok(ListTasksResult::default())
    }

    /// Cancel a task by ID.
    async fn cancel_task(
        &self,
        _context: &ClientCtx,
        _task_id: String,
    ) -> Result<schema::CancelTaskResult> {
        Err(Error::InvalidRequest("cancel_task not implemented".into()))
    }

    /// Handle a notification sent from the server
    ///
    /// The default implementation ignores the notification. Implementations
    /// can override this method to react to server-initiated notifications.
    async fn notification(
        &self,
        _context: &ClientCtx,
        _notification: schema::ServerNotification,
    ) -> Result<()> {
        Ok(())
    }
}

/// Handler trait for implementing MCP servers.
///
/// This trait defines how a server responds to client requests. Each client connection
/// gets its own handler instance, created by the factory function passed to [`crate::Server::new`].
///
/// # Default Behavior Philosophy
///
/// Methods follow a consistent default behavior pattern based on their purpose:
///
/// **Listing methods** return empty results by default. This means a minimal server
/// implementation automatically advertises no capabilities until you override these:
/// - [`list_tools`](Self::list_tools) → empty tool list
/// - [`list_resources`](Self::list_resources) → empty resource list
/// - [`list_prompts`](Self::list_prompts) → empty prompt list
///
/// **Dispatch methods** return errors by default. If a client requests something you
/// haven't implemented, they get a clear error rather than silent failure:
/// - [`call_tool`](Self::call_tool) → `Error::ToolNotFound`
/// - [`read_resource`](Self::read_resource) → `Error::ResourceNotFound`
/// - [`get_prompt`](Self::get_prompt) → handler error
///
/// **Lifecycle methods** are no-ops by default, allowing you to hook into events
/// only when needed:
/// - [`on_connect`](Self::on_connect), [`on_shutdown`](Self::on_shutdown)
/// - [`notification`](Self::notification)
///
/// # Concurrency
///
/// All methods take `&self` to allow concurrent request handling from the same client.
/// For mutable state, use interior mutability patterns like `Arc<Mutex<_>>` or `RwLock<_>`.
///
/// # Choosing Between Trait and Macro
///
/// For simple servers with just tools, consider the [`#[mcp_server]`](crate::mcp_server)
/// macro which generates the trait implementation automatically. Use this trait directly
/// when you need:
/// - Custom [`initialize`](Self::initialize) logic (capability negotiation, client validation)
/// - Per-connection state beyond what the macro provides
/// - Resources, prompts, or other MCP features beyond tools
/// - Fine-grained control over error handling
#[async_trait]
pub trait ServerHandler: Send + Sync {
    /// Called after the client has completed the initialize handshake.
    ///
    /// # Arguments
    /// * `context` - The server context
    /// * `remote_addr` - The remote address ("stdio" for stdio connections)
    async fn on_connect(&self, _context: &ServerCtx, _remote_addr: &str) -> Result<()> {
        Ok(())
    }

    /// Called when the server is shutting down
    async fn on_shutdown(&self) -> Result<()> {
        Ok(())
    }

    /// Handle initialize request
    async fn initialize(
        &self,
        _context: &ServerCtx,
        _protocol_version: String,
        _capabilities: schema::ClientCapabilities,
        _client_info: schema::Implementation,
    ) -> Result<InitializeResult>;

    /// Respond to a ping request from the client
    async fn pong(&self, _context: &ServerCtx) -> Result<()> {
        Ok(())
    }

    /// List available tools
    async fn list_tools(
        &self,
        _context: &ServerCtx,
        _cursor: Option<Cursor>,
    ) -> Result<ListToolsResult> {
        Ok(ListToolsResult::default())
    }

    /// Call a tool
    async fn call_tool(
        &self,
        _context: &ServerCtx,
        name: String,
        _arguments: Option<crate::Arguments>,
        _task: Option<schema::TaskMetadata>,
    ) -> Result<schema::CallToolResult> {
        Err(Error::ToolNotFound(name))
    }

    /// List available resources
    async fn list_resources(
        &self,
        _context: &ServerCtx,
        _cursor: Option<Cursor>,
    ) -> Result<ListResourcesResult> {
        Ok(ListResourcesResult::new())
    }

    /// List resource templates
    async fn list_resource_templates(
        &self,
        _context: &ServerCtx,
        _cursor: Option<Cursor>,
    ) -> Result<ListResourceTemplatesResult> {
        Ok(ListResourceTemplatesResult {
            resource_templates: vec![],
            next_cursor: None,
        })
    }

    /// Read a resource
    async fn read_resource(&self, _context: &ServerCtx, uri: String) -> Result<ReadResourceResult> {
        Err(Error::ResourceNotFound { uri })
    }

    /// Subscribe to resource updates
    async fn resources_subscribe(&self, _context: &ServerCtx, _uri: String) -> Result<()> {
        Ok(())
    }

    /// Unsubscribe from resource updates
    async fn resources_unsubscribe(&self, _context: &ServerCtx, _uri: String) -> Result<()> {
        Ok(())
    }

    /// List available prompts
    async fn list_prompts(
        &self,
        _context: &ServerCtx,
        _cursor: Option<Cursor>,
    ) -> Result<ListPromptsResult> {
        Ok(ListPromptsResult::new())
    }

    /// Get a prompt
    async fn get_prompt(
        &self,
        _context: &ServerCtx,
        name: String,
        _arguments: Option<HashMap<String, String>>,
    ) -> Result<GetPromptResult> {
        Err(Error::PromptNotFound(name))
    }

    /// Handle completion request
    async fn complete(
        &self,
        _context: &ServerCtx,
        _reference: schema::Reference,
        _argument: schema::ArgumentInfo,
        _context_info: Option<schema::CompleteContext>,
    ) -> Result<schema::CompleteResult> {
        Ok(schema::CompleteResult {
            completion: schema::CompletionInfo {
                values: vec![],
                total: None,
                has_more: None,
            },
            _meta: None,
        })
    }

    /// Set logging level
    async fn set_level(&self, _context: &ServerCtx, _level: LoggingLevel) -> Result<()> {
        Ok(())
    }

    /// List roots (for server-initiated roots/list request)
    async fn list_roots(&self, _context: &ServerCtx) -> Result<ListRootsResult> {
        Ok(ListRootsResult {
            roots: vec![],
            _meta: None,
        })
    }

    /// Handle sampling/createMessage request from server
    async fn create_message(
        &self,
        _context: &ServerCtx,
        _params: schema::CreateMessageParams,
    ) -> Result<schema::CreateMessageResult> {
        Err(Error::MethodNotFound("sampling/createMessage".to_string()))
    }

    /// Retrieve the state of a task.
    async fn get_task(
        &self,
        _context: &ServerCtx,
        _task_id: String,
    ) -> Result<schema::GetTaskResult> {
        Err(Error::InvalidRequest("get_task not implemented".into()))
    }

    /// Retrieve the result of a completed task.
    async fn get_task_payload(
        &self,
        _context: &ServerCtx,
        _task_id: String,
    ) -> Result<schema::GetTaskPayloadResult> {
        Err(Error::InvalidRequest(
            "get_task_payload not implemented".into(),
        ))
    }

    /// List tasks (for server-initiated tasks/list request)
    async fn list_tasks(
        &self,
        _context: &ServerCtx,
        _cursor: Option<Cursor>,
    ) -> Result<ListTasksResult> {
        Ok(ListTasksResult::default())
    }

    /// Cancel a task by ID.
    async fn cancel_task(
        &self,
        _context: &ServerCtx,
        _task_id: String,
    ) -> Result<schema::CancelTaskResult> {
        Err(Error::InvalidRequest("cancel_task not implemented".into()))
    }

    /// Handle a notification sent from the client
    ///
    /// The default implementation ignores the notification. Servers can
    /// override this to react to client-initiated notifications such as
    /// progress updates or cancellations.
    async fn notification(
        &self,
        _context: &ServerCtx,
        _notification: schema::ClientNotification,
    ) -> Result<()> {
        Ok(())
    }
}
